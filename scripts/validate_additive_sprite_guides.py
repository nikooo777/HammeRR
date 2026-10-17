import argparse
import hashlib
import json
import struct
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_sky_preview import rgba_png

SIZE = 401
FGD = """
@PointClass size(-2 -2 -2, 2 2 2) sprite() = env_sprite [
    model(sprite) : "Sprite Name" : "materials/audit/additive.vmt"
    scale(string) : "Scale" : "1"
    rendermode(integer) : "Render Mode" : 0
    rendercolor(color255) : "FX Color" : "255 255 255"
    renderamt(integer) : "FX Amount" : 255
    renderfx(integer) : "Render FX" : 0
    HDRColorScale(float) : "HDR" : "1"
]
@PointClass size(4 4 4) iconsprite("audit/additive.vmt") = icon_sample []
"""


def texture(pixels):
    header = bytearray(80)
    header[:4] = b"VTF\0"
    struct.pack_into("<IIIHH", header, 4, 7, 2, 80, 32, 32)
    for offset in (24, 63):
        struct.pack_into("<H", header, offset, 1)
    header[56] = 1
    struct.pack_into("<i", header, 57, -1)
    return bytes(header) + bytes(pixels)


def fixture(output):
    game = output / "game"
    materials = game / "materials/audit"
    materials.mkdir(parents=True, exist_ok=True)
    pixels = [
        channel
        for y in range(32)
        for x in range(32)
        for channel in (
            (100, 50, 20, 128) if 8 <= x < 24 and 8 <= y < 24 else (0, 0, 0, 255)
        )
    ]
    (materials / "additive.vtf").write_bytes(texture(pixels))
    (materials / "additive.vmt").write_text(
        'Sprite { $basetexture audit/additive $spriteorientation vp_parallel $spriteorigin "[0.5 0.5]" }\n'
    )
    (materials / "background.vtf").write_bytes(texture([60, 100, 150, 255] * 1024))
    (materials / "background.vmt").write_text(
        "LightmappedGeneric { $basetexture audit/background }\n"
    )
    gameinfo = game / "gameinfo.txt"
    gameinfo.write_text(
        'GameInfo { FileSystem { SearchPaths { game "|gameinfo_path|." } } }\n'
    )
    fgd = output / "additive.fgd"
    fgd.write_text(FGD)
    return gameinfo, materials, fgd


def linear(value):
    value /= 255
    return value / 12.92 if value <= 0.04045 else ((value + 0.055) / 1.055) ** 2.4


def encoded(value):
    value = value * 12.92 if value <= 0.0031308 else 1.055 * value ** (1 / 2.4) - 0.055
    return round(max(0, min(1, value)) * 255)


def pixel(pixels, world_x, world_y):
    x = round(SIZE / 2 + world_x * SIZE / 128)
    y = round(SIZE / 2 - world_y * SIZE / 128)
    offset = (y * SIZE + x) * 4
    return pixels[offset : offset + 4]


