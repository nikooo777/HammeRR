import argparse
import json
from pathlib import Path

from validate_face_lighting import ROOT, Client, detail, face_ref
from validate_terrain_lod import build, fixture


def validate(args):
    args.split_edge = True
    _, baseline = fixture(args)
    marked = args.output / "marked"
    marked.mkdir(exist_ok=True)
    text = (args.output / "terrain.vmf").read_text()
    assert text.count('"subdiv" "0"') == 3
    source = marked / "terrain.vmf"
    source.write_text(text.replace('"subdiv" "0"', '"subdiv" "1"'))
    client = Client(None, args.binary, marked)
    try:
        client.call("open_map", {"path": str(source)})
        objects = client.call("get_object_report", {"filter": {"kind": "solid"}})[
            "objects"
        ]
        shapes = detail(client, [row["id"] for row in objects])
        faces = [
            face_ref(shape, face)
            for shape in shapes
            for face in shape["faces"]
            if face.get("displacement") is not None
        ]
        assert len(faces) == 3
        client.call("select_faces", {"faces": faces})
        grids = client.call("get_terrain_grids", {})
        history = client.call("get_map_info")["undo"]
        client.call(
            "set_compile_settings", {"lighting": "direct_ldr", "physics": "engine"}
        )
        path, stats = build(client, marked, "subdivided")
        assert path.read_bytes() == baseline.read_bytes()
        assert client.call("get_terrain_grids", {}) == grids
        assert client.call("get_map_info")["undo"] == history
        client.call("preview_terrain_sew", {}, error=True)
        client.call("preview_terrain_resolution", {"power": 3}, error=True)
        assert client.call("get_terrain_grids", {}) == grids
        saved = marked / "saved.vmf"
        client.call("save_map", {"path": str(saved)})
        assert saved.read_text().count('"subdiv" "1"') == 3
        if args.reference:
            assert path.read_bytes() == args.reference.read_bytes()
        (args.output / "subdivided-results.json").write_text(
            json.dumps(
                {
                    "stats": stats,
                    "exact_marker_independent_native_bytes": True,
                    "exact_both_builds": True,
                    "source_history_preserved": True,
                    "marker_round_trip": True,
                    "editor_cache_guards_preserved": True,
                    "matches_reference": bool(args.reference),
                },
                indent=2,
            )
            + "\n"
        )
    finally:
        client.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-subdivided-terrain")
    )
    parser.add_argument(
        "--reference",
        type=Path,
        help="Require exact bytes against an independently validated BSP",
    )
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    validate(args)
    print(json.dumps({"subdivided_terrain": True, "output": str(args.output)}))


if __name__ == "__main__":
    main()
