import argparse
import json
import struct
from pathlib import Path
from unittest.mock import patch

from validate_compile_jobs import stopped_on_exit, wait_job
from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_visibility import ROOM
from validate_water_currents import game_lump_relative_digest

DIAGNOSTIC = "Unsupported authored initial brush-model lighting pose"
MARKER = '"targetname" "pose_test"'


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def payloads(data):
    require(data.startswith(b"VBSP"), "Missing native BSP framing")
    result = []
    for index in range(1, 64):
        offset, size, version, fourcc = struct.unpack_from("<4i", data, 8 + index * 16)
        require(0 <= offset <= len(data) and 0 <= size <= len(data) - offset, "Bad lump range")
        content = game_lump_relative_digest(data) if index == 35 else data[offset:offset + size]
        result.append((version, fourcc, content))
    return result


def state(client, ids, path):
    info = client.call("get_map_info")
    return detail(client, ids), info["undo"], info["selection"], path.read_bytes()


def pose_diagnostic(result, owner):
    return any(
        item["level"] == "error"
        and item["object"] == owner
        and DIAGNOSTIC in item["message"]
        for item in result["diagnostics"]
    )


def fixture(client):
    client.call("new_map", {"discard_changes": True})
    ids = []
    for low, high in ROOM:
        ids.append(client.call("create_block", {
            "mins": low, "maxs": high, "material": "dev/dev_measuregeneric01",
        })["id"])
    ids.append(client.call("create_entity", {
        "classname": "info_player_terrorist", "origin": [-64, 0, 8],
    })["id"])
    ids.append(client.call("create_entity", {
        "classname": "light", "origin": [-96, -96, 224],
        "properties": {"_light": "255 255 255 150", "_constant_attn": "1", "_quadratic_attn": "0"},
    })["id"])
    solid = client.call("create_block", {
        "mins": [-32, -32, 32], "maxs": [32, 32, 96],
        "material": "dev/dev_measuregeneric01",
    })["id"]
    owner = client.call("tie_to_entity", {
        "ids": [solid], "classname": "func_door",
        "properties": {"origin": "101.5 -67.25 32.125", "targetname": "pose_test"},
    })["id"]
    ids.append(owner)
    client.call("set_entity_properties", {"id": owner, "remove": ["angle", "angles"]})
    trigger = client.call("create_block", {
        "mins": [96, 96, 0], "maxs": [160, 160, 64], "material": "tools/toolstrigger",
    })["id"]
    ids.append(client.call("tie_to_entity", {
        "ids": [trigger], "classname": "trigger_multiple", "properties": {"angles": "20 35 10"},
    })["id"])
    return owner, ids


