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


def finish(client, token):
    return client.call("finish_terrain_stroke", {"token": token})


def begin(client, brush):
    return client.call("begin_terrain_stroke", brush)["token"]


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
        commit(
            client,
            client.call(
                "preview_terrain_sculpt",
                {
                    "mode": "noise",
                    "center": [0, 0, 16],
                    "radius": 8192,
                    "amount": 8,
                    "seed": 37,
                },
            ),
        )
        original = client.call("get_terrain_grids", {})
        history = client.call("get_map_info")["undo"]
        points = [[x, 0, 16] for x in [-160, -88, -16, 56, 128, 192]]
        for mode in ["raise", "flatten", "smooth", "noise", "paint"]:
            key = "blend" if mode == "paint" else "sculpt"
            brush = {key: {"mode": mode, "center": [0, 0, 48], "radius": 32}}
            if mode in ["raise", "noise"]:
                brush[key]["amount"] = 2
            if mode == "paint":
                brush[key].update({"value": 200, "strength": 0.8})
            token = begin(client, brush)
            result = client.call(
                "update_terrain_stroke", {"token": token, "points": points}
            )
            assert result["stroke"]["samples"] == 45
            whole = client.call("get_terrain_grids", {"preview": True})
            state = client.call("get_terrain_stroke")
            assert client.call("get_terrain_grids", {}) == original
            assert client.call("get_map_info")["undo"] == history
            for invalid in [
                {"token": token, "points": []},
                {"token": token, "points": [[0, 0, 16]] * 65},
                {"token": token, "points": [[4096, 0, 16]]},
                {"token": token, "points": [[0, 0, 16]], "typo": True},
            ]:
                client.call("update_terrain_stroke", invalid, error=True)
                assert client.call("get_terrain_stroke") == state
                assert client.call("get_terrain_grids", {"preview": True}) == whole
            client.call(
                "update_terrain_stroke", {"token": token, "points": [[192, 0, 100]]}
            )
            assert client.call("get_terrain_grids", {"preview": True}) == whole
            assert client.call("get_terrain_stroke")["samples"] == 45
            if mode == "raise":
                client.call(
                    "update_terrain_stroke",
                    {"token": token, "origin": [192, 0, 128], "direction": [0, 0, -1]},
                )
                assert client.call("get_terrain_grids", {"preview": True}) == whole
                client.render(
                    args.output / "stroke-guide.png",
                    camera={"position": [-220, -210, 180], "look_at": [0, 0, 32]},
                )
            client.call("cancel_terrain_stroke", {"token": token})
            assert client.call("get_terrain_grids", {}) == original
            token = begin(client, brush)
            for point in points:
                client.call(
                    "update_terrain_stroke", {"token": token, "points": [point]}
                )
            assert client.call("get_terrain_grids", {"preview": True}) == whole
            ready = finish(client, token)
            client.call(
                "cancel_face_preview", {"token": ready["state"]["preview_token"]}
            )
            assert client.call("get_terrain_grids", {}) == original
        client.call("begin_terrain_stroke", {"blend": {"mode": "fill"}}, error=True)
        first = begin(
            client,
            {
                "sculpt": {
                    "mode": "raise",
                    "center": [0, 0, 16],
                    "radius": 32,
                    "amount": 2,
                    "faces": [faces[0]],
                }
            },
        )
        second = begin(
            client,
            {
                "sculpt": {
                    "mode": "raise",
                    "center": [0, 0, 16],
                    "radius": 32,
                    "amount": 2,
                    "faces": [faces[0]],
                }
            },
        )
        state = client.call("get_terrain_stroke")
        client.call("cancel_terrain_stroke", {"token": first}, error=True)
        assert client.call("get_terrain_stroke") == state
        ray = {"origin": [-128, 0, 128], "direction": [0, 0, -3]}
        client.call("update_terrain_stroke", {"token": second, **ray})
        client.call("update_terrain_stroke", {"token": second, **ray})
        assert client.call("get_terrain_stroke")["samples"] == 1
        client.call(
            "update_terrain_stroke",
            {"token": second, "origin": [300, 0, 128], "direction": [0, 0, -1]},
            error=True,
        )
        assert client.call("get_terrain_stroke")["cursor"] is None
        client.call("update_terrain_stroke", {"token": second, **ray})
        assert client.call("get_terrain_stroke")["samples"] == 2
        client.call("break_terrain_stroke", {"token": second})
        assert client.call("get_terrain_stroke")["cursor"] is None
        client.call("cancel_terrain_stroke", {"token": second})
        targets = [
            {"face": face, "vertices": [size * size // 2]}
            for face, size in zip(faces, [5, 9, 17], strict=True)
        ]
        selected = client.call("highlight_terrain_vertices", {"targets": targets})
        mask = client.call("get_terrain_vertex_mask", {"token": selected["token"]})
        token = begin(
            client,
            {
                "sculpt": {
                    "mode": "raise",
                    "center": [0, 0, 16],
                    "radius": 32,
                    "amount": 2,
                    "mask_token": mask["token"],
                }
            },
        )
        client.call("update_terrain_stroke", {"token": token, "points": points})
        raised = client.call("get_terrain_grids", {"preview": True})
        for old, new, target in zip(
            original["faces"], raised["faces"], targets, strict=True
        ):
            for index, (a, b) in enumerate(
                zip(old["positions"], new["positions"], strict=True)
            ):
                assert (a != b) == (index in target["vertices"])
            assert old["alphas"] == new["alphas"]
        commit(client, finish(client, token))
        assert client.call("get_terrain_vertex_mask", {"token": mask["token"]}) == mask
        assert client.call("get_map_info")["undo"][1:] == history[:9]
        client.call("undo")
        assert client.call("get_terrain_grids", {}) == original
        client.call("redo")
        assert client.call("get_terrain_grids", {}) == raised
        selected = client.call("highlight_terrain_vertices", {"targets": targets})
        token = begin(
            client,
            {
                "blend": {
                    "mode": "paint",
                    "center": [0, 0, 16],
                    "radius": 32,
                    "value": 200,
                    "strength": 0.8,
                    "mask_token": selected["token"],
                }
            },
        )
        client.call("update_terrain_stroke", {"token": token, "points": points})
        blended = client.call("get_terrain_grids", {"preview": True})
        for old, new, target in zip(
            raised["faces"], blended["faces"], targets, strict=True
        ):
            assert old["positions"] == new["positions"]
            for index, alpha in enumerate(new["alphas"]):
                assert (alpha != 0) == (index in target["vertices"])
        commit(client, finish(client, token))
        client.call(
            "set_compile_settings", {"lighting": "direct_ldr", "physics": "engine"}
        )
        bsp, stats = build(client, args.output, "stroke")
        infos = records(bsp.read_bytes(), 26, "<3f4ifi2H2i48s40s10I")
        vertices = records(bsp.read_bytes(), 33, "<5f")
        inspected = client.call("get_terrain_vertices", {})["faces"]
        for info, grid, row in zip(infos, blended["faces"], inspected, strict=True):
            count = grid["grid_size"] ** 2
            for index, native in enumerate(vertices[info[3] : info[3] + count]):
                for axis in range(3):
                    assert math.isclose(
                        row["vertices"][index]["base_position"][axis]
                        + native[axis] * native[3],
                        grid["positions"][index][axis],
                        abs_tol=1e-5,
                    )
                assert math.isclose(native[4], grid["alphas"][index], abs_tol=1e-5)
        token = begin(
            client,
            {
                "sculpt": {
                    "mode": "raise",
                    "center": [0, 0, 16],
                    "radius": 32,
                    "amount": 2,
                }
            },
        )
        client.call("update_terrain_stroke", {"token": token, "points": points})
        foreign = client.call("preview_terrain_blend", {"mode": "fill", "value": 128})
        client.call("finish_terrain_stroke", {"token": token}, error=True)
        client.call("cancel_terrain_stroke", {"token": token})
        assert (
            client.call("get_face_selection")["preview_token"]
            == foreign["state"]["preview_token"]
        )
        client.call("cancel_face_preview", {"token": foreign["state"]["preview_token"]})
        source = args.output / "stroke.vmf"
        client.call("save_map", {"path": str(source)})
        client.call("open_map", {"path": str(source)})
        client.call("select_faces", {"faces": faces})
        assert client.call("get_terrain_grids", {}) == blended
        args.output.joinpath("results.json").write_text(
            json.dumps(
                {
                    "grouped_paths_exact": True,
                    "modes": ["raise", "flatten", "smooth", "noise", "paint"],
                    "powers": [2, 3, 4],
                    "stationary_dabs_stable": True,
                    "miss_breaks_path": True,
                    "atomic_invalid_updates": True,
                    "owned_cancel_and_foreign_preservation": True,
                    "masked_destinations_only": True,
                    "one_undo_step": True,
                    "exact_both_builds": True,
                    "native_sample_agreement": True,
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
        "--output", type=Path, default=Path("/tmp/hammerr-terrain-strokes")
    )
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    validate(args)
    print(json.dumps({"terrain_strokes": True, "output": str(args.output)}))


if __name__ == "__main__":
    main()
