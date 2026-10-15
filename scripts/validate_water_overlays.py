import argparse
import json
import struct
from pathlib import Path

import validate_native_runtime as runtime
from validate_authored_planes import records
from validate_compile_jobs import wait_job
from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_overlays import capture_report
from validate_visibility import ROOM


def inspect(path, stats):
    data = path.read_bytes()
    rows = records(data, 50, "<ihH256i22f")
    faces = records(data, 7, "<HbBihhhh4Bif2i2iiHHI")
    planes = records(data, 1, "<4fi")
    assert len(rows) == stats["overlays"] == 1
    assert not records(data, 45, "<ihH64i22f") and not records(data, 60, "<2f")
    for row in rows:
        count = row[2] & 0x3FFF
        assert 0 < count <= 256 and row[2] >> 14 == 0
        assert count == stats["overlay_face_refs"]
        for face in row[3 : 3 + count]:
            assert 0 <= face < len(faces)
            assert planes[faces[face][0]][:4] == (0, 0, 1, 0)
    offset, length, _, _ = struct.unpack_from("<4i", data, 8)
    entities = data[offset : offset + length].decode()
    assert (
        "info_overlay_transition" not in entities and "water_transition" not in entities
    )
    assert "info_overlay_accessor" not in entities
    return {
        "records": len(rows),
        "face_references": stats["overlay_face_refs"],
        "stride": struct.calcsize("<ihH256i22f"),
    }


