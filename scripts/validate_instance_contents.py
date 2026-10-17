import argparse
import hashlib
import json
import time
from pathlib import Path
from unittest.mock import patch

from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_normal_map_preview import fixture
from validate_sky_preview import rgba_png

TOP = {"projection": "top", "center": [0, 0, 0], "ortho_height": 96}
SIZE = 513
PROBE = (171, 256)


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def refresh(client, background=False):
    state = client.call("set_instance_contents", {"action": "refresh", "background": background})
    if background:
        deadline = time.monotonic() + 90
        while time.monotonic() < deadline:
            state = client.call("inspect_instance_contents")
            if not state["loading"]:
                break
            time.sleep(0.02)
        else:
            raise TimeoutError("Instance contents refresh")
    if state["state"] != "ready":
        raise AssertionError(state)
    return state


def validate(client, output, result):
    checks = result["checks"]

    def check(condition, label):
        if not condition:
            raise AssertionError(label)
        checks.append(label)

    def block(low, high, material="unavailable/reference-only"):
        return client.call("create_block", {"mins": low, "maxs": high, "material": material})["id"]

    def render(name, camera=TOP, width=SIZE, height=SIZE, **options):
        png, metadata = client.render_result(width=width, height=height, camera=camera,
            selection_outlines=False, fog_preview=False, **options)
        path = output / f"{name}.png"
        path.write_bytes(png)
        (output / f"{name}.metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
        result["images"].append({"file": path.name, "sha256": hashlib.sha256(png).hexdigest()})
        return rgba_png(path), metadata["instance_contents"]

    def probe(pixels):
        offset = (PROBE[1] * SIZE + PROBE[0]) * 4
        return pixels[offset:offset + 4]

    def pick(enabled):
        return client.call("pick_at_pixel", {"x": PROBE[0], "y": PROBE[1], "width": SIZE,
            "height": SIZE, "camera": TOP, "instance_contents": enabled})

    child = output / "parts/child.vmf"
    child.parent.mkdir()
    block([-24, -24, 0], [24, 24, 40])
    child_detail = block([40, -12, 0], [64, 12, 24])
    client.call("tie_to_entity", {"ids": [child_detail], "classname": "func_detail"})
    client.call("save_map", {"path": str(child)})
    child_bytes = child.read_bytes()
    client.call("new_map", {"discard_changes": True})
    wall = block([-4, -4, 48], [4, 4, 56], "audit/surface")
    owners = []
    for origin, angles in [([0, 0, 0], "0 0 0"), ([120, 16, 0], "17 43 11")]:
        owners.append(client.call("create_entity", {"classname": "func_instance", "origin": origin,
            "properties": {"file": "parts/child.vmf", "angles": angles}})["id"])
    client.call("select", {"ids": []})
    client.call("set_spatial_settings", {"grid_visible": False})
    client.call("set_view", {"instance_contents": False, "surface": "textured", "lighting_preview": False,
        "normal_maps": False, "entity_images": False, "entity_model_guides": False,
        "sky_preview": False, "rope_preview": False, "selection_outlines": False})
    parent = output / "parent.vmf"
    client.call("save_map", {"path": str(parent)})
    saved = parent.read_bytes()
    for extension, payload in [(".bsp", b"previous BSP"), (".lin", b"previous pointfile")]:
        parent.with_suffix(extension).write_bytes(payload)
    source = detail(client, [wall, *owners])
    state = client.call("get_map_info")
    camera = client.call("get_camera")
    initial = client.call("inspect_instance_contents")
    check(initial["status"] == "disabled" and initial["state"] == "not_loaded"
        and not state["rendering"]["initialized"], "Default-off inspection has no child snapshot or GPU")
    client.call("set_view", {"instance_contents": True})
    unloaded = client.call("inspect_instance_contents")
    check(unloaded["state"] == "not_loaded" and not unloaded["displayed"]
        and not client.call("get_map_info")["rendering"]["initialized"],
        "The checkbox never resolves child files or initializes graphics")
    client.call("set_view", {"instance_contents": False})
    ready = refresh(client, background=True)
    check(ready["available"] and not ready["requested"] and not ready["displayed"]
        and ready["stats"]["instances"] == 2 and ready["stats"]["files"] == 1
        and ready["stats"]["world_brushes"] == ready["stats"]["detail_brushes"] == 2
        and not client.call("get_map_info")["rendering"]["initialized"],
        "Background CPU Refresh resolves repeated world/detail contents atomically without graphics")
    check(ready["geometry"]["gpu_bytes"] <= ready["limits"]["gpu_bytes"]
        and ready["geometry"]["cpu_bytes"] <= ready["limits"]["cpu_bytes"],
        "Published geometry reports bounded retained CPU and GPU payloads")
    result["ready"] = ready
    off, _ = render("instance-contents-off", instance_contents=False)
    client.call("set_instance_contents", {"action": "clear", "token": str(ready["token"])})
    empty, _ = render("instance-contents-unloaded", instance_contents=False)
    check(empty == off, "Default-off pixels remain exact with or without a resolved snapshot")
    ready = refresh(client)
    on, report = render("instance-contents-on", instance_contents=True)
    check(on != off and probe(on) != probe(off) and report["displayed"],
        "Actual flat world/detail reference surfaces render at transformed positions")
    reference_hit = pick(True)
    check(reference_hit["object"] == owners[0] and reference_hit["solid"] is None
        and reference_hit["side"] is None and pick(False) == {"hit": None},
        "Temporary render/pixel-pick overrides agree and expose only the existing root owner")
    center_hit = client.call("pick_at_pixel", {"x": 256, "y": 256, "width": SIZE, "height": SIZE,
        "camera": TOP, "instance_contents": True})
    check(center_hit["object"] == wall and center_hit["solid"] is not None,
        "Ordinary root geometry occludes reference surfaces by true depth")
    wire, wire_report = render("instance-contents-wireframe", instance_contents=True, surface="wireframe")
    wire_off, _ = render("instance-contents-wireframe-off", instance_contents=False, surface="wireframe")
    check(wire_report["displayed"] and probe(wire) == probe(wire_off) and wire != wire_off,
        "Wireframe shows actual reference edges and leaves face interiors unfilled")
    for mode in ("flat", "lightmap_density", "smoothing_groups"):
        flat, flat_report = render(f"instance-contents-{mode}", instance_contents=True, surface=mode)
        check(flat_report["displayed"] and probe(flat) == probe(on),
            f"{mode} keeps the distinct flat reference display without child material evaluation")
    client.call("set_selection_mode", {"mode": "faces"})
    suppressed, suppressed_report = render("instance-contents-face-suppressed", instance_contents=True)
    check(suppressed == off and suppressed_report["status"] == "context_suppressed"
        and not suppressed_report["displayed"],
        "Entering Faces mode suppresses cached reference geometry before scene-key reuse")
    client.call("set_selection_mode", {"mode": "objects"})
    restored, _ = render("instance-contents-object-restored", instance_contents=True)
    check(restored == on, "Returning to ordinary Objects restores the same current snapshot without loading")
    framed = client.call("set_instance_contents", {"action": "frame", "token": str(ready["token"]),
        "ids": [owners[0]], "aspect": 1.5})
    check(framed is not None and client.call("get_camera") != camera,
        "Explicit Frame instance contents works for a current snapshot while the checkbox is off")
    client.call("set_camera", camera)
    unchanged = client.call("get_map_info")
    check(unchanged["view"] == state["view"] and unchanged["undo"] == state["undo"]
        and unchanged["redo"] == state["redo"] and client.call("get_camera") == camera
        and detail(client, [wall, *owners]) == source and parent.read_bytes() == saved,
        "Refresh, inspection and temporary rendering preserve parent source, history, selection, camera and view")
    before_showcase = {"projection": "perspective", "position": [-160, -240, 185],
        "look_at": [52, 0, 20], "fov": 50}
    render("instance-contents-perspective", camera=before_showcase, width=1001, height=701,
        instance_contents=True)
    child.write_bytes(child_bytes + b"\n// private dependency mutation\n")
    stale_pixels, stale = render("instance-contents-stale", instance_contents=True)
    check(stale_pixels == off and stale["state"] == "stale" and not stale["displayed"]
        and pick(True) == {"hit": None} and report["displayed"],
        "Child mutation removes stale cached pixels and hits before reuse while old rendered metadata stays retained")
    check(parent.with_suffix(".bsp").read_bytes() == b"previous BSP"
        and parent.with_suffix(".lin").read_bytes() == b"previous pointfile",
        "Reference inspection leaves existing BSP and pointfile outputs untouched")
    repaired = refresh(client)
    recovered, _ = render("instance-contents-refreshed", instance_contents=True)
    check(recovered == on and repaired["state"] == "ready", "Explicit Refresh restores the checked child geometry")
    child.unlink()
    failed = client.call("set_instance_contents", {"action": "refresh"})
    check(failed["state"] == "failed" and not failed["available"],
        "A missing child fails the entire refresh and evicts all repeated occurrences")
    failed_pixels, failed_report = render("instance-contents-missing", instance_contents=True)
    check(failed_pixels == off and not failed_report["displayed"],
        "Failed refresh never presents stale previous geometry or a successful subset")
    result["parent_sha256"] = digest(parent)
    result["child_source_sha256"] = hashlib.sha256(child_bytes).hexdigest()


def main():
    parser = argparse.ArgumentParser(description="Validate read-only instance contents using a private stdio MCP Session and offscreen captures.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve()
    if not binary.is_file():
        parser.error(f"Binary does not exist: {binary}")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    gameinfo, materials = fixture(output)
    (materials / "surface.vmt").write_text("LightmappedGeneric { $basetexture audit/base }\n")
    result = {"status": "running", "binary": str(binary), "binary_sha256": digest(binary),
        "transport": "private_stdio", "desktop_input": False, "native_exercised": False,
        "checks": [], "images": []}
    client = None
    try:
        with patch.dict("os.environ", {"HAMMERR_RECENT_MAPS_DIR": str(output / "recent"),
            "HAMMERR_BUILD_RECIPES": str(output / "recipes.json"), "HAMMERR_PREFABS_DIR": str(output / "prefabs")}):
            client = Client(None, binary, output, gameinfo=gameinfo)
        validate(client, output, result)
        result["status"] = "passed"
    except Exception as error:
        result["status"] = "failed"
        result["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        if client is not None:
            client.close()
        (output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps({"status": result["status"], "passed": len(result["checks"]), "output": str(output)}))


if __name__ == "__main__":
    main()
