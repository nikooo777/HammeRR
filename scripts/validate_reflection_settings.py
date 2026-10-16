import argparse
import hashlib
import json
import os
from pathlib import Path
from unittest.mock import patch

from validate_face_lighting import DEFAULT_BINARY, Client
from validate_visibility import ROOM


def validate(args):
    checks = []
    recipe_path = args.output / "build-recipes.json"
    with patch.dict(
        os.environ,
        {
            "HAMMERR_BUILD_RECIPES": str(recipe_path),
            "HAMMERR_RECENT_MAPS_DIR": str(args.output / "recent"),
        },
    ):
        client = Client(None, args.binary, args.output, args.gameinfo)

    def check(condition, label):
        assert condition, label
        checks.append(label)

    try:
        tools = {tool["name"]: tool for tool in client.rpc("tools/list", {})["tools"]}
        schema = tools["set_compile_settings"]["inputSchema"]["properties"]
        check(
            "boolean" in json.dumps(schema["generate_reflections"]),
            "Generation is a discoverable boolean setting",
        )
        check(
            {"get_native_capture_runtime", "set_native_capture_runtime"}
            <= tools.keys(),
            "Runtime configuration tools are discoverable",
        )
        before = client.call("get_map_info")
        initial = client.call("get_compile_settings")
        check(initial["generate_reflections"] is False, "Generation defaults off")
        for lighting in ["fullbright"]:
            client.call(
                "set_compile_settings",
                {
                    "lighting": lighting,
                    "generate_reflections": True,
                    "physics": "engine",
                },
                error=True,
            )
            check(
                client.call("get_compile_settings") == initial,
                f"Invalid {lighting} generation preserves the entire settings update",
            )
        enabled = client.call(
            "set_compile_settings",
            {"lighting": "direct_ldr", "generate_reflections": True},
        )
        check(
            enabled["generate_reflections"] is True
            and enabled["lighting"] == "direct_ldr",
            "Direct LDR and generation enable atomically",
        )
        for lighting in ["fullbright"]:
            client.call("set_compile_settings", {"lighting": lighting}, error=True)
            check(
                client.call("get_compile_settings") == enabled,
                f"Enabled generation rejects switching to {lighting}",
            )
        dual = client.call("set_compile_settings", {"lighting": "direct_ldr_hdr"})
        check(dual["generate_reflections"] is True and dual["lighting"] == "direct_ldr_hdr", "HDR lighting retains generation for both channels")
        enabled = client.call("set_compile_settings", {"lighting": "direct_ldr"})
        saved = client.call(
            "save_build_recipe", {"name": "Reflection settings validation"}
        )
        recipe_id = saved["active"]["id"]
        for builtin in ["fast", "final"]:
            check(
                client.call("apply_build_recipe", {"id": builtin})["settings"][
                    "generate_reflections"
                ]
                is False,
                f"Built-in {builtin} keeps generation off",
            )
        restored = client.call("apply_build_recipe", {"id": recipe_id})
        check(
            restored["settings"]["generate_reflections"] is True,
            "Custom recipe restores generation",
        )
        stored = json.loads(recipe_path.read_text())
        check(
            stored["recipes"][0]["settings"]["generate_reflections"] is True,
            "Custom recipe persists the generation setting",
        )
        runtime = client.call("get_native_capture_runtime")
        client.call(
            "set_native_capture_runtime",
            {
                "engine_root": str(args.output / "absent-install"),
                "steam_root": str(args.output / "absent-steam"),
            },
            error=True,
        )
        check(
            client.call("get_native_capture_runtime") == runtime,
            "Invalid runtime pair preserves both prior overrides",
        )
        runtime_roundtrip = False
        if runtime["ready"] and runtime["engine_root"] and runtime["steam_root"]:
            configured = client.call(
                "set_native_capture_runtime",
                {
                    "engine_root": runtime["engine_root"],
                    "steam_root": runtime["steam_root"],
                },
            )
            check(
                configured["configured_engine_root"] == runtime["engine_root"]
                and configured["configured_steam_root"] == runtime["steam_root"],
                "Resolved valid roots can be applied as explicit overrides",
            )
            reset = client.call(
                "set_native_capture_runtime",
                {
                    "engine_root": runtime["configured_engine_root"],
                    "steam_root": runtime["configured_steam_root"],
                },
            )
            check(
                reset == runtime,
                "Replacing both overrides restores the original automatic configuration",
            )
            runtime_roundtrip = True
        after = client.call("get_map_info")
        check(
            all(
                after[key] == before[key]
                for key in [
                    "undo",
                    "redo",
                    "unsaved_changes",
                    "world_brushes",
                    "selection",
                ]
            ),
            "Settings, runtime and recipes preserve source history and selection",
        )
        check(
            client.call("get_compile_status")["job"] is None,
            "Settings validation starts no compile or native client",
        )
        if args.check_staleness:
            for mins, maxs in ROOM:
                client.call(
                    "create_block",
                    {
                        "mins": mins,
                        "maxs": maxs,
                        "material": "dev/dev_measuregeneric01",
                    },
                )
            client.call(
                "create_entity",
                {"classname": "info_player_terrorist", "origin": [0, 0, 8]},
            )
            client.call(
                "set_compile_settings",
                {
                    "lighting": "direct_ldr",
                    "physics": "none",
                    "generate_reflections": False,
                    "bounces": 0,
                },
            )
            bsp = args.output / "settings.bsp"
            result = client.call("compile_map", {"path": str(bsp)})
            check(
                result["success"],
                "Generation-off baseline compiles without native capture",
            )
            baseline = hashlib.sha256(bsp.read_bytes()).hexdigest()
            frozen = client.call("get_compile_status")["job"]["settings"]
            client.call("set_compile_settings", {"generate_reflections": True})
            stale = client.call("get_compile_status")
            check(
                stale["job"]["stale"]
                and not stale["launch"]["ready"]
                and stale["job"]["settings"] == frozen,
                "Generation alone stales output while retaining frozen job settings",
            )
            client.call("set_compile_settings", {"generate_reflections": False})
            current = client.call("get_compile_status")
            check(
                not current["job"]["stale"] and current["launch"]["ready"],
                "Restoring generation-off settings restores freshness",
            )
            check(
                hashlib.sha256(bsp.read_bytes()).hexdigest() == baseline,
                "Settings changes leave published BSP bytes untouched",
            )
        report = {
            "checks": checks,
            "runtime": runtime,
            "runtime_override_roundtrip_checked": runtime_roundtrip,
            "compile_staleness_checked": args.check_staleness,
            "native_launch_requested": False,
        }
        (args.output / "validation.json").write_text(
            json.dumps(report, indent=2) + "\n"
        )
        print(json.dumps({"checks": len(checks), "output": str(args.output)}))
    finally:
        client.close()


def main():
    parser = argparse.ArgumentParser(
        description="Validate shared reflection settings and runtime configuration over isolated stdio MCP; never launch a native client."
    )
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--gameinfo", type=Path)
    parser.add_argument(
        "--check-staleness",
        action="store_true",
        help="Also compile one generation-off LDR baseline and verify generation-only staleness; no native capture or game launch",
    )
    args = parser.parse_args()
    args.binary = args.binary.resolve()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=False)
    validate(args)


if __name__ == "__main__":
    main()
