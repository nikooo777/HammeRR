import argparse
import json
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client
from validate_recovery_exit import require_clean_eof
from validate_visibility import ROOM


def validate(binary, output):
    output.mkdir(parents=True)
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    client = Client(None, binary.resolve(), output)
    try:
        tools = sorted(client.rpc("tools/list", {})["tools"], key=lambda tool: tool["name"])
        check(len({tool["name"] for tool in tools}) == len(tools) > 0, "Tool names are unique")
        check(not client.call("get_map_info")["rendering"]["initialized"], "Startup leaves graphics uninitialized")
        client.call("create_block", {"mins": [0, 0, 0], "maxs": [64, 64, 64], "material": "dev/dev_measuregeneric01"})
        client.call("undo")
        check(client.call("get_map_info")["world_brushes"] == 0, "Undo restores the empty map")
        client.call("redo")
        check(client.call("get_map_info")["world_brushes"] == 1, "Redo restores the brush")
        saved = output / "brush.vmf"
        client.call("save_map", {"path": str(saved)})
        client.call("new_map")
        client.call("open_map", {"path": str(saved)})
        check(client.call("get_map_info")["world_brushes"] == 1, "Saved geometry reopens")
        client.call("new_map")
        for mins, maxs in ROOM:
            client.call("create_block", {"mins": mins, "maxs": maxs, "material": "dev/dev_measuregeneric01"})
        client.call("create_entity", {"classname": "info_player_terrorist", "origin": [0, 0, 8]})
        client.call("set_compile_settings", {"lighting": "fullbright", "physics": "none", "visibility": "fast"})
        saved = output / "room.vmf"
        client.call("save_map", {"path": str(saved)})
        bsp = output / "room.bsp"
        result = client.call("compile_map", {"path": str(bsp)})
        check(result["success"] and bsp.read_bytes()[:4] == b"VBSP", "The independent session compiles native BSP output")
        check(not client.call("get_map_info")["rendering"]["initialized"], "Editing, history, saving and compilation need no graphics initialization")
        client.call("create_entity", {"classname": "info_target", "origin": [0, 0, 64], "properties": {"targetname": "headless_exit_snapshot_marker"}})
    finally:
        check(require_clean_eof(client) == 0, "The MCP server exits cleanly after stdin closes")

    snapshots = [json.loads(path.read_text()) for path in (output / "recovery").glob("*.json") if path.name != "settings.json"]
    snapshots = [entry for entry in snapshots if entry.get("kind") == "autosave"]
    check(any("headless_exit_snapshot_marker" in (output / "recovery" / f"{entry['id']}.vmf").read_text() for entry in snapshots), "Orderly shutdown captures the latest unsaved edit")
    check("headless_exit_snapshot_marker" not in saved.read_text(), "Exit recovery preserves the saved VMF")
    return tools, checks


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--desktop-binary", type=Path)
    parser.add_argument("--baseline", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    tools, checks = validate(args.binary, output / "headless")
    if args.desktop_binary:
        desktop_tools, desktop_checks = validate(args.desktop_binary, output / "desktop-compatible")
        assert tools == desktop_tools, "The standalone and desktop-compatible servers expose different tools or schemas"
        checks += [f"Desktop compatibility: {check}" for check in desktop_checks]
        checks.append("Standalone and desktop-compatible MCP surfaces match")
    if args.baseline:
        assert tools == json.loads(args.baseline.read_text()), "The MCP surface changed from its recorded baseline"
        checks.append("MCP tools and schemas match the recorded baseline")
    (output / "validation.json").write_text(json.dumps({"passed": len(checks), "tools": len(tools), "checks": checks}, indent=2) + "\n")
    print(json.dumps({"passed": len(checks), "tools": len(tools), "output": str(output)}))


if __name__ == "__main__":
    main()
