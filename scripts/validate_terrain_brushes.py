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
from validate_terrain_editing import grid
from validate_visibility import ROOM


def commit(client, result):
    client.call("commit_face_preview", {"token": result["state"]["preview_token"]})


def cancel(client, result):
    client.call("cancel_face_preview", {"token": result["state"]["preview_token"]})


def compile_pair(client, output, name):
    path = output / f"{name}.bsp"
    result = client.call("compile_map", {"path": str(path)})
    assert result["success"] and result["stats"]["displacement_vertices"] == 81
    assert result["stats"]["physics_mesh_triangles"] == 128
    background = output / f"{name}-background.bsp"
    client.call("start_compile", {"path": str(background)})
    assert (
        wait_job(client)["status"] == "completed"
        and background.read_bytes() == path.read_bytes()
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
        parent = client.call(
            "create_block",
            {
                "mins": [-128, -128, 32],
                "maxs": [128, 128, 48],
                "material": "dev/dev_measuregeneric01",
            },
        )["id"]
        shape = detail(client, [parent])[0]
        top = next(face for face in shape["faces"] if face["normal"][2] > 0.99)
        target = face_ref(shape, top)
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
                [32, 32, 144],
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
        client.call("select_faces", {"faces": [target]})
        client.call("create_terrain", {"power": 3, "start_corner": 2})
        before = grid(client)
        source = args.output / "before.vmf"
        client.call("save_map", {"path": str(source)})
        saved = source.read_bytes()
        request = {"mode": "raise", "center": [0, 0, 48], "radius": 32, "amount": 16}
        draft = client.call("preview_terrain_sculpt", request)
        raised = grid(client, True)
        assert grid(client) == before and raised["positions"][40][2] == 64
        assert all(
            point == before["positions"][index]
            for index, point in enumerate(raised["positions"])
            if index != 40
        )
        commit(client, draft)
        for mode, expected in [("flatten", 56), ("smooth", 48)]:
            draft = client.call(
                "preview_terrain_sculpt",
                {"mode": mode, "center": [0, 0, 56], "radius": 32, "strength": 1},
            )
            assert grid(client, True)["positions"][40][2] == expected
            cancel(client, draft)
            assert grid(client) == raised
        request = {
            "mode": "noise",
            "center": [0, 0, 64],
            "radius": 96,
            "amount": 16,
            "seed": 7,
        }
        draft = client.call("preview_terrain_sculpt", request)
        noise = grid(client, True)
        draft = client.call("preview_terrain_sculpt", request)
        assert grid(client, True) == noise
        client.call("preview_terrain_sculpt", {**request, "radius": 0}, error=True)
        client.call("preview_terrain_sculpt", {**request, "amount": 4097}, error=True)
        assert grid(client, True) == noise
        cancel(client, draft)
        client.call("undo", {})
        assert grid(client) == before
        draft = client.call(
            "preview_terrain_sculpt",
            {"mode": "raise", "center": [0, 0, 48], "radius": 8192, "amount": 24},
        )
        commit(client, draft)
        lifted = grid(client)
        assert all(71.9 < point[2] <= 72 for point in lifted["positions"])
        material = "nature/blendgrassdirt001a"
        assert (
            material
            in client.call(
                "search_materials", {"query": "nature/blendgrassdirt001a", "limit": 20}
            )["materials"]
        )
        client.call("edit_faces", {"material": material})
        client.call(
            "set_compile_settings", {"lighting": "direct_ldr", "physics": "engine"}
        )
        paths = {}
        results = []
        for value in [0, 255]:
            draft = client.call(
                "preview_terrain_blend", {"mode": "fill", "value": value}
            )
            expected = grid(client, True)
            assert expected["positions"] == lifted["positions"] and all(
                alpha == value for alpha in expected["alphas"]
            )
            commit(client, draft)
            path, stats = compile_pair(client, args.output, f"alpha-{value}")
            paths[value] = path
            vertices = records(path.read_bytes(), 33, "<5f")
            assert len(vertices) == 81 and all(
                vertex[4] == value for vertex in vertices
            )
            client.render(
                args.output / f"alpha-{value}.png",
                surface="textured",
                camera={"position": [-260, -240, 220], "look_at": [0, 0, 72]},
            )
            results.append({"value": value, "stats": stats, "exact_both_builds": True})
        draft = client.call("preview_terrain_blend", {"mode": "invert"})
        assert all(alpha == 0 for alpha in grid(client, True)["alphas"])
        cancel(client, draft)
        draft = client.call("preview_terrain_blend", {"mode": "fill", "value": 0})
        commit(client, draft)
        draft = client.call(
            "preview_terrain_blend",
            {
                "mode": "paint",
                "center": [0, 0, 72],
                "radius": 96,
                "value": 255,
                "strength": 1,
            },
        )
        painted = grid(client, True)
        assert (
            painted["positions"] == lifted["positions"]
            and painted["alphas"][40] == 255
            and painted["alphas"][0] == 0
        )
        client.call(
            "preview_terrain_blend",
            {"mode": "paint", "center": [0, 0, 72], "radius": 96, "value": 256},
            error=True,
        )
        assert grid(client, True) == painted
        commit(client, draft)
        destination = args.output / "painted.vmf"
        client.call("save_map", {"path": str(destination)})
        client.call("open_map", {"path": str(destination)})
        client.call("select_faces", {"faces": [target]})
        reopened = grid(client)
        assert all(
            abs(a - b) < 1e-6
            for p, q in zip(reopened["positions"], painted["positions"])
            for a, b in zip(p, q)
        )
        assert all(
            abs(a - b) < 1e-6 for a, b in zip(reopened["alphas"], painted["alphas"])
        )
        client.render(
            args.output / "painted.png",
            surface="textured",
            camera={"position": [-260, -240, 220], "look_at": [0, 0, 72]},
        )
        assert source.read_bytes() == saved
        (args.output / "results.json").write_text(
            json.dumps(
                {
                    "cases": results,
                    "source_preserved": True,
                    "sculpt_scope_and_cancel": True,
                    "seed_repeatable": True,
                    "blend_geometry_preserved": True,
                    "save_reopen": True,
                },
                indent=2,
            )
            + "\n"
        )
        return paths
    finally:
        client.close()


def worker(args):
    runtime.OBSERVER = runtime.OBSERVER.replace(
        "setpos -64 -64 32", "setpos -32 -32 144"
    ).replace(
        "setang 25 45 0", "setang 35 45 0; r_drawothermodels 0; r_drawviewmodel 0"
    )
    runtime.OBSERVER = runtime.OBSERVER.replace(
        '    local player = Entities.FindByClassname(null, "player");',
        '    local trace = TraceLine(Vector(-64,-64,180), Vector(-64,-64,-8), null); printl("HAMMERR_TERRAIN_TRACE " + trace);\n    local player = Entities.FindByClassname(null, "player");',
    )
    args.ground_tolerance = 2.0
    args.ground_z = 72.0
    args.capture_ppm = True
    runtime.worker(args)
    import re

    text = (args.output / "cstrike/hammerr-runtime.log").read_text(errors="replace")
    heights = [
        180 - 188 * float(value)
        for value in re.findall(r"HAMMERR_TERRAIN_TRACE ([\d.eE+-]+)", text)
    ]
    assert heights and all(abs(height - 72) < 0.1 for height in heights[-3:]), heights
    (args.output / "terrain-ray.json").write_text(
        json.dumps({"heights": heights}) + "\n"
    )


def difference(first, second):
    header, dimensions, maximum, a = first.read_bytes().split(b"\n", 3)
    h2, d2, m2, b = second.read_bytes().split(b"\n", 3)
    assert (
        (header, dimensions, maximum) == (h2, d2, m2)
        and header == b"P6"
        and maximum == b"255"
    )
    width, height = map(int, dimensions.split())
    total = count = 0
    for y in range(height * 65 // 100, height * 95 // 100):
        for x in range(width * 15 // 100, width * 85 // 100):
            offset = (y * width + x) * 3
            for channel in range(3):
                total += abs(a[offset + channel] - b[offset + channel])
                count += 1
    return total / count


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-terrain-brushes")
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
    paths = fixture(args)
    if args.runtime:
        for value, path in paths.items():
            output = args.output / f"runtime-{value}"
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
            report = json.loads((output / "runtime.json").read_text())
            assert all(
                90 < position[2] < 95 for position in report["prop_positions"][-3:]
            )
        change = difference(
            args.output / "runtime-0/game.ppm", args.output / "runtime-255/game.ppm"
        )
        assert change > 2, change
        (args.output / "blend-rendering.json").write_text(
            json.dumps(
                {
                    "floor_roi_difference": change,
                    "native_contact": True,
                    "insecure": True,
                },
                indent=2,
            )
            + "\n"
        )
    print(json.dumps({"terrain_brushes": True, "output": str(args.output)}))


if __name__ == "__main__":
    main()
