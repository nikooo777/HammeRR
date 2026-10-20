import argparse
import json
import os
import struct
from pathlib import Path
from unittest.mock import patch

import validate_material_read_limits as limits
from validate_compile_jobs import stopped_on_exit
from validate_face_lighting import DEFAULT_BINARY, ROOT, detail, face_ref
from validate_material_albedo import digest, open_client, payload, require
from validate_recovery_exit import require_clean_eof
from validate_static_props import static_records
from validate_visibility import ROOM

MODEL = "models/hammerr694/probe.mdl"
DIRECTORY_MODEL = "models/hammerr694/directory.mdl"
ARCHIVE_MODEL = "models/hammerr694/archive.mdl"
ORIGIN = (32.125, 48.25, 8.375)
OVERSIZED_BYTES = 64 * 1024 * 1024 + 1
LIMITS_SOURCE_SHA = "c9e7d4e622be3afd3f37fc2774e82eb061e36fa42cccadd88426f8a0c7c4dcca"


def model_bytes():
    data = bytearray(240)
    data[:4] = b"IDST"
    for offset, value in ((4, 48), (76, 240), (152, 16), (224, 2)):
        struct.pack_into("<i", data, offset, value)
    for offset, values in ((104, (-16, -8, 0)), (116, (16, 8, 32)),
                           (128, (-16, -8, 0)), (140, (16, 8, 32))):
        struct.pack_into("<3f", data, offset, *values)
    return bytes(data)


def physics_bytes(checksum=0):
    data = bytearray(61)
    struct.pack_into("<5i", data, 0, 16, 0, 1, checksum, 40)
    struct.pack_into("<4sHHI3fI", data, 20, b"VPHY", 0x100, 0, 12, 1, 2, 3, 0)
    return bytes(data)


def sparse(path, prefix, size):
    with path.open("wb") as destination:
        destination.write(prefix)
        destination.truncate(size)


def assets(game):
    limits.assets(game)
    selected, fallback = game / "selected", game / "fallback"
    for directory in (selected, fallback):
        (directory / "models/hammerr694").mkdir(parents=True)
    for name in (MODEL, DIRECTORY_MODEL):
        (selected / name).write_bytes(model_bytes())
    (selected / MODEL.replace(".mdl", ".phy")).write_bytes(physics_bytes())
    sparse(selected / DIRECTORY_MODEL.replace(".mdl", ".phy"), physics_bytes(), OVERSIZED_BYTES)
    for name in (DIRECTORY_MODEL, ARCHIVE_MODEL):
        (fallback / name).write_bytes(model_bytes())
        (fallback / name.replace(".mdl", ".phy")).write_bytes(physics_bytes())
    tree = b"phy\0models/hammerr694\0archive\0"
    tree += struct.pack("<IHHIIH", 0, 0, 0x7FFF, 0, OVERSIZED_BYTES, 0xFFFF) + b"\0\0\0"
    prefix = struct.pack("<III", 0x55AA1234, 1, len(tree)) + tree
    sparse(game / "winner_dir.vpk", prefix + physics_bytes(), len(prefix) + OVERSIZED_BYTES)
    (game / "gameinfo.txt").write_text(
        'GameInfo { FileSystem { SearchPaths { game "|gameinfo_path|selected" '
        'game "|gameinfo_path|winner_dir.vpk" game "|gameinfo_path|fallback" '
        'game "|gameinfo_path|." } } }\n')


def fixture(client, source):
    ids = []
    for low, high in ROOM:
        identifier = client.call("create_block", {"mins": low, "maxs": high, "material": limits.MATERIAL})["id"]
        brush = detail(client, [identifier])[0]
        client.call("edit_faces", {"faces": [face_ref(brush, face) for face in brush["faces"]], "lightmap_scale": 64})
        ids.append(identifier)
    ids.append(client.call("create_entity", {"classname": "info_player_terrorist", "origin": [-64, 0, 8]})["id"])
    prop = client.call("create_entity", {"classname": "prop_static", "origin": list(ORIGIN),
                       "properties": {"model": MODEL, "solid": "6", "angles": "0 90 0", "skin": "1",
                                      "fademindist": "64", "fademaxdist": "128", "fadescale": "0.5",
                                      "disableshadows": "1", "mindxlevel": "80", "maxdxlevel": "95"}})["id"]
    ids.append(prop)
    client.call("select", {"ids": ids[:1]})
    limits.configure(client)
    client.call("save_build_recipe", {"name": "Static prop physics limits"})
    client.call("save_map", {"path": str(source)})
    limits.drafts(client, ids)
    return ids, prop


