import argparse
import hashlib
import json
from pathlib import Path
from unittest.mock import patch

from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_normal_map_preview import fixture, texture
from validate_sky_preview import rgba_png

CONTROL_SIZE = 513
TOP = {"projection": "top", "center": [0, 0, 0], "ortho_height": 288}
PROBE = (427, 256)
POOL = (299, 256)
SOURCE_FIELDS = {
    "document_generation",
    "revision",
    "render_revision",
    "scene_serial",
    "working_region_stamp",
    "material_generation",
}


def file_sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while chunk := handle.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def assets(output):
    gameinfo, materials = fixture(output)
    (materials / "base.vtf").write_bytes(texture([184, 194, 207, 255]))
    (materials / "normal.vtf").write_bytes(texture([180, 128, 244, 255]))
    for name, color in {
        "wall": [88, 108, 142, 255],
        "side": [112, 126, 147, 255],
        "accent": [190, 125, 89, 255],
    }.items():
        (materials / f"{name}.vtf").write_bytes(texture(color))
        (materials / f"{name}.vmt").write_text(
            f"LightmappedGeneric {{ $basetexture audit/{name} }}\n"
        )
    fgd = output / "lighting.fgd"
    fgd.write_text(
        "\n".join(
            f"@PointClass size(-2 -2 -2, 2 2 2) color({color}) = {name} []"
            for name, color in [
                ("light", "255 180 90"),
                ("light_spot", "90 180 255"),
                ("light_dynamic", "255 70 120"),
                ("info_target", "100 180 140"),
            ]
        )
        + "\n"
    )
    return gameinfo, fgd


def patch_rgb(pixels, point, radius=4):
    x, y = point
    return bytes(
        channel
        for row in range(y - radius, y + radius + 1)
        for column in range(x - radius, x + radius + 1)
        for channel in pixels[
            (row * CONTROL_SIZE + column) * 4 : (row * CONTROL_SIZE + column) * 4 + 3
        ]
    )


