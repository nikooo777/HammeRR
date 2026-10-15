import argparse
import json
import struct
from pathlib import Path
from unittest.mock import patch

from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_material_failures import fixture
from validate_model_browser import image as model_image
from validate_prop_shadows import fixture as model_fixture
from validate_sky_preview import image_difference, rgba_png, set_sky


def model_checks(client, output, materials, check):
    path = "models/props/test.mdl"
    entity = client.call("create_entity", {"classname": "prop_dynamic", "origin": [64, 64, 64], "properties": {"model": path}})["id"]
    client.call("select", {"ids": []})
    source = output / "model-source.vmf"
    client.call("save_map", {"path": str(source)})
    saved = source.read_bytes()
    before = detail(client, [entity])
    history = client.call("get_map_info")["undo"]
    camera = client.call("get_camera")
    initial = client.call("inspect_entity_model", {"id": entity})
    geometry = {key: initial["asset"][key] for key in ["vertices", "triangles", "meshes", "local_bounds", "hull_bounds", "bodygroups", "attachments"]}
    choice = client.call("begin_model_choice", {"id": entity})["token"]
    client.call("choose_model", {"token": choice, "path": path})
    first = model_image(client, output / "model-fallback.png", path, 0)
    missing = model_image(client, output / "model-missing-skin.png", path, 1)
    original_view = output / "model-fallback-viewport.png"
    original_view.write_bytes(client.render_result(width=256, height=256)[0])
    check(initial["asset"]["materials"] == ["one"] and client.call("inspect_model", {"path": path, "skin": 1})["materials"] == ["two"], "Cached model initially uses its existing bare-name material policy")
    preferred = materials.parent / "models/test/one.vmt"
    repaired = materials.parent / "models/test/two.vmt"
    blue = bytearray((materials / "good.vtf").read_bytes())
    blue[80:] = bytes([15, 30, 240, 255])
    (materials / "blue.vtf").write_bytes(blue)
    preferred.write_text("VertexLitGeneric { $basetexture audit/blue }")
    repaired.write_text("VertexLitGeneric { $basetexture audit/good }")
    check(model_image(client, output / "model-before-reload.png", path, 0) == first, "New preferred model material waits for explicit reload")
    report = client.call("reload_material_assets")
    updated = client.call("inspect_entity_model", {"id": entity})
    check(report["models_refreshed"] == report["models_changed"] == 1 and updated["asset"]["materials"] == ["models/test/one"], "Material reload refreshes the cached model directory winner")
    check(client.call("get_model_choice", {"token": choice})["asset"]["materials"] == ["models/test/one"], "Current model browser choice observes refreshed paths without replacement")
    check(model_image(client, output / "model-preferred.png", path, 0) != first and model_image(client, output / "model-repaired-skin.png", path, 1) != missing, "Isolated model renderer refreshes preferred paths and a previously missing skin")
    current_view = output / "model-preferred-viewport.png"
    current_view.write_bytes(client.render_result(width=256, height=256)[0])
    check(image_difference(original_view, current_view) > 50, "Persistent viewport renderer uploads the newly selected model material")
    picked = client.call("pick_at_pixel", {"width": 256, "height": 256, "x": 128, "y": 128})
    check(picked["object"] == entity, "Refreshed model keeps its actual viewport mesh picking")
    preferred.unlink()
    client.call("reload_material_assets")
    check(client.call("inspect_model", {"path": path})["materials"] == ["one"] and model_image(client, output / "model-deleted-preferred.png", path, 0) == first, "Deleting the preferred VMT restores the existing bare-name winner")
    preferred.symlink_to("one.vmt")
    failed = client.call("reload_material_assets")
    unavailable = client.call("inspect_entity_model", {"id": entity})
    check(any(failure["material"] == path for failure in failed["failures"]) and unavailable["asset"]["material_error"] and unavailable["asset"]["materials"] == [], "Resolution I/O failure reports the model path and discards prior healthy bindings")
    check(unavailable["displayed"] and unavailable["world_bounds"] == initial["world_bounds"] and all(unavailable["asset"][key] == value for key, value in geometry.items()), "Unavailable model materials preserve decoded geometry, bounds and attachments")
    check(model_image(client, output / "model-resolution-error.png", path, 0) != first, "Resolution failure displays missing textures without a healthy fallback")
    check(client.call("get_model_choice", {"token": choice})["asset"]["material_error"], "Current model choice remains inspectable while materials are unavailable")
    preferred.unlink()
    preferred.write_text("VertexLitGeneric { $basetexture audit/blue }")
    client.call("reload_material_assets")
    check(client.call("inspect_entity_model", {"id": entity})["asset"]["material_error"] is None, "Repair clears cached model material resolution errors")
    client.call("select", {"ids": [entity]})
    client.call("reload_material_assets")
    client.call("apply_model_choice", {"token": choice}, error=True)
    client.call("cancel_model_choice", {"token": choice})
    client.call("transform_objects", {"ids": [entity], "translation": [8, 0, 0], "preview": True})
    draft = client.call("get_transform_handles")
    client.call("reload_material_assets")
    check(client.call("get_transform_handles") == draft, "Material path refresh retains the active geometry draft and bounds")
    check(source.read_bytes() == saved and detail(client, [entity]) == before and client.call("get_map_info")["undo"] == history and client.call("get_camera") == camera, "Model material refresh preserves authored source, history and camera")
    client.call("commit_transform_preview")
    check(client.call("get_map_info")["undo"] == ["move 1 objects", *history], "Preserved transform draft still applies as one Undo step")
    client.call("undo")
    check(detail(client, [entity]) == before, "Undo restores the model source after material refresh")


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
    model_checks(client, output, materials, check)
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
    model_fixture(gameinfo.parent)
    indices = gameinfo.parent / "models/props/test.dx90.vtx"
    geometry = bytearray(indices.read_bytes())
    struct.pack_into("<3H", geometry, 134, 0, 2, 1)
    indices.write_bytes(geometry)
    for name in ["one", "two"]:
        (materials.parent / f"models/test/{name}.vmt").unlink()
    (materials.parent / "one.vmt").write_text("VertexLitGeneric { $basetexture audit/good }")
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
