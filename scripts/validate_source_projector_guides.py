import argparse
import json
import math
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_sky_preview import rgba_png
from validate_working_region import configure

SIZE = 401
FGD = '''
@BaseClass = projector_values [
    angles(angle) : "Pitch Yaw Roll (Y Z X)" : "0 0 0"
    lightfov(float) : "FOV" : 90
    nearz(float) : "NearZ" : 4
    farz(float) : "FarZ" : 750
    lightcolor(color255) : "Light Color" : "255 255 255 200"
    target(target_destination) : "target"
    parentname(target_destination) : "Parent"
    cameraspace(integer) : "Camera Space" : 0
]
@PointClass base(projector_values) size(-2 -2 -2, 2 2 2)
    frustum(lightfov,nearz,farz,lightcolor,-1) = env_projectedtexture []
'''


def validate(client, output):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def helpers():
        return client.call("get_entity_inspector", {"id": entity})["helpers"]

    def guide():
        return helpers()["source_projector"]

    def author(**values):
        client.call("set_entity_properties", {"id": entity, "set": values})

    def render(name, camera):
        png, _ = client.render_result(width=SIZE, height=SIZE, camera=camera, selection_outlines=False)
        path = output / f"{name}.png"
        path.write_bytes(png)
        return rgba_png(path)

    def green(pixels):
        return sum(pixels[i + 1] > pixels[i] + 50 and pixels[i + 1] > pixels[i + 2] + 50 for i in range(0, len(pixels), 4))

    fgd = output / "projector.fgd"
    fgd.write_text(FGD)
    client.call("new_map", {"discard_changes": True})
    client.call("load_entity_definitions", {"path": str(fgd)})
    client.call("set_spatial_settings", {"grid_visible": False})
    client.call("set_transform_tool", {"mode": "select"})
    entity = client.call("create_entity", {"classname": "env_projectedtexture", "origin": [0, 0, 16]})["id"]
    first = guide()
    check(first["displayed"] and first["fov_degrees"] == 90 and first["near_z"] == 4 and first["far_z"] == 750, "New projector uses its authored FGD defaults, including FOV90 rather than runtime constructor45")
    check(not first["helper_pitch_scale_evaluated"] and first["helper_pitch_scales"][0]["raw"] == "-1" and not first["helper_pitch_scales"][0]["evaluated"] and "pitch_scale is not evaluated" in helpers()["status"], "Independent Source-angle policy and unevaluated helper scale are explicit in shared inspection")
    check(len(first["corners"]) == 8 and all(math.isclose(a, b, abs_tol=1e-9) for a, b in zip(first["corners"][0], [4, 4, 12], strict=True)), "Zero Source angles face+X with right-Y and up+Z; near corners use axial distances")
    client.call("set_entity_properties", {"id": entity, "remove": ["angles", "lightfov", "nearz", "farz", "lightcolor", "cameraspace"]})
    fallback = guide()
    check(all(row["value_source"] == "fgd_default" for row in fallback["inputs"] if row["property"] in {"angles", "lightfov", "nearz", "farz", "lightcolor", "cameraspace"}), "Absent authored values retain labeled individual FGD-default provenance")
    check("FGD defaults:" in helpers()["status"] and "lightfov=90" in helpers()["status"] and "engine defaults may differ" in helpers()["status"], "The human summary exposes FGD-default FOV and its engine-default limitation")
    check(client.call("get_map_info")["rendering"]["initialized"] is False, "Projector inspection stays headless until actual pixels are requested")
    author(angles="0 0 0", lightfov="90", nearz="16", farz="96", lightcolor="24 210 100 200", cameraspace="0")
    check(guide()["color"] == [24, 210, 100] and guide()["intensity"] == 200, "RGB and authored fourth-component intensity remain separate")
    body_ray = {"origin": [0, -200, 16], "direction": [0, 1, 0]}
    guide_ray = {"origin": [60, -200, 16], "direction": [0, 1, 0]}
    hits = client.call("get_object_hit_candidates", body_ray)
    bounds = detail(client, [entity])[0]["bounds"]
    check(hits["total"] == 1 and client.call("get_object_hit_candidates", guide_ray)["total"] == 0, "Ordinary helper box is pickable while projector-only space is not")
    volume = client.call("select_volume", {"mins": [56, -8, 8], "maxs": [64, 8, 24], "rule": "intersecting", "preview": True})["preview"]
    check(volume["objects"] == [], "Volume selection ignores projector-only space")
    client.call("cancel_volume_selection", {"token": volume["token"]})
    client.call("frame_selection", {"aspect": 1})
    framed = client.call("get_camera")
    camera = {"projection": "top", "center": [48, 0, 16], "ortho_height": 256}
    shown = render("source-projector-guide", camera)
    check(green(shown) > 200, "Actual top-view pixels contain the green projector guide beyond its small box")
    for projection in ["front", "side"]:
        planar = {**camera, "projection": projection}
        visible = render(f"source-projector-{projection}", planar)
        author(lightfov="0")
        absent = render(f"invalid-projector-{projection}", planar)
        check(green(visible) > 150 and green(absent) == 0, f"Shared guide pixels appear in {projection} and disappear for invalid FOV without fallback")
        client.call("undo")
    perspective = {"projection": "perspective", "position": [200, -260, 200], "look_at": [48, 0, 16]}
    check(green(render("source-projector-perspective", perspective)) > 100, "The same twelve-edge guide renders in perspective")
    original = detail(client, [entity])
    history = client.call("get_history")["undo_count"]
    original_camera = client.call("get_camera")
    source = output / "source-projector.vmf"
    client.call("save_map", {"path": str(source)})
    saved = source.read_bytes()
    baseline = guide()
    check(guide() == baseline and detail(client, [entity]) == original and client.call("get_history")["undo_count"] == history, "Repeated inspector and render reads preserve authored fields and Undo history")
    fgd.write_text(FGD.replace(",-1)", ",100)"))
    client.call("load_entity_definitions", {"path": str(fgd)})
    check(guide()["helper_pitch_scales"][0]["raw"] == "100" and guide()["corners"] == baseline["corners"] and render("unevaluated-helper-scale", camera) == shown, "Changing unevaluated FGD pitch_scale changes provenance without changing Source-angle geometry or pixels")
    fgd.write_text(FGD)
    client.call("load_entity_definitions", {"path": str(fgd)})
    client.call("set_entity_color", {"id": entity, "key": "lightcolor", "color": [31, 150, 91]})
    color = guide()
    check(color["intensity"] == 200 and color["inputs"][5]["raw"] == "31 150 91 200", "Shared color-picker operation preserves the fourth component when editing RGB")
    client.call("undo")
    for key, raw in [("target", "aim"), ("parentname", "parent"), ("cameraspace", "1")]:
        author(**{key: raw})
        row = guide()
        check(row["state"] == "unsupported" and key in row["error"] and row["corners"] is None and green(render(f"unsupported-{key}", camera)) == 0, f"{key} branch reports unsupported and adds no substitute pixels")
        client.call("undo")
    author(angles="30 40 25")
    rotated = guide()
    center = [(a + b) / 2 for a, b in zip(rotated["corners"][0], rotated["corners"][2], strict=True)]
    check(math.isclose(center[2], 8, abs_tol=1e-9) and render("source-projector-rotated", camera) != shown, "Positive Source pitch moves the near center downward and rotation changes actual pixels")
    client.call("undo")
    client.call("transform_objects", {"ids": [entity], "translation": [16, 24, 32], "preview": True})
    check(guide()["origin"] == [16, 24, 48] and render("source-projector-transform-preview", camera) != shown, "The guide follows the displayed transform-preview origin")
    client.call("cancel_transform_preview")
    client.call("select", {"ids": []})
    check(not guide()["displayed"] and guide()["suppression_reason"] == "not_selected" and green(render("source-projector-unselected", camera)) == 0, "Unselected projector retains intrinsic data and suppresses guide pixels")
    client.call("select", {"ids": [entity]})
    configure(client, action="set", mins=[80, -32, 0], maxs=[144, 32, 32])
    check(guide()["suppression_reason"] == "outside_working_region" and green(render("source-projector-filtered", camera)) == 0, "Working-region inclusion follows the ordinary box even where the projector crosses the region")
    configure(client, action="clear")
    client.call("set_object_visibility", {"action": "hide", "ids": [entity]})
    check(guide()["suppression_reason"] == "authored_hidden" and green(render("source-projector-hidden", camera)) == 0, "Authored hidden state suppresses selected projector pixels")
    client.call("undo")
    check(render("source-projector-restored", camera) == shown and guide()["corners"] == baseline["corners"], "Restoring selection, visibility and previews returns exact original guide pixels")
    client.call("frame_selection", {"aspect": 1})
    check(client.call("get_camera") == framed and detail(client, [entity])[0]["bounds"] == bounds and client.call("get_object_hit_candidates", body_ray) == hits, "Projector extents never enlarge framing, bounds or hit candidates")
    check(detail(client, [entity]) == original and source.read_bytes() == saved and client.call("get_history")["undo_count"] == history and client.call("get_camera") == original_camera, "All reversible controls restore authored fields, saved bytes, Undo count and camera")
    (output / "results.json").write_text(json.dumps({"passed": len(checks), "checks": checks, "source_projector": baseline, "helpers": helpers(), "desktop_input": False, "native_exercised": False}, indent=2) + "\n")
    print(json.dumps({"passed": len(checks), "output": str(output)}))


def main():
    parser = argparse.ArgumentParser(description="Validate independent authored Source projector guides through private MCP operations and actual PNG controls.")
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
