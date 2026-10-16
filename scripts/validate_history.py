import argparse
import json
import os
from pathlib import Path
from unittest.mock import patch

from validate_face_lighting import DEFAULT_BINARY, Client, detail


def history(client, **arguments):
    return client.call("get_history", arguments)


def navigate(client, target, state=None):
    state = state or history(client)
    return client.call("navigate_history", {"snapshot": state["snapshot"], "target": target})


def reject(client, tool, arguments):
    try:
        client.call(tool, arguments, error=True)
    except RuntimeError as error:
        if not (
            error.args
            and isinstance(error.args[0], dict)
            and error.args[0].get("code") == -32602
        ):
            raise


def validate(args):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    with patch.dict(os.environ, {
        "HAMMERR_RECENT_MAPS_DIR": str(args.output / "recent-maps"),
        "HAMMERR_BUILD_RECIPES": str(args.output / "build-recipes.json"),
    }):
        client = Client(None, args.binary, args.output)
    try:
        tools = {tool["name"]: tool for tool in client.rpc("tools/list", {})["tools"]}
        check({"get_history", "navigate_history"} <= tools.keys(), "History inspection and navigation are discoverable")
        check(tools["get_history"].get("annotations", {}).get("readOnlyHint") is True, "Inspection is marked read-only")
        for name in ["get_history", "navigate_history"]:
            check(tools[name]["inputSchema"].get("additionalProperties") is False, f"{name} rejects undeclared arguments")
        empty = history(client)
        check(empty["entries"] == [] and empty["current_state"] == "0", "A new document has an explicit initial state")
        check(empty["max_page_size"] == 100 and empty["max_navigation_steps"] == 256 and empty["max_object_samples"] == 16 and empty["max_label_chars"] == 256, "Response and navigation bounds are exposed")
        no_op = navigate(client, "0", empty)
        check(no_op["plan"]["direction"] == "none" and no_op["plan"]["steps"] == 0 and history(client) == empty, "Navigating to the current initial state records no change")
        ids, states = [], []
        for index in range(3):
            ids.append(client.call("create_entity", {"classname": "info_target", "origin": [index * 32, 0, 64], "properties": {"targetname": f"history_{index}"}})["id"])
            states.append(history(client)["current_state"])
        original = detail(client, ids)
        latest = history(client)
        camera = client.call("get_camera")
        check(latest["undo_count"] == 3 and latest["redo_count"] == 0 and latest["entries"][0]["current"], "History reports the exact applied and redo position")
        check(all(entry["actor"] == "agent" and entry["added"] == 1 and entry["modified"] == 0 and entry["removed"] == 0 for entry in latest["entries"]), "Transaction authors and actual object additions are retained")
        first_page = history(client, limit=1)
        older_page = history(client, offset=first_page["next_offset"], limit=2)
        check([entry["state"] for entry in first_page["entries"] + older_page["entries"]] == list(reversed(states)) and older_page["next_offset"] is None, "Bounded pages cover all retained transaction states in order")
        check(detail(client, ids) == original and client.call("get_camera") == camera and history(client) == latest, "Inspection leaves the document, camera and history unchanged")
        for arguments in [{"limit": 0}, {"limit": 101}, {"offset": 4}, {"offset": -1}, {"extra": True}]:
            reject(client, "get_history", arguments)
        for arguments in [{"snapshot": latest["snapshot"]}, {"target": "0"}, {"snapshot": latest["snapshot"], "target": "999999"}, {"snapshot": latest["snapshot"], "target": states[0], "extra": True}]:
            reject(client, "navigate_history", arguments)
        check(history(client) == latest, "Invalid pages and navigation requests reject without mutation")
        moved = navigate(client, states[0], latest)
        check(moved["plan"]["direction"] == "undo" and moved["plan"]["steps"] == 2 and moved["current_state"] == states[0], "Navigation returns the exact undo result")
        check(client.call("get_map_info")["point_entities"] == 1 and detail(client, ids[:1]) == original[:1] and history(client)["redo_count"] == 2, "Undo navigation restores the known source state and retains redo")
        reject(client, "navigate_history", {"snapshot": latest["snapshot"], "target": states[-1]})
        redone = navigate(client, states[-1])
        check(redone["plan"]["direction"] == "redo" and redone["plan"]["steps"] == 2 and detail(client, ids) == original, "Redo navigation restores exact original objects and IDs")
        client.call("set_entity_properties", {"id": ids[0], "set": {"targetname": "history_renamed"}})
        changed = history(client)
        check(changed["entries"][0]["modified"] == 1 and changed["entries"][0]["objects"][0] == {"id": ids[0], "kind": "modified"}, "Modified object summaries use actual transaction data")
        client.call("delete_objects", {"ids": [ids[-1]]})
        deleted = history(client)
        check(deleted["entries"][0]["removed"] == 1 and deleted["entries"][0]["objects"][0]["id"] == ids[-1], "Removed objects stay inspectable after deletion")
        navigate(client, states[-1])
        check(detail(client, ids) == original, "Navigating before modifications and deletions restores the exact original data")
        client.call("create_primitive", {"shape": "wedge", "mins": [0, 0, 0], "maxs": [64, 64, 64], "preview": True})
        draft = client.call("get_primitive_preview")
        blocked = history(client)
        check(blocked["navigation_blocked"] is not None, "History explains active preview navigation guards")
        reject(client, "navigate_history", {"snapshot": blocked["snapshot"], "target": "0"})
        check(client.call("get_primitive_preview") == draft and detail(client, ids) == original, "Rejected navigation preserves the active shared draft and source data")
        client.call("cancel_primitive_preview", {"token": draft["preview"]["token"]})
        path = args.output / "history.vmf"
        client.call("save_map", {"path": str(path)})
        saved_bytes = path.read_bytes()
        saved = history(client)
        check(not saved["unsaved_changes"], "History shows the document's saved state")
        reject(client, "navigate_history", {"snapshot": blocked["snapshot"], "target": "0"})
        no_op = navigate(client, saved["current_state"], saved)
        check(not no_op["unsaved_changes"] and history(client) == saved, "An already-current saved target stays clean and preserves history")
        undone = navigate(client, "0", saved)
        check(undone["unsaved_changes"] and client.call("get_map_info")["point_entities"] == 0 and path.read_bytes() == saved_bytes, "Navigation marks changed source unsaved and never writes the map file")
        navigate(client, states[-1])
        check(detail(client, ids) == original, "Navigation from the initial state restores the retained chain")
        discarded = history(client)["entries"][0]["state"]
        navigate(client, states[0])
        client.call("create_entity", {"classname": "info_target", "origin": [256, 0, 64]})
        branched = history(client)
        check(branched["redo_count"] == 0 and branched["total"] == 2 and branched["current_state"] != discarded, "A new edit follows the existing branch replacement rule with a fresh state ID")
        reject(client, "navigate_history", {"snapshot": branched["snapshot"], "target": discarded})
        old_snapshot = branched["snapshot"]
        client.call("open_map", {"path": str(path), "discard_changes": True})
        reopened = history(client)
        check(reopened["entries"] == [] and reopened["current_state"] == "0" and path.read_bytes() == saved_bytes, "Reopening a map begins a new document history without changing saved bytes")
        reject(client, "navigate_history", {"snapshot": old_snapshot, "target": "0"})
        for index in range(257):
            client.call("set_entity_properties", {"id": ids[0], "set": {"targetname": f"bounded_{index}"}})
        bounded = history(client)
        check(len(bounded["entries"]) == 50 and bounded["total"] == 257, "Long histories keep the default response bounded")
        reject(client, "navigate_history", {"snapshot": bounded["snapshot"], "target": "0"})
        check(history(client) == bounded, "Over-limit navigation rejects atomically")
        intermediate = history(client, offset=256, limit=1)["entries"][0]["state"]
        moved = navigate(client, intermediate, bounded)
        check(moved["plan"]["steps"] == 256 and history(client)["redo_count"] == 256, "The inclusive navigation bound preserves every redo transaction")
        info = client.call("get_map_info")
        check(not info["compiling"] and not info["launching"] and not info["game_running"], "History validation starts no compiler or game")
        result = {"checks": len(checks), "passed": checks, "transport": "stdio", "cross_actor_authorship_exercised": False, "compile_or_game_behavior_exercised": False}
        (args.output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps({"history": True, "checks": len(checks), "output": str(args.output)}))
    finally:
        client.close()


def main():
    parser = argparse.ArgumentParser(description="Validate current-document history inspection and exact navigation through an isolated MCP session.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.binary, args.output = args.binary.resolve(), args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=False)
    validate(args)


if __name__ == "__main__":
    main()
