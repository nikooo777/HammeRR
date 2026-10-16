import argparse
import json
import re
import sys
from pathlib import Path

import validate_native_runtime as runtime
from validate_compile_jobs import wait_job
from validate_face_lighting import DEFAULT_BINARY, Client, detail, face_ref
from validate_visibility import ROOM


def imported_grid():
    size = 5
    normals = "\n".join(
        f'"row{r}" "' + " ".join(["0 0 1"] * size) + '"' for r in range(size)
    )
    distances = "\n".join(
        f'"row{r}" "'
        + " ".join(
            str(16 if 1 <= r <= 3 and 1 <= c <= 3 else 12 if (r, c) == (0, 4) else 0)
            for c in range(size)
        )
        + '"'
        for r in range(size)
    )
    alphas = "\n".join(
        f'"row{r}" "' + " ".join(str(r * 25 + c * 8) for c in range(size)) + '"'
        for r in range(size)
    )
    tags = "\n".join(f'"row{r}" "' + " ".join(["7"] * 8) + '"' for r in range(4))
    return f'dispinfo {{ "power" "2" "startposition" "[-128 -128 48]" "flags" "0" "elevation" "0" "subdiv" "0" "custom_note" "preserve" normals {{ {normals} }} distances {{ {distances} }} alphas {{ {alphas} }} triangle_tags {{ {tags} }} allowed_verts {{ "10" "-1 -1 -1 -1 -1 -1 -1 -1 -1 -1" }} }}'


def grid(client, preview=False):
    return client.call("get_terrain_grids", {"preview": preview})["faces"][0]


def shared_samples(old, new):
    stride = (new["grid_size"] - 1) // (old["grid_size"] - 1)
    for row in range(old["grid_size"]):
        for col in range(old["grid_size"]):
            a = row * old["grid_size"] + col
            b = row * stride * new["grid_size"] + col * stride
            assert old["positions"][a] == new["positions"][b]
            assert old["alphas"][a] == new["alphas"][b]


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
        source = args.output / "imported.vmf"
        client.call("save_map", {"path": str(source)})
        pattern = rf'(\bside\s*\{{\s*"id"\s*"{top["side"]}"[^{{}}]*)(\}})'
        text, count = re.subn(
            pattern,
            lambda match: match[1] + imported_grid() + "\n" + match[2],
            source.read_text(),
        )
        assert count == 1
        source.write_text(text)
        client.call("open_map", {"path": str(source)})
        client.call("select_faces", {"faces": [target]})
        saved = source.read_bytes()
        old = grid(client)
        original = detail(client, [parent])
        history = client.call("get_map_info")["undo"]
        draft = client.call("preview_terrain_resolution", {"power": 4})
        token = draft["state"]["preview_token"]
        shared_samples(old, grid(client, True))
        assert grid(client) == old and client.call("get_map_info")["undo"] == history
        ordinary = face_ref(
            shape, next(face for face in shape["faces"] if face["side"] != top["side"])
        )
        client.call(
            "preview_terrain_resolution",
            {"faces": [target, ordinary], "power": 3},
            error=True,
        )
        client.call("preview_terrain_resolution", {"power": 5}, error=True)
        assert grid(client, True)["power"] == 4
        client.call("cancel_face_preview", {"token": token})
        assert grid(client) == old
        results = []
        final = None
        for power in [3, 4]:
            draft = client.call("preview_terrain_resolution", {"power": power})
            expected = grid(client, True)
            shared_samples(old, expected)
            client.call(
                "commit_face_preview", {"token": draft["state"]["preview_token"]}
            )
            assert grid(client) == expected
            final = args.output / f"refined-{power}.bsp"
            client.call(
                "set_compile_settings", {"lighting": "direct_ldr", "physics": "engine"}
            )
            result = client.call("compile_map", {"path": str(final)})
            assert (
                result["success"]
                and result["stats"]["displacement_vertices"] == ((1 << power) + 1) ** 2
            )
            assert result["stats"]["physics_mesh_triangles"] == 2 * (1 << power) ** 2
            background = args.output / f"background-{power}.bsp"
            client.call("start_compile", {"path": str(background)})
            assert (
                wait_job(client)["status"] == "completed"
                and final.read_bytes() == background.read_bytes()
            )
            destination = args.output / f"refined-{power}.vmf"
            client.call("save_map", {"path": str(destination)})
            client.call("open_map", {"path": str(destination)})
            client.call("select_faces", {"faces": [target]})
            assert grid(client) == expected
            client.render(
                args.output / f"refined-{power}.png",
                surface="wireframe",
                camera={"position": [-260, -240, 220], "look_at": [0, 0, 60]},
            )
            results.append(
                {"power": power, "stats": result["stats"], "exact_both_builds": True}
            )
        draft = client.call("preview_terrain_resolution", {"power": 2})
        assert grid(client, True) == old
        client.call("commit_face_preview", {"token": draft["state"]["preview_token"]})
        assert grid(client) == old and detail(client, [parent]) == original
        client.call("undo", {})
        assert grid(client)["power"] == 4 and source.read_bytes() == saved
        (args.output / "results.json").write_text(
            json.dumps(
                {
                    "cases": results,
                    "source_preserved": True,
                    "shared_samples_preserved": True,
                    "cancel_and_atomic_errors": True,
                    "coarsening_and_undo": True,
                },
                indent=2,
            )
            + "\n"
        )
        return final
    finally:
        client.close()


def worker(args):
    runtime.OBSERVER = runtime.OBSERVER.replace(
        "setpos -64 -64 32", "setpos -32 -32 120"
    ).replace(
        "setang 25 45 0", "setang 35 45 0; r_drawothermodels 0; r_drawviewmodel 0"
    )
    args.ground_z = 64.0
    runtime.worker(args)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-terrain-editing")
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
        report = json.loads((output / "runtime.json").read_text())
        assert all(
            80 < position[2] < 90 for position in report["prop_positions"][-3:]
        ), report["prop_positions"]
    print(json.dumps({"terrain_editing": True, "output": str(args.output)}))


if __name__ == "__main__":
    main()
