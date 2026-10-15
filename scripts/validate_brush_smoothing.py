import argparse
import json
import math
import struct
from pathlib import Path

import validate_native_runtime as runtime
from validate_authored_planes import records
from validate_compile_jobs import wait_job
from validate_direct_lighting import FACE_FORMAT
from validate_face_lighting import ROOT, Client, detail
from validate_visibility import ROOM


def payload(data, index):
    offset, size = struct.unpack_from("<2i", data, 8 + index * 16)
    return data[offset : offset + size]


def corner_normals(data):
    vertices = records(data, 3, "<3f")
    edges = records(data, 12, "<2H")
    surfedges = records(data, 13, "<i")
    normals = records(data, 30, "<3f")
    indices = records(data, 31, "<H")
    assert len(indices) == len(surfedges) and all(
        index[0] < len(normals) for index in indices
    )
    result = []
    for face in records(data, 7, FACE_FORMAT):
        if face[-1] == 0:
            continue
        for index in range(face[3], face[3] + face[4]):
            edge = surfedges[index][0]
            vertex = edges[abs(edge)][0 if edge >= 0 else 1]
            if vertices[vertex] == (64.0, 64.0, 160.0):
                result.append(normals[indices[index][0]])
    return result


def fixture(args):
    client = Client(None, args.binary, args.output)
    try:
        for low, high in ROOM:
            client.call(
                "create_block",
                {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"},
            )
        cube = client.call(
            "create_block",
            {
                "mins": [-64, -64, 32],
                "maxs": [64, 64, 160],
                "material": "dev/dev_measuregeneric01",
            },
        )["id"]
        shape = detail(client, [cube])[0]
        for classname, origin, properties in [
            ("info_player_terrorist", [-192, -176, 8], {}),
            ("info_player_counterterrorist", [192, 176, 8], {}),
            (
                "light",
                [-128, -128, 200],
                {
                    "_light": "255 255 255 250",
                    "_constant_attn": "1",
                    "_quadratic_attn": "0",
                },
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
                [128, 128, 128],
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
            "select_faces",
            {
                "faces": [
                    {"solid": shape["solid"], "side": face["side"]}
                    for face in shape["faces"]
                ]
            },
        )
        source = args.output / "cube.vmf"
        client.call("save_map", {"path": str(source)})
        source_bytes = source.read_bytes()
        client.call(
            "set_compile_settings", {"lighting": "direct_ldr", "physics": "engine"}
        )
        results = {}
        baseline = None
        files = []
        for name, groups in [
            ("flat", []),
            ("smooth", [1]),
            ("hard25", [1, 25]),
            ("hard32", [1, 32]),
        ]:
            client.call("edit_faces", {"smoothing_groups": groups})
            before = detail(client, [cube])
            history = client.call("get_map_info")["undo"]
            path = args.output / f"{name}.bsp"
            result = client.call("compile_map", {"path": str(path)})
            assert result["success"]
            data = path.read_bytes()
            stats = result["stats"]
            if name == "smooth":
                assert (
                    stats["smoothed_faces"] >= 6
                    and stats["smoothed_corners"] >= 24
                    and stats["normal_work"] > 0
                )
                normals = corner_normals(data)
                assert len(normals) == 3 and all(
                    abs(component - 1 / math.sqrt(3)) < 2e-7
                    for normal in normals
                    for component in normal
                )
                assert payload(data, 8) != payload(baseline, 8)
                assert payload(data, 29) == payload(baseline, 29)
            else:
                assert stats["smoothed_faces"] == 0 and stats["smoothed_corners"] == 0
                if baseline is not None:
                    assert all(
                        payload(data, index) == payload(baseline, index)
                        for index in [8, 30, 31]
                    )
            background = args.output / f"{name}-background.bsp"
            client.call("start_compile", {"path": str(background)})
            job = wait_job(client)
            assert job["status"] == "completed" and background.read_bytes() == data
            assert job["result"]["stats"]["normal_work"] == stats["normal_work"]
            assert (
                before == detail(client, [cube])
                and history == client.call("get_map_info")["undo"]
                and source.read_bytes() == source_bytes
            )
            results[name] = stats
            if name == "flat":
                baseline = data
            if name in ["flat", "smooth"]:
                files.append((name, path))
        (args.output / "results.json").write_text(json.dumps(results, indent=2) + "\n")
        return files
    finally:
        client.close()


def worker(args):
    runtime.capture_worker(args, "setpos -192 -176 32; setang 8 42 0; r_drawothermodels 0; r_drawviewmodel 0; mat_fullbright")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-brush-smoothing")
    )
    parser.add_argument("--runtime", action="store_true")
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--bsp", type=Path, help=argparse.SUPPRESS)
    parser.add_argument(
        "--game-dir",
        type=Path,
        default=Path.home()
        / ".local/share/Steam/steamapps/common/Counter-Strike Source",
    )
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    if args.worker:
        worker(args)
        return
    files = fixture(args)
    if args.runtime:
        for name, bsp in files:
            runtime.private_display(Path(__file__), args, bsp, output=args.output / name)
        (args.output / "render-results.json").write_text(
            json.dumps(runtime.compare_frames(args.output), indent=2) + "\n"
        )
    print(json.dumps({"output": str(args.output), "brush_smoothing": True}))


if __name__ == "__main__":
    main()
