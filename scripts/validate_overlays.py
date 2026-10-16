import argparse
import json
import re
import struct
import sys
from pathlib import Path

import validate_native_runtime as runtime
from validate_authored_planes import records
from validate_compile_jobs import wait_job
from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_visibility import ROOM


def overlays(data, stats):
    rows = records(data, 45, "<ihH64i22f")
    fades = records(data, 60, "<2f")
    faces = records(data, 7, "<HbBihhhh4Bif2i2iiHHI")
    planes = records(data, 1, "<4fi")
    assert len(rows) == stats["overlays"] == len(fades)
    assert sum(row[2] & 0x3FFF for row in rows) == stats["overlay_face_refs"]
    for row in rows:
        assert 0 < row[2] & 0x3FFF <= 64
        for face in row[3 : 3 + (row[2] & 0x3FFF)]:
            assert planes[faces[face][0]][:4] == (0, 0, 1, 0)
    return rows, fades


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
                "classname": "info_overlay",
                "origin": [1000, 0, 0],
                "properties": {
                    "targetname": "floor_marking",
                    "material": "decals/decalgraffiti001a",
                    "sides": str(side),
                    "RenderOrder": "3",
                },
            },
        )["id"]
        client.call(
            "set_compile_settings", {"lighting": "direct_ldr", "physics": "engine"}
        )
        source = args.output / "overlays.vmf"
        client.call("save_map", {"path": str(source)})
        fields = {
            "BasisOrigin": "0 0 0",
            "BasisU": "1 0 0",
            "BasisV": "0 1 0",
            "BasisNormal": "0 0 1",
            "uv0": "-192 -192 0",
            "uv1": "-192 192 0",
            "uv2": "192 192 0",
            "uv3": "192 -192 0",
        }
        marker = f'entity\n{{\n\t"id" "{helper.split(":")[1]}"'
        text = source.read_text()
        assert text.count(marker) == 1
        projection = "\n".join(f'\t"{key}" "{value}"' for key, value in fields.items())
        source.write_text(text.replace(marker, marker + "\n" + projection, 1))
        client.call("open_map", {"path": str(source), "discard_changes": True})
        source_bytes = source.read_bytes()
        client.call(
            "set_entity_properties",
            {"id": helper, "set": {"BasisU": "1 0 0"}},
            error=True,
        )
        before = detail(client, [helper])
        history = client.call("get_map_info")["undo"]
        path = args.output / "overlays.bsp"
        built = client.call("compile_map", {"path": str(path)})
        data = path.read_bytes()
        rows, fades = overlays(data, built["stats"])
        assert rows[0][2] >> 14 == 3 and fades == [(-1, 0)]
        offset, size, _, _ = struct.unpack_from("<4i", data, 8)
        entities = data[offset : offset + size].decode().rstrip("\0")
        assert (
            '"classname" "info_overlay_accessor"' in entities
            and '"OverlayID" "0"' in entities
        )
        background = args.output / "background.bsp"
        client.call("start_compile", {"path": str(background)})
        job = wait_job(client)
        assert job["status"] == "completed" and background.read_bytes() == data
        assert (
            detail(client, [helper]) == before
            and client.call("get_map_info")["undo"] == history
        )
        assert source.read_bytes() == source_bytes
        client.call(
            "set_entity_properties",
            {"id": helper, "set": {"fademindist": "16", "fademaxdist": "32"}},
        )
        faded = args.output / "faded.bsp"
        result = client.call("compile_map", {"path": str(faded)})
        assert overlays(faded.read_bytes(), result["stats"])[1] == [(256, 1024)]
        client.call(
            "set_entity_properties", {"id": helper, "set": {"material": "../invalid"}}
        )
        failed = client.call("compile_map", {"path": str(path)}, error=True)
        assert path.read_bytes() == data and "Overlay material" in json.dumps(failed)
        client.call("start_compile", {"path": str(path)})
        assert wait_job(client)["status"] == "failed" and path.read_bytes() == data
        client.call("open_map", {"path": str(source), "discard_changes": True})
        client.call("set_object_visibility", {"action": "hide", "ids": [helper]})
        baseline = args.output / "baseline.bsp"
        result = client.call("compile_map", {"path": str(baseline)})
        assert result["stats"]["overlays"] == 0 and not records(
            baseline.read_bytes(), 45, "<ihH64i22f"
        )
        (args.output / "results.json").write_text(
            json.dumps(
                {
                    "stats": built["stats"],
                    "shared_builds_identical": True,
                    "source_and_history_preserved": True,
                    "failures_preserve_output": True,
                    "hidden_overlay_omitted": True,
                },
                indent=2,
            )
            + "\n"
        )
        return [("baseline", baseline), ("visible", path), ("faded", faded)]
    finally:
        client.close()


