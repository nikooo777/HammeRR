import argparse
import json
import math
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client
from validate_sky_preview import rgba_png


def validate(client, output):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def render(name, **arguments):
        data, metadata = client.render_result(width=401, height=401, **arguments)
        path = output / f"{name}.png"
        path.write_bytes(data)
        return rgba_png(path), metadata

    def node(classname, origin, **properties):
        return client.call("create_entity", {
            "classname": classname, "origin": origin, "properties": properties,
        })["id"]

    def edit(entity, **properties):
        client.call("set_entity_properties", {"id": entity, "set": properties})

    client.call("new_map", {"discard_changes": True})
    client.call("set_spatial_settings", {"grid_visible": False})
    client.call("set_view", {"surface": "textured", "rope_preview": False})
    target = node("keyframe_rope", [160, 0, 128], targetname="rope_end")
    source = node("move_rope", [-160, 0, 128], targetname="rope_start", NextKey="rope_end", Slack="64", Width="12", Subdiv="8")
    client.call("select", {"ids": []})
    report = client.call("get_rope_preview")
    check(not report["enabled"] and len(report["links"]) == 1 and not report["issues"], "Preview defaults off and inspects the checked authored span")
    span = report["links"][0]
    check(span["source"] == source and span["target"] == target, "Span ownership follows the source entity and exact NextKey target")
    check(len(span["points"]) == 65 and abs(span["length"] - 384) < 1e-4, "Authored Slack and Subdiv produce the bounded sampled curve length")
    midpoint = span["points"][32]
    check(midpoint[2] < 128 and span["width"] == 12, "Static curve sags downward and preserves authored Width")
    camera = {"projection": "perspective", "position": [midpoint[0], -500, midpoint[2]], "look_at": midpoint}
    client.call("set_camera", camera)
    client.call("save_map", {"path": str(output / "authored.vmf")})
    original = (output / "authored.vmf").read_bytes()
    before = client.call("get_map_info")
    off, _ = render("off")
    on, metadata = render("temporary-on", rope_preview=True)
    check(on != off and metadata["rope_preview"]["applied"], "Temporary preview changes visible pixels and reports the applied static span")
    check(not client.call("get_map_info")["view"]["rope_preview"], "Render override preserves the shared setting")
    pick_args = {"x": 200, "y": 200, "width": 401, "height": 401}
    check(client.call("pick_at_pixel", pick_args)["hit"] is None, "Disabled spans leave empty midpoint picking unchanged")
    check(client.call("pick_at_pixel", {**pick_args, "rope_preview": True})["object"] == source, "Temporary pixel-pick override hits the same visible source-owned tube")
    client.call("set_view", {"rope_preview": True})
    check(render("shared-on")[0] == on, "Shared Session setting matches the temporary render")
    check(client.call("pick_at_pixel", pick_args)["object"] == source, "Shared tube picking follows the viewport setting")
    for mode in ["flat", "wireframe", "lightmap_density", "smoothing_groups"]:
        enabled, _ = render(f"{mode}-on", surface=mode)
        disabled, _ = render(f"{mode}-off", surface=mode, rope_preview=False)
        check(enabled != disabled, f"Static helper spans remain available in {mode}")
    planar = {"projection": "front", "center": midpoint, "ortho_height": 400}
    check(render("front-on", camera=planar)[0] != render("front-off", camera=planar, rope_preview=False)[0], "Static spans render in planar views")
    check(render("tools-hidden", tool_textures=False)[0] == on, "Tool-texture filtering preserves entity helper spans")
    after = client.call("get_map_info")
    check(before["undo"] == after["undo"] and before["selection"] == after["selection"] and not after["unsaved_changes"], "Preview controls preserve history, selection and dirty state")
    check((output / "authored.vmf").read_bytes() == original, "Preview controls preserve the source VMF")
    for key, value, expected in [
        ("NextKey", "missing", "no authored target"), ("NextKey", "rope_start", "same entity"),
        ("Width", "65", "Width"), ("Type", "2", "Type"),
        ("parentname", "moving_prop,hook", "Parented"), ("StartAttachment", "1", "attachment"),
    ]:
        edit(source, **{key: value})
        invalid = client.call("get_rope_preview")
        check(not invalid["links"] and any(expected in issue["message"] for issue in invalid["issues"]), f"{key}={value} fails visibly without invented geometry")
        check(render(f"invalid-{key}")[0] == off, f"{key}={value} draws only the retained endpoint helpers")
        client.call("undo")
        check(render(f"restored-{key}")[0] == on, f"Undo restores the span after {key} changes")
    duplicate = node("keyframe_rope", [160, 100, 128], targetname="ROPE_END")
    check(not client.call("get_rope_preview")["links"], "Ambiguous targetnames never choose an arbitrary endpoint")
    client.call("delete_objects", {"ids": [duplicate]})
    client.call("set_object_visibility", {"action": "hide", "ids": [target]})
    hidden = client.call("get_rope_preview")
    check(not hidden["links"] and "hidden" in hidden["issues"][0]["message"], "Hidden endpoints suppress spans with a visible diagnostic")
    client.call("undo")
    client.call("set_working_region", {
        "action": "set", "mins": [-180, -16, 100], "maxs": [-100, 16, 160],
        "snapshot": client.call("get_working_region")["snapshot"],
    })
    regional = client.call("get_rope_preview")
    check(not regional["links"] and "working region" in regional["issues"][0]["message"], "A filtered endpoint suppresses the shared span with a diagnostic")
    check(render("regional-on")[0] == render("regional-off", rope_preview=False)[0], "Region-filtered preview and picking use the retained endpoint scope")
    client.call("set_working_region", {"action": "clear", "snapshot": client.call("get_working_region")["snapshot"]})
    check(render("region-cleared")[0] == on, "Clearing the working region restores the static span")
    edit(target, NextKey="rope_start")
    cyclic = client.call("get_rope_preview")
    check(not cyclic["links"] and len(cyclic["issues"]) == 2, "Cyclic links are bounded and diagnosed")
    client.call("undo")
    edit(source, Slack="0")
    straight = client.call("get_rope_preview")["links"][0]
    check(math.isclose(straight["length"], 320) and straight["points"][32][2] == 128, "Zero Slack produces a straight authored span")
    client.call("undo")
    client.call("select", {"ids": [source]})
    check(render("selected")[0] != on, "Source selection remains visible on the tube and helper")
    client.call("select", {"ids": []})
    wall = client.call("create_block", {"mins": [-256, -120, -256], "maxs": [256, -104, 256], "material": "dev/dev_measuregeneric01"})["id"]
    client.call("select", {"ids": []})
    check(render("occluded-on")[0] == render("occluded-off", rope_preview=False)[0], "Foreground map geometry occludes the static rope helper")
    check(client.call("pick_at_pixel", pick_args)["object"] == wall, "Foreground picking wins over the rope tube")
    result = {"transport": "private-stdio", "checks": len(checks), "passed": checks}
    (output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"transport": result["transport"], "checks": len(checks)}))


def main():
    parser = argparse.ArgumentParser(description="Validate static rope preview in a private MCP process without desktop input.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--gameinfo", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    client = Client(None, args.binary.resolve(), output, args.gameinfo)
    try:
        validate(client, output)
    finally:
        client.close()


if __name__ == "__main__":
    main()
