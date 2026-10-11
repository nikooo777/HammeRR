import argparse
import json
from pathlib import Path

from validate_face_lighting import ROOT, Client, detail, face_ref
from validate_material_replacement import preserved_map

MODES = ["textured", "flat", "wireframe", "lightmap_density", "smoothing_groups"]


def validate(client, output):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    with preserved_map(client):
        client.call(
            "set_view",
            {"surface": "textured", "tool_textures": True, "smoothing_group": 1},
        )
        brush_id = client.call(
            "create_block",
            {
                "mins": [0, 0, 0],
                "maxs": [96, 64, 64],
                "material": "brick/brickwall001a",
            },
        )["id"]
        tool_id = client.call(
            "create_block",
            {
                "mins": [150, 0, 0],
                "maxs": [214, 64, 64],
                "material": "tools/toolsnodraw",
            },
        )["id"]
        client.call(
            "create_primitive",
            {
                "shape": "cylinder",
                "mins": [20, 100, 0],
                "maxs": [84, 164, 100],
                "segments": 12,
                "material": "tile/tilefloor001a",
            },
        )
        client.call(
            "create_entity",
            {
                "classname": "light",
                "origin": [115, 80, 115],
                "properties": {"_light": "255 215 160 400"},
            },
        )
        brush = detail(client, [brush_id])[0]
        client.call(
            "edit_faces",
            {
                "faces": [face_ref(brush, face) for face in brush["faces"]],
                "lightmap_scale": 8,
                "smoothing_groups": [1],
            },
        )
        client.call("select", {"ids": []})
        history = client.call("get_map_info")["undo"]
        modes = {}
        for projection in ["perspective", "top", "front", "side"]:
            spec = {"projection": projection}
            if projection == "perspective":
                spec.update({"position": [-190, -250, 230], "look_at": [95, 65, 40]})
            else:
                spec.update({"center": [100, 65, 50], "ortho_height": 310})
            client.call("set_camera", spec)
            current = client.call("get_map_info")["view"]
            images = {}
            for mode in MODES:
                images[mode] = client.render(
                    output / f"{projection}-{mode}.png", surface=mode
                )
                check(
                    client.call("get_map_info")["view"] == current,
                    f"{projection} {mode} render override preserves shared settings",
                )
            check(
                len(set(images.values())) == len(MODES),
                f"All five {projection} modes render distinct images",
            )
            client.call("set_view", {"surface": "wireframe"})
            check(
                client.render(output / f"{projection}-shared-wireframe.png")
                == images["wireframe"],
                f"{projection} shared mode matches its render override",
            )
            client.call("set_view", {"surface": "textured"})
            modes[projection] = images
        check(
            client.call("get_map_info")["undo"] == history,
            "Viewport modes add no map edits",
        )
        client.call(
            "set_camera",
            {
                "projection": "perspective",
                "position": [-190, -250, 230],
                "look_at": [95, 65, 40],
            },
        )
        for mode in ["flat", "wireframe"]:
            filtered = client.render(
                output / f"{mode}-tools-filtered.png", surface=mode, tool_textures=False
            )
            client.call("set_object_visibility", {"action": "hide", "ids": [tool_id]})
            hidden = client.render(
                output / f"{mode}-tools-hidden.png", surface=mode, tool_textures=True
            )
            check(filtered == hidden, f"{mode} respects the tool brush filter")
            client.call("undo")
        client.call("set_view", {"surface": "wireframe"})
        before = client.render(output / "wireframe-unselected.png")
        client.call("select", {"ids": [brush_id]})
        check(
            client.render(output / "wireframe-selected.png") != before,
            "Wireframe preserves selection highlights",
        )
        current = client.call("get_map_info")["view"]
        client.call("set_view", {"surface": "not_a_mode"}, error=True)
        check(
            client.call("get_map_info")["view"] == current,
            "Invalid mode preserves the current view",
        )
        client.call("save_map", {"path": str(output / "view-modes.vmf")})
        (output / "renders.json").write_text(json.dumps(modes, indent=2) + "\n")
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
        description="Validate all viewport modes through MCP."
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
