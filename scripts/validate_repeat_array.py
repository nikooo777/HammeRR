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
        check("space" in tools["preview_repeat_array"]["properties"], "array space is exposed in MCP")
        check("scale" in tools["preview_repeat_array"]["properties"], "array scale factors are exposed in MCP")
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

        local_parent = client.call("create_entity", {"classname": "info_target", "origin": [10, 0, 7], "properties": {"targetname": "local_parent", "angles": "0 37 0"}})["id"]
        local_child = client.call("create_entity", {"classname": "info_target", "origin": [3, -6, 9], "properties": {"targetname": "local_child", "parentname": "local_parent,attachment", "angles": "0 90 0"}})["id"]
        local_sources = [local_child, local_parent]
        client.call("select", {"ids": local_sources})
        local_original = detail(client, local_sources)
        local_history = client.call("get_map_info")["undo"]
        local_camera = client.call("get_camera")
        local_arguments = {"ids": local_sources, "count": 3, "translation": [4.5, -2.25, 1.125], "rotation": [90, 0, 0], "pivot": [0, 0, 0]}
        local = preview(client, space="local", **local_arguments)
        check(local["preview"]["space"] == "local" and local["preview"]["frame_source"] == local_parent, "local array uses lowest-ID original source despite request order")
        angle = math.radians(37)
        cosine, sine = math.cos(angle), math.sin(angle)
        expected_basis = [[cosine, sine, 0], [-sine, cosine, 0], [0, 0, 1]]
        check(all(math.isclose(a, b, abs_tol=1e-12) for actual, expected in zip(local["preview"]["basis"], expected_basis) for a, b in zip(actual, expected)), "reported source frame matches original entity orientation")
        entities = {entity["id"]: entity for entity in local["preview"]["entities"]}
        for copy in local["preview"]["copies"]:
            index = copy["index"]
            for copied_id, point in zip(copy["objects"], [[3, -6, 9], [10, 0, 7]]):
                x, y, z = point
                lx, ly = cosine * x + sine * y, -sine * x + cosine * y
                rx, ry, rz = {1: (lx, -z, ly), 2: (lx, -ly, -z), 3: (lx, z, -ly)}[index]
                rx += 4.5 * index
                ry -= 2.25 * index
                rz += 1.125 * index
                expected = [cosine * rx - sine * ry, sine * rx + cosine * ry, rz]
                check(all(math.isclose(a, b, abs_tol=1e-8) for a, b in zip(entities[copied_id]["origin"], expected)), f"local copy {index} uses fixed original axes and pivot")
            copied_child = entities[copy["objects"][0]]
            check(dict(copied_child["properties"])["parentname"] == f"local_parent_copy{index},attachment" and copy["remapped_references"] == 1, f"local copy {index} keeps internal parent remapping")
        check(detail(client, local_sources) == local_original and client.call("get_map_info")["undo"] == local_history, "local preview preserves committed originals and history")
        check(client.call("get_camera") == local_camera, "local preview preserves camera")
        client.render(args.output / "repeat-array-local.png", camera=render_camera)
        for space in ["source", "LOCAL", ""]:
            client.call("preview_repeat_array", {"snapshot": local["snapshot"], "space": space, **local_arguments}, error=True)
            check(client.call("get_repeat_array") == local, "invalid space preserves valid local draft")
        world_default = preview(client, **local_arguments)
        world_explicit = preview(client, space="world", **local_arguments)
        check(world_default["preview"]["space"] == "world" and world_explicit["preview"]["frame_source"] is None, "world remains the default mode")
        check(all(world_default["preview"][key] == world_explicit["preview"][key] for key in ["copies", "entities", "bounds", "basis"]), "explicit and default world previews are exact")
        client.call("cancel_repeat_array", {"token": local["preview"]["token"]}, error=True)
        check(client.call("get_repeat_array") == world_explicit, "old local token cannot discard replacement world draft")
        local = preview(client, space="local", **local_arguments)
        local_applied = client.call("apply_repeat_array", {"token": local["preview"]["token"]})
        local_copies = detail(client, local_applied["objects"])
        check(detail(client, local_sources) == local_original and len(local_copies) == 6, "local Apply retains originals and creates exact preview copies")
        check(client.call("get_map_info")["undo"] == ["repeat array", *local_history], "local Apply records one Undo step")
        client.call("undo")
        check(detail(client, local_sources) == local_original and client.call("get_map_info")["undo"] == local_history, "local Undo removes the complete array")
        client.call("redo")
        check(detail(client, local_applied["objects"]) == local_copies, "local Redo restores complete array")
        local_path = args.output / "repeat-array-local.vmf"
        client.call("save_map", {"path": str(local_path)})
        client.call("open_map", {"path": str(local_path), "discard_changes": True})
        check(detail(client, local_applied["objects"]) == local_copies, "local array survives save and reopen")
        stale = preview(client, ids=[local_parent], space="local", count=1, translation=[8, 0, 0])
        client.call("set_entity_properties", {"id": local_parent, "set": {"angles": "0 61 0"}})
        client.call("apply_repeat_array", {"token": stale["preview"]["token"]}, error=True)
        check(client.call("get_repeat_array")["preview"] is None, "changed source orientation prevents stale local Apply")

        scale_source = client.call("create_block", {"mins": [3.125, -7.25, 2.5], "maxs": [19.5, 3.75, 13.625], "material": "dev/dev_measuregeneric01"})["id"]
        client.call("transform_objects", {"ids": [scale_source], "rotation": [17, 29, -11], "pivot": [0, 0, 0]})
        client.call("select", {"ids": [scale_source]})
        scale_original = detail(client, [scale_source])[0]
        scale_history = client.call("get_map_info")["undo"]
        scale_camera = client.call("get_camera")
        scale_arguments = {"ids": [scale_source], "count": 3, "space": "local", "scale": [2, 0.5, 1.25], "rotation": [90, 0, 0], "translation": [4.5, -1.25, 2], "pivot": [-5.25, 8.125, 1.5]}
        scaled = preview(client, **scale_arguments)
        check(scaled["preview"]["scale"] == [2, 0.5, 1.25], "scale preview reports per-copy factors")
        for copy in scaled["preview"]["copies"]:
            check(all(math.isclose(actual, factor ** copy["index"], abs_tol=1e-12) for actual, factor in zip(copy["scale"], scale_arguments["scale"])), f"copy {copy['index']} reports factor to the copy number")
        check(detail(client, [scale_source])[0] == scale_original and client.call("get_map_info")["undo"] == scale_history and client.call("get_camera") == scale_camera, "scale preview preserves committed source history and camera")
        client.render(args.output / "repeat-array-scale.png", camera=render_camera)
        for changes in [
            {"scale": [0, 1, 1]}, {"scale": [-1, 1, 1]}, {"scale": [1e300, 1, 1]},
            {"scale": [1e-300, 1, 1]}, {"scale": [1000, 1000, 1000]},
            {"ids": [scale_source, local_parent]}, {"scale": [1, 2]},
        ]:
            client.call("preview_repeat_array", {"snapshot": scaled["snapshot"], **scale_arguments, **changes}, error=True)
            check(client.call("get_repeat_array") == scaled, "invalid or unsupported scale preserves valid draft")
        applied_scale = client.call("apply_repeat_array", {"token": scaled["preview"]["token"]})
        scale_copies = detail(client, applied_scale["objects"])
        basis = scaled["preview"]["basis"]
        pivot = scale_arguments["pivot"]
        for copied, copy in zip(scale_copies, scaled["preview"]["copies"]):
            index = copy["index"]
            for before, after in zip(scale_original["faces"], copied["faces"]):
                for point, actual in zip(before["plane"], after["plane"]):
                    relative = [value - origin for value, origin in zip(point, pivot)]
                    local_point = [sum(column[axis] * relative[axis] for axis in range(3)) * factor ** index for column, factor in zip(basis, scale_arguments["scale"])]
                    x, y, z = local_point
                    rotated = {1: [x, -z, y], 2: [x, -y, -z], 3: [x, z, -y]}[index]
                    moved = [value + step * index for value, step in zip(rotated, scale_arguments["translation"])]
                    expected = [pivot[axis] + sum(basis[column][axis] * moved[column] for column in range(3)) for axis in range(3)]
                    check(all(math.isclose(a, b, abs_tol=1e-8) for a, b in zip(actual, expected)), f"scaled copy {index} starts from immutable source and scales before local rotation")
        check(detail(client, [scale_source])[0] == scale_original and client.call("get_map_info")["undo"] == ["repeat array", *scale_history], "scale Apply preserves original and adds one Undo step")
        client.call("undo")
        check(detail(client, [scale_source])[0] == scale_original and client.call("get_map_info")["undo"] == scale_history, "scale Undo removes whole array")
        client.call("redo")
        check(detail(client, applied_scale["objects"]) == scale_copies, "scale Redo restores all geometry")
        scale_path = args.output / "repeat-array-scale.vmf"
        client.call("save_map", {"path": str(scale_path)})
        client.call("open_map", {"path": str(scale_path), "discard_changes": True})
        check(detail(client, applied_scale["objects"]) == scale_copies, "scaled geometry survives save and reopen")
        default_arguments = {"ids": [scale_source], "count": 2, "space": "local", "translation": [16, 0, 0], "rotation": [0, 0, 15]}
        implicit_scale = preview(client, **default_arguments)
        identity_scale = preview(client, scale=[1, 1, 1], **default_arguments)
        check(implicit_scale["preview"]["scale"] == [1, 1, 1] and all(implicit_scale["preview"][key] == identity_scale["preview"][key] for key in ["copies", "entities", "bounds", "basis"]), "omitted scale is exactly identity scale")
        scale_only = preview(client, ids=[scale_source], count=2, scale=[1.1, 1.1, 1.1])
        check(scale_only["preview"]["valid"] and all(math.isclose(value, 1.21, abs_tol=1e-12) for value in scale_only["preview"]["copies"][1]["scale"]), "scale-only array accepts ten percent growth per copy")
        client.call("cancel_repeat_array", {"token": scale_only["preview"]["token"]})
        check(detail(client, [scale_source])[0] == scale_original, "scale-only Cancel preserves source")
        results = {"checks": checks, "count": len(checks), "preview": replacement, "local_preview": local, "scale_preview": scaled}
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
