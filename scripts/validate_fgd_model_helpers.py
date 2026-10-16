import argparse
import hashlib
import json
import math
import os
from pathlib import Path
from unittest.mock import patch

from validate_entity_placement import begin, cancel, pick
from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_sky_preview import rgba_png
from validate_working_region import configure

SIZE = 401
MODEL = "models/props_junk/wood_crate001a.mdl"
LITERAL = MODEL.upper()


def definitions(helper):
    classes = [
        f'@PointClass size(4 4 4) {helper} = helper_sample [ angles(angle) : "Angles" : "0 0 0" ]',
        '@PointClass = choice_target [ model(studio) : "World Model" : "" ]',
        '@PointClass size(4 4 4) = info_target []',
    ]
    for name, properties in (("light", ""), ("move_rope", ""), ("info_overlay", ""), ("model_property", 'model(studio) : "World Model"')):
        classes.append(f'@PointClass size(4 4 4) studio("{MODEL}") = {name} [{properties}]')
    return "\n".join(classes) + "\n"


def changed_pixels(first, second):
    return [offset for offset in range(0, len(first), 4) if first[offset:offset + 3] != second[offset:offset + 3]]


def close(first, second):
    return all(math.isclose(a, b, abs_tol=1e-5) for a, b in zip(first, second, strict=True))


