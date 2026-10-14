import argparse
import json
from pathlib import Path

from validate_face_lighting import ROOT, Client, detail, face_ref
from validate_terrain_editing import grid


def validate(args):
    client = Client(None, args.binary, args.output)
    try:
        parent = client.call(
            "create_block",
            {
                "mins": [-64, -64, 0],
                "maxs": [64, 64, 16],
                "material": "dev/dev_measuregeneric01",
            },
        )["id"]
        shape = detail(client, [parent])[0]
        target = face_ref(
            shape, next(face for face in shape["faces"] if face["normal"][2] > 0.99)
        )
        client.call("select_faces", {"faces": [target]})
        client.call("create_terrain", {"power": 2})
        before = grid(client)
        history = client.call("get_map_info")["undo"]
        draft = client.call(
            "preview_terrain_sculpt",
            {"mode": "raise", "center": [0, 0, 16], "radius": 32, "amount": 64},
        )
        ray = {"origin": [0, 0, 200], "direction": [0, 0, -1]}
        assert client.call("raycast", ray)["point"] == [0, 0, 80]
        pixel = {
            "x": 255.5,
            "y": 255.5,
            "width": 512,
            "height": 512,
            "camera": {"projection": "top", "center": [0, 0, 16], "ortho_height": 256},
        }
        picked = client.call("pick_at_pixel", pixel)
        assert (
            picked["side"] == target["side"] and abs(picked["point"][2] - 80) < 1e-5
        ), picked
        framed_draft = client.call("frame_selection", {"aspect": 1})
        assert grid(client) == before and client.call("get_map_info")["undo"] == history
        client.call("commit_face_preview", {"token": draft["state"]["preview_token"]})
        bounds = detail(client, [parent])[0]["bounds"]
        assert bounds == {"mins": [-64, -64, 16], "maxs": [64, 64, 80]}, bounds
        assert client.call("frame_selection", {"aspect": 1}) == framed_draft
        history = client.call("get_map_info")["undo"]
        for _ in range(3):
            assert client.call("raycast", ray)["point"] == [0, 0, 80]
        assert client.call("get_map_info")["undo"] == history
        owner = client.call(
            "tie_to_entity", {"ids": [parent], "classname": "func_brush"}
        )
        hit = client.call("raycast", ray)
        assert (
            hit["object"].startswith("entity:") and hit["solid"] == target["solid"]
        ), (owner, hit)
        client.call("select_faces", {"faces": [target]})
        imported = grid(client)
        client.call("remove_terrain", {})
        client.call("undo", {})
        assert grid(client) == imported
        source = args.output / "query.vmf"
        client.call("save_map", {"path": str(source)})
        (args.output / "results.json").write_text(
            json.dumps(
                {
                    "ray_point": hit["point"],
                    "bounds": bounds,
                    "pixel_pick": picked,
                    "draft_frame_matches_committed": True,
                    "queries_preserve_history": True,
                    "entity_cleanup_undo": True,
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
        "--output", type=Path, default=Path("/tmp/hammerr-terrain-queries")
    )
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    validate(args)
    print(json.dumps({"terrain_queries": True, "output": str(args.output)}))


if __name__ == "__main__":
    main()
