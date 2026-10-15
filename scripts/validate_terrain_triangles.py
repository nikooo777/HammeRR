import argparse
import json
import math
import os
import struct
import subprocess
import sys
from pathlib import Path

import validate_native_runtime as runtime
from validate_authored_planes import records
from validate_face_lighting import DEFAULT_BINARY, ROOT, Client, detail, face_ref
from validate_terrain_lod import build
from validate_visibility import ROOM


def commit(client, draft):
    client.call("commit_face_preview", {"token": draft["state"]["preview_token"]})


def lump(data, index):
    start, length, _, _ = struct.unpack_from("<4i", data, 8 + index * 16)
    return data[start : start + length]


def validate(args):
    client = Client(None, args.binary, args.output)
    try:
        for low, high in ROOM:
            client.call(
                "create_block",
                {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"},
            )
        faces = []
        for index, power in enumerate([2, 3, 4]):
            left = -192 + index * 128
            parent = client.call(
                "create_block",
                {
                    "mins": [left, -128, 0],
                    "maxs": [left + 128, 128, 16],
                    "material": "dev/dev_measuregeneric01",
                },
            )["id"]
            shape = detail(client, [parent])[0]
            face = face_ref(
                shape, next(face for face in shape["faces"] if face["normal"][2] > 0.99)
            )
            client.call(
                "create_terrain",
                {"faces": [face], "power": power, "start_corner": index},
            )
            faces.append(face)
        for classname, origin, properties in [
            ("info_player_terrorist", [-192, -176, 8], {}),
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
        client.call("select_faces", {"faces": faces})
        client.call(
            "set_compile_settings", {"lighting": "direct_ldr", "physics": "engine"}
        )
        source = args.output / "source.vmf"
        client.call("save_map", {"path": str(source)})
        original = client.call("get_terrain_triangles", {})
        grids = client.call("get_terrain_grids", {})
        history = client.call("get_map_info")["undo"]
        baseline, _ = build(client, args.output, "automatic")
        targets = []
        for face, row, count in zip(
            faces, original["faces"], [32, 128, 512], strict=True
        ):
            triangles = row["triangles"]
            assert [triangle["id"] for triangle in triangles] == list(range(count))
            size = math.isqrt(count // 2) + 1
            assert triangles[0]["indices"] == [0, size, size + 1]
            assert all(
                triangle["source_tags"] is None
                and triangle["walkable"]
                and triangle["buildable"]
                for triangle in triangles
            )
            targets.append({"face": face, "triangles": [0, count - 1]})
        arguments = {"targets": targets, "walkable": "disabled", "buildable": "enabled"}
        draft = client.call("preview_terrain_triangle_tags", arguments)
        preview = client.call("get_terrain_triangles", {"preview": True})
        for row in preview["faces"]:
            for triangle in row["triangles"]:
                selected = triangle["id"] in [0, len(row["triangles"]) - 1]
                assert triangle["walkable"] == (not selected)
                assert triangle["buildable"]
                assert triangle["walkable_override"] == (False if selected else None)
                assert triangle["buildable_override"] == (True if selected else None)
                assert not triangle["removed"]
        assert client.call("get_terrain_grids", {"preview": True}) == grids
        assert client.call("get_map_info")["undo"] == history
        invalid = [
            {"face": faces[0], "triangles": [1]},
            {"face": faces[1], "triangles": [128]},
        ]
        client.call(
            "preview_terrain_triangle_tags",
            {**arguments, "targets": invalid},
            error=True,
        )
        assert client.call("get_terrain_triangles", {"preview": True}) == preview
        try:
            rejected = client.rpc(
                "tools/call",
                {
                    "name": "preview_terrain_triangle_tags",
                    "arguments": {**arguments, "unknown_setting": True},
                },
            )
        except RuntimeError as error:
            assert "unknown_setting" in str(error)
        else:
            assert rejected.get("isError"), rejected
        assert client.call("get_terrain_triangles", {"preview": True}) == preview
        client.call("cancel_face_preview", {"token": draft["state"]["preview_token"]})
        assert client.call("get_terrain_triangles", {}) == original
        commit(client, client.call("preview_terrain_triangle_tags", arguments))
        classified, stats = build(client, args.output, "classified")
        data, old = classified.read_bytes(), baseline.read_bytes()
        assert all(
            lump(data, index) == lump(old, index) for index in range(64) if index != 48
        )
        infos = records(data, 26, "<3f4ifi2H2i48s40s10I")
        tags = records(data, 48, "<H")
        for info in infos:
            start, count = info[4], 2 * (1 << info[5]) ** 2
            assert [tag[0] for tag in tags[start : start + count]] == [
                5 if index in [0, count - 1] else 7 for index in range(count)
            ]
        client.call("run_map_checks")
        issues = client.call(
            "get_map_checks", {"query": "compiler_displacement_triangles"}
        )
        assert issues["matching"] == 0
        client.call("undo")
        assert client.call("get_terrain_triangles", {}) == original
        client.call("redo")
        assert client.call("get_terrain_triangles", {}) == preview
        saved = args.output / "classified.vmf"
        client.call("save_map", {"path": str(saved)})
        client.call("open_map", {"path": str(saved)})
        client.call("select_faces", {"faces": faces})
        assert client.call("get_terrain_triangles", {}) == preview
        commit(
            client,
            client.call(
                "preview_terrain_triangle_tags",
                {"targets": targets, "walkable": "automatic", "buildable": "automatic"},
            ),
        )
        assert all(
            triangle["walkable_override"] is None
            and triangle["buildable_override"] is None
            for row in client.call("get_terrain_triangles", {})["faces"]
            for triangle in row["triangles"]
        )
        restored, _ = build(client, args.output, "restored")
        assert lump(restored.read_bytes(), 48) == lump(old, 48)
        args.output.joinpath("results.json").write_text(
            json.dumps(
                {
                    "stats": stats,
                    "native_order_and_bits": True,
                    "arbitrary_subsets": True,
                    "atomic_failure": True,
                    "owned_cancel": True,
                    "undo_redo_save_reopen": True,
                    "other_native_lumps_exact": True,
                    "both_compile_paths_exact": True,
                },
                indent=2,
            )
            + "\n"
        )
        return classified
    finally:
        client.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-terrain-triangles")
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
        args.ground_z = 16.0
        args.ground_tolerance = 2.0
        runtime.worker(args)
        return
    bsp = validate(args)
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
    print(json.dumps({"terrain_triangles": True, "output": str(args.output)}))


if __name__ == "__main__":
    main()
