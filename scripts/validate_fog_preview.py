import argparse
import json
from pathlib import Path
from unittest.mock import patch

from validate_face_lighting import DEFAULT_BINARY, Client
from validate_sky_preview import rgba_png, set_sky


def linear(channel):
    value = channel / 255
    return value / 12.92 if value <= 0.04045 else ((value + 0.055) / 1.055) ** 2.4


def encoded(value):
    value = 12.92 * value if value <= 0.0031308 else 1.055 * value ** (1 / 2.4) - 0.055
    return round(value * 255)


def validate(client, output):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def controller(name, **changes):
        properties = {
            "targetname": name, "fogcolor": "32 160 240", "fogstart": "200",
            "fogend": "600", "fogmaxdensity": "1", "fogenable": "1",
            "fogblend": "0", "fogradial": "0", "farz": "-1",
            "HDRColorScale": "1", "ZoomFogScale": "1", **changes,
        }
        return client.call("create_entity", {
            "classname": "env_fog_controller", "origin": [0, -4000, 0], "properties": properties,
        })["id"]

    def apply(enabled, controller_id=None, snapshot=None, error=False):
        return client.call("set_fog_preview", {
            "enabled": enabled, "controller": controller_id,
            "snapshot": snapshot or client.call("get_fog_preview")["snapshot"],
        }, error=error)

    def render(name, **changes):
        data, metadata = client.render_result(width=401, height=401, **changes)
        path = output / f"{name}.png"
        path.write_bytes(data)
        return rgba_png(path), metadata

    initial = client.call("get_fog_preview")
    check(not initial["enabled"] and initial["controller"] is None, "Preview defaults off with no selected controller")
    apply(True, error=True)
    client.call("set_spatial_settings", {"grid_visible": False})
    client.call("set_view", {"surface": "textured", "tool_textures": True})
    wall = client.call("create_block", {
        "mins": [400, -500, -500], "maxs": [416, 500, 500], "material": "dev/dev_measuregeneric01",
    })["id"]
    midpoint = controller("midpoint")
    full = controller("full", fogstart="0", fogend="200")
    capped = controller("capped", fogstart="0", fogend="200", fogmaxdensity="0.5")
    near = controller("before_start", fogstart="500", fogend="700")
    disabled = controller("disabled", fogenable="0")
    unsupported = controller("unsupported_blend", fogblend="1")
    client.call("select", {"ids": []})
    client.call("set_camera", {"projection": "perspective", "position": [0, 0, 0], "look_at": [1, 0, 0]})
    client.call("save_map", {"path": str(output / "source.vmf")})
    source = (output / "source.vmf").read_bytes()
    before = client.call("get_map_info")
    baseline, _ = render("off")
    pick = client.call("pick_at_pixel", {"x": 200, "y": 200, "width": 401, "height": 401})
    check(pick["object"] == wall, "Known-depth wall remains pickable")
    pixels, metadata = render("full", fog_preview=True, fog_controller=full)
    center = (200 * 401 + 200) * 4
    check(all(abs(actual - expected) <= 1 for actual, expected in zip(pixels[center:center + 3], [32, 160, 240])), "Full fog reaches the authored RGB color")
    check(metadata["fog_preview"]["applied"] and metadata["fog_preview"]["controller"] == full, "Render metadata reports the temporary controller and applied state")
    half, _ = render("midpoint", fog_preview=True, fog_controller=midpoint)
    cap, _ = render("density-cap", fog_preview=True, fog_controller=capped)
    check(half == cap and half != baseline, "Midpoint range equals the maximum-density cap")
    for x in [100, 200, 300]:
        offset = (200 * 401 + x) * 4
        expected = [encoded(linear(base) * 0.75 + linear(fog) * 0.25) for base, fog in zip(baseline[offset:offset + 3], [32, 160, 240])]
        check(all(abs(actual - target) <= 2 for actual, target in zip(half[offset:offset + 3], expected)), f"Squared linear-range factor uses linear-space RGB at x={x}")
    check(render("before-start", fog_preview=True, fog_controller=near)[0] == baseline, "Surfaces before fogstart retain their original pixels")
    check(render("authored-disabled", fog_preview=True, fog_controller=disabled)[0] == baseline, "fogenable=0 suppresses fog")
    check(not client.call("get_fog_preview")["enabled"], "Render overrides preserve shared settings")
    snapshot = client.call("get_fog_preview")["snapshot"]
    apply(True, midpoint)
    check(render("shared")[0] == half, "Shared Session preview equals the temporary render override")
    apply(False, midpoint, snapshot=snapshot, error=True)
    apply(True, unsupported, error=True)
    check(client.call("get_fog_preview")["controller"] == midpoint, "Unsupported and stale actions retain the selected controller")
    check(client.call("pick_at_pixel", {"x": 200, "y": 200, "width": 401, "height": 401}) == pick, "Fog preserves picking")
    for mode in ["flat", "wireframe", "lightmap_density", "smoothing_groups"]:
        on, meta = render(f"{mode}-on", surface=mode)
        off, _ = render(f"{mode}-off", surface=mode, fog_preview=False)
        check(on == off and not meta["fog_preview"]["applied"], f"{mode} is unchanged and reports fog unapplied")
    for projection in ["top", "front", "side"]:
        camera = {"projection": projection, "center": [400, 0, 0], "ortho_height": 1200}
        on, meta = render(f"{projection}-on", camera=camera)
        off, _ = render(f"{projection}-off", camera=camera, fog_preview=False)
        check(on == off and not meta["fog_preview"]["applied"], f"{projection} is unchanged and reports fog unapplied")
    client.call("render_view", {"fog_preview": True, "fog_controller": unsupported}, error=True)
    after = client.call("get_map_info")
    check(before["undo"] == after["undo"] and before["selection"] == after["selection"] and not after["unsaved_changes"], "Preview controls leave map history, selection and dirty state unchanged")
    check((output / "source.vmf").read_bytes() == source, "Preview controls leave the source file unchanged")
    apply(False)
    check(render("disabled-again")[0] == baseline, "Disabling restores the default pixels")
    apply(True, midpoint)
    for invalidation in ["invalid", "deleted", "hidden"]:
        if invalidation == "invalid":
            client.call("set_entity_properties", {"id": midpoint, "set": {"fogend": "100"}})
        elif invalidation == "deleted":
            client.call("delete_objects", {"ids": [midpoint]})
        else:
            client.call("set_object_visibility", {"action": "hide", "ids": [midpoint]})
        fallback, metadata = render(f"shared-{invalidation}")
        report = client.call("get_fog_preview")
        check(fallback == baseline and metadata["fog_preview"]["issue"] and not metadata["fog_preview"]["applied"], f"A shared {invalidation} controller renders without fog and reports its issue")
        if invalidation != "deleted":
            check(any(row["id"] == midpoint and not row["ready"] and row["issue"] for row in report["controllers"]), f"The {invalidation} controller is unavailable in the chooser")
        client.call("render_view", {"fog_preview": True, "fog_controller": midpoint}, error=True)
        client.call("undo")
        check(render(f"restored-{invalidation}")[0] == half, f"Undo restores fog after controller {invalidation}")
    apply(False)
    set_sky(client, "sky_dust")
    sky_ready = client.call("get_map_info")["skybox"]["ready"]
    for material in ["tools/toolsskybox", "tools/toolsskybox2d"]:
        client.call("set_material", {"ids": [wall], "material": material})
        name = material.rsplit("/", 1)[-1]
        sky_on, _ = render(f"{name}-fog-on", sky_preview=True, fog_preview=True, fog_controller=full)
        sky_off, _ = render(f"{name}-fog-off", sky_preview=True, fog_preview=False)
        check(sky_on == sky_off, f"{name} retains {'sampled sky' if sky_ready else 'unavailable-sky tool'} pixels")
    client.call("delete_objects", {"ids": [wall]})
    empty, _ = render("empty", fog_preview=False)
    check(render("empty-fog", fog_preview=True, fog_controller=full)[0] == empty, "The viewport background is unchanged")
    client.call("create_entity", {"classname": "info_target", "origin": [400, 0, 0]})
    client.call("select", {"ids": []})
    helper, _ = render("helper", fog_preview=False)
    helper_fog, _ = render("helper-fog", fog_preview=True, fog_controller=full)
    check(helper != empty and helper == helper_fog, "Visible entity helpers retain their pixels")
    result = {"checks": len(checks), "sampled_sky_ready": sky_ready, "passed": checks}
    (output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"checks": len(checks), "output": str(output)}))


def main():
    parser = argparse.ArgumentParser(description="Validate static fog via a private MCP process and offscreen pixels without desktop input.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--gameinfo", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    with patch.dict("os.environ", {
        "HAMMERR_RECENT_MAPS_DIR": str(output / "recent"),
        "HAMMERR_BUILD_RECIPES": str(output / "recipes.json"),
    }):
        client = Client(None, args.binary.resolve(), output, args.gameinfo)
    try:
        validate(client, output)
    finally:
        client.close()


if __name__ == "__main__":
    main()
