import argparse
import json
import struct
from pathlib import Path

from validate_compile_jobs import wait_job
from validate_direct_lighting import center_sample, floor_samples
from validate_face_lighting import ROOT, Client, detail
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


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument("--output", type=Path, default=Path("/tmp/hammerr-prop-shadows"))
    args = parser.parse_args()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    game = args.output / "game"
    fixture(game)
    client = Client(None, args.binary.resolve(), args.output, game / "gameinfo.txt")
    try:
        checks = validate(client, args.output, game)
        (args.output / "results.json").write_text(json.dumps({"checks": len(checks), "passed": checks}, indent=2) + "\n")
        print(json.dumps({"checks": len(checks), "output": str(args.output)}))
    finally:
        client.close()


if __name__ == "__main__":
    main()
