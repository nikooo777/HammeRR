import argparse
import json
import os
from pathlib import Path
from unittest.mock import patch

from validate_compile_jobs import wait_job
from validate_face_lighting import DEFAULT_BINARY, Client


def protected_state(client):
    info = client.call("get_map_info")
    return {
        **{key: info[key] for key in ["path", "unsaved_changes", "selection", "face_selection", "undo", "redo", "camera", "view", "world_brushes", "worldspawn"]},
        "generation": info["recovery"]["generation"],
        "revision": info["recovery"]["revision"],
    }


def validate_reload(binary, output, gameinfo, check):
    case = output / "reload"
    case.mkdir()
    path = case / "build-recipes.json"
    with patch.dict(os.environ, {"HAMMERR_BUILD_RECIPES": str(path)}):
        client = Client(None, binary, case, gameinfo)
    try:
        saved = client.call("save_build_recipe", {"name": "Reload original"})
        identifier = saved["active"]["id"]
        measurement = client.call("get_measurements")
        client.call("set_measurement", {"action": "preview", "snapshot": measurement["snapshot"], "name": "Owned clearance", "start": [0, 0, 0], "end": [64, 0, 0]})
        owned = client.call("get_measurements")["draft"]
        before = protected_state(client)
        options = saved["settings"]
        file = json.loads(path.read_text())
        file["recipes"][0]["name"] = "Reload external"
        valid = json.dumps(file).encode()
        path.write_bytes(valid)
        loaded = client.call("reload_build_recipes", {"revision": saved["revision"]})
        check(loaded["active"]["name"] == "Reload external" and loaded["settings"] == options, "Reload adopts an external rename only while saved values match current options")
        client.call("reload_build_recipes", {"revision": saved["revision"]}, error=True)
        client.call("delete_build_recipe", {"id": identifier, "revision": saved["revision"]}, error=True)
        check(client.call("get_build_recipes") == loaded, "Stale Reload and stale edits preserve the current recipe registry")
        for invalid in [b"malformed recipes", b" " * 65_537]:
            path.write_bytes(invalid)
            client.call("reload_build_recipes", error=True)
            failed = client.call("get_build_recipes")
            client.call("save_build_recipe", {"name": "Blocked"}, error=True)
            check(failed["recipes"] == loaded["recipes"] and failed["active"] == loaded["active"] and failed["persistence"] == "blocked" and not failed["custom_editable"] and failed["error"] and path.read_bytes() == invalid, "Failed bounded Reload retains the valid registry and blocks writes without replacing storage")
        path.write_bytes(valid)
        repaired = client.call("reload_build_recipes")
        check(repaired["error"] is None and repaired["custom_editable"], "Storage repair plus Reload restores editing without restart")
        file["recipes"][0]["settings"]["bounces"] = 1
        path.write_text(json.dumps(file))
        changed = client.call("reload_build_recipes")
        check(changed["active"] is None and changed["settings"] == options, "Changed saved values clear active identity without applying the recipe")
        check(protected_state(client) == before and client.call("get_measurements")["draft"] == owned, "Reload, failed repair and stale requests preserve map/history/camera and the Agent-owned review")
        check(not client.call("get_map_info")["rendering"]["initialized"], "Recipe Reload never initializes graphics")
    finally:
        client.close()
    startup = output / "reload-startup"
    startup.mkdir()
    path.write_bytes(b"malformed startup recipes")
    with patch.dict(os.environ, {"HAMMERR_BUILD_RECIPES": str(path)}):
        client = Client(None, binary, startup, gameinfo)
    try:
        check(client.call("get_build_recipes")["persistence"] == "blocked", "Malformed startup storage remains visibly blocked")
        path.write_bytes(valid)
        restored = client.call("reload_build_recipes")
        check(restored["persistence"] == "persistent" and restored["active"]["id"] == "fast" and len(restored["recipes"]) == 3, "Reload repairs malformed startup storage and retains current built-in settings")
    finally:
        client.close()


