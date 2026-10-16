import argparse
import hashlib
import json
import math
import os
from pathlib import Path
from unittest.mock import patch

from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_sky_preview import rgba_png
from validate_working_region import configure

SIZE = 401
HELPER = "frustum(_fov,_NearPlane,_FarPlane,_light,0.5)"
ORIGIN = [12, -24, 40]
ANGLES = [450.5, 37, 23]
FGD = '''
@BaseClass __HELPER__ = frustum_defaults [
    angles(angle) : "Angles" : "450.5 37 23"
    _fov(float) : "FOV" : 84
    _NearPlane(integer) readonly : "Near" : 16
    _FarPlane(float) : "Far" : 96
    _light(color255) : "Color" : "24 210 100 200"
]
@PointClass base(frustum_defaults) size(8 8 8) color(100 100 100) = frustum_sample []
'''
SOURCE_FGD = '''
@PointClass size(8 8 8) color(100 100 100) __HELPER__ = env_projectedtexture [
    angles(angle) : "Angles" : "30 40 25"
    lightfov(float) : "FOV" : 84
    nearz(float) : "Near" : 16
    farz(float) : "Far" : 96
    lightcolor(color255) : "Color" : "24 210 100 200"
    target(target_destination) : "Target"
    parentname(target_destination) : "Parent"
    cameraspace(integer) : "Camera space" : 0
]
'''


def corners(origin, angles, fov, near, far, scale):
    pitch, yaw, roll = map(math.radians, [-scale * angles[0], angles[1], angles[2]])
    cp, sp, cy, sy, cr, sr = math.cos(pitch), math.sin(pitch), math.cos(yaw), math.sin(yaw), math.cos(roll), math.sin(roll)
    result = []
    for distance in (near, far):
        extent = distance * math.tan(math.radians(fov / 2))
        for y, z in ((-extent, -extent), (extent, -extent), (extent, extent), (-extent, extent)):
            rx, ry, rz = distance, cr * y - sr * z, sr * y + cr * z
            px, py, pz = cp * rx + sp * rz, ry, -sp * rx + cp * rz
            result.append([origin[0] + cy * px - sy * py, origin[1] + sy * px + cy * py, origin[2] + pz])
    return result


def same_corners(actual, expected):
    return len(actual) == len(expected) == 8 and all(any(all(math.isclose(a, b, rel_tol=1e-12, abs_tol=1e-8) for a, b in zip(point, other, strict=True)) for other in actual) for point in expected)


def project(point, camera):
    if camera["projection"] != "perspective":
        axes = {"top": (0, 1), "front": (0, 2), "side": (1, 2)}[camera["projection"]]
        x, y = [(point[axis] - camera["center"][axis]) * SIZE / camera["ortho_height"] for axis in axes]
        return SIZE / 2 + x, SIZE / 2 - y
    forward = [b - a for a, b in zip(camera["position"], camera["look_at"], strict=True)]
    length = math.sqrt(sum(value * value for value in forward))
    forward = [value / length for value in forward]
    length = math.hypot(forward[0], forward[1])
    right = [forward[1] / length, -forward[0] / length, 0]
    up = [right[1] * forward[2], -right[0] * forward[2], right[0] * forward[1] - right[1] * forward[0]]
    delta = [a - b for a, b in zip(point, camera["position"], strict=True)]
    scale = SIZE / (2 * sum(a * b for a, b in zip(delta, forward, strict=True)) * math.tan(math.radians(camera.get("fov", 60) / 2)))
    return SIZE / 2 + scale * sum(a * b for a, b in zip(delta, right, strict=True)), SIZE / 2 - scale * sum(a * b for a, b in zip(delta, up, strict=True))


