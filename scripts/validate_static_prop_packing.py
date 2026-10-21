import argparse
import hashlib
import json
import struct
import sys
import time
from pathlib import Path

import validate_cubemap_capture_probe as probe
from validate_build_recipes import protected_state
from validate_compile_jobs import stopped_on_exit, wait_job
from validate_compile_regions import apply as apply_region
from validate_face_lighting import DEFAULT_BINARY, ROOT, Client, detail, face_ref
from validate_instance_props import cleanup_clients
from validate_material_albedo import digest, fixture_texture, identities, require
from validate_recovery_exit import require_clean_eof
from validate_static_props import static_records
from validate_visibility import ROOM

MANIFEST = "hammerr/packed-static-props.json"
SETTING = "pack_static_prop_assets"
MODES = ("fullbright", "direct_ldr", "direct_ldr_hdr")
WALL = "hammerr733/wall"
OMITTED = "models/hammerr733/omitted.mdl"
POINTFILE = b"13 17 19\n23 29 31\n"
MAX_ASSET = 16 * 1024**2
MAX_FIXTURE = 64 * 1024**2
MAX_BSP = 32 * 1024**2
STUB = '''import json
import shutil
import sys
from pathlib import Path

args = sys.argv[1:]
root = Path(__file__).resolve().parent
game = Path(args[args.index("-game") + 1])
name = args[args.index("+map") + 1]
assert "-insecure" in args and args[args.index("+sv_lan") + 1] == "1"
shutil.copyfile(game / "maps" / (name + ".bsp"), root / "captured-run.bsp")
(root / "captured-run.json").write_text(json.dumps({"argv": args, "map": name}) + "\\n")
'''


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def read(path, maximum):
    require(path.is_file() and not path.is_symlink() and path.stat().st_size <= maximum,
            f"Unbounded or aliased fixture input: {path}")
    with path.open("rb") as stream:
        data = stream.read(maximum + 1)
    require(len(data) <= maximum, f"Fixture input grew beyond its bound: {path}")
    return data


def file_digest(path):
    value = hashlib.sha256()
    with path.open("rb") as stream:
        while chunk := stream.read(1024**2):
            value.update(chunk)
    return value.hexdigest()


def canonical(name):
    return (isinstance(name, str) and name == name.lower() and "\\" not in name
            and not name.startswith("/") and all(part and part not in (".", "..") for part in name.split("/")))


def inventory(path, game):
    document = json.loads(read(path, 1024**2))
    model, assets = document["model"], document["assets"]
    require(canonical(model) and model.startswith("models/") and model.endswith(".mdl"), "Inventory model is not canonical")
    require(isinstance(assets, dict) and 5 <= len(assets) <= 256, "Inventory needs5..256 explicitly hashed original files")
    require(all(canonical(name) and name.startswith(("models/", "materials/")) for name in assets), "Inventory asset path is not canonical")
    required = {model[:-4] + suffix for suffix in (".mdl", ".vvd", ".dx90.vtx", ".phy")}
    require(required <= assets.keys(), "Fixture inventory needs all four original model companions, including PHY")
    payloads = {name: read(game / name, MAX_ASSET) for name in assets}
    require(sum(map(len, payloads.values())) <= MAX_FIXTURE, "Fixture inventory exceeds64MiB")
    require(all(digest(payloads[name]) == expected for name, expected in assets.items()), "Fixture source differs from its independently pinned inventory")
    return document, payloads


def fixture_game(output, payloads):
    game = output / "game"
    game.mkdir()
    for name, data in payloads.items():
        target = game / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
    (game / "gameinfo.txt").write_text('GameInfo { FileSystem { SearchPaths { Game "|gameinfo_path|." } } }\n')
    wall = game / f"materials/{WALL}.vmt"
    wall.parent.mkdir(parents=True, exist_ok=True)
    wall.write_text(f'LightmappedGeneric {{ $basetexture "{WALL}" }}\n')
    wall.with_suffix(".vtf").write_bytes(fixture_texture(2, 2, (0.5, 0.5, 0.5), (255, 255, 255, 255)))
    for suffix in (".mdl", ".vvd", ".dx90.vtx", ".phy"):
        path = game / (OMITTED[:-4] + suffix)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"Private malformed omitted model companion\n")
    tools = game / "materials/tools"
    tools.mkdir()
    (tools / "toolsblack.vtf").write_bytes(fixture_texture(2, 2, (0, 0, 0), (0, 0, 0, 255)))
    (tools / "toolsblack.vmt").write_text('LightmappedGeneric { $basetexture "tools/toolsblack" }\n')
    executable = output / "cstrike_linux64"
    executable.write_text(f"#!{sys.executable}\n" + STUB)
    executable.chmod(0o700)
    return game


