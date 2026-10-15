import argparse
import json
import os
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_terrain_lod import build
from validate_visibility import ROOM


def state(client):
    return client.call("get_clipboard")


def capture(client, ids, cut=False, system=False):
    return client.call("cut_objects" if cut else "copy_objects", {
        "snapshot": state(client)["snapshot"], "ids": ids,
        "destination": "system" if system else "session",
    })


def preview(client, **args):
    return client.call("preview_paste", {"snapshot": state(client)["snapshot"], **args})


def validate(args):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    directories = [args.output / "source", args.output / "destination"]
    for directory in directories:
        directory.mkdir(parents=True, exist_ok=True)
    source = Client(None, args.binary, directories[0])
    destination = Client(None, args.binary, directories[1])
    try:
        schemas = {tool["name"]: tool["inputSchema"] for tool in source.rpc("tools/list", {})["tools"]}
        check({"get_clipboard", "copy_objects", "cut_objects", "import_clipboard", "export_clipboard", "preview_paste", "apply_paste", "cancel_paste"} <= schemas.keys(), "All shared clipboard tools are discoverable")
        check(schemas["preview_paste"].get("additionalProperties") is False, "Unknown paste settings reject")
        brush = source.call("create_block", {"mins": [-32, -32, 0], "maxs": [32, 32, 32], "material": "dev/dev_measuregeneric01"})["id"]
        parent = source.call("create_entity", {"classname": "info_target", "origin": [0, 0, 48], "properties": {"targetname": "transfer_parent"}})["id"]
        child = source.call("create_entity", {"classname": "info_target", "origin": [32, 0, 48], "properties": {"targetname": "transfer_child", "parentname": "transfer_parent,attachment"}})["id"]
        link = source.call("begin_connection_edit", {"id": child})["token"]
        source.call("update_connection_draft", {"token": link, "output": "OnUser1", "target": "transfer_parent", "input": "SetParent", "parameter": "transfer_parent", "delay": "0.00", "times": "-1"})
        source.call("commit_connection_edit", {"token": link})
        link = source.call("begin_connection_edit", {"id": child})["token"]
        source.call("update_connection_draft", {"token": link, "output": "OnUser2", "target": "outside", "input": "Trigger", "delay": "0", "times": "-1"})
        source.call("commit_connection_edit", {"token": link})
        note = source.call("begin_context_edit", {"scope": brush})["draft"]["token"]
        source.call("update_context_edit", {"token": note, "summary": "Transfer source", "body": "Retain the object note and author across maps."})
        source.call("apply_context_edit", {"token": note})
        group = source.call("begin_object_group_edit")["token"]
        source.call("update_object_group_edit", {"token": group, "name": "Transfer assembly", "members": [brush, parent, child]})
        source.call("apply_object_group_edit", {"token": group})
        ids = [brush, parent, child]
        source.call("select", {"ids": ids})
        original = detail(source, ids)
        camera = source.call("get_camera")
        history = source.call("get_map_info")["undo"]
        copied = capture(source, ids, system=args.native)
        check(detail(source, ids) == original and source.call("get_map_info")["undo"] == history, "Copy preserves source geometry and history")
        check(source.call("get_camera") == camera, "Copy preserves camera")
        check(copied["payload"]["groups"] == 1 and len(copied["payload"]["external_links"]) == 1, "Clipboard reports native group scope and outgoing external link")
        exported = source.call("export_clipboard", {"content_revision": copied["payload"]["content_revision"]})
        check(exported["format"] == "vmf" and "hammerr_transfer" in exported["text"], "Export is a versioned portable VMF payload")
        if args.native:
            destination.call("import_clipboard", {"system": True, "snapshot": state(destination)["snapshot"]})
            check(state(destination)["payload"]["bytes"] == copied["payload"]["bytes"], "Independent native clipboard reader receives retained VMF text")
        else:
            destination.call("import_clipboard", {"text": exported["text"], "snapshot": state(destination)["snapshot"]})
        for low, high in ROOM:
            destination.call("create_block", {"mins": low, "maxs": high, "material": "tools/toolsnodraw"})
        destination.call("create_entity", {"classname": "info_player_terrorist", "origin": [-128, -128, 8]})
        outside = destination.call("create_entity", {"classname": "info_target", "origin": [-64, 64, 48], "properties": {"targetname": "outside"}})["id"]
        destination.call("set_view", {"tool_textures": False})
        source.call("new_map", {"discard_changes": True})
        check(state(source)["payload"] is not None, "Detached clipboard survives source map replacement")
        request = {"offset": [80, 0, 0], "keep_external_links": True}
        info = destination.call("get_map_info")
        dest_camera = destination.call("get_camera")
        snapshot = state(destination)
        destination.call("preview_paste", {"snapshot": snapshot["snapshot"]}, error=True)
        check(state(destination) == snapshot, "Unacknowledged external links reject without changing clipboard or draft")
        camera_args = {"position": [340, -410, 280], "look_at": [65, 0, 24]}
        baseline = destination.render(args.output / "before.png", camera=camera_args)
        planned = preview(destination, **request)
        token = planned["preview"]["token"]
        result = planned["preview"]["result"]
        check(len(result["objects"]) == 3 and len(result["group_ids"]) == 1, "Paste preview remaps complete native group and object scope")
        check(result["external_links"][0]["destination_matches"] == [int(outside.split(":")[1])], "External-link review resolves current destination names")
        check(all(pair["source"] != pair["copy"] for pair in result["object_ids"]) and all(old != new for old, new in result["side_ids"]), "Cross-map object and face IDs are fresh even with overlapping source IDs")
        check(destination.call("get_map_info")["undo"] == info["undo"] and destination.call("get_camera") == dest_camera, "Preview preserves destination history and camera")
        check(destination.call("get_map_info")["clipboard"] == state(destination), "Map info exposes the shared clipboard draft")
        draft_pixels = destination.render(args.output / "clipboard-preview.png", camera=camera_args)
        check(draft_pixels != baseline, "Offscreen rendering displays the detached paste preview")
        for invalid in ["world {", "hammerr_transfer { version 999 } world { id 1 }", "a {" * 65 + "}" * 65]:
            destination.call("import_clipboard", {"text": invalid, "snapshot": state(destination)["snapshot"]}, error=True)
            check(state(destination)["preview"]["token"] == token, "Invalid import retains the owned valid preview")
        destination.call("preview_paste", {"snapshot": "stale", **request}, error=True)
        destination.call("preview_paste", {"snapshot": state(destination)["snapshot"], "unknown": True}, error=True)
        destination.call("preview_paste", {"snapshot": state(destination)["snapshot"], "offset": [65536, 0, 0], "keep_external_links": True}, error=True)
        check(state(destination)["preview"]["token"] == token, "Invalid paste requests retain the owned valid preview")
        destination.call("cancel_paste", {"token": token})
        check(destination.render(args.output / "cancelled.png", camera=camera_args) == baseline, "Cancel restores exact destination pixels")
        first = preview(destination, **request)
        replacement = preview(destination, **request)
        destination.call("apply_paste", {"token": first["preview"]["token"]}, error=True)
        destination.call("cancel_paste", {"token": first["preview"]["token"]}, error=True)
        check(state(destination)["preview"]["token"] == replacement["preview"]["token"], "Stale Apply and Cancel preserve replacement paste")
        applied = destination.call("apply_paste", {"token": replacement["preview"]["token"]})
        check(destination.render(args.output / "clipboard-paste.png", camera=camera_args) == draft_pixels, "Applied geometry and preview pixels agree exactly")
        pasted = detail(destination, applied["objects"])
        properties = [dict(obj.get("properties", [])) for obj in pasted]
        check(any(obj.get("targetname") == "transfer_child_copy1" and obj.get("parentname") == "transfer_parent_copy1,attachment" for obj in properties), "Exact internal parent links and attachment suffix follow unique copied names")
        copied_note = destination.call("get_context_note", {"scope": applied["objects"][0]})
        check(copied_note["note"]["author"] == "agent" and copied_note["note"]["summary"] == "Transfer source", "Object note content and authorship survive detached transfer")
        check(destination.call("get_map_info")["undo"][0] == "paste objects", "Apply creates one paste Undo step")
        destination.call("undo")
        check(destination.call("get_map_info")["world_brushes"] == 6, "Undo removes the complete paste")
        destination.call("redo")
        check(detail(destination, applied["objects"]) == pasted, "Redo restores exact objects and IDs")
        saved = args.output / "clipboard-paste.vmf"
        destination.call("save_map", {"path": str(saved)})
        destination.call("open_map", {"path": str(saved), "discard_changes": True})
        check(detail(destination, applied["objects"]) == pasted, "Native VMF save/reopen retains transferred geometry and metadata")
        _, stats = build(destination, args.output, "clipboard-paste")
        check(stats["brushes"] > 0, "Transferred room compiles identically through foreground and background MCP paths")
        source.call("import_clipboard", {"text": exported["text"], "snapshot": state(source)["snapshot"]})
        draft = preview(source, keep_external_links=True)
        source.call("apply_paste", {"token": draft["preview"]["token"]})
        moved_id = source.call("get_map_info")["selection"][1]
        cut = capture(source, [moved_id], cut=True)
        check(cut["can_restore_cut_names"], "Same-map Cut offers guarded original-name restoration")
        moved = preview(source, offset=[0, 64, 0])
        check(moved["preview"]["restore_cut_names"], "Unchanged same-map Cut defaults to restoring its names")
        source.call("cancel_paste", {"token": moved["preview"]["token"]})
        source.call("undo")
        check(not state(source)["can_restore_cut_names"], "Undo invalidates the Cut-name receipt")
        results = {"count": len(checks), "checks": checks, "native": args.native}
        (args.output / "validation.json").write_text(json.dumps(results, indent=2) + "\n")
        return len(checks)
    finally:
        destination.close()
        source.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--native", action="store_true", help="Requires an isolated clipboard-capable test display")
    args = parser.parse_args()
    args.binary = args.binary.resolve()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    if args.native and os.environ.get("HAMMERR_ISOLATED_CLIPBOARD") != "1":
        parser.error("Native checks require HAMMERR_ISOLATED_CLIPBOARD=1 on an isolated display")
    count = validate(args)
    print(json.dumps({"clipboard": True, "checks": count, "native": args.native}))


if __name__ == "__main__":
    main()
