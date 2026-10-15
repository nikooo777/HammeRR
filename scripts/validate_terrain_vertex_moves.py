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
        targets = []
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
            targets.append({"face": face, "vertices": [(1 << power) + 3]})
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
        original = client.call("get_terrain_grids", {})
        history = client.call("get_map_info")["undo"]
        selected = client.call("highlight_terrain_vertices", {"targets": targets})
        arguments = {"mask_token": selected["token"], "offset": [3.5, -2, 12]}
        zero = client.call(
            "preview_terrain_vertex_move", {**arguments, "offset": [0, 0, 0]}
        )
        assert zero["faces_changed"] == 0
        assert client.call("get_terrain_grids", {"preview": True}) == original
        client.call("cancel_face_preview", {"token": zero["state"]["preview_token"]})
        draft = client.call("preview_terrain_vertex_move", arguments)
        preview = client.call("get_terrain_grids", {"preview": True})
        for old, new, target in zip(
            original["faces"], preview["faces"], targets, strict=True
        ):
            for index, (a, b) in enumerate(
                zip(old["positions"], new["positions"], strict=True)
            ):
                expected = (
                    [
                        value + delta
                        for value, delta in zip(a, arguments["offset"], strict=True)
                    ]
                    if index in target["vertices"]
                    else a
                )
                assert b == expected
            assert old["alphas"] == new["alphas"] and old["flags"] == new["flags"]
        assert client.call("get_terrain_grids", {}) == original
        assert client.call("get_map_info")["undo"] == history
        for invalid in [
            {**arguments, "offset": [4097, 0, 0]},
            {**arguments, "typo": True},
            {**arguments, "mask_token": "99999"},
        ]:
            client.call("preview_terrain_vertex_move", invalid, error=True)
            assert (
                client.call("get_face_selection")["preview_token"]
                == draft["state"]["preview_token"]
            )
        client.call("cancel_face_preview", {"token": draft["state"]["preview_token"]})
        assert client.call("get_terrain_grids", {}) == original
        commit(client, client.call("preview_terrain_vertex_move", arguments))
        assert client.call("get_terrain_grids", {}) == preview
        assert (
            client.call("get_terrain_vertex_mask", {"token": selected["token"]})[
                "vertex_count"
            ]
            == 3
        )
        client.call("undo")
        assert client.call("get_terrain_grids", {}) == original
        client.call("redo")
        assert client.call("get_terrain_grids", {}) == preview
        client.render(
            args.output / "moved-vertices.png",
            camera={"position": [-220, -210, 180], "look_at": [0, 0, 32]},
        )
        client.call(
            "set_compile_settings", {"lighting": "direct_ldr", "physics": "engine"}
        )
        bsp, stats = build(client, args.output, "vertices")
        infos = records(bsp.read_bytes(), 26, "<3f4ifi2H2i48s40s10I")
        vertices = records(bsp.read_bytes(), 33, "<5f")
        inspected = client.call("get_terrain_vertices", {})["faces"]
        for info, row in zip(infos, inspected, strict=True):
            for index, native in enumerate(
                vertices[info[3] : info[3] + row["size"] ** 2]
            ):
                expected = row["vertices"][index]
                for axis in range(3):
                    assert math.isclose(
                        expected["base_position"][axis] + native[axis] * native[3],
                        expected["position"][axis],
                        abs_tol=1e-5,
                    )
        source = args.output / "moved.vmf"
        client.call("save_map", {"path": str(source)})
        client.call("open_map", {"path": str(source)})
        client.call("select_faces", {"faces": faces})
        assert client.call("get_terrain_grids", {}) == preview
        client.call("preview_terrain_vertex_move", arguments, error=True)
        current = client.call("highlight_terrain_vertices", {"targets": targets})
        draft = client.call(
            "preview_terrain_vertex_move", {**arguments, "mask_token": current["token"]}
        )
        replacement = client.call(
            "highlight_terrain_triangles",
            {"targets": [{"face": faces[0], "triangles": [0]}]},
        )
        client.call(
            "preview_terrain_vertex_move",
            {**arguments, "mask_token": current["token"]},
            error=True,
        )
        assert client.call("get_terrain_triangle_highlight") == replacement
        assert (
            client.call("get_face_selection")["preview_token"]
            == draft["state"]["preview_token"]
        )
        client.call("cancel_face_preview", {"token": draft["state"]["preview_token"]})
        token = client.call(
            "get_terrain_vertex_mask",
            {"faces": [faces[0]], "token": replacement["token"]},
        )["token"]
        triangle_mask = client.call(
            "preview_terrain_vertex_move",
            {"faces": [faces[0]], "mask_token": token, "offset": [0, 1, 0]},
        )
        assert triangle_mask["faces_changed"] == 1
        client.call(
            "cancel_face_preview", {"token": triangle_mask["state"]["preview_token"]}
        )
        assert client.call("get_terrain_grids", {}) == preview
        args.output.joinpath("results.json").write_text(
            json.dumps(
                {
                    "exact_world_moves": True,
                    "powers": [2, 3, 4],
                    "zero_no_op": True,
                    "unselected_samples_fixed": True,
                    "source_history_fixed_during_preview": True,
                    "cancel_undo_redo_reopen": True,
                    "mask_reuse_and_replacement": True,
                    "triangle_derived_masks": True,
                    "native_xyz_agreement": True,
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
        "--output", type=Path, default=Path("/tmp/hammerr-terrain-vertex-moves")
    )
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    validate(args)
    print(json.dumps({"terrain_vertex_moves": True, "output": str(args.output)}))


if __name__ == "__main__":
    main()
