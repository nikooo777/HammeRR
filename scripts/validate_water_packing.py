import argparse
import json
import struct
from pathlib import Path

import validate_cubemap_capture_probe as probe
from validate_authored_planes import records
from validate_build_recipes import protected_state
from validate_compile_jobs import stopped_on_exit, wait_job
from validate_face_lighting import DEFAULT_BINARY, ROOT, Client, detail, face_ref
from validate_material_albedo import digest, identities, require
from validate_recovery_exit import require_clean_eof
from validate_visibility import ROOM, bit, visibility
from validate_water_packing_native import COLORS, cube, originals

MANIFEST = "hammerr/packed-water.json"
NAMESPACE = "hammerr710"
SURFACE = f"{NAMESPACE}/surface"
DRY = f"{NAMESPACE}/dry"
NODRAW = f"{NAMESPACE}/nodraw"
MODES = ("fullbright", "direct_ldr", "direct_ldr_hdr")
MAX_FIXTURE_BSP_BYTES = 16 * 1024 * 1024
POINTFILE = b"11 13 17\n19 23 29\n"


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def fixture_assets(game):
    expected = originals(NAMESPACE, "first")
    surface = f"materials/{SURFACE}.vmt"
    expected[f"materials/{NAMESPACE}/base.vmt"] = expected[surface]
    expected[surface] = f'Patch {{ include "materials/{NAMESPACE}/base.vmt" }}\n'.encode()
    files = {
        **expected,
        f"materials/{DRY}.vmt": f'LightmappedGeneric {{ $basetexture "{NAMESPACE}/white" }}\n'.encode(),
        f"materials/{NODRAW}.vmt": b"UnlitGeneric { %compilenodraw 1 }\n",
    }
    for name, payload in files.items():
        target = game / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(payload)
    (game / "gameinfo.txt").write_text('GameInfo { FileSystem { SearchPaths { Game "|gameinfo_path|." } } }\n')
    return expected


def start_client(binary, output, game, stores):
    output.mkdir()
    return Client(None, binary, output, game / "gameinfo.txt", environment={
        "HAMMERR_RECENT_MAPS_DIR": str(stores / "recent"),
        "HAMMERR_BUILD_RECIPES": str(stores / "recipes.json"),
        "HAMMERR_PREFABS_DIR": str(stores / "prefabs"),
        "HAMMERR_RECOVERY_DIR": str(output / "recovery"),
        "XDG_CONFIG_HOME": str(stores / "config"),
        "XDG_STATE_HOME": str(stores / "state"),
    })


def configure(client, mode, enabled=None):
    values = {"lighting": mode, "physics": "none", "bounces": 0, "visibility": "fast", "sky_quality": "fast",
              "generate_reflections": False, "pack_sky_assets": False, "bake_static_prop_vertices": False}
    if enabled is not None:
        values["pack_water_assets"] = enabled
    return client.call("set_compile_settings", values)


def fixture(client, source):
    ids = []
    for low, high in ROOM:
        block = client.call("create_block", {"mins": low, "maxs": high, "material": DRY})["id"]
        brush = detail(client, [block])[0]
        client.call("edit_faces", {"faces": [face_ref(brush, face) for face in brush["faces"]], "lightmap_scale": 64})
        ids.append(block)
    for classname, origin, properties in (
        ("info_player_terrorist", [-192, -192, 8], {}),
        ("info_player_counterterrorist", [192, -192, 8], {}),
        ("light", [0, 0, 224], {"_light": "255 192 128 64", "_lightHDR": "128 192 255 96",
                               "_constant_attn": "1", "_linear_attn": "0", "_quadratic_attn": "0"}),
    ):
        ids.append(client.call("create_entity", {"classname": classname, "origin": origin, "properties": properties})["id"])
    pool = client.call("create_block", {"mins": [-128, -128, 0], "maxs": [128, 128, 64], "material": NODRAW})["id"]
    ids.append(pool)
    brush = detail(client, [pool])[0]
    top = next(face for face in brush["faces"] if face["normal"] == [0, 0, 1])
    top = face_ref(brush, top)
    client.call("edit_faces", {"faces": [top], "material": SURFACE, "lightmap_scale": 64})
    client.call("set_selection_mode", {"mode": "objects"})
    client.call("select", {"ids": ids[:1]})
    client.call("save_map", {"path": str(source)})
    return ids, top


