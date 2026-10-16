import argparse
import hashlib
import json
import os
import struct
from pathlib import Path
from unittest.mock import patch

from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_sky_preview import rgba_png
from validate_working_region import configure

SIZE = 401
COLORS = ((24, 220, 72), (235, 45, 35), (30, 70, 235), (240, 205, 25))
FGD = '''
@PointClass size(-2 -2 -2, 2 2 2) iconsprite("audit/ring.vmt") = icon_sample [
    angles(angle) : "Angles" : "0 0 0"
]
'''


def texture(hole):
    header = bytearray(80)
    header[:4] = b"VTF\0"
    struct.pack_into("<IIIHH", header, 4, 7, 2, 80, 32, 32)
    struct.pack_into("<I", header, 20, 0x2340)
    for offset in (24, 63):
        struct.pack_into("<H", header, offset, 1)
    struct.pack_into("<3f", header, 32, 0.5, 0.5, 0.5)
    struct.pack_into("<f", header, 48, 1.0)
    header[56] = 1
    struct.pack_into("<i", header, 57, -1)
    pixels = bytes(
        channel
        for y in range(32)
        for x in range(32)
        for channel in (*COLORS[(y >= 16) * 2 + (x >= 16)], 0 if hole and 8 <= x < 24 and 8 <= y < 24 else 255)
    )
    return bytes(header) + pixels


def fixture(output):
    game = output / "game"
    first, second = [game / mount / "materials/audit" for mount in ("first", "second")]
    first.mkdir(parents=True)
    second.mkdir(parents=True)
    for name, hole in (("ring", True), ("opaque", False)):
        (first / f"{name}.vtf").write_bytes(texture(hole))
        (first / f"{name}.vmt").write_text(f'Sprite {{ $basetexture audit/{name} $spriteorientation vp_parallel $spriteorigin "[0.5 0.5]" $no_fullbright 1 }}\n')
    (first / "unsupported.vmt").write_text("LightmappedGeneric { $basetexture audit/ring }\n")
    (first / "bad_texture.vmt").write_text('Sprite { $basetexture audit/bad_texture $spriteorientation vp_parallel $spriteorigin "[0.5 0.5]" }\n')
    (first / "bad_texture.vtf").write_bytes(b"invalid VTF fixture")
    (second / "ring.vmt").write_text("UnlitGeneric { $basetexture audit/opaque }\n")
    (second / "opaque.vtf").write_bytes(texture(False))
    gameinfo = game / "gameinfo.txt"
    gameinfo.write_text('GameInfo { FileSystem { SearchPaths { game "|gameinfo_path|first" game "|gameinfo_path|second" } } }\n')
    return gameinfo, first


def rgb(pixels, x, y):
    offset = (y * SIZE + x) * 4
    return pixels[offset:offset + 3]


def different(first, second, x, y):
    return rgb(first, x, y) != rgb(second, x, y)


def color_kind(pixel):
    red, green, blue = pixel
    if green > red + 60 and green > blue + 60:
        return "green"
    if red > green + 60 and red > blue + 60:
        return "red"
    if blue > red + 60 and blue > green + 60:
        return "blue"
    if red > blue + 60 and green > blue + 60:
        return "yellow"
    return None


def image_receipt(path):
    return {"file": path.name, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}


