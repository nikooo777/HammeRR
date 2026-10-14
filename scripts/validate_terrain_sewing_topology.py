import argparse
import json
import os
import re
import subprocess
import sys
from pathlib import Path

import validate_native_runtime as runtime
from validate_face_lighting import ROOT, Client, detail, face_ref
from validate_terrain_lod import build
from validate_visibility import ROOM


def commit(client, draft):
    client.call("commit_face_preview", {"token": draft["state"]["preview_token"]})


def grids(client, preview=False):
    return client.call("get_terrain_grids", {"preview": preview})["faces"]


def point(grid, x, y):
    return next(
        (p, grid["alphas"][i])
        for i, p in enumerate(grid["positions"])
        if abs(p[0] - x) < 1e-6 and abs(p[1] - y) < 1e-6
    )


def fixture(args):
    client = Client(None, args.binary, args.output)
    try:
        for low, high in ROOM:
            client.call(
                "create_block",
                {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"},
            )
        faces = []
        for power, corner, low, high, height, alpha in [
            (2, 1, [-128, -128, 0], [0, 128, 16], 24, 0),
            (3, 3, [0, -128, 0], [128, 0, 16], 40, 128),
            (4, 2, [0, 0, 0], [128, 128, 16], 56, 255),
        ]:
            parent = client.call(
                "create_block",
                {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"},
            )["id"]
            shape = detail(client, [parent])[0]
            face = face_ref(
                shape, next(face for face in shape["faces"] if face["normal"][2] > 0.99)
            )
            faces.append(face)
            client.call("select_faces", {"faces": [face]})
            client.call("create_terrain", {"power": power, "start_corner": corner})
            for _ in range(3):
                commit(
                    client,
                    client.call(
                        "preview_terrain_sculpt",
                        {
                            "mode": "flatten",
                            "center": [0, 0, height],
                            "radius": 8192,
                            "strength": 1,
                        },
                    ),
                )
            commit(
                client,
                client.call("preview_terrain_blend", {"mode": "fill", "value": alpha}),
            )
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
        before = grids(client)
        history = client.call("get_map_info")["undo"]
        no_blend = client.call("preview_terrain_sew", {})
        assert no_blend["neighbor_pairs"] == 3 and no_blend["interpolated_vertices"] > 0
        assert "fine border points" in no_blend["summary"]
        assert [grid["alphas"] for grid in grids(client, True)] == [
            grid["alphas"] for grid in before
        ]
        client.call(
            "cancel_face_preview", {"token": no_blend["state"]["preview_token"]}
        )
        assert grids(client) == before
        draft = client.call("preview_terrain_sew", {"blend": True})
        preview = grids(client, True)
        assert (
            grids(client) == before and client.call("get_map_info")["undo"] == history
        )
        for grid in preview:
            p, alpha = point(grid, 0, 0)
            assert abs(p[2] - 40) < 1e-6 and abs(alpha - (0 + 128 + 255) / 3) < 1e-6
        p, alpha = point(preview[1], 0, -16)
        assert abs(p[2] - 38) < 1e-5 and abs(alpha - 111.75) < 1e-6, (p, alpha)
        for a, b in zip(before, preview, strict=True):
            assert (a["power"], a["flags"]) == (b["power"], b["flags"])
            n = a["grid_size"]
            for row in range(1, n - 1):
                for col in range(1, n - 1):
                    i = row * n + col
                    assert (
                        a["positions"][i] == b["positions"][i]
                        and a["alphas"][i] == b["alphas"][i]
                    )
        client.call(
            "preview_terrain_sew", {"faces": faces[:1], "blend": True}, error=True
        )
        assert grids(client, True) == preview
        commit(client, draft)
        assert grids(client) == preview
        repeated = client.call("preview_terrain_sew", {"blend": True})
        assert repeated["faces_changed"] == 0
        client.call(
            "cancel_face_preview", {"token": repeated["state"]["preview_token"]}
        )
        source = args.output / "sewn.vmf"
        client.call("save_map", {"path": str(source)})
        saved = source.read_bytes()
        history = client.call("get_map_info")["undo"]
        path, stats = build(client, args.output, "sewn")
        assert (
            stats["displacement_neighbor_pairs"] == 3
            and stats["displacement_half_edge_pairs"] == 2
        )
        assert (
            history == client.call("get_map_info")["undo"]
            and source.read_bytes() == saved
        )
        client.render(
            args.output / "sewn.png",
            camera={"position": [-270, -250, 230], "look_at": [0, 0, 36]},
        )
        client.call("undo", {})
        assert grids(client) == before
        client.call("open_map", {"path": str(source), "discard_changes": True})
        client.call("select_faces", {"faces": faces})
        assert grids(client) == preview
        (args.output / "results.json").write_text(
            json.dumps(
                {
                    "stats": stats,
                    "shared_preview": True,
                    "cancel_and_invalid_preserve_draft": True,
                    "exact_undo": True,
                    "save_reopen": True,
                    "idempotent": True,
                    "interiors_preserved": True,
                    "interpolated_vertices": draft["interpolated_vertices"],
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
        "setpos -64 -64 32; setang 25 45 0",
        "setpos -96 -96 128; setang 25 45 0; r_drawothermodels 0; r_drawviewmodel 0",
    )
    traces = []
    for y in [-64, -16, 0]:
        traces.append(
            f'printl("HAMMERR_SEW_TOPOLOGY {y} " + TraceLine(Vector(0,{y},180),Vector(0,{y},-8),null));'
        )
    runtime.OBSERVER = runtime.OBSERVER.replace(
        '    local player = Entities.FindByClassname(null, "player");',
        "\n".join(traces)
        + '\n    local player = Entities.FindByClassname(null, "player");',
    )
    args.ground_z = 24.0
    args.ground_tolerance = 2.0
    runtime.worker(args)
    report = json.loads((args.output / "runtime.json").read_text())
    assert all(75 < p[2] < 78 for p in report["prop_positions"][-3:]), report
    text = (args.output / "cstrike/hammerr-runtime.log").read_text(errors="replace")
    heights = [
        (float(y), 180 - 188 * float(fraction))
        for y, fraction in re.findall(
            r"HAMMERR_SEW_TOPOLOGY ([\d.eE+.-]+) ([\d.eE+.-]+)", text
        )
    ]
    expected = {-64: 32, -16: 38, 0: 40}
    assert len(heights) > 9 and all(
        abs(height - expected[y]) < 0.1 for y, height in heights[-9:]
    ), heights
    (args.output / "seam-heights.json").write_text(json.dumps(heights) + "\n")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-terrain-sewing-topology")
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
    print(json.dumps({"terrain_sewing_topology": True, "output": str(args.output)}))


if __name__ == "__main__":
    main()
