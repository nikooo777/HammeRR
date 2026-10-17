import argparse
import json
import os
from pathlib import Path
from unittest.mock import patch

from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_fgd_icons import SIZE, color_kind, fixture, image_receipt, rgb
from validate_sky_preview import rgba_png
from validate_working_region import configure

FGD = '''
@PointClass size(-2 -2 -2, 2 2 2) sprite() = env_sprite [
    model(sprite) : "Sprite Name" : "sprites/glow01.spr"
    scale(string) : "Scale" : ""
    rendercolor(color255) : "FX Color" : "255 255 255"
    renderamt(integer) : "FX Amount" : 255
    rendermode(integer) : "Render Mode" : 0
    renderfx(integer) : "Render FX" : 0
]
'''


def sprite_material(texture):
    return f'Sprite {{ $basetexture {texture} $spriteorientation vp_parallel $spriteorigin "[0.5 0.5]" }}\n'


def private_fixture(output):
    gameinfo, materials = fixture(output)
    first = materials.parent
    second = first.parent.parent / "second/materials/sprites"
    second.mkdir(parents=True)
    (second / "glow01.vmt").write_text(sprite_material("audit/opaque"))
    for name in ("sprites/glow01", "root", "nested/foo", "materials/foo"):
        path = first / f"{name}.vmt"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(sprite_material("audit/ring"))
    return gameinfo, first