def snapshot(client, source, ids, game):
    info = client.call("get_map_info")
    require(not info["rendering"]["initialized"] and not info["launching"] and not info["game_running"],
            "Headless Water validation initialized graphics or launched a game")
    recipes = client.call("get_build_recipes")
    recipe_path = Path(recipes["path"])
    draft = client.rpc("tools/call", {"name": "get_property_edit", "arguments": {}})
    require(not draft.get("isError") or [row.get("text") for row in draft["content"]] == ["No property draft"],
            f"Property-draft inspection failed unexpectedly: {draft}")
    clipboard = client.call("get_clipboard")
    del clipboard["snapshot"]
    return {"map": protected_state(client), "history": client.call("get_history"), "objects": detail(client, ids),
            "property_draft": draft, "camera": client.call("get_camera"),
            "clipboard": clipboard, "settings": client.call("get_compile_settings"),
            "recipes": recipes, "recipe_sha256": digest(recipe_path.read_bytes()),
            "source_sha256": digest(source.read_bytes()), "assets": identities(game)}


def read_bsp(path):
    require(1036 <= path.stat().st_size <= MAX_FIXTURE_BSP_BYTES, "Fixture BSP exceeds its bounded read")
    data = path.read_bytes()
    probe.bsp_lumps(data)
    return data


def unchanged_snapshot(before, after, path, message):
    if before != after:
        write_json(path, {"before": before, "after": after})
    require(before == after, message)


def pak(data):
    payload = probe.bsp_lumps(data)[40][2]
    return probe.zip_entries(payload)[0] if payload else {}


def verify_output(data, stats, expected, roots, enabled):
    entries = pak(data)
    require(stats["water_regions"] == (1 if roots else 0), "Fixture did not compile the expected wet/dry geometry")
    if not enabled:
        require(MANIFEST not in entries and not set(expected).intersection(entries), "Disabled build packed Water content")
        require(stats.get("water_assets") is None, "Disabled build reports an enabled Water packing plan")
        return {"enabled": False, "bsp_sha256": digest(data)}
    expected_manifest = {"version": 1, "roots": sorted(roots), "channels": "ldr_hdr",
                         "assets": [{"path": name, "sha256": digest(payload)} for name, payload in sorted(expected.items())]}
    require(MANIFEST in entries, "Requested packing omitted the Water manifest")
    manifest = json.loads(entries[MANIFEST])
    require(manifest == expected_manifest, f"Packed Water manifest differs from the original static closure: {manifest}")
    require(set(entries) == {MANIFEST, *expected}, "Water PAK contains missing or unregistered original files")
    require(all(entries[name] == payload for name, payload in expected.items()), "Water packing rewrote original VMT/Patch/VTF bytes")
    expected_stats = {"roots": len(roots), "channels": "ldr_hdr", "files": len(expected),
                      "asset_bytes": sum(len(payload) for payload in expected.values())}
    require(stats["water_assets"] == expected_stats, f"Water statistics differ from the actual packed assets: {stats['water_assets']}")
    return {"enabled": True, "bsp_sha256": digest(data), "manifest": manifest, "stats": expected_stats,
            "original_payloads_exact": True}


def parse_failure(response):
    content = [row["text"] for row in response["content"] if row["type"] == "text"]
    require(len(content) == 1, "Failed compile did not return one structured result")
    return json.loads(content[0])


def require_failure(result, fragments):
    require(not result["success"] and result.get("bsp") is None and not result.get("published", False),
            f"Unsupported requested Water closure was published: {result}")
    messages = [row["message"] for row in result.get("diagnostics", []) if row["level"] == "error"]
    if result.get("error"):
        messages.append(result["error"])
    require(any(all(fragment.lower() in message.lower() for fragment in fragments) for message in messages),
            f"Water refusal lacks its source/reason: expected {fragments}, observed {messages}")


