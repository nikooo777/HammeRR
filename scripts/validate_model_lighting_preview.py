import argparse
import hashlib
import json
import struct
from pathlib import Path
from unittest.mock import patch

from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_normal_map_preview import texture
from validate_prop_shadows import fixture
from validate_sky_preview import rgba_png

MODEL = "models/props/test.mdl"
CAMERA = {"projection": "top", "center": [-60, 0, 0], "ortho_height": 144}


def assets(output):
    game = output / "game"
    fixture(game)
    vvd_path = game / "models/props/test.vvd"
    vvd = bytearray(vvd_path.read_bytes())
    for index, point in enumerate([(-24, -24, 0), (24, -24, 0), (24, 24, 0), (-24, 24, 0)]):
        struct.pack_into("<3f", vvd, 64 + index * 48 + 16, *point)
    struct.pack_into("<3f", vvd, 64 + 2 * 48 + 28, 0.6, 0, 0.8)
    vvd_path.write_bytes(vvd)
    vtx_path = game / "models/props/test.dx90.vtx"
    vtx = bytearray(vtx_path.read_bytes())
    struct.pack_into("<i", vtx, 146, 6)
    struct.pack_into("<6H", vtx, 134, 0, 2, 1, 0, 3, 2)
    vtx_path.write_bytes(vtx)
    (game / "materials/test.vtf").write_bytes(texture([255, 255, 255, 255]))
    material = game / "materials/models/test/one.vmt"
    material.write_text("VertexLitGeneric { $basetexture test }\n")
    (game / "materials/models/test/two.vmt").write_text("VertexLitGeneric { $basetexture test $phong 0 }\n")
    fgd = output / "model-lighting.fgd"
    fgd.write_text('@PointClass size(-2 -2 -2, 2 2 2) = light []\n@PointClass = prop_dynamic [ model(studio): "Model" ]\n')
    return game / "gameinfo.txt", material, fgd


