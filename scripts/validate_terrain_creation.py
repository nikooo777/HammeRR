import argparse
import itertools
import json
import os
import struct
import subprocess
import sys
from pathlib import Path

import validate_native_runtime as runtime
from validate_compile_jobs import wait_job
from validate_face_lighting import DEFAULT_BINARY, ROOT, Client, detail, face_ref
from validate_visibility import ROOM


def fixture(args):
    client = Client(None, args.binary, args.output)
    try:
        for low, high in ROOM:
            client.call(
                "create_block",
                {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"},
            )
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
                [32, 32, 128],
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
        client.call("select_faces", {"faces": [target]})
        before = detail(client, [parent])
        source = args.output / "before.vmf"
        client.call("save_map", {"path": str(source)})
        saved = source.read_bytes()
        history = client.call("get_map_info")["undo"]
        client.call("create_terrain", {"power": 5}, error=True)
        client.call(
            "create_terrain",
            {"faces": [target, {"solid": target["solid"], "side": 999999}], "power": 2},
            error=True,
        )
        assert (
            detail(client, [parent]) == before
            and client.call("get_map_info")["undo"] == history
        )
        corners = client.call("get_terrain_creation", {})["faces"][0]["corners"]
        client.call("create_terrain", {"start_corner": 4}, error=True)
        assert detail(client, [parent]) == before
        results = []
        path = None
        for power, corner in itertools.product([2, 3, 4], range(4)):
            result = client.call(
                "create_terrain", {"power": power, "start_corner": corner}
            )
            assert (
                result["surfaces_created"] == 1
                and result["grid_size"] == (1 << power) + 1
                and result["start_corner"] == corner
            )
            faces = detail(client, [parent])[0]["faces"]
            assert (
                next(face for face in faces if face["side"] == target["side"])[
                    "displacement"
                ]
                == power
            )
            client.call("create_terrain", {"power": 2}, error=True)
            current = args.output / f"terrain-{power}-corner-{corner}.vmf"
            client.call("save_map", {"path": str(current)})
            client.call("open_map", {"path": str(current)})
            client.call("select_faces", {"faces": [target]})
            snapshot = current.read_bytes()
            client.call(
                "set_compile_settings", {"lighting": "direct_ldr", "physics": "engine"}
            )
            path = args.output / f"terrain-{power}-corner-{corner}.bsp"
            compiled = client.call("compile_map", {"path": str(path)})
            stats = compiled["stats"]
            assert compiled["success"] and stats["displacements"] == 1
            assert stats["displacement_vertices"] == ((1 << power) + 1) ** 2
            assert (
                stats["displacement_triangles"]
                == stats["physics_mesh_triangles"]
                == 2 * (1 << power) ** 2
            )
            data = path.read_bytes()
            offset, length, _, _ = struct.unpack_from("<4i", data, 8 + 26 * 16)
            assert length == 176 and struct.unpack_from("<3f", data, offset) == tuple(
                corners[corner]
            )
            assert stats["brushes"] == 6
            background = args.output / f"background-{power}-corner-{corner}.bsp"
            client.call("start_compile", {"path": str(background)})
            assert (
                wait_job(client)["status"] == "completed"
                and background.read_bytes() == path.read_bytes()
            )
            assert current.read_bytes() == snapshot
            if power in [2, 3] and corner == 0:
                client.render(
                    args.output / f"terrain-grid-{power}.png",
                    surface="wireframe",
                    camera={"position": [-260, -240, 220], "look_at": [0, 0, 48]},
                )
            results.append(
                {
                    "power": power,
                    "corner": corner,
                    "stats": stats,
                    "exact_both_builds": True,
                }
            )
            if (power, corner) != (4, 3):
                client.call("open_map", {"path": str(source), "discard_changes": True})
                client.call("select_faces", {"faces": [target]})
        current = detail(client, [parent])
        client.call(
            "remove_terrain",
            {
                "faces": [
                    target,
                    {
                        "solid": target["solid"],
                        "side": next(
                            face["side"]
                            for face in current[0]["faces"]
                            if face["displacement"] is None
                        ),
                    },
                ]
            },
            error=True,
        )
        assert detail(client, [parent]) == current
        removed = client.call("remove_terrain", {})
        assert removed["surfaces_removed"] == 1 and detail(client, [parent]) == before
        restored = client.call(
            "compile_map", {"path": str(args.output / "restored-brush.bsp")}
        )
        assert restored["success"] and restored["stats"]["brushes"] == 7
        assert (
            restored["stats"]["displacements"]
            == restored["stats"]["physics_mesh_triangles"]
            == 0
        )
        client.call("undo", {})
        assert detail(client, [parent]) == current
        assert source.read_bytes() == saved
        (args.output / "results.json").write_text(
            json.dumps(
                {
                    "cases": results,
                    "invalid_changes_atomic": True,
                    "source_preserved": True,
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
        "setpos -64 -64 32", "setpos -64 -64 96"
    ).replace(
        "setang 25 45 0", "setang 35 45 0; r_drawothermodels 0; r_drawviewmodel 0"
    )
    args.ground_z = 48.0
    runtime.worker(args)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-terrain-creation")
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
        report = json.loads((output / "runtime.json").read_text())
        assert all(
            60 < position[2] < 75 for position in report["prop_positions"][-3:]
        ), report["prop_positions"]

    print(json.dumps({"output": str(args.output), "terrain_creation": True}))


if __name__ == "__main__":
    main()
