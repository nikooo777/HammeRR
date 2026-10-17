import argparse
import hashlib
import json
from pathlib import Path
from unittest.mock import patch

from validate_authored_lighting_preview import SOURCE_FIELDS, assets, file_sha256
from validate_face_lighting import DEFAULT_BINARY, Client, detail, face_ref
from validate_sky_preview import rgba_png

COUNTS = {
    "admitted_surfaces": 3,
    "excluded_surfaces": 0,
    "admitted_triangles": 672,
    "excluded_triangles": 0,
}
EMPTY = dict.fromkeys(COUNTS, 0)
CAMERA = {
    "projection": "perspective",
    "position": [-310, -380, 270],
    "look_at": [0, 0, 18],
    "fov": 50,
}


def validate(client, output, fgd, material, result):
    checks = result["checks"]

    def check(condition, label):
        if not condition:
            raise AssertionError(label)
        checks.append(label)

    def render(name, **options):
        png, metadata = client.render_result(
            width=1001,
            height=701,
            camera=CAMERA,
            selection_outlines=False,
            fog_preview=False,
            **options,
        )
        path = output / f"{name}.png"
        path.write_bytes(png)
        (output / f"{name}.metadata.json").write_text(
            json.dumps(metadata, indent=2) + "\n"
        )
        report = metadata["lighting_preview"]["report"]
        check(
            report["source"] is not None and SOURCE_FIELDS <= report["source"].keys(),
            f"{name}: render retains complete source provenance",
        )
        result["images"].append(
            {"file": path.name, "sha256": hashlib.sha256(png).hexdigest()}
        )
        return rgba_png(path), report

    client.call("load_entity_definitions", {"path": str(fgd)})
    client.call("set_spatial_settings", {"grid_visible": False})
    client.call(
        "set_view",
        {
            "surface": "textured",
            "lighting_preview": False,
            "lighting_quality": "detail",
            "normal_maps": False,
            "lighting_exposure_ev": 0,
            "sky_preview": False,
            "rope_preview": False,
            "entity_images": False,
            "selection_outlines": False,
        },
    )
    parents, faces = [], []
    for x, power in [(-192, 2), (-64, 3), (64, 4)]:
        parent = client.call(
            "create_block",
            {
                "mins": [x, -64, -16],
                "maxs": [x + 128, 64, 0],
                "material": "audit/surface",
            },
        )["id"]
        shape = detail(client, [parent])[0]
        face = face_ref(
            shape, next(face for face in shape["faces"] if face["normal"][2] > 0.99)
        )
        client.call("create_terrain", {"faces": [face], "power": power})
        parents.append(parent)
        faces.append(face)
    sculpt = client.call(
        "preview_terrain_sculpt",
        {
            "faces": faces,
            "mode": "raise",
            "center": [0, 0, 0],
            "radius": 280,
            "amount": 52,
        },
    )
    client.call("commit_face_preview", {"token": sculpt["state"]["preview_token"]})
    light = client.call(
        "create_entity",
        {
            "classname": "light",
            "origin": [-100, -64, 140],
            "properties": {
                "_light": "255 164 86 450",
                "_constant_attn": "1",
                "_linear_attn": "0",
                "_quadratic_attn": "0",
            },
        },
    )["id"]
    spot = client.call(
        "create_entity",
        {
            "classname": "light_spot",
            "origin": [100, 8, 140],
            "properties": {
                "_light": "90 166 255 650",
                "pitch": "-90",
                "_inner_cone": "12",
                "_cone": "35",
                "_exponent": "2",
                "_constant_attn": "1",
                "_linear_attn": "0",
                "_quadratic_attn": "0",
            },
        },
    )["id"]
    client.call("select", {"ids": []})
    initial = client.call("get_lighting_preview")
    check(
        not initial["enabled"]
        and initial["report"]["terrain"] == EMPTY
        and not client.call("get_map_info")["rendering"]["initialized"],
        "Default-off terrain inspection performs no graphics or receiver admission",
    )
    client.call("set_camera", CAMERA)
    source = output / "terrain-source.vmf"
    client.call("save_map", {"path": str(source)})
    saved = source.read_bytes()
    committed = detail(client, parents + [light, spot])
    state = client.call("get_map_info")
    camera = client.call("get_camera")
    client.call("set_view", {"lighting_preview": True})
    admitted = client.call("get_lighting_preview")
    check(
        admitted["report"]["terrain"] == COUNTS
        and admitted["report"]["admitted_receivers"] == 0
        and admitted["report"]["admitted_lights"] == 2
        and not client.call("get_map_info")["rendering"]["initialized"],
        "CPU inspection admits all three mixed-power surfaces and 672 triangles separately from flat polygons",
    )
    client.call("set_view", {"lighting_preview": False})
    ordinary, _ = render("terrain-lighting-off")
    default, _ = render("terrain-lighting-default")
    detailed, report = render("terrain-lighting-detail", lighting_preview=True)
    fast, fast_report = render(
        "terrain-lighting-fast", lighting_preview=True, lighting_quality="fast"
    )
    check(
        ordinary == default
        and detailed != ordinary
        and fast != detailed
        and report["terrain"] == fast_report["terrain"] == COUNTS,
        "Actual Detail and Fast renders illuminate the same sewn terrain with distinct interpolation; disabled pixels stay exact",
    )
    generation = report["source"]["material_generation"]
    material.write_text(
        "LightmappedGeneric { $basetexture audit/base $bumpmap audit/normal }\n"
    )
    cached, cached_report = render("terrain-material-cached", lighting_preview=True)
    check(
        cached == detailed
        and cached_report["source"]["material_generation"] == generation,
        "External VMT changes cannot mix fresh receiver settings with cached base pixels",
    )
    client.call("reload_material_assets")
    rejected, excluded = render("terrain-normal-excluded", lighting_preview=True)
    rejected_off, _ = render("terrain-normal-ordinary", lighting_preview=False)
    check(
        rejected == rejected_off
        and excluded["terrain"]
        == {
            "admitted_surfaces": 0,
            "excluded_surfaces": 3,
            "admitted_triangles": 0,
            "excluded_triangles": 672,
        }
        and excluded["source"]["material_generation"] != generation
        and all(
            "bump and normal maps" in row["reason"]
            for row in excluded["diagnostics"]
        ),
        "Reload rejects declared terrain normal maps explicitly and retains exact ordinary geometry pixels",
    )
    material.write_text("LightmappedGeneric { $basetexture audit/base }\n")
    client.call("reload_material_assets")
    restored, _ = render("terrain-material-restored", lighting_preview=True)
    check(restored == detailed, "Repair plus explicit reload restores exact lit pixels")
    for surface in ("flat", "wireframe", "lightmap_density", "smoothing_groups"):
        disabled, _ = render(f"terrain-{surface}-off", surface=surface, lighting_preview=False)
        enabled, diagnostic = render(
            f"terrain-{surface}-on", surface=surface, lighting_preview=True
        )
        check(
            enabled == disabled and diagnostic["terrain"] == EMPTY,
            f"{surface} preserves exact terrain pixels and performs no authored-light admission",
        )
    draft = client.call(
        "preview_terrain_sculpt",
        {
            "faces": faces,
            "mode": "raise",
            "center": [0, 0, 16],
            "radius": 280,
            "amount": 24,
        },
    )
    token = draft["state"]["preview_token"]
    draft_state = client.call("get_face_selection")
    draft_geometry = client.call("get_terrain_grids", {"faces": faces, "preview": True})
    preview, preview_report = render("terrain-owned-draft", lighting_preview=True)
    check(
        preview != detailed
        and preview_report["terrain"] == COUNTS
        and preview_report["source"]["render_revision"]
        != preview_report["source"]["revision"],
        "Lighting follows the displayed owned terrain draft with distinct committed/render provenance",
    )
    for quality in ("fast", "detail"):
        client.call("set_view", {"lighting_preview": True, "lighting_quality": quality})
        check(
            client.call("get_face_selection") == draft_state
            and client.call("get_terrain_grids", {"faces": faces, "preview": True})
            == draft_geometry,
            f"Shared {quality} lighting settings preserve the owned terrain review",
        )
    client.call("set_view", {"lighting_preview": False, "lighting_quality": "detail"})
    unchanged = client.call("get_map_info")
    check(
        unchanged["view"] == state["view"]
        and unchanged["undo"] == state["undo"]
        and unchanged["redo"] == state["redo"]
        and detail(client, parents + [light, spot]) == committed
        and client.call("get_camera") == camera
        and source.read_bytes() == saved,
        "Shared controls and temporary renders preserve committed source, camera, history and saved bytes",
    )
    client.call("cancel_face_preview", {"token": token})
    cancelled, _ = render("terrain-draft-cancelled", lighting_preview=True)
    check(cancelled == detailed, "Cancelling the terrain draft restores exact lit pixels")
    result["source_sha256"] = hashlib.sha256(saved).hexdigest()
    result["receiver_contract"] = admitted


