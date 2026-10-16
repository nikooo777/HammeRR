import argparse
import json
import os
from pathlib import Path
from unittest.mock import patch

from validate_face_lighting import DEFAULT_BINARY, Client, detail

FGD = '''
@BaseClass = PathBase [
    targetname(target_source) : "Name"
    target(target_destination) : "Next" : "fgd_should_be_replaced"
    wait(float) : "Wait" : "2.5"
]
@PointClass base(PathBase) = path_corner [
    speed(float) : "Speed" : "80"
]
@PointClass base(PathBase) = path_track [
    orientationtype(choices) : "Orientation" : 1 = [ 0 : "None" 1 : "Face" ]
]
@PointClass = info_target [
    targetname(target_source) : "Name"
    target(target_destination) : "Target"
]
'''
POINTS = [[0.125, -16.25, 32.5], [64.5, 48.25, 48.125], [128.75, -8.5, 64.25]]


def state(client):
    return client.call("get_linked_path")


def preview(client, **changes):
    return client.call(
        "set_linked_path",
        {"action": "preview", "snapshot": state(client)["snapshot"], **changes},
    )


def reject(client, arguments):
    try:
        client.call("set_linked_path", arguments, error=True)
    except RuntimeError as error:
        if not (
            error.args
            and isinstance(error.args[0], dict)
            and error.args[0].get("code") == -32602
        ):
            raise


def snapshot(client, ids):
    info = client.call("get_map_info")
    return {
        "objects": detail(client, ids),
        **{
            key: info[key]
            for key in [
                "undo", "redo", "unsaved_changes", "selection",
                "world_brushes", "point_entities", "brush_entities",
            ]
        },
        "camera": client.call("get_camera"),
    }


