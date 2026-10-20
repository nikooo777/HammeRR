import argparse
import json
import math
import os
import re
import struct
from pathlib import Path
from unittest.mock import patch

import validate_cubemap_capture_probe as probe
from validate_bump_lighting import texture
from validate_build_recipes import protected_state
from validate_compile_jobs import stopped_on_exit, wait_job
from validate_face_lighting import DEFAULT_BINARY, ROOT, Client, detail
from validate_recovery_exit import require_clean_eof
from validate_static_props import static_records
from validate_visibility import ROOM

MANIFEST = "hammerr/generated-prop-lighting.json"
VHV_NAME = re.compile(r"sp_(?:hdr_)?[0-9]+\.vhv\Z")
WALL_MATERIAL = "hammerr_prop_vertex/wall"


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def asset_name(value, suffix):
    require("\\" not in value and not value.startswith("/") and ".." not in value.split("/"), f"Unsafe fixture asset name: {value}")
    require(value.startswith("models/" if suffix == ".mdl" else "materials/") and value.endswith(suffix), f"Expected an asset-relative {suffix} path: {value}")
    return value.lower()


def output_game(args):
    game = args.output / "game"
    game.mkdir()
    require('"' not in str(args.fixture_game), "Fixture path cannot contain a quote")
    (game / "gameinfo.txt").write_text(
        'GameInfo { FileSystem { SearchPaths { Game "|gameinfo_path|." '
        f'Game "{args.fixture_game}" }} }} }}\n'
    )
    wall = game / f"materials/{WALL_MATERIAL}.vmt"
    wall.parent.mkdir(parents=True)
    wall.write_text(f"LightmappedGeneric {{ $basetexture {WALL_MATERIAL} }}\n")
    wall.with_suffix(".vtf").write_bytes(texture(bytes([255] * 32 * 32 * 4)))
    return game


def start_client(args, directory, game):
    directory.mkdir(exist_ok=True)
    with patch.dict(os.environ, {
        "HAMMERR_RECENT_MAPS_DIR": str(args.output / "recent"),
        "HAMMERR_BUILD_RECIPES": str(args.output / "recipes.json"),
        "HAMMERR_PREFABS_DIR": str(args.output / "prefabs"),
        "XDG_CONFIG_HOME": str(args.output / "config"),
        "XDG_STATE_HOME": str(args.output / "state"),
    }):
        return Client(None, args.binary, directory, game / "gameinfo.txt")


def pak_and_flags(data):
    lumps = probe.bsp_lumps(data)
    require(lumps[59][1] == 0 and len(lumps[59][2]) in (0, 4), "Invalid level-flags lump")
    flags = struct.unpack("<I", lumps[59][2])[0] if lumps[59][2] else 0
    entries = probe.zip_entries(lumps[40][2])[0] if lumps[40][2] else {}
    return entries, flags


