import argparse
import json
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client, detail, face_ref


def begin(client, point, faces):
    return client.call(
        "begin_paint_selection",
        {
            "pointer": {"position": point, "width": 800, "height": 600},
            "radius": 6,
            "mode": "replace",
            "selection_mode": "faces" if faces else "objects",
        },
    )["preview"]


def validate(args):
    client = Client(None, args.binary, args.output)
    try:
        results = []
        for power in [2, 3, 4]:
            client.call("new_map", {"discard_changes": True})
            parent = client.call(
                "create_block",
                {
                    "mins": [-64, -64, 0],
                    "maxs": [64, 64, 16],
                    "material": "dev/dev_measuregeneric01",
                },
            )["id"]
            shape = detail(client, [parent])[0]
            face = face_ref(
                shape, next(face for face in shape["faces"] if face["normal"][2] > 0.99)
            )
            client.call("create_terrain", {"faces": [face], "power": power})
            client.call("select_faces", {"faces": [face]})
            draft = client.call(
                "preview_terrain_sculpt",
                {"mode": "raise", "center": [0, 0, 16], "radius": 8192, "amount": 64},
            )
            client.call(
                "commit_face_preview", {"token": draft["state"]["preview_token"]}
            )
            source = args.output / f"power-{power}.vmf"
            client.call("save_map", {"path": str(source)})
            saved = source.read_bytes()
            history = client.call("get_map_info")["undo"]
            client.call(
                "set_camera", {"position": [0, -180, 200], "look_at": [0, 0, 80]}
            )
            ray = client.call(
                "pick_at_pixel", {"x": 399.5, "y": 299.5, "width": 800, "height": 600}
            )
            assert ray["side"] == face["side"] and ray["point"][2] > 79.9
            for faces in [False, True]:
                stroke = begin(client, [400, 300], faces)
                assert stroke["face_count" if faces else "object_count"] == 1, stroke
                assert (
                    stroke["faces"] == [face]
                    if faces
                    else stroke["objects"] == [parent]
                )
                assert (
                    client.call("get_map_info")["undo"] == history
                    and source.read_bytes() == saved
                )
                if power == 3 and faces:
                    client.render(args.output / "painted-sheet.png")
                client.call("cancel_paint_selection", {"token": stroke["token"]})
            client.call("set_camera", {"position": [0, -180, 32], "look_at": [0, 0, 8]})
            stroke = begin(client, [400, 300], True)
            assert stroke["face_count"] == 0 and stroke["object_count"] == 0, stroke
            client.call("cancel_paint_selection", {"token": stroke["token"]})
            owner = client.call(
                "tie_to_entity", {"ids": [parent], "classname": "func_detail"}
            )["id"]
            client.call(
                "set_camera", {"position": [0, -180, 200], "look_at": [0, 0, 80]}
            )
            stroke = begin(client, [400, 300], False)
            assert stroke["objects"] == [owner]
            client.call("commit_paint_selection", {"token": stroke["token"]})
            assert client.call("get_map_info")["selection"] == [owner]
            client.call("set_object_visibility", {"action": "hide", "ids": [owner]})
            stroke = begin(client, [400, 300], True)
            assert stroke["face_count"] == 0
            client.call("cancel_paint_selection", {"token": stroke["token"]})
            results.append(
                {
                    "power": power,
                    "direct_and_indexed_match": True,
                    "no_parent_ghost": True,
                    "owner_commit": True,
                    "hidden_owner": True,
                    "source_history_preserved": True,
                }
            )
        args.output.joinpath("results.json").write_text(
            json.dumps(results, indent=2) + "\n"
        )
    finally:
        client.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-indexed-terrain")
    )
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    validate(args)
    print(json.dumps({"indexed_terrain": True, "output": str(args.output)}))


if __name__ == "__main__":
    main()
