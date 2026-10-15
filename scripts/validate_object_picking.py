import argparse
import json
from pathlib import Path
from unittest.mock import patch

from validate_face_lighting import DEFAULT_BINARY, Client, detail


RAY = {"origin": [0, 0, 0], "direction": [7, 0, 0]}


def block(client, x, material="dev/dev_measuregeneric01"):
    return client.call("create_block", {"mins": [x, -8, -8], "maxs": [x + 8, 8, 8], "material": material})["id"]


def create_group(client, members):
    draft = client.call("begin_object_group_edit")
    client.call("update_object_group_edit", {"token": draft["token"], "name": "Ray candidates", "members": members})
    return client.call("apply_object_group_edit", {"token": draft["token"]})["group"]


def validate(client):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    tools = {tool["name"] for tool in client.rpc("tools/list", {})["tools"]}
    check({"get_object_hit_candidates", "cycle_object_hit"} <= tools, "Object candidate inspection and cycling are discoverable")
    far, near, middle = [block(client, x) for x in [80, 24, 48]]
    client.call("select", {"ids": []})
    source = detail(client, [near, middle, far])
    before = client.call("get_map_info")
    report = client.call("get_object_hit_candidates", RAY)
    check(report["total"] == 3 and not report["truncated"] and [row["object"] for row in report["candidates"]] == [near, middle, far], "Candidates follow depth rather than creation order")
    check([row["distance"] for row in report["candidates"]] == [24, 48, 80], "Non-unit ray directions report world distances")
    nearest = client.call("raycast", RAY)
    check(nearest["object"] == near, "Ordinary nearest raycast matches the first candidate")
    check(client.call("get_map_info")["selection"] == [] and client.call("get_map_info")["undo"] == before["undo"], "Candidate queries leave selection and history unchanged")
    for index, expected in [(0, near), (1, middle), (2, far), (0, near)]:
        chosen = client.call("cycle_object_hit", RAY)
        check(chosen["chosen"] == index and chosen["selection"] == [expected], f"Stateless cycle selects candidate {index + 1} and wraps")
    client.call("select", {"ids": [near, far]})
    check(client.call("cycle_object_hit", RAY)["selection"] == [near], "Unmatched multi-selection restarts from the nearest hit")
    client.call("select", {"ids": [middle]})
    check(client.call("cycle_object_hit", RAY)["selection"] == [far], "Manual selection determines the next hit without a stored cursor")
    miss = client.call("cycle_object_hit", {"origin": [0, 128, 0], "direction": [1, 0, 0]})
    check(miss["chosen"] is None and miss["selection"] == [far], "A miss preserves the current selection")
    check(detail(client, [near, middle, far]) == source and client.call("get_map_info")["undo"] == before["undo"] and client.call("get_map_info")["camera"] == before["camera"], "Cycling changes no authored data, Undo history or camera")
    side = detail(client, [near])[0]["faces"][0]["side"]
    client.call("select_faces", {"faces": [{"solid": int(near.split(":")[1]), "side": side}]})
    client.call("cycle_object_hit", RAY, error=True)
    check(len(client.call("get_map_info")["face_selection"]["faces"]) == 1, "Face mode rejection preserves face selection")
    client.call("select", {"ids": [near]})
    client.call("transform_objects", {"ids": [near], "translation": [0, 16, 0], "preview": True})
    client.call("cycle_object_hit", RAY, error=True)
    check(client.call("get_map_info")["transform"]["preview"] is not None, "Active geometry previews reject cycling without discarding the draft")
    client.call("cancel_transform_preview")
    client.call("set_object_visibility", {"ids": [near], "action": "hide"})
    check(client.call("get_object_hit_candidates", RAY)["candidates"][0]["object"] == middle, "Hidden authored objects are absent from candidates")
    client.call("undo")
    tool = block(client, 16, "tools/toolsnodraw")
    check(client.call("get_object_hit_candidates", RAY)["candidates"][0]["object"] == tool, "Visible tool brushes participate in the same depth ordering")
    client.call("set_view", {"tool_textures": False})
    check(client.call("get_object_hit_candidates", RAY)["candidates"][0]["object"] == near, "Tool-texture visibility filters candidates and ordinary picking alike")
    client.call("set_view", {"tool_textures": True})
    client.call("new_map", {"discard_changes": True})
    members = [block(client, 16 + index * 12) for index in range(70)]
    owner = client.call("tie_to_entity", {"ids": members[:2], "classname": "func_brush"})["id"]
    inner = create_group(client, members[2:35])
    create_group(client, [f"group:{inner}", *members[35:]])
    far = block(client, 1000)
    client.call("select", {"ids": []})
    report = client.call("get_object_hit_candidates", RAY)
    check(report["total"] == 3 and not report["truncated"] and len(report["candidates"]) == 3, "Owner and outer-group deduplication happens before the candidate cap")
    check(report["candidates"][0]["object"] == owner and len(report["candidates"][1]["selection_unit"]) == 68 and report["candidates"][2]["object"] == far, "Candidates distinguish actual brush owner hits from expanded group units")
    client.call("cycle_object_hit", RAY)
    selected = client.call("cycle_object_hit", RAY)
    check(selected["chosen"] == 1 and len(selected["selection"]) == 68, "Cycling selects the complete outer group once")
    check(client.call("cycle_object_hit", RAY)["selection"] == [far], "Exact group selection advances to the next unit")
    client.call("set_group_selection_bypass", {"bypass": True})
    report = client.call("get_object_hit_candidates", RAY)
    check(report["total"] == 70 and report["truncated"] and len(report["candidates"]) == report["limit"] == 64, "Bypass reports the exact total and nearest-64 truncation")
    check(all(len(row["selection_unit"]) == 1 for row in report["candidates"]), "Group bypass retains brush-entity ownership while selecting single members")
    client.call("cycle_object_hit", {"origin": [0, 0, 0], "direction": [0, 0, 0]}, error=True)
    return checks


def render_example(client, output):
    client.call("new_map", {"discard_changes": True})
    ids = [client.call("create_block", {
        "mins": [x, -32, 0], "maxs": [x + 48, 32, 64],
        "material": "dev/dev_measuregeneric01",
    })["id"] for x in [0, 80, 160]]
    client.call("select", {"ids": []})
    ray = {"origin": [-64, 0, 32], "direction": [1, 0, 0]}
    for expected in ids:
        assert client.call("cycle_object_hit", ray)["selection"] == [expected]
    client.render(output / "object-hit-cycling.png", width=1100, height=700,
                  surface="wireframe", camera={"position": [-160, -280, 210], "look_at": [96, 0, 32], "fov": 35})


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, default=Path("/tmp/hammerr-object-picking"))
    parser.add_argument("--render", action="store_true")
    args = parser.parse_args()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    with patch.dict("os.environ", {"HAMMERR_RECENT_MAPS_DIR": str(args.output / "recent")}):
        client = Client(None, args.binary.resolve(), args.output)
    try:
        checks = validate(client)
        if args.render:
            render_example(client, args.output)
        (args.output / "results.json").write_text(json.dumps({"checks": len(checks), "passed": checks}, indent=2) + "\n")
        print(json.dumps({"checks": len(checks), "output": str(args.output)}))
    finally:
        client.close()


if __name__ == "__main__":
    main()
