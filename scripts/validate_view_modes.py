import argparse
import json
import math
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client, detail, face_ref
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
                "material": "de_dust/tilefloor01",
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
        client.call(
            "set_camera",
            {"position": [48, -300, 32], "look_at": [48, 32, 32], "fov": 60},
        )
        history = client.call("get_map_info")["undo"]
        framed = client.call("frame_selection", {"aspect": 0.4})
        half = math.tan(math.radians(framed["fov"]) / 2)
        check(
            all(
                abs(
                    (x - framed["position"][0])
                    / ((y - framed["position"][1]) * half * 0.4)
                )
                <= 1.001
                and abs(
                    (z - framed["position"][2]) / ((y - framed["position"][1]) * half)
                )
                <= 1.001
                for x in [0, 96]
                for y in [0, 64]
                for z in [0, 64]
            ),
            "Portrait perspective frame contains all selected brush corners",
        )
        portrait = client.render(
            output / "portrait-framed.png", width=360, height=900, surface="flat"
        )
        check(
            client.render(
                output / "portrait-focus.png",
                width=360,
                height=900,
                surface="flat",
                camera={"focus": [brush_id]},
            )
            == portrait,
            "MCP focus render and shared framing agree for a portrait viewport",
        )
        check(
            client.call("get_camera") == framed,
            "Focus render preserves the shared camera",
        )
        preview = client.call(
            "select_volume", {"mins": [0, 0, 0], "maxs": [96, 64, 64], "preview": True}
        )
        token = preview["preview"]["token"]
        client.call("frame_volume_selection", {"token": token, "aspect": 0.4})
        client.call("cancel_volume_selection", {"token": token})
        check(
            client.render(
                output / "portrait-volume.png", width=360, height=900, surface="flat"
            )
            == portrait,
            "Volume framing shares the corrected perspective fit",
        )
        check(
            client.call("get_map_info")["undo"] == history,
            "Framing adds no map history",
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
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
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
