import argparse
import json
import math
from pathlib import Path

from validate_authored_planes import records
from validate_face_lighting import ROOT, Client, detail, face_ref
from validate_terrain_lod import build
from validate_visibility import ROOM


def commit(client, draft):
    client.call("commit_face_preview", {"token": draft["state"]["preview_token"]})


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
                {"faces": [face], "power": power, "start_corner": index + 1},
            )
            faces.append(face)
        client.call(
            "create_entity",
            {"classname": "info_player_terrorist", "origin": [-192, -176, 8]},
        )
        client.call(
            "create_entity",
            {
                "classname": "light",
                "origin": [0, 0, 180],
                "properties": {"_light": "255 255 255 200", "_constant_attn": "1"},
            },
        )
        client.call("select_faces", {"faces": faces})
        client.call(
            "set_compile_settings", {"lighting": "direct_ldr", "physics": "engine"}
        )
        original = client.call("get_terrain_grids", {})
        rows = client.call("get_terrain_vertices", {})["faces"]
        targets = []
        for grid, row in zip(original["faces"], rows, strict=True):
            size = row["size"]
            assert len(row["vertices"]) == size * size
            for vertex in row["vertices"]:
                assert vertex["id"] == vertex["row"] * size + vertex["column"]
                assert vertex["position"] == grid["positions"][vertex["id"]]
            middle = row["vertices"][size + 2]
            point = middle["position"]
            hit = client.call(
                "pick_terrain_vertex",
                {
                    "origin": [point[0] + 0.01, point[1] + 0.01, 128],
                    "direction": [0, 0, -3],
                    "faces": faces,
                },
            )["hit"]
            assert hit["vertex"] == middle["id"] and hit["position"] == point
            targets.append(
                {
                    "face": {"solid": row["solid"], "side": row["side"]},
                    "vertices": [middle["id"], middle["id"]],
                }
            )
        history = client.call("get_map_info")["undo"]
        selected = client.call("highlight_terrain_vertices", {"targets": targets})
        assert (
            selected["kind"] == "vertices"
            and len(selected["vertices"]) == 3
            and not selected["triangles"]
        )
        for arguments in [
            {"targets": targets, "typo": True},
            {"targets": [targets[0], targets[0]]},
            {"targets": [{"face": faces[0], "vertices": [25]}]},
        ]:
            client.call("highlight_terrain_vertices", arguments, error=True)
            assert client.call("get_terrain_triangle_highlight") == selected
        mask = client.call("get_terrain_vertex_mask", {"token": selected["token"]})
        assert mask["vertex_count"] == 3 and all(
            face["triangle_count"] == 0 for face in mask["faces"]
        )
        client.call("frame_terrain_vertices", {"targets": targets, "aspect": 1.5})
        arguments = {
            "mode": "raise",
            "center": [0, 0, 16],
            "radius": 8192,
            "amount": 24,
            "mask_token": mask["token"],
        }
        draft = client.call("preview_terrain_sculpt", arguments)
        preview = client.call("get_terrain_grids", {"preview": True})
        for old, new, target in zip(
            original["faces"], preview["faces"], targets, strict=True
        ):
            for index, (a, b) in enumerate(
                zip(old["positions"], new["positions"], strict=True)
            ):
                assert (a != b) == (index in target["vertices"])
            assert old["alphas"] == new["alphas"] and old["flags"] == new["flags"]
        assert (
            client.call("get_terrain_grids", {}) == original
            and client.call("get_map_info")["undo"] == history
        )
        assert not client.call("get_terrain_triangle_highlight")["stale"]
        client.call("cancel_face_preview", {"token": draft["state"]["preview_token"]})
        assert client.call("get_terrain_grids", {}) == original
        commit(client, client.call("preview_terrain_sculpt", arguments))
        assert client.call("get_terrain_vertex_mask", {"token": mask["token"]}) == mask
        fill = client.call(
            "preview_terrain_blend",
            {"mode": "fill", "value": 200, "mask_token": mask["token"]},
        )
        blended = client.call("get_terrain_grids", {"preview": True})
        for grid in blended["faces"]:
            assert sum(alpha == 200 for alpha in grid["alphas"]) == 1
        commit(client, fill)
        client.render(
            args.output / "selected-vertices.png",
            camera={"position": [-220, -210, 180], "look_at": [0, 0, 32]},
        )
        bsp, stats = build(client, args.output, "vertices")
        infos = records(bsp.read_bytes(), 26, "<3f4ifi2H2i48s40s10I")
        native = records(bsp.read_bytes(), 33, "<5f")
        assert sum(vertex[4] == 200 for vertex in native) == 3
        assert sum(vertex[3] > 0 for vertex in native) == 3
        for info, grid in zip(infos, blended["faces"], strict=True):
            size = grid["grid_size"]
            for index, vertex in enumerate(native[info[3] : info[3] + size * size]):
                assert math.isclose(
                    vertex[3], grid["positions"][index][2] - 16, abs_tol=1e-5
                )
        client.call("undo")
        client.call("get_terrain_vertex_mask", {}, error=True)
        client.call("redo")
        assert client.call("get_terrain_grids", {}) == blended
        point = blended["faces"][1]["positions"][40]
        volume_arguments = {
            "mins": [value - 0.1 for value in point],
            "maxs": [value + 0.1 for value in point],
            "preview": True,
        }
        volume = client.call("select_volume", volume_arguments)["preview"]
        query = client.call(
            "get_terrain_volume_vertices", {"volume_token": volume["token"]}
        )
        assert query["targets"] == [{"face": faces[1], "vertices": [40]}]
        contained = client.call(
            "select_volume", {**volume_arguments, "rule": "contained"}
        )["preview"]
        assert (
            client.call(
                "get_terrain_volume_vertices", {"volume_token": contained["token"]}
            )
            == query
        )
        chosen = client.call(
            "select_terrain_vertices_in_volume", {"volume_token": contained["token"]}
        )
        assert chosen["targets"] == query["targets"]
        client.call(
            "select_terrain_vertices_in_volume",
            {"volume_token": volume["token"]},
            error=True,
        )
        assert client.call("get_terrain_triangle_highlight") == chosen["highlight"]
        draft = client.call(
            "preview_terrain_resolution", {"faces": [faces[1]], "power": 4}
        )
        assert client.call("get_terrain_triangle_highlight")["stale"]
        client.call("get_terrain_vertex_mask", {}, error=True)
        client.call("cancel_face_preview", {"token": draft["state"]["preview_token"]})
        current = client.call("highlight_terrain_vertices", {"targets": targets})
        draft = client.call(
            "preview_terrain_blend", {"mode": "invert", "mask_token": current["token"]}
        )
        replacement = client.call(
            "highlight_terrain_triangles",
            {"targets": [{"face": faces[0], "triangles": [0]}]},
        )
        client.call(
            "clear_terrain_triangle_highlight", {"token": current["token"]}, error=True
        )
        client.call(
            "preview_terrain_blend",
            {"mode": "invert", "mask_token": current["token"]},
            error=True,
        )
        assert (
            client.call("get_face_selection")["preview_token"]
            == draft["state"]["preview_token"]
        )
        assert client.call("get_terrain_triangle_highlight") == replacement
        client.call("cancel_face_preview", {"token": draft["state"]["preview_token"]})
        source = args.output / "vertices.vmf"
        client.call("save_map", {"path": str(source)})
        client.call("open_map", {"path": str(source)})
        client.call("select_faces", {"faces": faces})
        assert client.call("get_terrain_grids", {}) == blended
        client.call("get_terrain_vertex_mask", {}, error=True)
        args.output.joinpath("results.json").write_text(
            json.dumps(
                {
                    "exact_vertex_masks": True,
                    "powers": [2, 3, 4],
                    "cancel_undo_redo_reopen": True,
                    "guarded_volume_selection": True,
                    "replaced_token_preserves_foreign_state": True,
                    "exact_both_builds": True,
                    "native_modified_vertices": 3,
                    "stats": stats,
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
        "--output", type=Path, default=Path("/tmp/hammerr-terrain-vertices")
    )
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    validate(args)
    print(json.dumps({"terrain_vertices": True, "output": str(args.output)}))


if __name__ == "__main__":
    main()