def compile_pair(client, output, label, source, ids, game, enabled, expected, roots, prior=None, errors=()):
    before = snapshot(client, source, ids, game)
    paths = [output / f"{label}{suffix}.bsp" for suffix in ("", "-background")]
    for path in paths:
        path.with_suffix(".lin").write_bytes(POINTFILE)
        if prior is not None:
            path.write_bytes(prior)
    with stopped_on_exit(client):
        response = client.call("compile_map", {"path": str(paths[0])}, error=bool(errors))
        result = parse_failure(response) if errors else response
        foreground = client.call("get_compile_status")["job"]
        require(foreground["settings"]["pack_water_assets"] is enabled and foreground["native_runtime"] is None,
                "Foreground job lost its Water option or requested native capture")
        if errors:
            require_failure(result, errors)
            require(foreground["status"] == "failed" and foreground["result"]["published"] is False
                    and read_bsp(paths[0]) == prior, "Foreground failure replaced the prior BSP")
        else:
            require(result["success"] and foreground["result"]["published"] and result["leak"] is None
                    and foreground["status"] == "completed" and not foreground["stale"], f"Foreground compile failed: {result}")
        unchanged_snapshot(before, snapshot(client, source, ids, game), output / f"{label}-foreground-state.json",
                           "Foreground compile changed document, draft, settings or source bytes")
        started = client.call("start_compile", {"path": str(paths[1])})["job"]
        require(started["settings"]["pack_water_assets"] is enabled and started["native_runtime"] is None,
                "Background request did not freeze the Water option independently of native capture")
        job = wait_job(client)
        require(job["token"] == started["token"] and job["settings"] == started["settings"]
                and job["recipe"] == started["recipe"] and job["generation"] == started["generation"]
                and job["revision"] == started["revision"], "Background completion changed its captured settings or snapshot")
        if errors:
            require(job["status"] == "failed", f"Background unsupported closure succeeded: {job}")
            require_failure(job["result"], errors)
        else:
            require(job["status"] == "completed" and job["result"]["success"]
                    and job["result"]["published"] and not job["stale"], f"Background compile failed: {job}")
        data = read_bsp(paths[0])
        require(read_bsp(paths[1]) == data and all(path.with_suffix(".lin").read_bytes() == POINTFILE for path in paths),
                "Foreground/background compilation changed prior BSP agreement or pointfile bytes")
        unchanged_snapshot(before, snapshot(client, source, ids, game), output / f"{label}-background-state.json",
                           "Background compile changed document, draft, settings or source bytes")
    packed = None
    if not errors:
        packed = verify_output(data, result["stats"], expected, roots, enabled)
        verify_output(data, job["result"]["stats"], expected, roots, enabled)
    return data, {"bsp": str(paths[0]), "background_bsp": str(paths[1]), "bsp_sha256": digest(data),
                  "result": result, "background": job, "packed": packed, "document_and_sources_preserved": True,
                  "prior_pointfile_preserved": True, "state_sha256": digest(json.dumps(before, sort_keys=True).encode())}


def frozen_job(client):
    before = client.call("get_compile_status")
    require(before["job"]["settings"]["pack_water_assets"] is True and before["job"]["result"]["published"]
            and not before["job"]["stale"],
            "Frozen-job control requires a current packed successful build")
    client.call("set_compile_settings", {"pack_water_assets": False})
    changed = client.call("get_compile_status")
    expected = {**before["job"], "stale": True}
    require(changed["job"] == expected and not changed["launch"]["ready"]
            and "Build settings changed" in changed["launch"]["reason"], "Setting change rewrote captured job or retained readiness")
    client.call("set_compile_settings", {"pack_water_assets": True})
    restored = client.call("get_compile_status")
    require(restored["job"] == before["job"] and restored["launch"] == before["launch"],
            "Restored Water option did not restore the same frozen job and launch state")
    return {"before": before["job"], "disabled": changed["job"], "restored": restored["job"],
            "original_launch": before["launch"], "changed_launch": changed["launch"], "restored_launch": restored["launch"]}


