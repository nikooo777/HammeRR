import argparse
import hashlib
import json
import os
import struct
from pathlib import Path
from unittest.mock import patch

from validate_compile_jobs import wait_job
from validate_direct_lighting import center_sample, floor_samples
from validate_face_lighting import DEFAULT_BINARY, ROOT, Client, detail, face_ref
from validate_visibility import ROOM


def fixture(game):
    (game / "models/props").mkdir(parents=True, exist_ok=True)
    (game / "materials/models/test").mkdir(parents=True, exist_ok=True)
    (game / "materials/dev").mkdir(parents=True, exist_ok=True)
    (game / "gameinfo.txt").write_text(
        'GameInfo { FileSystem { SearchPaths { Game "|gameinfo_path|." } } }\n'
    )
    for path, text in {
        "models/test/one": "VertexLitGeneric { $basetexture test }",
        "models/test/two": "VertexLitGeneric { $alphatest 1 $basetexture test }",
        "dev/dev_measuregeneric01": "LightmappedGeneric { }",
    }.items():
        (game / f"materials/{path}.vmt").write_text(text)
    mdl, vvd, vtx = bytearray(1024), bytearray(256), bytearray(173)
    mdl[:4], vvd[:4] = b"IDST", b"IDSV"
    for offset, value in {
        4: 48, 8: 7, 76: 1024, 152: 16, 204: 2, 208: 256,
        212: 1, 216: 384, 220: 1, 224: 2, 228: 388, 232: 1,
        236: 400, 256: 464, 320: 416, 384: 752, 404: 1,
        408: 1, 412: 16, 488: 1, 492: 160, 496: 4,
        576: 0, 580: -160, 584: 4,
    }.items():
        struct.pack_into("<i", mdl, offset, value)
    for offset in [104, 108, 128, 132]:
        struct.pack_into("<f", mdl, offset, -80)
    for offset in [116, 120, 140, 144]:
        struct.pack_into("<f", mdl, offset, 80)
    struct.pack_into("<H", mdl, 390, 1)
    for offset, value in [(720, b"one"), (736, b"two"), (752, b"models/test/")]:
        mdl[offset:offset + len(value)] = value
    for offset, value in {4: 4, 8: 7, 12: 1, 16: 4, 56: 64}.items():
        struct.pack_into("<i", vvd, offset, value)
    for index, point in enumerate([(-24, -24, 0), (24, -24, 0), (0, 24, 0), (0, 0, 0)]):
        struct.pack_into("<3f", vvd, 64 + index * 48 + 16, *point)
        struct.pack_into("<f", vvd, 64 + index * 48 + 36, 1)
    for index, uv in enumerate([(0, 0), (1, 0), (0.5, 1)]):
        struct.pack_into("<2f", vvd, 64 + index * 48 + 40, *uv)
    for offset, value in {
        0: 7, 16: 7, 20: 1, 28: 1, 32: 36, 36: 1, 40: 8,
        44: 1, 48: 8, 52: 1, 56: 12, 64: 1, 68: 9, 73: 4,
        77: 25, 81: 6, 85: 61, 89: 1, 93: 73, 146: 3, 154: 4,
    }.items():
        struct.pack_into("<i", vtx, offset, value)
    for index in range(4):
        struct.pack_into("<H", vtx, 98 + index * 9 + 4, index)
    struct.pack_into("<6H", vtx, 134, 0, 1, 2, 0, 2, 3)
    vtx[164] = 1
    for extension, data in zip(["mdl", "vvd", "dx90.vtx"], [mdl, vvd, vtx], strict=True):
        (game / f"models/props/test.{extension}").write_bytes(data)
    (game / "materials/test.vtf").write_bytes(alpha_texture())


def alpha_texture(right_alpha=0):
    texture = bytearray(80)
    texture[:4] = b"VTF\0"
    struct.pack_into("<I", texture, 4, 7)
    struct.pack_into("<I", texture, 12, 80)
    struct.pack_into("<HH", texture, 16, 4, 4)
    struct.pack_into("<I", texture, 20, 0x1000)
    struct.pack_into("<H", texture, 24, 1)
    texture[56] = 1
    struct.pack_into("<i", texture, 57, -1)
    for _ in range(4):
        for alpha in [255, 255, right_alpha, right_alpha]:
            texture.extend([255, 255, 255, alpha])
    return texture


