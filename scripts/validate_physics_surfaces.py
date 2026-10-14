import argparse
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

import validate_native_runtime as runtime
from validate_compile_jobs import wait_job
from validate_engine_physics import physics_records
from validate_face_lighting import ROOT, Client, detail, face_ref
from validate_visibility import ROOM


def assets(args):
    game = runtime.private_game(args.game_dir.resolve(), args.output / "source-game")
    library = game.parent / "bin"
    if not library.exists():
        library.symlink_to(args.game_dir.resolve() / "bin", target_is_directory=True)
    folder = game / "materials/hammerr"
    folder.mkdir(parents=True, exist_ok=True)
    for name in ["default", "ice", "rubber"]:
        (folder / f"surface_{name}.vmt").write_text(
            f"LightmappedGeneric {{ $basetexture dev/dev_measuregeneric01 $surfaceprop {name} }}\n"
        )
    return game


def fixture(args, game, name):
    output = args.output / name
    output.mkdir(exist_ok=True)
    client = Client(None, args.binary, output, game / "gameinfo.txt")
    try:
        ids = [
            client.call(
                "create_block",
                {"mins": low, "maxs": high, "material": "hammerr/surface_default"},
            )["id"]
            for low, high in ROOM
        ]
        ramp = client.call(
            "create_block",
            {
                "mins": [-192, -96, 32],
                "maxs": [192, 96, 48],
                "material": "hammerr/surface_default",
            },
        )["id"]
        ids.append(ramp)
        shape = detail(client, [ramp])[0]
        top = next(face for face in shape["faces"] if face["normal"][2] > 0.99)
        client.call(
            "edit_faces",
            {"faces": [face_ref(shape, top)], "material": f"hammerr/surface_{name}"},
        )
        client.call(
            "transform_objects",
            {"ids": [ramp], "rotation": [0, 12, 0], "pivot": [0, 0, 40]},
        )
        for classname, origin, props in [
            ("info_player_terrorist", [-224, -176, 8], {}),
            ("info_player_counterterrorist", [224, 176, 8], {}),
            (
                "light",
                [0, 0, 180],
                {"_light": "255 255 255 120", "_constant_attn": "1"},
            ),
            (
                "logic_script",
                [-224, -176, 64],
                {
                    "targetname": "hammerr_observer",
                    "vscripts": "hammerr_observer.nut",
                    "thinkfunction": "HammeRRThink",
                },
            ),
            (
                "prop_physics",
                [-96, 0, 128],
                {
                    "targetname": "physics_crate",
                    "model": "models/props_junk/wood_crate001a.mdl",
                },
            ),
        ]:
            ids.append(
                client.call(
                    "create_entity",
                    {"classname": classname, "origin": origin, "properties": props},
                )["id"]
            )
        source = output / "surfaces.vmf"
        client.call("save_map", {"path": str(source)})
        before = detail(client, ids)
        history = client.call("get_map_info")["undo"]
        saved = source.read_bytes()
        client.call(
            "set_compile_settings", {"lighting": "direct_ldr", "physics": "engine"}
        )
        bsp = output / "surfaces.bsp"
        result = client.call("compile_map", {"path": str(bsp)})
        assert result["success"], result
        data = bsp.read_bytes()
        models = physics_records(data)
        keys = models[0][2].decode()
        assert (
            f'"{name}" "2"' in keys
            and result["stats"]["physics_surface_properties"] == 2
        )
        background = output / "background.bsp"
        client.call("start_compile", {"path": str(background)})
        assert (
            wait_job(client)["status"] == "completed"
            and background.read_bytes() == data
        )
        assert (
            before == detail(client, ids)
            and history == client.call("get_map_info")["undo"]
            and source.read_bytes() == saved
        )
        (output / "results.json").write_text(
            json.dumps(
                {
                    "stats": result["stats"],
                    "keys": keys,
                    "exact_shared_builds": True,
                    "source_history_preserved": True,
                },
                indent=2,
            )
            + "\n"
        )
        return bsp
    finally:
        client.close()


def worker(args):
    shutil.copytree(args.assets, args.output / "cstrike/materials", dirs_exist_ok=True)
    runtime.OBSERVER = runtime.OBSERVER.replace(
        "setpos -64 -64 32", "setpos -224 -176 32"
    )
    runtime.worker(args)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-physics-surfaces")
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
    parser.add_argument("--assets", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    if args.worker:
        worker(args)
        return
    game = assets(args)
    for name in ["ice", "rubber"]:
        bsp = fixture(args, game, name)
        if args.runtime:
            output = args.output / name / "runtime"
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
                        "--assets",
                        str(game / "materials"),
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
    if args.runtime:
        positions = {
            name: json.loads((args.output / name / "runtime/runtime.json").read_text())[
                "prop_positions"
            ]
            for name in ["ice", "rubber"]
        }
        last = {name: values[-1] for name, values in positions.items()}
        assert last["ice"][0] > last["rubber"][0] + 40, last
        assert abs(last["rubber"][0] + 96) < 40, last
        (args.output / "surface-contact.json").write_text(
            json.dumps(
                {"final_positions": last, "friction_changes_contact": True}, indent=2
            )
            + "\n"
        )
    print(json.dumps({"output": str(args.output), "physics_surfaces": True}))


if __name__ == "__main__":
    main()
