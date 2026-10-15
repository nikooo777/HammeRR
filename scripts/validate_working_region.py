import argparse
import json
from pathlib import Path
from unittest.mock import patch

from validate_compile_jobs import wait_job
from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_visibility import ROOM


def configure(client, **changes):
    return client.call("set_working_region", {"snapshot": client.call("get_working_region")["snapshot"], **changes})


def validate(client, output, render):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    tools = {tool["name"] for tool in client.rpc("tools/list", {})["tools"]}
    check({"get_working_region", "set_working_region"} <= tools, "Shared working-region inspection and settings are discoverable")
    for mins, maxs in ROOM:
        client.call("create_block", {"mins": mins, "maxs": maxs, "material": "dev/dev_measuregeneric01"})
    client.call("create_entity", {"classname": "info_player_terrorist", "origin": [-64, -64, 8]})
    client.call("create_entity", {"classname": "info_player_counterterrorist", "origin": [64, -64, 8]})
    inside = client.call("create_block", {"mins": [-8.125, -8.375, 16.625], "maxs": [8.875, 8.625, 33.625], "material": "dev/dev_measuregeneric01"})["id"]
    outside = client.call("create_block", {"mins": [100.125, 100.375, 16.625], "maxs": [120.875, 120.625, 33.625], "material": "dev/dev_measuregeneric01"})["id"]
    helper = client.call("create_entity", {"classname": "info_target", "origin": [110, 110, 150]})["id"]
    client.call("select", {"ids": [outside]})
    client.call("set_compile_settings", {"lighting": "fullbright", "physics": "none"})
    source = output / "source.vmf"
    client.call("save_map", {"path": str(source)})
    saved = source.read_bytes()
    before = detail(client, [inside, outside, helper])
    info = client.call("get_map_info")
    baseline = output / "baseline.bsp"
    client.call("compile_map", {"path": str(baseline)})
    camera = {"projection": "top", "center": [0, 0, 80], "ortho_height": 700}
    if render:
        initial_image, _ = client.render_result(width=500, height=500, camera=camera)
        (output / "unfiltered.png").write_bytes(initial_image)
    ray = {"origin": [110, 110, 220], "direction": [0, 0, -1]}
    initial_hits = [row["object"] for row in client.call("get_object_hit_candidates", ray)["candidates"]]
    initial = client.call("get_working_region")
    check(not initial["enabled"] and initial["mins"] is None, "Working region starts disabled without saved bounds")
    filtered = configure(client, action="set", mins=[-16.25, -16.5, 8], maxs=[16.75, 16.5, 80])
    rows = {row["id"]: row for row in filtered["objects"]}
    check(filtered["enabled"] and rows[inside]["included"] and not rows[outside]["included"] and not rows[helper]["included"], "Whole brush and point-helper membership follows fractional world bounds")
    check("outside_working_region" in rows[outside]["reasons"] and filtered["selected_excluded"] == 1, "Inspection explains the excluded selection without clearing it")
    check(client.call("get_map_info")["working_region"]["enabled"], "General map inspection exposes the active display filter")
    ray = {"origin": [110, 110, 220], "direction": [0, 0, -1]}
    check(client.call("get_object_hit_candidates", ray)["total"] == 0, "Object cycling excludes hidden-by-region helpers and brushes")
    state = client.call("get_working_region")
    for invalid in [
        {"action": "set", "mins": [0, 0, 0], "maxs": [0, 1, 1]},
        {"action": "set", "mins": [0, 0, 0], "maxs": [1e30, 1, 1]},
        {"action": "clear", "enabled": False},
        {"action": "selection_bounds", "mins": [0, 0, 0]},
    ]:
        client.call("set_working_region", {"snapshot": state["snapshot"], **invalid}, error=True)
        check(client.call("get_working_region") == state, f"Invalid region preserves settings and inspection: {invalid['action']}")
    page = client.call("get_working_region", {"offset": 1, "limit": 2})
    check(page["total"] == state["total"] and len(page["objects"]) == 2 and page["next_offset"] == 3, "Inspection pages retain an exact total and stable order")
    filtered_bsp = output / "filtered.bsp"
    client.call("compile_map", {"path": str(filtered_bsp)})
    check(filtered_bsp.read_bytes() == baseline.read_bytes(), "Working-region display filtering leaves synchronous BSP bytes exact")
    background = output / "filtered-background.bsp"
    client.call("start_compile", {"path": str(background)})
    check(wait_job(client)["status"] == "completed" and background.read_bytes() == baseline.read_bytes(), "Background builds also ignore the display filter")
    check(source.read_bytes() == saved and detail(client, [inside, outside, helper]) == before and client.call("get_map_info")["undo"] == info["undo"] and client.call("get_map_info")["selection"] == info["selection"], "Filtering and compilation preserve saved source, authored objects, selection and history")
    empty = configure(client, action="set", mins=[2000, 2000, 2000], maxs=[2100, 2100, 2100])
    check(empty["excluded"] == empty["total"], "An outside region can exclude every display unit")
    if render:
        empty_image, metadata = client.render_result(width=500, height=500, camera=camera)
        (output / "excluded.png").write_bytes(empty_image)
        check(empty_image != initial_image and metadata["working_region"]["enabled"], "MCP renders honor the same region and expose its state")
    configure(client, action="clear")
    check([row["object"] for row in client.call("get_object_hit_candidates", ray)["candidates"]] == initial_hits, "Clear restores ordinary point and brush hits")
    if render:
        restored_image, _ = client.render_result(width=500, height=500, camera=camera)
        (output / "restored.png").write_bytes(restored_image)
        check(restored_image == initial_image, "Clear restores exact rendering through the same persistent renderer")
    client.call("select", {"ids": [inside]})
    captured = configure(client, action="selection_bounds")
    check(captured["mins"] == [-8.125, -8.375, 16.625] and captured["maxs"] == [8.875, 8.625, 33.625], "Selection bounds capture uses exact shared world bounds")
    preview = client.call("transform_objects", {"ids": [inside], "translation": [100, 0, 0], "preview": True})["state"]["preview"]
    configure(client, action="clear")
    check(client.call("get_transform_handles")["state"]["preview"] == preview, "A view-only region change preserves an owned transform draft")
    client.call("cancel_transform_preview")
    client.call("select", {"ids": []})
    snapshot = client.call("get_working_region")["snapshot"]
    client.call("set_working_region", {"snapshot": snapshot, "action": "selection_bounds"}, error=True)
    client.call("new_map", {"discard_changes": True})
    check(not client.call("get_working_region")["enabled"], "New map resets session-local region state")
    return checks


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True, help="Fresh private output directory")
    parser.add_argument("--render", action="store_true")
    args = parser.parse_args()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True)
    with patch.dict("os.environ", {"HAMMERR_RECENT_MAPS_DIR": str(args.output / "recent"), "HAMMERR_BUILD_RECIPES": str(args.output / "recipes.json")}):
        client = Client(None, args.binary.resolve(), args.output)
    try:
        checks = validate(client, args.output, args.render)
        (args.output / "results.json").write_text(json.dumps({"checks": len(checks), "passed": checks}, indent=2) + "\n")
        print(json.dumps({"checks": len(checks), "output": str(args.output)}))
    finally:
        client.close()


if __name__ == "__main__":
    main()