def floor_sample(data, x, y):
    return next(color for point, color, _, _ in floor_samples(data) if point[:2] == [x, y])


def validate(client, output, game):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    client.call("new_map", {"discard_changes": True})
    for low, high in ROOM:
        client.call("create_block", {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"})
    client.call("create_entity", {"classname": "info_player_terrorist", "origin": [-64, 0, 8]})
    client.call("create_entity", {"classname": "light", "origin": [0, 0, 100], "properties": {"_light": "255 0 0 200", "_quadratic_attn": "1"}})
    prop = client.call("create_entity", {"classname": "prop_static", "origin": [0, 0, 50], "properties": {"model": "models/props/test.mdl", "solid": "0"}})["id"]
    client.call("set_compile_settings", {"lighting": "direct_ldr", "physics": "none"})
    vmf = output / "prop-shadow.vmf"
    bsp = output / "prop-shadow.bsp"
    client.call("save_map", {"path": str(vmf)})
    source = vmf.read_bytes()
    objects = detail(client, [prop])
    history = client.call("get_map_info")["undo"]
    result = client.call("compile_map", {"path": str(bsp)})
    stats = result["stats"]
    check(result["success"] and stats["prop_shadow_props"] == stats["prop_shadow_models"] == stats["prop_shadow_triangles"] == 1, "Shared compile result reports one real mesh triangle")
    check(stats["prop_shadow_work"] > 0 and stats["prop_shadow_disabled"] == 0, "Shared compile result reports bounded preparation work and native flag counts")
    shadowed = bsp.read_bytes()
    check(center_sample(shadowed) == [0, 0, 0], "Non-solid static-prop mesh occludes the center sample")
    hole = next(color for point, color, _, _ in floor_samples(shadowed) if point[:2] == [48, 48])
    check(hole[0] > 0, "A ray through model bounds but outside its triangle remains lit")
    background = output / "prop-shadow-background.bsp"
    client.call("start_compile", {"path": str(background)})
    job = wait_job(client)
    check(job["status"] == "completed" and background.read_bytes() == shadowed, "Synchronous and background MCP builds produce identical mesh shadows")
    check(detail(client, [prop]) == objects and client.call("get_map_info")["undo"] == history and vmf.read_bytes() == source, "Baking preserves authored props, undo history and VMF bytes")
    client.call("set_entity_properties", {"id": prop, "set": {"skin": "1"}})
    alpha = client.call("compile_map", {"path": str(bsp)})
    check(any(message["object"] == prop and "texture alpha" in message["message"] for message in alpha["diagnostics"]), "Selected alpha skin reports the opaque shadow approximation on its entity")
    unflagged = bsp.read_bytes()
    mdl_path = game / "models/props/test.mdl"
    original_mdl = mdl_path.read_bytes()
    flagged = bytearray(original_mdl)
    struct.pack_into("<I", flagged, 152, struct.unpack_from("<I", flagged, 152)[0] | 0x00040000)
    mdl_path.write_bytes(flagged)
    alpha_objects = detail(client, [prop])
    alpha_history = client.call("get_map_info")["undo"]
    cutout = client.call("compile_map", {"path": str(bsp)})
    cutout_bytes = bsp.read_bytes()
    stats = cutout["stats"]
    check(cutout["success"] and stats["prop_shadow_alpha_triangles"] == stats["prop_shadow_alpha_materials"] == stats["prop_shadow_alpha_textures"] == 1 and stats["prop_shadow_alpha_bytes"] == 16, "Requested native texture shadows report one cutout triangle, material and shared 4×4 alpha mask")
    check(center_sample(cutout_bytes) == [200, 0, 0] and floor_sample(cutout_bytes, -16, 0) == [0, 0, 0], "Barycentric UV cutouts illuminate the transparent center and retain an opaque texel shadow")
    client.call("start_compile", {"path": str(background)})
    check(wait_job(client)["status"] == "completed" and background.read_bytes() == cutout_bytes, "Synchronous and background builds produce identical alpha-test lightmaps")
    check(detail(client, [prop]) == alpha_objects and client.call("get_map_info")["undo"] == alpha_history and vmf.read_bytes() == source, "Texture-shadow baking preserves authored props, history and VMF bytes")
    material_path = game / "materials/models/test/two.vmt"
    original_material = material_path.read_text()
    material_path.write_text('VertexLitGeneric { $alphatest 1 $basetexture test $basetexturetransform "center 0 0 scale 1 1 translate 0.5 0 rotate 0" }')
    client.call("compile_map", {"path": str(bsp)})
    check(center_sample(bsp.read_bytes()) == [0, 0, 0] and floor_sample(bsp.read_bytes(), -16, 0)[0] > 0, "Material translation and repeat move the sampled cutout across real floor luxels")
    (game / "materials/test.vtf").write_bytes(alpha_texture(128))
    material_path.write_text('VertexLitGeneric { $alphatest 1 $basetexture test $alphatestreference 0.75 }')
    client.call("compile_map", {"path": str(bsp)})
    check(center_sample(bsp.read_bytes()) == [200, 0, 0], "Alpha below the authored reference transmits the complete direct ray")
    material_path.write_text('VertexLitGeneric { $alphatest 1 $basetexture test $alphatestreference 0.5 }')
    client.call("compile_map", {"path": str(bsp)})
    check(center_sample(bsp.read_bytes()) == [0, 0, 0], "The same texel above a lower reference blocks the direct ray")
    material_path.write_text('VertexLitGeneric { $translucent 1 $basetexture test }')
    transmitted = client.call("compile_map", {"path": str(bsp)})
    transmitted_bytes = bsp.read_bytes()
    check(transmitted["success"] and center_sample(transmitted_bytes) == [99.5, 0, 0]
          and floor_sample(transmitted_bytes, -16, 0) == [0, 0, 0],
          "Requested translucent alpha128 transmits 127/255 before RGBExp32 encoding and alpha255 remains opaque")
    client.call("start_compile", {"path": str(background)})
    check(wait_job(client)["status"] == "completed" and background.read_bytes() == transmitted_bytes,
          "Synchronous and background builds produce identical fractional texture shadows")
    check(detail(client, [prop]) == alpha_objects and client.call("get_map_info")["undo"] == alpha_history and vmf.read_bytes() == source,
          "Fractional texture-shadow baking preserves authored props, history and VMF bytes")
    previous = bsp.read_bytes()
    for material, expected in [
        ('VertexLitGeneric { $alphatest 1 $basetexture test $basetexturetransform "rotate invalid" }', "invalid $basetexturetransform"),
        ('VertexLitGeneric { $translucent 1 $alphatest 1 $basetexture test }', "$translucent"),
        ('VertexLitGeneric { $translucent 1 $alpha 0.5 $basetexture test }', "$alpha modulation"),
        ('Refract { $alphatest 1 $basetexture test }', "unsupported texture-shadow shader"),
    ]:
        material_path.write_text(material)
        failed = json.loads(client.call("compile_map", {"path": str(bsp)}, error=True)["content"][0]["text"])
        check(not failed["success"] and bsp.read_bytes() == previous and any(message["object"] == prop and expected in message["message"] for message in failed["diagnostics"]), f"Requested unsupported texture shadows fail on the prop and preserve BSP: {expected}")
    material_path.write_text(original_material)
    (game / "materials/test.vtf").write_bytes(b"VTF\0")
    failed = json.loads(client.call("compile_map", {"path": str(bsp)}, error=True)["content"][0]["text"])
    check(not failed["success"] and bsp.read_bytes() == previous and any(message["object"] == prop and "truncated" in message["message"] for message in failed["diagnostics"]), "Requested corrupt alpha texture fails without overwriting the previous BSP")
    mdl_path.write_bytes(original_mdl)
    (game / "materials/test.vtf").write_bytes(alpha_texture())
    client.call("compile_map", {"path": str(bsp)})
    check(bsp.read_bytes() == unflagged, "Unflagged alpha models retain their original opaque BSP bytes")
    vtx = game / "models/props/test.dx90.vtx"
    broken = bytearray(vtx.read_bytes())
    struct.pack_into("<i", broken, 16, 8)
    vtx.write_bytes(broken)
    previous = bsp.read_bytes()
    failed = json.loads(client.call("compile_map", {"path": str(bsp)}, error=True)["content"][0]["text"])
    check(not failed["success"] and bsp.read_bytes() == previous and any(message["object"] == prop and "checksum" in message["message"] for message in failed["diagnostics"]), "Malformed geometry fails on its entity and preserves the previous BSP")
    client.call("set_entity_properties", {"id": prop, "set": {"disableshadows": "1"}})
    disabled = client.call("compile_map", {"path": str(bsp)})
    check(disabled["stats"]["prop_shadow_triangles"] == 0 and disabled["stats"]["prop_shadow_disabled"] == 1 and center_sample(bsp.read_bytes()) == [200, 0, 0], "Native disableshadows skips mesh decoding and restores direct light")
    client.call("set_entity_properties", {"id": prop, "set": {"disableshadows": "0"}})
    client.call("set_compile_settings", {"lighting": "fullbright"})
    check(client.call("compile_map", {"path": str(bsp)})["success"], "Fullbright requires no shadow geometry")
    return checks


def brush_samples(data, point):
    from validate_authored_planes import records
    from validate_direct_lighting import FACE_FORMAT
    from validate_material_albedo import payload

    planes = records(data, 1, "<4fi")
    infos = records(data, 6, "<16f2i")
    result = {}
    for channel, face_slot, light_slot, version in [("ldr", 7, 8, 0), ("hdr", 58, 53, 1)]:
        colors = payload(data, light_slot)
        samples = []
        for face in records(data, face_slot, FACE_FORMAT, expected_version=version):
            assert face[0] < len(planes) and 0 <= face[5] < len(infos)
            if planes[face[0]][:4] != (0, 0, 1, 0):
                continue
            info = infos[face[5]]
            u = point[0] * info[8] + point[1] * info[9] + info[11] - face[14]
            v = point[0] * info[12] + point[1] * info[13] + info[15] - face[15]
            width, height = face[16] + 1, face[17] + 1
            assert face[6] == -1 and not info[16] & 0x800 and face[8:12] == (0, 255, 255, 255)
            assert 1 <= width <= 65 and 1 <= height <= 65
            if not (0 <= u < width and 0 <= v < height):
                continue
            assert u == int(u) and v == int(v), "Literal point must be an exact native luxel"
            offset = face[12] + (int(v) * width + int(u)) * 4
            assert 0 <= offset <= offset + 4 <= len(colors)
            samples.append(colors[offset:offset + 4].hex())
        assert samples, f"Missing {channel} floor sample at {point}"
        result[channel] = samples
    return result


def validate_brush_shadows(client, output, game, receipt):
    from validate_build_recipes import protected_state
    from validate_material_albedo import compile_pair, digest

    checks = receipt["passed"]

    def check(condition, label):
        assert condition, label
        checks.append(label)

    call = client.call

    def counted_call(name, arguments=None, error=False):
        if name in ("compile_map", "start_compile"):
            receipt["compile_calls"] += 1
        return call(name, arguments, error=error)

    client.call = counted_call
    material = game / "materials/brush-cutout.vmt"
    texture = game / "materials/brush-cutout.vtf"
    original_material = 'LightmappedGeneric { $alphatest 1 $basetexture "brush-cutout" }\n'
    original_texture = bytes(alpha_texture())
    material.write_text(original_material)
    texture.write_bytes(original_texture)
    client.call("new_map", {"discard_changes": True})
    ids = []
    for low, high in ROOM:
        ids.append(client.call("create_block", {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"})["id"])
    for z, entity in [(48, False), (72, True)]:
        block = client.call("create_block", {"mins": [-64, -64, z], "maxs": [64, 64, z + 4], "material": "dev/dev_measuregeneric01"})["id"]
        brush = detail(client, [block])[0]
        horizontal = [face_ref(brush, face) for face in brush["faces"] if abs(face["normal"][2]) == 1]
        check(len(horizontal) == 2, "Each literal slab has two horizontal boundary faces")
        client.call("edit_faces", {"faces": horizontal, "material": "brush-cutout", "u_axis": [1, 0, 0], "v_axis": [0, 1, 0],
                                   "scale_u": 16, "scale_v": 16, "shift_u": 2.5, "shift_v": 0})
        if entity:
            ids.append(client.call("tie_to_entity", {"ids": [block], "classname": "func_detail"})["id"])
        else:
            ids.append(block)
    ids.append(client.call("create_entity", {"classname": "info_player_terrorist", "origin": [-128, -128, 8]})["id"])
    ids.append(client.call("create_entity", {"classname": "light", "origin": [0, 0, 100], "properties": {
        "_light": "255 0 0 200", "_lightHDR": "0 0 255 400", "_constant_attn": "1", "_linear_attn": "0", "_quadratic_attn": "0"}})["id"])
    source = output / "brush-shadows.vmf"
    client.call("save_map", {"path": str(source)})
    token = client.call("begin_property_edit", {"world": True})["draft"]["token"]
    client.call("update_property_edit", {"token": token, "changes": [{"action": "set", "key": "message", "value": "Retained brush-shadow review"}]})
    draft = client.call("get_property_edit")
    protected = protected_state(client)
    initial = client.call("get_compile_settings")
    check(initial["brush_texture_shadows"] is False and not initial["brush_texture_shadowing"]["available"], "Brush texture shadows default off and unavailable in Fullbright")
    client.call("set_compile_settings", {"brush_texture_shadows": True, "physics": "engine"}, error=True)
    check(client.call("get_compile_settings") == initial, "Fullbright refuses the complete settings edit without partial mutation")
    client.call("set_compile_settings", {"lighting": "direct_ldr_hdr", "physics": "none", "bounces": 0, "visibility": "fast", "sky_quality": "fast"})
    client.call("save_build_recipe", {"name": "Opaque brush control"})
    receipt["source_sha256"] = digest(source.read_bytes())
    receipt["literal_samples"] = {"off": {"ldr": "00000000", "hdr": "00000000"}, "on": {"ldr": "c8000000", "hdr": "0000c801"}}

    def pair(label, error=None, retained=None):
        pointfiles = [output / f"{label}{suffix}.lin" for suffix in ("", "-background")]
        sentinel = b"retained brush-shadow pointfile\n"
        if error:
            for path in pointfiles:
                path.write_bytes(sentinel)
        data, measurement = compile_pair(client, output, label, ids, source, game, error, retained)
        check(measurement["background"]["settings"]["brush_texture_shadows"] == measurement["settings"]["brush_texture_shadows"],
              f"{label}: the background job retains its submitted brush shadow setting")
        check(client.call("get_property_edit") == draft and protected_state(client) == protected,
              f"{label}: map, history, camera, selection and populated review remain unchanged")
        if error:
            check(all(path.read_bytes() == sentinel for path in pointfiles), f"{label}: both refusals retain existing pointfiles")
        receipt["cases"][label] = measurement
        return data

    def exact(data, enabled):
        actual = brush_samples(data, (0, 0))
        expected = receipt["literal_samples"]["on" if enabled else "off"]
        check(all(all(row == expected[channel] for row in rows) for channel, rows in actual.items()),
              f"Exact independent LDR/HDR center RGBExp32 bytes with brush texture shadows {enabled}: {actual}")
        opaque = brush_samples(data, (-32, 0))
        check(all(all(row == "00000000" for row in rows) for rows in opaque.values()), "The independently selected opaque texel remains black in both channels")
        return {"center": actual, "opaque": opaque}

    try:
        baseline = pair("brush-off")
        receipt["cases"]["brush-off"]["samples"] = exact(baseline, False)
        client.call("set_compile_settings", {"brush_texture_shadows": True})
        saved = client.call("save_build_recipe", {"name": "Brush cutouts"})
        client.call("set_compile_settings", {"brush_texture_shadows": False})
        client.call("apply_build_recipe", {"id": saved["active"]["id"]})
        check(client.call("get_compile_settings")["brush_texture_shadows"] is True, "A custom recipe restores the shared brush shadow setting")
        settings = client.call("get_compile_settings")
        client.call("set_compile_settings", {"lighting": "fullbright"}, error=True)
        check(client.call("get_compile_settings") == settings, "Fullbright transition refuses while the brush shadow option remains enabled")
        cutout = pair("brush-on")
        receipt["cases"]["brush-on"]["samples"] = exact(cutout, True)
        check(cutout != baseline, "The enabled cutouts change actual compiled lighting")
        material.write_text('LightmappedGeneric { $translucent 1 $basetexture "brush-cutout" }\n')
        pair("brush-fractional-refused", "fractional translucency is unsupported", cutout)
        material.write_text(original_material)
        texture.write_bytes(b"VTF\0")
        pair("brush-corrupt-refused", "brush-cutout", cutout)
        texture.write_bytes(original_texture)
        client.call("set_compile_settings", {"brush_texture_shadows": False})
        restored = pair("brush-off-restored")
        check(restored == baseline, "Restored sources and disabled setting reproduce the complete original BSP bytes")
        check(receipt["compile_calls"] == 10 and len(receipt["cases"]) == 5, "All five foreground/background pairs completed")
    finally:
        material.write_text(original_material)
        texture.write_bytes(original_texture)
        client.call = call
    return checks


def main():
    parser = argparse.ArgumentParser(description="Validate shared shadow compilation using isolated generated game assets; requires an existing headless or graphical MCP-capable binary.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, default=Path("/tmp/hammerr-prop-shadows"))
    parser.add_argument("--brush-texture-shadows", action="store_true", help="Run five exact dual-channel brush-shadow foreground/background pairs, recipe/settings admission and BSP/pointfile/source custody; requires a fresh --output directory under target/.")
    args = parser.parse_args()
    args.output = args.output.resolve()
    if args.brush_texture_shadows:
        if not args.output.is_relative_to(ROOT / "target") or args.output.exists():
            parser.error("--brush-texture-shadows requires a fresh --output directory under target/")
    args.output.mkdir(parents=True, exist_ok=True)
    game = args.output / "game"
    fixture(game)
    with patch.dict(os.environ, {"HAMMERR_RECENT_MAPS_DIR": str(args.output / "recent"), "HAMMERR_BUILD_RECIPES": str(args.output / "recipes.json"),
                               "HAMMERR_PREFABS_DIR": str(args.output / "prefabs"), "XDG_CONFIG_HOME": str(args.output / "config"),
                               "XDG_STATE_HOME": str(args.output / "state")}):
        client = Client(None, args.binary.resolve(), args.output, game / "gameinfo.txt")
    receipt = {"status": "failed", "compile_calls": 0, "cases": {}, "passed": [],
               "binary": str(args.binary.resolve()), "binary_sha256": hashlib.sha256(args.binary.read_bytes()).hexdigest(),
               "script_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest()} if args.brush_texture_shadows else {}
    try:
        if args.brush_texture_shadows:
            from validate_compile_jobs import stopped_on_exit

            with stopped_on_exit(client):
                checks = validate_brush_shadows(client, args.output, game, receipt)
            receipt.update(status="passed", checks=len(checks), passed=checks)
        else:
            checks = validate(client, args.output, game)
            receipt = {"checks": len(checks), "passed": checks}
        print(json.dumps({"checks": len(checks), "output": str(args.output)}))
    finally:
        try:
            if args.brush_texture_shadows:
                from validate_recovery_exit import require_clean_eof

                require_clean_eof(client)
            else:
                client.close()
        except BaseException:
            if args.brush_texture_shadows:
                receipt["status"] = "failed"
            raise
        finally:
            if receipt:
                (args.output / "results.json").write_text(json.dumps(receipt, indent=2) + "\n")


if __name__ == "__main__":
    main()
