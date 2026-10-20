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
SELF_SHADOW_MODEL = "models/props_junk/wood_crate001a.mdl"
SELF_SHADOW_MATERIAL = "materials/models/props_junk/woodcrates01a.vmt"
SELF_SHADOW_HEIGHT = 20.01078987121582
SELF_SHADOW_GEOMETRY = {
    SELF_SHADOW_MODEL: "8fc43694049559d0512805630ff0ac7d6bb77fe819c98deeec2e02c364648126",
    "models/props_junk/wood_crate001a.vvd": "3fa20fc03c7cd6f754018f69c486ff5390344ee8af6684f0a2395be85d720849",
    "models/props_junk/wood_crate001a.dx90.vtx": "42879ae821ab5c95a482c1deca5451dc53c8875009c46dafc7195676c6c4734d",
}


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
    bounces = client.call("get_compile_settings")["bounces"]
    path = args.output / f"{name}.bsp"
    result = client.call("compile_map", {"path": str(path)})
    require(result["success"] and result["bsp"] == str(path) and result["leak"] is None, result)
    foreground = client.call("get_compile_status")["job"]
    require(foreground["status"] == "completed" and foreground["result"]["published"], foreground)
    data = probe.read_bounded(path, probe.MAX_BSP_BYTES)
    background = args.output / f"{name}-background.bsp"
    started = client.call("start_compile", {"path": str(background)})["job"]
    require(started["settings"]["bake_static_prop_vertices"] is enabled, "Submitted job did not capture the requested flag")
    require(started["settings"]["bounces"] == bounces, "Submitted job did not capture the requested bounce depth")
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


def self_shadow_sources(args, game):
    require(args.model == SELF_SHADOW_MODEL and args.origins[1][2] == SELF_SHADOW_HEIGHT,
            "--self-shadow-control requires the proven grounded wood_crate001a fixture and --origin-z 20.01078987121582")
    require(args.material in (None, SELF_SHADOW_MATERIAL), "Self-shadow control requires the crate's selected material")
    expected = []
    for name in sorted([*SELF_SHADOW_GEOMETRY, SELF_SHADOW_MATERIAL, f"materials/{WALL_MATERIAL}.vmt"]):
        path = game / name if (game / name).is_file() else args.fixture_game / name
        raw = probe.read_bounded(path, probe.MAX_BSP_BYTES)
        digest = probe.digest(raw)
        if name in SELF_SHADOW_GEOMETRY:
            require(digest == SELF_SHADOW_GEOMETRY[name], f"Self-shadow geometry prerequisite changed: {name}")
        expected.append({"path": name, "sha256": digest, "bytes": len(raw)})
    return expected