def start_client(args, directory, game, stores):
    directory.mkdir()
    return Client(None, args.binary, directory, game / "gameinfo.txt", request_timeout=args.request_timeout,
                  environment={"HAMMERR_RECENT_MAPS_DIR": str(stores / "recent"),
                               "HAMMERR_BUILD_RECIPES": str(stores / "recipes.json"),
                               "HAMMERR_PREFABS_DIR": str(stores / "prefabs"),
                               "HAMMERR_RECOVERY_DIR": str(directory / "recovery"),
                               "XDG_CONFIG_HOME": str(stores / "config"), "XDG_STATE_HOME": str(stores / "state"),
                               "TMPDIR": str(args.output / "tmp")})


def configure(client, mode="fullbright", enabled=None, bake=False):
    settings = {"lighting": mode, "physics": "none", "bounces": 0, "visibility": "fast", "sky_quality": "fast",
                "generate_reflections": False, "pack_sky_assets": False, "pack_water_assets": False,
                "bake_static_prop_vertices": bake}
    if enabled is not None:
        settings[SETTING] = enabled
    return client.call("set_compile_settings", settings)


def prop(client, model, solid, origin=(40, -8, 48)):
    return client.call("create_entity", {"classname": "prop_static", "origin": origin,
                       "properties": {"model": model, "angles": "0 0 0", "solid": str(solid), "skin": "0",
                                      "modelscale": "1", "body": "0", "rendercolor": "255 255 255",
                                      "renderamt": "255", "rendermode": "0", "renderfx": "0"}})["id"]


def fixture(client, source, model):
    ids = []
    for low, high in ROOM:
        identifier = client.call("create_block", {"mins": low, "maxs": high, "material": WALL})["id"]
        brush = detail(client, [identifier])[0]
        client.call("edit_faces", {"faces": [face_ref(brush, face) for face in brush["faces"]], "lightmap_scale": 64})
        ids.append(identifier)
    for classname, origin, properties in (
        ("info_player_terrorist", [-192, -192, 8], {}),
        ("info_player_counterterrorist", [192, 192, 8], {}),
        ("light", [0, 0, 224], {"_light": "255 192 128 64", "_lightHDR": "128 192 255 96",
                               "_constant_attn": "1", "_linear_attn": "0", "_quadratic_attn": "0"}),
    ):
        ids.append(client.call("create_entity", {"classname": classname, "origin": origin, "properties": properties})["id"])
    visible = prop(client, model, 6)
    ids.append(visible)
    hidden = prop(client, OMITTED, 6, (0, 0, 48))
    client.call("set_object_visibility", {"ids": [hidden], "action": "hide"})
    ids.append(hidden)
    outside = prop(client, OMITTED, 6, (512, 0, 48))
    ids.append(outside)
    apply_region(client, low=[-272, -272, -16], high=[272, 272, 272])
    client.call("save_map", {"path": str(source)})
    return ids, visible, hidden, outside


def snapshot(client, source, ids, game):
    info = client.call("get_map_info")
    require(not info["rendering"]["initialized"] and not info["launching"] and not info["game_running"],
            "Compiler validation initialized graphics or left a launcher running")
    recipes = client.call("get_build_recipes")
    return {"map": protected_state(client), "history": client.call("get_history"), "objects": detail(client, ids),
            "camera": client.call("get_camera"), "settings": client.call("get_compile_settings"),
            "recipes": recipes, "recipe_sha256": digest(Path(recipes["path"]).read_bytes()),
            "source_sha256": digest(source.read_bytes()), "assets": identities(game)}


def read_bsp(path):
    data = read(path, MAX_BSP)
    probe.bsp_lumps(data)
    return data


def pak(data):
    payload = probe.bsp_lumps(data)[40][2]
    return probe.zip_entries(payload)[0] if payload else {}


