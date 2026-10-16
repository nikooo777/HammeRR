import argparse
import json
import math
import shutil
import struct
import sys
from pathlib import Path

import validate_native_runtime as runtime
from validate_authored_planes import records
from validate_compile_jobs import wait_job
from validate_direct_lighting import FACE_FORMAT, lightmaps
from validate_face_lighting import DEFAULT_BINARY, Client, detail, face_ref
from validate_visibility import ROOM


def texture(pixels, size=32, normal=False):
    header = bytearray(80)
    header[:4] = b"VTF\0"
    for offset, value in [
        (4, 7),
        (8, 2),
        (12, 80),
        (20, 0x300 | (128 if normal else 64)),
    ]:
        struct.pack_into("<I", header, offset, value)
    for offset, value in [(16, size), (18, size), (24, 1), (63, 1)]:
        struct.pack_into("<H", header, offset, value)
    struct.pack_into("<3f", header, 32, 0.5, 0.5, 0.5)
    struct.pack_into("<f", header, 48, 1.0)
    header[56] = 1
    struct.pack_into("<i", header, 57, -1)
    return bytes(header) + pixels


def assets(args):
    game = runtime.private_game(args.game_dir.resolve(), args.output / "source-game", engine_physics=True)
    folder = game / "materials/hammerr"
    folder.mkdir(parents=True, exist_ok=True)
    (folder / "white.vtf").write_bytes(texture(bytes([255] * 32 * 32 * 4)))
    (folder / "flat_normal.vtf").write_bytes(
        texture(bytes([128, 128, 255, 255] * 32 * 32), normal=True)
    )
    z = round((math.sqrt(1 - 0.75**2) + 1) * 127.5)
    pixels = bytes(
        component
        for _y in range(32)
        for x in range(32)
        for component in [round(((-0.75 if x < 16 else 0.75) + 1) * 127.5), 128, z, 255]
    )
    (folder / "tilted_normal.vtf").write_bytes(texture(pixels, normal=True))
    for name, bump in [
        ("plain", None),
        ("flat", "flat_normal"),
        ("tilted", "tilted_normal"),
    ]:
        extra = f' $bumpmap "hammerr/{bump}"' if bump else ""
        (folder / f"{name}.vmt").write_text(
            'LightmappedGeneric { $basetexture "hammerr/white" $surfaceprop concrete'
            + extra
            + " }\n"
        )
    return game


def floor_maps(data):
    planes = records(data, 1, "<4fi")
    infos = records(data, 6, "<16f2i")
    colors = lightmaps(data)
    result = []
    for face in records(data, 7, FACE_FORMAT):
        if planes[face[0]][2] < 0.99 or abs(planes[face[0]][3]) > 0.001:
            continue
        info = infos[face[5]]
        count = (face[16] + 1) * (face[17] + 1)
        maps = 4 if info[16] & 0x800 else 1
        assert face[8:12] == (0, 255, 255, 255)
        values = []
        for index in range(maps):
            start = face[12] + index * count * 4
            values.append(colors[start : start + count * 4])
            assert len(values[-1]) == count * 4
        result.append(values)
    assert result
    return result


