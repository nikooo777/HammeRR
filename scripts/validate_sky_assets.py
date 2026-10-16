import argparse
import json
import os
import struct
from pathlib import Path
from unittest.mock import patch

from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_sky_preview import set_sky


def texture(hdr=False):
    data = bytearray(80)
    data[:4] = b"VTF\0"
    struct.pack_into("<IIIHH", data, 4, 7, 2, 80, 2, 2)
    struct.pack_into("<H", data, 24, 1)
    struct.pack_into("<I", data, 52, 24 if hdr else 0)
    data[56] = 1
    struct.pack_into("<i", data, 57, -1)
    struct.pack_into("<H", data, 63, 1)
    return bytes(data) + (struct.pack("<4e", 1.0, 0.5, 0.25, 1.0) if hdr else bytes([80, 120, 180, 255])) * 4


def packed_texture(path, payload):
    tree = b"vtf\0materials/skybox\0hdr\0"
    tree += struct.pack("<IHHIIH", 0, 0, 0x7FFF, 0, len(payload), 0xFFFF)
    tree += b"\0\0\0"
    path.write_bytes(struct.pack("<III", 0x55AA1234, 1, len(tree)) + tree + payload)


def state(client, brush, property_draft):
    info = client.call("get_map_info")
    return {
        "objects": detail(client, [brush]),
        "world": client.call("get_property_inspector", {"world": True}),
        "camera": client.call("get_camera"),
        "transform": client.call("get_transform_handles"),
        "clipboard": client.call("get_clipboard"),
        "property": client.call("get_property_edit") if property_draft else None,
        **{key: info[key] for key in ["undo", "redo", "selection", "unsaved_changes", "view"]},
    }