def sprp(data):
    payload = probe.bsp_lumps(data)[35][2]
    if not payload:
        return 0, b"", {"models": [], "props": []}
    require(len(payload) >= 4, "Truncated game-lump table")
    count = struct.unpack_from("<i", payload)[0]
    require(0 <= count <= 64 and 4 + count * 16 <= len(payload), "Unbounded game-lump table")
    matches = []
    for index in range(count):
        identifier, flags, version, offset, size = struct.unpack_from("<IHHii", payload, 4 + index * 16)
        if identifier == int.from_bytes(b"sprp", "big"):
            require(flags == 0 and 0 <= offset <= offset + size <= len(data), "Invalid native sprp range")
            matches.append((version, data[offset:offset + size]))
    require(len(matches) <= 1, "Duplicate sprp game lumps")
    if not matches:
        return 0, b"", {"models": [], "props": []}
    version, raw = matches[0]
    return version, raw, static_records(data)


def verify_output(data, stats, expected, enabled):
    entries = pak(data)
    version, raw, records = sprp(data)
    if not enabled:
        require(MANIFEST not in entries and not set(expected).intersection(entries), "Disabled build packed original model files")
        require(stats.get("static_prop_assets") is None, "Disabled build reports a requested original model plan")
        return {"enabled": False, "bsp_sha256": digest(data)}
    require(MANIFEST in entries, "Requested packing omitted its manifest")
    wanted = {"version": 1, "static_props_version": version, "static_props_sha256": digest(raw),
              "models": records["models"], "uses": [{"model": row[6], "skin": row[11], "solid": row[9]} for row in records["props"]],
              "assets": [{"path": name, "sha256": digest(payload), "bytes": len(payload)} for name, payload in sorted(expected.items())]}
    manifest = json.loads(entries[MANIFEST])
    require(manifest == wanted, f"Packed manifest does not bind exact native records and original closure: {manifest}")
    require(set(entries) == {MANIFEST, *expected}, "PAK contains missing or unregistered original files")
    require(all(entries[name] == payload for name, payload in expected.items()), "Compiler rewrote an original model/material companion")
    wanted_stats = {"models": len(records["models"]), "files": len(expected), "asset_bytes": sum(map(len, expected.values()))}
    require(stats["static_prop_assets"] == wanted_stats, "Original asset statistics differ from packed bytes")
    return {"enabled": True, "bsp_sha256": digest(data), "manifest": manifest, "stats": wanted_stats, "original_payloads_exact": True}


def parse_failure(response):
    text = [row["text"] for row in response["content"] if row["type"] == "text"]
    require(len(text) == 1, "Failed compile did not return one structured result")
    return json.loads(text[0])


def require_failure(result, fragments):
    require(not result["success"] and result.get("bsp") is None and not result.get("published", False), "Unsupported model closure was published")
    messages = [row["message"] for row in result.get("diagnostics", []) if row["level"] == "error"]
    if result.get("error"):
        messages.append(result["error"])
    require(any(all(fragment.lower() in message.lower() for fragment in fragments) for message in messages),
            f"Refusal lacks its source/reason: expected {fragments}, observed {messages}")


def compile_pair(client, args, source, ids, game, label, enabled, expected, prior=None, errors=()):
    before = snapshot(client, source, ids, game)
    paths = [args.output / f"{label}{suffix}.bsp" for suffix in ("", "-background")]
    for path in paths:
        path.with_suffix(".lin").write_bytes(POINTFILE)
        if prior is not None:
            path.write_bytes(prior)
    with stopped_on_exit(client):
        args.compile_calls += 1
        response = client.call("compile_map", {"path": str(paths[0])}, error=bool(errors))
        result = parse_failure(response) if errors else response
        foreground = client.call("get_compile_status")["job"]
        require(foreground["settings"][SETTING] is enabled and foreground["native_runtime"] is None,
                "Foreground job lost its frozen packing option or requested native capture")
        if errors:
            require_failure(result, errors)
            require(foreground["status"] == "failed" and read_bsp(paths[0]) == prior, "Foreground failure replaced the previous BSP")
        else:
            require(result["success"] and result["leak"] is None and foreground["status"] == "completed"
                    and foreground["result"]["published"] and not foreground["stale"], f"Foreground compile failed: {result}")
        require(snapshot(client, source, ids, game) == before, "Foreground compile changed authored state or mounted sources")
        args.compile_calls += 1
        started = client.call("start_compile", {"path": str(paths[1])})["job"]
        require(started["settings"][SETTING] is enabled and started["native_runtime"] is None, "Background request lost its packing option")
        job = wait_job(client)
        require(all(job[key] == started[key] for key in ("token", "settings", "recipe", "generation", "revision")),
                "Background completion changed its captured request")
        if errors:
            require(job["status"] == "failed", "Background unsupported closure succeeded")
            require_failure(job["result"], errors)
        else:
            require(job["status"] == "completed" and job["result"]["success"] and job["result"]["published"]
                    and not job["stale"], f"Background compile failed: {job}")
        data = read_bsp(paths[0])
        require(read_bsp(paths[1]) == data and all(path.with_suffix(".lin").read_bytes() == POINTFILE for path in paths),
                "Foreground/background BSP agreement or previous pointfile bytes changed")
        require(snapshot(client, source, ids, game) == before, "Background compile changed authored state or mounted sources")
    packed = None
    if not errors:
        packed = verify_output(data, result["stats"], expected, enabled)
        verify_output(data, job["result"]["stats"], expected, enabled)
    return data, {"bsp": str(paths[0]), "background_bsp": str(paths[1]), "bsp_sha256": digest(data),
                  "result": result, "background": job, "packed": packed, "document_and_sources_preserved": True,
                  "prior_pointfile_preserved": True}


