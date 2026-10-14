import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

import validate_native_runtime as runtime
from validate_authored_planes import records
from validate_compile_jobs import wait_job
from validate_face_lighting import ROOT, Client, detail, face_ref
from validate_visibility import ROOM


def fixture(args):
    client = Client(None, args.binary, args.output)
    try:
        for low, high in ROOM:
            client.call(
                "create_block",
                {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"},
            )
        parents = []
        corner_indices = []
        for index, (low, high) in enumerate(
            [([-128, -128, 0], [0, 0, 16]), ([0, 0, 0], [128, 128, 16])]
        ):
            parent = client.call(
                "create_block",
                {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"},
            )["id"]
            shape = detail(client, [parent])[0]
            target = face_ref(
                shape, next(face for face in shape["faces"] if face["normal"][2] > 0.99)
            )
            client.call("select_faces", {"faces": [target]})
            client.call(
                "create_terrain", {"power": index + 2, "start_corner": index * 3}
            )
            grid = client.call("get_terrain_grids", {})["faces"][0]
            n = grid["grid_size"]
            corners = [0, n * (n - 1), n * n - 1, n - 1]
            corner_indices.append(
                next(
                    corner
                    for corner, vertex in enumerate(corners)
                    if grid["positions"][vertex] == [0, 0, 16]
                )
            )
            parents.append(parent)
        for classname, origin, properties in [
            ("info_player_terrorist", [-192, -176, 8], {}),
            ("info_player_counterterrorist", [192, 176, 8], {}),
            (
                "light",
                [0, 0, 180],
                {"_light": "255 255 255 200", "_constant_attn": "1"},
            ),
            (
                "logic_script",
                [-192, -176, 64],
                {
                    "targetname": "hammerr_observer",
                    "vscripts": "hammerr_observer.nut",
                    "thinkfunction": "HammeRRThink",
                },
            ),
            (
                "prop_physics",
                [-32, -32, 128],
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
        source = args.output / "corners.vmf"
        client.call("save_map", {"path": str(source)})
        saved = source.read_bytes()
        history = client.call("get_map_info")["undo"]
        client.call(
            "set_compile_settings", {"lighting": "direct_ldr", "physics": "engine"}
        )
        path = args.output / "corners.bsp"
        result = client.call("compile_map", {"path": str(path)})
        assert (
            result["success"]
            and result["stats"]["displacement_corner_neighbor_pairs"] == 1
            and result["stats"]["displacement_neighbor_pairs"] == 0
        ), result
        infos = records(path.read_bytes(), 26, "<3f4ifi2H2i48s40s10I")
        import struct

        for index, info in enumerate(infos):
            lists = [
                struct.unpack_from("<4HBx", info[14], offset)
                for offset in range(0, 40, 10)
            ]
            for corner, values in enumerate(lists):
                assert values[4] == (1 if corner == corner_indices[index] else 0)
                if values[4]:
                    assert values[0] == 1 - index
        background = args.output / "background.bsp"
        client.call("start_compile", {"path": str(background)})
        assert (
            wait_job(client)["status"] == "completed"
            and path.read_bytes() == background.read_bytes()
        )
        assert (
            client.call("get_map_info")["undo"] == history
            and source.read_bytes() == saved
        )
        client.call("set_object_visibility", {"action": "hide", "ids": parents[1:]})
        hidden = client.call("compile_map", {"path": str(args.output / "hidden.bsp")})
        assert (
            hidden["success"]
            and hidden["stats"]["displacement_corner_neighbor_pairs"] == 0
        )
        client.call("set_object_visibility", {"action": "show", "ids": parents[1:]})
        (args.output / "results.json").write_text(
            json.dumps(
                {
                    "stats": result["stats"],
                    "reciprocal_indices": corner_indices,
                    "rotated_mixed_power_corners": True,
                    "exact_both_builds": True,
                    "hidden_omitted": True,
                    "source_history_preserved": True,
                },
                indent=2,
            )
            + "\n"
        )
        return path
    finally:
        client.close()


def worker(args):
    runtime.OBSERVER = runtime.OBSERVER.replace(
        "setpos -64 -64 32", "setpos -96 -96 96"
    ).replace(
        "setang 25 45 0", "setang 35 45 0; r_drawothermodels 0; r_drawviewmodel 0"
    )
    args.ground_z = 16.0
    args.ground_tolerance = 2.0
    runtime.worker(args)
    report = json.loads((args.output / "runtime.json").read_text())
    assert all(35 < point[2] < 38 for point in report["prop_positions"][-3:])


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-terrain-corners-native")
    )
    parser.add_argument(
        "--game-dir",
        type=Path,
        default=Path.home()
        / ".local/share/Steam/steamapps/common/Counter-Strike Source",
    )
    parser.add_argument("--runtime", action="store_true")
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--bsp", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    if args.worker:
        worker(args)
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
    print(json.dumps({"terrain_corners": True, "output": str(args.output)}))


if __name__ == "__main__":
    main()