def literal_records(data, model=MODEL, solid=6):
    record = static_records(data)
    require(record["models"] == [model] and len(record["props"]) == 1, "Static-prop dictionary/count differs")
    row = record["props"][0]
    require(row[:6] == (*ORIGIN, 0, 90, 0) and row[6:8] == (0, 0)
            and row[8] > 0 and row[9:14] == (solid, 17, 1, 64, 128)
            and row[14:20] == (0, 0, 0, 0.5, 80, 95), f"Literal native sprp fields differ: {row}")
    require(row[8] == len(record["leaves"]) and len(set(record["leaves"])) == row[8], "Native prop leaf range differs")
    require(payload(data, 28) == payload(data, 29) == b"", "Physics None emitted brush/terrain collision")
    require(b"prop_static" not in payload(data, 0), "Static prop remained in runtime entity text")
    return {"model": model, "origin": list(ORIGIN), "angles": [0, 90, 0], "solid": solid,
            "flags": 17, "skin": 1, "fade": [64, 128, 0.5], "dx_levels": [80, 95],
            "leaf_references": len(record["leaves"]), "physics_lumps_empty": True}


def old_control(client, output, game):
    source = output / "source.vmf"
    ids, prop = fixture(client, source)
    data, receipt = limits.compile_pair(client, output, "old-valid", ids, source, game)
    receipt["literal_records"] = literal_records(data)
    return {"normal": receipt, "ids": ids, "prop": prop}


def select_model(client, ids, prop, model, solid):
    client.call("set_entity_properties", {"id": prop, "set": {"model": model, "solid": str(solid)}})
    limits.drafts(client, ids)