def validate(client, output):
    checks, images, receipts = [], [], {}
    fgd = output / "frustums.fgd"

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def definitions(helper=HELPER, template=FGD):
        fgd.write_text(template.replace("__HELPER__", helper))
        client.call("load_entity_definitions", {"path": str(fgd)})

    def inspect():
        return client.call("get_entity_inspector", {"id": entity})["helpers"]

    def guide():
        return inspect()["frustums"][0]

    def snapshot():
        info = client.call("get_map_info")
        return {"objects": detail(client, [entity]), "history": client.call("get_history"), "camera": client.call("get_camera"), **{key: info[key] for key in ("view", "selection", "unsaved_changes")}}

    def render(name, camera):
        png, metadata = client.render_result(width=SIZE, height=SIZE, camera=camera, selection_outlines=False)
        path = output / f"{name}.png"
        path.write_bytes(png)
        images.append({"file": path.name, "sha256": hashlib.sha256(png).hexdigest(), "camera": metadata["camera"]})
        return rgba_png(path)

    def green_difference(shown, plain):
        return sum(shown[i + 1] > shown[i] + 40 and shown[i + 1] > shown[i + 2] + 40 and max(abs(a - b) for a, b in zip(shown[i:i + 3], plain[i:i + 3], strict=True)) > 8 for i in range(0, len(shown), 4))

    def edge_samples(shown, plain, camera):
        edges = [(0, 1), (1, 2), (2, 3), (3, 0), (4, 5), (5, 6), (6, 7), (7, 4), (0, 4), (1, 5), (2, 6), (3, 7)]
        hits = 0
        for first_index, second_index in edges:
            for fraction in (0.25, 0.5, 0.75):
                point = [a + fraction * (b - a) for a, b in zip(expected[first_index], expected[second_index], strict=True)]
                x, y = project(point, camera)
                nearby = []
                for row in range(max(0, round(y) - 2), min(SIZE, round(y) + 3)):
                    for column in range(max(0, round(x) - 2), min(SIZE, round(x) + 3)):
                        offset = (row * SIZE + column) * 4
                        nearby.append(shown[offset + 1] > shown[offset] + 40 and shown[offset + 1] > shown[offset + 2] + 40 and shown[offset:offset + 3] != plain[offset:offset + 3])
                hits += any(nearby)
        return hits

    client.call("new_map", {"discard_changes": True})
    definitions()
    client.call("set_spatial_settings", {"grid_visible": False})
    client.call("set_transform_tool", {"mode": "select"})
    entity = client.call("create_entity", {"classname": "frustum_sample", "origin": ORIGIN})["id"]
    client.call("set_entity_properties", {"id": entity, "remove": ["_fov", "_FarPlane", "_light"]})
    first = guide()
    expected = corners(ORIGIN, ANGLES, 84, 16, 96, 0.5)
    check(first["state"] == "ready" and first["displayed"] and first["presentation"] == "separate" and first["pitch_scale_evaluated"] and first["pitch_scale"] == 0.5 and first["pitch_scale_source"] == "helper_argument", "Generic custom class evaluates its literal fractional FGD pitch scale as a separate guide")
    check(first["angles"] == ANGLES and first["effective_angles"] == [-225.25, 37, 23] and same_corners(first["corners"], expected), "Eight corners match independent T Rz Ry(-scale times raw pitch) Rx with pitch above 360 before multiplication")
    check(first["color"] == [24, 210, 100] and first["intensity"] == 200 and all(value["value_source"] == "fgd_default" for value in first["inputs"][2:]), "Typed scalar/color bindings retain individual FGD defaults and separate RGB/intensity")
    check(inspect()["source_projector"] is None and not client.call("get_map_info")["rendering"]["initialized"], "Generic FGD inspection is independent of Source projector and remains CPU-only")
    before = snapshot()
    client.call("set_entity_properties", {"id": entity, "set": {"_NearPlane": "8"}}, error=True)
    check(snapshot() == before, "Readonly scalar bindings reject authoring without source or history changes")
    for count in range(5):
        helper = "frustum(" + ",".join(["_fov", "_NearPlane", "_FarPlane", "_light"][:count]) + ")"
        definitions(helper)
        row = guide()
        check(row["state"] == "ready" and row["pitch_scale"] == 1 and row["pitch_scale_source"] == "helper_default" and same_corners(row["corners"], corners(ORIGIN, ANGLES, 84, 16, 96, 1)), f"{count} arguments retain omitted default bindings and scale1 without constructor substitution")
    definitions()
    check(snapshot() == before, "Helper reload and all omitted-argument forms preserve complete history and authored values")
    body_ray = {"origin": [12, -200, 40], "direction": [0, 1, 0]}
    body_hits = client.call("get_object_hit_candidates", body_ray)
    bounds = detail(client, [entity])[0]["bounds"]
    guide_ray = {"origin": expected[4], "direction": [0, 0, 1]}
    check(body_hits["total"] == 1 and client.call("get_object_hit_candidates", guide_ray)["total"] == 0, "Picking retains the ordinary class box and ignores an exact frustum corner")
    point = expected[4]
    volume = client.call("select_volume", {"mins": [v - 1 for v in point], "maxs": [v + 1 for v in point], "rule": "intersecting", "preview": True})["preview"]
    check(not volume["objects"], "Volume selection ignores frustum-only geometry")
    client.call("cancel_volume_selection", {"token": volume["token"]})
    client.call("frame_selection", {"aspect": 1})
    framed = client.call("get_camera")
    path = output / "frustum-source.vmf"
    client.call("save_map", {"path": str(path)})
    saved = path.read_bytes()
    baseline = snapshot()
    cameras = {projection: {"projection": projection, "center": ORIGIN, "ortho_height": 400} for projection in ("top", "front", "side")}
    cameras["perspective"] = {"projection": "perspective", "position": [300, -360, 320], "look_at": ORIGIN}
    shown = {projection: render(f"fgd-frustum-{projection}", camera) for projection, camera in cameras.items()}
    definitions("")
    check(not inspect()["frustums"], "Removing the FGD declaration removes the generic guide report")
    for projection, camera in cameras.items():
        plain = render(f"no-frustum-{projection}", camera)
        changed = green_difference(shown[projection], plain)
        check(changed > 100, f"Actual {projection} pixels contain a colored frustum beyond identical no-helper geometry")
        samples = edge_samples(shown[projection], plain, camera)
        check(samples >= 30, f"Actual {projection} guide pixels cover at least 30 of 36 independently projected CPU edge samples")
        receipts[projection] = {"guide_pixels": changed, "cpu_edge_samples": samples, "total_cpu_edge_samples": 36}
    client.call("frame_selection", {"aspect": 1})
    check(snapshot() == baseline and client.call("get_camera") == framed, "Removing guide geometry preserves bounds, source, camera, full history and ordinary framing")
    definitions()
    client.call("transform_objects", {"ids": [entity], "translation": [24, -16, 8], "preview": True})
    moved = guide()
    check(same_corners(moved["corners"], [[p[0] + 24, p[1] - 16, p[2] + 8] for p in expected]) and render("frustum-transform-preview", cameras["top"]) != shown["top"] and detail(client, [entity]) == baseline["objects"], "Displayed preview moves independent frustum corners and pixels without authoring source")
    client.call("cancel_transform_preview")
    check(snapshot() == baseline, "Cancelling frustum preview preserves complete history and exact committed values")
    client.call("set_entity_properties", {"id": entity, "set": {"_light": "24 210 100 0"}})
    check(guide()["intensity"] == 0 and render("frustum-zero-intensity", cameras["top"]) == shown["top"], "Fourth color component remains metadata intensity rather than guide opacity")
    client.call("undo")
    for value in ("0.02", "178"):
        client.call("set_entity_properties", {"id": entity, "set": {"_fov": value}})
        check(guide()["state"] == "ready" and guide()["fov_degrees"] == float(value), f"Nominal FOV boundary {value} is admitted without changing its reported value")
        client.call("undo")
    for key, value in [("_fov", "0.019"), ("_fov", "178.001"), ("_FarPlane", "16"), ("_light", "-1 -1 -1 200")]:
        client.call("set_entity_properties", {"id": entity, "set": {key: value}})
        row = guide()
        check(row["state"] == "invalid" and not row["displayed"] and row["corners"] is None and row["error"], f"Authored {key}={value} reports invalid without clamp or constructor fallback")
        actual = render("invalid-" + key.strip("_") + "-" + value.split()[0], cameras["top"])
        definitions("")
        check(render("invalid-no-helper-" + str(len(checks)), cameras["top"]) == actual, "Invalid values contribute no geometry against the same authored no-helper control")
        definitions()
        client.call("undo")
    for name, helper in [("alias", "frustum(_fov,_fov,_FarPlane,_light)"), ("reserved", "frustum(origin,_NearPlane,_FarPlane,_light)"), ("oversized-name", "frustum(" + "é" * 40 + ",_NearPlane,_FarPlane,_light)"), ("extra-argument", HELPER[:-1] + ",1)"), ("bad-scale", HELPER.replace("0.5", "NaN"))]:
        state = snapshot()
        definitions(helper)
        row = guide()
        check(row["state"] != "ready" and not row["displayed"] and row["error"] and row["corners"] is None, f"{name} helper admission reports an explicit unavailable guide")
        check(snapshot() == state, "Invalid declaration reload does not mutate document or complete current history")
    name = "k" * 79
    definitions(HELPER.replace("_fov", name), FGD.replace("_fov", name))
    check(guide()["state"] == "ready" and same_corners(guide()["corners"], expected), "A 79-byte declared binding remains complete without truncation")
    definitions(template=FGD.replace("_fov(float)", "_fov(string)"))
    check(guide()["state"] != "ready" and not guide()["displayed"], "Numeric-looking string declarations do not bypass strict scalar property kinds")
    definitions(template=FGD.replace('_fov(float) : "FOV" : 84', '_fov(float) : "FOV"'))
    missing = guide()
    check(missing["state"] == "invalid" and missing["corners"] is None and any(value["property"] == "_fov" and value["value_source"] == "missing" and value["raw"] is None for value in missing["inputs"]), "Missing FOV retains missing provenance instead of substituting the native constructor90")
    definitions(" ".join(HELPER.replace("0.5", str(scale)) for scale in range(17)))
    limited = inspect()
    check(len(limited["frustums"]) == 16 and limited["omitted_frustum_declarations"] == 1 and [row["declaration_index"] for row in limited["frustums"]] == list(range(16)), "Seventeen declarations report the first16 with an explicit omission count")
    definitions()
    client.call("set_object_visibility", {"action": "hide", "ids": [entity]})
    check(guide()["suppression_reason"] == "authored_hidden", "Authored hidden state suppresses an otherwise ready frustum")
    hidden = render("frustum-hidden", cameras["top"])
    definitions("")
    check(render("frustum-hidden-no-helper", cameras["top"]) == hidden, "Hidden source contributes no guide pixels against the identical hidden control")
    definitions()
    client.call("undo")
    configure(client, action="set", mins=[80, 80, 80], maxs=[160, 160, 160])
    check(guide()["suppression_reason"] == "outside_working_region", "Working-region admission follows the ordinary source box")
    configure(client, action="clear")
    client.call("select", {"ids": []})
    check(guide()["suppression_reason"] == "not_selected", "Unselected source retains intrinsic data and suppresses its guide")
    client.call("select", {"ids": [entity]})
    restored = snapshot()
    check(all(restored[key] == baseline[key] for key in restored if key != "history") and restored["history"]["undo_count"] == baseline["history"]["undo_count"] and path.read_bytes() == saved, "Reversible controls restore fields, saved bytes, camera and Undo count")
    check(render("frustum-restored", cameras["top"]) == shown["top"] and snapshot() == restored and client.call("get_object_hit_candidates", body_ray) == body_hits and detail(client, [entity])[0]["bounds"] == bounds, "Restored reads retain exact pixels, full current history and ordinary picks/bounds")
    client.call("new_map", {"discard_changes": True})
    native_helper = "frustum(lightfov,nearz,farz,lightcolor,-1)"
    definitions(native_helper, SOURCE_FGD)
    entity = client.call("create_entity", {"classname": "env_projectedtexture", "origin": ORIGIN})["id"]
    native = inspect()["source_projector"]
    shared = guide()
    check(shared["displayed"] and shared["presentation"] == "shared_with_source_projector" and shared["corners"] == native["corners"] and shared["color"] == native["color"] and shared["pitch_scale_evaluated"] and not native["helper_pitch_scale_evaluated"], "Equal FGD pitch-scale -1 geometry explicitly shares the independently oriented Source projector presentation")
    state = snapshot()
    shared_pixels = render("frustum-shared-source", cameras["perspective"])
    definitions("", SOURCE_FGD)
    source_only = render("source-projector-only", cameras["perspective"])
    check(source_only == shared_pixels and inspect()["source_projector"]["corners"] == native["corners"], "Equal guide deduplication leaves exactly the independent Source-only pixels")
    definitions(native_helper.replace(",-1)", ",1)"), SOURCE_FGD)
    different = guide()
    check(different["state"] == "ready" and not different["displayed"] and different["suppression_reason"] == "source_projector_precedence" and different["corners"] != native["corners"] and render("frustum-source-precedence", cameras["perspective"]) == source_only, "Nonidentical FGD pitch policy is explicitly suppressed without altering Source projector pixels")
    check(snapshot() == state and path.read_bytes() == saved, "Source coexistence inspection/render/reload preserves complete current history and the earlier saved source")
    definitions(native_helper, SOURCE_FGD)
    client.call("set_entity_properties", {"id": entity, "set": {"target": "unresolved_target"}})
    unavailable = guide()
    unavailable_source = inspect()["source_projector"]
    state = snapshot()
    check(unavailable["state"] == "ready" and not unavailable["displayed"] and unavailable["suppression_reason"] == "source_projector_precedence" and not unavailable_source["displayed"], "Unavailable specialized Source target branch still blocks a generic FGD bypass")
    unavailable_pixels = render("source-unavailable-with-frustum", cameras["perspective"])
    definitions("", SOURCE_FGD)
    check(render("source-unavailable-no-frustum", cameras["perspective"]) == unavailable_pixels and snapshot() == state, "Unavailable Source branch has identical no-frustum control pixels and preserves complete current history")
    (output / "results.json").write_text(json.dumps({"passed": len(checks), "checks": checks, "frustum": first, "expected_corners": expected, "shared": shared, "source_projector": native, "suppressed": different, "unavailable_source": unavailable_source, "unavailable_precedence": unavailable, "measurements": receipts, "images": images, "source_sha256": hashlib.sha256(saved).hexdigest(), "orientation_oracle": "Independent scalar Rx then Ry(-scale*raw_pitch) then Rz then origin; multiplication before angle reduction", "desktop_input": False, "native_exercised": False}, indent=2) + "\n")
    print(json.dumps({"passed": len(checks), "output": str(output)}))


def main():
    parser = argparse.ArgumentParser(description="Validate evaluated FGD frustums through private MCP, independent CPU corners and actual paired PNG controls.")
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