def validate(client, output, materials):
    checks, images = [], []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def report():
        return client.call("get_entity_inspector", {"id": entity})["helpers"]["sprite_resource"]

    def edit(**values):
        client.call("set_entity_properties", {"id": entity, "set": values})

    def render(name, camera, **options):
        png, metadata = client.render_result(width=SIZE, height=SIZE, camera=camera, selection_outlines=False, **options)
        path = output / f"{name}.png"
        path.write_bytes(png)
        images.append(image_receipt(path))
        return rgba_png(path), metadata

    def patches(pixels, offset=75):
        center = SIZE // 2
        return [color_kind(rgb(pixels, center + x, center + y)) for y in (-offset, offset) for x in (-offset, offset)]

    client.call("new_map", {"discard_changes": True})
    fgd = output / "sprites.fgd"
    fgd.write_text(FGD)
    client.call("load_entity_definitions", {"path": str(fgd)})
    client.call("set_view", {"entity_images": True})
    client.call("set_spatial_settings", {"grid_visible": False})
    client.call("set_transform_tool", {"mode": "select"})
    entity = client.call("create_entity", {"classname": "env_sprite", "origin": [0, 0, 0]})["id"]
    blank = report()
    check(blank["state"] == "unavailable" and "Scale" in blank["error"] and blank["image"] is None, "Shipped blank Scale stays visibly unavailable without an inferred default or asset reads")
    edit(scale="2")
    ready = report()
    check(ready["displayed"] and ready["state"] == "ready" and ready["world_size"] == [64, 64], "Positive authored Scale multiplies physical texture dimensions")
    check(ready["model"]["raw"] == "sprites/glow01.spr" and ready["resolved_vmt"] == "materials/sprites/glow01.vmt", "Legacy authored spelling and exact winning VMT identity remain separate")
    check(ready["image"]["source"]["file"] == str(materials / "sprites/glow01.vmt") and ready["image"]["texture_source"]["file"] == str(materials / "audit/ring.vtf"), "Shared guide reports both winning mounted file origins")
    inspector = client.call("get_entity_inspector", {"id": entity})
    fields = {field["key"]: field for field in inspector["properties"]}
    check(fields["model"]["label"] == "Sprite Name" and fields["model"]["control"] == "material" and fields["rendercolor"]["label"] == "FX Color", "Shared inspector routes Sprite Name to material Browse and retains FX Color")
    check(not client.call("get_map_info")["rendering"]["initialized"], "Resource/inspector admission remains lazy and headless")
    region_row = next(row for row in client.call("get_working_region")["objects"] if row["id"] == entity)
    check(region_row["bounds_source"] == "fgd_size" and region_row["bounds_error"] is None, "Explicit Sprite source classification avoids fabricated MDL bounds failures")
    model_error = client.call("inspect_entity_model", {"id": entity}, error=True)
    check("Sprite resource source" in json.dumps(model_error), "Model inspection identifies the Sprite source before any MDL load")
    body_ray = {"origin": [0, -100, 0], "direction": [0, 1, 0]}
    outside_ray = {"origin": [20, -100, 0], "direction": [0, 1, 0]}
    bounds = detail(client, [entity])[0]["bounds"]
    hits = client.call("get_object_hit_candidates", body_ray)
    check(hits["total"] == 1 and client.call("get_object_hit_candidates", outside_ray)["total"] == 0, "Physical image extents do not enlarge ordinary point-helper picks")
    client.call("frame_selection", {"aspect": 1})
    camera = client.call("get_camera")
    source = output / "source.vmf"
    client.call("save_map", {"path": str(source)})
    saved, authored = source.read_bytes(), detail(client, [entity])
    history = client.call("get_history")["undo_count"]
    top = {"projection": "top", "center": [0, 0, 0], "ortho_height": 128}
    ring, _ = render("sprite-ring", top)
    expected = patches(ring)
    check(set(expected) == {"green", "red", "blue", "yellow"}, "Physical Sprite guide contributes the four asymmetric texture-color patches")
    edit(model="materials/audit/opaque.vmt")
    opaque, _ = render("sprite-opaque-control", top)
    client.call("undo")
    absent, metadata = render("sprite-disabled-control", top, entity_images=False)
    check(metadata["view"]["entity_images"] is False and report()["displayed"], "Temporary Entity images override controls scene eligibility without mutating Session")
    center = SIZE // 2
    hole = [(center + x, center + y) for y in range(-35, 36) for x in range(-35, 36) if abs(x) > 8 or abs(y) > 8]
    check(all(rgb(ring, x, y) == rgb(absent, x, y) and rgb(opaque, x, y) != rgb(absent, x, y) for x, y in hole), "Alpha-zero image pixels match the disabled control while the opaque control covers them")
    check(patches(opaque) == expected, "Opaque texture pixels retain the same unlit resource colors")
    for projection in ("top", "front", "side"):
        pixels, _ = render(f"sprite-{projection}", {**top, "projection": projection})
        check(patches(pixels) == expected, f"Shared physical image faces the actual {projection} camera")
    for index, position in enumerate(([-128, 0, 0], [0, -128, 0])):
        pixels, _ = render(f"sprite-perspective-{index}", {"projection": "perspective", "position": position, "look_at": [0, 0, 0], "fov": 60})
        check(set(patches(pixels, 65)) == set(expected), f"Cached image uses live perspective camera axes {index}")
    edit(scale="1")
    smaller, _ = render("sprite-half-scale", top)
    check(report()["world_size"] == [32, 32] and patches(smaller, 37) == expected and all(value is None for value in patches(smaller)), "Changing Scale halves physical image extent while preserving source texels")
    client.call("undo")
    edit(rendercolor="0 0 0", renderamt="0", rendermode="9", renderfx="17", HDRColorScale="100", framerate="0", spawnflags="0", targetname="disabled_runtime_name")
    runtime, _ = render("sprite-runtime-fields-unevaluated", top)
    check(runtime == ring and not report()["runtime_fields_evaluated"], "Runtime tint/amount/mode/FX/HDR/animation/enable fields remain explicitly unevaluated")
    client.call("undo")
    client.call("transform_objects", {"ids": [entity], "translation": [16, 0, 0], "preview": True})
    moved, _ = render("sprite-transform-draft", top)
    check(moved != ring and detail(client, [entity]) == authored, "Guide uses the displayed transform draft without changing committed properties")
    client.call("cancel_transform_preview")
    configure(client, action="set", mins=[8, -8, -8], maxs=[24, 8, 8])
    check(report()["suppression_reason"] == "outside_working_region", "Working-region eligibility retains the ordinary FGD box")
    configure(client, action="clear")
    for raw in ("", "0", "-1", "bad", "NaN", "1e99", "1e-99", "65536"):
        edit(scale=raw)
        row = report()
        check(row["state"] == "unavailable" and not row["displayed"] and row["error"], f"Invalid or unbounded scale stays explicit: {raw!r}")
        client.call("undo")
    for raw in ("sprites/glow01", "custom/glow01.spr", "sprites/glow01.foo.spr", "materials/sprites/glow01.foo.vmt"):
        edit(model=raw)
        row = report()
        check(row["state"] == "unavailable" and row["image"] is None and row["model"]["raw"] == raw and row["error"], f"Unsupported dispatch/filename preserves authored source and defers loading: {raw}")
        client.call("undo")
    for name in ("root", "nested/foo", "materials/foo"):
        reference = f"materials/{name}.vmt"
        edit(model=reference)
        row = report()
        check(row["displayed"] and row["resolved_vmt"] == reference and row["image"]["source"]["file"] == str(materials / f"{name}.vmt"), f"Full virtual Browse-form reference retains exact resource identity: {reference}")
        client.call("undo")
    path = materials / "sprites/glow01.vmt"
    healthy = path.read_bytes()
    path.write_text("malformed winning Sprite")
    client.call("reload_material_assets")
    failure = report()
    check(failure["state"] == "unavailable" and failure["image"]["source"]["file"] == str(path), "Malformed highest-priority VMT never falls through to a healthy lower mount")
    path.write_bytes(healthy)
    check(not report()["displayed"], "Shared image admission retains cached failure until reload")
    client.call("reload_material_assets")
    restored, _ = render("sprite-restored", top)
    check(report()["displayed"] and restored == ring, "Shared material reload repairs the exact original image")
    client.call("frame_selection", {"aspect": 1})
    check(client.call("get_camera") == camera and detail(client, [entity])[0]["bounds"] == bounds and client.call("get_object_hit_candidates", body_ray) == hits, "Scale, admission failures and rendering preserve exact ordinary framing and picking")
    check(detail(client, [entity]) == authored and source.read_bytes() == saved and client.call("get_history")["undo_count"] == history, "Read/preview operations and reversible probes preserve exact source, camera, selection and history")
    return {"passed": len(checks), "checks": checks, "sprite_resource": ready, "failure": failure, "images": images, "desktop_input": False, "native_exercised": False}


