import argparse
import json
import math
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client, detail, face_ref


def close(actual, expected):
    assert math.isclose(actual, expected, abs_tol=1e-5), (actual, expected)


def begin(client, classname="info_target", **options):
    state = client.call("get_entity_placement")
    return client.call(
        "begin_entity_placement",
        {"classname": classname, "snapshot": state["snapshot"], **options},
    )["draft"]["token"]


def pick(client, token, origin, direction):
    return client.call(
        "pick_entity_placement",
        {"token": token, "ray_origin": origin, "ray_direction": direction},
    )


def cancel(client, token):
    client.call("cancel_entity_placement", {"token": token})


def validate(args):
    client = Client(None, args.binary, args.output)
    checks = {}
    try:
        client.call("new_map", {"discard_changes": True})
        registered = {tool["name"] for tool in client.rpc("tools/list", {})["tools"]}
        assert {
            "get_entity_placement", "begin_entity_placement", "pick_entity_placement",
            "review_entity_placement", "apply_entity_placement", "cancel_entity_placement",
        } <= registered
        floor = client.call("create_block", {"mins": [-256, -256, -16], "maxs": [256, 256, 0]})["id"]
        wall = client.call("create_block", {"mins": [-256, 128, 0], "maxs": [256, 144, 256]})["id"]
        original = client.call("get_objects", {"ids": [floor, wall]})
        history = client.call("get_map_info")["undo"]
        selection = client.call("get_selection")
        token = begin(client, "light_spot", align=True, offset=4, snap=True,
                      properties={"pitch": "-25", "angles": "0 30 0", "targetname": "surface_spot"})
        armed = client.call("get_entity_placement")
        assert armed["draft"]["picking"]
        client.call("save_map", {"path": str(args.output / "pending.vmf")}, error=True)
        client.call("compile_map", {"path": str(args.output / "pending.vmf")}, error=True)
        client.call("pick_entity_placement", {"token": token, "ray_origin": [0, 0, 50], "ray_direction": [0, 0, 0]}, error=True)
        assert client.call("get_entity_placement") == armed
        report = pick(client, token, [35.1, 0, 93.7], [0, 1, 0])
        plan = report["draft"]["plan"]
        assert plan["target"] == wall and plan["normal"] == [0, -1, 0]
        assert plan["bounds_source"] == "editor_helper"
        close(plan["point"][0], 32)
        close(plan["point"][2], 96)
        close(plan["contact_offset"], 4)
        close(plan["bounds"]["maxs"][1], 124)
        properties = dict(plan["properties"])
        close(float(properties["pitch"]), -float(properties["angles"].split()[0]))
        assert client.call("get_objects", {"ids": [floor, wall]}) == original
        assert client.call("get_map_info")["undo"] == history
        assert client.call("get_selection") == selection
        for invalid in [{"offset": 65537}, {"classname": "func_detail"}, {"classname": "info_overlay"}, {"unknown": True}]:
            client.call("begin_entity_placement", {"classname": "info_target", "snapshot": report["snapshot"], **invalid}, error=True)
            assert client.call("get_entity_placement") == report
        client.call("pick_entity_placement", {"token": token, "ray_origin": [1000, 1000, 1000], "ray_direction": [0, 0, 1]}, error=True)
        assert client.call("get_entity_placement") == report
        client.render(args.output / "entity-placement.png", camera={"position": [-90, -100, 150], "look_at": [32, 110, 96]})
        reviewed = client.call("review_entity_placement", {"token": token})
        replacement_token = reviewed["draft"]["token"]
        assert replacement_token != token
        client.call("apply_entity_placement", {"token": token}, error=True)
        client.call("cancel_entity_placement", {"token": token}, error=True)
        created = client.call("apply_entity_placement", {"token": replacement_token})["created"]
        assert len(client.call("get_map_info")["undo"]) == len(history) + 1
        entity = detail(client, [created])[0]
        assert dict(entity["properties"]) == properties
        client.call("undo")
        assert client.call("get_objects", {"ids": [floor, wall]}) == original
        client.call("redo")
        assert detail(client, [created])[0] == entity
        path = args.output / "entity-placement.vmf"
        client.call("save_map", {"path": str(path)})
        client.call("open_map", {"path": str(path), "discard_changes": True})
        assert detail(client, [created])[0] == entity
        checks["wall_light"] = report

        token = begin(client, snap=False)
        good = pick(client, token, [-90, -90, 96], [0, 0, -1])
        assert good["draft"]["plan"]["target"] == floor
        client.call("transform_objects", {"ids": [wall], "translation": [1, 0, 0], "preview": True})
        client.call("cancel_entity_placement", {"token": token}, error=True)
        client.call("apply_entity_placement", {"token": token}, error=True)
        assert client.call("get_transform_handles")["state"]["preview"] is not None
        client.call("cancel_transform_preview")
        checks["replacement"] = True

        crate = client.call("create_entity", {"classname": "prop_dynamic", "origin": [-96, -96, 0],
            "properties": {"model": "models/props_junk/wood_crate001a.mdl"}})["id"]
        model = client.call("inspect_entity_model", {"id": crate})
        assert model["displayed"], model
        token = begin(client, snap=False, offset=2)
        model_hit = pick(client, token, [-96, -96, 128], [0, 0, -1])
        assert model_hit["draft"]["plan"]["target"] == crate
        close(model_hit["draft"]["plan"]["contact_offset"], 2)
        cancel(client, token)
        checks["model_target"] = model_hit
        token = begin(client, "prop_dynamic", snap=False, align=True, offset=3,
                      properties={"model": "models/props_junk/wood_crate001a.mdl", "angles": "0 17 0"})
        model_source = pick(client, token, [96, -96, 128], [0, 0, -1])
        assert model_source["draft"]["plan"]["bounds_source"] == "model_mesh"
        assert model_source["draft"]["plan"]["support_points"] > 8
        close(model_source["draft"]["plan"]["bounds"]["mins"][2], 3)
        cancel(client, token)
        checks["model_source"] = model_source

        terrain = client.call("create_block", {"mins": [-64, -64, 0], "maxs": [64, 64, 16]})["id"]
        shape = detail(client, [terrain])[0]
        face = face_ref(shape, next(face for face in shape["faces"] if face["normal"][2] > 0.99))
        client.call("select_faces", {"faces": [face]})
        client.call("create_terrain", {"power": 2})
        sculpt = client.call("preview_terrain_sculpt", {"mode": "raise", "center": [0, 0, 16], "radius": 100, "amount": 48})
        client.call("commit_face_preview", {"token": sculpt["state"]["preview_token"]})
        token = begin(client, snap=False, align=True, offset=3)
        terrain_hit = pick(client, token, [16, 24, 128], [0, 0, -1])
        plan = terrain_hit["draft"]["plan"]
        assert plan["target"] == terrain and plan["point"][2] > 16
        assert 0 < plan["normal"][2] < 0.99
        close(plan["contact_offset"], 3)
        cancel(client, token)
        checks["terrain"] = terrain_hit
        token = begin(client)
        client.call("set_spatial_settings", {"grid_size": 8})
        client.call("apply_entity_placement", {"token": token}, error=True)
        token = begin(client)
        generation = client.call("get_entity_placement")["generation"]
        client.call("new_map", {"discard_changes": True})
        new_token = begin(client)
        assert new_token > token
        assert client.call("get_entity_placement")["generation"] != generation
        client.call("cancel_entity_placement", {"token": token}, error=True)
        cancel(client, new_token)
        checks["stale_grid_and_generation"] = True
        (args.output / "validation.json").write_text(json.dumps(checks, indent=2) + "\n")
    finally:
        client.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, default=Path("/tmp/hammerr-entity-placement"))
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    validate(args)
    print(json.dumps({"entity_placement": True, "output": str(args.output)}))


if __name__ == "__main__":
    main()
