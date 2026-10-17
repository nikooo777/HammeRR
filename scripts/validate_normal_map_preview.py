import argparse
import hashlib
import json
import struct
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client


def texture(color):
    header = bytearray(80)
    header[:4] = b"VTF\0"
    struct.pack_into("<IIIHH", header, 4, 7, 2, 80, 4, 4)
    for offset in [24, 63]:
        struct.pack_into("<H", header, offset, 1)
    header[56] = 1
    struct.pack_into("<i", header, 57, -1)
    return bytes(header) + bytes(color) * 16


def fixture(output):
    game = output / "game"
    materials = game / "materials/audit"
    materials.mkdir(parents=True, exist_ok=True)
    (materials / "base.vtf").write_bytes(texture([240, 240, 240, 255]))
    (materials / "normal.vtf").write_bytes(texture([255, 128, 128, 255]))
    (materials / "surface.vmt").write_text(
        "LightmappedGeneric { $basetexture audit/base $bumpmap audit/normal }\n"
    )
    gameinfo = game / "gameinfo.txt"
    gameinfo.write_text('GameInfo { FileSystem { SearchPaths { game "|gameinfo_path|." } } }\n')
    return gameinfo, materials


def validate(args):
    gameinfo, materials = fixture(args.output)
    client = Client(None, args.binary, args.output, gameinfo=gameinfo)
    checks = []
    try:
        floor = client.call(
            "create_block",
            {"mins": [-64, -64, -16], "maxs": [64, 64, 0], "material": "audit/surface"},
        )["id"]
        client.call("select", {"ids": []})
        client.call("set_spatial_settings", {"grid_visible": False})
        camera = {"position": [-130, -170, 150], "look_at": [0, 0, -8]}
        original_view = client.call("get_map_info")["view"]
        original_camera = client.call("get_camera")
        history = client.call("get_map_info")["undo"]
        original_object = client.call("get_objects", {"ids": [floor]})
        original_hit = client.call("raycast", {"origin": [0, 0, 64], "direction": [0, 0, -1]})
        initial = client.call("get_normal_map_preview")
        assert not initial["enabled"] and not initial["report"]["materials"]
        assert not client.call("get_map_info")["rendering"]["initialized"]
        checks.append("lazy CPU inspection while disabled")

        def render(name, **options):
            data, report = client.render_result(
                width=601, height=401, camera=camera, selection_outlines=False, **options
            )
            (args.output / f"{name}.png").write_bytes(data)
            return hashlib.sha256(data).hexdigest(), report

        baseline, _ = render("normal-maps-off")
        active, report = render("normal-maps-on", normal_maps=True)
        assert active != baseline
        assert report["normal_maps"]["report"]["applied_faces"] == 6
        assert report["normal_maps"]["report"]["materials"][0]["texture_source"]
        assert client.call("get_map_info")["view"] == original_view
        assert client.call("get_camera") == original_camera
        checks.append("temporary admitted preview and provenance preserve shared view/camera")
        for surface in ["flat", "wireframe", "lightmap_density", "smoothing_groups"]:
            off, _ = render(f"{surface}-off", surface=surface, normal_maps=False)
            on, metadata = render(f"{surface}-on", surface=surface, normal_maps=True)
            assert on == off and not metadata["normal_maps"]["report"]["active"]
        checks.append("all diagnostic views retain exact PNG bytes")
        client.call("set_view", {"normal_maps": True})
        assert client.call("get_normal_map_preview")["report"]["applied_faces"] == 6
        assert render("shared-normal-preview")[0] == active
        client.call("set_view", {"normal_maps": False})
        (materials / "normal.vtf").unlink()
        client.call("reload_material_assets")
        assert not client.call("get_normal_map_preview")["report"]["materials"]
        (materials / "normal.vtf").write_bytes(texture([128, 255, 128, 255]))
        repaired, report = render("normal-reloaded", normal_maps=True)
        assert repaired != active and report["normal_maps"]["report"]["applied_faces"] == 6
        checks.append("disabled reload never caches a missing bump texture")
        material = materials / "surface.vmt"
        material.write_text(
            'LightmappedGeneric { $basetexture audit/base $bumpmap audit/normal $bumptransform "rotate 1" }\n'
        )
        client.call("reload_material_assets")
        failed, report = render("unsupported-bump-transform", normal_maps=True)
        assert failed == baseline
        assert report["normal_maps"]["report"]["rejected_faces"] == 6
        assert "zero rotation" in report["normal_maps"]["report"]["errors"][0]
        material.write_text("LightmappedGeneric { $basetexture audit/base $bumpmap audit/normal }\n")
        assert render("cached-failure", normal_maps=True)[0] == baseline
        client.call("reload_material_assets")
        assert render("repaired-after-reload", normal_maps=True)[0] == repaired
        checks.append("explicit unsupported-state diagnostics and cached failure recovery")
        assert client.call("get_map_info")["undo"] == history
        assert client.call("get_objects", {"ids": [floor]}) == original_object
        assert client.call("raycast", {"origin": [0, 0, 64], "direction": [0, 0, -1]}) == original_hit
        assert client.call("get_camera") == original_camera
        assert client.call("get_map_info")["view"] == original_view
        checks.append("source, history, geometry bounds and picking unchanged")
    finally:
        client.close()
    result = {"normal_map_preview": True, "checks": checks}
    (args.output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, default=Path("/tmp/hammerr-normal-map-preview"))
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    validate(args)


if __name__ == "__main__":
    main()