def fixture(args):
    game = assets(args)
    client = Client(None, args.binary, args.output, game / "gameinfo.txt")
    try:
        ids = []
        for low, high in ROOM:
            ids.append(
                client.call(
                    "create_block",
                    {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"},
                )["id"]
            )
        brush = detail(client, [ids[0]])[0]
        top = next(face for face in brush["faces"] if face["normal"] == [0, 0, 1])
        receiver = face_ref(brush, top)
        for classname, origin, properties in [
            ("info_player_terrorist", [-192, -176, 8], {}),
            ("info_player_counterterrorist", [192, 176, 8], {}),
            ("light", [-128, 0, 180], {"_light": "255 0 0 12", "_constant_attn": "1"}),
            ("light", [128, 0, 180], {"_light": "0 0 255 12", "_constant_attn": "1"}),
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
                [128, 128, 128],
                {
                    "targetname": "physics_crate",
                    "model": "models/props_junk/wood_crate001a.mdl",
                },
            ),
        ]:
            ids.append(
                client.call(
                    "create_entity",
                    {
                        "classname": classname,
                        "origin": origin,
                        "properties": properties,
                    },
                )["id"]
            )
        client.call(
            "set_compile_settings", {"lighting": "direct_ldr", "physics": "engine"}
        )
        results = {}
        reference = None
        files = []
        for name in ["plain", "flat", "tilted"]:
            client.call(
                "edit_faces",
                {"faces": [receiver], "material": f"hammerr/{name}", "fit": True},
            )
            source = args.output / f"{name}.vmf"
            client.call("save_map", {"path": str(source)})
            source_bytes = source.read_bytes()
            objects = detail(client, ids)
            history = client.call("get_map_info")["undo"]
            path = args.output / f"{name}.bsp"
            result = client.call("compile_map", {"path": str(path)})
            assert result["success"], result
            data = path.read_bytes()
            maps = floor_maps(data)
            if name == "plain":
                assert result["stats"]["bump_faces"] == 0
                reference = maps
            else:
                assert (
                    result["stats"]["bump_faces"] > 0
                    and result["stats"]["bump_lightmaps"]
                    == result["stats"]["bump_faces"] * 3
                )
                assert all(len(face) == 4 for face in maps)
                assert [face[0] for face in maps] == [face[0] for face in reference]
                assert any(face[1] != face[2] for face in maps)
            background = args.output / f"{name}-background.bsp"
            client.call("start_compile", {"path": str(background)})
            job = wait_job(client)
            assert job["status"] == "completed", job
            assert background.read_bytes() == data
            assert (
                job["result"]["stats"]["bump_lightmaps"]
                == result["stats"]["bump_lightmaps"]
            )
            assert (
                detail(client, ids) == objects
                and client.call("get_map_info")["undo"] == history
            )
            assert source.read_bytes() == source_bytes
            results[name] = result["stats"]
            files.append((name, path))
        (args.output / "results.json").write_text(json.dumps(results, indent=2) + "\n")
        return files, game / "materials"
    finally:
        client.close()


def worker(args):
    private = runtime.private_game(args.game_dir.resolve(), args.output, args.bsp)
    shutil.copytree(args.assets, private / "materials", dirs_exist_ok=True)
    args.capture_ppm = True
    runtime.OBSERVER = runtime.OBSERVER.replace(
        "setpos -64 -64 32; setang 25 45 0; mat_fullbright",
        "setpos -192 -176 32; setang 35 42 0; r_drawothermodels 0; r_drawviewmodel 0; mat_fullbright",
    )
    runtime.worker(args)


def compare(output):
    frames = []
    for name in ["plain", "flat", "tilted"]:
        _, dimensions, _, pixels = (
            (output / name / "game.ppm").read_bytes().split(b"\n", 3)
        )
        frames.append((dimensions, pixels))
    assert frames[0][0] == frames[1][0] == frames[2][0]
    width, height = map(int, frames[0][0].split())
    indices = [
        (y * width + x) * 3 + component
        for y in range(height // 2, height * 9 // 10)
        for x in range(width // 4, width * 3 // 4)
        for component in [0, 2]
    ]
    differences = []
    for left, right in [(0, 1), (1, 2)]:
        values = [
            abs(frames[left][1][index] - frames[right][1][index]) for index in indices
        ]
        differences.append(sum(values) / len(values))
    assert differences[0] < 5, differences
    assert differences[1] > 8 and differences[1] > 3 * differences[0], differences
    return {
        "flat_mean_difference": differences[0],
        "tilted_mean_difference": differences[1],
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-bump-lighting")
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
    args.output = args.output.resolve()
    args.game_dir = args.game_dir.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    if args.worker:
        worker(args)
        return
    files, materials = fixture(args)
    if args.runtime:
        for name, bsp in files:
            output = args.output / name
            output.mkdir(exist_ok=True)
            runtime.private_compositor(
                [
                    sys.executable,
                    "-B",
                    str(Path(__file__).resolve()),
                    "--worker",
                    "--bsp",
                    str(bsp),
                    "--assets",
                    str(materials),
                    "--output",
                    str(output),
                    "--game-dir",
                    str(args.game_dir),
                ],
                output,
                timeout=55,
            )
        result = compare(args.output)
        (args.output / "render-results.json").write_text(
            json.dumps(result, indent=2) + "\n"
        )
        print(json.dumps(result))
    print(json.dumps({"output": str(args.output), "bump_lighting": True}))


if __name__ == "__main__":
    main()
