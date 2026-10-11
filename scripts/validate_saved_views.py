import argparse
import json
from pathlib import Path

from validate_face_lighting import ROOT, Client
from validate_material_replacement import preserved_map


def validate(client, output):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def guard():
        state = client.call("get_camera_views")
        return {key: state[key] for key in ["generation", "revision"]}

    with preserved_map(client):
        check(
            not client.call("get_camera_views")["views"], "New map has no saved views"
        )
        client.call("set_view", {"tool_textures": True, "surface": "textured"})
        client.call(
            "create_block",
            {
                "mins": [0, 0, 0],
                "maxs": [96, 64, 48],
                "material": "brick/brickwall001a",
            },
        )
        entries = []
        for projection in ["perspective", "top", "front", "side"]:
            spec = {"projection": projection, "fov": 73.5}
            if projection == "perspective":
                spec.update({"position": [-180, -180, 140], "look_at": [40, 25, 20]})
            else:
                spec.update({"center": [48, 32, 24], "ortho_height": 320.25})
            client.call("set_camera", spec)
            camera = client.call("get_camera")
            image = client.render(output / f"{projection}-saved.png")
            before = client.call("get_map_info")
            name = (
                'Entry "été" \\ wide'
                if projection == "perspective"
                else f"Layout · {projection}"
            )
            saved = client.call("save_camera_view", {"name": name, **guard()})
            check(
                len(client.call("get_map_info")["undo"]) == len(before["undo"]) + 1,
                f"Saving {projection} adds one undo step",
            )
            check(
                client.call("get_camera") == camera,
                f"Saving {projection} preserves the camera",
            )
            entries.append((saved["id"], camera, image))
        history = client.call("get_map_info")["undo"]
        selection = client.call("get_map_info")["selection"]
        for index, (view_id, camera, image) in enumerate(entries):
            client.call("restore_camera_view", {"id": view_id, **guard()})
            check(
                client.call("get_camera") == camera,
                f"Restores exact {camera['projection']} camera",
            )
            check(
                client.render(output / f"restored-{index}.png") == image,
                f"Restores identical {camera['projection']} rendering",
            )
        check(
            client.call("get_map_info")["undo"] == history
            and client.call("get_map_info")["selection"] == selection,
            "Navigation preserves map history and selection",
        )
        before = client.call("get_camera_views")
        client.call("save_camera_view", {"name": 'entry "été" \\ wide'}, error=True)
        client.call("save_camera_view", {"name": ""}, error=True)
        client.call("save_camera_view", {"name": "x" * 81}, error=True)
        client.call(
            "save_camera_view", {"name": "Missing", "replace_id": 999}, error=True
        )
        client.call("restore_camera_view", {"id": 999}, error=True)
        client.call(
            "delete_camera_view",
            {"id": 1, "generation": before["generation"]},
            error=True,
        )
        check(
            client.call("get_camera_views") == before,
            "Invalid edits and guards preserve saved data",
        )
        old_guard = guard()
        client.call(
            "rename_camera_view", {"id": 1, "name": "Entrance overview", **old_guard}
        )
        client.call("delete_camera_view", {"id": 1, **old_guard}, error=True)
        client.call("restore_camera_view", {"id": 1, **old_guard}, error=True)
        check(
            client.call("get_camera_views")["views"][0]["name"] == "Entrance overview",
            "Rename changes only the selected name",
        )
        client.call("undo")
        check(
            client.call("get_camera_views")["views"] == before["views"],
            "Undo restores the prior name and camera",
        )
        client.call("redo")
        current = client.call("get_camera")
        client.call(
            "save_camera_view", {"name": "Entrance updated", "replace_id": 1, **guard()}
        )
        client.call("restore_camera_view", {"id": 1})
        check(
            client.call("get_camera") == current,
            "Replacing a view captures the current camera",
        )
        before_delete = client.call("get_camera_views")["views"]
        client.call("delete_camera_view", {"id": 2, **guard()})
        check(
            len(client.call("get_camera_views")["views"]) == 3,
            "Delete removes exactly one saved view",
        )
        client.call("undo")
        check(
            client.call("get_camera_views")["views"] == before_delete,
            "Undo restores deleted view metadata",
        )
        path = output / "saved-views.vmf"
        client.call("save_map", {"path": str(path)})
        persisted = client.call("get_camera_views")["views"]
        stale_guard = guard()
        client.call("open_map", {"path": str(path)})
        check(
            client.call("get_camera_views")["views"] == persisted,
            "All saved camera metadata survives VMF save/reopen",
        )
        client.call("restore_camera_view", {"id": 2, **stale_guard}, error=True)
        client.call("restore_camera_view", {"id": 2, **guard()})
        check(
            client.call("get_camera") == entries[1][1],
            "Reopened planar view restores zoom and location",
        )
        (output / "views.json").write_text(
            json.dumps(client.call("get_camera_views"), indent=2) + "\n"
        )
    checks.append("Original map, selection and camera restored")
    result = {
        "transport": "http" if client.url else "stdio",
        "checks": len(checks),
        "passed": checks,
    }
    (output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({key: value for key, value in result.items() if key != "passed"}))


def main():
    parser = argparse.ArgumentParser(
        description="Validate saved camera views through MCP."
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
