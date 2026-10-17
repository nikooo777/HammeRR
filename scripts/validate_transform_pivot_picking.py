import argparse
import json
import math
import struct
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client, detail, face_ref
from validate_prop_shadows import fixture

SIZE = 511


def validate(client, output):
    checks = []

    def check(condition, label):
        if not condition:
            raise AssertionError(label)
        checks.append(label)

    def inspect():
        return client.call("transform_pivot_pick", {"action": "inspect"})

    def action(name, error=False, **arguments):
        return client.call("transform_pivot_pick", {"action": name, **arguments}, error=error)

    def begin(camera=None):
        return action("begin", snapshot=inspect()["snapshot"], width=SIZE, height=SIZE, **({"camera": camera} if camera else {}))

    def pick(origin, direction, camera=None):
        token = begin(camera)["picking"]["token"]
        return action("ray", token=token, ray_origin=origin, ray_direction=direction)

    floor = client.call("create_block", {"mins": [-256, -256, -16], "maxs": [256, 256, 0]})["id"]
    terrain = client.call("create_block", {"mins": [-64, -64, 0], "maxs": [64, 64, 16]})["id"]
    shape = detail(client, [terrain])[0]
    face = face_ref(shape, next(row for row in shape["faces"] if row["normal"][2] > 0.99))
    client.call("create_terrain", {"faces": [face], "power": 2})
    model = client.call("create_entity", {"classname": "prop_dynamic", "origin": [128, -64, 64], "properties": {"model": "models/props/test.mdl"}})["id"]
    check(client.call("inspect_entity_model", {"id": model})["displayed"], "private MDL fixture provides actual supported model triangles")
    client.call("select", {"ids": [floor]})
    client.call("set_transform_tool", {"mode": "rotate", "pivot_mode": "custom", "pivot": [1.25, 2.5, 3.75], "space": "world", "texture_lock": False, "texture_scale_lock": True, "angle_step": 22.5})
    client.call("set_spatial_settings", {"grid_size": 64, "snap": True})
    client.call("save_map", {"path": str(output / "source.vmf")})
    source_bytes = (output / "source.vmf").read_bytes()
    before = client.call("get_map_info")
    history = client.call("get_history")
    camera = client.call("get_camera")
    bounds = detail(client, [floor])[0]["bounds"]
    old_pivot = inspect()["current_pivot"]
    state = begin()
    check(inspect()["current_pivot"] == old_pivot, "arming preserves exact custom pivot")
    token = state["picking"]["token"]
    for arguments in [
        {"ray_origin": [1000, 1000, 1000], "ray_direction": [0, 0, 1]},
        {"ray_origin": [0, 0, 100], "ray_direction": [0, 0, 0]},
    ]:
        action("ray", error=True, token=token, **arguments)
        check(inspect() == state, "miss or invalid ray preserves the entire armed picker and pivot")
    action("pixel", error=True, token=token, x=255.5, y=255.5, width=SIZE + 1, height=SIZE)
    check(inspect() == state, "wrong viewport dimensions preserve the picker")
    action("cancel", token=token)
    check(inspect()["current_pivot"] == old_pivot, "cancel preserves the exact prior pivot")
    for origin, target, expected, label in [
        ([-160.125, -128.25, 100], floor, [-160.125, -128.25, 0], "selected brush"),
        ([3.125, -5.25, 100], terrain, [3.125, -5.25, 16], "terrain sheet"),
        ([128, -64, 128], model, [128, -64, 64], "model triangle"),
        ([148, -44, 128], floor, [148, -44, 0], "model bounds without triangle"),
    ]:
        hit = pick(origin, [0, 0, -7])
        check(hit["picking"] is None and hit["last_pick"]["target"] == target, f"{label}: actual first visible surface provenance")
        check(all(math.isclose(a, b, rel_tol=0, abs_tol=1e-10) for a, b in zip(hit["current_pivot"], expected, strict=True)), f"{label}: exact unsnapped custom pivot")
        check(hit["last_pick"]["point"] == hit["current_pivot"] and hit["last_pick_matches_pivot"], f"{label}: inspectable successful choice")
        action("ray", error=True, token=token, ray_origin=origin, ray_direction=[0, 0, -1])
    for projection in ["perspective", "top", "front", "side"]:
        capture = {"projection": projection, "position": [-160.125, -128.25, 100], "look_at": [-160.125, -128.25, 0]} if projection == "perspective" else {"projection": projection, "center": [-160.125, -128.25, -8], "ortho_height": 300}
        state = begin(capture)
        hit = action("pixel", token=state["picking"]["token"], x=SIZE / 2, y=SIZE / 2, width=SIZE, height=SIZE)
        check(hit["last_pick"]["target"] == floor, f"{projection}: captured pixel camera reaches the brush surface")
        check(client.call("get_camera") == camera, f"{projection}: temporary pick camera leaves user camera unchanged")
    client.call("draw_block", {"start": [0, 0], "end": [64, 64], "projection": "top", "preview": True})
    preview = client.call("get_map_info")["block_preview"]
    action("begin", error=True, snapshot=inspect()["snapshot"], width=SIZE, height=SIZE)
    check(preview is not None and client.call("get_map_info")["block_preview"] == preview, "begin rejects active geometry without cancelling it")
    client.call("cancel_block_preview")
    state = begin()
    token = state["picking"]["token"]
    client.call("set_camera", {"projection": "top", "center": [0, 0, 0], "ortho_height": 300})
    pivot = inspect()["current_pivot"]
    action("ray", error=True, token=token, ray_origin=[0, 0, 100], ray_direction=[0, 0, -1])
    check(not inspect()["picking"]["valid"] and inspect()["current_pivot"] == pivot, "camera change makes the gesture stale without changing pivot")
    action("cancel", token=token)
    check(inspect()["picking"] is None and inspect()["current_pivot"] == pivot, "owned stale cancellation preserves pivot")
    after = client.call("get_map_info")
    for key in ["selection", "world_brushes", "point_entities", "unsaved_changes", "undo", "redo"]:
        check(before[key] == after[key], f"surface picking preserves map {key}")
    check(client.call("get_history")["undo_count"] == history["undo_count"], "picking adds no Undo transactions")
    check(detail(client, [floor])[0]["bounds"] == bounds, "pivot changes preserve framing bounds")
    check((output / "source.vmf").read_bytes() == source_bytes, "pivot actions preserve saved source bytes")
    check(not (output / "recovery/preview-defaults.json").exists(), "pivot actions never persist preview preferences")
    for target, origin in [(terrain, [3.125, -5.25, 100]), (model, [128, -64, 128])]:
        client.call("set_object_visibility", {"action": "hide", "ids": [target]})
        hidden_history = client.call("get_history")["undo_count"]
        hit = pick(origin, [0, 0, -1])
        check(hit["last_pick"]["target"] == floor and hit["current_pivot"][2] == 0, "hidden terrain/model is skipped for the visible floor")
        check(client.call("get_history")["undo_count"] == hidden_history, "hidden-surface picking adds no map history")
        client.call("set_object_visibility", {"action": "show", "ids": [target]})
    result = {"checks": len(checks), "passed": checks, "native_exercised": False}
    (output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"checks": len(checks), "native_exercised": False}))


def main():
    parser = argparse.ArgumentParser(description="Validate exact surface-picked transform pivots over private headless MCP.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    game = output / "game"
    fixture(game)
    vtx = game / "models/props/test.dx90.vtx"
    data = bytearray(vtx.read_bytes())
    struct.pack_into("<3H", data, 134, 0, 2, 1)
    vtx.write_bytes(data)
    client = Client(None, args.binary.resolve(), output, game / "gameinfo.txt")
    try:
        validate(client, output)
    finally:
        client.close()


if __name__ == "__main__":
    main()
