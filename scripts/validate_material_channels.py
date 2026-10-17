import argparse
import base64
import hashlib
import json
import struct
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client
from validate_sky_preview import rgba_png


def texture(colors):
    header = bytearray(80)
    header[:4] = b"VTF\0"
    struct.pack_into("<IIIHH", header, 4, 7, 2, 80, 4, 4)
    for offset in [24, 63]:
        struct.pack_into("<H", header, offset, 1)
    header[56] = 1
    struct.pack_into("<i", header, 57, -1)
    return bytes(header) + bytes(channel for _ in range(4) for pixel in colors for channel in pixel)


def validate(args):
    game = args.output / "game"
    folder = game / "materials/audit"
    folder.mkdir(parents=True)
    source = folder / "surface.vmt"
    source.write_text("UnlitGeneric { $basetexture audit/first }\n")
    colors = [[255, 0, 0, 0], [0, 255, 0, 64], [0, 0, 255, 128], [255, 255, 0, 255]]
    first = texture(colors)
    second = texture([[11, 22, 33, 47]] * 4)
    (folder / "first.vtf").write_bytes(first)
    (folder / "second.vtf").write_bytes(second)
    gameinfo = game / "gameinfo.txt"
    gameinfo.write_text('GameInfo { FileSystem { SearchPaths { game "|gameinfo_path|." } } }\n')
    client = Client(None, args.binary.resolve(), args.output, gameinfo)
    checks = []
    receipts = []

    def inspect(name, **arguments):
        before = {path: path.read_bytes() for path in folder.iterdir()}
        result = client.rpc("tools/call", {"name": "preview_material", "arguments": {"material": "audit/surface", **arguments}})
        assert not result.get("isError"), result
        report = json.loads(next(item["text"] for item in result["content"] if item["type"] == "text"))
        images = [item for item in result["content"] if item["type"] == "image"]
        image = base64.b64decode(images[0]["data"]) if images else None
        if image is not None:
            path = args.output / f"{name}.png"
            path.write_bytes(image)
            receipts.append({"file": path.name, "sha256": hashlib.sha256(image).hexdigest(), "report": report})
        assert all(path.read_bytes() == data for path, data in before.items())
        return image, report

    try:
        schemas = {tool["name"]: tool["inputSchema"] for tool in client.rpc("tools/list", {})["tools"]}
        assert "channel" in schemas["preview_material"]["properties"]
        block = client.call("create_block", {"mins": [0, 0, 0], "maxs": [32, 32, 32], "material": "audit/surface"})["id"]
        client.call("get_normal_map_preview")
        client.call("transform_objects", {"ids": [block], "translation": [16, 0, 0], "preview": True})
        original = client.call("get_map_info")
        camera = client.call("get_camera")
        objects = client.call("get_objects", {"ids": [block]})
        default, color_report = inspect("material-default")
        color, _ = inspect("material-color", channel="color")
        alpha, alpha_report = inspect("material-alpha", channel="alpha")
        assert default == color and alpha != color
        assert bytes(rgba_png(args.output / "material-color.png")) == first[80:]
        expected_alpha = bytes(value for _ in range(4) for pixel in colors for value in [pixel[3], pixel[3], pixel[3], 255])
        assert bytes(rgba_png(args.output / "material-alpha.png")) == expected_alpha
        assert alpha_report["alpha_range"] == [0, 255]
        assert alpha_report["source"] == color_report["source"]
        assert alpha_report["source"]["texture"] == "materials/audit/first.vtf"
        assert alpha_report["source"]["cached"]
        checks.append("default Color PNG unchanged; Alpha equals decoded source bytes with opaque output")
        source.write_text("LightmappedGeneric { $basetexture audit/second }\n")
        retained, retained_report = inspect("material-alpha-retained", channel="alpha")
        assert retained == alpha and retained_report["source"] == alpha_report["source"]
        client.call("reload_material_assets")
        refreshed, refreshed_report = inspect("material-alpha-reloaded", channel="alpha")
        assert refreshed != alpha
        assert refreshed_report["alpha_range"] == [47, 47]
        assert refreshed_report["source"]["texture"] == "materials/audit/second.vtf"
        assert refreshed_report["source"]["material_identity"] != alpha_report["source"]["material_identity"]
        checks.append("cached VMT/pixels remain paired until explicit material reload")
        image, missing = inspect("missing", material="audit/absent", channel="alpha")
        assert image is None and missing["missing"] and missing["source"] is None
        for channel in ["opacity", "Alpha", 1, True]:
            try:
                result = client.rpc("tools/call", {"name": "preview_material", "arguments": {"material": "audit/surface", "channel": channel}})
            except RuntimeError:
                continue
            assert result.get("isError"), result
        (folder / "second.vtf").write_bytes(b"invalid VTF")
        client.call("reload_material_assets")
        failed = client.call("preview_material", {"material": "audit/surface", "channel": "alpha"}, error=True)
        assert not any(item["type"] == "image" for item in failed["content"])
        (folder / "second.vtf").write_bytes(second)
        client.call("reload_material_assets")
        assert inspect("material-alpha-repaired", channel="alpha")[0] == refreshed
        checks.append("missing sources and decode errors stay explicit; invalid channels reject; reload repairs failure")
        current = client.call("get_map_info")
        for key in ["path", "unsaved_changes", "world_brushes", "undo", "redo", "selection", "view", "transform"]:
            assert current[key] == original[key], key
        assert not current["rendering"]["initialized"]
        assert client.call("get_camera") == camera
        assert client.call("get_objects", {"ids": [block]}) == objects
        checks.append("source, history, selection, camera and owned transform remain unchanged without graphics startup")
    finally:
        client.close()
    result = {"material_channels": True, "checks": checks, "images": receipts}
    (args.output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    validate(args)


if __name__ == "__main__":
    main()
