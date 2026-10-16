import argparse
import hashlib
import json
import os
from pathlib import Path
from unittest.mock import patch

from validate_entity_placement import begin, pick
from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_fgd_icons import texture
from validate_prop_shadows import fixture
from validate_sky_preview import rgba_png

SIZE = 401
FLAGS = ("entity_images", "entity_model_guides")
CAMERAS = {
    "top": {"projection": "top", "center": [0, 0, 16], "ortho_height": 256},
    "perspective": {"projection": "perspective", "position": [160, -220, 200], "look_at": [0, 0, 16]},
}


def definitions(images=True, models=True):
    icon = 'iconsprite("audit/deferred")' if images else ""
    model = 'studio("models/props/deferred.mdl")' if models else ""
    return f'''@PointClass size(8 8 8) color(100 100 100) {icon} = icon_sample []
@PointClass size(8 8 8) color(100 100 100) {model} = model_sample [ angles(angle) : "Angles" : "0 0 0" ]
@PointClass = choice_target [ model(studio) : "World Model" : "" ]
@PointClass size(8 8 8) = info_target []
@PointClass = prop_dynamic [ model(studio) : "World Model" : "" ]
'''


def repair(game):
    material = game / "materials/audit/deferred.vmt"
    material.parent.mkdir(parents=True, exist_ok=True)
    material.write_text('Sprite { $basetexture audit/ring $spriteorientation vp_parallel $spriteorigin "[0.5 0.5]" $no_fullbright 1 }\n')
    (material.parent / "ring.vtf").write_bytes(texture(True))
    for extension in ("mdl", "vvd", "dx90.vtx"):
        (game / f"models/props/deferred.{extension}").write_bytes((game / f"models/props/test.{extension}").read_bytes())


