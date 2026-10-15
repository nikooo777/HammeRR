import argparse
import json
import math
from pathlib import Path

from validate_authored_planes import records
from validate_face_lighting import DEFAULT_BINARY, Client, detail, face_ref
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
                {"faces": [face], "power": power, "start_corner": index},
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
        targets = [{"face": faces[index], "triangles": [0, 1]} for index in [0, 2]]
        selected = client.call("highlight_terrain_triangles", {"targets": targets})
        mask = client.call("get_terrain_vertex_mask", {"token": selected["token"]})
        assert mask["vertex_count"] == 8 and len(mask["faces"]) == 2
        masks = {
            (face["solid"], face["side"]): set(face["vertices"])
            for face in mask["faces"]
        }
        history = client.call("get_map_info")["undo"]
        arguments = {
            "mode": "raise",
            "center": [0, 0, 16],
            "radius": 8192,
            "amount": 16,
            "mask_token": mask["token"],
        }
        draft = client.call("preview_terrain_sculpt", arguments)
        preview = client.call("get_terrain_grids", {"preview": True})
        for old, new in zip(original["faces"], preview["faces"], strict=True):
            vertices = masks.get((old["solid"], old["side"]), set())
            for index, (a, b) in enumerate(
                zip(old["positions"], new["positions"], strict=True)
            ):
                assert (a != b) == (index in vertices)
            assert old["alphas"] == new["alphas"] and old["flags"] == new["flags"]
        assert (
            client.call("get_terrain_grids", {}) == original
            and client.call("get_map_info")["undo"] == history
        )
        client.call("cancel_face_preview", {"token": draft["state"]["preview_token"]})
        commit(client, client.call("preview_terrain_sculpt", arguments))
        assert client.call("get_terrain_vertex_mask", {"token": mask["token"]}) == mask
        raised = client.call("get_terrain_grids", {})
        filled = client.call(
            "preview_terrain_blend",
            {"mode": "fill", "value": 200, "mask_token": mask["token"]},
        )
        blend = client.call("get_terrain_grids", {"preview": True})
        for old, new in zip(raised["faces"], blend["faces"], strict=True):
            vertices = masks.get((old["solid"], old["side"]), set())
            assert old["positions"] == new["positions"]
            assert new["alphas"] == [
                200 if index in vertices else 0 for index in range(len(old["alphas"]))
            ]
        commit(client, filled)
        assert client.call("get_terrain_vertex_mask", {"token": mask["token"]}) == mask
        client.render(
            args.output / "masked.png",
            camera={"position": [-220, -210, 180], "look_at": [0, 0, 16]},
        )
        bsp, stats = build(client, args.output, "masked")
        infos = records(bsp.read_bytes(), 26, "<3f4ifi2H2i48s40s10I")
        native = records(bsp.read_bytes(), 33, "<5f")
        assert stats["displacement_neighbor_pairs"] == 0
        assert sum(vertex[4] == 200 for vertex in native) == 8
        assert sum(vertex[3] > 0 for vertex in native) == 8
        for info, grid in zip(infos, blend["faces"], strict=True):
            start, size = info[3], grid["grid_size"]
            for index, vertex in enumerate(native[start : start + size * size]):
                assert math.isclose(
                    vertex[3], grid["positions"][index][2] - 16, abs_tol=1e-5
                )
        source = args.output / "masked.vmf"
        client.call("save_map", {"path": str(source)})
        client.call("open_map", {"path": str(source)})
        client.call("select_faces", {"faces": faces})
        assert client.call("get_terrain_grids", {}) == blend
        client.call("get_terrain_vertex_mask", {}, error=True)
        current = client.call("highlight_terrain_triangles", {"targets": targets})
        draft = client.call(
            "preview_terrain_sculpt", {**arguments, "mask_token": current["token"]}
        )
        replacement = client.call(
            "highlight_terrain_triangles", {"targets": [targets[0]]}
        )
        client.call(
            "preview_terrain_sculpt",
            {**arguments, "mask_token": current["token"]},
            error=True,
        )
        assert (
            client.call("get_face_selection")["preview_token"]
            == draft["state"]["preview_token"]
        )
        client.call("cancel_face_preview", {"token": draft["state"]["preview_token"]})
        client.call(
            "preview_terrain_blend",
            {"mode": "invert", "mask_token": replacement["token"]},
        )
        client.call("cancel_face_preview", {})
        assert client.call("get_terrain_grids", {}) == blend
        args.output.joinpath("results.json").write_text(
            json.dumps(
                {
                    "vertex_count": 8,
                    "selected_destinations_only": True,
                    "other_faces_unchanged": True,
                    "source_history_preserved": True,
                    "reusable_mask_after_apply": True,
                    "replaced_token_preserves_draft": True,
                    "save_reopen": True,
                    "native_geometry_alpha": True,
                    "exact_both_builds": True,
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
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-terrain-vertex-masks")
    )
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    validate(args)
    print(json.dumps({"terrain_vertex_masks": True, "output": str(args.output)}))


if __name__ == "__main__":
    main()