def installed_probe(client, output):
    client.call("set_view", {"entity_images": True})
    fgd = output / "installed-sprites.fgd"
    fgd.write_text(FGD)
    client.call("load_entity_definitions", {"path": str(fgd)})
    entity = client.call("create_entity", {"classname": "env_sprite", "origin": [0, 0, 0], "properties": {"scale": "0.25"}})["id"]
    row = client.call("get_entity_inspector", {"id": entity})["helpers"]["sprite_resource"]
    assert row["displayed"] and row["resolved_vmt"] == "materials/sprites/glow01.vmt", row
    assert row["image"]["width"] == row["image"]["height"] == 64 and row["world_size"] == [16, 16], row
    assert row["image"]["source"] and row["image"]["texture_source"], row
    assert not client.call("get_map_info")["rendering"]["initialized"]
    return {"sprite_resource": row, "lazy_headless": True, "native_exercised": False}


def main():
    parser = argparse.ArgumentParser(description="Validate static Sprite resource guides through private MCP and offscreen image controls.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--installed-gameinfo", type=Path)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    gameinfo, materials = private_fixture(output)
    with patch.dict(os.environ, {"HAMMERR_RECENT_MAPS_DIR": str(output / "recent"), "HAMMERR_BUILD_RECIPES": str(output / "recipes.json")}):
        client = Client(None, args.binary.resolve(), output, gameinfo)
    try:
        result = validate(client, output, materials)
    finally:
        client.close()
    if args.installed_gameinfo:
        installed = output / "installed"
        installed.mkdir()
        with patch.dict(os.environ, {"HAMMERR_RECENT_MAPS_DIR": str(installed / "recent"), "HAMMERR_BUILD_RECIPES": str(installed / "recipes.json")}):
            client = Client(None, args.binary.resolve(), installed, args.installed_gameinfo.resolve())
        try:
            result["installed"] = installed_probe(client, installed)
        finally:
            client.close()
    (output / "results.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"passed": result["passed"], "output": str(output), "installed": bool(args.installed_gameinfo)}))


if __name__ == "__main__":
    main()
