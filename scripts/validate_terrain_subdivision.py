import argparse
import json
import math
from pathlib import Path

from validate_authored_planes import records
from validate_face_lighting import DEFAULT_BINARY, Client, detail, face_ref
from validate_terrain_lod import build
from validate_visibility import ROOM


def discard(client, draft):
    client.call("cancel_face_preview", {"token": draft["state"]["preview_token"]})


def validate(args):
    client = Client(None, args.binary, args.output)
    try:
        for low, high in ROOM:
            client.call(
                "create_block",
                {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"},
            )
        faces, ordinary = [], None
        patches = (
            [
                ([-64, -64, 128], [64, 64, 144], [0, 0, 1], 2, 1),
                ([-64, -64, 16], [-48, 0, 144], [-1, 0, 0], 3, 3),
                ([-64, 0, 16], [-48, 64, 144], [-1, 0, 0], 4, 2),
            ]
            if args.split_edge
            else [
                ([-64, -64, 128], [64, 64, 144], [0, 0, 1], 2, 0),
                ([-64, -64, 16], [64, -48, 144], [0, -1, 0], 3, 1),
                ([-64, -64, 16], [-48, 64, 144], [-1, 0, 0], 4, 3),
            ]
        )
        for index, (low, high, normal, power, corner) in enumerate(patches):
            parent = client.call(
                "create_block",
                {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"},
            )["id"]
            shape = detail(client, [parent])[0]
            row = next(
                row
                for row in shape["faces"]
                if sum(a * b for a, b in zip(row["normal"], normal, strict=True)) > 0.99
            )
            face = face_ref(shape, row)
            ordinary = face_ref(
                shape,
                next(other for other in shape["faces"] if other["side"] != row["side"]),
            )
            client.call(
                "create_terrain",
                {"faces": [face], "power": power, "start_corner": corner},
            )
            filled = client.call(
                "preview_terrain_blend",
                {"faces": [face], "mode": "fill", "value": 40 + index * 60},
            )
            client.call(
                "commit_face_preview", {"token": filled["state"]["preview_token"]}
            )
            faces.append(face)
        client.call(
            "create_entity",
            {"classname": "info_player_terrorist", "origin": [-160, -160, 8]},
        )
        client.call(
            "create_entity",
            {
                "classname": "light",
                "origin": [-160, -128, 200],
                "properties": {"_light": "255 240 220 300", "_constant_attn": "1"},
            },
        )
        client.call("select_faces", {"faces": faces})
        before = client.call("get_terrain_grids")
        authored = client.call("get_terrain_vertices")["faces"]
        history = client.call("get_map_info")["undo"]
        draft = client.call("preview_terrain_subdivision")
        assert (draft["faces_changed"], draft["shared_edges"], draft["vertices"]) == (
            3,
            3,
            395,
        )
        rounded = client.call("get_terrain_grids", {"preview": True})
        assert client.call("get_terrain_grids") == before
        assert client.call("get_map_info")["undo"] == history
        joint = []
        for index, (old, new, vertices) in enumerate(
            zip(before["faces"], rounded["faces"], authored, strict=True)
        ):
            assert new["subdivided"] and not old["subdivided"]
            for key in ["power", "grid_size", "alphas", "flags"]:
                assert old[key] == new[key]
            assert old["positions"] != new["positions"]
            for vertex, point in zip(
                vertices["vertices"], new["positions"], strict=True
            ):
                base = vertex["base_position"]
                if base == ([-64, 0, 144] if args.split_edge else [-64, -64, 144]):
                    joint.append(point)
                fixed = (
                    (
                        (index == 0 and (base[0] == 64 or base[1] in [-64, 64]))
                        or (index == 1 and (base[1] == -64 or base[2] == 16))
                        or (index == 2 and (base[1] == 64 or base[2] == 16))
                    )
                    if args.split_edge
                    else (
                        (index == 0 and (base[0] == 64 or base[1] == 64))
                        or (index == 1 and (base[0] == 64 or base[2] == 16))
                        or (index == 2 and (base[1] == 64 or base[2] == 16))
                    )
                )
                if fixed:
                    assert all(
                        math.isclose(a, b, abs_tol=1e-9)
                        for a, b in zip(base, point, strict=True)
                    )
        assert len(joint) == 3
        assert all(math.dist(joint[0], point) < 1e-9 for point in joint)
        assert joint[0][0] > -64 and joint[0][1] > -64 and joint[0][2] < 144
        shared = {}
        for old, new in zip(authored, rounded["faces"], strict=True):
            for vertex, point in zip(old["vertices"], new["positions"], strict=True):
                key = tuple(vertex["base_position"])
                if key in shared:
                    assert math.dist(point, shared[key]) < 1e-9
                shared[key] = point
        for invalid in [{"typo": True}, {"faces": [*faces, ordinary]}]:
            client.call("preview_terrain_subdivision", invalid, error=True)
            assert (
                client.call("get_face_selection")["preview_token"]
                == draft["state"]["preview_token"]
            )
        discard(client, draft)
        assert client.call("get_terrain_grids") == before
        draft = client.call("preview_terrain_subdivision")
        assert client.call("get_terrain_grids", {"preview": True}) == rounded
        client.call("commit_face_preview", {"token": draft["state"]["preview_token"]})
        assert client.call("get_map_info")["undo"][1:] == history[:9]
        no_op = client.call("preview_terrain_subdivision")
        assert no_op["faces_changed"] == 0
        discard(client, no_op)
        client.render(
            args.output / "subdivided-terrain.png",
            camera={"position": [-220, -240, 210], "look_at": [-16, -16, 88]},
        )

        mask = client.call(
            "highlight_terrain_vertices",
            {"targets": [{"face": faces[0], "vertices": [12]}]},
        )
        moved = client.call(
            "preview_terrain_vertex_move",
            {"faces": [faces[0]], "mask_token": mask["token"], "offset": [0, 0, 2]},
        )
        assert client.call("get_terrain_grids", {"preview": True})["faces"][0][
            "subdivided"
        ]
        discard(client, moved)
        for tool, arguments in [
            (
                "preview_terrain_sculpt",
                {"center": [0, 0, 144], "radius": 128, "mode": "raise", "amount": 2},
            ),
            ("preview_terrain_blend", {"mode": "fill", "value": 180}),
            ("preview_terrain_sew", {}),
            ("preview_terrain_resolution", {"power": 3}),
        ]:
            draft = client.call(tool, {"faces": faces, **arguments})
            assert all(
                row["subdivided"]
                for row in client.call("get_terrain_grids", {"preview": True})["faces"]
            )
            discard(client, draft)
            assert client.call("get_terrain_grids") == rounded
        stroke = client.call(
            "begin_terrain_stroke",
            {
                "sculpt": {
                    "faces": [faces[0]],
                    "center": [0, 0, 144],
                    "radius": 128,
                    "mode": "raise",
                    "amount": 2,
                }
            },
        )
        client.call(
            "update_terrain_stroke", {"token": stroke["token"], "points": [[0, 0, 144]]}
        )
        finished = client.call("finish_terrain_stroke", {"token": stroke["token"]})
        discard(client, finished)
        client.call("undo")
        assert client.call("get_terrain_grids") == before
        client.call("redo")
        assert client.call("get_terrain_grids") == rounded

        client.call(
            "set_compile_settings", {"lighting": "direct_ldr", "physics": "engine"}
        )
        source = args.output / "subdivided.vmf"
        client.call("save_map", {"path": str(source)})
        bsp, stats = build(client, args.output, "subdivided")
        assert stats["displacements"] == 3 and stats["displacement_neighbor_pairs"] == 3
        assert stats["displacement_half_edge_pairs"] == (2 if args.split_edge else 0)
        assert stats["displacement_lod_snapped_vertices"] > 0
        infos = records(bsp.read_bytes(), 26, "<3f4ifi2H2i48s40s10I")
        native = records(bsp.read_bytes(), 33, "<5f")
        inspected = client.call("get_terrain_vertices")["faces"]
        checked = 0
        for info, grid in zip(infos, inspected, strict=True):
            for index, record in enumerate(
                native[info[3] : info[3] + grid["size"] ** 2]
            ):
                vertex = grid["vertices"][index]
                assert math.isclose(vertex["alpha"], record[4], abs_tol=1e-5)
                point = [
                    a + b * record[3]
                    for a, b in zip(vertex["base_position"], record[:3], strict=True)
                ]
                assert all(
                    math.isfinite(value) and abs(value) <= 65536 for value in point
                )
                if info[15 + index // 32] & (1 << (index % 32)):
                    assert math.dist(point, vertex["position"]) < 2e-5
                    checked += 1
        assert checked > 50
        assert source.read_text().count('"subdiv" "1"') == 3
        client.call("open_map", {"path": str(source)})
        assert client.call("get_terrain_grids", {"faces": faces}) == rounded
        reopened, reopened_stats = build(client, args.output, "reopened")
        assert reopened.read_bytes() == bsp.read_bytes()
        assert {
            key: value for key, value in reopened_stats.items() if key != "elapsed_ms"
        } == {key: value for key, value in stats.items() if key != "elapsed_ms"}
        (args.output / "validation.json").write_text(
            json.dumps(
                {
                    "subdivision": True,
                    "powers": [2, 3, 4],
                    "shared_edges": 3,
                    "half_edge_pairs": 2 if args.split_edge else 0,
                    "native_active_samples": checked,
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
    parser.add_argument("--split-edge", action="store_true")
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-terrain-subdivision")
    )
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    validate(args)
    print(json.dumps({"terrain_subdivision": True, "output": str(args.output)}))


if __name__ == "__main__":
    main()
