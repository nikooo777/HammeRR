import argparse
import json
import os
import re
import struct
import subprocess
import sys
from pathlib import Path

import validate_native_runtime as runtime
from validate_authored_planes import records
from validate_compile_jobs import wait_job
from validate_face_lighting import ROOT, Client, detail, face_ref
from validate_visibility import ROOM


def build(client, output, name):
    path = output / f"{name}.bsp"
    result = client.call("compile_map", {"path": str(path)})
    assert result["success"], result
    background = output / f"{name}-background.bsp"
    client.call("start_compile", {"path": str(background)})
    assert wait_job(client)["status"] == "completed"
    assert background.read_bytes() == path.read_bytes()
    return path, result["stats"]


def fixture(args):
    client = Client(None, args.binary, args.output)
    try:
        for low, high in ROOM:
            client.call(
                "create_block",
                {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"},
            )
        parents, faces = [], []
        for power, corner, low, high in [
            (2, 1, [-128, -128, 0], [0, 128, 16]),
            (4, 3, [0, -128, 0], [128, 128, 16]),
        ]:
            parent = client.call(
                "create_block",
                {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"},
            )["id"]
            shape = detail(client, [parent])[0]
            target = face_ref(
                shape, next(face for face in shape["faces"] if face["normal"][2] > 0.99)
            )
            client.call("select_faces", {"faces": [target]})
            client.call("create_terrain", {"power": power, "start_corner": corner})
            parents.append(parent)
            faces.append(target)
        for classname, origin, properties in [
            ("info_player_terrorist", [-192, -176, 8], {}),
            ("info_player_counterterrorist", [192, 176, 8], {}),
            (
                "light",
                [0, 0, 200],
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
                [96, 96, 128],
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
        client.call(
            "set_compile_settings", {"lighting": "direct_ldr", "physics": "engine"}
        )
        client.call("select_faces", {"faces": faces})
        flat, flat_stats = build(client, args.output, "flat")
        assert flat_stats["displacement_mixed_power_pairs"] == 1
        assert flat_stats["displacement_lod_disabled_vertices"] > 12
        assert flat_stats["displacement_lod_snapped_vertices"] == 0
        assert (
            flat_stats["physics_mesh_triangles"] < flat_stats["displacement_triangles"]
        )
        draft = client.call(
            "preview_terrain_sculpt",
            {"mode": "raise", "center": [0, 0, 16], "radius": 96, "amount": 64},
        )
        client.call("commit_face_preview", {"token": draft["state"]["preview_token"]})
        original_grids = client.call("get_terrain_grids", {})
        source = args.output / "terrain.vmf"
        client.call("save_map", {"path": str(source)})
        saved = source.read_bytes()
        history = client.call("get_map_info")["undo"]
        curved, curved_stats = build(client, args.output, "curved")
        assert curved_stats["displacement_mixed_power_pairs"] == 1
        assert curved_stats["displacement_lod_snapped_vertices"] > 0
        infos = records(curved.read_bytes(), 26, "<3f4ifi2H2i48s40s10I")
        for index, info in enumerate(infos):
            edges = [
                struct.unpack_from("<H4B", info[13], offset)
                for offset in range(0, 48, 6)
            ]
            connected = [edge for edge in edges if edge[0] != 65535]
            assert len(connected) == 1 and connected[0][0] == 1 - index
            assert connected[0][2:4] == (0, 0)
            assert (info[15:] == (0xFFFFFFFF,) * 10) == (index == 0)
        assert client.call("get_terrain_grids", {}) == original_grids
        assert (
            client.call("get_map_info")["undo"] == history
            and source.read_bytes() == saved
        )
        client.call("set_object_visibility", {"action": "hide", "ids": parents[1:]})
        _, hidden_stats = build(client, args.output, "hidden")
        assert hidden_stats["displacement_mixed_power_pairs"] == 0
        assert hidden_stats["displacement_lod_disabled_vertices"] == 0
        client.call("set_object_visibility", {"action": "show", "ids": parents[1:]})
        client.call("select_faces", {"faces": faces[1:]})
        draft = client.call(
            "preview_terrain_sculpt",
            {"mode": "raise", "center": [0, 0, 80], "radius": 4096, "amount": 1},
        )
        client.call("commit_face_preview", {"token": draft["state"]["preview_token"]})
        _, mismatched_stats = build(client, args.output, "mismatched")
        assert mismatched_stats["displacement_neighbor_pairs"] == 0
        assert mismatched_stats["displacement_lod_disabled_vertices"] == 0
        client.call("undo", {})
        client.call("select_faces", {"faces": faces})
        assert client.call("get_terrain_grids", {}) == original_grids
        (args.output / "results.json").write_text(
            json.dumps(
                {
                    "flat": flat_stats,
                    "curved": curved_stats,
                    "hidden": hidden_stats,
                    "mismatched": mismatched_stats,
                    "exact_both_builds": True,
                    "source_history_preserved": True,
                },
                indent=2,
            )
            + "\n"
        )
        return flat, curved
    finally:
        client.close()


def worker(args):
    runtime.OBSERVER = runtime.OBSERVER.replace(
        'mat_fullbright"', 'r_drawothermodels 0; r_drawviewmodel 0; mat_fullbright"'
    )
    if args.view == "near":
        runtime.OBSERVER = runtime.OBSERVER.replace(
            "setpos -64 -64 32; setang 25 45 0", "setpos -96 -96 128; setang 15 45 0"
        )
        args.ground_z = 16.0
    else:
        runtime.OBSERVER = runtime.OBSERVER.replace(
            "setpos -64 -64 32; setang 25 45 0", "setpos -224 -224 128; setang 0 45 0"
        )
        args.ground_z = 0.0
    traces = []
    for y in [0, 16, 64]:
        for x in [-0.25, 0, 0.25]:
            traces.append(
                f'printl("HAMMERR_LOD_TRACE {x} {y} " + TraceLine(Vector({x},{y},180), Vector({x},{y},-8), null));'
            )
    runtime.OBSERVER = runtime.OBSERVER.replace(
        '    local player = Entities.FindByClassname(null, "player");',
        "\n".join(traces)
        + '\n    local player = Entities.FindByClassname(null, "player");',
    )
    args.ground_tolerance = 2.0
    runtime.worker(args)
    report = json.loads((args.output / "runtime.json").read_text())
    assert all(35 < point[2] < 38 for point in report["prop_positions"][-3:]), report
    text = (args.output / "cstrike/hammerr-runtime.log").read_text(errors="replace")
    heights = [
        [float(x), float(y), 180 - 188 * float(fraction)]
        for x, y, fraction in re.findall(
            r"HAMMERR_LOD_TRACE ([\d.eE+.-]+) ([\d.eE+.-]+) ([\d.eE+.-]+)", text
        )
    ]
    assert len(heights) >= 27, heights
    expected = (
        {0: 80, 16: 68.148148, 64: 32.592593}
        if args.case == "curved"
        else {0: 16, 16: 16, 64: 16}
    )
    for y, height in expected.items():
        rows = [row for row in heights[-9:] if row[1] == y]
        assert len(rows) == 3 and all(abs(row[2] - height) < 0.3 for row in rows), (
            rows,
            height,
        )
    (args.output / "seam-heights.json").write_text(json.dumps(heights, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument("--output", type=Path, default=Path("/tmp/hammerr-terrain-lod"))
    parser.add_argument(
        "--game-dir",
        type=Path,
        default=Path.home()
        / ".local/share/Steam/steamapps/common/Counter-Strike Source",
    )
    parser.add_argument("--runtime", action="store_true")
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--bsp", type=Path, help=argparse.SUPPRESS)
    parser.add_argument("--view", choices=["near", "far"], default="near")
    parser.add_argument("--case", choices=["flat", "curved"], default="flat")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    if args.worker:
        worker(args)
        return
    paths = fixture(args)
    if args.runtime:
        env = {
            **os.environ,
            "WLR_BACKENDS": "headless",
            "WLR_HEADLESS_OUTPUTS": "1",
            "WLR_RENDERER": "pixman",
        }
        for case, bsp in zip(["flat", "curved"], paths, strict=True):
            for view in ["near", "far"]:
                output = args.output / f"{case}-{view}"
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
                            "--case",
                            case,
                            "--view",
                            view,
                        ],
                        cwd=ROOT,
                        env=env,
                        stdout=log,
                        stderr=log,
                        check=True,
                        timeout=55,
                    )
    print(json.dumps({"mixed_power_terrain_lod": True, "output": str(args.output)}))


if __name__ == "__main__":
    main()
