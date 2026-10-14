import argparse
import json
import struct
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
        for power, corner, low, high, amount, alpha in [
            (2, 0, [-128, -128, 0], [0, 0, 16], 16, 40),
            (4, 2, [0, 0, 0], [128, 128, 16], 48, 200),
        ]:
            parent = client.call(
                "create_block",
                {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"},
            )["id"]
            shape = detail(client, [parent])[0]
            face = face_ref(
                shape, next(face for face in shape["faces"] if face["normal"][2] > 0.99)
            )
            client.call(
                "create_terrain",
                {"faces": [face], "power": power, "start_corner": corner},
            )
            client.call("select_faces", {"faces": [face]})
            commit(
                client,
                client.call(
                    "preview_terrain_sculpt",
                    {
                        "mode": "raise",
                        "center": [0, 0, 16],
                        "radius": 8192,
                        "amount": amount,
                    },
                ),
            )
            commit(
                client,
                client.call("preview_terrain_blend", {"mode": "fill", "value": alpha}),
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
        history = client.call("get_map_info")["undo"]
        draft = client.call("preview_terrain_sew", {"blend": True})
        assert (
            draft["neighbor_pairs"] == 0
            and draft["corner_joins"] == 1
            and draft["sewn_vertices"] == 1
            and draft["interpolated_vertices"] == 0
        )
        preview = client.call("get_terrain_grids", {"preview": True})
        for old, new in zip(original["faces"], preview["faces"], strict=True):
            count = 0
            for index, (a, b) in enumerate(
                zip(old["positions"], new["positions"], strict=True)
            ):
                if a[:2] == [0, 0]:
                    assert b == [0, 0, 48] and new["alphas"][index] == 120
                    count += 1
                else:
                    assert a == b and old["alphas"][index] == new["alphas"][index]
            assert count == 1
        assert (
            client.call("get_terrain_grids", {}) == original
            and client.call("get_map_info")["undo"] == history
        )
        client.call("cancel_face_preview", {"token": draft["state"]["preview_token"]})
        assert client.call("get_terrain_grids", {}) == original
        commit(client, client.call("preview_terrain_sew", {"blend": True}))
        assert client.call("get_terrain_grids", {}) == preview
        client.call("undo")
        assert client.call("get_terrain_grids", {}) == original
        client.call("redo")
        assert client.call("get_terrain_grids", {}) == preview
        repeat = client.call("preview_terrain_sew", {"blend": True})
        assert repeat["faces_changed"] == 0
        client.call("cancel_face_preview", {"token": repeat["state"]["preview_token"]})
        path, stats = build(client, args.output, "sewn-corners")
        assert (
            stats["displacement_neighbor_pairs"] == 0
            and stats["displacement_corner_neighbor_pairs"] == 1
        )
        infos = records(path.read_bytes(), 26, "<3f4ifi2H2i48s40s10I")
        assert len(infos) == 2
        links = []
        for index, info in enumerate(infos):
            for corner in range(4):
                neighbors = struct.unpack_from("<4HBB", info[14], corner * 10)
                links.extend(
                    (index, neighbor) for neighbor in neighbors[: neighbors[4]]
                )
        assert sorted(links) == [(0, 1), (1, 0)]
        client.render(
            args.output / "sewn-corners.png",
            camera={"position": [-220, -210, 180], "look_at": [0, 0, 48]},
        )
        source = args.output / "sewn.vmf"
        client.call("save_map", {"path": str(source)})
        client.call("open_map", {"path": str(source)})
        client.call("select_faces", {"faces": faces})
        assert client.call("get_terrain_grids", {}) == preview
        args.output.joinpath("results.json").write_text(
            json.dumps(
                {
                    "stats": stats,
                    "corner_joins": 1,
                    "unrelated_vertices_exact": True,
                    "optional_alpha_mean": 120,
                    "height_mean": 48,
                    "cancel_undo_redo_reopen": True,
                    "repeated_no_op": True,
                    "native_reciprocal_corners": True,
                    "exact_both_builds": True,
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
        "--output", type=Path, default=Path("/tmp/hammerr-terrain-corner-sewing")
    )
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    validate(args)
    print(json.dumps({"terrain_corner_sewing": True, "output": str(args.output)}))


if __name__ == "__main__":
    main()