def validate(client, output):
    owner, ids = fixture(client)
    original = output / "fixture.vmf"
    client.call("save_map", {"path": str(original)})
    source = original.read_text()
    require(source.count(MARKER) == 1, "Fixture owner is not unique")
    cases = [
        ("absent", [], False),
        ("signed-zero", [("AnGlEs", "-0 +0.0 0e-300")], False),
        ("legacy-zero", [("angle", "-0.000")], False),
        ("pitch", [("angles", "1 0 0")], True),
        ("yaw", [("angles", "0 -90 0")], True),
        ("roll", [("angles", "0 0 45")], True),
        ("mixed", [("angles", "20 35 10")], True),
        ("legacy-up", [("angle", "-1")], True),
        ("legacy-down", [("angle", "-2")], True),
        ("invalid", [("angles", "bad")], True),
        ("nonfinite", [("angles", "NaN 0 0")], True),
        ("source-underflow", [("angles", "1e-4000 0 0")], True),
        ("native-underflow", [("angle", "1e-50")], True),
        ("duplicate", [("angles", "0 0 0"), ("AnGlEs", "0 90 0")], True),
        ("coexisting", [("angles", "0 0 0"), ("angle", "0")], True),
        ("suffix", [("angles#1", "0 0 0")], True),
        ("padded-key", [("angles ", "0 90 0")], True),
        ("padded-late-override", [("angles", "0 0 0"), ("AnGlEs ", "0 90 0")], True),
    ]
    checks = []
    for lighting in ["direct_ldr", "direct_ldr_hdr"]:
        reference = None
        for name, properties, rejected in cases:
            vmf = output / f"{lighting}-{name}.vmf"
            fields = "".join(f'\n\t"{key}" "{value}"' for key, value in properties)
            vmf.write_text(source.replace(MARKER, MARKER + fields))
            client.call("open_map", {"path": str(vmf), "discard_changes": True})
            client.call("set_compile_settings", {
                "lighting": lighting, "physics": "none", "bounces": 1,
                "generate_reflections": False, "pack_sky_assets": False,
            })
            before = state(client, ids, vmf)
            target = output / f"{lighting}-{name}.bsp"
            if rejected:
                require(reference is not None, "Missing accepted prior BSP")
                target.write_bytes(reference)
            response = client.call("compile_map", {"path": str(target)}, error=rejected)
            result = json.loads(response["content"][0]["text"]) if rejected else response
            if rejected:
                require(not result["success"] and result["bsp"] is None and pose_diagnostic(result, owner), f"{name}: missing synchronous refusal")
                require(target.read_bytes() == reference, f"{name}: synchronous refusal replaced prior BSP")
            else:
                require(result["success"] and result["stats"]["lit_brush_models"] == 1 and result["stats"]["model_lighting_faces"] == 6, f"{name}: receiver/trigger admission changed")
                if reference is None:
                    reference = target.read_bytes()
                require(payloads(target.read_bytes()) == payloads(reference), f"{name}: exact-zero baked payloads changed")
            foreground = target.read_bytes()
            client.call("start_compile", {"path": str(target)})
            job = wait_job(client)
            if rejected:
                require(job["status"] == "failed" and not job["result"]["published"] and pose_diagnostic(job["result"], owner), f"{name}: missing background refusal")
            else:
                require(job["status"] == "completed" and job["result"]["success"] and job["result"]["published"], f"{name}: background admission failed")
            require(target.read_bytes() == foreground, f"{name}: background path changed BSP")
            require(state(client, ids, vmf) == before, f"{name}: compile changed source, geometry, selection or history")
            checks.append({
                "lighting": lighting, "case": name, "rejected": rejected,
                "blocking_result": result, "background_job": job,
            })
    fullbright_vmf = output / "direct_ldr_hdr-mixed.vmf"
    client.call("open_map", {"path": str(fullbright_vmf), "discard_changes": True})
    client.call("set_compile_settings", {"lighting": "fullbright"})
    before = state(client, ids, fullbright_vmf)
    fullbright = output / "fullbright-nonzero-pose.bsp"
    require(client.call("compile_map", {"path": str(fullbright)})["success"], "Fullbright refused authored pose")
    previous = fullbright.read_bytes()
    client.call("start_compile", {"path": str(fullbright)})
    require(wait_job(client)["status"] == "completed" and fullbright.read_bytes() == previous, "Background Fullbright changed admission or output")
    require(state(client, ids, fullbright_vmf) == before, "Fullbright changed source, geometry, selection or history")
    result = {
        "success": True, "transport": "private stdio", "checks": checks,
        "prior_bsp_preserved": True, "source_geometry_history_preserved": True,
        "fullbright_nonzero_and_trigger_unlit_admitted": True,
        "native_exercised": False, "desktop_input": False,
    }
    (output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"success": True, "checks": len(checks), "output": str(output)}))


def main():
    parser = argparse.ArgumentParser(description="Validate receiver-only initial brush-model pose admission through both compile MCP paths.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True)
    game = args.output / "game"
    (game / "materials/tools").mkdir(parents=True)
    (game / "materials/dev").mkdir()
    (game / "materials/dev/dev_measuregeneric01.vmt").write_text("LightmappedGeneric {}\n")
    (game / "materials/tools/toolstrigger.vmt").write_text("UnlitGeneric { %compiletrigger 1 }\n")
    gameinfo = game / "gameinfo.txt"
    gameinfo.write_text('GameInfo { FileSystem { SearchPaths { game "|gameinfo_path|." } } }\n')
    with patch.dict("os.environ", {
        "HAMMERR_RECENT_MAPS_DIR": str(args.output / "recent"),
        "HAMMERR_BUILD_RECIPES": str(args.output / "recipes.json"),
        "HAMMERR_PREFABS_DIR": str(args.output / "prefabs"),
        "XDG_CONFIG_HOME": str(args.output / "config"),
        "XDG_STATE_HOME": str(args.output / "state"),
    }):
        client = Client(None, args.binary.resolve(), args.output, gameinfo)
    try:
        with stopped_on_exit(client):
            validate(client, args.output)
    finally:
        client.close()


if __name__ == "__main__":
    main()