def validate(client, output, game):
    checks, images, measurements = [], [], {}

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def load(images=True, models=True):
        fgd.write_text(definitions(images, models))
        client.call("load_entity_definitions", {"path": str(fgd)})

    def reports():
        return (client.call("get_entity_inspector", {"id": icon})["helpers"]["icon_sprite"],
                client.call("get_entity_inspector", {"id": model})["helpers"]["model_guide"])

    def state():
        info = client.call("get_map_info")
        return {"objects": detail(client, ids), "history": client.call("get_history"),
                "camera": client.call("get_camera"), "selection": client.call("get_selection"),
                "unsaved_changes": info["unsaved_changes"], "view": info["view"]}

    def source_state():
        return {key: value for key, value in state().items() if key != "view"}

    def render(name, camera, **options):
        png, metadata = client.render_result(width=SIZE, height=SIZE, camera=camera, selection_outlines=False, **options)
        path = output / f"{name}.png"
        path.write_bytes(png)
        images.append({"file": path.name, "sha256": hashlib.sha256(png).hexdigest(), "camera": metadata["camera"], "view": metadata["view"]})
        return rgba_png(path), metadata

    client.call("new_map", {"discard_changes": True})
    initial = client.call("get_map_info")["view"]
    check(all(initial[flag] is True for flag in FLAGS), "Both resource visibility flags default on")
    tools = {tool["name"]: tool for tool in client.rpc("tools/list", {})["tools"]}
    check(all(flag in tools[tool]["inputSchema"]["properties"] and flag not in tools[tool]["inputSchema"].get("required", []) for tool in ("set_view", "render_view") for flag in FLAGS), "Both existing MCP tools expose optional resource flags")
    client.call("set_view", dict.fromkeys(FLAGS, False))
    client.call("set_spatial_settings", {"grid_visible": False})
    client.call("set_transform_tool", {"mode": "select"})
    fgd = output / "resources.fgd"
    load()
    floor = client.call("create_block", {"mins": [-112, -112, -24], "maxs": [112, 112, -16], "material": "dev/dev_measuregeneric01"})["id"]
    icon = client.call("create_entity", {"classname": "icon_sample", "origin": [-48, 0, 16]})["id"]
    model = client.call("create_entity", {"classname": "model_sample", "origin": [48, 0, 16]})["id"]
    ordinary = client.call("create_entity", {"classname": "prop_dynamic", "origin": [0, 60, 16], "properties": {"model": "models/props/test.mdl"}})["id"]
    light = client.call("create_entity", {"classname": "light", "origin": [-60, 60, 16]})["id"]
    end = client.call("create_entity", {"classname": "keyframe_rope", "origin": [48, -60, 16], "properties": {"targetname": "rope_end"}})["id"]
    rope = client.call("create_entity", {"classname": "move_rope", "origin": [-48, -60, 16], "properties": {"targetname": "rope_start", "NextKey": "rope_end", "Slack": "16", "Width": "2", "Subdiv": "4"}})["id"]
    choice = client.call("create_entity", {"classname": "choice_target", "origin": [2000, 2000, 2000]})["id"]
    ids = [floor, icon, model, ordinary, light, end, rope, choice]
    client.call("select", {"ids": []})
    client.call("set_view", {"rope_preview": True})
    disabled_icon, disabled_model = reports()
    check(disabled_icon["status"] == "suppressed" and disabled_icon["suppression_reason"] == "view_disabled" and not disabled_icon["displayed"] and disabled_icon["source"] is None and disabled_icon["texture_source"] is None and disabled_icon["width"] == disabled_icon["height"] == 0, "Disabled icon truthfully reports deferred admission without source or dimensions")
    check(disabled_model["state"] == "not_loaded" and disabled_model["declaration_supported"] and disabled_model["suppression_reason"] == "view_disabled" and not disabled_model["displayed"] and all(disabled_model[key] is None for key in ("visual_bounds", "pose", "skin", "body", "error")), "Disabled model guide reports a valid declaration with deferred geometry")
    before = state()
    absent = render("deferred-before-repair", CAMERAS["top"])[0]
    check(state() == before, "Disabled admission and actual rendering preserve full source history, camera and shared view")
    repair(game)
    client.call("set_view", dict.fromkeys(FLAGS, True))
    ready = reports()
    check(ready[0]["status"] == ready[1]["state"] == "ready" and all(row["displayed"] for row in ready), "Repair while disabled becomes ready on first enable without an asset reload")
    visible = render("both-resources", CAMERAS["top"])[0]
    check(sum(visible[i:i + 3] != absent[i:i + 3] for i in range(0, len(visible), 4)) > 1000, "Actual icon and real model pixels appear after deferred admission")
    check(client.call("inspect_entity_model", {"id": ordinary})["displayed"], "Ordinary authored model remains independently displayed")
    rope_report = client.call("get_rope_preview")
    check(rope_report["enabled"] and len(rope_report["links"]) == 1, "Independent authored rope preview supplies a real control span")
    source = output / "resources.vmf"
    client.call("select", {"ids": [icon, model]})
    client.call("frame_selection", {"aspect": 1})
    framed = client.call("get_camera")
    client.call("select", {"ids": []})
    client.call("save_map", {"path": str(source)})
    saved = source.read_bytes()
    baseline = state()
    bounds = detail(client, [icon, model])
    body_ray = {"origin": [-48, -100, 16], "direction": [0, 1, 0]}
    hits = client.call("get_object_hit_candidates", body_ray)
    controls = {}
    for image_on, model_on in ((False, False), (False, True), (True, False), (True, True)):
        load(image_on, model_on)
        for name, camera in CAMERAS.items():
            controls[image_on, model_on, name] = render(f"declaration-control-{int(image_on)}-{int(model_on)}-{name}", camera)[0]
    load()
    check(state() == baseline, "Changing only loaded helper declarations preserves full authored state and camera")
    for name, camera in CAMERAS.items():
        for flag, variant in (("entity_images", (True, False)), ("entity_model_guides", (False, True))):
            shown, plain = controls[*variant, name], controls[False, False, name]
            changed = sum(shown[i:i + 3] != plain[i:i + 3] for i in range(0, len(shown), 4))
            measurements[f"{flag}-{name}"] = {"changed_pixels": changed}
            check(changed > 80, f"Independent {flag} declaration contributes actual pixels in {name}")
        omitted, _ = render(f"defaults-{name}", camera)
        explicit, metadata = render(f"explicit-on-{name}", camera, **dict.fromkeys(FLAGS, True))
        check(omitted == explicit == controls[True, True, name] and all(metadata["view"][flag] for flag in FLAGS), f"Omitted and explicit true produce exact default pixels in {name}")
        for cycle in range(2):
            for image_on, model_on in ((False, True), (True, False), (False, False), (True, True)):
                options = dict(zip(FLAGS, (image_on, model_on), strict=True))
                pixels, metadata = render(f"temporary-{name}-{cycle}-{int(image_on)}-{int(model_on)}", camera, **options)
                check(pixels == controls[image_on, model_on, name] and all(metadata["view"][key] == value for key, value in options.items()), f"Persistent renderer temporary {options} matches ordinary-box declaration control in {name} cycle {cycle}")
    check(state() == baseline and source.read_bytes() == saved, "Temporary flag alternation preserves shared view, full history, source bytes and camera")
    for image_on, model_on in ((False, True), (True, False), (False, False), (True, True)):
        unchanged = source_state()
        client.call("set_view", dict(zip(FLAGS, (image_on, model_on), strict=True)))
        rows = reports()
        check(source_state() == unchanged and all(row["displayed"] == enabled for row, enabled in zip(rows, (image_on, model_on), strict=True)), "Shared switches independently affect only their resource helper while preserving source and history")
        check(detail(client, [icon, model]) == bounds and client.call("get_object_hit_candidates", body_ray) == hits, "Resource visibility preserves ordinary bounds and object hit candidates")
        client.call("select", {"ids": [icon, model]})
        client.call("frame_selection", {"aspect": 1})
        check(client.call("get_camera") == framed, "Resource visibility preserves exact framing from the ordinary FGD boxes")
        client.call("select", {"ids": []})
    check(state() == baseline, "Restoring shared flags restores exact persistent view and authored state")
    for draft_kind in ("model_choice", "entity_placement"):
        if draft_kind == "model_choice":
            token = client.call("begin_model_choice", {"id": choice})["token"]
            client.call("choose_model", {"token": token, "path": "models/props/test.mdl"})
            tool = "get_model_choice"
            cancel_tool = "cancel_model_choice"
        else:
            token = begin(client, snap=False)
            check(pick(client, token, [80, 80, 128], [0, 0, -1])["draft"]["plan"]["target"] == floor, "Placement draft uses the real floor surface")
            reviewed = client.call("review_entity_placement", {"token": token})
            token = reviewed["draft"]["token"]
            tool = "get_entity_placement"
            cancel_tool = "cancel_entity_placement"
        query = {"token": token} if draft_kind == "model_choice" else None
        draft = client.call(tool, query)
        check(draft["valid"] if draft_kind == "model_choice" else draft["draft"]["valid"], f"Pending {draft_kind} starts with a valid guarded token")
        original = source_state()
        for enabled in (False, True, False, True):
            client.call("set_view", dict.fromkeys(FLAGS, enabled))
            render(f"pending-{draft_kind}-{int(enabled)}-{len(images)}", CAMERAS["top"], entity_images=not enabled, entity_model_guides=not enabled)
            check(client.call(tool, query) == draft and source_state() == original, f"Shared and temporary switches preserve the exact pending {draft_kind} token and review state")
        client.call("set_view", {"entity_images": False, "sky_exposure_ev": 9}, error=True)
        client.call("render_view", {"entity_model_guides": False, "sky_channel": "invalid"}, error=True)
        client.call("set_view", {"entity_images": "false"}, error=True)
        check(client.call(tool, query) == draft and source_state() == original and all(client.call("get_map_info")["view"][flag] for flag in FLAGS), f"Rejected mixed view requests atomically retain pending {draft_kind} and both accepted flags")
        client.call(cancel_tool, {"token": token})
    check(state() == baseline and source.read_bytes() == saved, "Cancelling unchanged pending tools restores exact source, view, camera and history")
    (game / "materials/audit/deferred.vmt").write_text("broken Sprite material")
    (game / "models/props/deferred.mdl").write_bytes(b"broken MDL")
    client.call("reload_material_assets")
    client.call("reload_model_assets")
    failures = reports()
    check(failures[0]["status"] == failures[1]["state"] == "unavailable" and all(row["error"] for row in failures), "Explicit reload caches actual broken winning icon and model assets as unavailable")
    repair(game)
    for enabled in (False, True, False, True):
        client.call("set_view", dict.fromkeys(FLAGS, enabled))
        if enabled:
            check(reports() == failures, "Off/on visibility does not reset previously cached asset failures")
    client.call("reload_material_assets")
    client.call("reload_model_assets")
    check(all(row["displayed"] for row in reports()) and render("reloaded-resources", CAMERAS["top"])[0] == controls[True, True, "top"], "Explicit asset reload restores repaired resources and exact original pixels")
    check(state() == baseline and source.read_bytes() == saved, "Failure, repair, toggles and reload preserve complete source history, camera and shared view")
    measurements["deferred_changed_pixels"] = sum(visible[i:i + 3] != absent[i:i + 3] for i in range(0, len(visible), 4))
    (output / "results.json").write_text(json.dumps({"passed": len(checks), "checks": checks, "disabled": [disabled_icon, disabled_model], "ready": ready, "cached_failures": failures, "measurements": measurements, "images": images, "source_sha256": hashlib.sha256(saved).hexdigest(), "observation_limit": "MCP exposes neither SceneSerial nor GPU cache counters. Exact pixels, deferred repair, retained failures and live draft tokens are checked; source tests cover serial/cache keys and absence of filesystem admission.", "desktop_input": False, "native_exercised": False}, indent=2) + "\n")
    print(json.dumps({"passed": len(checks), "output": str(output)}))


def main():
    parser = argparse.ArgumentParser(description="Validate independent entity resource visibility through private MCP and actual paired PNG controls.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    game = output / "game"
    fixture(game)
    with patch.dict(os.environ, {"HAMMERR_RECENT_MAPS_DIR": str(output / "recent"), "HAMMERR_BUILD_RECIPES": str(output / "recipes.json")}):
        client = Client(None, args.binary.resolve(), output, game / "gameinfo.txt")
    try:
        validate(client, output, game)
    finally:
        client.close()


if __name__ == "__main__":
    main()