def worker(args):
    args.capture_ppm = True
    runtime.OBSERVER = runtime.OBSERVER.replace(
        "setang 25 45 0; mat_fullbright",
        "setang 25 45 0; r_drawothermodels 0; mat_fullbright",
    )
    runtime.OBSERVER = runtime.OBSERVER.replace(
        '    local player = Entities.FindByClassname(null, "player");',
        """
    local overlay = Entities.FindByName(null, "floor_marking");
    if (overlay != null && NetProps.HasProp(overlay, "m_iOverlayID"))
        printl("HAMMERR_OVERLAY_ID " + NetProps.GetPropInt(overlay, "m_iOverlayID"));
    local player = Entities.FindByClassname(null, "player");""",
    )
    runtime.worker(args)
    report = capture_report(args.output)
    (args.output / "render.json").write_text(json.dumps(report, indent=2) + "\n")


def capture_report(output):
    magic, dimensions, maximum, pixels = (
        (output / "game.ppm").read_bytes().split(b"\n", 3)
    )
    width, height = map(int, dimensions.split())
    assert magic == b"P6" and maximum == b"255" and len(pixels) == width * height * 3
    lower = pixels[width * (height // 2) * 3 :]
    dark = sum(max(pixel) < 100 for pixel in struct.iter_unpack("3B", lower))
    text = (output / "cstrike/hammerr-runtime.log").read_text(errors="replace")
    ids = list(map(int, re.findall(r"HAMMERR_OVERLAY_ID (\d+)", text)))
    return {"dark_floor_pixels": dark, "accessor_ids": ids}


def check_rendering(reports):
    assert (
        reports["visible"]["dark_floor_pixels"]
        > reports["baseline"]["dark_floor_pixels"] + 1000
    ), reports
    assert (
        reports["faded"]["dark_floor_pixels"]
        <= reports["baseline"]["dark_floor_pixels"] + 100
    ), reports
    assert reports["visible"]["accessor_ids"] and set(
        reports["visible"]["accessor_ids"]
    ) == {0}, reports
    assert not reports["baseline"]["accessor_ids"], reports


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, default=Path("/tmp/hammerr-overlays"))
    parser.add_argument("--runtime", action="store_true")
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--bsp", type=Path, help=argparse.SUPPRESS)
    parser.add_argument(
        "--game-dir",
        type=Path,
        default=Path.home()
        / ".local/share/Steam/steamapps/common/Counter-Strike Source",
    )
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    if args.worker:
        worker(args)
        return
    cases = fixture(args)
    if args.runtime:
        reports = {}
        for name, bsp in cases:
            output = args.output / name
            output.mkdir(exist_ok=True)
            runtime.private_compositor(
                [
                    sys.executable,
                    "-B",
                    str(Path(__file__).resolve()),
                    "--worker",
                    "--bsp",
                    str(bsp),
                    "--output",
                    str(output),
                    "--game-dir",
                    str(args.game_dir),
                ],
                output,
                timeout=55,
            )
            reports[name] = json.loads((output / "render.json").read_text())
        check_rendering(reports)
        (args.output / "render-results.json").write_text(
            json.dumps(reports, indent=2) + "\n"
        )
    print(json.dumps({"output": str(args.output), "overlays": True}))


if __name__ == "__main__":
    main()