def validate(client, output, material, fgd, result):
    checks = result["checks"]

    def check(condition, reason):
        if not condition:
            raise AssertionError(reason)
        checks.append(reason)

    def render(name, **options):
        png, metadata = client.render_result(width=401, height=401, camera=CAMERA,
            selection_outlines=False, fog_preview=False, **options)
        path = output / f"{name}.png"
        path.write_bytes(png)
        (output / f"{name}.metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
        result["images"].append({"file": path.name, "sha256": hashlib.sha256(png).hexdigest()})
        return rgba_png(path), metadata["lighting_preview"]["report"]

    client.call("load_entity_definitions", {"path": str(fgd)})
    model = client.call("create_entity", {"classname": "prop_dynamic", "origin": [-60, 0, 0],
        "properties": {"model": MODEL, "angles": "30 0 0", "modelscale": "2", "rendercolor": "64 128 255"}})["id"]
    other = client.call("create_entity", {"classname": "prop_dynamic", "origin": [60, 0, 0],
        "properties": {"model": MODEL, "skin": "1"}})["id"]
    light = client.call("create_entity", {"classname": "light", "origin": [-20, 0, 64],
        "properties": {"_light": "255 128 32 1000", "_fifty_percent_distance": "64", "_zero_percent_distance": "160"}})["id"]
    client.call("select", {"ids": []})
    client.call("set_spatial_settings", {"grid_visible": False})
    path = output / "model-lighting.vmf"
    client.call("save_map", {"path": str(path)})
    source = path.read_bytes()
    rows = detail(client, [model, other, light])
    state = client.call("get_map_info")
    camera = client.call("get_camera")
    initial = client.call("get_lighting_preview")
    check(not initial["enabled"] and initial["report"]["models"]["admitted_instances"] == 0
        and not state["rendering"]["initialized"], "Default-off CPU inspection initializes no GPU or model lighting admission")
    client.call("set_view", {"lighting_preview": True})
    report = client.call("get_lighting_preview")["report"]
    check(report["models"] == {"admitted_instances": 1, "excluded_instances": 1,
        "admitted_parts": 1, "excluded_parts": 1, "admitted_triangles": 2, "excluded_triangles": 2}
        and any(row["material"] == "models/test/two" and "$phong" in row["reason"] for row in report["diagnostics"])
        and not client.call("get_map_info")["rendering"]["initialized"],
        "The same CPU path admits transformed tinted models and explicitly rejects the selected unsupported skin")
    client.call("set_view", {"lighting_preview": False})
    off, _ = render("model-lighting-off", lighting_preview=False)
    default, _ = render("model-lighting-default")
    detail_pixels, detail_report = render("model-lighting-detail", lighting_preview=True)
    fast_pixels, fast_report = render("model-lighting-fast", lighting_preview=True, lighting_quality="fast")
    check(default == off and detail_pixels != off and fast_pixels != detail_pixels
        and detail_report["models"] == fast_report["models"],
        "Actual off/Detail/Fast images retain exact default pixels and distinct shared-evaluator lighting")
    check(render("model-lighting-cached", lighting_preview=True)[0] == detail_pixels,
        "An identical temporary render reuses the uploaded scene and retained model coverage")
    generation = detail_report["source"]["material_generation"]
    material.write_text("VertexLitGeneric { $basetexture test $bumpmap no/fresh/lookup }\n")
    cached, cached_report = render("model-lighting-material-cached", lighting_preview=True)
    check(cached == detail_pixels and cached_report["source"]["material_generation"] == generation,
        "Changed VMT bytes cannot replace capability while cached base pixels remain")
    client.call("reload_material_assets")
    rejected, rejected_report = render("model-lighting-normal-rejected", lighting_preview=True)
    ordinary, _ = render("model-lighting-rejected-off", lighting_preview=False)
    check(rejected == ordinary and rejected_report["models"]["admitted_instances"] == 0
        and rejected_report["models"]["excluded_instances"] == 2
        and any("$bumpmap" in row["reason"] for row in rejected_report["diagnostics"]),
        "Material reload explicitly rejects declared model normals and keeps ordinary shading")
    material.write_text("VertexLitGeneric { $basetexture test %compilenodraw 1 }\n")
    client.call("reload_material_assets")
    tool_images = []
    for enabled in (True, False, True):
        pixels, actual = render(f"model-lighting-tool-{len(tool_images)}", lighting_preview=True, tool_textures=enabled)
        check(actual["models"]["excluded_triangles"] == (4 if enabled else 2),
            f"Temporary Tool textures={enabled} reports actual drawn model triangles")
        again, again_report = render(f"model-lighting-tool-cached-{len(tool_images)}", lighting_preview=True, tool_textures=enabled)
        check(again == pixels and again_report == actual, "Cached uploaded report and pixels retain the same tool suppression")
        tool_images.append(pixels)
    check(tool_images[0] != tool_images[1] and tool_images[0] == tool_images[2],
        "Temporary tool suppression rebuilds and restores actual model pixels")
    material.write_text("VertexLitGeneric { $basetexture test }\n")
    client.call("reload_material_assets")
    restored, _ = render("model-lighting-restored", lighting_preview=True)
    check(restored == detail_pixels, "Restoring the checked material restores the same supported image")
    for surface in ("flat", "wireframe", "lightmap_density", "smoothing_groups"):
        baseline, _ = render(f"model-lighting-{surface}-off", surface=surface, lighting_preview=False)
        enabled, report = render(f"model-lighting-{surface}-on", surface=surface, lighting_preview=True)
        check(enabled == baseline and not any(report["models"].values()),
            f"{surface} keeps exact prior pixels and zero model admission")
    committed = detail(client, [model, other, light])
    client.call("transform_objects", {"ids": [model], "translation": [8, 0, 0], "preview": True})
    preview = detail(client, [model, other, light])
    client.call("set_view", {"lighting_preview": True, "lighting_quality": "fast"})
    draft_report = client.call("get_lighting_preview")["report"]
    client.call("set_view", {"lighting_preview": False, "lighting_quality": "detail"})
    check(draft_report["models"]["admitted_instances"] == 1
        and detail(client, [model, other, light]) == preview and path.read_bytes() == source,
        "Lighting controls retain the owned model transform draft and committed source")
    client.call("cancel_transform_preview")
    unchanged = client.call("get_map_info")
    check(detail(client, [model, other, light]) == committed == rows
        and unchanged["view"] == state["view"] and unchanged["undo"] == state["undo"]
        and unchanged["redo"] == state["redo"] and client.call("get_camera") == camera
        and path.read_bytes() == source and detail_report["models"]["admitted_instances"] == 1,
        "Temporary render/admission/reload/Cancel preserve source, history, camera, shared controls and prior retained metadata")
    png, metadata = client.render_result(width=1001, height=701, lighting_preview=True,
        camera={"position": [-180, -220, 150], "look_at": [0, 0, 0], "fov": 50}, selection_outlines=False)
    showcase = output / "model-lighting-perspective.png"
    showcase.write_bytes(png)
    (output / "model-lighting-perspective.metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
    result["images"].append({"file": showcase.name, "sha256": hashlib.sha256(png).hexdigest()})
    result["source_sha256"] = hashlib.sha256(source).hexdigest()


def main():
    parser = argparse.ArgumentParser(description="Validate bounded model authored lighting in a private stdio MCP Session.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve()
    if not binary.is_file():
        parser.error(f"Binary does not exist: {binary}")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    gameinfo, material, fgd = assets(output)
    result = {"status": "running", "binary": str(binary), "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
        "transport": "private_stdio", "desktop_input": False, "native_exercised": False, "checks": [], "images": []}
    client = None
    try:
        with patch.dict("os.environ", {"HAMMERR_RECENT_MAPS_DIR": str(output / "recent"),
            "HAMMERR_BUILD_RECIPES": str(output / "recipes.json"), "HAMMERR_PREFABS_DIR": str(output / "prefabs")}):
            client = Client(None, binary, output, gameinfo=gameinfo)
        validate(client, output, material, fgd, result)
        result["status"] = "passed"
    except Exception as error:
        result["status"] = "failed"
        result["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        if client is not None:
            client.close()
        (output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps({"status": result["status"], "checks": len(result["checks"]), "output": str(output)}))


if __name__ == "__main__":
    main()
