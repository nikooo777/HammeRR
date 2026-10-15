import argparse
import json
import struct
import zlib
from pathlib import Path

from validate_face_lighting import ROOT, Client
from validate_material_replacement import preserved_map


def rgba_png(path):
    data = path.read_bytes()
    position, chunks = 8, []
    while position < len(data):
        size = struct.unpack_from(">I", data, position)[0]
        kind, content = data[position + 4:position + 8], data[position + 8:position + 8 + size]
        if kind == b"IHDR":
            width, height, bits, color = struct.unpack_from(">IIBB", content)
            assert bits == 8 and color == 6, "Expected RGBA8 PNG"
        if kind == b"IDAT":
            chunks.append(content)
        position += size + 12
    raw = zlib.decompress(b"".join(chunks))
    stride = width * 4
    previous = bytearray(stride)
    pixels = bytearray()
    for row in range(height):
        offset = row * (stride + 1)
        mode = raw[offset]
        current = bytearray(raw[offset + 1:offset + stride + 1])
        for index in range(stride):
            left = current[index - 4] if index >= 4 else 0
            above = previous[index]
            diagonal = previous[index - 4] if index >= 4 else 0
            if mode == 1:
                predictor = left
            elif mode == 2:
                predictor = above
            elif mode == 3:
                predictor = (left + above) // 2
            elif mode == 4:
                estimate = left + above - diagonal
                predictor = min([(abs(estimate - left), 0, left), (abs(estimate - above), 1, above), (abs(estimate - diagonal), 2, diagonal)])[2]
            else:
                assert mode == 0, f"PNG filter {mode}"
                predictor = 0
            current[index] = (current[index] + predictor) & 255
        pixels.extend(current)
        previous = current
    return pixels


def image_difference(first, second):
    return max(abs(a - b) for a, b in zip(rgba_png(first), rgba_png(second), strict=True))


def set_sky(client, name):
    token = client.call("begin_property_edit", {"world": True})["draft"]["token"]
    client.call(
        "update_property_edit",
        {"token": token, "changes": [{"action": "set", "key": "skyname", "value": name}]},
    )
    client.call("commit_property_edit", {"token": token})