def validate(binary, output, gameinfo):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    path = output / "build-recipes.json"
    with patch.dict(os.environ, {"HAMMERR_BUILD_RECIPES": str(path)}):
        client = Client(None, binary, output, gameinfo)
    try:
        initial = client.call("get_build_recipes")
        check(
            [entry["id"] for entry in initial["recipes"]] == ["fast", "final"],
            "Fast preview and Final lighting expose exact supported settings",
        )
        check(initial["path"] == str(path), "Recipe storage override is isolated")
        before = client.call("get_map_info")
        final = client.call("apply_build_recipe", {"id": "final"})
        check(
            final["settings"]
            == {
                "lighting": "direct_ldr",
                "physics": "engine",
                "sky_quality": "detailed",
                "visibility": "detailed",
                "bounces": 4,
                "smoothing_angle": 45,
                "generate_reflections": False,
            },
            "Final lighting applies all seven compiler settings atomically",
        )
        client.call("set_compile_settings", {"physics": "none", "bounces": 1})
        check(
            client.call("get_compile_settings")["recipe"] is None,
            "Manual changes clear the applied recipe label",
        )
        saved = client.call("save_build_recipe", {"name": "Evening preview"})
        custom_id = saved["active"]["id"]
        client.call("apply_build_recipe", {"id": "fast"})
        restored = client.call("apply_build_recipe", {"id": custom_id})
        check(
            restored["settings"]["bounces"] == 1
            and restored["settings"]["physics"] == "none",
            "Saved custom recipe restores all current values",
        )
        client.call("rename_build_recipe", {"id": custom_id, "name": "Evening review"})
        client.call(
            "delete_build_recipe",
            {"id": custom_id, "revision": saved["revision"]},
            error=True,
        )
        check(
            client.call("get_build_recipes")["active"]["name"] == "Evening review",
            "Rename updates the active name and stale revisions reject deletion",
        )
        client.call("save_build_recipe", {"name": "FAST PREVIEW"}, error=True)
        client.call("delete_build_recipe", {"id": "final"}, error=True)
        check(
            len(client.call("get_build_recipes")["recipes"]) == 3,
            "Built-ins stay immutable and names are unique ignoring case",
        )
        after = client.call("get_map_info")
        check(
            all(
                after[key] == before[key] for key in ["undo", "redo", "unsaved_changes"]
            ),
            "Recipe edits and applying settings preserve map history and dirty state",
        )
        client.call("apply_build_recipe", {"id": "fast"})
        for mins, maxs in [
            ([-272, -272, -16], [272, 272, 0]),
            ([-272, -272, 256], [272, 272, 272]),
            ([-272, -272, 0], [-256, 272, 256]),
            ([256, -272, 0], [272, 272, 256]),
            ([-256, -272, 0], [256, -256, 256]),
            ([-256, 256, 0], [256, 272, 256]),
        ]:
            client.call(
                "create_block",
                {"mins": mins, "maxs": maxs, "material": "dev/dev_measuregeneric01"},
            )
        client.call(
            "create_entity", {"classname": "info_player_terrorist", "origin": [0, 0, 8]}
        )
        geometry = client.call("save_build_recipe", {"name": "Geometry pass"})
        geometry_id = geometry["active"]["id"]
        started = client.call("start_compile", {"path": str(output / "room.bsp")})
        check(
            started["job"]["recipe"]["name"] == "Geometry pass",
            "Compile jobs capture the recipe name at start",
        )
        completed = wait_job(client)
        check(
            completed["status"] == "completed",
            "Fast custom recipe builds a sealed room",
        )
        client.call(
            "rename_build_recipe", {"id": geometry_id, "name": "Geometry review"}
        )
        status = client.call("get_compile_status")
        check(
            status["job"]["recipe"]["name"] == "Geometry pass"
            and not status["job"]["stale"]
            and status["launch"]["ready"],
            "Renaming preserves completed job provenance and build freshness",
        )
        client.call("reload_build_recipes")
        reloaded_status = client.call("get_compile_status")
        check(reloaded_status["job"]["recipe"] == status["job"]["recipe"] and reloaded_status["job"]["settings"] == status["job"]["settings"] and reloaded_status["launch"]["ready"], "Reload preserves captured completed-job identity, settings and launch readiness")
        client.call("set_compile_settings", {"lighting": "direct_ldr"})
        check(
            client.call("get_compile_status")["job"]["stale"],
            "Different values stale output",
        )
        client.call("apply_build_recipe", {"id": geometry_id})
        check(
            client.call("get_compile_status")["launch"]["ready"],
            "Restoring recipe values restores freshness",
        )
        client.call("delete_build_recipe", {"id": geometry_id})
        status = client.call("get_compile_status")
        check(
            status["job"]["recipe"]["name"] == "Geometry pass"
            and status["launch"]["ready"]
            and status["settings"]["recipe"] is None,
            "Deleting a recipe preserves options and job provenance",
        )
    finally:
        client.close()

    restart = output / "restart"
    restart.mkdir()
    with patch.dict(os.environ, {"HAMMERR_BUILD_RECIPES": str(path)}):
        client = Client(None, binary, restart, gameinfo)
    try:
        state = client.call("get_build_recipes")
        check(
            len(state["recipes"]) == 3
            and state["recipes"][2]["name"] == "Evening review"
            and state["persistence"] == "persistent"
            and state["error"] is None,
            "Custom recipe names and settings survive a headless restart",
        )
        restored = client.call("apply_build_recipe", {"id": custom_id})
        check(
            restored["settings"]["lighting"] == "direct_ldr"
            and restored["settings"]["bounces"] == 1,
            "Restarted recipe applies the stored settings",
        )
    finally:
        client.close()
    validate_reload(binary, output, gameinfo, check)
    result = {"checks": len(checks), "passed": checks, "recipe_store": str(path)}
    (output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"checks": len(checks)}))


def main():
    parser = argparse.ArgumentParser(
        description="Validate named build recipes through isolated headless MCP sessions."
    )
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--gameinfo", type=Path)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    validate(args.binary.resolve(), output, args.gameinfo)


if __name__ == "__main__":
    main()
