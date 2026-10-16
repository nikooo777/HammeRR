import argparse
import json
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_sky_preview import rgba_png


def validate(client, output, model):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def render(name, **arguments):
        data, metadata = client.render_result(width=401, height=401, **arguments)
        path = output / f"{name}.png"
        path.write_bytes(data)
        return rgba_png(path), metadata

    def entity(classname, origin, **properties):
        return client.call("create_entity", {"classname": classname, "origin": origin, "properties": properties})["id"]

    def compare(name, object_id, camera):
        client.call("select", {"ids": [object_id]})
        client.call("set_camera", camera)
        enabled, metadata = render(f"{name}-on", selection_outlines=True)
        disabled, off_metadata = render(f"{name}-off", selection_outlines=False)
        check(enabled != disabled, f"{name}: selected edges or brackets change visible pixels")
        check(metadata["view"]["selection_outlines"] and not off_metadata["view"]["selection_outlines"], f"{name}: render metadata identifies the effective override")
        return enabled, disabled

    client.call("new_map", {"discard_changes": True})
    client.call("set_spatial_settings", {"grid_visible": False})
    client.call("set_transform_tool", {"mode": "select"})
    client.call("set_view", {"surface": "textured", "tool_textures": True})
    check(client.call("get_map_info")["view"]["selection_outlines"], "Selection outlines default on")
    block = client.call("create_block", {"mins": [0, 0, 0], "maxs": [64, 64, 64], "material": "dev/dev_measuregeneric01"})["id"]
    camera = {"projection": "perspective", "position": [220, -300, 220], "look_at": [32, 32, 32]}
    enabled, disabled = compare("brush", block, camera)
    client.call("save_map", {"path": str(output / "authored.vmf")})
    source = (output / "authored.vmf").read_bytes()
    before = client.call("get_map_info")
    pixel = {"x": 200, "y": 200, "width": 401, "height": 401}
    pick = client.call("pick_at_pixel", pixel)
    ray = {"origin": [32, -128, 32], "direction": [0, 1, 0]}
    candidates = client.call("get_object_hit_candidates", ray)
    client.call("set_view", {"selection_outlines": False})
    check(render("shared-off")[0] == disabled, "Shared Session setting matches the temporary disabled render")
    check(client.call("pick_at_pixel", pixel) == pick and client.call("get_object_hit_candidates", ray) == candidates, "Pixel and overlapping-object picks remain unchanged")
    check(render("temporary-on", selection_outlines=True)[0] == enabled, "An enabled render override restores the default pixels")
    check(not client.call("get_map_info")["view"]["selection_outlines"], "Temporary render overrides preserve shared settings")
    for mode in ["flat", "wireframe", "lightmap_density", "smoothing_groups"]:
        on, _ = render(f"{mode}-on", surface=mode, selection_outlines=True)
        off, _ = render(f"{mode}-off", surface=mode, selection_outlines=False)
        check(on != off, f"Selection outlines can be controlled in {mode}")
    for projection in ["top", "front", "side"]:
        planar = {"projection": projection, "center": [32, 32, 32], "ortho_height": 160}
        on, _ = render(f"{projection}-on", camera=planar, selection_outlines=True)
        off, _ = render(f"{projection}-off", camera=planar, selection_outlines=False)
        check(on != off, f"Selection outlines can be controlled in {projection} projection")
    areas = []
    for outlines in [False, True]:
        client.call("set_view", {"selection_outlines": outlines})
        area = client.call("select_area", {"start": [0, 0], "end": [401, 401], "width": 401, "height": 401, "preview": True})
        areas.append(area["matches"])
        client.call("cancel_area_selection")
    check(areas[0] == areas[1], "Area selection matches remain unchanged by outline visibility")
    after = client.call("get_map_info")
    check(before["undo"] == after["undo"] and before["selection"] == after["selection"] and not after["unsaved_changes"], "View controls preserve authored selection, history and dirty state")
    check((output / "authored.vmf").read_bytes() == source, "View controls preserve the source VMF")
    client.call("select", {"ids": []})
    unselected, _ = render("unselected")
    check(unselected != disabled, "Selected surface tint remains visible while outlines are off")
    measurement = client.call("set_measurement", {
        "action": "preview", "name": "Outline-independent ruler", "start": [-32, 32, 96], "end": [96, 32, 96],
        "snapshot": client.call("get_measurements")["snapshot"],
    })
    guide_on, _ = render("measurement-on", selection_outlines=True)
    guide_off, _ = render("measurement-off", selection_outlines=False)
    check(guide_on == guide_off and guide_off != unselected, "Measurement guides retain identical pixels with outlines on or off")
    client.call("set_measurement", {"action": "cancel", "token": measurement["draft"]["token"]})
    side = detail(client, [block])[0]["faces"][0]["side"]
    client.call("select_faces", {"faces": [{"solid": int(block.split(":")[1]), "side": side}]})
    check(render("face-on", selection_outlines=True)[0] != render("face-off", selection_outlines=False)[0], "Selected face outlines can be hidden independently of the face tint")
    for classname, x in [("info_target", 400), ("light", 800), ("light_spot", 1200)]:
        point = entity(classname, [x, 0, 64], angles="0 -90 0", _light="255 210 150 300")
        compare(classname, point, {"projection": "perspective", "position": [x + 80, -160, 130], "look_at": [x, 0, 64]})
    if model:
        prop = entity("prop_static", [1600, 0, 64], model=model)
        inspection = client.call("inspect_entity_model", {"id": prop})
        check(inspection["displayed"] and not inspection["helper"], "The model bracket case uses supported model geometry")
        compare("model", prop, {"projection": "perspective", "position": [1800, -240, 180], "look_at": [1600, 0, 96]})
    target = entity("keyframe_rope", [2128, 0, 128], targetname="outline_rope_end")
    rope = entity("move_rope", [1872, 0, 128], NextKey="outline_rope_end", Slack="48", Width="8")
    client.call("set_view", {"rope_preview": True})
    client.call("select", {"ids": [rope]})
    rope_camera = {"projection": "perspective", "position": [2000, -500, 64], "look_at": [2000, 0, 64]}
    client.call("set_camera", rope_camera)
    rope_off, _ = render("rope-off", selection_outlines=False)
    check(rope_off != render("rope-hidden", rope_preview=False, selection_outlines=False)[0], "Hiding selection outlines preserves the rope tube")
    check(client.call("get_rope_preview")["links"][0]["target"] == target, "Rope links remain unchanged by outline presentation")
    result = {"transport": "private-stdio", "checks": len(checks), "passed": checks, "model": model}
    (output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"transport": result["transport"], "checks": len(checks), "model": model}))


def main():
    parser = argparse.ArgumentParser(description="Validate Selection outlines in a private MCP process without desktop input.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--gameinfo", type=Path)
    parser.add_argument("--model", help="Optional mounted model path for the model-bracket render case.")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    client = Client(None, args.binary.resolve(), output, args.gameinfo)
    try:
        validate(client, output, args.model)
    finally:
        client.close()


if __name__ == "__main__":
    main()
