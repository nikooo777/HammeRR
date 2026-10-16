import argparse
import json
import os
from pathlib import Path
from unittest.mock import patch

from validate_compile_jobs import wait_job
from validate_face_lighting import DEFAULT_BINARY, Client


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
