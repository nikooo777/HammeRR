import argparse
import hashlib
import json
import math
import os
import struct
from pathlib import Path
from unittest.mock import patch

from validate_face_lighting import DEFAULT_BINARY, Client
from validate_sky_preview import rgba_png, set_sky

SIZE = 257
TEXTURE_SIZE = 64
FAMILY = "hammerr_hdr_preview"
SUFFIXES = ["rt", "bk", "lf", "ft", "up", "dn"]
DIRECTIONS = [[1, 0, 0], [0, 1, 0], [-1, 0, 0], [0, -1, 0], [0, 0, 1], [0, 0, -1]]
SAMPLES = [(64, 64), (192, 64), (64, 192), (192, 192)]
RADIANCE = [
    [(0, 0.25, 1), (0.25, 1, 4), (1, 4, 16), (4, 16, 0)],
    [(16, 1, 0.25), (4, 0.25, 0), (0.25, 16, 1), (0, 4, 16)],
    [(1, 0, 16), (16, 4, 0.25), (4, 1, 0), (0.25, 16, 4)],
    [(4, 16, 1), (0, 1, 4), (16, 0.25, 0), (1, 0, 0.25)],
    [(0.25, 4, 0), (1, 0.25, 16), (0, 16, 4), (16, 0, 1)],
    [(16, 16, 0.25), (0.25, 0, 4), (4, 1, 16), (1, 0.25, 0)],
]
LDR_COLORS = [(31, 83, 149), (179, 47, 97), (61, 191, 107), (223, 163, 53)]


