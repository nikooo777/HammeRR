import argparse
import json
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_sky_preview import rgba_png
from validate_working_region import configure

SIZE = 401
FGD = '''
@BaseClass = sphere_defaults [
    outer(float) : "Outer radius" : 96
    inner(integer) : "Inner radius" : 48
    textual(string) : "Text radius" : "24"
]
@PointClass base(sphere_defaults) size(-8 -8 -8, 8 8 8) color(40 170 225)
    sphere(outer) sphere(inner) sphere(textual) = sphere_sample []
@PointClass size(-8 -8 -8, 8 8 8) sphere(zero) sphere(negative) sphere(missing)
    = sphere_edge_cases [
    zero(float) : "Zero radius" : 0
    negative(integer) : "Negative radius" : -16
    missing(float) : "Missing radius"
]
'''


def validate(client, output):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def inspect(entity):
        return client.call("get_entity_inspector", {"id": entity})["helpers"]

    def spheres(entity):
        return {row["property"]: row for row in inspect(entity)["spheres"]}

    def author(entity, **values):
        client.call("set_entity_properties", {"id": entity, "set": {key: str(value) for key, value in values.items()}})

    def render(name, camera):
        png, metadata = client.render_result(width=SIZE, height=SIZE, camera=camera, selection_outlines=False)
        path = output / f"{name}.png"
        path.write_bytes(png)
        return rgba_png(path), metadata

    def changed_outside(first, second, low, high):
        changed = 0
        for y in range(SIZE):
            for x in range(SIZE):
                radius = ((x - SIZE // 2) ** 2 + (y - SIZE // 2) ** 2) ** 0.5
                if low <= radius <= high:
                    offset = (y * SIZE + x) * 4
                    changed += max(abs(a - b) for a, b in zip(first[offset:offset + 3], second[offset:offset + 3], strict=True)) > 8
        return changed

    fgd = output / "spheres.fgd"
    fgd.write_text(FGD)
    client.call("new_map", {"discard_changes": True})
    client.call("load_entity_definitions", {"path": str(fgd)})
    client.call("set_spatial_settings", {"grid_visible": False})
    client.call("set_transform_tool", {"mode": "select"})
    entity = client.call("create_entity", {"classname": "sphere_sample", "origin": [0, 0, 16]})["id"]
    authored_defaults = spheres(entity)
    check(all(row["value_source"] == "authored" for row in authored_defaults.values()), "New entities retain their ordinary authored FGD defaults")
    client.call("set_entity_properties", {"id": entity, "remove": ["outer", "inner", "textual"]})
    defaults = spheres(entity)
    check({key: row["radius"] for key, row in defaults.items()} == {"outer": 96, "inner": 48, "textual": 24}, "Inherited float, integer and numeric string defaults resolve independently")
    check(all(row["value_source"] == "fgd_default" and row["state"] == "ready" and row["displayed"] for row in defaults.values()), "Missing authored values use explicit FGD fallback provenance and selected display eligibility")
    defaults_before = detail(client, [entity]), client.call("get_map_info")["undo"]
    fgd.write_text(FGD.replace('"Outer radius" : 96', '"Outer radius" : 112'))
    client.call("load_entity_definitions", {"path": str(fgd)})
    check(spheres(entity)["outer"]["radius"] == 112 and (detail(client, [entity]), client.call("get_map_info")["undo"]) == defaults_before, "Definition reload updates an absent property's guide default without authoring values or history")
    fgd.write_text(FGD)
    client.call("load_entity_definitions", {"path": str(fgd)})
    check(client.call("get_map_info")["rendering"]["initialized"] is False, "Sphere inspection and FGD reload remain headless until pixels are requested")
    author(entity, outer=128, inner=64, textual=32)
    positive = spheres(entity)
    check(all(row["value_source"] == "authored" for row in positive.values()) and positive["outer"]["radius"] == 128, "Authored radii override defaults without changing the other guides")
    check(positive["outer"]["center"] == [0, 0, 16] and positive["outer"]["guide_bounds"] == [[-128, -128, -112], [128, 128, 144]], "Guide center and potential radius bounds are exact Source-unit metadata")
    box = {"mins": [-8, -8, 8], "maxs": [8, 8, 24]}
    check(detail(client, [entity])[0]["bounds"] == box and inspect(entity)["world_bounds"] == list(box.values()), "Radius bounds remain separate from the entity's ordinary class box")
    body_ray = {"origin": [0, -200, 16], "direction": [0, 1, 0]}
    ring_ray = {"origin": [100, -200, 16], "direction": [0, 1, 0]}
    body_hit = client.call("get_object_hit_candidates", body_ray)
    check(body_hit["total"] == 1 and client.call("get_object_hit_candidates", ring_ray)["total"] == 0, "Rays pick the ordinary box and pass through guide-only positions")
    volume = client.call("select_volume", {"mins": [96, -16, 8], "maxs": [128, 16, 24], "rule": "intersecting", "preview": True})
    check(volume["preview"]["objects"] == [], "Volume selection ignores guide-only space")
    client.call("cancel_volume_selection", {"token": volume["preview"]["token"]})
    client.call("frame_selection", {"aspect": 1})
    framed = client.call("get_camera")
    author(entity, outer=0, inner=0, textual=0)
    check(all(row["state"] == "nonpositive" and not row["displayed"] and row["value_source"] == "authored" for row in spheres(entity).values()), "Explicit zero disables guides without falling back to positive defaults")
    client.call("frame_selection", {"aspect": 1})
    check(client.call("get_camera") == framed, "Selection framing ignores sphere extents")
    camera = {"projection": "top", "center": [0, 0, 16], "ortho_height": 320}
    empty, _ = render("selected-box-without-spheres", camera)
    author(entity, outer=128, inner=64, textual=32)
    rings, _ = render("fgd-spheres", camera)
    check(changed_outside(empty, rings, 45, 180) > 300, "Actual MCP pixels contain radius guides beyond the unchanged selected box")
    author(entity, inner=0)
    no_inner, _ = render("outer-and-text-radii", camera)
    check(changed_outside(no_inner, rings, 76, 84) > 60, "The independently authored inner radius creates a distinct visible ring")
    author(entity, inner=64)
    for projection in ["front", "side"]:
        planar = {**camera, "projection": projection}
        shown, _ = render(f"sphere-{projection}", planar)
        author(entity, outer=0, inner=0, textual=0)
        absent, _ = render(f"no-sphere-{projection}", planar)
        check(changed_outside(shown, absent, 45, 180) > 300, f"The same sphere guides render in {projection} projection")
        author(entity, outer=128, inner=64, textual=32)
    perspective = {"projection": "perspective", "position": [260, -320, 240], "look_at": [0, 0, 16]}
    shown, _ = render("sphere-perspective", perspective)
    author(entity, outer=0, inner=0, textual=0)
    absent, _ = render("no-sphere-perspective", perspective)
    check(shown != absent, "The shared guides also render in perspective")
    author(entity, outer=128, inner=64, textual=32)
    check(client.call("get_object_hit_candidates", body_ray) == body_hit and detail(client, [entity])[0]["bounds"] == box, "Rendered rings leave ordinary hits and object bounds unchanged")
    source = output / "authored.vmf"
    client.call("save_map", {"path": str(source)})
    saved = source.read_bytes()
    before = client.call("get_map_info")
    original = detail(client, [entity])
    camera_before = client.call("get_camera")
    client.call("set_entity_properties", {"id": entity, "set": {"inner": "72", "outer": "not a number"}}, error=True)
    check(detail(client, [entity]) == original and client.call("get_map_info")["undo"] == before["undo"], "A malformed numeric batch rejects atomically before either radius changes")
    first = inspect(entity)
    check(inspect(entity) == first and detail(client, [entity]) == original, "Repeated structured sphere inspection is read-only")
    client.call("select", {"ids": []})
    unselected = spheres(entity)
    check(all(row["state"] == "ready" and not row["displayed"] and row["suppression_reason"] for row in unselected.values()), "Unselected entities retain intrinsic radii and explain why their guides are suppressed")
    render("unselected-spheres-suppressed", camera)
    client.call("select", {"ids": [entity]})
    region = configure(client, action="set", mins=[80, -16, 0], maxs=[144, 16, 32])
    check(not region["objects"][0]["included"] and all(not row["displayed"] for row in spheres(entity).values()), "Working-region filtering follows the ordinary box even where the sphere crosses the region")
    filtered, _ = render("filtered-spheres-suppressed", camera)
    check(filtered != rings and client.call("get_object_hit_candidates", body_ray)["total"] == 0, "Filtered entities suppress real guide pixels and ordinary hits together")
    configure(client, action="clear")
    client.call("set_object_visibility", {"action": "hide", "ids": [entity]})
    check(all(not row["displayed"] and row["suppression_reason"] for row in spheres(entity).values()), "Authored hidden state suppresses selected guides with explicit diagnostics")
    hidden, _ = render("hidden-spheres-suppressed", camera)
    check(hidden != rings, "Hidden source guide pixels disappear from the MCP render")
    client.call("undo")
    check(spheres(entity)["outer"]["displayed"], "Undo restores source visibility and guide eligibility")
    author(entity, textual="broken")
    malformed_before = detail(client, [entity]), client.call("get_map_info")["undo"]
    malformed = spheres(entity)["textual"]
    check(malformed["state"] == "invalid" and malformed["radius"] is None and malformed["error"] and not malformed["displayed"], "Malformed imported/string radius reports failure without a substitute guide")
    inspect(entity)
    malformed_pixels, _ = render("malformed-string-radius", camera)
    check((detail(client, [entity]), client.call("get_map_info")["undo"]) == malformed_before, "Malformed-radius report and rendering do not repair source or add history")
    author(entity, textual=0)
    check(render("suppressed-string-radius", camera)[0] == malformed_pixels, "Malformed radius adds no substitute ring or box change")
    client.call("undo")
    client.call("undo")
    edge = client.call("create_entity", {"classname": "sphere_edge_cases", "origin": [512, 0, 16]})["id"]
    edges = spheres(edge)
    check(edges["zero"]["state"] == edges["negative"]["state"] == "nonpositive" and not edges["zero"]["displayed"] and not edges["negative"]["displayed"], "Zero and negative radii explicitly produce no editor guide")
    check(edges["missing"]["value_source"] == "missing" and edges["missing"]["radius"] is None and edges["missing"]["error"], "A missing radius and default produce an explicit unresolved diagnostic")
    client.call("undo")
    client.call("select", {"ids": [entity]})
    check(source.read_bytes() == saved and detail(client, [entity]) == original, "Guide inspection and visibility checks retain saved source and authored fields")
    check(client.call("get_map_info")["undo"] == before["undo"] and client.call("get_camera") == camera_before, "Read/render/filter workflows retain map history and the active camera")
    check(render("restored-spheres", camera)[0] == rings, "Restored guide settings reproduce identical actual pixels")
    (output / "results.json").write_text(json.dumps({"passed": len(checks), "checks": checks, "helpers": first, "edge_cases": edges, "malformed": malformed, "desktop_input": False}, indent=2) + "\n")
    print(json.dumps({"passed": len(checks), "output": str(output)}))


def main():
    parser = argparse.ArgumentParser(description="Validate selected FGD sphere guides through private shared MCP operations and actual PNG renders.")
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
