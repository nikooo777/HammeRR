import argparse
import json
import math
from pathlib import Path

from validate_face_lighting import ROOT, Client, detail
from validate_material_replacement import preserved_map


def screen(camera, point, width=1100, height=700):
    projection = camera["projection"]
    if projection == "top":
        forward, right, up = [0, 0, -1], [1, 0, 0], [0, 1, 0]
    elif projection == "front":
        forward, right, up = [0, 1, 0], [1, 0, 0], [0, 0, 1]
    elif projection == "side":
        forward, right, up = [-1, 0, 0], [0, 1, 0], [0, 0, 1]
    else:
        yaw, pitch = map(math.radians, [camera["yaw"], camera["pitch"]])
        forward = [
            math.cos(pitch) * math.cos(yaw),
            math.cos(pitch) * math.sin(yaw),
            math.sin(pitch),
        ]
        right = [math.sin(yaw), -math.cos(yaw), 0]
        up = [
            -math.sin(pitch) * math.cos(yaw),
            -math.sin(pitch) * math.sin(yaw),
            math.cos(pitch),
        ]
    relative = [a - b for a, b in zip(point, camera["position"], strict=True)]
    depth = sum(a * b for a, b in zip(relative, forward, strict=True))
    half = (
        math.tan(math.radians(camera["fov"]) / 2) * depth
        if projection == "perspective"
        else camera["ortho_height"] / 2
    )
    x = width / 2 + sum(
        a * b for a, b in zip(relative, right, strict=True)
    ) * height / (2 * half)
    y = height / 2 - sum(a * b for a, b in zip(relative, up, strict=True)) * height / (
        2 * half
    )
    return {"x": x, "y": y, "width": width, "height": height}


