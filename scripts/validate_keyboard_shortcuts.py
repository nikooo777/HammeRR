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
        check(initial["ready"] and not initial["stored"] and len(initial["commands"]) == 5, "five commands and factory aliases are discoverable without creating storage")
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
        saved = change(client, "save", bindings=custom)
        custom["undo"][0]["key"] = "F6"
        check(saved["write_committed"] and saved["ready"] and saved["current"] == custom, "Save canonicalizes, persists and activates the complete configuration")
        check(client.call("get_objects", {"ids": [block]}) == before and client.call("get_history")["undo_count"] == history and client.call("get_camera") == camera and client.call("get_selection") == selection, "configuration changes no map, history, selection or camera")
        saved_bytes = path.read_bytes()
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