def compare_self_shadow(before, after, channels, sources):
    left_lumps, right_lumps = probe.bsp_lumps(before), probe.bsp_lumps(after)
    require(all(left == right for index, (left, right) in enumerate(zip(left_lumps, right_lumps, strict=True)) if index not in (35, 40)),
            "Receiver self-shadow authoring changed world, terrain, ambient or other non-prop BSP lumps")
    left_props, right_props = static_records(before), static_records(after)
    expected = {**left_props, "props": list(left_props["props"])}
    target = list(expected["props"][1])
    require(target[10] & 0x80 == 0, "Absent self-shadow control already set native flag 0x80")
    target[10] |= 0x80
    expected["props"][1] = tuple(target)
    require(right_props == expected, "Self-shadow flag changed another prop, ordinal, placement or native field")
    left_records, right_records = probe.game_lump_records(before)[1], probe.game_lump_records(after)[1]
    require(len(left_records) == len(right_records), "Self-shadow control changed game-lump membership")
    for left, right in zip(left_records, right_records, strict=True):
        require(left[:4] == right[:4], "Self-shadow control changed game-lump identity/layout")
        if left[0] == int.from_bytes(b"sprp", "big"):
            expected_payload = bytearray(left[4])
            flag_offset = 4 + len(left_props["models"]) * 128 + 4 + len(left_props["leaves"]) * 2 + 4 + 64 + 31
            expected_payload[flag_offset] |= 0x80
            require(right[4] == bytes(expected_payload), "Self-shadow control changed sprp bytes beyond ordinal one's native flag")
        else:
            require(left == right, "Self-shadow control changed a non-static-prop game lump")
    left_entries, left_flags = pak_and_flags(before)
    right_entries, right_flags = pak_and_flags(after)
    require(left_flags == right_flags and left_entries.keys() == right_entries.keys(), "Self-shadow control changed map bits or PAK membership")
    left_manifest, right_manifest = json.loads(left_entries[MANIFEST]), json.loads(right_entries[MANIFEST])
    require(left_manifest["sources"] == right_manifest["sources"] == sources
            and left_manifest["missing_sources"] == right_manifest["missing_sources"] == [],
            "Self-shadow control lost or changed the independent expected model/material source closure")
    require({key: value for key, value in left_manifest.items() if key not in ("assets", "static_props_sha256")} ==
            {key: value for key, value in right_manifest.items() if key not in ("assets", "static_props_sha256")},
            "Self-shadow control changed receiver identities, channel order or hardware layout")
    changed_names = {f"sp_{'hdr_' if channel == 'hdr' else ''}1.vhv" for channel in channels}
    require(all(left_entries[name] == right_entries[name] for name in left_entries if name not in changed_names | {MANIFEST}),
            "Self-shadow control changed another prop's VHV or an unrelated packed asset")
    for left, right in zip(left_manifest["assets"], right_manifest["assets"], strict=True):
        require({key: value for key, value in left.items() if key != "sha256"} ==
                {key: value for key, value in right.items() if key != "sha256"}, "Self-shadow control changed generated asset ordinals/layout/byte counts")
        if left["path"] not in changed_names:
            require(left == right, "Self-shadow control changed another prop's asset identity")
    receiver = left_manifest["receivers"][0]
    require(receiver["ordinal"] == 1 and receiver["meshes"] == [{"lod": 0, "vertex_count": 400}, {"lod": 1, "vertex_count": 84}],
            "Self-shadow fixture lost its independently checked hardware stream")
    measurements = {}
    for channel in channels:
        name = f"sp_{'hdr_' if channel == 'hdr' else ''}1.vhv"
        _, old = read_vhv(left_entries[name], receiver["checksum"], receiver["meshes"])
        _, new = read_vhv(right_entries[name], receiver["checksum"], receiver["meshes"])
        require(all(all(after_component >= before_component for before_component, after_component in zip(a[:3], b[:3], strict=True))
                    for a, b in zip(old, new, strict=True)), "Removing own occlusion reduced a static direct vertex color")
        changed = sum(a != b for a, b in zip(old, new, strict=True))
        opened = sum(not any(a[:3]) and any(b[:3]) for a, b in zip(old, new, strict=True))
        color = 2 if channel == "ldr" else 0
        require(changed > 0 and opened > 0 and old[0][:3] == (0, 0, 0) and new[0][color] > 0,
                "Known crate hardware vertex zero did not open its substantial own-frame occlusion")
        measurements[channel] = {"changed_vertices": changed, "dark_vertices_opened": opened,
                                 "hardware_vertex_zero_before_bgra": old[0], "hardware_vertex_zero_after_bgra": new[0],
                                 "baseline_sha256": probe.digest(left_entries[name]), "disabled_sha256": probe.digest(right_entries[name])}
    return measurements


def validate_self_shadow(client, args, game, source, ids, props, report):
    sources = self_shadow_sources(args, game)
    policy = client.call("get_compile_settings")["prop_vertex_lighting"]["limits"]["receiver"]["self_shadowing"]
    require(policy == {"default": True, "disable_property": "disableselfshadowing", "disable_value": 1,
                       "disabled_scope": "receiver_owned_triangles", "retained_blockers": ["other_props", "world", "terrain"]},
            "Shared compile metadata does not expose the supported owner-only self-shadow policy")
    original_recipe = client.call("get_compile_settings")["recipe"]
    original_props, original_source = detail(client, props), source.read_bytes()
    baseline = {args.output / f"{mode}{suffix}.bsp": (args.output / f"{mode}{suffix}.bsp").read_bytes()
                for mode in ("direct_ldr", "direct_ldr_hdr") for suffix in ("", "-background")}
    saved_recipes = {row["name"]: row["id"] for row in client.call("get_build_recipes")["recipes"]}
    cases = {}
    try:
        for mode, channels in [("direct_ldr", ["ldr"]), ("direct_ldr_hdr", ["ldr", "hdr"])]:
            client.call("apply_build_recipe", {"id": saved_recipes[f"Prop vertex {mode}"]})
            absent = baseline[args.output / f"{mode}.bsp"]
            require(report["channels"][mode]["manifest"]["sources"] == sources, "Baseline does not retain the independent expected source closure")
            client.call("set_entity_properties", {"id": props[1], "set": {"disableselfshadowing": "0"}})
            _, zero, _ = compile_pair(client, args, source, ids, f"self-shadow-{mode}-zero", True)
            require(zero == absent, "Explicit self-shadow zero differs from absent authoring in complete BSP bytes")
            client.call("set_entity_properties", {"id": props[1], "set": {"disableselfshadowing": "1"}})
            path, disabled, result = compile_pair(client, args, source, ids, f"self-shadow-{mode}-disabled", True)
            verified = verify_output(args, game, disabled, result["stats"], channels)
            measurements = compare_self_shadow(absent, disabled, channels, sources)
            client.call("set_entity_properties", {"id": props[1], "remove": ["disableselfshadowing"]})
            _, restored, _ = compile_pair(client, args, source, ids, f"self-shadow-{mode}-restored", True)
            require(restored == absent and detail(client, props) == original_props and source.read_bytes() == original_source,
                    "Removing authored self-shadow control failed to restore default BSP/property/source bytes")
            cases[mode] = {"artifact": str(path), "bsp_sha256": verified["bsp_sha256"], "channels": measurements,
                           "explicit_zero_matches_absence": True, "default_restored_exactly": True,
                           "other_prop_and_world_bytes_unchanged": True, "source_closure": sources}
    finally:
        client.call("set_entity_properties", {"id": props[1], "remove": ["disableselfshadowing"]})
        client.call("apply_build_recipe", {"id": original_recipe["id"]})
    require(client.call("get_compile_settings")["recipe"] == original_recipe, "Self-shadow controls failed to restore the original active recipe")
    require(all(path.read_bytes() == raw for path, raw in baseline.items()), "Self-shadow controls overwrote baseline native-evidence artifacts")
    report["self_shadowing"] = {"policy": policy, "cases": cases, "baseline_artifacts_preserved": True,
                               "compiler_preserves_source_history_recipe_each_compile": True, "native_exercised": False}