def validate(client, output, materials):
    checks, images, measurements = [], [], {}

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def definitions(material="audit/ring.vmt", extra=""):
        fgd.write_text(FGD.replace("audit/ring.vmt", material).replace("= icon_sample", f"{extra} = icon_sample"))
        client.call("load_entity_definitions", {"path": str(fgd)})

    def report():
        return client.call("get_entity_inspector", {"id": entity})["helpers"]["icon_sprite"]

    def render(name, camera, **options):
        png, metadata = client.render_result(width=SIZE, height=SIZE, camera=camera, selection_outlines=False, **options)
        path = output / f"{name}.png"
        path.write_bytes(png)
        images.append(image_receipt(path))
        return rgba_png(path), metadata

    def patches(pixels):
        return [
            color_kind(rgb(pixels, round(low_x + fraction_x * width), round(low_y + fraction_y * height)))
            for fraction_y in (0.125, 0.875)
            for fraction_x in (0.125, 0.875)
        ]

    client.call("new_map", {"discard_changes": True})
    fgd = output / "icons.fgd"
    definitions()
    client.call("set_spatial_settings", {"grid_visible": False})
    client.call("set_transform_tool", {"mode": "select"})
    entity = client.call("create_entity", {"classname": "icon_sample", "origin": [0, 0, 0]})["id"]
    ready = report()
    check(ready["status"] == "ready" and ready["displayed"] and ready["error"] is None, "Literal Sprite helper is admitted through shared entity inspection")
    check(ready["material"] == "materials/audit/ring.vmt" and ready["texture"] == "materials/audit/ring.vtf" and ready["source"]["file"] == str(materials / "ring.vmt") and ready["texture_source"]["file"] == str(materials / "ring.vtf"), "Both winning material and texture provenance identify the first private mount")
    check(ready["width"] == ready["height"] == 32 and ready["world_size"] == [32, 32] and ready["orientation_policy"] == "live_camera_right_up" and ready["rendering_policy"] == "unlit_rgba_editor_image", "Shared report exposes the admitted image dimensions, fixed Source-unit size and rendering policies")
    check(not client.call("get_map_info")["rendering"]["initialized"], "Asset inspection stays headless before actual PNG rendering")
    body_ray = {"origin": [0, -100, 0], "direction": [0, 1, 0]}
    outside_ray = {"origin": [12, -100, 0], "direction": [0, 1, 0]}
    body_hits = client.call("get_object_hit_candidates", body_ray)
    bounds = detail(client, [entity])[0]["bounds"]
    check(body_hits["total"] == 1 and client.call("get_object_hit_candidates", outside_ray)["total"] == 0, "Ordinary helper box remains pickable while the larger image is not")
    volume = client.call("select_volume", {"mins": [10, -1, -1], "maxs": [14, 1, 1], "rule": "intersecting", "preview": True})["preview"]
    check(not volume["objects"], "Volume selection ignores image-only space")
    client.call("cancel_volume_selection", {"token": volume["token"]})
    client.call("frame_selection", {"aspect": 1})
    camera_before = client.call("get_camera")
    source = output / "source.vmf"
    client.call("save_map", {"path": str(source)})
    saved, authored = source.read_bytes(), detail(client, [entity])
    history = client.call("get_history")["undo_count"]
    top = {"projection": "top", "center": [0, 0, 0], "ortho_height": 128}
    ring, _ = render("icon-ring", top)
    definitions("audit/opaque.vmt")
    opaque, _ = render("icon-opaque-alpha-control", top)
    definitions("audit/missing.vmt")
    missing = report()
    absent, _ = render("icon-missing-control", top)
    check(missing["status"] == "unavailable" and not missing["displayed"] and missing["error"], "A missing material reports unavailable and leaves a diagnostic marker")
    client.call("frame_selection", {"aspect": 1})
    check(client.call("get_camera") == camera_before and detail(client, [entity])[0]["bounds"] == bounds and client.call("get_object_hit_candidates", body_ray) == body_hits, "Missing-icon control preserves exactly the admitted icon's framing, box and body pick")
    changed = [(x, y) for y in range(SIZE) for x in range(SIZE) if different(opaque, absent, x, y)]
    check(len(changed) > 1000, "Opaque control proves the helper occupies actual pixels beyond the marker")
    low_x, high_x = min(x for x, _ in changed), max(x for x, _ in changed)
    low_y, high_y = min(y for _, y in changed), max(y for _, y in changed)
    width, height = high_x - low_x, high_y - low_y
    check(80 <= width <= 120 and abs(width - height) <= 2, "Square Source-unit icon has the expected orthographic screen extent")
    hole = [(x, y) for x, y in changed if 0.34 < (x - low_x) / width < 0.66 and 0.34 < (y - low_y) / height < 0.66 and (abs((x - low_x) / width - 0.5) > 0.11 or abs((y - low_y) / height - 0.5) > 0.11)]
    retained = [(x, y) for x, y in changed if 0.05 < (x - low_x) / width < 0.95 and 0.05 < (y - low_y) / height < 0.95 and ((x - low_x) / width < 0.18 or (x - low_x) / width > 0.82 or (y - low_y) / height < 0.18 or (y - low_y) / height > 0.82)]
    check(len(hole) > 150 and all(not different(ring, absent, x, y) and different(opaque, absent, x, y) for x, y in hole), "Interior alpha-zero pixels exactly match the missing-icon control while identical opaque RGB covers them")
    check(len(retained) > 1000 and all(not different(ring, opaque, x, y) for x, y in retained), "Opaque ring pixels exactly match the otherwise identical all-opaque texture")
    expected_patches = patches(ring)
    check(set(expected_patches) == {"green", "red", "blue", "yellow"}, "Four asymmetric source-color patches appear independently of the generic marker")
    measurements["alpha"] = {"extent": [low_x, low_y, high_x, high_y], "hole_pixels": len(hole), "opaque_ring_pixels": len(retained), "screen_quadrants": expected_patches}
    definitions()
    for projection in ("top", "front", "side"):
        pixels, _ = render(f"icon-{projection}", {**top, "projection": projection})
        check(patches(pixels) == expected_patches, f"Camera-facing asymmetric patches retain screen orientation in {projection}")
    perspectives = [
        {"projection": "perspective", "position": [-128, 0, 0], "look_at": [0, 0, 0], "fov": 60},
        {"projection": "perspective", "position": [0, -128, 0], "look_at": [0, 0, 0], "fov": 60},
    ]
    perspective_pixels, perspective_cameras = [], []
    for index, camera in enumerate(perspectives):
        pixels, metadata = render(f"icon-perspective-{index}", camera)
        perspective_pixels.append(pixels)
        perspective_cameras.append(metadata["camera"])
        check(patches(pixels) == expected_patches, f"Cached scene billboards toward perspective camera {index} without a document or selection edit")
    center = SIZE // 2
    billboard_samples = [(center + offset_x + dx, center + offset_y + dy) for offset_y in (-30, 30) for offset_x in (-30, 30) for dy in range(-3, 4) for dx in range(-3, 4)]
    check(perspective_cameras[0]["forward"] != perspective_cameras[1]["forward"] and all(not different(perspective_pixels[0], perspective_pixels[1], x, y) for x, y in billboard_samples), "Opaque patch interiors preserve exact unlit pixels across a ninety-degree camera change")
    measurements["billboard"] = {"cameras": perspective_cameras, "exact_patch_pixels": len(billboard_samples)}
    client.call("select", {"ids": []})
    check(report()["displayed"] and render("icon-unselected", top)[0] == ring, "Unselected icons remain visible with the same RGBA pixels when selection outlines are disabled")
    client.call("select", {"ids": [entity]})
    client.call("set_entity_properties", {"id": entity, "set": {"angles": "30 40 25"}})
    check(render("icon-authored-angles", top)[0] == ring, "Authored entity angles do not rotate the live camera-facing image")
    client.call("undo")
    client.call("transform_objects", {"ids": [entity], "translation": [16, 0, 0], "preview": True})
    check(render("icon-transform-preview", top)[0] != ring and detail(client, [entity]) == authored, "Image follows the shared transform draft while committed entity fields stay unchanged")
    client.call("cancel_transform_preview")
    configure(client, action="set", mins=[8, -8, -8], maxs=[24, 8, 8])
    check(not report()["displayed"] and report()["suppression_reason"] == "outside_working_region", "Working-region admission follows the small ordinary box even where the image crosses the region")
    filtered, _ = render("icon-filtered", top)
    check(all(color_kind(rgb(filtered, x, y)) is None for x, y in retained), "Excluded icon contributes no retained colored image pixels")
    configure(client, action="clear")
    client.call("set_object_visibility", {"action": "hide", "ids": [entity]})
    check(not report()["displayed"] and report()["suppression_reason"] == "authored_hidden", "Authored-hidden icon inspection explains suppression")
    hidden, _ = render("icon-hidden", top)
    check(all(color_kind(rgb(hidden, x, y)) is None for x, y in retained), "Authored hidden state removes actual icon pixels")
    client.call("undo")
    definitions(extra="studio(missing.mdl)")
    check(not report()["displayed"] and report()["suppression_reason"] == "model_or_sprite_helper", "Missing model helpers retain precedence over an otherwise admitted icon")
    model, _ = render("icon-model-precedence", top)
    check(all(color_kind(rgb(model, x, y)) is None for x, y in retained), "Model precedence suppresses the icon image rather than substituting it for a missing model")
    for material in ("audit/unsupported.vmt", "audit/bad_texture.vmt"):
        definitions(material)
        row = report()
        check(row["status"] in {"unsupported", "invalid", "unavailable"} and not row["displayed"] and row["error"], f"Unsupported shader or malformed VTF remains explicit: {material}")
        pixels, _ = render(Path(material).stem, top)
        check(all(color_kind(rgb(pixels, x, y)) is None for x, y in retained), f"Rejected asset contributes no healthy-looking icon pixels: {material}")
    definitions()
    fog_checks(client, render, check, report, definitions, patches, expected_patches, perspectives[0], measurements)
    client.call("select", {"ids": [entity]})
    check(report()["displayed"] and render("icon-restored", top)[0] == ring, "Restoring all controls returns exact original icon pixels")
    client.call("frame_selection", {"aspect": 1})
    check(client.call("get_camera") == camera_before and detail(client, [entity])[0]["bounds"] == bounds and client.call("get_object_hit_candidates", body_ray) == body_hits, "Image rendering never enlarges ordinary framing, bounds or pick candidates")
    check(detail(client, [entity]) == authored and source.read_bytes() == saved and client.call("get_history")["undo_count"] == history, "All reversible controls preserve saved source, authored entity fields and exact Undo count")
    assets = [{"file": str(path.relative_to(output)), "sha256": hashlib.sha256(path.read_bytes()).hexdigest()} for path in sorted((output / "game").rglob("*")) if path.is_file()]
    (output / "results.json").write_text(json.dumps({"passed": len(checks), "checks": checks, "icon_sprite": ready, "missing": missing, "measurements": measurements, "images": images, "assets": assets, "camera_cache_evidence": "Consecutive render_view camera overrides use one unchanged document/selection and the persistent scene cache; scene_serial is not exposed over MCP.", "desktop_input": False, "native_exercised": False}, indent=2) + "\n")
    print(json.dumps({"passed": len(checks), "output": str(output)}))


