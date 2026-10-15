import argparse
import json
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client, detail, face_ref


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
        face = face_ref(
            shape, next(face for face in shape["faces"] if face["normal"][2] > 0.99)
        )
        client.call("create_terrain", {"faces": [face], "power": 3})
        client.call("select_faces", {"faces": [face]})
        draft = client.call(
            "preview_terrain_sculpt",
            {"mode": "raise", "center": [0, 0, 16], "radius": 8192, "amount": 64},
        )
        client.call("commit_face_preview", {"token": draft["state"]["preview_token"]})
        source = args.output / "elevated.vmf"
        client.call("save_map", {"path": str(source)})
        saved = source.read_bytes()
        history = client.call("get_map_info")["undo"]
        for mode in ["objects", "faces"]:
            for rule in ["contained", "intersecting"]:
                volume = client.call(
                    "select_volume",
                    {
                        "mins": [-65, -65, 79.9],
                        "maxs": [65, 65, 80.1],
                        "rule": rule,
                        "selection_mode": mode,
                        "preview": True,
                    },
                )["preview"]
                assert (
                    volume["object_count" if mode == "objects" else "face_count"] == 1
                )
                query = client.call(
                    "get_terrain_volume_triangles",
                    {"volume_token": volume["token"], "faces": [face]},
                )
                assert query["triangle_count"] == 128
                parent_volume = client.call(
                    "select_volume",
                    {
                        "mins": [-65, -65, -1],
                        "maxs": [65, 65, 17],
                        "rule": rule,
                        "selection_mode": mode,
                        "preview": True,
                    },
                )["preview"]
                assert (
                    parent_volume["object_count"] == 0
                    and parent_volume["face_count"] == 0
                )
                assert (
                    client.call(
                        "get_terrain_volume_triangles",
                        {"volume_token": parent_volume["token"], "faces": [face]},
                    )["triangle_count"]
                    == 0
                )
                client.call(
                    "get_terrain_volume_triangles",
                    {"volume_token": volume["token"], "faces": [face]},
                    error=True,
                )
        p = client.call("get_terrain_triangles", {"faces": [face]})["faces"][0][
            "triangles"
        ][0]["centroid"]
        box = {
            "mins": [p[0] - 2, p[1] - 2, p[2] - 1],
            "maxs": [p[0] + 2, p[1] + 2, p[2] + 1],
            "selection_mode": "faces",
            "preview": True,
        }
        contained = client.call("select_volume", {**box, "rule": "contained"})[
            "preview"
        ]
        assert (
            client.call(
                "get_terrain_volume_triangles",
                {"volume_token": contained["token"], "faces": [face]},
            )["triangle_count"]
            == 0
        )
        crossing = client.call("select_volume", {**box, "rule": "intersecting"})[
            "preview"
        ]
        picked = client.call(
            "select_terrain_triangles_in_volume",
            {"volume_token": crossing["token"], "faces": [face]},
        )
        assert picked["targets"] == [{"face": face, "triangles": [0]}]
        assert client.call("get_terrain_vertex_mask", {"faces": [face]})["faces"][0][
            "vertices"
        ] == [0, 9, 10]
        client.render(
            args.output / "volume-triangle.png",
            camera={"position": [p[0] - 40, p[1] - 40, p[2] + 50], "look_at": p},
        )
        client.call("cancel_volume_selection", {"token": crossing["token"]})
        client.call(
            "select_terrain_triangles_in_volume",
            {"volume_token": crossing["token"], "faces": [face]},
            error=True,
        )
        for mode in ["objects", "faces"]:
            client.call("set_selection_mode", {"mode": mode})
            area = client.call(
                "select_area",
                {
                    "start": [90, 238],
                    "end": [422, 274],
                    "width": 512,
                    "height": 512,
                    "camera": {
                        "projection": "front",
                        "center": [0, 0, 80],
                        "ortho_height": 256,
                    },
                    "rule": "contained",
                    "preview": True,
                },
            )
            assert len(area["matches"][mode]) == 1, area
            client.call("cancel_area_selection", {})
        assert (
            client.call("get_map_info")["undo"] == history
            and source.read_bytes() == saved
        )
        assert (
            client.call("get_terrain_grids", {"faces": [face]})["faces"][0]["power"]
            == 3
        )
        owner = client.call(
            "tie_to_entity", {"ids": [parent], "classname": "func_detail"}
        )["id"]
        client.call("set_object_visibility", {"action": "hide", "ids": [owner]})
        hidden = client.call("select_volume", {**box, "rule": "intersecting"})[
            "preview"
        ]
        assert (
            client.call(
                "get_terrain_volume_triangles",
                {"volume_token": hidden["token"], "faces": [face]},
            )["triangle_count"]
            == 0
        )
        client.call("set_object_visibility", {"action": "show", "ids": [owner]})
        client.call("edit_faces", {"faces": [face], "material": "tools/toolsnodraw"})
        client.call("set_view", {"tool_textures": False})
        hidden = client.call("select_volume", {**box, "rule": "intersecting"})[
            "preview"
        ]
        assert (
            client.call(
                "get_terrain_volume_triangles",
                {"volume_token": hidden["token"], "faces": [face]},
            )["triangle_count"]
            == 0
        )
        client.call("set_view", {"tool_textures": True})
        client.call(
            "get_terrain_volume_triangles",
            {"volume_token": hidden["token"], "faces": [face]},
            error=True,
        )
        hidden = client.call("select_volume", {**box, "rule": "intersecting"})[
            "preview"
        ]
        assert (
            client.call(
                "get_terrain_volume_triangles",
                {"volume_token": hidden["token"], "faces": [face]},
            )["triangle_count"]
            == 1
        )
        args.output.joinpath("results.json").write_text(
            json.dumps(
                {
                    "actual_surface_volume_matches": True,
                    "no_parent_ghosts": True,
                    "native_triangle_box_rules": True,
                    "atomic_selection_and_masks": True,
                    "stale_and_canceled_tokens": True,
                    "marquee_height_matching": True,
                    "source_history_preserved": True,
                    "hidden_owner_and_tools": True,
                },
                indent=2,
            )
            + "\n"
        )
    finally:
        client.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-terrain-regions")
    )
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    validate(args)
    print(json.dumps({"terrain_regions": True, "output": str(args.output)}))


if __name__ == "__main__":
    main()