def validate_one_bounce(client, args, game, source, ids, report):
    original_recipe = client.call("get_compile_settings")["recipe"]
    original_state = protected_state(client), detail(client, ids), source.read_bytes()
    baseline = {args.output / f"{mode}{suffix}.bsp": (args.output / f"{mode}{suffix}.bsp").read_bytes()
                for mode in ("direct_ldr", "direct_ldr_hdr") for suffix in ("", "-background")}
    saved_recipes = {row["name"]: row["id"] for row in client.call("get_build_recipes")["recipes"]}
    texture_name = f"materials/{WALL_MATERIAL}.vtf"
    texture_path = game / texture_name
    texture_bytes = probe.read_bounded(texture_path, probe.MAX_BSP_BYTES)
    material_path = texture_path.with_suffix(".vmt")
    material_bytes = material_path.read_bytes()
    expected_texture = {"path": texture_name, "sha256": probe.digest(texture_bytes), "bytes": len(texture_bytes)}
    cases = {}
    try:
        for mode, channels in [("direct_ldr", ["ldr"]), ("direct_ldr_hdr", ["ldr", "hdr"])]:
            client.call("apply_build_recipe", {"id": saved_recipes[f"Prop vertex {mode}"]})
            updated = client.call("set_compile_settings", {"bounces": 1})
            require(updated["bake_static_prop_vertices"] and updated["bounces"] == 1 and updated["prop_vertex_lighting"]["available"],
                    "One bounce is not available through the shared enabled-bake settings")
            recipe = client.call("save_build_recipe", {"name": f"One-bounce prop {mode}"})
            path, baked, result = compile_pair(client, args, source, ids, f"one-bounce-{mode}", True)
            verified = verify_output(args, game, baked, result["stats"], channels)
            require(result["stats"]["bounces"] == 1, "Published one-bounce job did not execute the requested world pass")
            require(next((row for row in verified["manifest"]["sources"] if row["path"] == texture_name), None) == expected_texture,
                    "One-bounce publication omitted or changed the original whole source VTF dependency")
            current_settings, current_state = client.call("get_compile_settings"), unchanged(client, source, ids)
            token = client.call("get_compile_status")["job"]["token"]
            client.call("set_compile_settings", {"bounces": 2, "physics": "engine"}, error=True)
            require(client.call("get_compile_settings") == current_settings and unchanged(client, source, ids) == current_state
                    and client.call("get_compile_status")["job"]["token"] == token and path.read_bytes() == baked,
                    "Incompatible depth update changed settings, the current job, source/history/recipe or its published output")
            cleared = client.call("set_compile_settings", {"bake_static_prop_vertices": False, "bounces": 2})
            require(not cleared["bake_static_prop_vertices"] and cleared["bounces"] == 2,
                    "Clearing prop baking did not restore the ordinary world bounce range")
            _, plain, plain_result = compile_pair(client, args, source, ids, f"one-bounce-{mode}-cleared", False)
            entries, flags = pak_and_flags(plain)
            require(flags & 3 == 0 and MANIFEST not in entries and not any(VHV_NAME.fullmatch(name) for name in entries)
                    and "prop_vertex_lighting" not in plain_result["stats"], "Cleared prop baking retained generated prop output")
            client.call("apply_build_recipe", {"id": recipe["active"]["id"]})
            restored_settings = client.call("get_compile_settings")
            require(restored_settings["bake_static_prop_vertices"] and restored_settings["bounces"] == 1,
                    "Saved one-bounce recipe failed to restore shared settings")
            _, restored, _ = compile_pair(client, args, source, ids, f"one-bounce-{mode}-restored", True)
            require(restored == baked, "Restored one-bounce settings did not reproduce exact published BSP bytes")
            cases[mode] = {"artifact": str(path), "bsp_sha256": verified["bsp_sha256"], "stats": verified["stats"],
                           "recipe_id": recipe["active"]["id"], "source_vtf": expected_texture,
                           "invalid_depth_preserved_settings_and_output": True, "clear_and_recipe_restore_exact": True}
    finally:
        client.call("apply_build_recipe", {"id": original_recipe["id"]})
    require((protected_state(client), detail(client, ids), source.read_bytes()) == original_state,
            "One-bounce controls changed the source, history, selection or document revision")
    require(client.call("get_compile_settings")["recipe"] == original_recipe
            and all(path.read_bytes() == raw for path, raw in baseline.items())
            and texture_path.read_bytes() == texture_bytes and material_path.read_bytes() == material_bytes,
            "One-bounce controls changed the original recipe, N0 artifacts or private source assets")
    report["one_bounce"] = {"cases": cases, "zero_bounce_artifacts_and_assets_preserved": True, "native_exercised": False}


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
    if args.self_shadow_control:
        target = client.call("set_entity_properties", {"id": props[1], "remove": ["disableselfshadowing"]})
        require("disableselfshadowing" not in dict(target["properties"]), "Self-shadow absence control retains an authored FGD default")
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
    client.call("set_compile_settings", {"bounces": 2}, error=True)
    require(client.call("get_compile_settings") == settings, "Bounce refusal silently changed a setting")
    require(client.call("get_compile_status")["job"]["stale"], "Changed bake setting did not stale previous output")
    for mode, channels in [("direct_ldr", ["ldr"]), ("direct_ldr_hdr", ["ldr", "hdr"])]:
        client.call("set_compile_settings", {"lighting": mode})
        client.call("save_build_recipe", {"name": f"Prop vertex {mode}"})
        path, data, result = compile_pair(client, args, source, ids, mode, True)
        report["channels"][mode] = verify_output(args, game, data, result["stats"], channels)
    if args.self_shadow_control:
        validate_self_shadow(client, args, game, source, ids, props, report)
    if args.one_bounce_control:
        validate_one_bounce(client, args, game, source, ids, report)
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
    if args.one_bounce_control:
        client.call("set_compile_settings", {"bounces": 1})
    saved = client.call("save_build_recipe", {"name": "Persisted prop vertex bake"})
    require(saved["settings"]["bake_static_prop_vertices"] is True, "Recipe lost enabled bake setting")
    after_runtime = client.call("get_native_capture_runtime")
    require(runtime["channel"] == "ldr" and after_runtime["channel"] == "ldr_hdr", "Runtime descriptor did not follow the selected lighting channel")
    require({key: value for key, value in runtime.items() if key != "channel"} ==
            {key: value for key, value in after_runtime.items() if key != "channel"}, "CPU compilation changed native runtime configuration")
    require(not client.call("get_map_info")["rendering"]["initialized"], "CPU compilation initialized graphics")
    report["native_runtime_before"], report["native_runtime_after"] = runtime, after_runtime
    report["recipe_id"], report["source"] = saved["active"]["id"], str(source)
    report["persisted_bounces"] = saved["settings"]["bounces"]


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
    parser.add_argument("--self-shadow-control", action="store_true", help="Check positive own-occlusion using the exact grounded wood_crate001a companions; requires --origin-z 20.01078987121582")
    parser.add_argument("--one-bounce-control", action="store_true", help="Build, reject incompatible depths, clear and restore one-bounce prop lighting in both channels using the private measured wall VTF")
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
            require(settings["bounces"] == report["persisted_bounces"], "Recent context failed to restore the persisted bounce depth")
            client.call("apply_build_recipe", {"id": "fast"})
            require(client.call("get_compile_settings")["bake_static_prop_vertices"] is False, "Built-in recipe did not disable the bake")
            client.call("apply_build_recipe", {"id": report["recipe_id"]})
            require(client.call("get_compile_settings")["bake_static_prop_vertices"] is True, "Restarted saved recipe failed to restore the bake")
            require(client.call("get_compile_settings")["bounces"] == report["persisted_bounces"], "Restarted saved recipe failed to restore the bounce depth")
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
