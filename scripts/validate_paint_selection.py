import argparse
import json
import math
from pathlib import Path

from validate_face_lighting import ROOT, Client, detail
from validate_material_replacement import preserved_map


def validate(client, output):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def begin(point, **options):
        return client.call(
            "begin_paint_selection",
            {
                "pointer": {"position": point, "width": 800, "height": 600},
                "radius": 6,
                "mode": "replace",
                "selection_mode": "objects",
                **options,
            },
        )["preview"]

    def update(token, points):
        return client.call(
            "update_paint_selection", {"token": token, "points": points}
        )["preview"]

    def current():
        return client.call("get_paint_selection")["preview"]

    def block(center, half=16, material="brick/brickwall001a"):
        return client.call(
            "create_block",
            {
                "mins": [v - half for v in center],
                "maxs": [v + half for v in center],
                "material": material,
            },
        )["id"]

    with preserved_map(client):
        tools = {tool["name"] for tool in client.rpc("tools/list", {})["tools"]}
        check(
            all(
                name in tools
                for name in [
                    "set_paint_tool",
                    "begin_paint_selection",
                    "update_paint_selection",
                    "get_paint_selection",
                    "commit_paint_selection",
                    "cancel_paint_selection",
                ]
            ),
            "All shared paint tools discoverable",
        )
        for projection, axis, depth, direction in [
            ("top", 0, 2, -1),
            ("front", 0, 1, 1),
            ("side", 1, 0, -1),
            ("perspective", 0, 1, 1),
        ]:
            client.call("new_map", {"discard_changes": True})
            client.call(
                "set_view",
                {"surface": "textured", "tool_textures": True},
            )
            ids = []
            solid_ids = []
            for distance in [-64, 64]:
                center = [0, 0, 0]
                center[axis] = distance
                ids.append(block(center))
                solid_ids.append(int(ids[-1].split(":")[1]))
                if projection == "perspective":
                    center[axis] *= 1.25
                center[depth] = direction * 128
                block(center, half=8)
            ids[1] = client.call(
                "tie_to_entity", {"ids": [ids[1]], "classname": "func_detail"}
            )["id"]
            hidden = block([0, 0, 0])
            client.call("set_object_visibility", {"action": "hide", "ids": [hidden]})
            client.call("select", {"ids": []})
            if projection == "perspective":
                camera = {
                    "projection": projection,
                    "position": [0, -512, 0],
                    "look_at": [0, 0, 0],
                    "fov": 60,
                }
                span = 64 * 300 / (512 * math.tan(math.radians(30)))
            else:
                camera = {
                    "projection": projection,
                    "center": [0, 0, 0],
                    "ortho_height": 384,
                }
                span = 100
            client.call("set_camera", camera)
            points = [[400 - span, 300], [400 + span, 300]]
            original = detail(client, ids)
            history = client.call("get_map_info")["undo"]
            image_args = {"width": 800, "height": 600}
            baseline = client.render(output / f"{projection}-before.png", **image_args)
            draft = begin(points[0])
            token = draft["token"]
            draft = update(token, [points[1]])
            (output / f"{projection}-stroke.json").write_text(
                json.dumps(
                    {
                        "expected": ids,
                        "draft": draft,
                        "camera": client.call("get_map_info")["camera"],
                    },
                    indent=2,
                )
            )
            check(
                draft["objects"] == ids,
                f"{projection}: visible world brush and brush entity matched",
            )
            check(
                not client.call("get_map_info")["selection"],
                f"{projection}: preview preserves committed selection",
            )
            check(
                detail(client, draft["objects"]) == original,
                f"{projection}: returned IDs work in object inspection",
            )
            check(
                client.render(output / f"{projection}-paint.png", **image_args)
                != baseline,
                f"{projection}: preview highlights and outline render",
            )
            client.call("cancel_paint_selection", {"token": token})
            check(
                client.render(output / f"{projection}-cancel.png", **image_args)
                == baseline,
                f"{projection}: cancellation restores pixels",
            )
            token = begin(points[0])["token"]
            dense = [
                [points[0][0] + (points[1][0] - points[0][0]) * i / 32, 300]
                for i in range(1, 33)
            ]
            sampled = update(token, dense)
            check(
                sampled["objects"] == draft["objects"]
                and sampled["sample_count"] == draft["sample_count"],
                f"{projection}: sparse and dense paths are identical",
            )
            page = client.call("get_paint_selection", {"offset": 1, "limit": 1})[
                "preview"
            ]
            check(
                page["objects"] == ids[1:] and page["object_count"] == 2,
                f"{projection}: candidate pagination",
            )
            update(token, [points[0], points[1]])
            check(
                current()["sample_count"] == draft["sample_count"],
                f"{projection}: repeat passes deduplicated",
            )
            for args in [
                {"points": []},
                {"points": [[0, 0]] * 257},
                {"points": [[1e20, 0]]},
                {"token": "0", "points": points},
            ]:
                client.call(
                    "update_paint_selection", {"token": token, **args}, error=True
                )
                check(
                    current()["token"] == token and current()["objects"] == ids,
                    f"{projection}: invalid update preserves preview",
                )
            client.call("commit_paint_selection", {"token": token})
            check(
                client.call("get_map_info")["selection"] == ids,
                f"{projection}: commit applies candidates",
            )
            token = begin(points[0], mode="toggle")["token"]
            update(token, [points[1], points[0], points[1]])
            client.call("commit_paint_selection", {"token": token})
            check(
                not client.call("get_map_info")["selection"],
                f"{projection}: toggle counts each object once",
            )
            draft = begin(points[0], selection_mode="faces")
            token = draft["token"]
            draft = update(token, [points[1]])
            check(
                draft["face_count"] >= 2
                and {face["solid"] for face in draft["faces"]} == set(solid_ids)
                and not draft["objects"],
                f"{projection}: face strokes find only front faces",
            )
            client.call("commit_paint_selection", {"token": token})
            check(
                len(client.call("get_face_selection")["faces"]) == draft["face_count"],
                f"{projection}: shared face selection committed",
            )
            check(
                detail(client, ids) == original
                and client.call("get_map_info")["undo"] == history,
                f"{projection}: map and undo unchanged",
            )
            old = begin(points[0])["token"]
            token = begin(points[1])["token"]
            for command in ["commit_paint_selection", "cancel_paint_selection"]:
                client.call(command, {"token": old}, error=True)
                check(
                    current()["token"] == token,
                    f"{projection}: stale {command} preserves newer stroke",
                )
            client.call("set_paint_tool", {"radius": 65}, error=True)
            check(
                current()["token"] == token,
                f"{projection}: invalid radius preserves preview",
            )
            client.call("set_paint_tool", {"radius": 20})
            check(current() is None, f"{projection}: radius change discards preview")
            token = begin(points[0])["token"]
            client.call(
                "set_camera", {"projection": "side" if projection != "side" else "top"}
            )
            check(current() is None, f"{projection}: camera change invalidates stroke")
        client.call("new_map", {"discard_changes": True})
        tool = block([0, 0, 0], material="tools/toolsnodraw")
        client.call(
            "set_camera",
            {"projection": "top", "center": [0, 0, 0], "ortho_height": 384},
        )
        client.call("set_view", {"tool_textures": False})
        check(begin([400, 300])["objects"] == [], "Hidden tool brushes excluded")
        client.call("set_view", {"tool_textures": True})
        check(begin([400, 300])["objects"] == [tool], "Visible tool brushes included")
        client.call("set_object_visibility", {"action": "hide", "ids": [tool]})
        check(current() is None, "Visibility changes invalidate stroke")
        light = client.call(
            "create_entity", {"classname": "light", "origin": [0, 0, 64]}
        )["id"]
        check(
            begin([400, 300])["objects"] == [light],
            "Object strokes include point helpers",
        )
        check(
            begin([400, 300], selection_mode="faces")["faces"] == [],
            "Face strokes ignore point helpers",
        )
        token = current()["token"]
        client.call("cancel_paint_selection", {"token": token})
        client.call("save_map", {"path": str(output / "paint-selection.vmf")})
    checks.append("Original map, selection, camera and paint settings restored")
    result = {
        "transport": "http" if client.url else "stdio",
        "checks": len(checks),
        "passed": checks,
    }
    (output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({k: v for k, v in result.items() if k != "passed"}))


def main():
    parser = argparse.ArgumentParser(
        description="Validate paint selection through MCP; HTTP requires a clean idle map with empty history."
    )
    parser.add_argument("--url")
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    client = Client(args.url, args.binary.resolve(), output)
    try:
        validate(client, output)
    finally:
        client.close()


if __name__ == "__main__":
    main()