def non_pak_equal(first, second):
    require(all(a == b for index, (a, b) in enumerate(zip(probe.bsp_lumps(first), probe.bsp_lumps(second), strict=True)) if index != 40),
            "Original packing changed non-PAK geometry, lighting or native records")


def frozen_job(client):
    before = client.call("get_compile_status")
    require(before["job"]["settings"][SETTING] is True and not before["job"]["stale"], "Frozen-job control needs a current packed build")
    client.call("set_compile_settings", {SETTING: False})
    changed = client.call("get_compile_status")
    require(changed["job"] == {**before["job"], "stale": True} and not changed["launch"]["ready"], "Packing setting rewrote the captured job")
    client.call("set_compile_settings", {SETTING: True})
    restored = client.call("get_compile_status")
    require(restored["job"] == before["job"] and restored["launch"] == before["launch"], "Restoring packing did not restore the frozen job")
    return {"before": before, "disabled": changed, "restored": restored}


def refresh_repaired_source(client, args):
    status = client.call("get_compile_status")
    path = Path(status["job"]["path"])
    original = read_bsp(path)
    args.compile_calls += 1
    result = client.call("compile_map", {"path": str(path)})
    refreshed = client.call("get_compile_status")
    require(result["success"] and refreshed["job"]["result"]["published"] and not refreshed["job"]["stale"]
            and refreshed["launch"]["ready"], "Rebuilding restored sources did not produce a current packed build")
    require(read_bsp(path) == original, "Rebuilding identical restored inputs changed packed BSP bytes")
    return {"recompiled": True, "bsp_sha256": digest(original), "current": True}


def source_custody(client, args, game, originals):
    before = client.call("get_compile_status")
    require(not before["job"]["stale"], "Source custody needs a current successful build")
    results = {}
    for name, payload in originals.items():
        path = game / name
        changed = bytearray(payload)
        changed[-1] ^= 1
        try:
            path.write_bytes(changed)
            stale = client.call("get_compile_status")
            require(stale["job"]["stale"] and not stale["launch"]["ready"] and name in json.dumps(stale),
                    f"Completed packed build did not retain original source custody: {name}")
            results[name] = {"stale": stale["job"]["stale"], "launch": stale["launch"]}
        finally:
            path.write_bytes(payload)
        require(client.call("get_compile_status")["job"]["stale"], "Rewriting source bytes unexpectedly restored their captured file identity")
        results[name]["repair"] = refresh_repaired_source(client, args)
    return results


def unused_sources(client, game, extra=()):
    names = [OMITTED[:-4] + suffix for suffix in (".mdl", ".vvd", ".dx90.vtx", ".phy")]
    names.extend(extra)
    before = client.call("get_compile_status")
    originals = {name: (game / name).read_bytes() for name in names}
    try:
        for name in names:
            (game / name).write_bytes(b"Another malformed input outside the emitted root closure\n")
        current = client.call("get_compile_status")
        require(not current["job"]["stale"] and current["launch"] == before["launch"],
                "Hidden/region-omitted companion or unused PHY entered original source custody")
        return {"unconsumed": names, "status": current}
    finally:
        for name, payload in originals.items():
            (game / name).write_bytes(payload)


