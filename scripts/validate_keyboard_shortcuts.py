import argparse
import copy
import json
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client


def validate(binary, output):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def state(client):
        return client.call("get_keyboard_shortcuts")

    def change(client, action, error=False, **arguments):
        return client.call("set_keyboard_shortcuts", {
            "snapshot": state(client)["snapshot"], "action": action, **arguments,
        }, error=error)

    client = Client(None, binary, output)
    try:
        initial = state(client)
        path = Path(initial["path"])
        factory = initial["factory_defaults"]
        original_commands = {"undo", "redo", "frame_selection", "delete_selection", "duplicate_selection"}
        unbound_commands = {
            "perspective", "top", "front", "side", "objects", "faces",
            "show_grid", "snap_to_grid", "tool_textures", "selection_outlines",
            "select_tool", "move_tool", "rotate_tool", "scale_tool",
            "draw_brush", "clip_mode",
        }
        expected_commands = original_commands | unbound_commands
        check(initial["ready"] and not initial["stored"] and {row["command"] for row in initial["commands"]} == expected_commands and set(factory) == expected_commands, "shared command identifiers and factory aliases are discoverable without creating storage")
        check(len(expected_commands) == 21 and all(factory[command] == [None, None] for command in unbound_commands), "all 21 commands are discoverable and extension factory bindings start unbound")
        check(path == output / "recovery/shortcuts.json" and not path.exists(), "shortcut storage stays in the private recovery directory")
        block = client.call("create_block", {"mins": [0, 0, 0], "maxs": [64, 64, 64]})["id"]
        before = client.call("get_objects", {"ids": [block]})
        history = client.call("get_history")["undo_count"]
        camera = client.call("get_camera")
        selection = client.call("get_selection")
        custom = copy.deepcopy(factory)
        custom["undo"] = [{"key": "f6", "control": True, "shift": False}, None]
        custom["duplicate_selection"] = [None, None]
        custom["frame_selection"] = [{"key": "F8", "control": False, "shift": True}, None]
        custom["draw_brush"] = [{"key": "F10", "control": False, "shift": True}, None]
        custom["clip_mode"] = [{"key": "F11", "control": False, "shift": True}, None]
        saved = change(client, "save", bindings=custom)
        custom["undo"][0]["key"] = "F6"
        check(saved["write_committed"] and saved["ready"] and saved["current"] == custom, "Save canonicalizes, persists and activates the complete configuration")
        check(client.call("get_objects", {"ids": [block]}) == before and client.call("get_history")["undo_count"] == history and client.call("get_camera") == camera and client.call("get_selection") == selection, "configuration changes no map, history, selection or camera")
        saved_bytes = path.read_bytes()
        for command, field in [
            ("show_grid", "grid_visible"), ("snap_to_grid", "snap"),
            ("tool_textures", "tool_textures"), ("selection_outlines", "selection_outlines"),
        ]:
            for enabled in [False, True]:
                toggled = client.call("execute_editor_command", {"command": command})
                check(toggled["command"] == command and toggled[field] is enabled, f"typed {command} toggles the actual {field} boolean to {enabled}")
        tool_settings = client.call("get_transform_handles")["state"]["settings"]
        for mode in ["move", "rotate", "scale", "select"]:
            command = f"{mode}_tool"
            changed = client.call("execute_editor_command", {"command": command})
            expected_tool = {**tool_settings, "mode": mode}
            check(changed["tool"] == mode and client.call("get_transform_handles")["state"]["settings"] == expected_tool, f"typed {command} selects the shared tool while preserving its other options")
        check(path.read_bytes() == saved_bytes and state(client)["current"] == custom, "viewport commands do not rewrite or rebind saved shortcuts")
        check(client.call("get_objects", {"ids": [block]}) == before and client.call("get_history")["undo_count"] == history and client.call("get_camera") == camera and client.call("get_selection") == selection, "viewport toggles preserve authored state, history, camera and selection")
        stale = initial["snapshot"]
        client.call("set_keyboard_shortcuts", {"action": "save", "snapshot": stale, "bindings": factory}, error=True)
        check(path.read_bytes() == saved_bytes and state(client)["current"] == custom, "stale configuration snapshots cannot overwrite saved shortcuts")
        for bad in [
            {"key": "W", "control": False, "shift": True},
            {"key": "S", "control": True, "shift": False},
            {"key": "Escape", "control": False, "shift": False},
            factory["redo"][0],
            {"key": "F6", "control": True, "shift": False, "alt": True},
        ]:
            invalid = copy.deepcopy(custom)
            invalid["undo"][0] = bad
            change(client, "save", error=True, bindings=invalid)
            check(path.read_bytes() == saved_bytes and state(client)["current"] == custom, f"reserved, duplicate or unsupported chord rejects atomically: {bad}")
        duplicate = client.call("execute_editor_command", {"command": "duplicate_selection"})
        check(len(duplicate["copies"]) == 1 and client.call("get_history")["undo_count"] == history + 1, "typed duplicate command uses the shared grid-offset transaction independently of its binding")
        client.call("execute_editor_command", {"command": "undo"})
        check(client.call("get_objects", {"ids": [block]}) == before and client.call("get_history")["undo_count"] == history, "typed Undo restores the source map")
        client.call("execute_editor_command", {"command": "redo"})
        copy_id = duplicate["copies"][0]
        client.call("select", {"ids": [copy_id]})
        client.call("execute_editor_command", {"command": "delete_selection"})
        client.call("undo")
        check(client.call("get_selection")["objects"] == [], "existing Undo and configured Delete share ordinary selection pruning")
        client.call("select", {"ids": [block]})
        count = client.call("get_history")["undo_count"]
        client.call("execute_editor_command", {"command": "frame_selection", "aspect": 1.5})
        check(client.call("get_history")["undo_count"] == count, "typed Frame selection moves only the camera")
        path.write_bytes(b"malformed storage")
        change(client, "reload", error=True)
        broken = state(client)
        check(not broken["ready"] and broken["error"] and broken["current"] == custom, "invalid Reload retains current valid bindings and exposes unavailable storage")
        change(client, "save", error=True, bindings=factory)
        check(path.read_bytes() == b"malformed storage", "Save stays blocked and malformed storage remains untouched")
        client.close()
        client = Client(None, binary, output)
        broken = state(client)
        check(not broken["ready"] and broken["current"] == factory and path.read_bytes() == b"malformed storage", "startup retains usable factory bindings without labeling malformed storage loaded")
        change(client, "reset")
        check(state(client)["ready"] and state(client)["current"] == factory, "explicit Reset repairs saved storage and activates factory shortcuts")
        change(client, "save", bindings=custom)
        client.close()
        client = Client(None, binary, output)
        check(state(client)["ready"] and state(client)["current"] == custom, "saved custom bindings survive a separate process startup")
        saved_bytes = path.read_bytes()
        spatial = client.call("set_spatial_settings", {"grid_size": 8, "elevation": 12.5, "depth": 24})
        initial_history = client.call("get_history")["undo_count"]
        drawing = client.call("execute_editor_command", {"command": "draw_brush"})
        check(drawing["draw_block"] and drawing["projection"] == "top" and drawing["camera_changed"], "Draw brush enters Top from Perspective without the toolbar Add brush transaction")
        check(client.call("get_map_info")["world_brushes"] == 0 and client.call("get_history")["undo_count"] == initial_history, "entering drawing mode creates no brush or history step")
        footprint = {"start": [0, 0], "end": [64, 32], "material": "tools/toolsnodraw"}
        client.call("draw_block", {**footprint, "preview": True})
        pending = client.call("get_map_info")["block_preview"]
        repeated = client.call("execute_editor_command", {"command": "draw_brush"})
        check(repeated["draw_block"] and not repeated["camera_changed"] and client.call("get_map_info")["block_preview"] == pending, "repeated Draw brush activation retains the pending footprint")
        client.call("execute_editor_command", {"command": "clip_mode"}, error=True)
        rejected = client.call("get_map_info")
        check(rejected["block_preview"] == pending and rejected["spatial"]["draw_block"] and not rejected["clipping"]["enabled"], "Clip mode without a brush selection preserves the pending drawing review")
        drawn = client.call("draw_block", footprint)["id"]
        authored = client.call("get_objects", {"ids": [drawn]})
        check(client.call("get_map_info")["world_brushes"] == 1 and client.call("get_history")["undo_count"] == initial_history + 1, "the subsequent planar footprint creates exactly one brush and one Undo step")
        perspective = client.call("set_camera", {"projection": "perspective"})
        repeated = client.call("execute_editor_command", {"command": "draw_brush"})
        check(repeated["draw_block"] and repeated["projection"] == "perspective" and not repeated["camera_changed"] and client.call("get_camera") == perspective, "reactivating an already active drawing mode preserves an explicitly chosen Perspective camera")
        client.call("set_spatial_settings", {"draw_block": False})
        check(client.call("get_map_info")["spatial"] == spatial, "the existing drawing setter leaves the mode with grid, depth and elevation preserved")
        cut = {"plane": {"normal": [1, 0, 0], "distance": 32}, "keep": "both", "preview": True}
        clipping = client.call("execute_editor_command", {"command": "clip_mode"})
        check(clipping["clip_tool"] and clipping["projection"] == "top" and clipping["camera_changed"], "Clip mode enters Top from Perspective for the selected drawn brush")
        client.call("clip_brushes", cut)
        pending = client.call("get_clip_preview")["preview"]
        clipping = client.call("execute_editor_command", {"command": "clip_mode"})
        check(clipping["clip_tool"] and not clipping["camera_changed"] and client.call("get_clip_preview")["preview"] == pending and client.call("get_objects", {"ids": [drawn]}) == authored and client.call("get_history")["undo_count"] == initial_history + 1, "repeated Clip mode activation preserves the cut review without committing geometry")
        client.call("commit_clip_preview")
        check(client.call("get_map_info")["world_brushes"] == 2 and client.call("get_history")["undo_count"] == initial_history + 2, "explicit Apply cut splits the drawn brush in one Undo step")
        client.call("undo")
        check(client.call("get_objects", {"ids": [drawn]}) == authored, "Undo restores the uncut drawn brush")
        client.call("undo")
        check(client.call("get_map_info")["world_brushes"] == 0 and client.call("get_history")["undo_count"] == initial_history, "the next Undo removes only the footprint creation")
        check(path.read_bytes() == saved_bytes and state(client)["current"] == custom, "authoring mode commands leave the saved bindings unchanged")
    finally:
        client.close()
    return checks


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, default=Path("/tmp/hammerr-keyboard-shortcuts"))
    args = parser.parse_args()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    checks = validate(args.binary, args.output)
    (args.output / "results.json").write_text(json.dumps({"checks": len(checks), "passed": checks}, indent=2) + "\n")
    print(json.dumps({"checks": len(checks), "output": str(args.output)}))


if __name__ == "__main__":
    main()
