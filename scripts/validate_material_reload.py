import argparse
import json
from pathlib import Path
from unittest.mock import patch

from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_material_failures import fixture
from validate_sky_preview import image_difference, rgba_png, set_sky


def validate(client, output, materials):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    set_sky(client, "")
    brush = client.call("create_block", {"mins": [0, 0, 0], "maxs": [128, 128, 32], "material": "audit/good"})["id"]
    client.call("select", {"ids": []})
    source = output / "source.vmf"
    client.call("save_map", {"path": str(source)})
    saved = source.read_bytes()
    before = detail(client, [brush])
    history = client.call("get_map_info")["undo"]
    camera = {"projection": "top", "center": [64, 64, 16], "ortho_height": 256}
    client.call("set_camera", camera)
    client.call("browse_materials")

    def render(name):
        path = output / f"{name}.png"
        data, metadata = client.render_result(width=256, height=256, camera=camera)
        path.write_bytes(data)
        return path, metadata

    def thumbnail():
        response = client.rpc("tools/call", {"name": "preview_material", "arguments": {"material": "audit/good"}})
        assert not response.get("isError"), response
        return next(item["data"] for item in response["content"] if item["type"] == "image")

    initial, _ = render("initial")
    initial_thumbnail = thumbnail()
    texture = materials / "good.vtf"
    original_texture = texture.read_bytes()
    changed = bytearray(original_texture)
    changed[80:] = bytes([15, 30, 240, 255])
    texture.write_bytes(changed)
    (materials / "new.vmt").write_text("LightmappedGeneric { $basetexture audit/good }")
    check(thumbnail() == initial_thumbnail, "Warm material remains cached before explicit reload")
    first = client.call("reload_material_assets")
    refreshed, _ = render("refreshed")
    pixels = rgba_png(refreshed)
    offset = (128 * 256 + 128) * 4
    check(first["failed"] == 0 and first["reloaded"] >= 1, "Reload reports refreshed cached assets")
    check(client.call("get_map_info")["material_reload"] == first, "CPU map information exposes the last reload report without rendering")
    check(image_difference(initial, refreshed) > 50 and pixels[offset + 2] > pixels[offset] + 50, "Persistent offscreen renderer uploads new VTF pixels")
    check(thumbnail() != initial_thumbnail, "Material thumbnail uses refreshed cached pixels")
    check(any(row["name"] == "audit/new" for row in client.call("browse_materials")["entries"]), "Reload rescans new mounted material names")
    texture.write_bytes(b"broken VTF")
    broken = client.call("reload_material_assets")
    failed, metadata = render("failed")
    check(broken["failed"] == 1 and "good.vtf" in broken["failures"][0]["error"], "Reload exposes the malformed asset path and error")
    check(image_difference(refreshed, failed) > 50 and metadata["material_reload"]["failed"] == 1, "Failed reload replaces stale GPU texture and exposes render metadata diagnostics")
    client.call("preview_material", {"material": "audit/good"}, error=True)
    texture.write_bytes(original_texture)
    (materials / "new.vmt").unlink()
    repaired = client.call("reload_material_assets")
    restored, metadata = render("repaired")
    check(repaired["failed"] == 0 and metadata["material_reload"]["failed"] == 0 and image_difference(initial, restored) == 0, "Repair restores original pixels through the same renderer")
    check(thumbnail() == initial_thumbnail, "Repaired thumbnail clears prior asset failure")
    check(not any(row["name"] == "audit/new" for row in client.call("browse_materials")["entries"]), "Reload removes deleted catalog names")
    check(source.read_bytes() == saved and detail(client, [brush]) == before and client.call("get_map_info")["undo"] == history, "Reload preserves source and undo history")
    report = {"passed": len(checks), "checks": checks, "desktop_input": False}
    (output / "validation.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"passed": len(checks), "output": str(output)}))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True)
    gameinfo, materials = fixture(args.output)
    with patch.dict("os.environ", {
        "HAMMERR_RECENT_MAPS_DIR": str(args.output / "recent"),
        "HAMMERR_BUILD_RECIPES": str(args.output / "recipes.json"),
    }):
        client = Client(None, args.binary.resolve(), args.output, gameinfo=gameinfo)
    try:
        validate(client, args.output, materials)
    finally:
        client.close()


if __name__ == "__main__":
    main()