def missing_candidate_custody(client, args, game, candidates, originals):
    results = {}
    for name, fallback in candidates.items():
        require(canonical(name) and name.startswith("materials/") and name.endswith(".vmt")
                and fallback in originals and fallback.endswith(".vmt"), "Missing-candidate inventory is invalid")
        path = game / name
        require(not path.exists(), "Missing-candidate control already exists")
        path.parent.mkdir(parents=True, exist_ok=True)
        try:
            path.write_bytes(originals[fallback])
            status = client.call("get_compile_status")
            require(status["job"]["stale"] and not status["launch"]["ready"] and name in json.dumps(status),
                    "A new earlier material candidate did not invalidate the original packed build")
            results[name] = status
        finally:
            path.unlink(missing_ok=True)
        results[name]["repair"] = refresh_repaired_source(client, args)
    return results


def completed_output_tamper(client, output, original):
    status = client.call("get_compile_status")
    path = Path(status["job"]["path"])
    require(read_bsp(path) == original, "Completed-output tamper needs the current successful exact BSP")
    entries = pak(original)
    manifest = json.loads(entries[MANIFEST])
    changed = {**manifest, "static_props_sha256": "0" * 64}
    payload = probe.make_zip({**entries, MANIFEST: json.dumps(changed, separators=(",", ":")).encode()})
    altered = bytearray(original)
    altered.extend(bytes(-len(altered) % 4))
    struct.pack_into("<4i", altered, 8 + 40 * 16, len(altered), len(payload), 0, 0)
    altered.extend(payload)
    receipt = output / "captured-run.json"
    receipt.unlink(missing_ok=True)
    try:
        path.write_bytes(altered)
        refused = client.call("launch_game", error=True)
        require(not receipt.exists(), "Tampered completed output reached the safe Run child")
        return {"response": refused, "child_not_invoked": True, "coverage": "completed-output identity and Run admission"}
    finally:
        path.write_bytes(original)


def stage_run(client, output, originals, stats):
    capture, receipt = output / "captured-run.bsp", output / "captured-run.json"
    capture.unlink(missing_ok=True)
    receipt.unlink(missing_ok=True)
    launched = client.call("launch_game")
    try:
        deadline = time.monotonic() + 15
        while not receipt.is_file() and time.monotonic() < deadline:
            time.sleep(0.02)
        require(receipt.is_file() and capture.is_file(), "Safe Run-stage child did not capture the actual staged BSP")
        data = read_bsp(capture)
        verification = verify_output(data, stats, originals, True)
        status = client.call("game_log")
        return data, {"launch": launched, "log": status, "capture": str(capture),
                      "argv": json.loads(receipt.read_text()), "packed": verification, "safe_child_only": True}
    finally:
        client.call("stop_game")


def persistence(args, game, stores, source, recipe, clients):
    client = start_client(args, args.output / "restart", game, stores)
    clients.append(client)
    require(client.call("get_compile_settings")[SETTING] is False, "Saved recipes changed startup defaults")
    listing = client.call("get_build_recipes")
    saved = next(row for row in listing["recipes"] if row["id"] == recipe)
    require(saved["settings"][SETTING] is True, "Restart lost the saved packing setting")
    client.call("apply_build_recipe", {"id": recipe})
    require(client.call("get_compile_settings")[SETTING] is True, "Saved packing recipe did not apply")
    client.call("set_compile_settings", {SETTING: False})
    client.call("open_recent_map", {"path": str(source)})
    require(client.call("get_compile_settings")[SETTING] is True, "Recent-map restart lost original packing")
    return {"recipe_id": recipe, "recipe_and_recent_restored": True, "clean_eof": require_clean_eof(client)}