def validate_membership_changes(client, args, check, existing):
    for classname in ["path_corner", "path_track"]:
        for closed in [False, True]:
            label = f"{classname} {'closed' if closed else 'open'} membership"
            prefix = f"membership_{classname}_{int(closed)}"
            created = preview(client, classname=classname, prefix=prefix, points=POINTS, closed=closed)["preview"]
            original_ids = client.call("set_linked_path", {"action": "apply", "token": created["token"]})["objects"]
            existing.extend(original_ids)
            names = [node["targetname"] for node in created["nodes"]]
            client.call("set_entity_properties", {"id": original_ids[1], "set": {"custom_intent": "retain exact identity", "wait": "3.7500", "spawnflags": "1025"}})
            watcher = client.call("create_entity", {"classname": "info_target", "origin": [0, -256, 0], "properties": {"targetname": prefix + "_watcher", "target": names[0]}})["id"]
            existing.append(watcher)
            inspected = client.call("set_linked_path", {"action": "inspect", "id": original_ids[0]})
            requested = [
                {"id": original_ids[1], "origin": POINTS[1]},
                {"origin": [192.125, 32.5, 48.25]},
                {"id": original_ids[2], "origin": POINTS[2]},
                {"origin": [256.125, 64.5, 64.25]},
            ]
            arguments = {"action": "edit", "seed": inspected["seed"], "prefix": prefix + "_insert", "nodes": requested, "snapshot": inspected["snapshot"]}
            before = snapshot(client, existing)
            reject(client, arguments)
            check(snapshot(client, existing) == before and state(client)["preview"] is None, f"{label}: outside reference blocks head deletion without source/history changes")
            client.call("set_entity_properties", {"id": watcher, "set": {"target": names[1]}})
            connection = client.call("begin_connection_edit", {"id": original_ids[2]})
            client.call("update_connection_draft", {"token": connection["token"], "output": "OnUser1", "target": prefix + "_watcher", "input": "Use", "parameter": "", "delay": "0.25", "times": "-1"})
            client.call("commit_connection_edit", {"token": connection["token"]})
            inspected = client.call("set_linked_path", {"action": "inspect", "id": original_ids[0]})
            arguments["snapshot"] = inspected["snapshot"]
            before = snapshot(client, existing)
            draft = client.call("set_linked_path", arguments)
            planned = draft["preview"]["nodes"]
            result_ids = [node["id"] for node in planned]
            added_ids = [node["id"] for node in planned if node["source_id"] is None]
            check([node["source_id"] for node in planned] == [original_ids[1], None, original_ids[2], None], f"{label}: draft distinguishes retained source IDs and inserted nodes")
            check(draft["preview"]["seed"] == original_ids[0] and draft["preview"]["removed"] == [{"id": original_ids[0], "targetname": names[0]}], f"{label}: original seed and exact removed head identity remain reviewable")
            check(len(added_ids) == 2 and not set(added_ids) & set(existing), f"{label}: only inserted nodes receive fresh IDs")
            check(snapshot(client, existing) == before, f"{label}: membership preview preserves source, selection, camera and history")
            blocked = {**arguments, "nodes": requested[:-2] + [requested[-1]], "snapshot": draft["snapshot"]}
            reject(client, blocked)
            check(state(client) == draft and snapshot(client, existing) == before, f"{label}: deleting a node with connections preserves the reviewed draft")
            repeat = client.call("set_linked_path", {**arguments, "snapshot": draft["snapshot"]})
            check(repeat["preview"]["nodes"] == planned, f"{label}: repeated preview preserves exact planned identities")
            client.call("set_linked_path", {"action": "cancel", "token": repeat["preview"]["token"]})
            check(snapshot(client, existing) == before, f"{label}: Cancel leaves all committed identities intact")
            draft = client.call("set_linked_path", {**arguments, "snapshot": state(client)["snapshot"]})["preview"]
            client.call("set_linked_path", {"action": "apply", "token": draft["token"]})
            after_ids = [node_id for node_id in existing if node_id != original_ids[0]] + added_ids
            actual = detail(client, result_ids)
            retained_before = {node["id"]: node for node in before["objects"] if node["id"] in original_ids[1:]}
            for node in actual:
                properties = dict(node["properties"])
                if node["id"] in retained_before:
                    original = retained_before[node["id"]]
                    check({key: value for key, value in properties.items() if key not in ["origin", "target"]} == {key: value for key, value in dict(original["properties"]).items() if key not in ["origin", "target"]} and node["connections"] == original["connections"], f"{label}: retained {node['id']} keeps authored properties and I/O")
                else:
                    check(properties["wait"] == "2.5" and "custom_intent" not in properties, f"{label}: inserted {node['id']} receives FGD defaults only")
            for index, node in enumerate(actual):
                target = planned[index + 1]["targetname"] if index + 1 < len(planned) else planned[0]["targetname"] if closed else None
                check(dict(node["properties"]).get("target") == target, f"{label}: result node {index} has repaired internal next link")
            after = detail(client, after_ids)
            check(len(client.call("get_map_info")["undo"]) == len(before["undo"]) + 1, f"{label}: membership edit creates one Undo step")
            client.call("undo")
            check(detail(client, existing) == before["objects"], f"{label}: Undo restores removed head and original identities, links and I/O")
            client.call("redo")
            check(detail(client, after_ids) == after, f"{label}: Redo restores fresh inserted identities and repaired links")
            existing[:] = after_ids
            path = args.output / f"{prefix}.vmf"
            client.call("save_map", {"path": str(path)})
            client.call("open_map", {"path": str(path)})
            check(detail(client, existing) == after, f"{label}: membership changes survive save/reopen with exact identities")