def validate(args):
    gameinfo, materials, fgd = fixture(args.output)
    client = Client(None, args.binary, args.output, gameinfo=gameinfo)
    checks, images = [], []
    try:
        client.call("load_entity_definitions", {"path": str(fgd)})
        assert not client.call("get_map_info")["view"]["additive_sprite_guides"]
        client.call(
            "set_view", {"entity_images": True, "additive_sprite_guides": False}
        )
        client.call("set_spatial_settings", {"grid_visible": False})
        client.call("set_transform_tool", {"mode": "select"})
        client.call(
            "create_block",
            {
                "mins": [-128, -128, -8],
                "maxs": [128, 128, -4],
                "material": "audit/background",
            },
        )
        sprite = client.call(
            "create_entity",
            {
                "classname": "env_sprite",
                "origin": [-24, 0, 0],
                "properties": {"rendermode": "5", "HDRColorScale": "0.7"},
            },
        )["id"]
        icon = client.call(
            "create_entity", {"classname": "icon_sample", "origin": [24, 0, 0]}
        )["id"]
        client.call("select", {"ids": []})
        authored = detail(client, [sprite, icon])
        history = client.call("get_map_info")["undo"]
        shared_view = client.call("get_map_info")["view"]
        shared_camera = client.call("get_camera")
        ray = {"origin": [-24, 0, 64], "direction": [0, 0, -1]}
        hits = client.call("get_object_hit_candidates", ray)

        def guide():
            return client.call("get_entity_inspector", {"id": sprite})["helpers"][
                "sprite_resource"
            ]

        def edit(**values):
            client.call("set_entity_properties", {"id": sprite, "set": values})

        def render(name, **options):
            png, report = client.render_result(
                width=SIZE,
                height=SIZE,
                camera={"projection": "top", "center": [0, 0, 0], "ortho_height": 128},
                selection_outlines=False,
                **options,
            )
            path = args.output / f"{name}.png"
            path.write_bytes(png)
            images.append(
                {"file": path.name, "sha256": hashlib.sha256(png).hexdigest()}
            )
            return rgba_png(path), report

        initial = guide()
        assert initial["displayed"] and not initial["blend_preview"]["requested"]
        assert not client.call("get_map_info")["rendering"]["initialized"]
        backdrop, _ = render("background", entity_images=False)
        raw, _ = render("raw-default")
        enabled, metadata = render("additive-guide", additive_sprite_guides=True)
        assert metadata["view"]["additive_sprite_guides"]
        assert metadata["sprite_guides"]["guides"][0]["guide"]["blend_preview"][
            "applied"
        ]
        assert pixel(raw, -36, 12)[:3] == bytes([0, 0, 0])
        assert pixel(enabled, -36, 12) == pixel(backdrop, -36, 12)
        assert pixel(enabled, 12, 12) == pixel(raw, 12, 12)
        base = pixel(backdrop, -28, 4)
        glow = pixel(enabled, -28, 4)
        expected = [
            encoded(linear(dst) + linear(src) * 128 / 255)
            for dst, src in zip(base[:3], (100, 50, 20))
        ]
        assert all(
            abs(value - wanted) <= 2 for value, wanted in zip(glow[:3], expected)
        ), (list(glow), expected)
        checks.append(
            "opaque black contributes zero, positive RGB matches independent linear SRC_ALPHA/ONE, literal icon stays raw"
        )
        assert client.call("get_map_info")["view"] == shared_view
        assert client.call("get_camera") == shared_camera
        client.call("set_view", {"additive_sprite_guides": True})
        admitted = guide()
        assert (
            admitted["blend_preview"]["applied"]
            and "HDRColorScale" in admitted["unevaluated_runtime_fields"]
        )
        assert admitted["image"]["source"]["file"] == str(materials / "additive.vmt")
        assert admitted["image"]["texture_source"]["file"] == str(
            materials / "additive.vtf"
        )
        assert render("shared-additive")[0] == enabled
        checks.append(
            "authored mode and mounted provenance shared with inspector; HDRColorScale0.7 is unevaluated in LDR"
        )
        for key, value in (
            ("rendercolor", "254 255 255"),
            ("renderamt", "128"),
            ("renderfx", "1"),
            ("rendermode", "3"),
        ):
            edit(**{key: value})
            rejected = guide()
            assert (
                rejected["displayed"]
                and rejected["blend_preview"]["error"]
                and not rejected["blend_preview"]["applied"]
            )
            assert render(f"unsupported-{key}")[0] == raw
            client.call("undo")
            assert guide()["blend_preview"]["applied"]
        checks.append(
            "unsupported requested appearance retains exact labeled raw image with diagnostics"
        )
        assert render("explicit-off", additive_sprite_guides=False)[0] == raw
        assert render("parent-disabled", entity_images=False)[0] == backdrop
        client.call(
            "set_view", {"additive_sprite_guides": False, "entity_images": False}
        )
        healthy = (materials / "additive.vmt").read_bytes()
        (materials / "additive.vmt").write_text("broken winning material")
        client.call("reload_material_assets")
        assert guide()["image"] is None
        (materials / "additive.vmt").write_bytes(healthy)
        assert (
            render("lazy-admission", entity_images=True, additive_sprite_guides=True)[0]
            == enabled
        )
        client.call("set_view", {"entity_images": True})
        (materials / "additive.vmt").write_text("broken winning material")
        client.call("reload_material_assets")
        assert not guide()["displayed"]
        (materials / "additive.vmt").write_bytes(healthy)
        assert not guide()["displayed"]
        client.call("reload_material_assets")
        assert guide()["displayed"]
        checks.append(
            "parent admission remains lazy and reload preserves shared success/failure semantics"
        )
        assert detail(client, [sprite, icon]) == authored
        assert client.call("get_map_info")["undo"] == history
        assert client.call("get_map_info")["view"] == shared_view
        assert client.call("get_camera") == shared_camera
        assert client.call("get_object_hit_candidates", ray) == hits
        checks.append(
            "temporary/shared preview preserves source, history, box bounds, picking and camera"
        )
    finally:
        client.close()
    result = {
        "passed": len(checks),
        "checks": checks,
        "images": images,
        "native_exercised": False,
        "desktop_input": False,
    }
    (args.output / "results.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"passed": len(checks), "output": str(args.output)}))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-additive-sprite-guides")
    )
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    validate(args)


if __name__ == "__main__":
    main()