def validate(args):
    client = None
    result = {"status": "failed", "binary": str(args.binary), "binary_fingerprint": limits.fingerprint(args.binary),
              "oracle_fingerprint": limits.fingerprint(Path(__file__)), "shared_helper_fingerprint": limits.fingerprint(Path(limits.__file__)),
              "native_or_graphics_exercised": False, "positive": {}, "negative": {}}
    try:
        require(result["shared_helper_fingerprint"]["sha256"] == LIMITS_SOURCE_SHA, "Use the frozen 693 v2 shared helper")
        old = json.loads(args.old_controls.read_text())
        require(old["status"] == "accepted_old_valid" and old["binary_fingerprint"]["sha256"] == limits.OLD_BINARY_SHA
                and old["oracle_fingerprint"] == result["oracle_fingerprint"]
                and old["shared_helper_fingerprint"] == result["shared_helper_fingerprint"], "Retained old control source/binary differs")
        original = old["controls"]["normal"]
        source, retained = Path(original["source"]), Path(original["bsp"])
        require(limits.fingerprint(source) == original["source_fingerprint"]
                and limits.fingerprint(retained)["sha256"] == original["bsp_sha256"], "Retained old source/BSP changed")
        game = args.output / "game"
        assets(game)
        require(limits.fingerprints(game) == original["asset_fingerprints"], "Old/new private literal assets differ")
        temporary = args.output / "tmp"
        temporary.mkdir()
        with patch.dict(os.environ, {"TMPDIR": str(temporary)}):
            client = open_client(args.binary, args.output / "client", game)
        tools = {tool["name"]: tool for tool in client.rpc("tools/list", {})["tools"]}
        require(all(name in tools for name in ("compile_map", "start_compile")), "Both shared compile tools must be discoverable")
        result["discovery"] = {name: tools[name] for name in ("compile_map", "start_compile")}
        client.call("open_map", {"path": str(source), "discard_changes": True})
        ids, prop = old["controls"]["ids"], old["controls"]["prop"]
        require({row["id"] for row in client.call("find_objects", {"limit": 100})["objects"]} == set(ids), "Retained source object IDs differ")
        client.call("select", {"ids": ids[:1]})
        limits.configure(client)
        client.call("save_build_recipe", {"name": "Static prop physics limits"})
        limits.drafts(client, ids)
        baseline, receipt = limits.compile_pair(client, args.output, "valid", ids, source, game)
        require(digest(baseline) == original["bsp_sha256"], "Valid whole BSP changed from the pinned old binary")
        receipt["literal_records"] = literal_records(baseline)
        result["positive"]["valid"] = receipt
        phy = game / "selected" / MODEL.replace(".mdl", ".phy")
        for name, data, errors in (("corrupt", b"broken PHY", (MODEL.replace(".mdl", ".phy"), "file size")),
                                   ("checksum", physics_bytes(1), (MODEL.replace(".mdl", ".phy"), "checksum")),
                                   ("missing", None, (MODEL, ".phy", "collision"))):
            if data is None:
                phy.unlink()
            else:
                phy.write_bytes(data)
            _, result["negative"][name] = limits.compile_pair(client, args.output, name, ids, source, game, baseline, errors)
        phy.write_bytes(physics_bytes())
        for name, model, errors in (("directory-winner", DIRECTORY_MODEL, (DIRECTORY_MODEL.replace(".mdl", ".phy"), "byte read limit")),
                                    ("vpk-winner", ARCHIVE_MODEL, (ARCHIVE_MODEL.replace(".mdl", ".phy"), "VPK", "byte read limit"))):
            select_model(client, ids, prop, model, 6)
            _, result["negative"][name] = limits.compile_pair(client, args.output, name, ids, source, game, baseline, errors)
        for solid in (0, 2):
            select_model(client, ids, prop, DIRECTORY_MODEL, solid)
            data, receipt = limits.compile_pair(client, args.output, f"unused-solid-{solid}", ids, source, game)
            receipt["literal_records"] = literal_records(data, DIRECTORY_MODEL, solid)
            result["positive"][f"unused-solid-{solid}"] = receipt
        select_model(client, ids, prop, MODEL, 6)
        data, receipt = limits.compile_pair(client, args.output, "restored", ids, source, game)
        require(data == baseline, "Restoring the valid model changed whole BSP after refusals")
        result["positive"]["restored"] = receipt
        before = limits.snapshot(client, ids, source, game)
        result["clean_eof"] = require_clean_eof(client)
        require(limits.fingerprint(source) == before["source"] and limits.fingerprints(game) == before["assets"]
                and limits.fingerprint(Path(before["recipes"]["path"])) == before["recipe_file"], "EOF changed retained inputs")
        result["old_receipt_fingerprint"] = limits.fingerprint(args.old_controls)
        result["sparse_logical_payload_bytes"] = OVERSIZED_BYTES
        result["status"] = "accepted"
    except Exception as error:
        result["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        try:
            if client is not None and client.process is not None:
                with stopped_on_exit(client):
                    pass
                require_clean_eof(client)
        except Exception as error:
            result["cleanup_error"] = f"{type(error).__name__}: {error}"
            result["status"] = "failed"
            raise
        finally:
            (args.output / "results.json").write_text(json.dumps(result, indent=2) + "\n")
    return result


def main():
    parser = argparse.ArgumentParser(description="Validate selected static-prop PHY byte limits through both MCP compile paths, using streamed sparse assets and a pinned old valid BSP.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--old-controls", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.binary, args.old_controls, args.output = args.binary.resolve(), args.old_controls.resolve(), args.output.resolve()
    require(args.output.is_relative_to(ROOT / "target/validation"), "Artifacts must stay under target/validation")
    args.output.mkdir(parents=True, exist_ok=False)
    result = validate(args)
    print(json.dumps({"status": result["status"], "positive": len(result["positive"]), "negative": len(result["negative"])}))


if __name__ == "__main__":
    main()
