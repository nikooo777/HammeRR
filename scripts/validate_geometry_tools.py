import argparse
import json
from pathlib import Path

from validate_component_picking import screen
from validate_face_lighting import ROOT, Client, detail
from validate_material_replacement import preserved_map


def validate(client, output):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def components():
        return client.call("get_brush_components")

    def begin(handle="z", **kwargs):
        return client.call(
            "begin_component_drag",
            {"handle": handle, "snapshot": components()["snapshot"], **kwargs},
        )["drag"]["token"]

    def update(token, offset, error=False):
        return client.call(
            "update_component_drag", {"token": token, "offset": offset}, error=error
        )

    def clip():
        return client.call("get_clip_preview")

    def cut_points(points, **kwargs):
        return client.call(
            "set_clip_points",
            {"points": points, "snapshot": clip()["snapshot"], **kwargs},
        )

    with preserved_map(client):
        tools = {t["name"] for t in client.rpc("tools/list", {})["tools"]}
        check(
            {
                "get_component_handles",
                "set_component_move_options",
                "begin_component_drag",
                "update_component_drag",
                "commit_component_drag",
                "cancel_component_drag",
                "set_clip_points",
                "pick_clip_point",
            }
            <= tools,
            "All eight geometry tools are discoverable",
        )
        block = client.call(
            "create_block",
            {
                "mins": [0, 0, 0],
                "maxs": [64, 64, 64],
                "material": "dev/dev_measuregeneric01",
            },
        )["id"]
        shape = components()["brushes"][0]
        face = next(
            f
            for f in shape["faces"]
            if all(shape["vertices"][i]["position"][2] == 64 for i in f["vertices"])
        )
        client.call(
            "set_component_picking",
            {
                "enabled": True,
                "kind": "face",
                "snapshot": components()["snapshot"],
            },
        )
        client.call(
            "select_brush_components",
            {
                "components": [{"solid": shape["solid"], "sides": [face["side"]]}],
                "snapshot": components()["snapshot"],
            },
        )
        client.call(
            "set_component_move_options",
            {
                "snap": True,
                "texture_lock": True,
                "snapshot": components()["snapshot"],
            },
        )
        client.call(
            "set_camera",
            {
                "projection": "perspective",
                "position": [135, -130, 135],
                "fov": 40,
                "look_at": [32, 32, 64],
            },
        )
        source = detail(client, [block])
        history = client.call("get_map_info")["undo"]
        handles = client.call("get_component_handles")
        check(handles["origin"] == [32, 32, 64], "Handles center on selected face")
        for handle in handles["handles"]:
            a, b = handle["pixel_segments"][0]
            point = [(x + y) / 2 for x, y in zip(a, b, strict=True)]
            hit = client.call("get_component_handles", {"point": point})
            check(
                hit["picked"] == handle["handle"],
                f"Rendered {handle['handle']} handle is pickable",
            )
        baseline = client.render(output / "component-handles.png", surface="flat")
        z_handle = next(h for h in handles["handles"] if h["handle"] == "z")
        world_a, world_b = z_handle["segments"][0]
        world = [(a + b) / 2 for a, b in zip(world_a, world_b, strict=True)]
        pointer = screen(handles["camera"], world, handles["width"], handles["height"])
        end = screen(
            handles["camera"],
            [world[0], world[1], world[2] + 16],
            handles["width"],
            handles["height"],
        )
        pointer = {
            "position": [pointer["x"], pointer["y"]],
            "width": pointer["width"],
            "height": pointer["height"],
        }
        end = {
            "position": [end["x"], end["y"]],
            "width": end["width"],
            "height": end["height"],
        }
        token = begin(pointer=pointer)
        moved = client.call("update_component_drag", {"token": token, "pointer": end})
        check(
            moved["preview"]["delta"] == [0, 0, 16],
            "MCP viewport-pointer dragging matches numeric movement",
        )
        client.call("cancel_component_drag", {"token": token})
        check(
            detail(client, [block]) == source,
            "Pointer cancellation preserves the source map",
        )
        token = begin()
        state = update(token, [19, 0])
        check(state["preview"]["delta"] == [0, 0, 16], "Handle movement snaps to grid")
        check(
            state["preview"]["texture_lock"],
            "Handle movement retains texture attachment",
        )
        state = update(token, [33, 0])
        check(
            state["preview"]["delta"] == [0, 0, 32],
            "Offsets are totals from drag start",
        )
        client.render(output / "component-handle-preview.png", surface="flat")
        valid = state["preview"]
        update(token, [-64, 0], error=True)
        check(
            components()["preview"] == valid,
            "Invalid solid retains the last valid preview",
        )
        client.call("commit_component_drag", {"token": token}, error=True)
        check(
            detail(client, [block]) == source, "Rejected drag leaves the map untouched"
        )
        update(token, [33, 0])
        client.call("commit_component_drag", {"token": token})
        changed = detail(client, [block])
        check(changed != source, "Commit applies the component move")
        check(
            len(client.call("get_map_info")["undo"]) == len(history) + 1,
            "Drag creates one undo step",
        )
        check(
            components()["picking"] is not None,
            "Commit retains fixed viewport picking scope",
        )
        token = begin()
        update(token, [16, 0])
        update(token, [0, 0])
        client.call("commit_component_drag", {"token": token})
        check(
            detail(client, [block]) == changed,
            "Returning to zero does not edit the map",
        )
        check(
            len(client.call("get_map_info")["undo"]) == len(history) + 1,
            "Zero drag adds no history",
        )
        token = begin()
        newer = begin()
        client.call("cancel_component_drag", {"token": token}, error=True)
        check(
            components()["drag"]["token"] == newer,
            "Stale cancellation cannot replace a new drag",
        )
        client.call("cancel_component_drag", {"token": newer})
        token = begin()
        update(token, [16, 0])
        client.call(
            "set_camera",
            {"projection": "front", "center": [32, 32, 64], "ortho_height": 256},
        )
        client.call("commit_component_drag", {"token": token}, error=True)
        check(
            components()["drag"] is None and components()["preview"] is None,
            "Camera changes cancel active drags",
        )
        client.call("undo")
        check(
            detail(client, [block]) == source,
            "Undo restores original brush metadata and geometry",
        )
        client.call("redo")
        check(detail(client, [block]) == changed, "Redo restores committed geometry")
        client.call("undo")
        client.call("select", {"ids": [block]})
        client.call(
            "set_camera",
            {"projection": "top", "center": [32, 32, 32], "ortho_height": 128},
        )
        state = cut_points([], keep="both", material="tools/toolsnodraw")
        old_stamp = state["snapshot"]
        for point in [[16, 16, 64], [48, 16, 64]]:
            camera = client.call("get_map_info")["camera"]
            result = client.call(
                "pick_clip_point",
                {**screen(camera, point), "snapshot": clip()["snapshot"]},
            )
            check(
                result["point"] == point, "Viewport surface point shares grid snapping"
            )
        client.call(
            "set_clip_points", {"points": [], "snapshot": old_stamp}, error=True
        )
        check(
            len(clip()["point_draft"]["points"]) == 2,
            "Stale cut edit preserves current points",
        )
        camera = client.call("get_map_info")["camera"]
        client.call(
            "pick_clip_point",
            {**screen(camera, [48, 16, 64]), "snapshot": clip()["snapshot"]},
            error=True,
        )
        check(
            len(clip()["point_draft"]["points"]) == 2,
            "Duplicate picked point is rejected",
        )
        client.call(
            "set_camera",
            {"projection": "front", "center": [32, 32, 32], "ortho_height": 128},
        )
        camera = client.call("get_map_info")["camera"]
        client.call(
            "pick_clip_point",
            {**screen(camera, [32, 0, 16]), "snapshot": clip()["snapshot"]},
        )
        state = clip()
        check(
            len(state["preview"]["result"]["cuts"]) == 1,
            "Third point previews an oblique cut",
        )
        check(
            detail(client, [block]) == source,
            "Cut points and preview add no document edits",
        )
        client.render(output / "three-point-cut.png", surface="flat")
        points = state["point_draft"]["points"]
        preview = state["preview"]
        client.call(
            "set_clip_points",
            {
                "points": [[0, 0, 0], [16, 0, 0], [32, 0, 0]],
                "snapshot": state["snapshot"],
            },
            error=True,
        )
        check(clip()["preview"] == preview, "Collinear points preserve a valid cut")
        state = cut_points(points[:2])
        check(state["preview"] is None, "Removing a point cancels the cut preview")
        state = cut_points(points)
        client.call("commit_clip_preview")
        check(
            client.call("get_map_info")["world_brushes"] == 2,
            "Applying the cut creates two pieces",
        )
        check(clip()["point_draft"] is None, "Commit clears cut-point guides")
        client.call("undo")
        check(detail(client, [block]) == source, "Cut undo restores exact source brush")
        client.call("redo")
        destination = output / "geometry-tools.vmf"
        client.call("save_map", {"path": str(destination)})
        saved = detail(client, client.call("get_map_info")["selection"])
        client.call("open_map", {"path": str(destination)})
        check(
            client.call("get_map_info")["world_brushes"] == 2,
            "Clipped geometry survives save/reopen",
        )
        check(
            bool(saved) and destination.is_file(),
            "Saved fixture is available for inspection",
        )
        client.call("select", {"ids": [block]})
        cut_points([])
        client.call("cancel_clip_preview")
        check(
            clip()["point_draft"] is None, "Cancel clears point capture without an edit"
        )
        check(bool(baseline), "Component guides rendered through MCP")

    (output / "checks.json").write_text(json.dumps(checks, indent=2) + "\n")
    print(json.dumps({"checks": len(checks), "output": str(output)}))


def main():
    parser = argparse.ArgumentParser(
        description="Validate component handles and three-point cuts over MCP."
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
