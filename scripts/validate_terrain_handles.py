import argparse
import json
import math
from pathlib import Path

from validate_face_lighting import ROOT, Client, detail, face_ref


def assert_move(before, after, targets, offset):
    for old, new, target in zip(before["faces"], after["faces"], targets, strict=True):
        for index, (a, b) in enumerate(
            zip(old["positions"], new["positions"], strict=True)
        ):
            delta = offset if index in target["vertices"] else [0, 0, 0]
            assert all(
                math.isclose(value + change, result, abs_tol=1e-5)
                for value, change, result in zip(a, delta, b, strict=True)
            )
        assert old["alphas"] == new["alphas"] and old["flags"] == new["flags"]


def validate(args):
    client = Client(None, args.binary, args.output)
    try:
        faces, targets = [], []
        for index, power in enumerate([2, 3, 4]):
            parent = client.call(
                "create_block",
                {
                    "mins": [index * 144, 0, 0],
                    "maxs": [index * 144 + 128, 128, 16],
                    "material": "dev/dev_measuregeneric01",
                },
            )["id"]
            shape = detail(client, [parent])[0]
            face = face_ref(
                shape, next(row for row in shape["faces"] if row["normal"][2] > 0.99)
            )
            client.call(
                "create_terrain",
                {"faces": [face], "power": power, "start_corner": index + 1},
            )
            faces.append(face)
            targets.append({"face": face, "vertices": [(1 << power) + 3]})
        client.call("select_faces", {"faces": faces})
        selected = client.call("highlight_terrain_vertices", {"targets": targets})
        client.call(
            "set_camera", {"position": [-160, -280, 260], "look_at": [200, 64, 16]}
        )
        before = client.call("get_terrain_grids")
        history = client.call("get_map_info")["undo"]
        for handle in ["x", "y", "z", "xy", "xz", "yz", "view"]:
            enabled = client.call(
                "enable_terrain_move_handles", {"mask_token": selected["token"]}
            )
            token = enabled["token"]
            client.call(
                "begin_terrain_move_drag",
                {"token": token, "handle": "uniform"},
                error=True,
            )
            client.call("begin_terrain_move_drag", {"token": token, "handle": handle})
            offset = [19, -19 if len(handle) > 1 else 0]
            updated = client.call(
                "update_terrain_move_drag", {"token": token, "offset": offset}
            )
            preview = client.call("get_terrain_grids", {"preview": True})
            assert_move(before, preview, targets, updated["handles"]["offset"])
            assert client.call("get_terrain_grids") == before
            assert client.call("get_map_info")["undo"] == history
            for invalid in [
                {"token": token, "offset": [9000, 0]},
                {"token": token, "offset": [1, 0], "typo": True},
                {"token": "99999", "offset": [1, 0]},
            ]:
                client.call("update_terrain_move_drag", invalid, error=True)
                assert (
                    client.call("get_face_selection")["preview_token"]
                    == updated["state"]["preview_token"]
                )
            client.call("finish_terrain_move_drag", {"token": token}, error=True)
            recovered = client.call(
                "update_terrain_move_drag", {"token": token, "offset": offset}
            )
            assert (
                recovered["state"]["preview_token"] == updated["state"]["preview_token"]
            )
            client.call("cancel_terrain_move_handles", {"token": token})
            assert client.call("get_terrain_grids", {"preview": True}) == before

        client.call(
            "set_camera",
            {"projection": "front", "center": [200, 64, 16], "ortho_height": 320},
        )
        token = client.call(
            "enable_terrain_move_handles", {"mask_token": selected["token"]}
        )["token"]
        query = client.call("get_terrain_move_handles", {"width": 1000, "height": 800})
        shape = next(shape for shape in query["handles"] if shape["handle"] == "z")
        segment = shape["pixel_segments"][0]
        point = [(a + b) / 2 for a, b in zip(*segment, strict=True)]
        assert (
            client.call(
                "get_terrain_move_handles",
                {"width": 1000, "height": 800, "point": point},
            )["picked"]
            == "z"
        )
        pointer = {"position": point, "width": 1000, "height": 800}
        client.call(
            "begin_terrain_move_drag",
            {"token": token, "handle": "z", "pointer": pointer},
        )
        end = {**pointer, "position": [point[0], point[1] - 40]}
        updated = client.call(
            "update_terrain_move_drag", {"token": token, "pointer": end}
        )
        assert updated["handles"]["offset"] == [0, 0, 16]
        client.call(
            "update_terrain_move_drag",
            {"token": token, "pointer": {**end, "width": 900}},
            error=True,
        )
        client.call("update_terrain_move_drag", {"token": token, "pointer": end})
        preview = client.call("get_terrain_grids", {"preview": True})
        assert_move(before, preview, targets, [0, 0, 16])
        client.render(
            args.output / "terrain-move-handles.png",
            camera={
                "projection": "perspective",
                "position": [-130, -250, 240],
                "look_at": [200, 64, 16],
            },
        )
        finished = client.call("finish_terrain_move_drag", {"token": token})
        client.call(
            "commit_face_preview", {"token": finished["state"]["preview_token"]}
        )
        assert client.call("get_terrain_grids") == preview
        assert client.call("get_map_info")["undo"][1:] == history[:9]
        assert (
            client.call("get_terrain_vertex_mask", {"token": selected["token"]})[
                "vertex_count"
            ]
            == 3
        )
        client.call("undo")
        assert client.call("get_terrain_grids") == before
        client.call("redo")
        assert client.call("get_terrain_grids") == preview

        selected = client.call("highlight_terrain_vertices", {"targets": targets})
        token = client.call(
            "enable_terrain_move_handles", {"mask_token": selected["token"]}
        )["token"]
        client.call("begin_terrain_move_drag", {"token": token, "handle": "z"})
        client.call("update_terrain_move_drag", {"token": token, "offset": [16, 0]})
        foreign = client.call("preview_terrain_resolution", {"power": 3})
        assert client.call("get_terrain_move_handles")["handles"] == []
        client.call("finish_terrain_move_drag", {"token": token}, error=True)
        client.call("cancel_terrain_move_handles", {"token": token})
        assert (
            client.call("get_face_selection")["preview_token"]
            == foreign["state"]["preview_token"]
        )
        client.call("cancel_face_preview", {"token": foreign["state"]["preview_token"]})
        path = args.output / "terrain-handles.vmf"
        client.call("save_map", {"path": str(path)})
        client.call("open_map", {"path": str(path)})
        assert client.call("get_terrain_grids", {"faces": faces}) == preview
        (args.output / "validation.json").write_text(
            json.dumps(
                {
                    "handles": True,
                    "faces": faces,
                    "powers": [2, 3, 4],
                    "pointer_offset": [0, 0, 16],
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
        "--output", type=Path, default=Path("/tmp/hammerr-terrain-handles")
    )
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    validate(args)
    print(json.dumps({"terrain_handles": True, "output": str(args.output)}))


if __name__ == "__main__":
    main()
