import argparse
import json
import math
import struct
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client, detail, face_ref
from validate_terrain_lod import build
from validate_visibility import ROOM


def close(actual, expected):
    assert math.isclose(actual, expected, abs_tol=1e-5), (actual, expected)


def bounds(client, entity):
    return client.call("inspect_entity_model", {"id": entity})["world_bounds"]


def validate(args):
    client = Client(None, args.binary, args.output)
    try:
        for low, high in ROOM:
            client.call("create_block", {"mins": low, "maxs": high})
        client.call(
            "create_entity",
            {"classname": "info_player_terrorist", "origin": [-192, -192, 8]},
        )
        model = "models/props_junk/wood_crate001a.mdl"
        crate = client.call(
            "create_entity",
            {
                "classname": "prop_dynamic",
                "origin": [0, 0, 96],
                "properties": {"model": model, "solid": "0", "targetname": "placed"},
            },
        )["id"]
        client.call("select", {"ids": [crate]})
        original = client.call("get_objects", {"ids": [crate]})
        history = client.call("get_map_info")["undo"]
        camera = client.call("get_camera")
        floor = client.call("preview_surface_placement", {"offset": 2})
        assert floor["valid"] and floor["plan"]["method"] == "floor"
        close(bounds(client, crate)[0][2], 2)
        assert client.call("get_objects", {"ids": [crate]}) == original
        assert client.call("get_map_info")["undo"] == history
        client.render(
            args.output / "floor.png",
            camera={"position": [-180, -220, 150], "look_at": [0, 0, 30]},
        )
        assert client.call("get_camera") == camera
        client.call("apply_surface_placement", {"token": floor["token"]})
        close(bounds(client, crate)[0][2], 2)
        client.call("undo")
        assert client.call("get_objects", {"ids": [crate]}) == original
        client.call("redo")
        close(bounds(client, crate)[0][2], 2)
        client.call("undo")

        ceiling = client.call(
            "preview_surface_placement", {"method": "ceiling", "align": True, "offset": 4}
        )
        assert ceiling["plan"]["normal"] == [0, 0, -1]
        close(bounds(client, crate)[1][2], 252)
        client.call("cancel_surface_placement", {"token": ceiling["token"]})

        good = client.call("preview_surface_placement")
        for arguments in [
            {"offset": 65537},
            {"method": "ray", "ray_origin": [1000, 1000, 1000], "ray_direction": [0, 0, 1]},
            {"method": "ray", "ray_origin": [0, 0, 100], "ray_direction": [0, 0, 0]},
            {"unknown": True},
        ]:
            client.call("preview_surface_placement", arguments, error=True)
            assert client.call("get_surface_placement")["token"] == good["token"]
        client.call("transform_objects", {"translation": [4, 0, 0], "preview": True})
        replacement = bounds(client, crate)
        client.call("apply_surface_placement", {"token": good["token"]}, error=True)
        client.call("cancel_surface_placement", {"token": good["token"]})
        assert bounds(client, crate) == replacement
        client.call("cancel_transform_preview")

        pick = client.call("begin_surface_placement_pick", {"align": True, "offset": 8})
        client.call(
            "pick_surface_placement",
            {"token": pick["token"], "origin": [1000, 1000, 1000], "direction": [0, 0, 1]},
            error=True,
        )
        assert client.call("get_surface_placement")["picking"]
        wall = client.call(
            "pick_surface_placement",
            {"token": pick["token"], "origin": [0, -180, 128], "direction": [0, 1, 0]},
        )
        assert wall["plan"]["normal"] == [0, -1, 0] and not wall["picking"]
        close(bounds(client, crate)[1][1], 248)
        close(wall["plan"]["contact_offset"], 8)
        client.render(
            args.output / "surface-placement.png",
            camera={"position": [-100, 130, 170], "look_at": [0, 240, 128]},
        )
        client.call("cancel_surface_placement", {"token": pick["token"] + 1}, error=True)
        client.call("apply_surface_placement", {"token": pick["token"]})
        placed = client.call("get_objects", {"ids": [crate]})
        vmf = args.output / "surface-placement.vmf"
        client.call("save_map", {"path": str(vmf)})
        client.call("open_map", {"path": str(vmf), "discard_changes": True})
        assert client.call("get_objects", {"ids": [crate]}) == placed
        bsp, stats = build(client, args.output, "surface-placement")
        data = bsp.read_bytes()
        offset, length = struct.unpack_from("<ii", data, 8)
        entities = data[offset : offset + length].decode()
        properties = dict(placed["objects"][0]["properties"])
        assert f'"origin" "{properties["origin"]}"' in entities
        assert f'"angles" "{properties["angles"]}"' in entities

        helper = client.call(
            "create_entity", {"classname": "info_target", "origin": [96, 0, 64]}
        )["id"]
        helper_floor = client.call("preview_surface_placement", {"ids": [helper]})
        close(helper_floor["plan"]["contact_offset"], 0)
        close(helper_floor["plan"]["translation"][2], -56)
        client.call("cancel_surface_placement", {"token": helper_floor["token"]})

        target = client.call(
            "create_entity",
            {"classname": "prop_dynamic", "origin": [128, 0, 0], "properties": {"model": model}},
        )["id"]
        model_top = bounds(client, target)[1][2]
        model_hit = client.call(
            "preview_surface_placement",
            {"ids": [helper], "method": "ray", "ray_origin": [128, 0, 200], "ray_direction": [0, 0, -1]},
        )
        assert model_hit["plan"]["target"] == target
        assert model_top - 1 < model_hit["plan"]["point"][2] <= model_top
        assert model_hit["plan"]["normal"][2] > 0.9
        client.call("cancel_surface_placement", {"token": model_hit["token"]})
        client.call("set_object_visibility", {"action": "hide", "ids": [target]})
        hidden = client.call(
            "preview_surface_placement",
            {"ids": [helper], "method": "ray", "ray_origin": [128, 0, 200], "ray_direction": [0, 0, -1]},
        )
        close(hidden["plan"]["point"][2], 0)
        client.call("cancel_surface_placement", {"token": hidden["token"]})

        terrain = client.call(
            "create_block", {"mins": [-128, -128, 0], "maxs": [0, 128, 16]}
        )["id"]
        shape = detail(client, [terrain])[0]
        face = face_ref(shape, next(face for face in shape["faces"] if face["normal"][2] > 0.99))
        client.call("select_faces", {"faces": [face]})
        client.call("create_terrain", {"power": 2})
        sculpt = client.call(
            "preview_terrain_sculpt", {"mode": "raise", "center": [-64, 0, 16], "radius": 100, "amount": 48}
        )
        client.call("commit_face_preview", {"token": sculpt["state"]["preview_token"]})
        terrain_hit = client.call(
            "preview_surface_placement",
            {"ids": [helper], "method": "ray", "align": True, "offset": 3, "ray_origin": [-48, 24, 200], "ray_direction": [0, 0, -1]},
        )
        assert terrain_hit["plan"]["target"] == terrain
        assert terrain_hit["plan"]["point"][2] > 16
        assert 0 < terrain_hit["plan"]["normal"][2] < 0.99
        close(terrain_hit["plan"]["contact_offset"], 3)
        client.call("cancel_surface_placement", {"token": terrain_hit["token"]})
        (args.output / "validation.json").write_text(
            json.dumps({"floor": floor, "ceiling": ceiling, "wall": wall, "model": model_hit, "terrain": terrain_hit, "stats": stats}, indent=2) + "\n"
        )
    finally:
        client.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, default=Path("/tmp/hammerr-surface-placement"))
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    validate(args)
    print(json.dumps({"surface_placement": True, "output": str(args.output)}))


if __name__ == "__main__":
    main()
