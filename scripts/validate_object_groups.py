import argparse
import json
from pathlib import Path

from validate_face_lighting import ROOT, Client, detail


def validate(args):
    client = Client(None, args.binary.resolve(), args.output)
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def groups():
        return client.call("get_object_groups")

    def create(name, members):
        draft = client.call("begin_object_group_edit")
        token = draft["token"]
        report = client.call(
            "update_object_group_edit",
            {"token": token, "name": name, "members": members},
        )
        check(report["valid"], f"Valid draft: {name}")
        return client.call("apply_object_group_edit", {"token": token})["group"]

    try:
        tools = {tool["name"] for tool in client.rpc("tools/list", {})["tools"]}
        check(
            {
                "get_object_groups", "begin_object_group_edit",
                "update_object_group_edit", "add_object_group_selection",
                "apply_object_group_edit", "cancel_object_group_edit",
                "select_object_group", "select_object_group_member",
                "set_group_selection_bypass",
            } <= tools,
            "Group inspection, authoring, selection and bypass tools are discoverable",
        )
        ids = [
            client.call("create_entity", {"classname": "info_target", "origin": [x, 0, 0]})["id"]
            for x in [0, 64, 128]
        ]
        ids.append(client.call("create_block", {
            "mins": [192, -8, -8], "maxs": [208, 8, 8], "material": "brick/test",
        })["id"])
        client.call("select", {"ids": ids[:2]})
        original = detail(client, ids)
        history = client.call("get_map_info")["undo"]
        draft = client.call("begin_object_group_edit")
        check(len(draft["members"]) == 2, "New draft captures the current selection")
        client.call("cancel_object_group_edit", {"token": draft["token"]})
        check(detail(client, ids) == original and client.call("get_map_info")["undo"] == history,
              "Cancelled draft preserves source and Undo history")

        inner = create("Courtyard props", ids[:2])
        outer = create("Courtyard assembly", [f"group:{inner}", *ids[2:]])
        check(len(groups()["groups"]) == 2, "Nested native groups created")
        selected = client.call("select", {"ids": [ids[0]]})["selection"]
        check(set(selected) == set(ids), "Ordinary selection expands to the outer group")
        selected = client.call("select", {"ids": ids[:2], "mode": "toggle"})["selection"]
        check(not selected, "Several members toggle the group only once")
        selected = client.call("select_object_group", {"group": inner})["members"]
        check(selected == ids[:2], "Explicit nested selection excludes ancestors")
        client.call("set_group_selection_bypass", {"bypass": True})
        selected = client.call("select", {"ids": [ids[0]]})["selection"]
        check(selected == ids[:1], "Bypass selects a single member")
        client.call("set_group_selection_bypass", {"bypass": False})

        original = detail(client, ids)
        client.call("transform_objects", {"ids": [ids[0]], "translation": [8, 0, 0], "preview": True})
        check(client.call("get_map_info")["selection"] == ids[:1], "Explicit transform preview retains exact scope")
        check(detail(client, ids) == original, "Transform preview preserves source")
        client.call("commit_transform_preview")
        check(detail(client, ids)[1:] == original[1:], "Transform commit preserves grouped peers")
        client.call("undo")
        check(detail(client, ids) == original, "Transform Undo restores the grouped source")

        bounds = {"mins": [-12, -12, -12], "maxs": [12, 12, 12], "preview": True}
        draft = client.call("select_volume", bounds)["preview"]
        check(set(draft["objects"]) == set(ids), "Intersecting volume previews the whole visible group")
        client.call("commit_volume_selection", {"token": draft["token"]})
        check(set(client.call("get_map_info")["selection"]) == set(ids), "Volume commit matches its candidates")
        draft = client.call("select_volume", {**bounds, "rule": "contained"})["preview"]
        check(not draft["objects"], "Contained volume requires the whole group")
        client.call("cancel_volume_selection", {"token": draft["token"]})

        client.call("set_object_visibility", {"action": "hide", "ids": [ids[1]]})
        selected = client.call("select", {"ids": [ids[0]]})["selection"]
        check(set(selected) == set(ids) - {ids[1]}, "Ordinary group selection skips hidden peers")
        selected = client.call("select_object_group", {"group": inner, "include_hidden": True})["members"]
        check(selected == ids[:2], "Explicit group selection can include hidden members")
        client.call("undo")

        draft = client.call("begin_object_group_edit", {"group": outer})
        invalid = client.call("update_object_group_edit", {"token": draft["token"], "parent": inner})
        check(not invalid["valid"], "Parent cycle is visible as an invalid draft")
        original = detail(client, ids)
        client.call("apply_object_group_edit", {"token": draft["token"]}, error=True)
        check(detail(client, ids) == original, "Invalid Apply preserves object memberships")
        replacement = client.call("begin_object_group_edit", {"group": inner})
        client.call("cancel_object_group_edit", {"token": draft["token"]}, error=True)
        check(groups()["draft"]["token"] == replacement["token"], "Old token cannot cancel a replacement")
        client.call("transform_objects", {"ids": [ids[2]], "translation": [8, 0, 0]})
        client.call("apply_object_group_edit", {"token": replacement["token"]}, error=True)
        client.call("cancel_object_group_edit", {"token": replacement["token"]})
        client.call("undo")
        checks.append("Map revision changes reject Apply while permitting Cancel")

        draft = client.call("begin_object_group_edit", {"group": inner})
        client.call("update_object_group_edit", {"token": draft["token"], "members": ids[:1]})
        client.call("apply_object_group_edit", {"token": draft["token"]})
        member = client.call("select_object_group", {"group": inner})["members"]
        check(member == ids[:1], "Membership editing removes the requested direct member")
        member = client.call("select_object_group", {"group": outer})["members"]
        check(set(member) == set(ids), "Removed member returns to the previous parent")
        draft = client.call("begin_object_group_edit", {"group": inner, "ungroup": True})
        client.call("apply_object_group_edit", {"token": draft["token"]})
        check(len(groups()["groups"]) == 1, "Ungroup removes only the chosen group")
        client.call("undo")
        check(len(groups()["groups"]) == 2, "Undo restores native group records")
        client.call("redo")
        check(len(groups()["groups"]) == 1, "Redo reapplies ungroup atomically")

        saved_groups = groups()["groups"]
        saved_objects = detail(client, ids)
        path = (args.output / "object-groups.vmf").resolve()
        client.call("save_map", {"path": str(path)})
        client.call("open_map", {"path": str(path), "discard_changes": True})
        check(groups()["groups"] == saved_groups and detail(client, ids) == saved_objects,
              "Save and reopen preserve native nesting, names and object membership")
        draft = client.call("begin_object_group_edit", {"group": outer})
        client.call("new_map", {"discard_changes": True})
        client.call("apply_object_group_edit", {"token": draft["token"]}, error=True)
        check(groups()["draft"] is None, "Map replacement invalidates the captured draft")

        result = {"checks": len(checks), "passed": checks, "transport": "stdio"}
        (args.output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps({"object_groups": True, "checks": len(checks), "output": str(args.output)}))
    finally:
        client.close()


def main():
    parser = argparse.ArgumentParser(description="Validate object groups through an isolated headless MCP session.")
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    validate(args)


if __name__ == "__main__":
    main()
