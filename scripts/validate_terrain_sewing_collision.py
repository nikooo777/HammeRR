import argparse
import json
import os
import re
import subprocess
import sys
from pathlib import Path

import validate_native_runtime as runtime
from validate_authored_planes import records
from validate_compile_jobs import wait_job
from validate_face_lighting import DEFAULT_BINARY, ROOT, Client, detail, face_ref
from validate_visibility import ROOM


def grids(client, faces=None, preview=False):
    return client.call("get_terrain_grids", {"faces": faces, "preview": preview})[
        "faces"
    ]


def commit(client, result):
    client.call("commit_face_preview", {"token": result["state"]["preview_token"]})


def compile_pair(client, output, name):
    path = output / f"{name}.bsp"
    result = client.call("compile_map", {"path": str(path)})
    assert result["success"] and result["stats"]["displacement_neighbor_pairs"] == 1
    other = output / f"{name}-background.bsp"
    client.call("start_compile", {"path": str(other)})
    assert (
        wait_job(client)["status"] == "completed"
        and path.read_bytes() == other.read_bytes()
    )
    return path, result["stats"]


def fixture(args):
    client = Client(None, args.binary, args.output)
    try:
        for low, high in ROOM:
            client.call(
                "create_block",
                {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"},
            )
        faces = []
        ids = []
        for index, (low, high) in enumerate(
            [([-128, -128, 0], [0, 128, 16]), ([0, -128, 0], [128, 128, 16])]
        ):
            parent = client.call(
                "create_block",
                {"mins": low, "maxs": high, "material": "nature/blendgrassdirt001a"},
            )["id"]
            shape = detail(client, [parent])[0]
            target = face_ref(
                shape, next(face for face in shape["faces"] if face["normal"][2] > 0.99)
            )
            client.call("select_faces", {"faces": [target]})
            client.call("create_terrain", {"power": 2, "start_corner": index})
            draft = client.call(
                "preview_terrain_blend", {"mode": "fill", "value": index * 255}
            )
            commit(client, draft)
            if index:
                draft = client.call(
                    "preview_terrain_sculpt",
                    {
                        "mode": "raise",
                        "center": [64, 0, 16],
                        "radius": 8192,
                        "amount": 16,
                    },
                )
                commit(client, draft)
            faces.append(target)
            ids.append(parent)
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
                [-64, 32, 128],
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
        client.call("select_faces", {"faces": faces})
        old = grids(client)
        source = args.output / "before.vmf"
        client.call("save_map", {"path": str(source)})
        saved = source.read_bytes()
        original = detail(client, ids)
        draft = client.call("preview_terrain_sew", {"blend": True})
        assert (
            draft["neighbor_pairs"] == 1
            and draft["sewn_vertices"] == 5
            and grids(client) == old
        )
        sewn = grids(client, preview=True)
        edges = []
        for face in sewn:
            edges.append(
                sorted(
                    (point[1], point[2], face["alphas"][index])
                    for index, point in enumerate(face["positions"])
                    if point[0] == 0
                )
            )
        assert edges[0] == edges[1] and all(value[2] == 127.5 for value in edges[0])
        client.call("preview_terrain_sew", {"faces": faces[:1]}, error=True)
        assert grids(client, preview=True) == sewn
        client.call("cancel_face_preview", {"token": draft["state"]["preview_token"]})
        assert grids(client) == old
        draft = client.call("preview_terrain_sew", {"blend": True})
        commit(client, draft)
        assert grids(client) == sewn
        client.call("undo", {})
        assert grids(client) == old and detail(client, ids) == original
        draft = client.call("preview_terrain_sew", {"blend": True})
        commit(client, draft)
        client.render(
            args.output / "sewn.png",
            surface="textured",
            camera={"position": [-260, -240, 220], "look_at": [0, 0, 24]},
        )
        client.call(
            "set_compile_settings", {"lighting": "direct_ldr", "physics": "engine"}
        )
        paths = {}
        results = []
        for name, settings, flag, triangles in [
            (
                "baseline",
                {
                    "physics_collision": True,
                    "player_collision": True,
                    "ray_collision": True,
                },
                0,
                64,
            ),
            (
                "no-physics",
                {
                    "physics_collision": False,
                    "player_collision": True,
                    "ray_collision": True,
                },
                2,
                32,
            ),
            (
                "no-player",
                {
                    "physics_collision": True,
                    "player_collision": False,
                    "ray_collision": True,
                },
                4,
                64,
            ),
            (
                "no-ray",
                {
                    "physics_collision": True,
                    "player_collision": True,
                    "ray_collision": False,
                },
                8,
                64,
            ),
        ]:
            draft = client.call(
                "preview_terrain_collision", {"faces": faces[:1], **settings}
            )
            expected = grids(client, faces[:1], True)[0]
            assert expected["flags"] == flag and all(
                expected[key] == value for key, value in settings.items()
            )
            commit(client, draft)
            path, stats = compile_pair(client, args.output, name)
            assert stats["physics_mesh_triangles"] == triangles
            infos = records(path.read_bytes(), 26, "<3f4ifi2H2i48s40s10I")
            assert sorted(info[6] & 14 for info in infos) == sorted([flag, 0])
            paths[name] = path
            results.append(
                {"case": name, "stats": stats, "flags": flag, "exact_both_builds": True}
            )
        assert source.read_bytes() == saved
        (args.output / "results.json").write_text(
            json.dumps(
                {
                    "cases": results,
                    "source_preserved": True,
                    "sew_scope_cancel_undo": True,
                    "alpha_sewn": True,
                },
                indent=2,
            )
            + "\n"
        )
        return paths
    finally:
        client.close()


def worker(args):
    runtime.OBSERVER = (
        runtime.OBSERVER.replace("setpos -64 -64 32", "setpos -64 -64 96")
        .replace(
            "setang 25 45 0", "setang 35 45 0; r_drawothermodels 0; r_drawviewmodel 0"
        )
        .replace(
            '    local player = Entities.FindByClassname(null, "player");',
            '    local trace = TraceLine(Vector(-96,-96,180), Vector(-96,-96,-8), null); printl("HAMMERR_TERRAIN_TRACE " + trace);\n    local player = Entities.FindByClassname(null, "player");',
        )
    )
    args.ground_z = 0 if args.case == "no-player" else 16
    args.ground_tolerance = 2.0
    runtime.worker(args)
    text = (args.output / "cstrike/hammerr-runtime.log").read_text(errors="replace")
    heights = [
        180 - 188 * float(value)
        for value in re.findall(r"HAMMERR_TERRAIN_TRACE ([\d.eE+-]+)", text)
    ]
    expected = 0 if args.case == "no-ray" else 16
    assert heights and all(abs(height - expected) < 0.1 for height in heights[-3:]), (
        heights
    )
    report = json.loads((args.output / "runtime.json").read_text())
    expected = 20 if args.case == "no-physics" else 36
    assert all(
        abs(point[2] - expected) < 1 for point in report["prop_positions"][-3:]
    ), report["prop_positions"]
    (args.output / "terrain-ray.json").write_text(
        json.dumps({"heights": heights, "case": args.case}) + "\n"
    )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-terrain-sewing-collision")
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
    parser.add_argument("--case", help=argparse.SUPPRESS)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    if args.worker:
        worker(args)
        return
    paths = fixture(args)
    if args.runtime:
        for name, path in paths.items():
            output = args.output / name
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
                        str(path),
                        "--case",
                        name,
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
    print(json.dumps({"terrain_sewing_collision": True, "output": str(args.output)}))


if __name__ == "__main__":
    main()
