import argparse
import json
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client, detail, face_ref


def validate(args):
    client = Client(None, args.binary, args.output)
    try:
        faces, parents = [], []
        for index, power in enumerate([2, 3, 4]):
            left = -192 + index * 144
            parent = client.call(
                "create_block",
                {
                    "mins": [left, -64, 0],
                    "maxs": [left + 128, 64, 16],
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
            parents.append(parent)
        client.call("select_faces", {"faces": faces})
        source = args.output / "source.vmf"
        client.call("save_map", {"path": str(source)})
        saved = source.read_bytes()
        history = client.call("get_map_info")["undo"]
        rows = client.call("get_terrain_triangles", {})["faces"]
        targets = []
        for face, row in zip(faces, rows, strict=True):
            triangles = row["triangles"]
            for index in [0, 7, len(triangles) // 2, len(triangles) - 1]:
                p = triangles[index]["centroid"]
                hit = client.call(
                    "pick_terrain_triangle",
                    {"origin": [p[0], p[1], p[2] + 64], "direction": [0, 0, -2]},
                )["hit"]
                assert (
                    hit["solid"] == face["solid"]
                    and hit["side"] == face["side"]
                    and hit["triangle"] == index
                )
                assert (
                    all(abs(a - b) < 1e-9 for a, b in zip(hit["point"], p, strict=True))
                    and abs(hit["distance"] - 64) < 1e-9
                ), (hit, p)
            targets.append({"face": face, "triangles": [0, 7, len(triangles) - 1]})
        camera = {"position": [-340, -300, 260], "look_at": [0, 0, 16]}
        baseline = client.render(args.output / "baseline.png", camera=camera)
        highlight = client.call("highlight_terrain_triangles", {"targets": targets})
        assert not highlight["stale"] and len(highlight["triangles"]) == 9
        marked = client.render(args.output / "highlighted.png", camera=camera)
        assert marked != baseline
        client.call(
            "highlight_terrain_triangles",
            {"targets": [{"face": faces[0], "triangles": [32]}]},
            error=True,
        )
        assert client.call("get_terrain_triangle_highlight") == highlight
        assert client.render(args.output / "preserved.png", camera=camera) == marked
        old_camera = client.call("get_map_info")["camera"]
        client.call("frame_terrain_triangles", {"targets": targets, "aspect": 1.6})
        assert client.call("get_map_info")["camera"] != old_camera
        assert (
            client.call("get_map_info")["undo"] == history
            and source.read_bytes() == saved
        )
        draft = client.call(
            "preview_terrain_sculpt",
            {"mode": "raise", "center": [0, 0, 16], "radius": 8192, "amount": 32},
        )
        preview = client.call("get_terrain_triangles", {"preview": True})["faces"]
        p = preview[0]["triangles"][7]["centroid"]
        assert p[2] > 47
        hit = client.call(
            "pick_terrain_triangle",
            {"origin": [p[0], p[1], p[2] + 64], "direction": [0, 0, -1]},
        )["hit"]
        assert hit["triangle"] == 7 and abs(hit["point"][2] - p[2]) < 1e-9
        assert not client.call("get_terrain_triangle_highlight")["stale"]
        client.render(args.output / "draft.png", camera=camera)
        client.call("cancel_face_preview", {"token": draft["state"]["preview_token"]})
        resolution = client.call(
            "preview_terrain_resolution", {"faces": [faces[0]], "power": 3}
        )
        assert client.call("get_terrain_triangle_highlight")["stale"]
        client.call("frame_terrain_triangles", {"targets": targets}, error=True)
        client.call(
            "cancel_face_preview", {"token": resolution["state"]["preview_token"]}
        )
        assert not client.call("get_terrain_triangle_highlight")["stale"]
        replacement = client.call(
            "highlight_terrain_triangles", {"targets": [targets[1]]}
        )
        client.call(
            "clear_terrain_triangle_highlight",
            {"token": highlight["token"]},
            error=True,
        )
        assert client.call("get_terrain_triangle_highlight") == replacement
        client.call("clear_terrain_triangle_highlight", {"token": replacement["token"]})
        assert client.render(args.output / "cleared.png", camera=camera) == baseline
        assert client.call("get_map_info")["undo"] == history
        assert client.call("get_terrain_triangles", {})["faces"] == rows
        p = rows[0]["triangles"][7]["centroid"]
        ray = {
            "faces": [faces[0]],
            "origin": [p[0], p[1], 128],
            "direction": [0, 0, -1],
        }
        blocker = client.call(
            "create_block",
            {
                "mins": [p[0] - 4, p[1] - 4, 32],
                "maxs": [p[0] + 4, p[1] + 4, 48],
                "material": "dev/dev_measuregeneric01",
            },
        )["id"]
        assert client.call("pick_terrain_triangle", ray)["hit"] is None
        client.call("set_object_visibility", {"action": "hide", "ids": [blocker]})
        assert client.call("pick_terrain_triangle", ray)["hit"]["triangle"] == 7
        owner = client.call(
            "tie_to_entity", {"ids": [parents[0]], "classname": "func_detail"}
        )["id"]
        client.call("set_object_visibility", {"action": "hide", "ids": [owner]})
        assert client.call("pick_terrain_triangle", ray)["hit"] is None
        client.call("set_object_visibility", {"action": "show", "ids": [owner]})
        assert client.call("pick_terrain_triangle", ray)["hit"]["triangle"] == 7
        client.call(
            "edit_faces", {"faces": [faces[0]], "material": "tools/toolsnodraw"}
        )
        client.call("set_view", {"tool_textures": False})
        assert client.call("pick_terrain_triangle", ray)["hit"] is None
        client.call("set_view", {"tool_textures": True})
        assert client.call("pick_terrain_triangle", ray)["hit"]["triangle"] == 7
        args.output.joinpath("results.json").write_text(
            json.dumps(
                {
                    "native_picks": True,
                    "preview_geometry": True,
                    "batch_highlights": True,
                    "visible_render_difference": True,
                    "owned_clear": True,
                    "stale_topology_guard": True,
                    "framing": True,
                    "source_history_preserved": True,
                    "hidden_tools_and_occlusion": True,
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
        "--output", type=Path, default=Path("/tmp/hammerr-terrain-triangle-picking")
    )
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    validate(args)
    print(json.dumps({"terrain_triangle_picking": True, "output": str(args.output)}))


if __name__ == "__main__":
    main()
