import argparse
import base64
import hashlib
import json
import math
from pathlib import Path

from validate_bump_lighting import texture
from validate_face_lighting import DEFAULT_BINARY, Client, detail, face_ref
from validate_texture_chain import uv


def fixture(output):
    game = output / "game"
    materials = game / "materials/audit"
    materials.mkdir(parents=True)
    colors = [(210, 70, 48, 255), (55, 170, 220, 255), (75, 190, 100, 255), (225, 175, 55, 255)]
    pixels = bytes(channel for y in range(64) for x in range(64)
                   for channel in colors[(y >= 32) * 2 + (x >= 32)])
    (materials / "atlas.vtf").write_bytes(texture(pixels, size=64))
    (materials / "atlas.vmt").write_text("LightmappedGeneric { $basetexture audit/atlas }\n")
    (materials / "substitute.vmt").write_text("LightmappedGeneric { $basetexture audit/atlas %tooltexture audit/other }\n")
    (materials / "transform.vmt").write_text('LightmappedGeneric { $basetexture audit/atlas $basetexturetransform "scale 2 1" }\n')
    gameinfo = game / "gameinfo.txt"
    gameinfo.write_text('GameInfo { FileSystem { SearchPaths { game "|gameinfo_path|." } } }\n')
    return gameinfo, materials


def inspect(client, output):
    result = client.rpc("tools/call", {"name": "inspect_material_atlas", "arguments": {"material": "audit/atlas"}})
    assert not result.get("isError"), result
    report = json.loads(next(item["text"] for item in result["content"] if item["type"] == "text"))
    image = base64.b64decode(next(item["data"] for item in result["content"] if item["type"] == "image"))
    assert image[:8] == b"\x89PNG\r\n\x1a\n"
    (output / "atlas.png").write_bytes(image)
    assert [report["width"], report["height"]] == [64, 64]
    return report


