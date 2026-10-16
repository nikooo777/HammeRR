import argparse
import json
from pathlib import Path
from unittest.mock import patch

from validate_face_lighting import DEFAULT_BINARY, Client, detail


def report(client, **args):
    return client.call("get_prefabs", args)


def launch(binary, output, library):
    output.mkdir(parents=True)
    with patch.dict("os.environ", {"HAMMERR_PREFABS_DIR": str(library), "HAMMERR_RECENT_MAPS_DIR": str(output / "recent-maps"), "HAMMERR_BUILD_RECIPES": str(output / "recipes.json")}):
        return Client(None, binary, output)


def validate(args):
    checks = []

    def check(value, label):
        assert value, label
        checks.append(label)

    library = args.output / "library"
    source = launch(args.binary, args.output / "source", library)
    destination = launch(args.binary, args.output / "destination", library)
    restarted = None
    try:
        schemas = {tool["name"]: tool["inputSchema"] for tool in source.rpc("tools/list", {})["tools"]}
        tools = {"get_prefabs", "save_prefab", "preview_prefab", "delete_prefab", "rename_prefab"}
        check(tools <= schemas.keys() and all(schemas[name].get("additionalProperties") is False for name in tools), "All shared prefab tools and strict settings are discoverable")
        check({"position", "rotation", "scale", "pivot"} <= schemas["preview_prefab"]["properties"].keys(), "Prefab placement exposes world rotation, positive scale and an optional source pivot")
        brush = source.call("create_block", {"mins": [0, 0, 0], "maxs": [32, 32, 32], "material": "dev/dev_measuregeneric01"})["id"]
        parent = source.call("create_entity", {"classname": "info_target", "origin": [16, 16, 64], "properties": {"targetname": "prefab_anchor"}})["id"]
        child = source.call("create_entity", {"classname": "info_target", "origin": [48, 16, 64], "properties": {"targetname": "prefab_child", "parentname": "prefab_anchor"}})["id"]
        ids = [brush, parent, child]
        note = source.call("begin_context_edit", {"scope": brush})["draft"]["token"]
        source.call("update_context_edit", {"token": note, "summary": "Assembly intent", "body": "Keep the anchor and child together."})
        source.call("apply_context_edit", {"token": note})
        group = source.call("begin_object_group_edit")["token"]
        source.call("update_object_group_edit", {"token": group, "name": "Prefab assembly", "members": ids})
        source.call("apply_object_group_edit", {"token": group})
        layer = source.call("begin_layer_edit", {"snapshot": source.call("get_layers")["snapshot"]})["draft"]
        layer = source.call("update_layer_edit", {"token": layer["token"], "version": layer["version"], "name": "Prefab layer", "color": [40, 160, 220], "members": [brush, child]})["draft"]
        source.call("apply_layer_edit", {"token": layer["token"], "version": layer["version"]})
        source.call("select", {"ids": ids})
        geometry = detail(source, ids)
        history = source.call("get_map_info")["undo"]
        clipboard = source.call("get_clipboard")
        state = report(source)
        check(state["total"] == 0 and not library.exists(), "Read-only library inspection creates no storage")
        saved = source.call("save_prefab", {"name": "Fixture assembly", "snapshot": state["snapshot"]})
        file = library / "Fixture assembly.vmf"
        content = file.read_bytes()
        check(saved["selected"]["objects"] == 3 and saved["selected"]["groups"] == saved["selected"]["layers"] == 1, "Captured assembly retains native groups and layers")
        check(detail(source, ids) == geometry and source.call("get_map_info")["undo"] == history and source.call("get_clipboard") == clipboard, "Saving preserves source geometry, history and clipboard")
        check(b"hammerr_transfer" in content and b"visgroups" in content, "Library files are portable native VMF payloads")
        source.call("save_prefab", {"name": "fixture assembly", "snapshot": saved["snapshot"]}, error=True)
        check(file.read_bytes() == content, "Case-insensitive duplicate save never overwrites a file")
        source.call("save_prefab", {"name": "../escape", "snapshot": report(source)["snapshot"]}, error=True)
        check(not (args.output / "escape.vmf").exists(), "Unsafe prefab names cannot escape library storage")
        stale = report(source)["snapshot"]
        source.call("select", {"ids": []})
        source.call("save_prefab", {"name": "Stale", "ids": ids, "snapshot": stale}, error=True)
        check(not (library / "Stale.vmf").exists(), "Changed selection rejects an old save request")
        source.call("save_prefab", {"name": "Scale fixture", "ids": [brush], "snapshot": report(source)["snapshot"]})
        scale_file = (library / "Scale fixture.vmf").read_bytes()
        transformed = destination.call("preview_prefab", {"name": "Scale fixture", "snapshot": report(destination)["snapshot"], "position": [128, 64, 16], "rotation": [0, 0, 90], "scale": [2, 0.5, 1.25], "pivot": [8, 4, 0]})
        bounds = transformed["paste"]["preview"]["bounds"]
        check(all(abs(actual - expected) < 1e-6 for row, expected_row in zip(bounds, [[114, 48, 16], [130, 112, 56]], strict=True) for actual, expected in zip(row, expected_row, strict=True)), "Scale applies before world rotation and maps the custom source pivot to position")
        check(transformed["prefab"]["placement"]["source_pivot"] == [8, 4, 0], "Placement report identifies the exact source pivot")
        retained = destination.call("get_clipboard")
        for changes in [{"scale": [0, 1, 1]}, {"scale": [-1, 1, 1]}, {"scale": [1e100, 1, 1]}, {"pivot": [70000, 0, 0]}, {"rotation": [70000, 0, 0]}]:
            destination.call("preview_prefab", {"name": "Scale fixture", "snapshot": report(destination)["snapshot"], **changes}, error=True)
            check(destination.call("get_clipboard") == retained, f"Invalid transform retains the owned preview: {changes}")
        destination.call("preview_prefab", {"name": "Fixture assembly", "snapshot": report(destination)["snapshot"], "scale": [2, 2, 2]}, error=True)
        check(destination.call("get_clipboard") == retained, "Point entities refuse nonidentity scale before replacing the clipboard or owned preview")
        source.call("rename_prefab", {"name": "Scale fixture", "new_name": "fixture assembly", "snapshot": report(source)["snapshot"]}, error=True)
        check((library / "Scale fixture.vmf").read_bytes() == scale_file and file.read_bytes() == content, "Rename collision preserves both prefab files byte for byte")
        stale = report(source)["snapshot"]
        source.call("rename_prefab", {"name": "Scale fixture", "new_name": "Renamed scale fixture", "snapshot": stale})
        check(not (library / "Scale fixture.vmf").exists() and (library / "Renamed scale fixture.vmf").read_bytes() == scale_file, "Atomic rename preserves VMF identities and external asset paths")
        source.call("rename_prefab", {"name": "Renamed scale fixture", "new_name": "Outdated rename", "snapshot": stale}, error=True)
        check(not (library / "Outdated rename.vmf").exists(), "An old library snapshot cannot rename the refreshed entry")
        invalidated = destination.call("get_clipboard")
        check(not invalidated["preview"]["valid"] and "renamed" in invalidated["preview"]["stale_reason"], "Another Session's rename explicitly stales the existing Paste review")
        destination.call("apply_paste", {"token": transformed["paste"]["preview"]["token"]}, error=True)
        check(destination.call("get_map_info")["world_brushes"] == 0, "Stale renamed-source placement cannot edit the map")
        transformed = destination.call("preview_prefab", {"name": "Renamed scale fixture", "snapshot": report(destination)["snapshot"], "position": [128, 64, 16], "rotation": [0, 0, 90], "scale": [2, 0.5, 1.25], "pivot": [8, 4, 0]})
        scaled = destination.call("apply_paste", {"token": transformed["paste"]["preview"]["token"]})
        scaled_objects = detail(destination, scaled["objects"])
        check(destination.call("get_map_info")["world_brushes"] == 1 and (library / "Renamed scale fixture.vmf").read_bytes() == scale_file, "Transformed Apply inserts one assembly without changing the prefab VMF")
        destination.call("undo")
        check(destination.call("get_map_info")["world_brushes"] == 0, "One Undo removes the scaled assembly")
        destination.call("redo")
        check(detail(destination, scaled["objects"]) == scaled_objects, "Redo restores exact transformed geometry and generated identities")
        destination.call("undo")
        source.call("delete_prefab", {"name": "Renamed scale fixture", "snapshot": report(source)["snapshot"]})
        spatial = source.call("create_entity", {"classname": "info_target", "origin": [0, 0, 0], "properties": {"movedir": "0 90 0"}})["id"]
        source.call("save_prefab", {"name": "Spatial fixture", "ids": [spatial], "snapshot": report(source)["snapshot"]})
        retained = destination.call("get_clipboard")
        destination.call("preview_prefab", {"name": "Spatial fixture", "snapshot": report(destination)["snapshot"], "rotation": [0, 0, 90]}, error=True)
        check(destination.call("get_clipboard") == retained, "Unsupported directional metadata refuses rotation without changing prior clipboard state")
        source.call("delete_prefab", {"name": "Spatial fixture", "snapshot": report(source)["snapshot"]})
        destination.call("create_entity", {"classname": "info_target", "origin": [-128, 0, 16], "properties": {"targetname": "prefab_anchor"}})
        inspected = report(destination, query="fixture", selected="Fixture assembly")
        check(inspected["total"] == 1 and inspected["selected"]["objects"] == 3, "Independent sessions search and inspect the same library")
        previous = destination.call("get_map_info")
        target = [128, 64, 64]
        preview = destination.call("preview_prefab", {"name": "Fixture assembly", "position": target, "snapshot": inspected["snapshot"]})
        token = preview["paste"]["preview"]["token"]
        bounds = preview["paste"]["preview"]["bounds"]
        check(all(abs((a + b) * 0.5 - center) < 1e-6 for a, b, center in zip(bounds[0], bounds[1], target, strict=True)), "Placement centers the authored bounds at exact world coordinates")
        check(destination.call("get_map_info")["undo"] == previous["undo"] and destination.call("get_map_info")["world_brushes"] == previous["world_brushes"], "Prefab preview preserves map geometry and history")
        destination.call("cancel_paste", {"token": token})
        check(destination.call("get_map_info")["world_brushes"] == 0, "Shared Cancel discards prefab placement")
        preview = destination.call("preview_prefab", {"name": "Fixture assembly", "position": target, "rotation": [0, 0, 90], "pivot": [16, 16, 16], "snapshot": report(destination)["snapshot"]})
        applied = destination.call("apply_paste", {"token": preview["paste"]["preview"]["token"]})
        objects = detail(destination, applied["objects"])
        properties = [dict(obj.get("properties", [])) for obj in objects]
        anchor = next(row["targetname"] for row in properties if row.get("targetname", "").startswith("prefab_anchor_copy"))
        check(any(row.get("parentname") == anchor for row in properties), "Prefab internal links follow generated names across destination collisions")
        check(destination.call("get_layers")["total"] == 1 and len(applied["visgroup_ids"]) == 1, "Placement uses fresh native layer identities")
        check(destination.call("get_context_note", {"scope": applied["objects"][0]})["note"]["summary"] == "Assembly intent", "Assembly context follows the pasted brush")
        check(destination.call("get_map_info")["undo"][0] == "paste objects", "Apply records one shared paste Undo step")
        destination.call("undo")
        check(destination.call("get_map_info")["world_brushes"] == 0, "Undo removes the complete assembly")
        destination.call("redo")
        check(detail(destination, applied["objects"]) == objects, "Redo restores exact prefab objects and IDs")
        path = args.output / "prefab-map.vmf"
        destination.call("save_map", {"path": str(path)})
        destination.call("open_map", {"path": str(path), "discard_changes": True})
        check(detail(destination, applied["objects"]) == objects, "Native save/reopen retains the placed assembly")
        restarted = launch(args.binary, args.output / "restart", library)
        check(report(restarted, selected="Fixture assembly")["selected"]["objects"] == 3, "Library persists across editor restart")
        preview = restarted.call("preview_prefab", {"name": "Fixture assembly", "snapshot": report(restarted)["snapshot"], "position": [0, 0, 64]})
        restarted.call("delete_prefab", {"name": "Fixture assembly", "snapshot": report(restarted)["snapshot"]})
        check(not file.exists(), "Explicit deletion removes only the library VMF")
        restarted.call("apply_paste", {"token": preview["paste"]["preview"]["token"]})
        check(restarted.call("get_map_info")["world_brushes"] == 1, "Detached preview remains usable after deleting its library file")
        before = restarted.call("get_clipboard")
        (library / "Broken.vmf").write_text("not a VMF")
        broken = report(restarted, selected="Broken")
        check("selection_error" in broken and broken["total"] == 1, "Malformed VMFs remain listed with an inspection error")
        restarted.call("preview_prefab", {"name": "Broken", "snapshot": broken["snapshot"]}, error=True)
        check(restarted.call("get_clipboard") == before, "Failed load preserves the prior clipboard and owned preview")
        restarted.call("delete_prefab", {"name": "Broken", "snapshot": report(restarted)["snapshot"]})
        check(report(restarted)["total"] == 0, "Malformed entries can be removed without editing the map")
        result = {"passed": len(checks), "checks": checks, "desktop_input": False}
        (args.output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps({"passed": len(checks), "output": str(args.output)}))
    finally:
        source.close()
        destination.close()
        if restarted:
            restarted.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.binary = args.binary.resolve()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True)
    validate(args)


if __name__ == "__main__":
    main()