def water_fog_compatibility(previous, current):
    if previous == current:
        return {"bytes_identical": True}
    require([index for index, (a, b) in enumerate(zip(probe.bsp_lumps(previous), probe.bsp_lumps(current), strict=True)) if a != b] == [10],
            "Default-off compatibility changed a non-leaf lump")
    leaves = records(previous, 10, "<ihh6h4Hhh", expected_version=1)
    _, _, rows = visibility(previous)
    sources = {leaf[1] for leaf in leaves if leaf[13] >= 0 and leaf[0] & 0x10 == 0 and leaf[1] >= 0}
    offset = struct.unpack_from("<i", previous, 8 + 10 * 16)[0]
    expected = bytearray(previous)
    changed = []
    for index, leaf in enumerate(leaves):
        require(leaf[0] & 0x100 == 0, "Old compatibility fixture already contains mixed fog flags")
        if leaf[13] < 0 and leaf[0] & 0x31 == 0 and leaf[1] >= 0 and any(bit(rows[source][0], leaf[1]) for source in sources):
            struct.pack_into("<i", expected, offset + 32 * index, leaf[0] | 0x100)
            changed.append(index)
    require(changed and bytes(expected) == current, "Default-off BSP differs beyond required visible-Water fog flags")
    return {"bytes_identical": False, "only_testfogvolume_added": True, "changed_leaves": changed}


def compatibility(binary, output, game, source, controls):
    client = start_client(binary, output, game, output / "stores")
    result = {"binary": str(binary), "binary_sha256": digest(binary.read_bytes()), "cases": {}}
    try:
        client.call("open_map", {"path": str(source)})
        for mode, current in controls.items():
            configure(client, mode)
            target = output / f"{mode}.bsp"
            compiled = client.call("compile_map", {"path": str(target)})
            require(compiled["success"], f"Prefeature compatibility compile failed: {compiled}")
            previous = read_bsp(target)
            result["cases"][mode] = {"bsp": str(target), "bsp_sha256": digest(previous), "current_bsp_sha256": digest(current),
                                     **water_fog_compatibility(previous, current)}
        require(not client.call("get_map_info")["rendering"]["initialized"], "Compatibility control initialized graphics")
        result["clean_eof"] = require_clean_eof(client)
    finally:
        if client.process is not None:
            require_clean_eof(client)
        write_json(output / "results.json", result)
    return result


def persistence(binary, output, game, stores, source, recipe):
    client = start_client(binary, output, game, stores)
    try:
        require(client.call("get_compile_settings")["pack_water_assets"] is False, "Saving a recipe changed startup defaults")
        listing = client.call("get_build_recipes")
        saved = next(row for row in listing["recipes"] if row["id"] == recipe)
        require(saved["settings"]["pack_water_assets"] is True, "Restart lost the saved Water setting")
        client.call("apply_build_recipe", {"id": recipe})
        require(client.call("get_compile_settings")["pack_water_assets"] is True, "Saved recipe did not apply Water packing")
        client.call("set_compile_settings", {"pack_water_assets": False})
        client.call("open_recent_map", {"path": str(source)})
        require(client.call("get_compile_settings")["pack_water_assets"] is True, "Recent-map restart did not restore Water packing")
        require(not client.call("get_map_info")["rendering"]["initialized"], "Settings persistence initialized graphics")
        return {"recipe_id": recipe, "recipe_and_recent_restored": True, "clean_eof": require_clean_eof(client)}
    finally:
        if client.process is not None:
            require_clean_eof(client)