def main():
    parser = argparse.ArgumentParser(
        description="Validate terrain authored-light preview in a private stdio MCP Session."
    )
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve()
    if not binary.is_file():
        parser.error(f"Binary does not exist: {binary}")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    gameinfo, fgd = assets(output)
    material = gameinfo.parent / "materials/audit/surface.vmt"
    material.write_text("LightmappedGeneric { $basetexture audit/base }\n")
    result = {
        "status": "running",
        "binary": str(binary),
        "binary_sha256": file_sha256(binary),
        "transport": "private_stdio",
        "desktop_input": False,
        "native_exercised": False,
        "checks": [],
        "images": [],
    }
    client = None
    try:
        with patch.dict(
            "os.environ",
            {
                "HAMMERR_RECENT_MAPS_DIR": str(output / "recent"),
                "HAMMERR_BUILD_RECIPES": str(output / "recipes.json"),
                "HAMMERR_PREFABS_DIR": str(output / "prefabs"),
            },
        ):
            client = Client(None, binary, output, gameinfo=gameinfo)
        validate(client, output, fgd, material, result)
        result["status"] = "passed"
    except Exception as error:
        result["status"] = "failed"
        result["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        if client is not None:
            client.close()
        (output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps({"status": result["status"], "passed": len(result["checks"]), "output": str(output)}))


if __name__ == "__main__":
    main()
