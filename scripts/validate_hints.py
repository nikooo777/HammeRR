import argparse
import json
import os
import struct
import subprocess
import sys
from pathlib import Path

import validate_native_runtime as runtime
from validate_compile_jobs import wait_job
from validate_face_lighting import ROOT, Client, detail
from validate_portal_flow import compare_captures
from validate_visibility import ROOM, bit, cluster_at, visibility


def payload(data, slot):
    offset, size, _, _ = struct.unpack_from("<4i", data, 8 + slot * 16)
    return data[offset : offset + size]


def fixture(args):
    client = Client(None, args.binary, args.output)
    try:
        for low, high in ROOM:
            client.call(
                "create_block",
                {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"},
            )
        for classname, origin, properties in [
            ("info_player_terrorist", [-64, -64, 8], {}),
            ("info_player_counterterrorist", [64, 64, 8], {}),
            (
                "light",
                [0, 0, 128],
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
                [128, 128, 128],
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
            "create_block",
            {"mins": [-8, -96, 0], "maxs": [8, 96, 192], "material": "tools/toolsskip"},
        )["id"]
        shape = detail(client, [helper])[0]
        side = next(
            face["side"] for face in shape["faces"] if face["normal"] == [1, 0, 0]
        )
        client.call(
            "select_faces", {"faces": [{"solid": shape["solid"], "side": side}]}
        )
        client.call("edit_faces", {"material": "tools/toolshint"})
        client.call(
            "set_compile_settings", {"lighting": "direct_ldr", "physics": "engine"}
        )
        source = args.output / "hints.vmf"
        client.call("save_map", {"path": str(source)})
        before = detail(client, [helper])
        history = client.call("get_map_info")["undo"]
        source_bytes = source.read_bytes()
        metrics = []
        for mode in ["fast", "detailed"]:
            client.call("set_compile_settings", {"visibility": mode})
            path = args.output / f"hints-{mode}.bsp"
            result = client.call("compile_map", {"path": str(path)})
            data = path.read_bytes()
            count, _, rows = visibility(data)
            assert (
                result["success"]
                and result["stats"]["hint_surfaces"] == 1
                and result["stats"]["hint_splits"] == 1
            )
            assert count == 2 and result["stats"]["brushes"] == 6
            a, b = [cluster_at(data, point) for point in [[0, 0, 32], [16, 0, 32]]]
            assert a != b and bit(rows[a][0], b) and bit(rows[b][0], a)
            background = args.output / f"background-{mode}.bsp"
            client.call("start_compile", {"path": str(background)})
            assert (
                wait_job(client)["status"] == "completed"
                and background.read_bytes() == data
            )
            assert (
                detail(client, [helper]) == before
                and client.call("get_map_info")["undo"] == history
                and source.read_bytes() == source_bytes
            )
            metrics.append({"mode": mode, "stats": result["stats"]})
        client.render(
            args.output / "viewport.png",
            width=1000,
            height=700,
            camera={"position": [-180, -180, 160], "look_at": [8, 0, 96]},
        )
        original = (args.output / "hints-detailed.bsp").read_bytes()
        client.call("set_object_visibility", {"action": "hide", "ids": [helper]})
        baseline = args.output / "baseline.bsp"
        result = client.call("compile_map", {"path": str(baseline)})
        assert result["stats"]["hint_surfaces"] == result["stats"]["hint_splits"] == 0
        assert result["stats"]["clusters"] == 1
        assert payload(baseline.read_bytes(), 29) == payload(original, 29)
        client.call("open_map", {"path": str(source), "discard_changes": True})
        bad = next(
            face["side"] for face in shape["faces"] if face["normal"] == [-1, 0, 0]
        )
        client.call("select_faces", {"faces": [{"solid": shape["solid"], "side": bad}]})
        client.call("edit_faces", {"material": "dev/dev_measuregeneric01"})
        failed = client.call(
            "compile_map", {"path": str(args.output / "hints-detailed.bsp")}, error=True
        )
        assert (
            "hint/skip materials" in json.dumps(failed)
            and (args.output / "hints-detailed.bsp").read_bytes() == original
        )
        client.call("start_compile", {"path": str(args.output / "hints-detailed.bsp")})
        assert (
            wait_job(client)["status"] == "failed"
            and (args.output / "hints-detailed.bsp").read_bytes() == original
        )
        (args.output / "results.json").write_text(
            json.dumps(
                {
                    "metrics": metrics,
                    "exact_both_builds": True,
                    "source_and_history_preserved": True,
                    "physics_bytes_unchanged": True,
                    "failures_preserve_output": True,
                },
                indent=2,
            )
            + "\n"
        )
        return [("baseline", baseline), ("hint", args.output / "hints-detailed.bsp")]
    finally:
        client.close()


def worker(args):
    args.capture_ppm = True
    runtime.OBSERVER = runtime.OBSERVER.replace(
        "setpos -64 -64 32; setang 25 45 0; mat_fullbright",
        "setpos 0 0 32; setang 20 0 0; r_drawothermodels 0; r_drawviewmodel 0; mat_fullbright",
    )
    runtime.OBSERVER = runtime.OBSERVER.replace(
        "        next_report = Time() + 3.0;",
        """
        else if (stage == 2) {
            SendToConsole("setpos 16 0 32");
            stage = 3;
        }
        next_report = Time() + 3.0;""",
    )
    runtime.worker(args)
    players = json.loads((args.output / "runtime.json").read_text())["players"]
    for x in [0, 16]:
        assert any(
            abs(row[2] - x) < 0.1 and abs(row[3]) < 0.1 and abs(row[4]) < 1
            for row in players
        ), (x, players)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument("--output", type=Path, default=Path("/tmp/hammerr-hints"))
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
        env = {
            **os.environ,
            "WLR_BACKENDS": "headless",
            "WLR_HEADLESS_OUTPUTS": "1",
            "WLR_RENDERER": "pixman",
        }
        for name, bsp in cases:
            output = args.output / name
            output.mkdir(exist_ok=True)
            with (output / "compositor.log").open("w") as log:
                subprocess.run(
                    [
                        "cage",
                        "--",
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
                    cwd=ROOT,
                    env=env,
                    stdout=log,
                    stderr=log,
                    check=True,
                    timeout=55,
                )
        report = compare_captures(args.output, ("baseline", "hint"))
        (args.output / "render-results.json").write_text(
            json.dumps(report, indent=2) + "\n"
        )
    print(json.dumps({"output": str(args.output), "hint_planes": True}))


if __name__ == "__main__":
    main()
