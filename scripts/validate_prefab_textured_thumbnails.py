import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
from unittest.mock import patch

from validate_bump_lighting import texture
from validate_face_lighting import DEFAULT_BINARY, Client, detail


def fixture(output):
    game = output / "game"
    materials = game / "materials/audit"
    materials.mkdir(parents=True)
    pixels = bytes(channel for y in range(64) for x in range(64)
                   for channel in ([220, 60, 40, 255] if (x // 8 + y // 8) % 2 else [40, 150, 220, 255]))
    (materials / "atlas.vtf").write_bytes(texture(pixels, size=64))
    (materials / "base.vmt").write_text("LightmappedGeneric { $basetexture audit/atlas }\n")
    (materials / "atlas.vmt").write_text('Patch { include "materials/audit/base.vmt" insert { $surfaceprop concrete } }\n')
    gameinfo = game / "gameinfo.txt"
    gameinfo.write_text('GameInfo { FileSystem { SearchPaths { game "|gameinfo_path|." } } }\n')
    return gameinfo, materials


def render(client, style=None, refresh=False):
    arguments = {"name": "Textured assembly", "refresh": refresh}
    if style is not None:
        arguments["style"] = style
    result = client.rpc("tools/call", {"name": "render_prefab_thumbnail", "arguments": arguments})
    assert not result.get("isError"), result
    report = json.loads(next(item["text"] for item in result["content"] if item["type"] == "text"))
    png = base64.b64decode(next(item["data"] for item in result["content"] if item["type"] == "image"))
    assert png.startswith(b"\x89PNG\r\n\x1a\n")
    return png, report


def validate(args):
    gameinfo, materials = fixture(args.output)
    library = args.output / "library"
    session_dir = args.output / "session"
    session_dir.mkdir()
    with patch.dict(os.environ, {"HAMMERR_PREFABS_DIR": str(library), "HAMMERR_RECENT_MAPS_DIR": str(session_dir / "recent"), "HAMMERR_BUILD_RECIPES": str(session_dir / "recipes.json")}):
        client = Client(None, args.binary, session_dir, gameinfo=gameinfo)
    checks = []
    try:
        schemas = {tool["name"]: tool["inputSchema"] for tool in client.rpc("tools/list", {})["tools"]}
        for name in ["inspect_prefab_thumbnail", "render_prefab_thumbnail"]:
            assert schemas[name]["additionalProperties"] is False
            assert "style" in schemas[name]["properties"]
        brush = client.call("create_block", {"mins": [0, 0, 0], "maxs": [96, 64, 24], "material": "audit/atlas"})["id"]
        client.call("select", {"ids": [brush]})
        client.call("save_prefab", {"name": "Textured assembly", "snapshot": client.call("get_prefabs")["snapshot"]})
        source = library / "Textured assembly.vmf"
        source_hash = hashlib.sha256(source.read_bytes()).hexdigest()
        flat = client.call("inspect_prefab_thumbnail", {"name": "Textured assembly"})
        assert flat["style"] == "flat_geometry" and flat["textures_loaded"] is False
        textured = client.call("inspect_prefab_thumbnail", {"name": "Textured assembly", "style": "textured"})
        assert "error" not in textured, textured
        assert textured["style"] == "textured" and textured["textures_loaded"] is True
        assert [textured["texture_assets"]["bindings"][0][key] for key in ["width", "height"]] == [64, 64]
        assert len(textured["texture_assets"]["sources"]) == 3
        assert all(len(item["sha256"]) == 64 for item in textured["texture_assets"]["sources"])
        assert textured["graphics"]["initialized"] is False
        checks.append("Default Flat and explicit Textured share bounded CPU inspection, exact dimensions and Patch/texture fingerprints without graphics")
        client.call("preview_prefab", {"name": "Textured assembly", "position": [160, 0, 0], "snapshot": client.call("get_prefabs")["snapshot"]})
        clipboard = client.call("get_clipboard")
        history = client.call("get_map_info")["undo"]
        camera = client.call("get_camera")
        geometry = detail(client, [brush])
        base = materials / "base.vmt"
        original_material = base.read_bytes()
        base.write_text("LightmappedGeneric { $basetexture audit/missing }\n")
        failed = client.call("render_prefab_thumbnail", {"name": "Textured assembly", "style": "textured"}, error=True)
        assert all(item["type"] != "image" for item in failed["content"])
        failure = client.call("inspect_prefab_thumbnail", {"name": "Textured assembly", "style": "textured"})
        assert "missing" in failure["error"] and failure["graphics"]["initialized"] is False
        assert "error" not in client.call("inspect_prefab_thumbnail", {"name": "Textured assembly", "style": "flat"})
        base.write_bytes(original_material)
        checks.append("Failed Textured material returns no substitute image and leaves explicit Flat available before graphics startup")
        if not args.cpu_only:
            flat_png, flat = render(client)
            textured_png, first = render(client, "textured")
            assert first["style"] == "textured" and textured_png != flat_png
            assert first["normal_maps"] is False and first["authored_lighting"] is False
            cached_png, cached = render(client, "textured")
            assert cached["cached"] is True and cached_png == textured_png
            refreshed_png, refreshed = render(client, "textured", refresh=True)
            assert refreshed["cached"] is False and refreshed_png == textured_png
            assert render(client)[0] == flat_png
            (args.output / "prefab-thumbnail-flat.png").write_bytes(flat_png)
            (args.output / "prefab-thumbnail-textured.png").write_bytes(textured_png)
            checks.append("Actual shared renderer gives different Flat/Textured pixels, independent cache hits and deterministic explicit Refresh")
            vtf = materials / "atlas.vtf"
            original_texture = vtf.read_bytes()
            previous = vtf.stat()
            changed = bytearray(original_texture)
            changed[80:] = bytes([70, 210, 60, 255]) * (64 * 64)
            vtf.write_bytes(changed)
            os.utime(vtf, ns=(previous.st_atime_ns, previous.st_mtime_ns))
            assert vtf.stat().st_size == previous.st_size and vtf.stat().st_mtime_ns == previous.st_mtime_ns
            failed = client.call("render_prefab_thumbnail", {"name": "Textured assembly", "style": "textured"}, error=True)
            assert all(item["type"] != "image" for item in failed["content"])
            assert render(client)[0] == flat_png
            changed_png, refreshed = render(client, "textured", refresh=True)
            assert changed_png != textured_png and refreshed["cached"] is False
            vtf.write_bytes(original_texture)
            render(client, "textured", refresh=True)
            (args.output / "prefab-thumbnail-changed-texture.png").write_bytes(changed_png)
            checks.append("Same-size/mtime VTF edits invalidate only Textured success and explicit Refresh admits current pixels")
            original_material = base.read_bytes()
            previous = base.stat()
            base.write_bytes(original_material.replace(b"LightmappedGeneric", b"lightmappedgeneric"))
            os.utime(base, ns=(previous.st_atime_ns, previous.st_mtime_ns))
            failed = client.call("render_prefab_thumbnail", {"name": "Textured assembly", "style": "textured"}, error=True)
            assert all(item["type"] != "image" for item in failed["content"])
            base.write_bytes(original_material)
            render(client, "textured", refresh=True)
            checks.append("Patch include byte changes invalidate cached Textured images even when semantics and metadata match")
        else:
            assert client.call("inspect_prefab_thumbnail", {"name": "Textured assembly", "style": "textured"})["graphics"]["initialized"] is False
        assert client.call("get_clipboard") == clipboard
        assert client.call("get_map_info")["undo"] == history
        assert client.call("get_camera") == camera
        assert detail(client, [brush]) == geometry
        assert hashlib.sha256(source.read_bytes()).hexdigest() == source_hash
        checks.append("Inspection, style changes, rendering, failures and Refresh preserve committed geometry, source VMF, owned placement, clipboard, Undo and camera")
        result = {"checks": checks, "passed": len(checks), "graphics": not args.cpu_only, "desktop_input": False}
        (args.output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps({"passed": len(checks)}))
    finally:
        client.close()


def main():
    parser = argparse.ArgumentParser(description="Validate opt-in Textured prefab thumbnails through actual isolated MCP.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--cpu-only", action="store_true")
    args = parser.parse_args()
    args.binary = args.binary.resolve()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=False)
    validate(args)


if __name__ == "__main__":
    main()
