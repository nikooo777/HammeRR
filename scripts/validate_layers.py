import argparse
import json
from pathlib import Path
from unittest.mock import patch

from validate_face_lighting import ROOT, Client, detail
from validate_terrain_lod import build
from validate_visibility import ROOM


def state(client, **args):
    return client.call("get_layers", args)


def begin(client, layer=None, delete=False):
    return client.call("begin_layer_edit", {"snapshot": state(client)["snapshot"], "id": layer, "delete": delete})["draft"]


def update(client, draft, name, members, parent=None, color=None):
    return client.call("update_layer_edit", {"token": draft["token"], "version": draft["version"], "name": name, "color": color or [40, 160, 220], "parent": parent, "members": members})["draft"]


def apply(client, draft):
    return client.call("apply_layer_edit", {"token": draft["token"], "version": draft["version"]})


def validate(args):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    directories = [args.output / "source", args.output / "destination"]
    for directory in directories:
        directory.mkdir(parents=True)
    with patch.dict("os.environ", {"HAMMERR_RECENT_MAPS_DIR": str(args.output / "recent-maps"), "HAMMERR_BUILD_RECIPES": str(args.output / "recipes.json")}):
        source = Client(None, args.binary, directories[0])
        destination = Client(None, args.binary, directories[1])
    try:
        tools = {tool["name"]: tool for tool in source.rpc("tools/list", {})["tools"]}
        expected = {"get_layers", "begin_layer_edit", "update_layer_edit", "apply_layer_edit", "cancel_layer_edit", "select_layer_members", "set_layer_visibility"}
        check(expected <= tools.keys(), "Layer tools are discoverable")
        check(all(tools[name]["inputSchema"].get("additionalProperties") is False for name in expected), "Unknown layer settings reject")
        for low, high in ROOM:
            source.call("create_block", {"mins": low, "maxs": high, "material": "tools/toolsnodraw"})
        source.call("create_entity", {"classname": "info_player_terrorist", "origin": [-128, -128, 8]})
        brush = source.call("create_block", {"mins": [0, 0, 0], "maxs": [32, 32, 32], "material": "dev/dev_measuregeneric01"})["id"]
        owned = source.call("create_block", {"mins": [64, 0, 0], "maxs": [96, 32, 32], "material": "dev/dev_measuregeneric01"})["id"]
        entity = source.call("tie_to_entity", {"ids": [owned], "classname": "func_detail"})["id"]
        original = source.call("get_map_info")
        draft = begin(source)
        root_draft = update(source, draft, "Architecture", [brush], color=[20, 180, 220])
        check(source.call("get_map_info")["undo"] == original["undo"], "Layer drafts preserve history")
        source.call("apply_layer_edit", {"token": draft["token"], "version": draft["version"]}, error=True)
        source.call("cancel_layer_edit", {"token": draft["token"], "version": draft["version"]}, error=True)
        check(state(source)["draft"] == root_draft, "Stale Apply and Cancel retain newer draft")
        root = apply(source, root_draft)["layer"]
        child = apply(source, update(source, begin(source), "Detail", [owned], parent=root))["layer"]
        check(state(source, selected=child)["selected"]["members"][0]["owner"] == entity, "Native child-brush membership reports whole entity owner")
        stale = state(source)["snapshot"]
        draft = begin(source, child)
        invalid = update(source, draft, "Detail", [owned], parent=child)
        check(not invalid["valid"], "Layer cycle rejects before map mutation")
        source.call("apply_layer_edit", {"token": invalid["token"], "version": invalid["version"]}, error=True)
        source.call("cancel_layer_edit", {"token": invalid["token"], "version": invalid["version"]})
        source.call("select_layer_members", {"id": root, "snapshot": stale}, error=True)
        selection = source.call("select_layer_members", {"id": root, "recursive": True, "snapshot": state(source)["snapshot"]})
        check(set(selection["selection"]) == {brush, entity}, "Recursive selection resolves native child-brush owners")
        selection = source.call("select_layer_members", {"id": root, "recursive": False, "snapshot": state(source)["snapshot"]})
        check(selection["selection"] == [brush], "Direct selection excludes child layers")
        source.call("set_layer_visibility", {"id": child, "action": "hide", "snapshot": state(source)["snapshot"]})
        check(state(source, selected=child)["selected"]["members"][0]["hidden"], "Owned-brush layer hide affects its whole entity")
        check(source.call("select_layer_members", {"id": child, "snapshot": state(source)["snapshot"]})["selection"] == [], "Default layer selection skips hidden owners")
        _, hidden_stats = build(source, args.output, "hidden-layer")
        source.call("set_layer_visibility", {"id": child, "action": "show", "snapshot": state(source)["snapshot"]})
        _, shown_stats = build(source, args.output, "shown-layer")
        check(shown_stats["brushes"] == hidden_stats["brushes"] + 1, "Shared layer visibility controls both compiler paths")
        source.call("undo")
        check(state(source, selected=child)["selected"]["members"][0]["hidden"], "Undo restores layer visibility")
        source.call("redo")
        check(not state(source, selected=child)["selected"]["members"][0]["hidden"], "Redo restores visible members")
        draft = update(source, begin(source, root), "Architecture and detail", [brush, owned], color=[180, 80, 220])
        apply(source, draft)
        check(state(source, selected=root)["selected"]["member_total"] == 2, "Membership edits preserve memberships in other layers")
        copied = source.call("copy_objects", {"ids": [entity], "snapshot": source.call("get_clipboard")["snapshot"], "destination": "session"})
        check(copied["payload"]["layers"] == 2, "Clipboard retains only required layer ancestors")
        exported = source.call("export_clipboard", {"content_revision": copied["payload"]["content_revision"]})
        source.call("new_map", {"discard_changes": True})
        destination_root = apply(destination, update(destination, begin(destination), "Existing", []))["layer"]
        destination.call("import_clipboard", {"text": exported["text"], "snapshot": destination.call("get_clipboard")["snapshot"]})
        paste = destination.call("preview_paste", {"snapshot": destination.call("get_clipboard")["snapshot"]})
        pasted = destination.call("apply_paste", {"token": paste["preview"]["token"]})
        records = state(destination)["layers"]
        check(len(records) == 3 and any(row["id"] == destination_root and row["name"] == "Existing" for row in records), "Cross-map paste remaps layers without changing destination records")
        check(all(new != old for old, new in pasted["visgroup_ids"]), "Transferred layer IDs differ from original IDs")
        child = next(row["id"] for row in records if row["name"] == "Detail")
        root = next(row["id"] for row in records if row["name"] == "Architecture and detail")
        check(next(row["parent"] for row in records if row["id"] == child) == root, "Transferred nested parent IDs follow remapping")
        check(state(destination, selected=root)["selected"]["member_total"] == 1 and state(destination, selected=child)["selected"]["member_total"] == 1, "Transferred owned-brush memberships retain multiple layers")
        destination.call("undo")
        check(state(destination)["total"] == 1, "Undo removes pasted layer closure and memberships")
        destination.call("redo")
        saved = args.output / "layers.vmf"
        destination.call("save_map", {"path": str(saved)})
        destination.call("open_map", {"path": str(saved), "discard_changes": True})
        check(state(destination)["layers"] == records, "Native VMF reopen retains hierarchy, colors and membership")
        deleted = begin(destination, root, delete=True)
        apply(destination, deleted)
        check(state(destination, selected=child)["selected"]["parent"] is None and len(detail(destination, pasted["objects"])) == 1, "Delete promotes child layers and preserves objects")
        destination.call("undo")
        check(state(destination)["layers"] == records, "Undo restores deleted records and exact membership")
        report = {"checks": checks, "passed": len(checks), "desktop_input": False}
        (args.output / "validation.json").write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps(report, indent=2))
    finally:
        source.close()
        destination.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.binary = args.binary.resolve()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True)
    validate(args)


if __name__ == "__main__":
    main()
