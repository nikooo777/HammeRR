import argparse
import json
import os
import struct
from pathlib import Path
from unittest.mock import patch

from validate_face_lighting import DEFAULT_BINARY, Client, detail


def packed_material(path, text):
    payload = text.encode()
    tree = b"vmt\0materials/test\0packed\0"
    tree += struct.pack("<IHHIIH", 0, 0, 0x7FFF, 0, len(payload), 0xFFFF)
    tree += b"\0\0\0"
    path.write_bytes(struct.pack("<III", 0x55AA1234, 1, len(tree)) + tree + payload)


def inspect(client, material="test/selected"):
    return client.call("get_material_source", {"material": material})


def snapshot(client, ids):
    info = client.call("get_map_info")
    return {
        "objects": detail(client, ids),
        "camera": client.call("get_camera"),
        "transform": client.call("get_transform_handles"),
        **{key: info[key] for key in ["undo", "redo", "selection", "unsaved_changes"]},
    }


def reject(client, arguments):
    try:
        client.call("get_material_source", arguments, error=True)
    except RuntimeError as error:
        if not (error.args and isinstance(error.args[0], dict) and error.args[0].get("code") == -32602):
            raise


def validate(args):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    game = args.output / "game"
    first, second = [game / name / "materials/test" for name in ["first", "second"]]
    first.mkdir(parents=True)
    second.mkdir(parents=True)
    selected = first / "selected.vmt"
    text = "Patch { include materials/test/base.vmt replace { $basetexture test/replaced } insert { $envmap env_cubemap } }"
    selected.write_text(text)
    (second / "selected.vmt").write_text("UnlitGeneric { $basetexture wrong_mount }")
    (second / "base.vmt").write_text("UnlitGeneric { $basetexture test/original $bumpmap test/normal }")
    archive = game / "source_dir.vpk"
    packed_material(archive, "UnlitGeneric { $basetexture test/packed }")
    gameinfo = game / "gameinfo.txt"
    gameinfo.write_text('GameInfo { FileSystem { SearchPaths { game |gameinfo_path|first game |gameinfo_path|second game |gameinfo_path|source_dir.vpk } } }')
    with patch.dict(os.environ, {
        "HAMMERR_RECENT_MAPS_DIR": str(args.output / "recent-maps"),
        "HAMMERR_BUILD_RECIPES": str(args.output / "build-recipes.json"),
    }):
        client = Client(None, args.binary, args.output, gameinfo)
    try:
        tools = {tool["name"]: tool for tool in client.rpc("tools/list", {})["tools"]}
        check("get_material_source" in tools and tools["get_material_source"].get("annotations", {}).get("readOnlyHint") is True, "Material source inspection is discoverable and read-only")
        check(tools["get_material_source"]["inputSchema"].get("additionalProperties") is False, "Material source schema rejects undeclared arguments")
        brush = client.call("create_block", {"mins": [0, 0, 0], "maxs": [64, 64, 64], "material": "test/selected"})["id"]
        source = args.output / "source.vmf"
        client.call("save_map", {"path": str(source)})
        saved = source.read_bytes()
        client.call("transform_objects", {"ids": [brush], "translation": [16, 0, 0], "preview": True})
        before = snapshot(client, [brush])
        report = inspect(client, "MATERIALS\\TEST\\SELECTED.VMT")
        check(report["material"] == "test/selected" and report["vmt_path"] == "materials/test/selected.vmt", "Material names normalize to their exact game VMT path")
        check(report["raw_source"] == text and not report["raw_source_truncated"], "Selected raw source is returned without alteration")
        check(report["files"][0]["origin"]["file"] == str(selected) and report["files"][1]["origin"]["file"] == str(second / "base.vmt"), "Each VMT reports the winning directory and actual file path")
        textures = {row["parameter"]: row["value"] for row in report["resolved"]["texture_parameters"]}
        check(report["error"] is None and report["resolved"]["shader"] == "unlitgeneric" and textures == {"$basetexture": "test/replaced", "$bumpmap": "test/normal", "$envmap": "env_cubemap"}, "Patch declarations resolve while texture values remain explicit names")
        check(report["limits"]["source_bytes"] == 65536 and report["limits"]["file_bytes"] == 1048576 and report["limits"]["total_bytes"] == 4194304 and report["limits"]["files"] == 9, "Source and resolution limits are exposed")
        for content in [b"invalid", b"Patch { include materials/test/missing.vmt }", b"UnlitGeneric { $basetexture \xff }"]:
            selected.write_bytes(content)
            failed = inspect(client)
            check(failed["error"] and failed["resolved"] is None and failed["files"][0]["origin"]["file"] == str(selected), f"Higher-priority source errors preserve provenance without healthy fallback: {content!r}")
            if b"\xff" in content:
                check("UTF-8" in failed["error"] and failed["raw_source"] is None, "Invalid UTF-8 is explicit")
            else:
                check(failed["raw_source"] == content.decode(), "Malformed source remains readable")
        selected.unlink()
        selected.symlink_to("missing-target.vmt")
        failed = inspect(client)
        check(failed["error"] and "dangling" in failed["error"] and failed["resolved"] is None, "A dangling higher-priority VMT does not expose lower-priority source")
        selected.unlink()
        selected.symlink_to("selected.vmt")
        failed = inspect(client)
        check(failed["error"] and failed["resolved"] is None, "An unreadable higher-priority symlink loop does not expose lower-priority source")
        selected.unlink()
        selected.write_text("UnlitGeneric { $basetexture test/exact }\n// " + "雪" * 65536)
        bounded = inspect(client)
        check(bounded["error"] is None and bounded["raw_source_truncated"] and len(bounded["raw_source"].encode()) <= 65536 and selected.read_text().startswith(bounded["raw_source"]), "Raw source truncation is explicit and preserves UTF-8 character boundaries")
        with selected.open("wb") as stream:
            stream.truncate(1048577)
        failed = inspect(client)
        check(failed["error"] and "limit" in failed["error"] and failed["raw_source"] is None and failed["resolved"] is None, "Oversized files reject before source text is allocated")
        selected.write_text(text)
        for arguments in [{}, {"material": "../outside"}, {"material": "/absolute"}, {"material": ""}, {"material": "x" * 513}, {"material": "test/selected", "extra": True}]:
            reject(client, arguments)
        packed = inspect(client, "test/packed")
        check(packed["error"] is None and packed["files"][0]["origin"]["kind"] == "vpk" and packed["files"][0]["origin"]["mount"] == str(archive) and str(archive) in packed["files"][0]["origin"]["storage_files"], "Packed VMT inspection reports its winning VPK and storage paths")
        archive.write_bytes(b"changed mounted index")
        failed = inspect(client, "test/packed")
        check(failed["error"] and "reload mounted content" in failed["error"] and failed["resolved"] is None, "Changed mounted VPK indexes reject stale provenance")
        check(snapshot(client, [brush]) == before and source.read_bytes() == saved, "Every inspection preserves map bytes, history, selection, camera and active transform draft")
        result = {"checks": len(checks), "passed": checks, "transport": "stdio", "compile_or_game_behavior_exercised": False}
        (args.output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps({"material_source": True, "checks": len(checks), "output": str(args.output)}))
    finally:
        client.close()


def main():
    parser = argparse.ArgumentParser(description="Validate bounded material source/provenance inspection through isolated MCP and private content mounts.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.binary, args.output = args.binary.resolve(), args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=False)
    validate(args)


if __name__ == "__main__":
    main()
