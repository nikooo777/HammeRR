import argparse
import json
import math
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client, detail


def preview(client, **arguments):
    state = client.call("get_repeat_array")
    return client.call("preview_repeat_array", {"snapshot": state["snapshot"], **arguments})


def validate(args):
    client = Client(None, args.binary, args.output)
    checks = []

    def check(condition, name):
        assert condition, name
        checks.append(name)

    try:
        tools = {tool["name"]: tool["inputSchema"] for tool in client.rpc("tools/list", {})["tools"]}
        check(all(name in tools for name in ["get_repeat_array", "preview_repeat_array", "apply_repeat_array", "cancel_repeat_array"]), "shared array tools")
        check(tools["preview_repeat_array"].get("additionalProperties") is False, "unknown preview fields rejected")
        brush = client.call("create_block", {"mins": [-16, -16, 0], "maxs": [16, 16, 24], "material": "dev/dev_measuregeneric01"})["id"]
        parent = client.call("create_entity", {"classname": "info_target", "origin": [0, 0, 32], "properties": {"targetname": "array_parent"}})["id"]
        child = client.call("create_entity", {"classname": "info_target", "origin": [32, 0, 32], "properties": {"targetname": "array_child", "parentname": "array_parent,attachment"}})["id"]
        draft = client.call("begin_connection_edit", {"id": child})
        client.call("update_connection_draft", {"token": draft["token"], "output": "OnUser1", "target": "array_parent", "input": "SetParent", "parameter": "array_parent", "delay": "0.00", "times": "-1"})
        client.call("commit_connection_edit", {"token": draft["token"]})
        note = client.call("begin_context_edit", {"scope": brush})
        client.call("update_context_edit", {"token": note["draft"]["token"], "summary": "Array source", "body": "Keep this geometry and copied authorship."})
        client.call("apply_context_edit", {"token": note["draft"]["token"]})
        sources = [brush, parent, child]
        client.call("select", {"ids": sources})
        original = detail(client, sources)
        history = client.call("get_map_info")["undo"]
        camera = client.call("get_camera")
        render_camera = {"position": [380, -440, 300], "look_at": [90, 25, 24]}
        before_hash = client.render(args.output / "before.png", camera=render_camera)
        report = preview(client, count=3, translation=[64, 0, 0], rotation=[0, 0, 90], pivot=[0, 0, 0])
        draft = report["preview"]
        check(draft["valid"] and draft["object_count"] == 9, "nine preview objects")
        check(detail(client, sources) == original and client.call("get_map_info")["undo"] == history, "preview leaves source and history unchanged")
        check(client.call("get_camera") == camera, "preview preserves camera")
        check(client.call("get_map_info")["repeat_array"] == report, "map info exposes same draft")
        entities = {entity["id"]: entity for entity in draft["entities"]}
        expected_positions = [[64, 32, 32], [96, 0, 32], [192, -32, 32]]
        for copy, expected in zip(draft["copies"], expected_positions):
            copied_child = entities[copy["objects"][2]]
            properties = dict(copied_child["properties"])
            index = copy["index"]
            check(all(math.isclose(a, b, abs_tol=1e-8) for a, b in zip(copied_child["origin"], expected)), f"copy {index} fixed-pivot transform from source")
            check(properties["parentname"] == f"array_parent_copy{index},attachment", f"copy {index} parent suffix")
            check(copied_child["connections"][0]["value"].split(",")[:3] == [f"array_parent_copy{index}", "SetParent", f"array_parent_copy{index}"], f"copy {index} target and input parameter remapping")
            check(copy["remapped_references"] == 3, f"copy {index} exact internal reference count")
        array_hash = client.render(args.output / "repeat-array.png", camera=render_camera)
        check(array_hash != before_hash, "render_view displays new copies")
        for arguments in [
            {"count": 0}, {"count": 65}, {"count": 2, "ids": [brush, brush]},
            {"count": 2, "translation": [50000, 0, 0]}, {"count": 2, "translation": [1, 0, 0], "snapshot": "old"},
            {"count": 2, "translation": [1, 0, 0], "unknown": True},
        ]:
            state = client.call("get_repeat_array")
            client.call("preview_repeat_array", {"snapshot": state["snapshot"], **arguments}, error=True)
            check(client.call("get_repeat_array") == report, "invalid update preserves last valid draft")
        client.call("cancel_repeat_array", {"token": draft["token"]})
        check(client.render(args.output / "cancelled.png", camera=render_camera) == before_hash, "cancel restores source rendering")
        check(detail(client, sources) == original and client.call("get_map_info")["undo"] == history, "cancel preserves source and history")

        array = preview(client, count=2, translation=[48, 0, 0])
        client.call("transform_objects", {"translation": [0, 10, 0], "preview": True})
        foreign = client.call("get_transform_handles")
        client.call("cancel_repeat_array", {"token": array["preview"]["token"]}, error=True)
        check(client.call("get_transform_handles") == foreign, "old array Cancel preserves numeric draft")
        client.call("cancel_transform_preview")
        array = preview(client, count=2, translation=[48, 0, 0])
        client.call("create_primitive", {"shape": "wedge", "mins": [-32, -32, 0], "maxs": [32, 32, 64], "preview": True})
        foreign = client.call("get_primitive_preview")
        client.call("cancel_repeat_array", {"token": array["preview"]["token"]}, error=True)
        check(client.call("get_primitive_preview") == foreign, "old array Cancel preserves primitive draft")
        client.call("cancel_primitive_preview", {"token": foreign["preview"]["token"]})

        report = preview(client, count=3, translation=[64, 0, 0], rotation=[0, 0, 90], pivot=[0, 0, 0])
        token = report["preview"]["token"]
        replacement = preview(client, count=3, translation=[64, 0, 0], rotation=[0, 0, 90], pivot=[0, 0, 0])
        client.call("apply_repeat_array", {"token": token}, error=True)
        client.call("cancel_repeat_array", {"token": token}, error=True)
        applied = client.call("apply_repeat_array", {"token": replacement["preview"]["token"]})
        copies = applied["objects"]
        check(len(copies) == 9 and len(set(sources + copies)) == 12, "collision-free copied IDs")
        check(client.call("get_map_info")["undo"][0] == "repeat array", "one atomic array Undo label")
        check(detail(client, sources) == original, "Apply retains original objects")
        actual = detail(client, copies)
        for copy in replacement["preview"]["copies"]:
            copied_brush = copy["objects"][0]
            note = client.call("get_context_note", {"scope": copied_brush})
            check(note["note"]["summary"] == "Array source" and note["note"]["author"] == "agent", "copied note retains content and author")
        client.call("undo")
        check(detail(client, sources) == original and client.call("get_map_info")["world_brushes"] == 1, "Undo removes entire array")
        client.call("redo")
        check(detail(client, copies) == actual, "Redo restores complete array")
        path = args.output / "repeat-array.vmf"
        client.call("save_map", {"path": str(path)})
        client.call("open_map", {"path": str(path), "discard_changes": True})
        check(detail(client, copies) == actual, "VMF reopen retains all copies")
        report = preview(client, ids=[parent], count=1, translation=[16, 0, 0])
        client.call("select", {"ids": [child]})
        client.call("apply_repeat_array", {"token": report["preview"]["token"]}, error=True)
        check(client.call("get_repeat_array")["preview"] is None, "selection change invalidates array")
        results = {"checks": checks, "count": len(checks), "preview": replacement}
        (args.output / "validation.json").write_text(json.dumps(results, indent=2) + "\n")
        return len(checks)
    finally:
        client.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, default=Path("/tmp/hammerr-repeat-array"))
    args = parser.parse_args()
    args.binary = args.binary.resolve()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    count = validate(args)
    print(json.dumps({"repeat_array": True, "checks": count, "output": str(args.output)}))


if __name__ == "__main__":
    main()
