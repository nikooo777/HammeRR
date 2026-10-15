import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

from validate_authored_planes import records
from validate_face_lighting import DEFAULT_BINARY, ROOT, Client, detail, face_ref
from validate_spawn_clearance import issues
from validate_terrain_lod import build, fixture, worker


def validate(args):
    args.split_edge = True
    _, baseline = fixture(args)
    output = args.output / "detail"
    output.mkdir(exist_ok=True)
    client = Client(None, args.binary, output)
    try:
        client.call("open_map", {"path": str(args.output / "terrain.vmf")})
        objects = client.call("get_object_report", {"filter": {"kind": "solid"}})[
            "objects"
        ]
        shapes = detail(client, [row["id"] for row in objects])
        parents = [
            shape["id"]
            for shape in shapes
            if any(face.get("displacement") is not None for face in shape["faces"])
        ]
        faces = [
            face_ref(shape, face)
            for shape in shapes
            for face in shape["faces"]
            if face.get("displacement") is not None
        ]
        entity = client.call(
            "tie_to_entity", {"ids": parents, "classname": "func_detail"}
        )["id"]
        client.call("select_faces", {"faces": faces})
        source = output / "terrain.vmf"
        client.call("save_map", {"path": str(source)})
        saved = source.read_bytes()
        history = client.call("get_map_info")["undo"]
        original = client.call("get_terrain_grids", {})
        client.call(
            "set_compile_settings", {"lighting": "direct_ldr", "physics": "engine"}
        )
        path, stats = build(client, output, "detail")
        assert (
            stats["displacements"] == 3
            and stats["models"] == 1
            and stats["displacement_half_edge_pairs"] == 2
        )
        data = path.read_bytes()
        old = baseline.read_bytes()
        infos = records(data, 26, "<3f4ifi2H2i48s40s10I")
        assert all(info[8] == 1 | 0x08000000 for info in infos)
        assert records(data, 33, "<5f") == records(old, 33, "<5f")
        assert records(data, 48, "<H") == records(old, 48, "<H")
        assert client.call("get_terrain_grids", {}) == original
        assert (
            source.read_bytes() == saved
            and client.call("get_map_info")["undo"] == history
        )
        client.call("run_map_checks")
        assert not issues(client, "compiler_displacement")
        client.call("set_object_visibility", {"action": "hide", "ids": [entity]})
        _, hidden = build(client, output, "hidden")
        assert hidden["displacements"] == 0
        client.call("set_object_visibility", {"action": "show", "ids": [entity]})
        client.call(
            "set_entity_properties",
            {"id": entity, "set": {"classname": "func_brush"}},
        )
        client.call(
            "compile_map", {"path": str(output / "unsupported.bsp")}, error=True
        )
        (args.output / "detail-results.json").write_text(
            json.dumps(
                {
                    "stats": stats,
                    "exact_both_builds": True,
                    "grid_and_tags_preserved": True,
                    "native_detail_contents": True,
                    "source_history_preserved": True,
                    "hidden_owner_omitted": True,
                    "runtime_owner_rejected": True,
                },
                indent=2,
            )
            + "\n"
        )
        return path
    finally:
        client.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-detail-terrain")
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
        args.view = "near"
        args.case = "curved"
        worker(args)
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
    print(json.dumps({"detail_terrain": True, "output": str(args.output)}))


if __name__ == "__main__":
    main()
