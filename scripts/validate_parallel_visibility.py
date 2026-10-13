import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

import validate_native_runtime as runtime
from validate_compile_jobs import wait_job
from validate_face_lighting import ROOT, Client, detail
from validate_visibility import ROOM, bit, visibility


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
        helpers = []
        for axis in [0, 1]:
            for index in range(1, args.grid):
                coordinate = -256 + 512 * index / args.grid
                low, high = [-264, -264, 0], [264, 264, 256]
                low[axis], high[axis] = coordinate - 8, coordinate
                helper = client.call(
                    "create_block",
                    {"mins": low, "maxs": high, "material": "tools/toolsskip"},
                )["id"]
                shape = detail(client, [helper])[0]
                normal = [int(component == axis) for component in range(3)]
                side = next(
                    face["side"] for face in shape["faces"] if face["normal"] == normal
                )
                client.call(
                    "select_faces", {"faces": [{"solid": shape["solid"], "side": side}]}
                )
                client.call("edit_faces", {"material": "tools/toolshint"})
                helpers.append(helper)
        source = args.output / "grid.vmf"
        client.call("save_map", {"path": str(source)})
        before = detail(client, helpers)
        history = client.call("get_map_info")["undo"]
        source_bytes = source.read_bytes()
        client.call(
            "set_compile_settings",
            {"visibility": "detailed", "lighting": "direct_ldr", "physics": "engine"},
        )
        path = args.output / "grid.bsp"
        result = client.call("compile_map", {"path": str(path)})
        data = path.read_bytes()
        count, size, rows = visibility(data)
        stats = result["stats"]
        assert (
            result["success"]
            and count == args.grid**2
            and stats["visibility_bytes"] == size
        )
        assert (
            stats["hint_surfaces"] == 2 * (args.grid - 1)
            and stats["hint_splits"] == count - 1
        )
        assert (
            1 <= stats["visibility_workers"] <= 8
            and stats["visibility_work"] <= 100_000_000
        )
        assert all(
            bit(pair[0], target) and bit(pair[1], target)
            for pair in rows
            for target in range(count)
        )
        background = args.output / "background.bsp"
        client.call("start_compile", {"path": str(background)})
        job = wait_job(client)
        assert job["status"] == "completed" and background.read_bytes() == data
        assert job["result"]["stats"]["visibility_work"] == stats["visibility_work"]
        assert (
            before == detail(client, helpers)
            and history == client.call("get_map_info")["undo"]
            and source.read_bytes() == source_bytes
        )
        (args.output / "results.json").write_text(
            json.dumps(
                {
                    "grid": args.grid,
                    "stats": stats,
                    "all_cells_visible_and_audible": True,
                    "exact_both_builds": True,
                    "deterministic_work": True,
                    "source_and_history_preserved": True,
                },
                indent=2,
            )
            + "\n"
        )
        return path
    finally:
        client.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-parallel-visibility")
    )
    parser.add_argument("--grid", type=int, default=10)
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
    if not 4 <= args.grid <= 10:
        parser.error("--grid must be between 4 and 10")
    args.output.mkdir(parents=True, exist_ok=True)
    if args.worker:
        runtime.worker(args)
        return
    bsp = fixture(args)
    if args.runtime:
        output = args.output / "runtime"
        output.mkdir(exist_ok=True)
        env = {
            **os.environ,
            "WLR_BACKENDS": "headless",
            "WLR_HEADLESS_OUTPUTS": "1",
            "WLR_RENDERER": "pixman",
        }
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
    print(json.dumps({"output": str(args.output), "parallel_visibility": True}))


if __name__ == "__main__":
    main()