def validate(args):
    document, originals = inventory(args.fixture_inventory, args.fixture_game)
    game = fixture_game(args.output, originals)
    source, stores = args.output / "source.vmf", args.output / "stores"
    clients, failure = [], None
    report = {"status": "failed", "binary": str(args.binary), "binary_sha256": file_digest(args.binary),
              "script_sha256": digest(Path(__file__).read_bytes()), "fixture_inventory": document,
              "fixture_inventory_sha256": digest(args.fixture_inventory.read_bytes()), "native_exercised": False,
              "desktop_input": False, "graphics_initialized": False, "positive": {}, "negative": {}}
    try:
        client = start_client(args, args.output / "client", game, stores)
        clients.append(client)
        schemas = {tool["name"]: tool["inputSchema"] for tool in client.rpc("tools/list", {})["tools"]}
        field = schemas["set_compile_settings"]["properties"][SETTING]
        types = field["type"] if isinstance(field["type"], list) else [field["type"]]
        require("boolean" in types and set(types) <= {"boolean", "null"} and SETTING not in schemas["set_compile_settings"].get("required", []),
                "Original packing is not a discoverable optional boolean")
        require(client.call("get_compile_settings")[SETTING] is False, "Original packing must default off")
        configure(client)
        ids, visible, hidden, outside = fixture(client, source, document["model"])
        initial_assets = identities(game)
        client.call("save_build_recipe", {"name": "Original model packing disabled"})
        require(SETTING not in json.loads((stores / "recipes.json").read_text())["recipes"][0]["settings"], "Default-false recipe changed legacy encoding")
        require(SETTING not in json.loads((stores / "recent/recent-maps.json").read_text())["entries"][0]["build_settings"], "Default-false recent map changed legacy encoding")
        default, report["positive"]["default"] = compile_pair(client, args, source, ids, game, "default", False, originals)
        configure(client, enabled=False)
        disabled, report["positive"]["explicit-false"] = compile_pair(client, args, source, ids, game, "explicit-false", False, originals)
        require(default == disabled, "Default and explicit false whole BSP bytes differ")
        controls = {"fullbright": default}
        recipe = None
        retained = None
        for mode in MODES:
            if mode != "fullbright":
                configure(client, mode, False)
                controls[mode], report["positive"][f"disabled-{mode}"] = compile_pair(client, args, source, ids, game, f"disabled-{mode}", False, originals)
            configure(client, mode, True)
            saved = client.call("save_build_recipe", {"name": f"Original model packing {mode}"})
            recipe = saved["active"]["id"]
            before = snapshot(client, source, ids, game)
            client.call("set_compile_settings", {"lighting": "unknown", SETTING: False}, error=True)
            require(snapshot(client, source, ids, game) == before, "Rejected combined update changed packing or document state")
            data, receipt = compile_pair(client, args, source, ids, game, f"packed-{mode}", True, originals)
            report["positive"][f"packed-{mode}"] = receipt
            non_pak_equal(controls[mode], data)
            require(receipt["result"]["stats"]["static_props"] == 1, "Hidden/region-omitted props entered emitted roots")
            if mode == "fullbright":
                retained = data
                report["frozen_job"] = frozen_job(client)
            if mode == "direct_ldr_hdr":
                lumps = probe.bsp_lumps(data)
                require(lumps[53][2] and lumps[58][2], "Dual-lighting fixture omitted HDR lightmaps/faces")
        report["source_custody"] = source_custody(client, args, game, originals)
        report["missing_candidate_custody"] = missing_candidate_custody(client, args, game, document.get("missing_candidates", {}), originals)
        report["omitted_sources"] = unused_sources(client, game)
        configure(client, enabled=True)
        current, report["positive"]["run-source"] = compile_pair(client, args, source, ids, game, "run-source", True, originals)
        staged, report["run_staging"] = stage_run(client, args.output, originals, report["positive"]["run-source"]["result"]["stats"])
        require(pak(staged) == pak(current), "Run staging changed original model manifest or entries")
        report["completed_output_manifest_tamper"] = completed_output_tamper(client, args.output, current)
        for name, original in originals.items():
            if name.endswith(".vmt"):
                replacement = b'VertexLitGeneric { $basetexture "env_cubemap" }\n'
            elif name.endswith(".vtf"):
                replacement = original[:-1]
            else:
                replacement = None
            if replacement is None:
                continue
            path = game / name
            try:
                path.write_bytes(replacement)
                label = "refuse-" + name.replace("/", "-")
                fragments = ("materials/env_cubemap.vtf",) if name.endswith(".vmt") else (name,)
                _, report["negative"][label] = compile_pair(client, args, source, ids, game, label, True, originals, retained, fragments)
            finally:
                path.write_bytes(original)
        client.call("set_entity_properties", {"id": visible, "set": {"solid": "0"}})
        source0 = args.output / "solid0.vmf"
        client.call("save_map", {"path": str(source0)})
        without_phy = {name: value for name, value in originals.items() if not name.endswith(".phy")}
        _, report["positive"]["solid0"] = compile_pair(client, args, source0, ids, game, "solid0", True, without_phy)
        report["unused_phy"] = unused_sources(client, game, [document["model"][:-4] + ".phy"])
        client.call("set_object_visibility", {"ids": [visible], "action": "hide"})
        empty = args.output / "zero.vmf"
        client.call("save_map", {"path": str(empty)})
        _, report["positive"]["zero"] = compile_pair(client, args, empty, ids, game, "zero", True, {})
        client.call("open_map", {"path": str(source), "discard_changes": True})
        configure(client, "direct_ldr_hdr", True)
        client.call("save_map")
        report["clean_eof"] = require_clean_eof(client)
        require(identities(game) == initial_assets, "Validation failed to restore original mounted assets after releasing owned Run logs")
        report["persistence"] = persistence(args, game, stores, source, recipe, clients)
        require(identities(game) == initial_assets, "Restart/EOF changed original mounted assets")
        if args.compare_report:
            reference = json.loads(read(args.compare_report, 32 * 1024**2))
            require(reference["status"] == "accepted" and reference["fixture_inventory"] == document, "Cross-binary reference is not accepted for the same fixture")
            require(reference["positive"].keys() == report["positive"].keys(), "Cross-binary positive case sets differ")
            for label, receipt in report["positive"].items():
                other = read_bsp(Path(reference["positive"][label]["bsp"]))
                require(read_bsp(Path(receipt["bsp"])) == other, f"Production MCP programs emitted different actual BSPs: {label}")
            report["cross_binary_actual_bsp_parity"] = list(report["positive"])
        report["status"] = "accepted"
    except BaseException as error:
        failure = error
        report["error"] = f"{type(error).__name__}: {error!r}"
        raise
    finally:
        report["compile_calls"] = args.compile_calls
        errors = cleanup_clients(clients, report)
        if errors:
            report["status"] = "failed"
        try:
            write_json(args.output / "results.json", report)
        except BaseException as error:
            errors.append(error)
        if errors:
            raise BaseExceptionGroup("Original model packing validation and cleanup failed", ([failure] if failure else []) + errors) from None
    return report