def fixture(args):
    client = Client(None, args.binary, args.output)
    try:
        brushes = []
        for low, high in ROOM:
            brushes.append(
                client.call(
                    "create_block",
                    {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"},
                )["id"]
            )
        floor = detail(client, brushes[:1])[0]
        side = next(
            face["side"] for face in floor["faces"] if face["normal"] == [0, 0, 1]
        )
        for classname, origin, properties in [
            ("info_player_terrorist", [-64, -64, 8], {}),
            ("info_player_counterterrorist", [128, 64, 8], {}),
            (
                "light",
                [0, 0, 180],
                {
                    "_light": "255 255 255 200",
                    "_constant_attn": "1",
                    "_quadratic_attn": "0",
                },
            ),
            (
                "logic_script",
                [0, 0, 64],
                {
                    "targetname": "hammerr_observer",
                    "vscripts": "hammerr_observer.nut",
                    "thinkfunction": "HammeRRThink",
                },
            ),
            (
                "prop_physics",
                [160, 128, 128],
                {
                    "targetname": "physics_crate",
                    "model": "models/props_junk/wood_crate001a.mdl",
                },
            ),
        ]:
            client.call(
                "create_entity",
                {"classname": classname, "origin": origin, "properties": properties},
            )
        helper = client.call(
            "create_entity",
            {
                "classname": "info_overlay_transition",
                "origin": [1000, 0, 0],
                "properties": {"targetname": "water_transition"},
            },
        )["id"]
        client.call(
            "set_compile_settings", {"lighting": "direct_ldr", "physics": "engine"}
        )
        source = args.output / "water-overlays.vmf"
        client.call("save_map", {"path": str(source)})
        fields = {
            "material": "decals/decalgraffiti001a",
            "sides": str(side),
            "BasisOrigin": "0 0 0",
            "BasisU": "1 0 0",
            "BasisV": "0 1 0",
            "BasisNormal": "0 0 1",
            "StartU": "0",
            "EndU": "1",
            "StartV": "0",
            "EndV": "1",
            "uv0": "-192 -192 0",
            "uv1": "-192 192 0",
            "uv2": "192 192 0",
            "uv3": "192 -192 0",
        }
        marker = f'entity\n{{\n\t"id" "{helper.split(":")[1]}"'
        original = source.read_text()
        assert original.count(marker) == 1
        projection = "\n".join(f'"{key}" "{value}"' for key, value in fields.items())
        block = "\noverlaytransition\n{\noverlaydata\n{\n" + projection + "\n}\n}\n"
        source.write_text(original.replace(marker, marker + block, 1))
        client.call("open_map", {"path": str(source), "discard_changes": True})
        before = (
            source.read_bytes(),
            detail(client, [helper]),
            client.call("get_map_info")["undo"],
        )
        path = args.output / "visible.bsp"
        compiled = client.call("compile_map", {"path": str(path)})
        assert compiled["success"], compiled
        native = inspect(path, compiled["stats"])
        published = path.read_bytes()
        background = args.output / "background.bsp"
        client.call("start_compile", {"path": str(background)})
        job = wait_job(client)
        assert job["status"] == "completed" and background.read_bytes() == published, (
            job
        )
        assert (
            source.read_bytes(),
            detail(client, [helper]),
            client.call("get_map_info")["undo"],
        ) == before
        bad = args.output / "malformed.vmf"
        text = source.read_text()
        target = f'"sides" "{side}"'
        assert text.count(target) == 1
        bad.write_text(text.replace(target, '"sides" "4294967295"', 1))
        client.call("open_map", {"path": str(bad), "discard_changes": True})
        client.call("compile_map", {"path": str(path)}, error=True)
        assert path.read_bytes() == published
        client.call("start_compile", {"path": str(path)})
        assert wait_job(client)["status"] == "failed" and path.read_bytes() == published
        client.call("open_map", {"path": str(source), "discard_changes": True})
        client.call("set_object_visibility", {"action": "hide", "ids": [helper]})
        baseline = args.output / "baseline.bsp"
        hidden = client.call("compile_map", {"path": str(baseline)})
        assert hidden["stats"]["overlays"] == 0 and not records(
            baseline.read_bytes(), 50, "<ihH256i22f"
        )
        report = {
            "stats": compiled["stats"],
            "native": native,
            "source_and_history_preserved": True,
            "shared_builds_identical": True,
            "failures_preserve_output": True,
            "hidden_helper_omitted": True,
            "runtime_checked": False,
        }
        (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
        return [("baseline", baseline), ("visible", path)]
    finally:
        client.close()


def worker(args):
    args.capture_ppm = True
    runtime.OBSERVER = runtime.OBSERVER.replace(
        "setang 25 45 0; mat_fullbright",
        "setang 25 45 0; r_drawothermodels 0; mat_fullbright",
    )
    runtime.worker(args)
    console = (args.output / "cstrike/hammerr-runtime.log").read_text(errors="replace")
    assert '"sv_lan" = "1"' in console
    report = capture_report(args.output)
    (args.output / "render.json").write_text(json.dumps(report, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser(
        description="Validate imported water transition blocks; optionally compare fresh private CS:S loads"
    )
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--runtime", action="store_true")
    parser.add_argument(
        "--game-dir",
        type=Path,
        default=Path.home()
        / ".local/share/Steam/steamapps/common/Counter-Strike Source",
    )
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--bsp", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    args.output = args.output.resolve()
    args.binary = args.binary.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    if args.worker:
        worker(args)
        return
    cases = fixture(args)
    report = json.loads((args.output / "results.json").read_text())
    if args.runtime:
        captures = {}
        for name, bsp in cases:
            output = runtime.private_display(
                Path(__file__), args, bsp, output=args.output / name
            )
            captures[name] = json.loads((output / "render.json").read_text())
        assert (
            captures["visible"]["dark_floor_pixels"]
            > captures["baseline"]["dark_floor_pixels"] + 1000
        ), captures
        difference = runtime.compare_frames(args.output, ("baseline", "visible"))
        assert difference["changed_pixels"] > 1000, difference
        report.update(
            {
                "runtime_checked": True,
                "captures": captures,
                "frame_difference": difference,
            }
        )
        (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    print(
        json.dumps(
            {
                "output": str(args.output),
                "water_overlays": True,
                "runtime_checked": args.runtime,
            }
        )
    )


if __name__ == "__main__":
    main()