def validate(args):
    checks, cases = [], []
    fgd = args.output / "linked-paths.fgd"
    fgd.write_text(FGD)
    with patch.dict(
        os.environ,
        {
            "HAMMERR_RECENT_MAPS_DIR": str(args.output / "recent-maps"),
            "HAMMERR_BUILD_RECIPES": str(args.output / "build-recipes.json"),
        },
    ):
        client = Client(None, args.binary, args.output)

    def check(condition, label):
        assert condition, label
        checks.append(label)

    try:
        tools = {tool["name"]: tool for tool in client.rpc("tools/list", {})["tools"]}
        check({"get_linked_path", "set_linked_path"} <= tools.keys(), "Linked-path MCP tools are discoverable")
        schema = json.dumps(tools["set_linked_path"]["inputSchema"])
        check(all(f'"{action}"' in schema for action in ["preview", "review", "apply", "cancel", "inspect", "edit"]), "Tagged actions are discoverable")
        check('"additionalProperties": false' in schema, "Linked-path actions reject undeclared fields")
        client.call("load_entity_definitions", {"path": str(fgd)})
        initial = state(client)
        check(initial["preview"] is None and set(initial["classes"]) == {"path_corner", "path_track"}, "Initial state reports both supported classes without a draft")
        check(initial["limits"] == {"min_points": 2, "max_points": 64, "prefix_bytes": 96, "coordinate_extent": 65536}, "Bounds are exposed")
        existing = [
            client.call("create_entity", {"classname": "info_target", "origin": [index * 16, -64, 0], "properties": properties})["id"]
            for index, properties in enumerate([
                {"targetname": "route_001"},
                {"targetname": "ROUTE_002"},
                {"targetname": "external", "target": "blocked_001"},
            ])
        ]
        client.call("set_object_visibility", {"action": "hide", "ids": [existing[1]]})
        before = snapshot(client, existing)
        valid = preview(client, classname="path_corner", prefix="route", points=POINTS)
        check(valid["preview"]["valid"] and valid["preview"]["owner"] == "agent", "Agent owns a valid preview")
        check(snapshot(client, existing) == before, "Preview preserves source, selection, camera and history")
        for changes in [
            {"points": []}, {"points": POINTS[:1]},
            {"points": [[index, 0, 0] for index in range(65)]},
            {"points": [POINTS[0], POINTS[0]]},
            {"points": [*POINTS, POINTS[0]], "closed": True},
            {"points": [[65537, 0, 0], [0, 0, 0]]},
            {"points": [[1e300, 0, 0], [0, 0, 0]]},
            {"prefix": ""}, {"prefix": "a" * 97}, {"prefix": "1route"},
            {"prefix": "bad name"}, {"prefix": "bad/name"}, {"prefix": "páth"},
            {"prefix": "blocked"}, {"classname": "func_train"},
            {"snapshot": "stale-snapshot"}, {"unexpected": True},
        ]:
            reject(client, {"action": "preview", "snapshot": valid["snapshot"], "classname": "path_corner", "prefix": "route", "points": POINTS, **changes})
            check(state(client) == valid and snapshot(client, existing) == before, f"Invalid update preserves the valid draft: {changes}")
        reviewed = client.call("set_linked_path", {"action": "review", "token": valid["preview"]["token"]})
        check(reviewed["preview"]["token"] != valid["preview"]["token"] and reviewed["preview"]["nodes"] == valid["preview"]["nodes"], "Review issues a fresh token for the same planned nodes")
        for action in ["apply", "cancel"]:
            reject(client, {"action": action, "token": valid["preview"]["token"]})
            check(state(client) == reviewed, f"Superseded token cannot {action} the reviewed draft")
        client.call("set_linked_path", {"action": "cancel", "token": reviewed["preview"]["token"]})
        check(state(client)["preview"] is None and snapshot(client, existing) == before, "Cancel discards only the draft without source edits")

        for classname, closed, prefix, points in [
            ("path_corner", False, "route", POINTS),
            ("path_corner", True, "corner_loop", POINTS),
            ("path_track", False, "track_open", POINTS),
            ("path_track", True, "track_loop", [[index * 8 + 0.125, (index % 7) * 12.5, 16.25] for index in range(64)]),
        ]:
            label = f"{classname} {'closed' if closed else 'open'}"
            before = snapshot(client, existing)
            draft = preview(client, classname=classname, prefix=prefix, points=points, closed=closed)["preview"]
            nodes = draft["nodes"]
            ids = [node["id"] for node in nodes]
            names = [f"{prefix}_{index + 1:03d}" for index in range(len(points))]
            if prefix == "route":
                names[:2] = ["route_001_2", "route_002_2"]
            targets = [*names[1:], names[0] if closed else None]
            check([node["targetname"] for node in nodes] == names, f"{label}: deterministic names avoid occupied and hidden case-insensitive names")
            check([node["origin"] for node in nodes] == points and [node["target"] for node in nodes] == targets, f"{label}: preview preserves point order and exact next links")
            check(snapshot(client, existing) == before, f"{label}: preview leaves committed data fixed")
            applied = client.call("set_linked_path", {"action": "apply", "token": draft["token"]})
            check(applied["objects"] == ids and applied["state"]["preview"] is None, f"{label}: Apply uses the planned IDs and clears its draft")
            after = client.call("get_map_info")
            check(set(after["selection"]) == set(ids), f"{label}: Apply selects all new nodes")
            check(len(after["undo"]) == len(before["undo"]) + 1 and after["undo"][1:] == before["undo"], f"{label}: Apply adds one Undo step")
            actual = detail(client, ids)
            for node, origin, name, target in zip(actual, points, names, targets, strict=True):
                properties = dict(node["properties"])
                check(node["classname"] == classname and node["origin"] == origin and properties["targetname"] == name, f"{label}: created node {name} matches its preview")
                check(properties["wait"] == "2.5" and properties["speed" if classname == "path_corner" else "orientationtype"] == ("80" if classname == "path_corner" else "1"), f"{label}: {name} retains inherited and class defaults")
                check(properties.get("target") == target and (target is not None or "target" not in properties), f"{label}: {name} has the exact target or an omitted open endpoint")
            check(detail(client, existing) == before["objects"], f"{label}: existing entities and references stay fixed")
            client.call("undo")
            undone = client.call("get_map_info")
            check(undone["undo"] == before["undo"] and undone["point_entities"] == before["point_entities"] and detail(client, existing) == before["objects"], f"{label}: one Undo removes every new node")
            client.call("redo")
            check(detail(client, ids) == actual, f"{label}: Redo restores IDs, links and defaults")
            existing.extend(ids)
            saved = detail(client, existing)
            path = args.output / f"{prefix}.vmf"
            client.call("save_map", {"path": str(path)})
            saved_bytes = path.read_bytes()
            client.call("open_map", {"path": str(path)})
            check(detail(client, existing) == saved and path.read_bytes() == saved_bytes, f"{label}: saved/reopened VMF retains all IDs, links, coordinates and defaults")
            cases.append({"classname": classname, "closed": closed, "points": len(points), "objects": ids, "names": names, "targets": targets, "vmf": str(path)})

        route = cases[0]
        ids = route["objects"]
        client.call("set_entity_properties", {"id": ids[1], "set": {"custom_intent": "retain this node", "wait": "3.75"}})
        watcher = client.call("create_entity", {"classname": "info_target", "origin": [0, -128, 0], "properties": {"target": route["names"][0]}})["id"]
        existing.append(watcher)
        before = snapshot(client, existing)
        inspected = client.call("set_linked_path", {"action": "inspect", "id": ids[1]})
        nodes = [{"id": node["id"], "origin": node["origin"]} for node in inspected["nodes"]]
        nodes = [nodes[2], nodes[0], nodes[1]]
        nodes[2]["origin"][2] += 8.5
        edited = client.call("set_linked_path", {"action": "edit", "nodes": nodes, "snapshot": inspected["snapshot"]})["preview"]
        check(edited["existing"] and [node["id"] for node in edited["nodes"]] == [node["id"] for node in nodes], "Existing-chain preview retains IDs in the requested order")
        check(snapshot(client, existing) == before, "Inspecting and previewing an existing chain preserves committed source/history")
        client.call("set_linked_path", {"action": "apply", "token": edited["token"]})
        after = detail(client, existing)
        current = {node["id"]: dict(node["properties"]) for node in detail(client, ids)}
        check(current[ids[1]]["custom_intent"] == "retain this node" and current[ids[1]]["wait"] == "3.75", "Existing edits preserve custom properties and authored defaults")
        check(dict(detail(client, [watcher])[0]["properties"])["target"] == route["names"][0], "Outside entity references retain their original named node")
        check(len(client.call("get_map_info")["undo"]) == len(before["undo"]) + 1, "Reordering and moving existing nodes adds one Undo step")
        client.call("undo")
        check(detail(client, existing) == before["objects"], "Undo restores exact existing objects, identities and references")
        client.call("redo")
        check(detail(client, existing) == after, "Redo restores the exact edited chain")
        path = args.output / "edited-existing.vmf"
        client.call("save_map", {"path": str(path)})
        client.call("open_map", {"path": str(path)})
        check(detail(client, existing) == after, "Edited existing identities, links and metadata survive save/reopen")

        validate_membership_changes(client, args, check, existing)

        bounded = preview(client, classname="path_corner", prefix="_" + "a" * 95, points=[[-65536, 0, 0], [65536, 0, 0]])
        check(bounded["preview"]["valid"], "Inclusive coordinate and prefix bounds accept a two-node path")
        client.call("set_linked_path", {"action": "cancel", "token": bounded["preview"]["token"]})
        draft = preview(client, classname="path_corner", prefix="replaced", points=POINTS)
        client.call("create_primitive", {"shape": "wedge", "mins": [-16, -16, 0], "maxs": [16, 16, 32], "preview": True})
        foreign = client.call("get_primitive_preview")
        reject(client, {"action": "cancel", "token": draft["preview"]["token"]})
        check(client.call("get_primitive_preview") == foreign, "Old linked-path Cancel preserves another geometry draft")
        client.call("cancel_primitive_preview", {"token": foreign["preview"]["token"]})
        draft = preview(client, classname="path_track", prefix="definition_stale", points=POINTS)
        before = snapshot(client, existing)
        client.call("load_entity_definitions", {"path": str(fgd)})
        reject(client, {"action": "apply", "token": draft["preview"]["token"]})
        check(state(client)["preview"] is None and snapshot(client, existing) == before, "Reloading FGD definitions invalidates the draft without map edits")
        draft = preview(client, classname="path_corner", prefix="selection_stale", points=POINTS)
        client.call("select", {"ids": [existing[0]]})
        reject(client, {"action": "apply", "token": draft["preview"]["token"]})
        check(state(client)["preview"] is None, "Selection changes invalidate the captured draft")
        draft = preview(client, classname="path_corner", prefix="source_stale", points=POINTS)
        added = client.call("create_entity", {"classname": "info_target", "origin": [256, 256, 64]})["id"]
        existing.append(added)
        before = snapshot(client, existing)
        reject(client, {"action": "apply", "token": draft["preview"]["token"]})
        check(state(client)["preview"] is None and snapshot(client, existing) == before, "An intervening map edit prevents stale Apply")
        draft = preview(client, classname="path_track", prefix="map_stale", points=POINTS)
        client.call("new_map", {"discard_changes": True})
        reject(client, {"action": "apply", "token": draft["preview"]["token"]})
        check(state(client)["preview"] is None and client.call("get_map_info")["point_entities"] == 0, "Replacing the map prevents stale Apply")
        info = client.call("get_map_info")
        check(not info["compiling"] and not info["launching"] and not info["game_running"], "Authoring validation launches no build or game")
        result = {"checks": len(checks), "passed": checks, "cases": cases, "transport": "stdio", "cross_actor_ownership_exercised": False, "compile_or_game_behavior_exercised": False}
        (args.output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps({"linked_paths": True, "checks": len(checks), "output": str(args.output)}))
    finally:
        client.close()


def main():
    parser = argparse.ArgumentParser(description="Validate linked-path authoring through an isolated MCP session; no compiler or game is launched.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.binary, args.output = args.binary.resolve(), args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=False)
    validate(args)


if __name__ == "__main__":
    main()