def read_vhv(data, checksum, expected):
    require(512 <= len(data) <= probe.MAX_ENTRY_BYTES and len(data) % 512 == 0, "Invalid VHV aligned file size")
    version, actual_checksum, flags, stride, vertices, meshes = struct.unpack_from("<6I", data)
    require((version, actual_checksum, flags, stride) == (2, checksum, 4, 4), "VHV identity/format differs from the source MDL")
    require(0 < meshes <= 256 and 40 + meshes * 28 <= len(data) and data[24:40] == bytes(16), "Invalid VHV mesh count or reserved header")
    layout, colors = [], []
    cursor = (40 + meshes * 28 + 511) // 512 * 512
    require(data[40 + meshes * 28:cursor] == bytes(cursor - 40 - meshes * 28), "Nonzero VHV table padding")
    for index in range(meshes):
        lod, count, offset = struct.unpack_from("<3I", data, 40 + index * 28)
        require(data[52 + index * 28:68 + index * 28] == bytes(16), "Nonzero VHV section reserved fields")
        require(count > 0 and offset == cursor and offset + count * 4 <= len(data), "VHV payload is missing, reordered or out of bounds")
        layout.append({"lod": lod, "vertex_count": count})
        colors.extend(struct.iter_unpack("<4B", data[offset:offset + count * 4]))
        cursor += count * 4
    require(layout == expected and vertices == sum(row["vertex_count"] for row in layout), "VHV hardware layout differs from its manifest")
    require((cursor + 511) // 512 * 512 == len(data) and data[cursor:] == bytes(len(data) - cursor), "Invalid VHV trailing alignment or padding")
    require(all(color[3] == 255 for color in colors), "VHV alpha is not fully opaque")
    return {"vertices": vertices, "meshes": meshes, "bytes": len(data), "sha256": probe.digest(data)}, colors


def verify_output(args, game, data, stats, channels):
    entries, flags = pak_and_flags(data)
    require(flags & 3 == (3 if channels == ["ldr", "hdr"] else 1), "Baked-prop level bits do not match complete channels")
    require(MANIFEST in entries, "Generated prop-lighting manifest is missing")
    manifest = json.loads(entries[MANIFEST])
    require(manifest["version"] == 1 and manifest["channels"] == channels, "Manifest version/channel differs")
    sprp = static_records(data)
    require(len(sprp["models"]) == 1 and sprp["models"][0].lower() == args.model, "Final static-prop model identity changed")
    require(len(sprp["props"]) == 3 and [row[:3] for row in sprp["props"]] == args.origins, "Hidden prop consumed an ordinal or final prop ordering changed")
    require([row[10] & 64 for row in sprp["props"]] == [64, 0, 0], "Explicit disabled prop did not retain ordinal zero")
    records = probe.game_lump_records(data)[1]
    record = next(row for row in records if row[0] == int.from_bytes(b"sprp", "big"))
    require(manifest["static_props_version"] == record[2] and manifest["static_props_sha256"] == probe.digest(record[4]), "Manifest does not identify the actual final sprp payload")
    model = probe.read_bounded(args.fixture_game / args.model, probe.MAX_BSP_BYTES)
    checksum = struct.unpack_from("<I", model, 8)[0]
    vtx = probe.read_bounded(args.fixture_game / (args.model[:-4] + ".dx90.vtx"), probe.MAX_BSP_BYTES)
    require(len(vtx) >= 36 and struct.unpack_from("<I", vtx, 16)[0] == checksum, "Fixture VTX checksum differs from its MDL")
    lod_count = struct.unpack_from("<i", vtx, 20)[0]
    require(1 <= lod_count <= 8, "Fixture draw-LOD count is outside the admitted scope")
    receivers = manifest["receivers"]
    require([row["ordinal"] for row in receivers] == [1, 2], "Manifest lost the retained disabled-prop ordinal hole")
    assets, colors, reports = {}, {}, {}
    for receiver in receivers:
        require(receiver["model"] == args.model and receiver["checksum"] == checksum, "Receiver model/checksum differs from the mounted source")
        require(receiver["meshes"] and receiver["meshes"][0]["lod"] == 0 and all(0 <= row["lod"] < 8 for row in receiver["meshes"]) and {row["lod"] for row in receiver["meshes"]} == set(range(lod_count)), "Receiver omits a file-declared draw LOD or has unsupported LOD indices")
        for channel in channels:
            name = f"sp_{'hdr_' if channel == 'hdr' else ''}{receiver['ordinal']}.vhv"
            asset = next(row for row in manifest["assets"] if row["path"] == name)
            require(asset["ordinal"] == receiver["ordinal"] and asset["channel"] == channel and name in entries, "Generated asset has the wrong prop/channel identity")
            report, color = read_vhv(entries[name], checksum, receiver["meshes"])
            require(asset["sha256"] == report["sha256"] and asset["bytes"] == report["bytes"], "Manifest VHV hash/size differs from packed bytes")
            assets[name], colors[(receiver["ordinal"], channel)], reports[name] = asset, color, report
    require(len(manifest["assets"]) == len(assets) and {name for name in entries if VHV_NAME.fullmatch(name)} == set(assets), "PAK contains missing, extra or unregistered VHV siblings")
    require(not any(name in entries for name in ("sp_0.vhv", "sp_hdr_0.vhv", "sp_3.vhv", "sp_hdr_3.vhv")), "Disabled or hidden prop received generated lighting")
    for source in manifest["sources"]:
        name = source["path"]
        require(not name.startswith("/") and ".." not in name.split("/") and "\\" not in name, "Manifest contains a machine-local source path")
        path = game / name if (game / name).is_file() else args.fixture_game / name
        raw = probe.read_bounded(path, probe.MAX_BSP_BYTES)
        require(len(raw) == source["bytes"] and probe.digest(raw) == source["sha256"], f"Manifest source differs from mounted fixture bytes: {name}")
    actual = stats["prop_vertex_lighting"]
    expected = {
        "props": 2, "models": 1,
        "meshes": sum(len(row["meshes"]) for row in receivers),
        "vertices": sum(mesh["vertex_count"] for row in receivers for mesh in row["meshes"]),
        "ldr_bytes": sum(row["bytes"] for row in assets.values() if row["channel"] == "ldr"),
        "hdr_bytes": sum(row["bytes"] for row in assets.values() if row["channel"] == "hdr"),
    }
    require(all(actual[key] == value for key, value in expected.items()), f"Published statistics differ from actual hardware files: {actual} != {expected}")
    require(actual["work"] > 0 and actual["asset_bytes"] > 0, "Actual bake work/assets are unreported")
    for ordinal in (1, 2):
        require(any(color[2] > color[0] for color in colors[(ordinal, "ldr")]), "Authored red LDR light did not reach a receiver")
        if "hdr" in channels:
            require(any(color[0] > color[2] for color in colors[(ordinal, "hdr")]), "Authored blue HDR light did not reach a receiver")
            require(colors[(ordinal, "ldr")] != colors[(ordinal, "hdr")], "HDR reused LDR vertex colors")
    return {"bsp_sha256": probe.digest(data), "map_flags": flags, "manifest": manifest, "files": reports, "stats": actual}


def unchanged(client, source, ids):
    return protected_state(client), detail(client, ids), source.read_bytes(), client.call("get_build_recipes")


def compile_pair(client, args, source, ids, name, enabled):
    before = unchanged(client, source, ids)
    path = args.output / f"{name}.bsp"
    result = client.call("compile_map", {"path": str(path)})
    require(result["success"] and result["bsp"] == str(path) and result["leak"] is None, result)
    foreground = client.call("get_compile_status")["job"]
    require(foreground["status"] == "completed" and foreground["result"]["published"], foreground)
    data = probe.read_bounded(path, probe.MAX_BSP_BYTES)
    background = args.output / f"{name}-background.bsp"
    started = client.call("start_compile", {"path": str(background)})["job"]
    require(started["settings"]["bake_static_prop_vertices"] is enabled, "Submitted job did not capture the requested flag")
    job = wait_job(client)
    require(job["token"] == started["token"] and job["status"] == "completed" and job["result"]["published"] and not job["stale"], job)
    require(background.read_bytes() == data and job["result"]["stats"].get("prop_vertex_lighting") == result["stats"].get("prop_vertex_lighting"), "Foreground/background publication differs")
    require(unchanged(client, source, ids) == before, "Compilation changed source, history, selection or the current recipe")
    return path, data, result


def rejected_compile(client, path, source, ids, prop, fragment):
    before, previous = unchanged(client, source, ids), path.read_bytes()
    failure = json.loads(client.call("compile_map", {"path": str(path)}, error=True)["content"][0]["text"])
    require(not failure["success"] and failure["bsp"] is None and path.read_bytes() == previous, "Rejected requested receiver replaced the previous BSP")
    require(any(row["level"] == "error" and row["object"] == prop and fragment.lower() in row["message"].lower() for row in failure["diagnostics"]), failure)
    require(unchanged(client, source, ids) == before, "Rejected compile changed source/history or the current recipe")
    client.call("start_compile", {"path": str(path)})
    job = wait_job(client)
    require(job["status"] == "failed" and not job["result"]["published"] and path.read_bytes() == previous, job)
    require(unchanged(client, source, ids) == before, "Rejected background compile changed source/history or the current recipe")
    return failure


def validate(client, args, game, report):
    initial = client.call("get_compile_settings")
    require(initial["bake_static_prop_vertices"] is False, "Prop vertex lighting is not default-off")
    require(initial["prop_vertex_lighting"]["limits"]["receiver"]["hardware_meshes_max"] == 256, "Shared settings do not expose the native per-receiver hardware-section bound")
    client.call("set_compile_settings", {"bake_static_prop_vertices": True}, error=True)
    require(client.call("get_compile_settings") == initial, "Fullbright refusal changed settings")
    ids = [client.call("create_block", {"mins": low, "maxs": high, "material": WALL_MATERIAL})["id"] for low, high in ROOM]
    ids.append(client.call("create_entity", {"classname": "info_player_terrorist", "origin": [0, -160, 8]})["id"])
    for point in [(-192, 0, 128), (192, 0, 128), (0, -192, 128), (0, 192, 128), (0, 0, 224), (0, 0, 16)]:
        ids.append(client.call("create_entity", {"classname": "light", "origin": point, "properties": {
            "_light": "255 0 0 24", "_lightHDR": "0 0 255 48", "_constant_attn": "1", "_linear_attn": "0", "_quadratic_attn": "0",
        }})["id"])
    hidden = client.call("create_entity", {"classname": "prop_static", "origin": [0, 96, 48], "properties": {"model": "models/hammerr_prop_vertex_missing.mdl", "solid": "0"}})["id"]
    client.call("set_object_visibility", {"ids": [hidden], "action": "hide"})
    props = [client.call("create_entity", {"classname": "prop_static", "origin": origin, "properties": {
        "model": args.model, "solid": "0", "disablevertexlighting": "1" if index == 0 else "0",
    }})["id"] for index, origin in enumerate(args.origins)]
    ids += [hidden, *props]
    source = args.output / "prop-vertex-lighting.vmf"
    client.call("save_map", {"path": str(source)})
    client.call("set_compile_settings", {"lighting": "direct_ldr", "physics": "none", "bounces": 0, "generate_reflections": False})
    _, omitted, result = compile_pair(client, args, source, ids, "default-off", False)
    client.call("set_compile_settings", {"bake_static_prop_vertices": False})
    _, disabled, disabled_result = compile_pair(client, args, source, ids, "explicit-off", False)
    require(omitted == disabled and "prop_vertex_lighting" not in result["stats"] and "prop_vertex_lighting" not in disabled_result["stats"], "Default/explicit disabled output or statistics differ")
    entries, flags = pak_and_flags(disabled)
    require(flags & 3 == 0 and MANIFEST not in entries and not any(VHV_NAME.fullmatch(name) for name in entries), "Feature-off output gained baked-prop flags/files")
    runtime = client.call("get_native_capture_runtime")
    client.call("set_compile_settings", {"bake_static_prop_vertices": True})
    settings = client.call("get_compile_settings")
    client.call("set_compile_settings", {"bounces": 1}, error=True)
    require(client.call("get_compile_settings") == settings, "Bounce refusal silently changed a setting")
    require(client.call("get_compile_status")["job"]["stale"], "Changed bake setting did not stale previous output")
    for mode, channels in [("direct_ldr", ["ldr"]), ("direct_ldr_hdr", ["ldr", "hdr"])]:
        client.call("set_compile_settings", {"lighting": mode})
        client.call("save_build_recipe", {"name": f"Prop vertex {mode}"})
        path, data, result = compile_pair(client, args, source, ids, mode, True)
        report["channels"][mode] = verify_output(args, game, data, result["stats"], channels)
    manifest = report["channels"]["direct_ldr_hdr"]["manifest"]
    material_paths = [row["path"] for row in manifest["sources"] if row["path"].endswith(".vmt")]
    material = args.material or (material_paths[0] if len(material_paths) == 1 else None)
    require(material in material_paths, "Pass --material with a selected receiver VMT from the manifest when its source closure has multiple VMTs")
    overlay = game / material
    overlay.parent.mkdir(parents=True, exist_ok=True)
    require(not overlay.exists(), "Private material override collides with a fixture asset")
    try:
        overlay.write_text("UnlitGeneric { }\n")
        report["rejected_material"] = rejected_compile(client, path, source, ids, props[1], "material")
    finally:
        overlay.unlink(missing_ok=True)
    client.call("set_entity_properties", {"id": props[1], "set": {"model": args.unsupported_model}})
    report["rejected_model"] = rejected_compile(client, path, source, ids, props[1], args.unsupported_model_diagnostic)
    client.call("set_entity_properties", {"id": props[1], "set": {"disablevertexlighting": "1"}})
    result = client.call("compile_map", {"path": str(args.output / "explicit-opt-out.bsp")})
    require(result["success"] and result["stats"]["prop_vertex_lighting"]["props"] == 1, "Explicit per-prop opt-out did not admit a supported remaining receiver")
    entries, flags = pak_and_flags((args.output / "explicit-opt-out.bsp").read_bytes())
    require(flags & 3 == 3 and {name for name in entries if VHV_NAME.fullmatch(name)} == {"sp_2.vhv", "sp_hdr_2.vhv"}, "Opt-out renumbered the surviving receiver")
    client.call("set_entity_properties", {"id": props[2], "set": {"disablevertexlighting": "1"}})
    result = client.call("compile_map", {"path": str(args.output / "all-opt-out.bsp")})
    entries, flags = pak_and_flags((args.output / "all-opt-out.bsp").read_bytes())
    require(result["success"] and flags & 3 == 0 and MANIFEST not in entries and not any(VHV_NAME.fullmatch(name) for name in entries), "No eligible receivers still produced baked-prop flags/files")
    client.call("set_entity_properties", {"id": props[1], "set": {"model": args.model, "disablevertexlighting": "0"}})
    client.call("set_entity_properties", {"id": props[2], "set": {"disablevertexlighting": "0"}})
    client.call("save_map", {"path": str(source)})
    saved = client.call("save_build_recipe", {"name": "Persisted prop vertex bake"})
    require(saved["settings"]["bake_static_prop_vertices"] is True, "Recipe lost enabled bake setting")
    after_runtime = client.call("get_native_capture_runtime")
    require(runtime["channel"] == "ldr" and after_runtime["channel"] == "ldr_hdr", "Runtime descriptor did not follow the selected lighting channel")
    require({key: value for key, value in runtime.items() if key != "channel"} ==
            {key: value for key, value in after_runtime.items() if key != "channel"}, "CPU compilation changed native runtime configuration")
    require(not client.call("get_map_info")["rendering"]["initialized"], "CPU compilation initialized graphics")
    report["native_runtime_before"], report["native_runtime_after"] = runtime, after_runtime
    report["recipe_id"], report["source"] = saved["active"]["id"], str(source)


def main():
    parser = argparse.ArgumentParser(description="Validate static-prop vertex-lighting publication through either production private-stdio MCP binary, without starting a game.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--fixture-game", type=Path, required=True, help="Prepared standalone game directory containing local model/material companions; read only")
    parser.add_argument("--model", required=True, help="Admitted simple opaque asset-relative models/...mdl")
    parser.add_argument("--unsupported-model", required=True, help="Existing structurally unsupported model, such as a fixture declaring root LOD one")
    parser.add_argument("--unsupported-model-diagnostic", default="root LOD")
    parser.add_argument("--material", help="Optional selected receiver materials/...vmt to shadow for rejection; auto-detected only when unambiguous")
    parser.add_argument("--origin-z", type=float, default=48.0, help="Receiver origin height; use the fixture's exact minimum-Z offset to check floor contact")
    args = parser.parse_args()
    args.binary, args.fixture_game, args.output = args.binary.resolve(strict=True), args.fixture_game.resolve(strict=True), args.output.resolve()
    require(args.output.is_relative_to(ROOT / "target" / "validation"), "Keep acceptance artifacts on disk below target/validation")
    args.output.mkdir(parents=True, mode=0o700, exist_ok=False)
    args.model, args.unsupported_model = asset_name(args.model, ".mdl"), asset_name(args.unsupported_model, ".mdl")
    if args.material:
        args.material = asset_name(args.material, ".vmt")
    require(args.model != args.unsupported_model, "Supported and unsupported fixture models must differ")
    require(math.isfinite(args.origin_z) and abs(args.origin_z) <= 16384, "Receiver height must fit finite Source coordinates")
    height = struct.unpack("<f", struct.pack("<f", args.origin_z))[0]
    args.origins = [(0, -96, height), (-64, 0, height), (64, 0, height)]
    require((args.fixture_game / "gameinfo.txt").is_file() and all((args.fixture_game / name).is_file() for name in [args.model, args.unsupported_model]), "Prepared fixture game/model files are missing")
    game = output_game(args)
    report = {"status": "running", "binary": str(args.binary), "binary_sha256": probe.digest(args.binary.read_bytes()), "transport": "private_stdio", "native_exercised": False, "desktop_input": False, "origins": args.origins, "channels": {}}
    client = None
    try:
        client = start_client(args, args.output / "first", game)
        with stopped_on_exit(client):
            validate(client, args, game, report)
        report["first_exit_code"] = require_clean_eof(client)
        client = start_client(args, args.output / "restart", game)
        with stopped_on_exit(client):
            require(client.call("get_compile_settings")["bake_static_prop_vertices"] is False, "A fresh process did not start with the default disabled setting")
            client.call("open_recent_map", {"path": report["source"]})
            settings = client.call("get_compile_settings")
            require(settings["bake_static_prop_vertices"] is True and settings["lighting"] == "direct_ldr_hdr", "Compatible recent context failed to restore the persisted bake")
            client.call("apply_build_recipe", {"id": "fast"})
            require(client.call("get_compile_settings")["bake_static_prop_vertices"] is False, "Built-in recipe did not disable the bake")
            client.call("apply_build_recipe", {"id": report["recipe_id"]})
            require(client.call("get_compile_settings")["bake_static_prop_vertices"] is True, "Restarted saved recipe failed to restore the bake")
            require(not client.call("get_map_info")["rendering"]["initialized"], "Persistence checks initialized graphics")
        report["restart_exit_code"] = require_clean_eof(client)
        report["status"] = "passed"
    except Exception as error:
        report["status"], report["error"] = "failed", f"{type(error).__name__}: {error}"
        raise
    finally:
        if client is not None:
            client.close()
        (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"status": report["status"], "results": str(args.output / "results.json"), "native_exercised": False}))


if __name__ == "__main__":
    main()