def validate(client, output):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def state():
        return client.call("get_brush_components")

    def picking(kind="vertex", solids=None, enabled=True):
        return client.call(
            "set_component_picking",
            {
                "enabled": enabled,
                "kind": kind,
                "solids": solids,
                "snapshot": state()["snapshot"],
            },
        )

    def select(result, mode="replace"):
        return client.call(
            "select_brush_components",
            {
                "components": [c["component"] for c in result["candidates"][:1]],
                "mode": mode,
                "snapshot": result["snapshot"],
            },
        )

    with preserved_map(client):
        tools = {tool["name"] for tool in client.rpc("tools/list", {})["tools"]}
        check(
            {"set_component_picking", "pick_brush_components"} <= tools,
            "Viewport picking tools discoverable",
        )
        objects = [
            client.call(
                "create_block",
                {
                    "mins": [x, 0, 0],
                    "maxs": [x + 64, 64, 64],
                    "material": "brick/brickwall001a",
                },
            )["id"]
            for x in [0, 128]
        ]
        solids = [int(object_id.split(":")[1]) for object_id in objects]
        original = detail(client, objects)
        history = client.call("get_map_info")["undo"]
        client.call("select", {"ids": objects})
        start = picking()
        check(
            start["picking"] == {"kind": "vertex", "solids": solids},
            "Picking captures all selected brushes",
        )
        for projection in ["top", "front", "side", "perspective"]:
            camera_spec = (
                {
                    "projection": projection,
                    "position": [-240, -280, 230],
                    "look_at": [64, 32, 32],
                }
                if projection == "perspective"
                else {
                    "projection": projection,
                    "center": [64, 32, 32],
                    "ortho_height": 384,
                }
            )
            client.call("set_camera", camera_spec)
            camera = client.call("get_map_info")["camera"]
            point = [0, 0, 64]
            args = {**screen(camera, point), "kind": "vertex", "radius": 2}
            before = state()
            result = client.call("pick_brush_components", args)
            check(
                result["total"] >= 1
                and any(c["position"] == point for c in result["candidates"]),
                f"{projection}: corner query matches viewport coordinates",
            )
            check(state() == before, f"{projection}: query leaves state unchanged")
            chosen = select(result)
            check(
                chosen["selection"] == [result["candidates"][0]["component"]]
                and chosen["picking"]["solids"] == solids
                and len(state()["brushes"]) == 2,
                f"{projection}: selection retains multibrush scope",
            )
            client.call(
                "select_brush_components",
                {"components": [], "snapshot": result["snapshot"]},
                error=True,
            )
            result = client.call("pick_brush_components", args)
            check(
                not select(result, "toggle")["selection"],
                f"{projection}: toggle clears the component",
            )
            for kind, target in [("edge", [32, 0, 64]), ("face", [32, 32, 32])]:
                picking(kind)
                hit = client.call(
                    "pick_brush_components",
                    {**screen(camera, target), "kind": kind, "radius": 2},
                )
                check(
                    hit["total"] >= 1
                    and len(hit["candidates"][0]["component"]["sides"])
                    == (2 if kind == "edge" else 1),
                    f"{projection}: {kind} picking",
                )
            client.render(output / f"{projection}-guides.png", surface="flat")
        check(
            detail(client, objects) == original
            and client.call("get_map_info")["undo"] == history,
            "Picking does not edit geometry or history",
        )
        client.call(
            "set_camera",
            {"projection": "top", "center": [64, 32, 32], "ortho_height": 512},
        )
        camera = client.call("get_map_info")["camera"]
        picking("vertex")
        empty = client.call(
            "pick_brush_components",
            {"x": 1, "y": 1, "width": 1100, "height": 700, "kind": "vertex"},
        )
        check(
            empty["total"] == 0 and select(empty)["picking"]["solids"] == solids,
            "Empty click clears components and keeps scope",
        )
        shape = state()["brushes"][0]
        components = [
            {"solid": shape["solid"], "sides": v["sides"]} for v in shape["vertices"]
        ]
        client.call(
            "select_brush_components",
            {"components": components, "snapshot": state()["snapshot"]},
        )
        draft = client.call(
            "preview_component_move",
            {"delta": [0, 128, 0], "snapshot": state()["snapshot"]},
        )
        moved = client.call(
            "pick_brush_components", {**screen(camera, [0, 128, 64]), "kind": "vertex"}
        )
        check(
            any(c["position"] == [0, 128, 64] for c in moved["candidates"]),
            "Queries follow the component preview",
        )
        client.render(output / "preview-guides.png", surface="flat")
        before_invalid = state()
        for args in [
            {"enabled": True, "kind": "edge", "solids": [4294967295]},
            {"enabled": False, "snapshot": "stale"},
        ]:
            client.call(
                "set_component_picking",
                {"snapshot": state()["snapshot"], **args},
                error=True,
            )
        check(
            state() == before_invalid, "Invalid mode changes preserve preview and scope"
        )
        for radius in [0, 33]:
            client.call(
                "pick_brush_components",
                {"x": 10, "y": 10, "kind": "vertex", "radius": radius},
                error=True,
            )
        client.call("cancel_component_move", {"token": draft["preview"]["token"]})
        check(
            client.call(
                "pick_brush_components",
                {**screen(camera, [0, 128, 64]), "kind": "vertex"},
            )["total"]
            == 0,
            "Cancelling restores pick geometry",
        )
        client.call("select", {"ids": [objects[0]]})
        check(state()["picking"] is None, "External selection exits component picking")
        owner = client.call(
            "tie_to_entity", {"ids": [objects[0]], "classname": "func_detail"}
        )["id"]
        picking("face")
        hit = client.call(
            "pick_brush_components", {**screen(camera, [32, 32, 64]), "kind": "face"}
        )
        chosen = select(hit)
        check(
            chosen["selection"][0]["solid"] == solids[0]
            and client.call("get_map_info")["selection"] == [owner],
            "Face picking preserves brush entity ownership",
        )
        stopped = picking(enabled=False)
        check(
            stopped["picking"] is None
            and not stopped["selection"]
            and stopped["preview"] is None,
            "Stop picking clears mode, draft and markers",
        )
        client.render(output / "stopped.png", surface="flat")
    checks.append("Original session restored")
    report = {
        "transport": "http" if client.url else "stdio",
        "tools": len(tools),
        "checks": len(checks),
        "passed": checks,
    }
    (output / "validation.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({key: value for key, value in report.items() if key != "passed"}))


def main():
    parser = argparse.ArgumentParser(
        description="Validate viewport component picking over MCP in a clean idle session."
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