def validate(client, output, sky):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    with preserved_map(client) as original:
        try:
            set_sky(client, sky)
            client.call("set_spatial_settings", {"grid_visible": False})
            client.call("set_view", {"surface": "textured", "tool_textures": True, "sky_preview": False})
            for axis in range(3):
                for sign in [-1, 1]:
                    mins, maxs = [-272] * 3, [272] * 3
                    mins[axis] = -272 if sign < 0 else 256
                    maxs[axis] = -256 if sign < 0 else 272
                    client.call("create_block", {"mins": mins, "maxs": maxs, "material": "tools/toolsskybox"})
            client.call("select", {"ids": []})
            state = client.call("get_map_info")
            check(state["skybox"]["ready"], state["skybox"]["summary"])
            check(len(state["skybox"]["faces"]) == 6, "Six mounted LDR faces reported")
            images = {}
            directions = [("rt", [1, 0, 0]), ("bk", [0, 1, 0]), ("lf", [-1, 0, 0]), ("ft", [0, -1, 0]), ("up", [0, 0, 1]), ("dn", [0, 0, -1])]
            for suffix, direction in directions:
                camera = {"projection": "perspective", "position": [0, 0, 0], "look_at": direction}
                images[suffix] = client.render(output / f"sky-{suffix}.png", camera=camera, sky_preview=True, width=641, height=641)
            camera = {"projection": "perspective", "position": [0, 0, 0], "look_at": [1, 0, 0]}
            client.call("set_camera", camera)
            baseline = client.render(output / "sky-disabled.png", width=641, height=641)
            pick = client.call("pick_at_pixel", {"x": 320, "y": 320, "width": 641, "height": 641})
            check(baseline != images["rt"], "Preview resolves the sky instead of the tool surface")
            check(not client.call("get_map_info")["view"]["sky_preview"], "Render overrides leave the shared preview disabled")
            history = client.call("get_map_info")["undo"]
            client.call("set_view", {"sky_preview": True})
            check(client.render(output / "sky-shared.png", width=641, height=641) == images["rt"], "Shared Session control matches the render override")
            check(client.call("pick_at_pixel", {"x": 320, "y": 320, "width": 641, "height": 641}) == pick, "Sky preview preserves sky-surface picking")
            check(client.call("get_map_info")["undo"] == history, "Preview controls add no map history")
            moved = {"position": [20, -15, 10], "yaw": 0, "pitch": 0}
            client.render(output / "sky-translated.png", camera=moved, width=641, height=641)
            difference = image_difference(output / "sky-rt.png", output / "sky-translated.png")
            check(difference <= 1, f"Sky remains at infinity when the camera translates (maximum channel difference {difference})")
            for mode in ["flat", "wireframe", "lightmap_density", "smoothing_groups"]:
                enabled = client.render(output / f"{mode}-enabled.png", surface=mode, sky_preview=True)
                disabled = client.render(output / f"{mode}-disabled.png", surface=mode, sky_preview=False)
                check(enabled == disabled, f"{mode} remains unchanged")
            for projection in ["top", "front", "side"]:
                planar = {"projection": projection, "center": [0, 0, 0], "ortho_height": 700}
                enabled = client.render(output / f"{projection}-enabled.png", camera=planar, sky_preview=True)
                disabled = client.render(output / f"{projection}-disabled.png", camera=planar, sky_preview=False)
                check(enabled == disabled, f"{projection} remains unchanged")
            hidden = client.render(output / "tools-hidden-sky.png", tool_textures=False, sky_preview=True)
            check(hidden == client.render(output / "tools-hidden.png", tool_textures=False, sky_preview=False), "Tool texture filtering still hides sky surfaces")
            client.call("create_block", {"mins": [300, -100, -100], "maxs": [340, 100, 100], "material": "brick/brickwall001a"})
            client.call("select", {"ids": []})
            check(client.render(output / "beyond-sky.png", width=641, height=641) == images["rt"], "Sky surfaces occlude geometry beyond their depth")
            client.call("create_block", {"mins": [64, -32, -32], "maxs": [96, 32, 32], "material": "brick/brickwall001a"})
            client.call("select", {"ids": []})
            check(client.render(output / "foreground.png", width=641, height=641) != images["rt"], "Foreground geometry occludes sky")
            client.call("undo")
            set_sky(client, "hammerr_missing_sky_preview_fixture")
            missing = client.call("get_map_info")["skybox"]
            check(not missing["ready"] and all(face["issue"] for face in missing["faces"]), "Missing sky faces have explicit diagnostics")
            check(client.render(output / "missing-enabled.png", sky_preview=True) == client.render(output / "missing-disabled.png", sky_preview=False), "Unavailable skies retain the tool surface")
            client.call("undo")
            check(client.call("get_map_info")["skybox"]["name"] == sky.lower(), "Undo restores the selected sky")
            client.call("save_map", {"path": str(output / "sky-preview.vmf")})
            client.call("open_map", {"path": str(output / "sky-preview.vmf")})
            check(client.call("get_map_info")["skybox"]["ready"], "Sky resolves after save and reopen")
        finally:
            client.call("set_spatial_settings", original["spatial"])
    result = {"transport": "http" if client.url else "stdio", "sky": sky, "checks": len(checks), "passed": checks}
    (output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({key: value for key, value in result.items() if key != "passed"}))


def main():
    parser = argparse.ArgumentParser(description="Validate 2D sky preview through MCP without desktop input.")
    parser.add_argument("--url")
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument("--gameinfo", type=Path)
    parser.add_argument("--sky", default="sky_dust")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    client = Client(args.url, args.binary.resolve(), output, args.gameinfo)
    try:
        validate(client, output, args.sky)
    finally:
        client.close()


if __name__ == "__main__":
    main()