def validate(args):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    game = args.output / "game"
    first, second = [game / mount / "materials/skybox" for mount in ["first", "second"]]
    first.mkdir(parents=True)
    second.mkdir(parents=True)
    ldr_material = "UnlitGeneric { $basetexture skybox/ldr }"
    hdr_material = "Sky { $basetexture skybox/ldr $hdrbasetexture skybox/hdr }"
    for suffix in ["rt", "bk", "lf", "ft", "up", "dn"]:
        (first / f"fixture{suffix}.vmt").write_text(ldr_material)
        (second / f"fixture{suffix}.vmt").write_text(ldr_material)
        (first / f"fixture_hdr{suffix}.vmt").write_text(hdr_material)
    (second / "hdr_base.vmt").write_text(hdr_material)
    (first / "fixture_hdrrt.vmt").write_text("Patch { include materials/skybox/hdr_base.vmt }")
    (first / "ldr.vtf").write_bytes(texture())
    archive = game / "sky_dir.vpk"
    packed_texture(archive, texture(hdr=True))
    gameinfo = game / "gameinfo.txt"
    gameinfo.write_text('GameInfo { FileSystem { SearchPaths { game |gameinfo_path|first game |gameinfo_path|second game |gameinfo_path|sky_dir.vpk } } }')
    with patch.dict(os.environ, {
        "HAMMERR_RECENT_MAPS_DIR": str(args.output / "recent-maps"),
        "HAMMERR_BUILD_RECIPES": str(args.output / "build-recipes.json"),
        "HAMMERR_PREFABS_DIR": str(args.output / "prefabs"),
    }):
        client = Client(None, args.binary, args.output, gameinfo)
    try:
        tools = {tool["name"]: tool for tool in client.rpc("tools/list", {})["tools"]}
        tool = tools["inspect_sky_assets"]
        check(tool.get("annotations", {}).get("readOnlyHint") is True, "Sky inspection is discoverable and read-only")
        check(tool["inputSchema"].get("additionalProperties") is False and not tool["inputSchema"].get("properties"), "Schema accepts only empty arguments")
        brush = client.call("create_block", {"mins": [0, 0, 0], "maxs": [64, 64, 64], "material": "test/unused"})["id"]
        set_sky(client, "fixture")
        source = args.output / "source.vmf"
        client.call("save_map", {"path": str(source)})
        saved = source.read_bytes()
        client.call("transform_objects", {"ids": [brush], "translation": [16, 0, 0], "preview": True})
        token = client.call("begin_property_edit", {"world": True})["draft"]["token"]
        client.call("update_property_edit", {"token": token, "changes": [{"action": "set", "key": "skyname", "value": "pending_family"}]})

        def inspect(label, property_draft=False):
            before = state(client, brush, property_draft)
            assets = {path.relative_to(game).as_posix(): path.read_bytes() for path in game.rglob("*") if path.is_file()}
            report = client.call("inspect_sky_assets")
            check(state(client, brush, property_draft) == before and source.read_bytes() == saved, f"{label}: document, history, camera, selection, clipboard and drafts stay unchanged")
            check({path.relative_to(game).as_posix(): path.read_bytes() for path in game.rglob("*") if path.is_file()} == assets, f"{label}: mounted source bytes stay unchanged")
            (args.output / f"{label}.json").write_text(json.dumps(report, indent=2) + "\n")
            return report

        report = inspect("ldr-with-pending-edits", property_draft=True)
        check(report["authored_name"] == "fixture" and report["ldr_ready"] and not report["hdr_ready"], "Inspection reads the applied LDR family while a different skyname is staged")
        check([face["suffix"] for face in report["faces"]] == ["rt", "bk", "lf", "ft", "up", "dn"], "All six exact authored material names are reported in native face order")
        check(all(face["material"] == f"skybox/fixture{face['suffix']}" for face in report["faces"]), "No HDR family is substituted")
        limits = {key: report["limits"][key] for key in ["asset_lookups", "total_bytes", "material_bytes", "texture_axis"]}
        check(limits == {"asset_lookups": 128, "total_bytes": 67108864, "material_bytes": 1048576, "texture_axis": 4096} and "active channel" in report["readiness_scope"], "Read limits and structural-only readiness scope travel with the report")
        check(all(face["source"]["kind"] == "directory" and face["ldr"]["textures"][0]["size"] == [2, 2] for face in report["faces"]), "Directory provenance and parsed dimensions are available for every face")
        client.call("cancel_property_edit", {"token": token})
        client.call("cancel_transform_preview")
        set_sky(client, "fixture_hdr")
        report = inspect("paired-hdr")
        check(report["authored_name"] == "fixture_hdr" and report["ldr_ready"] and report["hdr_ready"], "Exact Sky family reports both structural channels")
        check(len(report["faces"][0]["material_sources"]) == 2 and report["faces"][0]["material_sources"][1]["source"]["file"] == str(second / "hdr_base.vmt"), "Patch include provenance identifies the actual winning mount")
        hdr = report["faces"][0]["hdr"]["textures"][0]
        check(hdr["parameter"] == "$hdrbasetexture" and hdr["reference"] == "skybox/hdr" and hdr["format"] == "Rgba16161616F" and hdr["source"]["kind"] == "vpk" and hdr["source"]["index"] == str(archive), "HDR references, format and VPK provenance remain explicit")
        (first / "hdr.vtf").write_bytes(b"malformed higher-priority HDR texture")
        failed = inspect("malformed-hdr")
        check(failed["ldr_ready"] and not failed["hdr_ready"] and all(face["hdr"]["textures"][0]["source"]["file"] == str(first / "hdr.vtf") for face in failed["faces"]), "Malformed winning HDR texture stays visible without packed fallback or loss of LDR readiness")
        (first / "hdr.vtf").unlink()
        (first / "fixture_hdrup.vmt").write_text("Sky { $basetexture skybox/ldr $hdrcompressedtexture skybox/hdr }")
        failed = inspect("compressed-hdr")
        check(failed["ldr_ready"] and not failed["hdr_ready"] and any("compressed HDR" in issue for face in failed["faces"] for issue in face["hdr"]["issues"]), "Unsupported compressed HDR encoding is explicit while its texture reference remains inspectable")
        set_sky(client, "fixture")
        (first / "fixturert.vmt").write_bytes(b"invalid winning material")
        failed = inspect("malformed-material")
        check(not failed["ldr_ready"] and failed["faces"][0]["issues"] and failed["faces"][0]["source"]["file"] == str(first / "fixturert.vmt"), "Malformed winning material is reported without healthy lower-mount substitution")
        (first / "fixturert.vmt").write_text(ldr_material)
        (first / "fixtureup.vmt").unlink()
        (second / "fixtureup.vmt").unlink()
        failed = inspect("missing-face")
        check(not failed["ldr_ready"] and len(failed["faces"]) == 6 and failed["faces"][4]["issues"], "One missing face prevents family readiness and leaves the other five inspectable")
        for index, name in enumerate([" fixture ", "../fixture", "fixture\n", "missing_family"]):
            set_sky(client, name)
            failed = inspect(f"invalid-family-{index}")
            check(failed["authored_name"] == name and not failed["ldr_ready"] and not failed["hdr_ready"], "Invalid or absent family retains exact authored identity with unavailable channels")
            check(bool(failed["issues"]) if index < 3 else len(failed["faces"]) == 6, "Invalid names have actionable family issues; missing valid names retain six face diagnostics")
        try:
            client.call("inspect_sky_assets", {"skyname": "fixture_hdr"}, error=True)
        except RuntimeError as error:
            check(error.args[0].get("code") == -32602, "Undeclared family override is rejected by the protocol schema")
        else:
            checks.append("Undeclared family override is rejected by the tool schema")
        result = {"checks": len(checks), "passed": checks, "transport": "stdio", "render_or_game_behavior_exercised": False}
        (args.output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps({"sky_assets": True, "checks": len(checks), "output": str(args.output)}))
    finally:
        client.close()


def main():
    parser = argparse.ArgumentParser(description="Validate read-only sky-family structure and provenance through isolated MCP and private directory/VPK mounts.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.binary, args.output = args.binary.resolve(), args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=False)
    validate(args)


if __name__ == "__main__":
    main()