def main():
    parser = argparse.ArgumentParser(description="Validate original static-prop packing through either production MCP executable in private stdio mode. Python>=3.11 and a complete admitted standalone fixture are required; no installed game, GPU, desktop input or native engine. Inventory JSON is {model: canonical models/...mdl, assets: {canonical relative path: SHA256}} and must enumerate every declared model/material dependency plus matching PHY. Inputs are read only; bounded copies under --output are mutated/restored for closure and completed-source custody checks. Covers default false, all three lighting modes without VHV, foreground/background exact BSP parity, recipes/restart, hidden/region omission, solid0 unused PHY, empty manifest and actual Run staging through a safe file-copy child. --compare-report compares actual corresponding BSP files across production programs. Native source-absent rendering/collision requires the separately pinned private725 observer.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY, help="Production hammerr-headless or editor; invoked with --headless")
    parser.add_argument("--fixture-game", type=Path, required=True, help="Read-only standalone directory holding the exact inventory assets")
    parser.add_argument("--fixture-inventory", type=Path, required=True, help="Independently checked original asset inventory JSON")
    parser.add_argument("--output", type=Path, required=True, help="Fresh directory under repository target/validation; artifacts stay on disk")
    parser.add_argument("--request-timeout", type=float, default=180, help="Client HTTP/stdio request timeout in seconds; default180")
    parser.add_argument("--compare-report", type=Path, help="Accepted results.json from the other production MCP executable")
    args = parser.parse_args()
    args.binary = args.binary.resolve(strict=True)
    args.fixture_game = args.fixture_game.resolve(strict=True)
    args.fixture_inventory = args.fixture_inventory.resolve(strict=True)
    args.output = args.output.resolve()
    require(args.output.is_relative_to(ROOT / "target/validation") and not args.output.exists(), "Use a fresh output under target/validation")
    require(args.request_timeout > 0, "Request timeout must be positive")
    require(not args.fixture_game.is_relative_to(args.output), "Original fixture cannot be a child of writable output")
    args.output.mkdir(parents=True)
    (args.output / "tmp").mkdir()
    args.compile_calls = 0
    report = validate(args)
    print(json.dumps({"status": report["status"], "compile_calls": report["compile_calls"], "output": str(args.output)}))


if __name__ == "__main__":
    main()