def validate(client, output):
    checks, images, measurements = [], [], {}

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def load(helper=f'studio("{LITERAL}")'):
        fgd.write_text(definitions(helper))
        client.call("load_entity_definitions", {"path": str(fgd)})

    def inspect(object_id=None):
        return client.call("get_entity_inspector", {"id": object_id or entity})["helpers"]

    def guide(object_id=None):
        return inspect(object_id)["model_guide"]

    def render(name, camera):
        png, metadata = client.render_result(width=SIZE, height=SIZE, camera=camera, selection_outlines=False)
        path = output / f"{name}.png"
        path.write_bytes(png)
        images.append({"file": path.name, "sha256": hashlib.sha256(png).hexdigest(), "camera": metadata["camera"]})
        return rgba_png(path)

    def placement(classname, origin):
        token = begin(client, classname, snap=False)
        plan = pick(client, token, origin, [0, 0, -1])["draft"]["plan"]
        cancel(client, token)
        return {key: plan[key] for key in ("origin", "bounds", "bounds_source", "target", "point", "normal", "contact_offset", "support_points")}

    client.call("new_map", {"discard_changes": True})
    fgd = output / "model-helpers.fgd"
    load()
    client.call("set_spatial_settings", {"grid_visible": False})
    client.call("set_transform_tool", {"mode": "select"})
    asset = client.call("inspect_model", {"path": MODEL})
    check(asset["static_prop"] and asset["pose"] == "static" and asset["triangles"] > 12 and asset["vertices"] > 24, "Existing mounted static crate supplies independently decoded three-dimensional model geometry")
    floor = client.call("create_block", {"mins": [-128, -128, -16], "maxs": [128, 128, 0], "material": "dev/dev_measuregeneric01"})["id"]
    choice_target = client.call("create_entity", {"classname": "choice_target", "origin": [2000, 2000, 2000]})["id"]
    entity = client.call("create_entity", {"classname": "helper_sample", "origin": [0, 0, 32]})["id"]
    first = guide()
    check(first["source"] == "fgd_literal" and first["helper"] == "studio" and first["literal"] == LITERAL and not first["literal_truncated"] and first["path"] == MODEL and str(fgd) in inspect()["definition_sources"], "Shared report distinguishes exact loaded FGD declaration provenance from its canonical mounted model path")
    check(first["declaration_supported"] and first["state"] == "ready" and first["displayed"] and first["pose"] == "static" and first["skin"] == first["body"] == 0 and first["material_policy"] == "flat_editor", "Literal model guide reports its ready static skin0/body0 flat editor mesh")
    check(not client.call("get_map_info")["rendering"]["initialized"], "Model and helper inspection remain headless until actual PNG rendering")
    bounds = {"mins": [-2, -2, 30], "maxs": [2, 2, 34]}
    check(detail(client, [entity])[0]["bounds"] == bounds and inspect()["world_bounds"] == [bounds["mins"], bounds["maxs"]] and first["visual_bounds"] != [bounds["mins"], bounds["maxs"]], "Visual mesh bounds are separate from the exact ordinary FGD box")
    body_ray = {"origin": [0, -100, 32], "direction": [0, 1, 0]}
    glyph_ray = {"origin": [12, -100, 56], "direction": [0, 1, 0]}
    body_hits = client.call("get_object_hit_candidates", body_ray)
    check(body_hits["total"] == 1 and client.call("get_object_hit_candidates", glyph_ray)["total"] == 0, "The ordinary box is pickable while model-only image space adds no object hit")
    volume = client.call("select_volume", {"mins": [10, -2, 54], "maxs": [14, 2, 58], "rule": "intersecting", "preview": True})["preview"]
    check(not volume["objects"], "Volume selection ignores the literal guide's larger visual mesh")
    client.call("cancel_volume_selection", {"token": volume["token"]})
    contact = placement("helper_sample", [80, 80, 128])
    through = placement("info_target", [12, 0, 128])
    check(contact["bounds_source"] == "editor_helper" and close(contact["origin"], [80, 80, 2]) and contact["bounds"] == {"mins": [78, 78, 0], "maxs": [82, 82, 4]}, "New helper contact uses the four-unit FGD box rather than its visual model")
    check(through["target"] == floor and close(through["point"], [12, 0, 0]), "Editor guide triangles never become contextual placement surface hits")
    client.call("select", {"ids": [entity]})
    client.call("frame_selection", {"aspect": 1})
    camera_before = client.call("get_camera")
    source = output / "model-helpers.vmf"
    client.call("save_map", {"path": str(source)})
    saved = source.read_bytes()
    authored = detail(client, [floor, choice_target, entity])
    history = client.call("get_history")["undo_count"]
    client.call("begin_model_choice", {"id": entity}, error=True)
    token = client.call("begin_model_choice", {"id": choice_target})["token"]
    client.call("choose_model", {"token": token, "path": MODEL})
    choice = client.call("get_model_choice", {"token": token})
    low, high = first["visual_bounds"]
    center = [(a + b) / 2 for a, b in zip(low, high, strict=True)]
    cameras = {projection: {"projection": projection, "center": center, "ortho_height": 128} for projection in ("top", "front", "side")}
    cameras["perspective"] = {"projection": "perspective", "position": [-128, -144, 136], "look_at": center}
    shown = {name: render(f"model-helper-{name}", camera) for name, camera in cameras.items()}
    check(client.call("get_model_choice", {"token": token}) == choice and client.call("get_camera") == camera_before and detail(client, [floor, choice_target, entity]) == authored and client.call("get_history")["undo_count"] == history, "Read-only guide inspection and all projection renders preserve an independent pending model choice, camera, source fields and history")
    client.call("cancel_model_choice", {"token": token})
    load("")
    check(guide() is None, "Removing the FGD declaration removes the visual guide without writing a model property")
    plain_views, mesh_masks = {}, {}
    for name, camera in cameras.items():
        plain = render(f"ordinary-box-{name}", camera)
        plain_views[name] = plain
        changed = changed_pixels(shown[name], plain)
        mesh_masks[name] = changed
        check(len(changed) > 1000, f"Actual flat mesh pixels exceed the ordinary marker in {name}")
        measurements[name] = {"changed_pixels": len(changed)}
    client.call("frame_selection", {"aspect": 1})
    check(client.call("get_camera") == camera_before and detail(client, [entity])[0]["bounds"] == bounds and client.call("get_object_hit_candidates", body_ray) == body_hits, "Removing the visual guide preserves exact ordinary framing, bounds and body hit candidates")
    check(placement("helper_sample", [80, 80, 128]) == contact and placement("info_target", [12, 0, 128]) == through, "Guide removal leaves source contact and target surface placement plans identical")
    load(f'studioprop("{MODEL}")')
    check(guide()["helper"] == "studioprop" and guide()["path"] == MODEL and guide()["displayed"] and render("studioprop-same-mesh", cameras["perspective"]) == shown["perspective"], "One-argument studioprop shares the same canonical visual mesh")
    load()
    client.call("select", {"ids": []})
    unselected = render("unselected-model-helper", cameras["top"])
    mesh_only = [offset for offset in mesh_masks["top"] if abs((offset // 4) % SIZE - SIZE // 2) > 12 or abs((offset // 4) // SIZE - SIZE // 2) > 12]
    check(guide()["displayed"] and len(mesh_only) > 500 and all(unselected[offset:offset + 3] == shown["top"][offset:offset + 3] for offset in mesh_only), "Unselected literal model guides retain exact mesh pixels outside the ordinary selection box")
    client.call("select", {"ids": [entity]})
    client.call("set_entity_properties", {"id": entity, "set": {"angles": "0 30 0"}})
    check(guide()["visual_bounds"] != first["visual_bounds"] and detail(client, [entity])[0]["bounds"] == bounds and render("model-helper-authored-angles", cameras["perspective"]) != shown["perspective"], "Shared authored angles rotate only the visual mesh while the ordinary box stays world-aligned")
    client.call("undo")
    client.call("transform_objects", {"ids": [entity], "translation": [16, 8, 12], "preview": True})
    moved = guide()["visual_bounds"]
    check(all(close(actual, [value + delta for value, delta in zip(original, [16, 8, 12], strict=True)]) for original, actual in zip(first["visual_bounds"], moved, strict=True)) and render("model-helper-transform-preview", cameras["perspective"]) != shown["perspective"], "Visual bounds and actual mesh pixels follow the shared transform preview")
    check(detail(client, [floor, choice_target, entity]) == authored, "Transform preview leaves committed authored fields unchanged")
    client.call("cancel_transform_preview")
    configure(client, action="set", mins=[8, -8, 32], maxs=[24, 8, 80])
    check(not guide()["displayed"] and guide()["suppression_reason"] == "outside_working_region", "Working regions use the ordinary box even when the visual model crosses the region")
    filtered = render("model-helper-filtered", cameras["perspective"])
    load("")
    check(render("model-helper-filtered-control", cameras["perspective"]) == filtered, "Region-excluded source contributes no actual guide pixels against the same filtered no-guide control")
    load()
    configure(client, action="clear")
    client.call("set_object_visibility", {"action": "hide", "ids": [entity]})
    check(not guide()["displayed"] and guide()["suppression_reason"] == "authored_hidden", "Authored-hidden source explicitly suppresses its model guide")
    hidden = render("model-helper-hidden", cameras["perspective"])
    load("")
    check(render("model-helper-hidden-control", cameras["perspective"]) == hidden, "Authored-hidden source contributes no actual guide pixels against the same hidden no-guide control")
    load()
    client.call("undo")
    client.call("set_entity_properties", {"id": entity, "set": {"model": ""}})
    check(not guide()["displayed"] and guide()["suppression_reason"] == "authored_model", "An authored empty model field keeps precedence without a literal fallback")
    client.call("undo")
    for classname, reason in (("light", "specialized_light"), ("move_rope", "specialized_rope"), ("info_overlay", "specialized_helper"), ("model_property", "model_property")):
        other = client.call("create_entity", {"classname": classname, "origin": [400, 400, 400]})["id"]
        row = guide(other)
        check(not row["displayed"] and row["suppression_reason"] == reason, f"Existing {classname} semantics retain precedence over literal studio")
        client.call("undo")
    client.call("select", {"ids": [entity]})
    load('studio("models/hammerr_missing_model_helper.mdl")')
    missing = guide()
    check(missing["declaration_supported"] and missing["state"] == "unavailable" and not missing["displayed"] and missing["error"], "Missing literal model is an explicit unavailable asset, with no healthy guide fallback")
    check(render("missing-model-helper", cameras["perspective"]) == plain_views["perspective"], "Missing asset leaves the same ordinary editable box pixels as a removed declaration")
    for declaration in ("studio()", f'studio("{MODEL}", 1)', f'studio("{MODEL}") studioprop("{MODEL}")'):
        load(declaration)
        row = guide()
        check(not row["declaration_supported"] and row["state"] == "unsupported" and not row["displayed"] and row["error"], f"Unsupported or ambiguous declaration is explicit: {declaration}")
    load()
    reloaded = client.call("reload_model_assets")
    check(reloaded["reloaded"] and guide()["displayed"] and guide()["path"] == MODEL and render("model-helper-reloaded", cameras["perspective"]) == shown["perspective"], "Shared model reload and repaired declaration restore exact original mesh pixels")
    client.call("frame_selection", {"aspect": 1})
    check(client.call("get_camera") == camera_before and detail(client, [floor, choice_target, entity]) == authored and source.read_bytes() == saved and client.call("get_history")["undo_count"] == history and client.call("get_model_choice")["token"] is None, "All reversible controls restore authored objects, saved source, camera and Undo count without creating a model choice")
    check(dict(detail(client, [entity])[0]["properties"]).get("model") is None and client.call("get_object_hit_candidates", glyph_ray)["total"] == 0, "Literal helper never writes an authored model or enables visual-mesh picking")
    (output / "results.json").write_text(json.dumps({"passed": len(checks), "checks": checks, "model_guide": first, "missing": missing, "asset": asset, "placement": {"source_contact": contact, "through_guide": through}, "measurements": measurements, "images": images, "source_sha256": hashlib.sha256(saved).hexdigest(), "provenance_limit": "Loaded FGD definition source and canonical mounted model path are checked; mounted MDL/VVD/VTX file origins are not exposed by this report.", "desktop_input": False, "native_exercised": False}, indent=2) + "\n")
    print(json.dumps({"passed": len(checks), "output": str(output)}))


def main():
    parser = argparse.ArgumentParser(description="Validate literal FGD studio/studioprop editor guides through private MCP and actual mounted static mesh pixels.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--gameinfo", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    with patch.dict(os.environ, {"HAMMERR_RECENT_MAPS_DIR": str(output / "recent"), "HAMMERR_BUILD_RECIPES": str(output / "recipes.json")}):
        client = Client(None, args.binary.resolve(), output, args.gameinfo)
    try:
        validate(client, output)
    finally:
        client.close()


if __name__ == "__main__":
    main()