def validate(args):
    gameinfo, materials = fixture(args.output)
    client = Client(None, args.binary, args.output, gameinfo=gameinfo)
    checks = []
    try:
        schemas = {tool["name"]: tool for tool in client.rpc("tools/list", {})["tools"]}
        assert schemas["inspect_material_atlas"]["inputSchema"]["additionalProperties"] is False
        assert schemas["preview_atlas_fit"]["inputSchema"]["additionalProperties"] is False
        ids = [client.call("create_block", {"mins": [x, 0, 0], "maxs": [x + 128, 64, 16], "material": "audit/atlas"})["id"] for x in [0, 256]]
        faces = [face_ref(brush, next(face for face in brush["faces"] if face["normal"] == [0, 0, 1])) for brush in detail(client, ids)]
        client.call("tie_to_entity", {"ids": [ids[1]], "classname": "func_brush"})
        client.call("select_faces", {"faces": faces})
        client.call("edit_faces", {"faces": faces, "rotation": 37, "scale_u": -0.5, "lightmap_scale": 8, "smoothing_groups": [1, 4, 32]})
        before = client.call("get_face_selection")["faces"]
        history = client.call("get_map_info")["undo"]
        camera = client.call("get_camera")
        source = args.output / "source.vmf"
        client.call("save_map", {"path": str(source)})
        source_hash = hashlib.sha256(source.read_bytes()).hexdigest()
        texture_hash = hashlib.sha256((materials / "atlas.vtf").read_bytes()).hexdigest()
        atlas = inspect(client, args.output)
        assert not client.call("get_map_info")["rendering"]["initialized"]
        checks.append("Strict discoverable MCP calls inspect actual atlas dimensions and CPU PNG without graphics startup")
        for material in ["audit/substitute", "audit/transform"]:
            client.call("inspect_material_atlas", {"material": material}, error=True)
        checks.append("Substituted editor images and nonidentity base transforms reject explicitly")
        arguments = {"material": "audit/atlas", "rectangle": {"x": 32, "y": 0, "width": 32, "height": 32}, "snapshot": atlas["snapshot"]}
        preview = client.call("preview_atlas_fit", arguments)
        token = preview["atlas"]["preview"]["token"]
        assert preview["atlas"]["preview"]["uv_bounds"] == [[0.5, 0.0], [1.0, 0.5]]
        committed = client.call("get_face_selection")["faces"]
        assert [{key: value for key, value in face.items() if key != "preview"} for face in committed] == [{key: value for key, value in face.items() if key != "preview"} for face in before]
        for face, original, x in zip(preview["state"]["preview_faces"], before, [0, 256], strict=True):
            points = [[x, 0, 16], [x + 128, 0, 16], [x + 128, 64, 16], [x, 64, 16]]
            coordinates = [uv(face, point) for point in points]
            for axis, lo, hi in [(0, 32, 64), (1, 0, 32)]:
                assert math.isclose(min(value[axis] for value in coordinates), lo, abs_tol=1e-8)
                assert math.isclose(max(value[axis] for value in coordinates), hi, abs_tol=1e-8)
            for key in ["u_axis", "v_axis", "rotation", "lightmap_scale", "smoothing_groups"]:
                assert face[key] == original[key], key
        assert client.call("get_map_info")["undo"] == history
        checks.append("Rotated mirrored world and brush-entity faces fit the exact selected UV rectangle without lighting or history changes")
        for rectangle in [{"x": 0, "y": 0, "width": 0, "height": 1}, {"x": 63, "y": 0, "width": 2, "height": 1}]:
            client.call("preview_atlas_fit", {**arguments, "rectangle": rectangle, "token": token}, error=True)
            assert client.call("get_face_selection")["preview_token"] == token
        client.call("commit_face_preview", {}, error=True)
        client.call("edit_faces", {"faces": faces, "shift_u": 123}, error=True)
        client.call("cancel_face_preview", {"token": str(int(token) + 1)}, error=True)
        assert client.call("get_face_selection")["preview_token"] == token
        client.call("cancel_face_preview", {"token": token})
        assert client.call("get_face_selection")["faces"] == before
        checks.append("Invalid requests, absent commit token and stale Cancel preserve the owned draft; Cancel restores committed faces")
        preview = client.call("preview_atlas_fit", arguments)
        client.call("commit_face_preview", {"token": preview["atlas"]["preview"]["token"]})
        applied = client.call("get_face_selection")["faces"]
        assert len(client.call("get_map_info")["undo"]) == len(history) + 1
        client.call("undo")
        assert client.call("get_face_selection")["faces"] == before
        client.call("redo")
        assert client.call("get_face_selection")["faces"] == applied
        client.call("undo")
        checks.append("Apply commits the reviewed preview in one Undo step with exact Undo/Redo")
        preview = client.call("preview_atlas_fit", arguments)
        token = preview["atlas"]["preview"]["token"]
        (materials / "atlas.vmt").write_text('LightmappedGeneric { $basetexture audit/atlas $color "[1 0 0]" }\n')
        error = client.call("inspect_material_atlas", {"material": "audit/atlas"}, error=True)
        assert "Reload material assets" in error["content"][0]["text"]
        client.call("commit_face_preview", {"token": token}, error=True)
        assert client.call("get_face_selection")["preview_token"] == token
        client.call("reload_material_assets")
        client.call("commit_face_preview", {"token": token}, error=True)
        client.call("cancel_face_preview", {"token": token})
        checks.append("Changed cached declarations and material reload cannot bypass the shared face commit guard")
        assert client.call("get_face_selection")["faces"] == before
        assert client.call("get_camera") == camera
        assert hashlib.sha256(source.read_bytes()).hexdigest() == source_hash
        assert hashlib.sha256((materials / "atlas.vtf").read_bytes()).hexdigest() == texture_hash
        assert not client.call("get_map_info")["rendering"]["initialized"]
        checks.append("Source VMF, source texture, camera and graphics initialization remain unchanged")
        (args.output / "preview.json").write_text(json.dumps(preview, indent=2) + "\n")
        result = {"checks": len(checks), "passed": checks, "atlas": atlas}
        (args.output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps({"checks": len(checks)}))
    finally:
        client.close()


def main():
    parser = argparse.ArgumentParser(description="Validate shared atlas-region face fits through actual MCP without a game or graphics launch.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output = args.output.resolve()
    args.binary = args.binary.resolve()
    args.output.mkdir(parents=True, exist_ok=False)
    validate(args)


if __name__ == "__main__":
    main()