def fog_checks(client, render, check, report, definitions, patches, expected_patches, camera, measurements):
    wall = client.call("create_block", {"mins": [32, -128, -128], "maxs": [40, 128, 128], "material": "audit/opaque"})["id"]
    controller = client.call("create_entity", {"classname": "env_fog_controller", "origin": [0, -1000, 0], "properties": {"fogcolor": "255 0 255", "fogstart": "0", "fogend": "64", "fogmaxdensity": "1", "fogenable": "1", "fogblend": "0", "fogradial": "0", "farz": "-1", "HDRColorScale": "1", "ZoomFogScale": "1"}})["id"]
    definitions("audit/opaque.vmt")
    check(report()["displayed"], "Eligible icon stays visible while a fog controller is selected")
    off, _ = render("icon-fog-off", camera, fog_preview=False)
    on, metadata = render("icon-fog-on", camera, fog_preview=True, fog_controller=controller)
    check(metadata["fog_preview"]["applied"] and patches(off) == patches(on) == expected_patches, "Applied full fog leaves the unlit icon's four opaque color patches unchanged")
    center = SIZE // 2
    opaque_samples = [(x, y) for y in range(center - 24, center + 25) for x in range(center - 24, center + 25)]
    background = [(x, y) for y in range(SIZE) for x in range(SIZE) if abs(x - center) > 90 or abs(y - center) > 90]
    changed = sum(different(off, on, x, y) for x, y in background)
    check(all(not different(off, on, x, y) for x, y in opaque_samples) and changed > 1000, "Every central opaque image pixel is exact while an independent brush visibly receives fog")
    measurements["fog"] = {"exact_opaque_pixels": len(opaque_samples), "changed_background_pixels": changed, "wall": wall, "controller": controller}
    client.call("undo")
    client.call("undo")
    definitions()


def main():
    parser = argparse.ArgumentParser(description="Validate mounted literal FGD icon helpers through private MCP and actual offscreen alpha, camera and fog controls.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    gameinfo, materials = fixture(output)
    with patch.dict(os.environ, {
        "HAMMERR_RECENT_MAPS_DIR": str(output / "recent"),
        "HAMMERR_BUILD_RECIPES": str(output / "recipes.json"),
    }):
        client = Client(None, args.binary.resolve(), output, gameinfo)
    try:
        validate(client, output, materials)
    finally:
        client.close()


if __name__ == "__main__":
    main()