def validate(client, output, fgd, result):
    checks = result["checks"]
    images = result["images"]

    def check(condition, label):
        if not condition:
            raise AssertionError(label)
        checks.append(label)

    def block(mins, maxs, material="audit/surface"):
        return client.call(
            "create_block", {"mins": mins, "maxs": maxs, "material": material}
        )["id"]

    def entity(classname, origin, **properties):
        return client.call(
            "create_entity",
            {"classname": classname, "origin": origin, "properties": properties},
        )["id"]

    def emitter(classname, origin, color="255 220 180 480", **properties):
        return entity(
            classname,
            origin,
            _light=color,
            _constant_attn="1",
            _linear_attn="0",
            _quadratic_attn="0",
            style="0",
            **properties,
        )

    def visibility(ids, action):
        client.call("set_object_visibility", {"action": action, "ids": ids})

    def reset():
        client.call("new_map", {"discard_changes": True})
        client.call("set_transform_tool", {"mode": "select"})
        client.call("set_spatial_settings", {"grid_visible": False})
        client.call(
            "set_view",
            {
                "surface": "textured",
                "lighting_preview": False,
                "lighting_quality": "detail",
                "lighting_exposure_ev": 0,
                "normal_maps": False,
                "selection_outlines": False,
                "entity_images": False,
                "sky_preview": False,
                "rope_preview": False,
            },
        )
        client.call("select", {"ids": []})

    def render(name, camera=TOP, width=CONTROL_SIZE, height=CONTROL_SIZE, **options):
        png, metadata = client.render_result(
            width=width,
            height=height,
            camera=camera,
            selection_outlines=False,
            fog_preview=False,
            **options,
        )
        path = output / f"{name}.png"
        path.write_bytes(png)
        metadata_path = output / f"{name}.metadata.json"
        metadata_path.write_text(json.dumps(metadata, indent=2) + "\n")
        report = metadata["lighting_preview"]["report"]
        check(
            report["source"] is not None and SOURCE_FIELDS <= report["source"].keys(),
            f"{name}: retained render snapshot provenance is complete",
        )
        images.append(
            {
                "file": path.name,
                "metadata": metadata_path.name,
                "sha256": hashlib.sha256(png).hexdigest(),
                "size": [width, height],
                "active": report["active"],
                "admitted_lights": report["admitted_lights"],
                "admitted_receivers": report["admitted_receivers"],
                "unavailable": report["unavailable"],
            }
        )
        return rgba_png(path), metadata

    client.call("load_entity_definitions", {"path": str(fgd)})
    reset()
    floor = block([-128, -112, -12], [128, 112, 0])
    blocker = block([-8, -16, 0], [8, 16, 128], "audit/accent")
    client.call("select", {"ids": []})
    initial = client.call("get_lighting_preview")
    check(
        not initial["enabled"]
        and not initial["report"]["active"]
        and initial["report"]["admitted_lights"] == 0
        and not client.call("get_map_info")["rendering"]["initialized"],
        "Default-off CPU inspection performs no light admission or graphics initialization",
    )
    unlit, _ = render("control-off", lighting_preview=False)
    zero, metadata = render("control-zero-lights", lighting_preview=True)
    report = metadata["lighting_preview"]["report"]
    check(
        report["active"]
        and report["admitted_lights"] == 0
        and report["admitted_receivers"] == 12
        and not any(patch_rgb(zero, PROBE))
        and any(patch_rgb(unlit, PROBE)),
        "Zero admitted lights make the known supported floor patch black",
    )
    client.call("set_view", {"lighting_preview": True})
    zero_report = client.call("get_lighting_preview")
    check(
        "Zero admitted lights" in zero_report["summary"]
        and "black" in zero_report["summary"],
        "Shared CPU inspection explains the zero-light black-receiver result",
    )
    result["zero_light_report"] = zero_report
    client.call("set_view", {"lighting_preview": False})
    point = emitter("light", [-96, 0, 128])
    client.call("select", {"ids": []})
    point_pixels, point_metadata = render("control-point", lighting_preview=True)
    check(
        point_metadata["lighting_preview"]["report"]["admitted_lights"] == 1
        and any(patch_rgb(point_pixels, PROBE)),
        "One authored point light illuminates the floor receiver",
    )
    occluder_hit = client.call(
        "raycast", {"origin": [-72, 0, 112], "direction": [168, 0, -112]}
    )
    check(
        occluder_hit["object"] == blocker, "A real brush blocks the light-to-probe ray"
    )
    visibility([blocker], "hide")
    clear, _ = render("control-blocker-hidden", lighting_preview=True)
    check(
        clear != point_pixels
        and patch_rgb(clear, PROBE) == patch_rgb(point_pixels, PROBE),
        "Removing the visible blocker leaves the obstructed floor patch exactly as bright",
    )
    result["measurements"]["unshadowed_probe"] = {
        "emitter": [-96, 0, 128],
        "receiver": [96, 0, 0],
        "pixel": list(PROBE),
        "blocker_ray_hit": occluder_hit,
        "blocked_rgb_sum": sum(patch_rgb(point_pixels, PROBE)),
        "clear_rgb_sum": sum(patch_rgb(clear, PROBE)),
    }
    fast, fast_meta = render(
        "control-fast", lighting_preview=True, lighting_quality="fast"
    )
    check(
        fast_meta["view"]["lighting_quality"] == "fast" and fast != clear,
        "Fast vertex interpolation and Detail resolve visibly different point-light pools",
    )
    low, low_meta = render(
        "control-exposure-minus-eight", lighting_preview=True, lighting_exposure_ev=-8
    )
    high, high_meta = render(
        "control-exposure-plus-eight", lighting_preview=True, lighting_exposure_ev=8
    )
    probe_sums = [sum(patch_rgb(pixels, PROBE)) for pixels in (low, clear, high)]
    check(
        probe_sums[0] < probe_sums[1] < probe_sums[2]
        and low_meta["view"]["lighting_exposure_ev"] == -8
        and high_meta["view"]["lighting_exposure_ev"] == 8,
        "Both exposure endpoints are accepted and order the same lit floor patch",
    )
    result["measurements"]["exposure_probe_rgb_sums"] = probe_sums
    visibility([point], "hide")
    target = entity("info_target", [0, 0, 0], targetname="pool_target")
    spot = emitter(
        "light_spot",
        [0, 0, 112],
        color="100 170 255 420",
        target="pool_target",
        _inner_cone="16",
        _cone="28",
        _exponent="1",
    )
    client.call("select", {"ids": []})
    spot_pixels, spot_meta = render("control-spot", lighting_preview=True)
    check(
        spot_meta["lighting_preview"]["report"]["admitted_lights"] == 1
        and any(patch_rgb(spot_pixels, POOL))
        and not any(patch_rgb(spot_pixels, PROBE)),
        "A targeted spotlight lights its inner pool and leaves the outside floor patch black",
    )
    result["measurements"]["spot_probe_rgb_sums"] = {
        "inside": sum(patch_rgb(spot_pixels, POOL)),
        "outside": sum(patch_rgb(spot_pixels, PROBE)),
    }
    visibility([spot], "hide")
    unsupported = emitter("light_dynamic", [-96, -80, 64])
    client.call("select", {"ids": []})
    rejected, rejected_meta = render("control-unsupported-light", lighting_preview=True)
    report = rejected_meta["lighting_preview"]["report"]
    check(
        report["admitted_lights"] == 0
        and report["excluded_lights"] == 1
        and any("light_dynamic" in row["reason"] for row in report["diagnostics"])
        and not any(patch_rgb(rejected, PROBE)),
        "Unsupported dynamic lighting has an explicit exclusion and cannot illuminate receivers",
    )
    visibility([unsupported], "hide")
    limit = client.call("get_lighting_preview")["limits"]["lights"]
    check(limit == 32, "The authored-light editor budget is explicitly reported as 32")
    extra = [
        emitter("light", [0, -80, 64], color="255 255 255 1") for _ in range(limit + 1)
    ]
    client.call("select", {"ids": []})
    overflow, overflow_meta = render("control-overflow", lighting_preview=True)
    ordinary, _ = render("control-overflow-off", lighting_preview=False)
    report = overflow_meta["lighting_preview"]["report"]
    check(
        not report["active"]
        and report["admitted_lights"] == 0
        and report["admitted_entities"] == []
        and report["admitted_receivers"] == 0
        and "33 supported lights" in report["unavailable"]
        and overflow == ordinary,
        "Overflow selects no light prefix and retains exact ordinary pixels",
    )
    client.call("delete_objects", {"ids": [extra[-1]]})
    recovered, recovered_meta = render("control-budget-recovery", lighting_preview=True)
    report = recovered_meta["lighting_preview"]["report"]
    check(
        report["active"]
        and report["admitted_lights"] == limit
        and report["unavailable"] is None
        and recovered != ordinary
        and any(patch_rgb(recovered, PROBE)),
        "Removing the excess light restores all 32 admitted emitters and lit pixels",
    )
    client.call("delete_objects", {"ids": extra[:-1]})
    visibility([point], "show")
    client.call("select", {"ids": []})
    before_draft, _ = render("control-before-draft", lighting_preview=True)
    off_before_draft, _ = render("control-before-draft-off", lighting_preview=False)
    client.call(
        "set_camera",
        {
            "projection": "perspective",
            "position": [-220, -260, 200],
            "look_at": [0, 0, 24],
        },
    )
    source_path = output / "control-source.vmf"
    client.call("save_map", {"path": str(source_path)})
    source = source_path.read_bytes()
    ids = [floor, blocker, point, spot, target, unsupported]
    committed = detail(client, ids)
    client.call(
        "transform_objects",
        {"ids": [point], "translation": [96, 0, 0], "preview": True},
    )
    client.call("set_view", {"lighting_preview": True, "lighting_quality": "detail"})
    snapshot = client.call("get_map_info")
    camera = client.call("get_camera")
    draft = snapshot["transform"]["preview"]
    check(draft is not None, "The private Session owns an active light transform draft")
    moved, moved_meta = render("control-owned-light-draft", lighting_preview=True)
    check(
        patch_rgb(moved, PROBE) != patch_rgb(before_draft, PROBE)
        and moved_meta["lighting_preview"]["report"]["source"]["render_revision"]
        != moved_meta["lighting_preview"]["report"]["source"]["revision"],
        "Lighting follows the displayed draft while the render report distinguishes committed source",
    )
    for options in [
        {"lighting_exposure_ev": 9},
        {"lighting_exposure_ev": -9},
        {"lighting_quality": "invalid"},
        {"lighting_preview": True, "lighting_quality": "fast", "normal_maps": True},
    ]:
        client.call("set_view", options, error=True)
        client.call("render_view", {"width": 65, "height": 65, **options}, error=True)
        check(
            client.call("get_map_info")["view"] == snapshot["view"],
            f"Invalid shared/temporary settings are atomic: {options}",
        )
    for quality in ("fast", "detail"):
        client.call("set_view", {"lighting_quality": quality})
        check(
            client.call("get_map_info")["transform"]["preview"] == draft,
            f"Shared {quality} quality preserves the owned geometry draft",
        )
    normals, normal_meta = render("control-detail-normals", normal_maps=True)
    check(
        normal_meta["normal_maps"]["report"]["applied_faces"] > 0 and normals != moved,
        "Detail authored lighting composes with the admitted private normal texture",
    )
    for surface in ("flat", "wireframe", "lightmap_density", "smoothing_groups"):
        off, _ = render(
            f"control-{surface}-off", surface=surface, lighting_preview=False
        )
        on, metadata = render(
            f"control-{surface}-on", surface=surface, lighting_preview=True
        )
        report = metadata["lighting_preview"]["report"]
        check(
            on == off
            and not report["active"]
            and report["admitted_lights"] == 0
            and report["admitted_receivers"] == 0,
            f"{surface} retains exact pixels and performs no authored-light admission",
        )
    unchanged = client.call("get_map_info")
    check(
        unchanged["view"] == snapshot["view"]
        and client.call("get_camera") == camera
        and unchanged["undo"] == snapshot["undo"]
        and unchanged["redo"] == snapshot["redo"]
        and unchanged["unsaved_changes"] == snapshot["unsaved_changes"]
        and unchanged["transform"]["preview"] == draft
        and detail(client, ids) == committed
        and source_path.read_bytes() == source,
        "Temporary renders and rejected settings preserve shared camera/view, source, history and draft",
    )
    client.call("cancel_transform_preview")
    client.call("select", {"ids": []})
    restored, _ = render("control-draft-cancelled", lighting_preview=True)
    off_restored, _ = render("control-restored-off", lighting_preview=False)
    check(
        restored == before_draft and off_restored == off_before_draft,
        "Cancelling the draft restores exact authored-light and disabled pixels",
    )
    result["control_source_sha256"] = hashlib.sha256(source).hexdigest()

    reset()
    block([-160, -128, -12], [160, 128, 0])
    block([-160, 112, 0], [160, 128, 128], "audit/wall")
    block([-160, -128, 0], [-148, 112, 128], "audit/side")
    block([148, 32, 0], [160, 112, 96], "audit/side")
    client.call(
        "create_primitive",
        {
            "shape": "wedge",
            "mins": [-104, -8, 0],
            "maxs": [-16, 84, 64],
            "material": "audit/surface",
        },
    )
    block([48, 24, 0], [116, 94, 28], "audit/side")
    block([66, 42, 28], [100, 76, 62], "audit/accent")
    emitter("light", [-88, -40, 108], color="255 164 86 260")
    entity("info_target", [80, 58, 10], targetname="showcase_pool")
    cool = emitter(
        "light_spot",
        [104, 72, 140],
        color="90 166 255 380",
        target="showcase_pool",
        _inner_cone="22",
        _cone="38",
        _exponent="1",
    )
    visibility([cool], "hide")
    client.call("select", {"ids": []})
    room_camera = {
        "projection": "perspective",
        "position": [-310, -420, 290],
        "look_at": [0, 32, 42],
        "fov": 50,
    }
    room_options = {"camera": room_camera, "width": 1001, "height": 701}
    room_off, _ = render("showcase-off", lighting_preview=False, **room_options)
    room_point, metadata = render(
        "showcase-point", lighting_preview=True, **room_options
    )
    check(
        room_point != room_off
        and metadata["lighting_preview"]["report"]["admitted_lights"] == 1,
        "The owned room and ramp render under a warm authored point light",
    )
    visibility([cool], "show")
    room_both, metadata = render(
        "showcase-point-spot", lighting_preview=True, **room_options
    )
    check(
        room_both != room_point
        and metadata["lighting_preview"]["report"]["admitted_lights"] == 2,
        "The cool targeted spotlight adds a second authored pool to the room",
    )
    room_normals, metadata = render(
        "showcase-detail-normals",
        lighting_preview=True,
        normal_maps=True,
        **room_options,
    )
    check(
        room_normals != room_both
        and metadata["normal_maps"]["report"]["applied_faces"] > 0,
        "The final room render combines Detail lighting and flat-brush normal preview",
    )
    client.call("set_camera", room_camera)
    client.call("save_map", {"path": str(output / "showcase-source.vmf")})
    result["showcase"] = {
        "source": "showcase-source.vmf",
        "camera": room_camera,
        "images": [
            "showcase-off.png",
            "showcase-point.png",
            "showcase-point-spot.png",
            "showcase-detail-normals.png",
        ],
        "policy": "Private unshadowed LDR editor preview; no native brightness or shadow claim",
    }


def main():
    parser = argparse.ArgumentParser(
        description="Validate authored lighting through an owned stdio MCP Session and private offscreen renders."
    )
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.binary = args.binary.resolve()
    if not args.binary.is_file():
        parser.error(f"Binary does not exist: {args.binary}")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    gameinfo, fgd = assets(output)
    result = {
        "status": "running",
        "transport": "private_stdio",
        "binary": str(args.binary),
        "binary_sha256": file_sha256(args.binary),
        "gameinfo": str(gameinfo),
        "desktop_input": False,
        "native_exercised": False,
        "checks": [],
        "images": [],
        "measurements": {},
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
            client = Client(None, args.binary, output, gameinfo=gameinfo)
        validate(client, output, fgd, result)
        result["status"] = "passed"
    except Exception as error:
        result["status"] = "failed"
        result["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        if client is not None:
            client.close()
        result["passed"] = len(result["checks"])
        (output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
        print(
            json.dumps(
                {
                    "status": result["status"],
                    "passed": result["passed"],
                    "output": str(output),
                }
            )
        )


if __name__ == "__main__":
    main()