def texture(colors, image_format, *, invalid_component=None):
    header = bytearray(80)
    header[:4] = b"VTF\0"
    struct.pack_into("<IIIHH", header, 4, 7, 2, 80, TEXTURE_SIZE, TEXTURE_SIZE)
    struct.pack_into("<I", header, 20, 0x0C)
    struct.pack_into("<H", header, 24, 1)
    struct.pack_into("<fI", header, 48, 1, image_format)
    header[56] = 1
    struct.pack_into("<i", header, 57, -1)
    struct.pack_into("<H", header, 63, 1)
    payload = bytearray()
    for y in range(TEXTURE_SIZE):
        for x in range(TEXTURE_SIZE):
            quadrant = int(x >= TEXTURE_SIZE // 2) + 2 * int(y >= TEXTURE_SIZE // 2)
            components = [*colors[quadrant], quadrant / 3]
            if image_format == 0:
                payload.extend(bytes([*colors[quadrant], 255]))
            elif image_format == 24:
                if invalid_component is not None and x == y == 0:
                    axis, value = invalid_component
                    components[axis] = value
                payload.extend(struct.pack("<4e", *components))
            else:
                assert image_format == 25
                payload.extend(struct.pack("<4H", *(round(value / 16 * 65535) for value in components)))
    return bytes(header + payload)


def create_fixture(output):
    game = output / "game"
    sky = game / "materials/skybox"
    tools = game / "materials/tools"
    sky.mkdir(parents=True)
    tools.mkdir(parents=True)
    (tools / "toolsskybox.vmt").write_text("LightmappedGeneric { %compilesky 1 }")
    (sky / "foreground.vmt").write_text("UnlitGeneric { $basetexture skybox/ldr }")
    (sky / "ldr.vtf").write_bytes(texture(LDR_COLORS, 0))
    for family, image_format in [(FAMILY, 24), (FAMILY + "_integer", 25)]:
        for suffix, colors in zip(SUFFIXES, RADIANCE, strict=True):
            (sky / f"{family}{suffix}.vmt").write_text(
                f"Sky {{ $basetexture skybox/ldr $hdrbasetexture skybox/{family}{suffix} }}"
            )
            (sky / f"{family}{suffix}.vtf").write_bytes(texture(colors, image_format))
    gameinfo = game / "gameinfo.txt"
    gameinfo.write_text("GameInfo { FileSystem { SearchPaths { game |gameinfo_path|. } } }")
    return gameinfo, sky


def encoded(radiance, exposure):
    value = radiance * 2**exposure
    value = value / (1 + value)
    value = 12.92 * value if value <= 0.0031308 else 1.055 * value ** (1 / 2.4) - 0.055
    return round(value * 255)


def expected_rgb(colors, exposure, image_format):
    if image_format == 25:
        colors = [round(value / 16 * 65535) / 65535 * 16 for value in colors]
    return [encoded(value, exposure) for value in colors]


def camera(index):
    if index >= 4:
        return {"projection": "perspective", "position": [0, 0, 0], "yaw": 0, "pitch": 89 if index == 4 else -89, "fov": 60}
    return {"projection": "perspective", "position": [0, 0, 0], "look_at": DIRECTIONS[index], "fov": 60}


def pixel(pixels, x, y):
    offset = (y * SIZE + x) * 4
    return list(pixels[offset:offset + 4])


def require_rejection(client, method, arguments):
    try:
        result = client.call(method, arguments, error=True)
    except RuntimeError as error:
        return str(error)
    return json.dumps(result)


def validate(client, output, sky):
    checks = []
    measurements = {}

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def render(name, index=0, **changes):
        camera_spec = changes.pop("camera", camera(index))
        data, metadata = client.render_result(width=SIZE, height=SIZE, camera=camera_spec, **changes)
        path = output / f"{name}.png"
        path.write_bytes(data)
        (output / f"{name}.json").write_text(json.dumps(metadata, indent=2) + "\n")
        pixels = rgba_png(path)
        assert metadata["width"] == metadata["height"] == SIZE and len(pixels) == SIZE * SIZE * 4, metadata
        return pixels, metadata

    def reference(pixels, index, exposure, image_format, label):
        actual = [pixel(pixels, x, y) for x, y in SAMPLES]
        expected = [expected_rgb(colors, exposure, image_format) for colors in RADIANCE[index]]
        difference = max(abs(value - target) for color, targets in zip(actual, expected, strict=True) for value, target in zip(color[:3], targets, strict=True))
        check(difference <= 1, f"{label}: linear radiance uses Reinhard then one sRGB encoding (max error {difference})")
        check(all(color[3] == 255 for color in actual), f"{label}: authored texture alpha does not alter opaque sky")
        measurements[label] = {"actual_rgba": actual, "expected_rgb": expected, "maximum_error": difference, "exposure_ev": exposure, "format": image_format}

    def document_state():
        info = client.call("get_map_info")
        return {
            "history": client.call("get_history"),
            "camera": client.call("get_camera"),
            **{key: info[key] for key in ["selection", "unsaved_changes", "view"]},
        }

    initial = client.call("get_map_info")
    check(not initial["rendering"]["initialized"], "Headless startup leaves graphics uninitialized")
    check(initial["view"]["sky_channel"] == "ldr" and initial["view"]["sky_exposure_ev"] == 0, "Default view retains LDR and zero exposure")
    schemas = {tool["name"]: tool["inputSchema"] for tool in client.rpc("tools/list", {})["tools"]}
    check(all({"sky_channel", "sky_exposure_ev"} <= schemas[name]["properties"].keys() for name in ["set_view", "render_view"]), "Shared and temporary preview controls are discoverable through MCP")
    set_sky(client, FAMILY)
    client.call("set_spatial_settings", {"grid_visible": False})
    client.call("set_view", {"surface": "textured", "tool_textures": True, "sky_preview": True, "selection_outlines": False})
    for axis in range(3):
        for sign in [-1, 1]:
            low, high = [-272] * 3, [272] * 3
            low[axis], high[axis] = (-272, -256) if sign < 0 else (256, 272)
            client.call("create_block", {"mins": low, "maxs": high, "material": "tools/toolsskybox"})
    client.call("select", {"ids": []})
    client.call("set_camera", camera(0))
    source = output / "source.vmf"
    client.call("save_map", {"path": str(source)})
    original_source = source.read_bytes()
    before = document_state()
    client.call("set_view", {"sky_channel": "hdr", "sky_exposure_ev": 2})
    hdr_report = client.call("get_map_info")["skybox"]
    check(hdr_report["ready"] and hdr_report["requested_channel"] == "hdr" and hdr_report["exposure_ev"] == 2, "Shared HDR settings resolve the exact authored family without rendering")
    for method in ["set_view", "render_view"]:
        for invalid in [{"sky_channel": "automatic"}, {"sky_exposure_ev": -8.01}, {"sky_exposure_ev": 8.01}, {"sky_exposure_ev": "NaN"}]:
            state = document_state()
            message = require_rejection(client, method, {"surface": "flat", **invalid})
            check(bool(message) and document_state() == state, f"{method} rejects {invalid} atomically")
    check(not client.call("get_map_info")["rendering"]["initialized"], "Settings, reports and invalid requests remain CPU-only before the first render")
    client.call("set_view", {"sky_channel": "ldr", "sky_exposure_ev": 0})
    check(document_state() == before, "Restoring preview defaults preserves document, camera and history")
    ldr = []
    for index, suffix in enumerate(SUFFIXES):
        pixels, meta = render(f"ldr-before-{suffix}", index)
        ldr.append(pixels)
        check(all(all(abs(a - b) <= 1 for a, b in zip(pixel(pixels, x, y)[:3], target, strict=True)) for (x, y), target in zip(SAMPLES, LDR_COLORS, strict=True)), f"LDR {suffix} retains authored byte colors and face orientation")
        check(meta["skybox"]["requested_channel"] == "ldr" and meta["skybox"]["tonemap"] == "none", f"LDR {suffix} metadata reports the original color path")
    stable = document_state()
    half = []
    for index, suffix in enumerate(SUFFIXES):
        pixels, meta = render(f"half-{suffix}", index, sky_channel="hdr")
        half.append(pixels)
        reference(pixels, index, 0, 24, suffix)
        report = meta["skybox"]
        check(report["ready"] and report["eligible"] and report["requested_channel"] == "hdr" and report["tonemap"] == "reinhard_per_channel" and bool(report["color_policy"]), f"HDR {suffix} reports its actual temporary view and explicit color policy")
        if index == 0:
            check(report["name"] == FAMILY and len(report["faces"]) == 6 and all(face["material"] == f"skybox/{FAMILY}{suffix}" and face["texture"] == f"materials/skybox/{FAMILY}{suffix}.vtf" and face["source"] and face["texture_source"] and face["format"] == "Rgba16161616F" and face["size"] == [TEXTURE_SIZE, TEXTURE_SIZE] for suffix, face in zip(SUFFIXES, report["faces"], strict=True)), "HDR metadata retains the exact authored six materials, textures, formats, sizes and provenance")
    exposure_images = {}
    for exposure in [-8, -2, 2, 8]:
        pixels, _ = render(f"exposure-{exposure}", sky_channel="hdr", sky_exposure_ev=exposure)
        exposure_images[exposure] = pixels
        reference(pixels, 0, exposure, 24, f"EV {exposure}")
    chain = [exposure_images[-8], exposure_images[-2], half[0], exposure_images[2], exposure_images[8]]
    check(all(all(first <= second for first, second in zip(pixel(a, x, y)[:3], pixel(b, x, y)[:3], strict=True)) for a, b in zip(chain, chain[1:]) for x, y in SAMPLES), "Increasing exposure is monotonic for every reference channel including exact black")
    check(document_state() == stable and source.read_bytes() == original_source, "Temporary HDR and exposure renders preserve shared view, camera, selection, source and history")
    client.call("set_view", {"sky_channel": "hdr", "sky_exposure_ev": 2})
    check(render("shared-hdr")[0] == exposure_images[2], "Shared Session HDR settings match temporary render overrides exactly")
    client.call("set_view", {"sky_channel": "ldr", "sky_exposure_ev": 0})
    set_sky(client, FAMILY + "_integer")
    for index, suffix in enumerate(SUFFIXES):
        pixels, _ = render(f"integer-{suffix}", index, sky_channel="hdr")
        reference(pixels, index, 0, 25, "integer " + suffix)
        check(max(abs(a - b) for a, b in zip(pixels, half[index], strict=True)) <= 1, f"UNORM16 {suffix} scales by sixteen and agrees with float radiance")
    set_sky(client, FAMILY)
    disabled, _ = render("sky-disabled", sky_preview=False)
    for mode in ["flat", "wireframe", "lightmap_density", "smoothing_groups"]:
        enabled, meta = render(f"{mode}-hdr", surface=mode, sky_channel="hdr")
        off, _ = render(f"{mode}-off", surface=mode, sky_preview=False)
        check(enabled == off and not meta["skybox"]["eligible"] and bool(meta["skybox"]["suppression_reason"]), f"{mode} keeps its existing pixels and reports suppressed sky")
    for projection in ["top", "front", "side"]:
        planar = {"projection": projection, "center": [0, 0, 0], "ortho_height": 700}
        enabled, meta = render(f"{projection}-hdr", camera=planar, sky_channel="hdr")
        off, _ = render(f"{projection}-off", camera=planar, sky_preview=False)
        check(enabled == off and not meta["skybox"]["eligible"], f"{projection} preserves the planar view")
    hidden, meta = render("tools-hidden-hdr", tool_textures=False, sky_channel="hdr")
    check(hidden == render("tools-hidden-off", tool_textures=False, sky_preview=False)[0] and not meta["skybox"]["eligible"], "Tool filtering still hides sky surfaces")
    client.call("create_block", {"mins": [300, -100, -100], "maxs": [340, 100, 100], "material": "skybox/foreground"})
    client.call("select", {"ids": []})
    check(render("behind-sky", sky_channel="hdr")[0] == half[0], "Sky depth occludes geometry behind its surface")
    client.call("undo")
    client.call("create_block", {"mins": [64, -16, -16], "maxs": [96, 16, 16], "material": "skybox/foreground"})
    client.call("select", {"ids": []})
    foreground, _ = render("foreground", sky_channel="hdr")
    brighter, _ = render("foreground-exposed", sky_channel="hdr", sky_exposure_ev=2)
    ldr_foreground, _ = render("foreground-ldr")
    center = [(x, y) for y in range(112, 145) for x in range(112, 145)]
    check(all(pixel(foreground, x, y) == pixel(brighter, x, y) == pixel(ldr_foreground, x, y) for x, y in center) and pixel(foreground, 128, 128) != pixel(half[0], 128, 128), "HDR channel and exposure leave visible foreground pixels exact")
    check(pixel(foreground, 64, 64) != pixel(brighter, 64, 64), "Visible sky still responds to exposure beside foreground geometry")
    client.call("undo")
    original_texture = (sky / f"{FAMILY}rt.vtf").read_bytes()
    original_vmt = (sky / f"{FAMILY}up.vmt").read_bytes()
    texture_path = sky / f"{FAMILY}rt.vtf"
    material_path = sky / f"{FAMILY}up.vmt"
    cache_state = document_state()
    texture_path.write_bytes(texture([(0.25, 0.25, 0.25)] * 4, 24))
    check(render("cached-before-reload", sky_channel="hdr")[0] == half[0], "Changed source assets remain cached until explicit reload")
    client.call("reload_material_assets")
    refreshed, _ = render("cache-refreshed", sky_channel="hdr")
    expected = [encoded(0.25, 0)] * 3
    check(all(abs(a - b) <= 1 for a, b in zip(pixel(refreshed, 64, 64)[:3], expected, strict=True)) and refreshed != half[0], "Material reload refreshes CPU and GPU HDR radiance")
    failures = []
    cases = [
        ("missing-texture", None, "missing"),
        ("negative", texture(RADIANCE[0], 24, invalid_component=(0, -0.25)), "negative"),
        ("nan", texture(RADIANCE[0], 24, invalid_component=(1, math.nan)), "nonfinite"),
        ("infinite", texture(RADIANCE[0], 24, invalid_component=(2, math.inf)), "nonfinite"),
        ("nonfinite-alpha", texture(RADIANCE[0], 24, invalid_component=(3, math.nan)), "nonfinite"),
        ("ldr-in-hdr-slot", texture(LDR_COLORS, 0), "hdr"),
    ]
    for name, data, issue in cases:
        if data is None:
            texture_path.unlink()
        else:
            texture_path.write_bytes(data)
        client.call("reload_material_assets")
        bad, meta = render("invalid-" + name, sky_channel="hdr")
        report = meta["skybox"]
        check(not report["ready"] and not report["eligible"] and issue in report["faces"][0]["issue"].lower(), f"{name} radiance has an explicit unavailable-HDR diagnostic")
        check(bad == disabled and bad != ldr[0], f"{name} retains the tool surface without LDR substitution")
        failures.append({"case": name, "report": report})
    texture_path.write_bytes(original_texture)
    check(not render("failed-cache-retained", sky_channel="hdr")[1]["skybox"]["ready"], "A cached HDR failure remains until explicit reload")
    client.call("reload_material_assets")
    check(render("failed-cache-restored", sky_channel="hdr")[0] == half[0], "Reload restores exact HDR pixels after a cached failure")
    for name, replacement in [
        ("missing-face", None),
        ("compressed", "Sky { $basetexture skybox/ldr $hdrcompressedtexture skybox/ldr }"),
        ("patch", f"Patch {{ include materials/skybox/{FAMILY}rt.vmt }}"),
        ("proxy", f'Sky {{ $hdrbasetexture skybox/{FAMILY}up Proxies {{ AnimatedTexture {{ }} }} }}'),
        ("unknown-parameter", f"Sky {{ $hdrbasetexture skybox/{FAMILY}up $unsupported 1 }}"),
    ]:
        if replacement is None:
            material_path.unlink()
        else:
            material_path.write_text(replacement)
        client.call("reload_material_assets")
        bad, meta = render("material-" + name, sky_channel="hdr")
        report = meta["skybox"]
        check(not report["ready"] and len(report["faces"]) == 6 and report["faces"][4]["issue"] and bad == disabled, f"{name} rejects the exact family without partial sky or another-family fallback")
        failures.append({"case": name, "report": report})
        material_path.write_bytes(original_vmt)
    client.call("reload_material_assets")
    check(document_state() == cache_state, "Cache invalidation and asset failures preserve document, settings, camera and full history")
    for index, suffix in enumerate(SUFFIXES):
        pixels, _ = render(f"ldr-after-{suffix}", index, sky_channel="ldr", sky_exposure_ev=8)
        check(pixels == ldr[index], f"LDR {suffix} is pixel-exact after HDR rendering, exposure, cache reload and failures")
    check(source.read_bytes() == original_source, "Preview and asset validation never rewrite the saved map")
    report = {
        "success": True, "checks": len(checks), "passed": checks, "measurements": measurements,
        "unavailable_cases": failures, "ldr_rgba_sha256": [hashlib.sha256(pixels).hexdigest() for pixels in ldr],
        "policy": "HammeRR per-channel Reinhard with linear exposure and one sRGB target encoding",
        "native_game_exercised": False, "engine_hardware_parity_claimed": False,
    }
    (output / "validation.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"success": True, "checks": len(checks), "output": str(output)}))


def main():
    parser = argparse.ArgumentParser(description="Validate explicit HDR sky preview through private MCP settings, CPU radiance references and actual offscreen PNGs.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=False)
    gameinfo, sky = create_fixture(args.output)
    with patch.dict(os.environ, {
        "HAMMERR_RECENT_MAPS_DIR": str(args.output / "recent"),
        "HAMMERR_BUILD_RECIPES": str(args.output / "recipes.json"),
        "HAMMERR_PREFABS_DIR": str(args.output / "prefabs"),
    }):
        client = Client(None, args.binary.resolve(), args.output, gameinfo)
    try:
        validate(client, args.output, sky)
    finally:
        client.close()


if __name__ == "__main__":
    main()