def validate(args):
    game, stores = args.output / "game", args.output / "stores"
    source = args.output / "source.vmf"
    expected = fixture_assets(game)
    roots = [f"materials/{NAMESPACE}/bottom.vmt", f"materials/{SURFACE}.vmt"]
    client = None
    report = {"status": "failed", "binary": str(args.binary), "binary_sha256": digest(args.binary.read_bytes()),
              "script_sha256": digest(Path(__file__).read_bytes()), "fixture_helper_sha256": digest((ROOT / "scripts/validate_water_packing_native.py").read_bytes()),
              "native_exercised": False, "desktop_input": False, "graphics_initialized": False,
              "positive": {}, "negative": {}}
    try:
        client = start_client(args.binary, args.output / "client", game, stores)
        schemas = {tool["name"]: tool["inputSchema"] for tool in client.rpc("tools/list", {})["tools"]}
        field = schemas["set_compile_settings"]["properties"]["pack_water_assets"]
        types = field["type"] if isinstance(field["type"], list) else [field["type"]]
        require("boolean" in types and set(types) <= {"boolean", "null"}
                and "pack_water_assets" not in schemas["set_compile_settings"].get("required", []),
                "Water setting is not a discoverable optional boolean")
        require(client.call("get_compile_settings")["pack_water_assets"] is False, "Water packing must default off")
        configure(client, "fullbright")
        ids, top = fixture(client, source)
        original_source_hash, original_assets = digest(source.read_bytes()), identities(game)
        client.call("save_build_recipe", {"name": "Water packing disabled"})
        recipes_path = stores / "recipes.json"
        require("pack_water_assets" not in json.loads(recipes_path.read_text())["recipes"][0]["settings"],
                "Default-false recipe changed the legacy persisted encoding")
        recent_path = stores / "recent/recent-maps.json"
        require("pack_water_assets" not in json.loads(recent_path.read_text())["entries"][0]["build_settings"],
                "Default-false recent-map settings changed the legacy persisted encoding")
        default, receipt = compile_pair(client, args.output, "default-fullbright", source, ids, game, False, expected, roots)
        report["positive"]["default-fullbright"] = receipt
        token = client.call("begin_property_edit", {"world": True})["draft"]["token"]
        client.call("update_property_edit", {"token": token, "changes": [{"action": "set", "key": "message", "value": "Unapplied Water packing review"}]})
        configure(client, "fullbright", False)
        disabled, receipt = compile_pair(client, args.output, "explicit-false-fullbright", source, ids, game, False, expected, roots)
        require(disabled == default, "Explicit false or an unapplied property draft changed the default whole BSP")
        report["positive"]["explicit-false-fullbright"] = receipt
        controls = {"fullbright": default}
        for mode in MODES[1:]:
            configure(client, mode, False)
            controls[mode], report["positive"][f"disabled-{mode}"] = compile_pair(
                client, args.output, f"disabled-{mode}", source, ids, game, False, expected, roots)
        if args.compat_binary:
            report["compatibility"] = compatibility(args.compat_binary, args.output / "compatibility", game, source, controls)
        else:
            report["compatibility"] = {"prefeature_binary_supplied": False, "default_explicit_false_bytes_identical": True}
        retained = None
        recipe = None
        for mode in MODES:
            configure(client, mode, True)
            saved = client.call("save_build_recipe", {"name": f"Packed Water {mode}"})
            recipe = saved["active"]["id"]
            require(saved["settings"]["pack_water_assets"] is True, "Recipe omitted the enabled Water setting")
            before = snapshot(client, source, ids, game)
            client.call("set_compile_settings", {"lighting": "unknown", "pack_water_assets": False}, error=True)
            require(snapshot(client, source, ids, game) == before, "Rejected combined settings update changed the draft or enabled Water option")
            data, receipt = compile_pair(client, args.output, f"packed-{mode}", source, ids, game, True, expected, roots)
            report["positive"][f"packed-{mode}"] = receipt
            if mode == "fullbright":
                retained = data
                report["frozen_job"] = frozen_job(client)
            require(all(left == right for index, (left, right) in enumerate(zip(probe.bsp_lumps(data), probe.bsp_lumps(controls[mode]), strict=True)) if index != 40),
                    "Water packing changed non-PAK BSP geometry, lighting or entities")
            if mode == "direct_ldr_hdr":
                lumps = probe.bsp_lumps(data)
                require(lumps[53][2] and lumps[58][2], "Dual-lighting Water fixture omitted native HDR lightmaps or faces")
        client.call("cancel_property_edit", {"token": token})
        configure(client, "fullbright", True)
        token = client.call("begin_property_edit", {"world": True})["draft"]["token"]
        client.call("update_property_edit", {"token": token, "changes": [{"action": "set", "key": "message", "value": "Keep this draft through Water refusals"}]})
        base = game / f"materials/{NAMESPACE}/base.vmt"
        bottom = game / f"materials/{NAMESPACE}/bottom.vmt"
        hdr = game / f"materials/{NAMESPACE}/cube.hdr.vtf"
        ldr = game / f"materials/{NAMESPACE}/cube.vtf"
        cases = (
            ("runtime-root", base, expected[base.relative_to(game).as_posix()].replace(f'"{NAMESPACE}/cube"'.encode(), b'"env_cubemap"'), ("static", "$envmap")),
            ("default-child", bottom, expected[bottom.relative_to(game).as_posix()].replace(f'    $envmap "{NAMESPACE}/cube"\n'.encode(), b""), ("static", "$envmap")),
            ("cube-extension", base, expected[base.relative_to(game).as_posix()].replace(f'"{NAMESPACE}/cube"'.encode(), f'"{NAMESPACE}/cube.vtf"'.encode()), ("logical paths", "extension")),
            ("proxy", bottom, expected[bottom.relative_to(game).as_posix()].replace(b"Water\n{", b"Water\n{\nProxies { AnimatedTexture { animatedTextureVar $normalmap } }"), ("unsupported",)),
            ("missing-hdr", hdr, None, ("cube.hdr.vtf",)),
            ("wrong-hdr-format", hdr, expected[f"materials/{NAMESPACE}/cube.vtf"], ("HDR", "format")),
            ("legacy-six-faces", ldr, cube(COLORS["green"], False, faces=6), ("seven stored square faces",)),
            ("native-unsupported-vtf75", hdr, cube(COLORS["blue"], True, faces=6, version=5), ("unsupported by CS:S",)),
        )
        for label, path, payload, fragments in cases:
            original = path.read_bytes()
            try:
                if payload is None:
                    path.unlink()
                else:
                    require(payload != original, f"{label}: negative fixture did not change its original source")
                    path.write_bytes(payload)
                _, report["negative"][label] = compile_pair(client, args.output, label, source, ids, game, True, expected, roots,
                                                           prior=retained, errors=fragments)
            finally:
                path.write_bytes(original)
        repaired, report["positive"]["repaired"] = compile_pair(client, args.output, "repaired", source, ids, game, True, expected, roots)
        require(repaired == retained, "Repaired original assets did not restore the packed whole BSP")
        client.call("cancel_property_edit", {"token": token})
        client.call("edit_faces", {"faces": [top], "material": DRY})
        dry = args.output / "dry.vmf"
        client.call("save_map", {"path": str(dry)})
        _, report["positive"]["dry-zero"] = compile_pair(client, args.output, "dry-zero", dry, ids, game, True, {}, [])
        client.call("open_map", {"path": str(source)})
        configure(client, "direct_ldr_hdr", True)
        current_assets, source_hash = identities(game), digest(source.read_bytes())
        require(current_assets == original_assets and source_hash == original_source_hash,
                "Validation changed the original authored VMF or failed to restore a mounted source")
        report["clean_eof"] = require_clean_eof(client)
        require(identities(game) == current_assets and digest(source.read_bytes()) == source_hash, "EOF changed authored source or assets")
        report["persistence"] = persistence(args.binary, args.output / "restart", game, stores, source, recipe)
        require(identities(game) == current_assets and digest(source.read_bytes()) == source_hash, "Persistence checks changed source or mounted assets")
        report["source_sha256"] = source_hash
        report["fixture_assets"] = current_assets
        report["status"] = "accepted"
    except Exception as error:
        report["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        try:
            if client is not None and client.process is not None:
                with stopped_on_exit(client):
                    pass
                report["cleanup_eof"] = require_clean_eof(client)
        except Exception as error:
            report["cleanup_error"] = f"{type(error).__name__}: {error}"
            report["status"] = "failed"
            raise
        finally:
            write_json(args.output / "results.json", report)
    return report


def main():
    parser = argparse.ArgumentParser(description="Validate portable static Water packing through isolated production MCP foreground/background builds, without graphics or game execution.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY, help="hammerr-headless or editor; launched with --headless")
    parser.add_argument("--compat-binary", type=Path, help="Optional prefeature binary for default-off BSP compatibility; only independently derived visible-Water fog flag additions may differ")
    parser.add_argument("--output", type=Path, required=True, help="New artifact directory below target/validation")
    args = parser.parse_args()
    args.binary, args.output = args.binary.resolve(), args.output.resolve()
    if args.compat_binary:
        args.compat_binary = args.compat_binary.resolve()
    require(args.output.is_relative_to(ROOT / "target/validation"), "Artifacts must stay under target/validation")
    args.output.mkdir(parents=True, exist_ok=False)
    report = validate(args)
    print(json.dumps({"status": report["status"], "compile_pairs": len(report["positive"]) + len(report["negative"]), "output": str(args.output)}))


if __name__ == "__main__":
    main()
