import argparse
import hashlib
import json
import math
import os
import struct
from pathlib import Path
from unittest.mock import patch

from validate_face_lighting import DEFAULT_BINARY, Client, detail, face_ref
from validate_linked_paths import FGD
from validate_prop_shadows import fixture
from validate_sky_preview import rgba_png

SIZE = 511
POINTS = [[-192, -160, 0], [-112, -160, 8.25], [-32, -160, 8.5]]


def close(actual, expected, tolerance=1e-10):
    return all(math.isclose(a, b, rel_tol=0, abs_tol=tolerance) for a, b in zip(actual, expected, strict=True))


def validate(client, output):
    checks, images, receipts = [], [], {}

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def state():
        return client.call("get_linked_path")

    def action(name, **arguments):
        return client.call("set_linked_path", {"action": name, **arguments})

    def token():
        return state()["preview"]["token"]

    def arm(row=1, camera=None):
        current = state()
        return action("begin_pick", token=token(), row=row, width=SIZE, height=SIZE, snapshot=current["pick_snapshot"], **({"camera": camera} if camera else {}))

    def ray(origin, direction):
        return action("pick_ray", token=token(), origin=origin, direction=direction)

    def reject(name, **arguments):
        before = state()
        client.call("set_linked_path", {"action": name, **arguments}, error=True)
        check(state() == before, f"Rejected {name} leaves the complete owned path draft unchanged")

    def source_state():
        info = client.call("get_map_info")
        return {"objects": detail(client, objects), "history": client.call("get_history"), "camera": client.call("get_camera"), **{key: info[key] for key in ("selection", "unsaved_changes", "view", "property_draft")}}

    def render(name, camera):
        png, metadata = client.render_result(width=SIZE, height=SIZE, camera=camera, selection_outlines=False)
        path = output / f"{name}.png"
        path.write_bytes(png)
        images.append({"file": path.name, "sha256": hashlib.sha256(png).hexdigest(), "camera": metadata["camera"]})
        return rgba_png(path)

    def edit(points):
        inspected = action("inspect", id=path_ids[0])
        return action("edit", seed=inspected["seed"], nodes=[{"id": node, "origin": point} for node, point in zip(path_ids, points, strict=True)], snapshot=inspected["snapshot"])

    def assert_hit(before, picked, point, target, label, tolerance=1e-10):
        prior, after = before["preview"], picked["preview"]
        (output / "latest-hit.json").write_text(json.dumps({"label": label, "expected_point": point, "expected_target": target, "row": after["nodes"][1], "last_pick": after["last_pick"]}, indent=2) + "\n")
        check(after["token"] != prior["token"] and after["picking"] is None, f"{label}: a successful hit finishes the gesture with a fresh token")
        check(close(after["nodes"][1]["origin"], point, tolerance) and close(after["last_pick"]["point"], point, tolerance) and after["last_pick"]["target"] == target and after["last_pick"]["row"] == 1, f"{label}: selected row and surface provenance match the independent expected world point")
        check(after["nodes"][0] == prior["nodes"][0] and after["nodes"][2] == prior["nodes"][2], f"{label}: other rows retain exact IDs, names, positions and links")
        check(all(after["nodes"][1][key] == prior["nodes"][1][key] for key in ("id", "source_id", "targetname", "target")), f"{label}: moved row retains its identity and link topology")

    client.call("new_map", {"discard_changes": True})
    fgd = output / "paths.fgd"
    fgd.write_text(FGD)
    client.call("load_entity_definitions", {"path": str(fgd)})
    client.call("set_spatial_settings", {"grid_visible": False, "grid_size": 64})
    client.call("set_transform_tool", {"mode": "select"})
    floor = client.call("create_block", {"mins": [-256, -256, -16], "maxs": [256, 256, 0]})["id"]
    north = client.call("create_block", {"mins": [-200, 160, 0], "maxs": [200, 176, 160]})["id"]
    east = client.call("create_block", {"mins": [192, -200, 0], "maxs": [208, 200, 160]})["id"]
    terrain = client.call("create_block", {"mins": [-64, -64, 0], "maxs": [64, 64, 16]})["id"]
    shape = detail(client, [terrain])[0]
    face = face_ref(shape, next(row for row in shape["faces"] if row["normal"][2] > 0.99))
    client.call("create_terrain", {"faces": [face], "power": 2})
    sculpt = client.call("preview_terrain_sculpt", {"faces": [face], "mode": "raise", "center": [0, 0, 16], "radius": 32, "amount": 64})
    client.call("commit_face_preview", {"token": sculpt["state"]["preview_token"]})
    model = client.call("create_entity", {"classname": "prop_dynamic", "origin": [128, -64, 64], "properties": {"model": "models/props/test.mdl"}})["id"]
    helper = client.call("create_entity", {"classname": "info_target", "origin": [300, 0, 32]})["id"]
    missing = client.call("create_entity", {"classname": "prop_dynamic", "origin": [340, 0, 32], "properties": {"model": "models/props/missing.mdl"}})["id"]
    check(client.call("inspect_entity_model", {"id": model})["displayed"], "Private real static MDL/VVD/VTX geometry is available as a surface")
    created = action("preview", classname="path_corner", prefix="route", points=POINTS, snapshot=state()["snapshot"])
    arm(0)
    first_hit = ray([-160.125, -128.25, 100], [0, 0, -7])
    check(first_hit["preview"]["nodes"][0]["origin"] == [-160.125, -128.25, 0] and first_hit["preview"]["nodes"][1:] == created["preview"]["nodes"][1:] and first_hit["preview"]["last_pick"]["target"] == floor, "New path creation drafts support exact unsnapped row positioning before any Apply")
    action("cancel", token=first_hit["preview"]["token"])
    created = action("preview", classname="path_corner", prefix="route", points=POINTS, snapshot=state()["snapshot"])
    path_ids = action("apply", token=created["preview"]["token"])["objects"]
    client.call("set_entity_properties", {"id": path_ids[1], "set": {"wait": "3.7500", "custom_intent": "retain exact raw value"}})
    client.call("select", {"ids": []})
    objects = [floor, north, east, terrain, model, helper, missing, *path_ids]
    source = output / "source.vmf"
    client.call("save_map", {"path": str(source)})
    saved = source.read_bytes()
    original = detail(client, objects)
    edit(POINTS)
    property_token = client.call("begin_property_edit", {"world": True})["draft"]["token"]
    client.call("update_property_edit", {"token": property_token, "changes": [{"action": "set", "key": "message", "value": "pending unrelated property"}]})
    baseline = source_state()
    tools = {tool["name"]: tool for tool in client.rpc("tools/list", {})["tools"]}
    schema = json.dumps(tools["set_linked_path"]["inputSchema"])
    check(all(f'"{name}"' in schema for name in ("begin_pick", "pick_ray", "pick_pixel", "cancel_pick")), "Existing linked-path tool exposes all four surface picking actions")
    reject("begin_pick", token=token(), row=99, width=SIZE, height=SIZE, snapshot=state()["pick_snapshot"])
    reject("begin_pick", token=token(), row=1, width=0, height=SIZE, snapshot=state()["pick_snapshot"])
    reject("begin_pick", token=token(), row=1, width=SIZE, height=SIZE, snapshot="stale")
    before = arm()
    armed_token = token()
    for name, arguments in (("pick_ray", {"origin": [1000, 1000, 1000], "direction": [0, 0, 1]}), ("pick_ray", {"origin": [0, 0, 128], "direction": [0, 0, 0]}), ("pick_ray", {"origin": [-192, -160, 128], "direction": [0, 0, -1]}), ("pick_ray", {"origin": [300, 0, 128], "direction": [0, 0, -1]}), ("pick_ray", {"origin": [340, 0, 128], "direction": [0, 0, -1]}), ("pick_pixel", {"x": 255.5, "y": 255.5, "width": SIZE + 1, "height": SIZE}), ("pick_pixel", {"x": SIZE, "y": 0, "width": SIZE, "height": SIZE}), ("apply", {})):
        reject(name, token=armed_token, **arguments)
    cancelled = action("cancel_pick", token=armed_token)
    check(cancelled["preview"]["nodes"] == before["preview"]["nodes"] and cancelled["preview"]["picking"] is None and cancelled["preview"]["token"] != armed_token, "Cancel picking disarms only the gesture with every row unchanged")
    reject("cancel_pick", token=armed_token)
    for point, direction, target, expected, label in (
        ([35.125, -128.25, 128], [0, 0, -7], floor, [35.125, -128.25, 0], "floor"),
        ([35.125, 0, 93.125], [0, 7, 0], north, [35.125, 160, 93.125], "north wall"),
        ([0, 0, 128], [0, 0, -1], terrain, [0, 0, 80], "displaced terrain peak"),
        ([128, -64, 128], [0, 0, -3], model, [128, -64, 64], "real model triangle"),
        ([148, -44, 128], [0, 0, -1], floor, [148, -44, 0], "outside model triangle inside its bounding box"),
    ):
        before = arm()
        picked = ray(point, direction)
        assert_hit(before, picked, expected, target, label)
        normal = picked["preview"]["last_pick"]["normal"]
        check(math.isclose(sum(value * value for value in normal), 1, abs_tol=1e-6), f"{label}: surface normal is finite and unit length")
        if target == north:
            check(close(normal, [0, -1, 0]), "Wall hit reports its actual outward normal")
        if target == model:
            check(picked["preview"]["last_pick"]["solid"] is None and picked["preview"]["last_pick"]["side"] is None, "Model triangle provenance never invents brush IDs")
            armed = arm()
            reject("pick_ray", token=token(), origin=[128, -64, 32], direction=[0, 0, 1])
            check(state() == armed, "A below-plane model backface miss preserves the exact armed row")
            action("cancel_pick", token=token())
        receipts[label] = picked["preview"]["last_pick"]
    cameras = {
        "top": ({"projection": "top", "center": [35.125, -128.25, 0], "ortho_height": 256}, [35.125, -128.25, 0], floor),
        "front": ({"projection": "front", "center": [35.125, 160, 93.125], "ortho_height": 256}, [35.125, 160, 93.125], north),
        "side": ({"projection": "side", "center": [208, 8.125, 93.125], "ortho_height": 256}, [208, 8.125, 93.125], east),
        "perspective": ({"projection": "perspective", "position": [-96, -220, 128], "look_at": [-96, -128, 0]}, [-96, -128, 0], floor),
    }
    for name, (camera, expected, target) in cameras.items():
        edit(POINTS)
        plain = render(f"before-pick-{name}", camera)
        before = arm(camera=camera)
        picked = action("pick_pixel", token=token(), x=SIZE / 2, y=SIZE / 2, width=SIZE, height=SIZE)
        assert_hit(before, picked, expected, target, f"{name} center pixel", 2e-4 if name == "perspective" else 1e-10)
        shown = render(f"surface-picked-{name}", camera)
        changed = sum(shown[index:index + 3] != plain[index:index + 3] for index in range(0, len(shown), 4))
        check(changed > 20, f"{name}: actual draft marker pixels move after surface picking")
        exact = picked["preview"]["nodes"][1]["origin"]
        numeric = [POINTS[0], exact, POINTS[2]]
        edit(numeric)
        check(render(f"numeric-control-{name}", camera) == shown, f"{name}: picked row pixels match independent exact numeric placement")
        receipts[f"pixel_{name}"] = {"last_pick": picked["preview"]["last_pick"], "changed_pixels": changed}
    check(source_state() == baseline and source.read_bytes() == saved and detail(client, objects) == original, "All precise ray/pixel reads, renders and draft edits preserve committed source, full history, camera, selection and unrelated property draft")
    client.call("cancel_property_edit", {"token": property_token})
    for change in ("camera", "view", "definitions", "map"):
        edit(POINTS)
        armed = arm()
        stale_token = token()
        if change == "camera":
            client.call("set_camera", {"position": [200, -220, 180], "look_at": [0, 0, 0]})
        elif change == "view":
            client.call("set_view", {"entity_images": False})
        elif change == "definitions":
            client.call("load_entity_definitions", {"path": str(fgd)})
        else:
            client.call("create_block", {"mins": [512, 512, 0], "maxs": [528, 528, 16]})
        current = state()
        reject("pick_ray", token=stale_token, origin=[35, -128, 128], direction=[0, 0, -1])
        if current["preview"] is None:
            check(change in ("definitions", "map"), f"{change}: source invalidation closes the geometry preview")
            reject("cancel_pick", token=stale_token)
        else:
            check(not current["preview"]["picking"]["valid"], f"{change}: captured pick context becomes explicitly stale")
            cleaned = action("cancel_pick", token=stale_token)
            check(cleaned["preview"]["nodes"] == armed["preview"]["nodes"] and cleaned["preview"]["picking"] is None, f"{change}: owned stale cleanup retains every planned row")
            action("cancel", token=cleaned["preview"]["token"])
    client.call("set_view", {"entity_images": True})
    for target, origin in ((terrain, [0, 0, 128]), (model, [128, -64, 128])):
        client.call("set_object_visibility", {"action": "hide", "ids": [target]})
        edit(POINTS)
        arm()
        picked = ray(origin, [0, 0, -1])
        check(picked["preview"]["last_pick"]["target"] == floor and picked["preview"]["nodes"][1]["origin"][2] == 0, "Hidden terrain/model surfaces are skipped for the real floor beneath")
        action("cancel", token=token())
        client.call("undo")
    final_before = detail(client, objects)
    history = client.call("get_history")["undo_count"]
    edit(POINTS)
    arm()
    picked = ray([35.123456789012, -128.25, 128], [0, 0, -1])
    expected = picked["preview"]["nodes"]
    check(close(expected[1]["origin"], [35.123456789012, -128.25, 0], 1e-12), "Sub-grid double precision survives surface picking without float32 or grid quantization")
    reviewed = action("review", token=token())
    reject("apply", token=picked["preview"]["token"])
    action("apply", token=reviewed["preview"]["token"])
    actual = detail(client, path_ids)
    check(client.call("get_history")["undo_count"] == history + 1 and [row["origin"] for row in actual] == [row["origin"] for row in expected], "Existing Apply commits exact picked rows and links in one Undo step")
    check(dict(actual[1]["properties"])["wait"] == "3.7500" and dict(actual[1]["properties"])["custom_intent"] == "retain exact raw value", "Retained node custom values and raw formatting survive positional Apply")
    client.call("undo")
    check(detail(client, objects) == final_before, "One Undo restores every original source object and node identity")
    client.call("redo")
    check(detail(client, path_ids) == actual, "Redo restores exact picked positions, IDs and links")
    saved_path = output / "surface-picked.vmf"
    client.call("save_map", {"path": str(saved_path)})
    client.call("open_map", {"path": str(saved_path), "discard_changes": True})
    check(detail(client, path_ids) == actual, "Picked path positions survive save and reopen")
    edit([row["origin"] for row in actual])
    arm()
    replaced_token = token()
    client.call("new_map", {"discard_changes": True})
    replacement = action("preview", classname="path_corner", prefix="replacement", points=POINTS, snapshot=state()["snapshot"])
    reject("cancel_pick", token=replaced_token)
    check(state() == replacement, "A previous map's token cannot cancel a replacement path draft")
    action("cancel", token=token())
    (output / "results.json").write_text(json.dumps({"passed": len(checks), "checks": checks, "surface_receipts": receipts, "images": images, "source_sha256": hashlib.sha256(saved).hexdigest(), "desktop_input": False, "native_exercised": False}, indent=2) + "\n")
    print(json.dumps({"passed": len(checks), "output": str(output)}))


def main():
    parser = argparse.ArgumentParser(description="Validate owned linked-path row surface picks through private MCP, analytic surfaces and actual paired PNG controls.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    game = output / "game"
    fixture(game)
    vtx = game / "models/props/test.dx90.vtx"
    data = bytearray(vtx.read_bytes())
    assert struct.unpack_from("<I", data, 146)[0] == 3
    assert struct.unpack_from("<3H", data, 134) == (0, 1, 2)
    struct.pack_into("<3H", data, 134, 0, 2, 1)
    vtx.write_bytes(data)
    with patch.dict(os.environ, {"HAMMERR_RECENT_MAPS_DIR": str(output / "recent"), "HAMMERR_BUILD_RECIPES": str(output / "recipes.json")}):
        client = Client(None, args.binary.resolve(), output, game / "gameinfo.txt")
    try:
        validate(client, output)
    finally:
        client.close()


if __name__ == "__main__":
    main()
