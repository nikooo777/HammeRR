import argparse
import json
import math
from pathlib import Path

from validate_component_picking import screen
from validate_face_lighting import ROOT, Client, detail
from validate_material_replacement import preserved_map


def validate(client, output):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def state():
        return client.call("get_clip_preview")

    def options(**kwargs):
        return client.call(
            "set_clip_handle_options", {"snapshot": state()["snapshot"], **kwargs}
        )

    def begin(handle="x", **kwargs):
        return client.call(
            "begin_clip_drag",
            {"handle": handle, "snapshot": state()["snapshot"], **kwargs},
        )["drag"]["token"]

    def update(token, amount, error=False):
        return client.call(
            "update_clip_drag", {"token": token, "amount": amount}, error=error
        )

    def cut():
        return client.call(
            "clip_brushes",
            {
                "plane": {"normal": [1, 0, 0], "distance": 32},
                "keep": "both",
                "material": "tools/toolsnodraw",
                "preview": True,
            },
        )

    def pointer(camera, world, width, height):
        pixel = screen(camera, world, width, height)
        return {"position": [pixel["x"], pixel["y"]], "width": width, "height": height}

    with preserved_map(client):
        tools = {t["name"] for t in client.rpc("tools/list", {})["tools"]}
        check(
            {
                "get_clip_handles",
                "set_clip_handle_options",
                "begin_clip_drag",
                "update_clip_drag",
                "finish_clip_drag",
                "cancel_clip_drag",
            }
            <= tools,
            "All six cut-plane handle tools are discoverable",
        )
        block = client.call(
            "create_block",
            {
                "mins": [0, 0, 0],
                "maxs": [64, 64, 64],
                "material": "dev/dev_measuregeneric01",
            },
        )["id"]
        source = detail(client, [block])
        history = client.call("get_map_info")["undo"]
        client.call(
            "set_camera",
            {
                "projection": "perspective",
                "position": [160, -180, 160],
                "look_at": [32, 32, 32],
                "fov": 42,
            },
        )
        cut()
        options(mode="move", snap=True, angle_step=15)
        handles = client.call("get_clip_handles")
        check(
            handles["origin"] == [32, 32, 32],
            "Plane handles pivot at the projected selection center",
        )
        for handle in handles["handles"]:
            a, b = handle["pixel_segments"][0]
            point = [(x + y) / 2 for x, y in zip(a, b, strict=True)]
            hit = client.call("get_clip_handles", {"point": point})
            check(
                hit["picked"] == handle["handle"],
                f"Rendered {handle['handle']} move handle is pickable",
            )
        client.render(output / "cut-plane-move.png", surface="flat")
        stale = state()["snapshot"]
        token = begin()
        result = update(token, [19, 0])
        check(
            result["preview"]["plane"]["distance"] == 48,
            "Translation snaps to the shared grid",
        )
        client.call("commit_clip_preview", error=True)
        check(
            detail(client, [block]) == source,
            "Active positioning cannot commit geometry",
        )
        update(token, [1, 1], error=True)
        check(
            state()["preview"]["plane"]["distance"] == 48,
            "Invalid update retains last valid preview",
        )
        client.call("finish_clip_drag", {"token": token}, error=True)
        check(state()["drag"]["error"] is not None, "Invalid update blocks finishing")
        update(token, [0, 0])
        client.call("finish_clip_drag", {"token": token})
        check(
            client.call("get_map_info")["undo"] == history
            and detail(client, [block]) == source,
            "Finishing positioning leaves geometry and history untouched",
        )
        check(
            state()["preview"]["plane"]["distance"] == 32,
            "Zero amount restores the starting plane",
        )
        client.call(
            "set_clip_handle_options", {"mode": "rotate", "snapshot": stale}, error=True
        )
        token = begin()
        update(token, [16, 0])
        client.call("cancel_clip_drag", {"token": token})
        check(
            state()["preview"]["plane"]["distance"] == 32,
            "Cancel positioning restores the preceding cut preview",
        )
        newer = begin()
        for tool in ["finish_clip_drag", "cancel_clip_drag"]:
            client.call(tool, {"token": token}, error=True)
        update(token, [32, 0], error=True)
        check(
            state()["drag"]["token"] == newer,
            "Stale drag tokens cannot modify or close newer work",
        )
        client.call("cancel_clip_drag", {"token": newer})
        move = next(h for h in handles["handles"] if h["handle"] == "x")
        world = [(a + b) / 2 for a, b in zip(*move["segments"][0], strict=True)]
        start = pointer(handles["camera"], world, handles["width"], handles["height"])
        end = pointer(
            handles["camera"],
            [world[0] + 16, *world[1:]],
            handles["width"],
            handles["height"],
        )
        token = begin(pointer=start)
        client.call("update_clip_drag", {"token": token, "pointer": end})
        check(
            state()["preview"]["plane"]["distance"] == 48,
            "Pointer translation matches numeric movement",
        )
        client.call(
            "update_clip_drag",
            {"token": token, "pointer": {**end, "width": end["width"] + 1}},
            error=True,
        )
        check(
            state()["drag"]["error"] is not None,
            "Viewport resize rejects the old pointer gesture",
        )
        client.call("cancel_clip_drag", {"token": token})
        options(mode="rotate")
        handles = client.call("get_clip_handles")
        for h in handles["handles"]:
            hit = any(
                client.call(
                    "get_clip_handles",
                    {"point": [(a + b) / 2 for a, b in zip(*segment, strict=True)]},
                )["picked"]
                == h["handle"]
                for segment in h["pixel_segments"]
            )
            check(hit, f"Rendered {h['handle']} rotation ring is pickable")
        token = begin("z")
        rotated = update(token, [31, 0])["preview"]
        normal = rotated["plane"]["normal"]
        check(
            abs(normal[0] - math.cos(math.pi / 6)) < 1e-9
            and abs(normal[1] - 0.5) < 1e-9,
            "Rotation snaps to 30 degrees around world Z",
        )
        check(
            abs(rotated["plane"]["distance"] - 32 * (normal[0] + normal[1])) < 1e-9,
            "Rotation keeps the projected selection center on the plane",
        )
        check(
            rotated["keep"] == "both" and rotated["material"] == "tools/toolsnodraw",
            "Positioning preserves keep side and cap material",
        )
        client.call("finish_clip_drag", {"token": token})
        client.render(output / "cut-plane-rotate.png", surface="flat")
        check(
            client.call("get_map_info")["undo"] == history,
            "Rotation release remains a preview",
        )
        client.call("commit_clip_preview")
        check(
            len(client.call("get_map_info")["undo"]) == len(history) + 1,
            "Apply cut creates one undo step",
        )
        client.call("undo")
        check(
            detail(client, [block]) == source,
            "Undo restores original geometry and face data",
        )
        client.call("redo")
        path = output / "positioned-cut.vmf"
        client.call("save_map", {"path": str(path)})
        saved = detail(client, client.call("get_map_info")["selection"])
        client.call("open_map", {"path": str(path)})
        check(
            detail(client, [o["id"] for o in saved]) == saved,
            "Positioned cut survives save and reopen",
        )
        client.call("new_map", {"discard_changes": True})
        block = client.call(
            "create_block",
            {
                "mins": [0, 0, 0],
                "maxs": [64, 64, 64],
                "material": "dev/dev_measuregeneric01",
            },
        )["id"]
        client.call(
            "set_clip_points",
            {
                "points": [[32, 0, 0], [32, 64, 0], [32, 64, 64]],
                "snap": True,
                "snapshot": state()["snapshot"],
            },
        )
        options(mode="rotate", snap=True)
        before = state()
        token = begin("z")
        moved = update(token, [30, 0])
        check(
            moved["point_draft"]["points"] != before["point_draft"]["points"],
            "Three-point markers rotate with the cut plane",
        )
        client.call("cancel_clip_drag", {"token": token})
        check(
            state()["point_draft"] == before["point_draft"],
            "Cancellation restores original three-point markers",
        )
        token = begin("z")
        moved = update(token, [30, 0])
        client.call("finish_clip_drag", {"token": token})
        changed = client.call(
            "set_clip_points",
            {
                **moved["point_draft"],
                "keep": "back",
                "material": "tools/toolsnodraw",
                "snapshot": state()["snapshot"],
            },
        )
        check(
            changed["point_draft"]["points"] == moved["point_draft"]["points"],
            "Side/material changes retain rotated points without re-snapping",
        )
        token = begin("z")
        update(token, [15, 0])
        options(snap=False)
        check(
            state()["drag"] is None
            and state()["preview"]["plane"] == changed["preview"]["plane"],
            "Changing handle settings cancels positioning and restores its starting plane",
        )
        token = begin("z")
        update(token, [15, 0])
        client.call("set_camera", {"projection": "top"})
        check(
            state()["drag"] is None
            and state()["point_draft"] == changed["point_draft"],
            "Camera changes cancel positioning without losing the cut",
        )
        token = begin("z")
        client.call("select", {"ids": []})
        client.call("cancel_clip_drag", {"token": token}, error=True)
        check(
            state()["preview"] is None and state()["drag"] is None,
            "Selection changes discard the obsolete preview and gesture",
        )
    (output / "checks.json").write_text(json.dumps(checks, indent=2) + "\n")
    print(f"Passed {len(checks)} cut-plane handle checks", flush=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--url")
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    client = Client(args.url, args.binary, args.output)
    try:
        validate(client, args.output)
    finally:
        client.close()


if __name__ == "__main__":
    main()
