import argparse
import json
import os
import re
import resource
import struct
import subprocess
import sys
import time
import traceback
import uuid
from pathlib import Path
from unittest.mock import patch

import validate_cubemap_capture_probe as probe
import validate_hdr_reflections as reflections
import validate_native_runtime as runtime
from private_runtime_config import (
    private_launch_options,
    private_native_process,
    private_observer,
    verify_private_launch,
)
from validate_compile_jobs import wait_job
from validate_cubemap_reflections import ppm
from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_hdr_lighting import read_probe
from validate_sky_preview import set_sky

MANIFEST = "hammerr/packed-sky.json"
FACES = ("rt", "bk", "lf", "ft", "up", "dn")
ANGLES = ((0, 0), (0, 90), (0, 180), (0, 270), (-89, 0), (89, 0))
COLORS = ((220, 24, 16), (16, 220, 32), (16, 32, 220), (220, 200, 16), (16, 200, 200), (200, 16, 200))
FACTORS = (0.25, 0.45, 0.70, 1.0)
PATCHES = ((400, 240), (880, 240), (400, 480), (880, 480))
OBSERVER_NAME = "hammerr_packed_sky_observer"
OBSERVER_CLASS = "info_target"
PRESERVATION_SERVER_SHA256 = "7887c8cc24a3dec6b8739bfebcad48c00b95423ebcbfc647f49fef1ab904dbe2"
RUNTIME_OBSERVER_READY = "HAMMERR_PACKED_SKY_RUNTIME_OBSERVER_CREATED"
RUNTIME_OBSERVER_SCHEDULED = "HAMMERR_PACKED_SKY_RUNTIME_OBSERVER_SCHEDULED"
RUNTIME_OBSERVER_REGISTERED = "HAMMERR_PACKED_SKY_PLAYER_SPAWN_LISTENER_REGISTERED"
RUNTIME_OBSERVER_EVENT = "HAMMERR_PACKED_SKY_PLAYER_SPAWN_CALLBACK"
RUNTIME_OBSERVER_MISSING = "HAMMERR_PACKED_SKY_RUNTIME_CONTROLLER_MISSING"
FINISHED = "HAMMERR_PACKED_SKY_FINISHED"
NATIVE_CASES = ("published-ldr", "published-dual-ldr", "published-dual-hdr", "run-dual-hdr", "run-reflections-hdr")
OBSERVER = """
stage <- 0;
view <- 0;
ready_at <- 0.0;
function HammeRRThink() {
    local player = Entities.FindByClassname(null, "player");
    if (player == null) return 0.25;
    if (stage == 0) {
        SendToConsole("sv_cheats 1; sv_lan 1; joystick 0; bot_quota 0; bot_kick; jointeam 2; joinclass 1; hidepanel info; hidepanel team; hidepanel class_ter; hidepanel class_ct; r_drawvgui 0; gameui_hide; cl_drawhud 0; r_drawviewmodel 0; r_drawothermodels 0; mat_fullbright 0; toggle con_notifytime 0");
        ready_at = Time() + 6.0;
        stage = 1;
    }
    if (stage == 1 && Time() > ready_at && player.GetTeam() == 2 && player.GetHealth() == 100) {
        SendToConsole("noclip; god; setpos 0 0 0; setang 0 0 0; sv_lan; mat_hdr_level; mat_fullbright");
        ready_at = Time() + 4.0;
        stage = 2;
    }
    if (stage >= 2) {
        player.SetOrigin(Vector(0, 0, 0) - (player.EyePosition() - player.GetOrigin()));
        player.SetVelocity(Vector(0, 0, 0));
    }
    if (stage == 2 && Time() > ready_at) {
        printl("HAMMERR_PACKED_SKY_FACE " + view + " " + player.GetTeam() + " " + player.GetHealth());
        stage = 3;
    }
    if (stage == 3) SendToConsole("exec hammerr_packed_sky_once.cfg");
    return 0.25;
}
"""


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def require_script_free(data):
    require(not re.search(rb'"(?:vscripts|thinkfunction)"|"classname"\s*"logic_script"', data, re.IGNORECASE) and OBSERVER_NAME.encode() not in data, "Sky capture fixture contains an authored script or observer")


def observer_mapspawn():
    return f'''printl("HAMMERR_MAPSPAWN");
if (Entities.FindByName(null, "{OBSERVER_NAME}") != null) throw "Private sky observer name collision";
local observer = SpawnEntityFromTable("{OBSERVER_CLASS}", {{
    targetname = "{OBSERVER_NAME}",
    vscripts = "{OBSERVER_NAME}.nut"
}});
if (observer == null) throw "Private sky observer creation failed";
printl("{RUNTIME_OBSERVER_READY}");
HammeRRPackedSkyObserverStarted <- false;
function OnGameEvent_player_spawn(params) {{
    if (HammeRRPackedSkyObserverStarted) return;
    HammeRRPackedSkyObserverStarted = true;
    if (!("userid" in params) || typeof(params.userid) != "integer" || params.userid <= 0) throw "Private sky player_spawn lacks a positive userid";
    printl("{RUNTIME_OBSERVER_EVENT} " + params.userid);
    local observer = Entities.FindByName(null, "{OBSERVER_NAME}");
    if (observer == null) {{
        printl("{RUNTIME_OBSERVER_MISSING} " + params.userid);
        throw "Private sky observer missing during player_spawn";
    }}
    AddThinkToEnt(observer, "HammeRRThink");
    printl("{RUNTIME_OBSERVER_SCHEDULED}");
}}
__CollectGameEventCallbacks(getroottable());
printl("{RUNTIME_OBSERVER_REGISTERED}");
'''


def sky_texture(color, hdr=False):
    header = bytearray(80)
    header[:4] = b"VTF\0"
    struct.pack_into("<IIIHH", header, 4, 7, 2, 80, 32, 32)
    struct.pack_into("<I", header, 20, 0x0C | (0 if hdr else 0x40))
    struct.pack_into("<H", header, 24, 1)
    struct.pack_into("<3f", header, 32, 0.5, 0.5, 0.5)
    struct.pack_into("<fI", header, 48, 1.0, 24 if hdr else 0)
    header[56] = 6
    struct.pack_into("<i", header, 57, -1)
    struct.pack_into("<H", header, 63, 1)
    images = bytearray()
    for size in (1, 2, 4, 8, 16, 32):
        for y in range(size):
            for x in range(size):
                factor = 0.60 if size == 1 else FACTORS[2 * (y >= size // 2) + (x >= size // 2)]
                if hdr:
                    images.extend(struct.pack("<4e", *(channel / 255 * 4 * factor for channel in color), 1.0))
                else:
                    images.extend(bytes([*(round(channel * factor) for channel in color), 255]))
    return bytes(header + images)


def family_fixture(game, family, paired):
    folder = game / "materials/skybox"
    folder.mkdir(parents=True, exist_ok=True)
    expected = {}

    def asset(name, data, runtime_asset=True):
        path = "materials/skybox/" + name
        (game / path).write_bytes(data)
        if runtime_asset:
            expected[path] = data

    for index, face in enumerate(FACES):
        ldr = f"{family}_{face}_ldr"
        hdr = f"{family}_{face}_hdr"
        asset(ldr + ".vtf", sky_texture(COLORS[index]))
        if paired:
            asset(hdr + ".vtf", sky_texture(COLORS[(index + 2) % len(COLORS)], True))
        parameters = f'"$basetexture" "skybox/{ldr}"\r\n'
        if paired:
            parameters += f'"$hdrbasetexture" "skybox/{hdr}"\r\n'
        if face == "rt":
            parameters = f'"replace" {{ {parameters} }}\r\n'
            material = f'// Original Patch bytes remain portable.\r\n"Patch" {{ "include" "materials/skybox/{family}_base.vmt" {parameters} }}\r\n'
        else:
            material = f'// Original {face} material.\r\n"{"Sky" if paired else "UnlitGeneric"}" {{ {parameters} "$nofog" "1" "$ignorez" "1" "%tooltexture" "skybox/{family}_editor" }}\r\n'
        asset(family + face + ".vmt", material.encode())
    parameters = f'"$basetexture" "skybox/{family}_overridden_ldr"\r\n'
    if paired:
        parameters += f'"$hdrbasetexture" "skybox/{family}_overridden_hdr"\r\n'
        asset(family + "_overridden_hdr.vtf", sky_texture((48, 64, 96), True))
    asset(family + "_overridden_ldr.vtf", sky_texture((48, 64, 96)))
    base = f'// Preserve include and overridden runtime references.\r\n"{"Sky" if paired else "UnlitGeneric"}" {{ {parameters} "$nofog" "1" "$ignorez" "1" "%tooltexture" "skybox/{family}_editor" }}\r\n'
    asset(family + "_base.vmt", base.encode())
    asset(family + "_editor.vtf", sky_texture((255, 128, 0)), False)
    return expected


def pak(data):
    payload = probe.bsp_lumps(data)[40][2]
    return probe.zip_entries(payload)[0] if payload else {}


def verify_sky(data, family, channels, expected):
    entries = pak(data)
    require(MANIFEST in entries, "Packed sky manifest is absent")
    manifest = json.loads(entries[MANIFEST])
    require(set(manifest) == {"version", "family", "channels", "assets"}, manifest)
    require((manifest["version"], manifest["family"], manifest["channels"]) == (1, family, channels), manifest)
    require(manifest["assets"] == [{"path": name, "sha256": probe.digest(expected[name])} for name in sorted(expected)], "Manifest differs from the exact original runtime closure")
    require(all(entries.get(name) == raw for name, raw in expected.items()), "Packed VMT/Patch/VTF bytes changed or are missing")
    require({name for name in entries if name.startswith("materials/skybox/")} == set(expected), "Packed sky includes an editor image or an unregistered file")
    require(not any(key in entries[MANIFEST].decode() for key in ["source-game", "gameinfo.txt", str(Path.home())]), "Manifest contains machine-local provenance")
    return {"manifest": manifest, "manifest_sha256": probe.digest(entries[MANIFEST]), "bsp_sha256": probe.digest(data), "files": len(expected), "asset_bytes": sum(map(len, expected.values())), "original_runtime_bytes_exact": True, "archive_verified": True, "native_exercised": False}


def snapshot(client, source, ids):
    info = client.call("get_map_info")
    return source.read_bytes(), detail(client, ids), client.call("get_property_inspector", {"world": True}), {key: info[key] for key in ["undo", "redo", "selection", "unsaved_changes"]}


def asset_snapshot(game):
    return {path.relative_to(game).as_posix(): probe.digest(path.read_bytes()) for path in sorted((game / "materials").rglob("*")) if path.is_file()}


def build_pair(client, output, name, source, ids, expected):
    before = snapshot(client, source, ids)
    path = output / f"{name}.bsp"
    result = client.call("compile_map", {"path": str(path)})
    require(result["success"] and result["leak"] is None, result)
    data = probe.read_bounded(path, probe.MAX_BSP_BYTES)
    background = output / f"{name}-background.bsp"
    client.call("start_compile", {"path": str(background)})
    job = wait_job(client)
    require(job["status"] == "completed" and background.read_bytes() == data, job)
    require(job["settings"]["pack_sky_assets"] is expected, job)
    require(snapshot(client, source, ids) == before, "Compilation changed source, world properties, selection or edit history")
    return background, data, result, job


def capture_run(client, output, path, data, family, channels, expected, name):
    launched = client.call("launch_game")
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        log = client.call("game_log")
        if log["status"] != "running":
            break
        time.sleep(0.05)
    require(log["status"] == "exited" and log["exit_code"] == 0, log)
    launch = json.loads((output / "run-launch.json").read_text())
    require(launch["map"] == launched["map"] != path.stem, "Run did not use a fresh private namespace")
    staged = probe.read_bounded(output / "run-staged.bsp", probe.MAX_BSP_BYTES)
    verification = verify_sky(staged, family, channels, expected)
    left, right = pak(data), pak(staged)
    require(left[MANIFEST] == right[MANIFEST], "Run rewrote the packed-sky manifest")
    reflection_report = None
    if reflections.MANIFEST in left:
        original = json.loads(left[reflections.MANIFEST])
        rebased = json.loads(right[reflections.MANIFEST])
        reflections.verify_rebased_assets(data, staged, original, rebased)
        require(rebased["namespace"] == launch["map"] and rebased["logical_namespace"] == path.stem, "Run lost the reflection logical or private namespace")
        reflection_report = {"original_namespace": original["namespace"], "staged_namespace": rebased["namespace"], "captured_textures_preserved": True}
    else:
        require(probe.bsp_lumps(data) == probe.bsp_lumps(staged), "Pack-only Run changed BSP lump content")
    target = output / f"{name}-run.bsp"
    target.write_bytes(staged)
    return {**launch, "bsp": str(target), "verification": verification, "reflection_rebase": reflection_report}


def wait_native_job(client, timeout):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        job = client.call("get_compile_status")["job"]
        if job["status"] not in ["running", "cancelling"]:
            return job
        time.sleep(0.2)
    raise TimeoutError("Native reflection build exceeded its bounded deadline")


def author(args):
    game = runtime.private_game(args.game_dir, args.output / "source-game", engine_physics=True)
    token = "hps_" + uuid.uuid4().hex[:12]
    families = [(token, False), (token + "_dual", True)]
    fixtures = {name: family_fixture(game, name, paired) for name, paired in families}
    reflections.fake_install(game, args.output)
    with patch.dict(os.environ, {
        "HAMMERR_RECENT_MAPS_DIR": str(args.output / "recent-maps"),
        "HAMMERR_BUILD_RECIPES": str(args.output / "build-recipes.json"),
        "HAMMERR_PREFABS_DIR": str(args.output / "prefabs"),
    }):
        client = Client(None, args.binary, args.output, game / "gameinfo.txt")
    ids = []
    try:
        for axis in range(3):
            for sign in [-1, 1]:
                low, high = [-272] * 3, [272] * 3
                low[axis], high[axis] = (-272, -256) if sign < 0 else (256, 272)
                ids.append(client.call("create_block", {"mins": low, "maxs": high, "material": "tools/toolsskybox"})["id"])
        material = "dev/dev_measuregeneric01"
        if args.reflections:
            folder = game / "materials" / token
            folder.mkdir()
            (folder / "receiver.vmt").write_text('LightmappedGeneric { $basetexture dev/dev_measuregeneric01 $envmap env_cubemap $envmaptint "[1 1 1]" }\n')
        ids.append(client.call("create_block", {"mins": [144, -240, -248], "maxs": [248, -136, -232], "material": material})["id"])
        entities = [
            ("info_player_terrorist", [192, -192, -224], {}),
            ("info_player_counterterrorist", [216, -192, -224], {}),
            ("light", [0, 0, 128], {"_light": "255 255 255 200", "_constant_attn": "1", "_linear_attn": "0", "_quadratic_attn": "0"}),
        ]
        if args.reflections:
            entities.append(("env_cubemap", [0, 0, 0], {"cubemapsize": str(probe.SIZE_ENUM)}))
        for classname, origin, properties in entities:
            ids.append(client.call("create_entity", {"classname": classname, "origin": origin, "properties": properties})["id"])
        source = args.output / "packed_sky.vmf"
        require(client.call("get_compile_settings")["pack_sky_assets"] is False, "Packing is not off by default")
        client.call("set_compile_settings", {"lighting": "direct_ldr", "physics": "none", "bounces": 0, "generate_reflections": False})
        set_sky(client, families[0][0])
        client.call("save_map", {"path": str(source)})
        _, omitted, omitted_result, _ = build_pair(client, args.output, "omitted", source, ids, False)
        client.call("set_compile_settings", {"pack_sky_assets": False})
        _, disabled, disabled_result, _ = build_pair(client, args.output, "disabled", source, ids, False)
        require(omitted == disabled and MANIFEST not in pak(disabled), "Default-off and explicit-off BSPs differ")
        require("sky_assets" not in omitted_result["stats"] and "sky_assets" not in disabled_result["stats"], "Disabled packing reports packed asset stats")
        original_assets = asset_snapshot(game)
        cases = []
        for family, paired in families:
            set_sky(client, family)
            client.call("save_map", {"path": str(source)})
            before = snapshot(client, source, ids)
            inspection = client.call("inspect_sky_assets")
            require(inspection["authored_name"] == family and inspection["ldr_ready"] and inspection["hdr_ready"] is paired, inspection)
            client.call("set_compile_settings", {"pack_sky_assets": False})
            reference = args.output / f"{family}-unpacked.bsp"
            unpacked_result = client.call("compile_map", {"path": str(reference)})
            require(unpacked_result["success"] and unpacked_result["leak"] is None, unpacked_result)
            unpacked = probe.read_bounded(reference, probe.MAX_BSP_BYTES)
            require("sky_assets" not in unpacked_result["stats"] and MANIFEST not in pak(unpacked), "Unpacked reference contains packed sky assets")
            require(snapshot(client, source, ids) == before, "Reference compilation changed the frozen source or history")
            runtime_before = client.call("get_native_capture_runtime")
            client.call("set_compile_settings", {"pack_sky_assets": True})
            require(client.call("get_native_capture_runtime") == runtime_before and snapshot(client, source, ids) == before, "Enabling packing changed runtime readiness or source")
            require(not client.call("get_compile_status")["launch"]["ready"], "Previous settings output remained launchable")
            path, data, result, job = build_pair(client, args.output, family, source, ids, True)
            expected = fixtures[family]
            channels = "ldr_hdr" if paired else "ldr"
            verified = verify_sky(data, family, channels, expected)
            require(result["stats"]["sky_assets"] == {"family": family, "channels": channels, "files": len(expected), "asset_bytes": sum(map(len, expected.values()))}, result)
            require(all(old == new for index, (old, new) in enumerate(zip(probe.bsp_lumps(unpacked), probe.bsp_lumps(data), strict=True)) if index != 40), "Packing changed non-PAK output from the same frozen source")
            client.call("set_compile_settings", {"pack_sky_assets": False})
            require(not client.call("get_compile_status")["launch"]["ready"], "Packing setting change did not reject previous output")
            client.call("set_compile_settings", {"pack_sky_assets": True})
            missing = game / f"materials/skybox/{family}up.vmt"
            backup = missing.with_suffix(".vmt-offline")
            missing.rename(backup)
            current = path.read_bytes()
            try:
                failure = client.call("compile_map", {"path": str(path)}, error=True)
                require(not json.loads(failure["content"][0]["text"])["success"] and path.read_bytes() == current, "Incomplete sky replaced the previous output")
                client.call("start_compile", {"path": str(path)})
                failed_job = wait_job(client)
                require(failed_job["status"] == "failed" and path.read_bytes() == current, failed_job)
                require(snapshot(client, source, ids) == before, "Failed packing changed source or history")
            finally:
                backup.rename(missing)
            if paired:
                client.call("set_compile_settings", {"lighting": "direct_ldr_hdr"})
            path, data, result, job = build_pair(client, args.output, family + "-portable", source, ids, True)
            verified = verify_sky(data, family, channels, expected)
            if paired:
                require(result["stats"]["hdr"]["map_data_present"] is True, "Paired native fixture lacks actual HDR map data")
            live = game / "materials/skybox"
            offline = args.output / "offline-sky-sources"
            live.rename(offline)
            try:
                client.call("reload_material_assets")
                require(not client.call("inspect_sky_assets")["ldr_ready"], "Original custom sky source remained mounted")
                readiness = client.call("get_compile_status")["launch"]
                require(readiness["ready"], f"Packed Run still depends on the removed source mount: {readiness}")
                run = capture_run(client, args.output, path, data, family, channels, expected, family)
                entries = pak(data)
                del entries[next(name for name in sorted(expected) if name.endswith(".vtf"))]
                launch_before = (args.output / "run-launch.json").read_bytes()
                try:
                    path.write_bytes(reflections.patch_lump(data, 40, probe.make_zip(entries)))
                    client.call("launch_game", error=True)
                    require((args.output / "run-launch.json").read_bytes() == launch_before, "A missing packed sky asset reached the Run executable")
                finally:
                    path.write_bytes(data)
            finally:
                offline.rename(live)
                client.call("reload_material_assets")
            require(snapshot(client, source, ids) == before and asset_snapshot(game) == original_assets, "Packing or Run changed source, history or original assets")
            cases.append({"family": family, "channels": channels, "bsp": str(path), "published": verified, "compile": result, "job_settings": job["settings"], "inspection": inspection, "unpacked_reference": {"bsp": str(reference), "bsp_sha256": probe.digest(unpacked), "packing_only_non_pak_exact": True}, "run": run, "missing_source_foreground_background_preserves_output": True, "source_mount_removed_and_reloaded_before_run": True, "tampered_output_rejected": True})
        coexistence = None
        if args.reflections:
            ids.append(client.call("create_block", {"mins": [144, 136, -248], "maxs": [248, 240, -232], "material": token + "/receiver"})["id"])
            client.call("save_map", {"path": str(source)})
            require_script_free(source.read_bytes())
            native = client.call("set_native_capture_runtime", {"engine_root": str(args.game_dir), **({"steam_root": str(args.steam_root)} if args.steam_root else {})})
            require(native["ready"], native)
            client.call("set_compile_settings", {"generate_reflections": True})
            path = args.output / "packed_sky_reflections.bsp"
            before = snapshot(client, source, ids)
            client.call("start_compile", {"path": str(path)})
            job = wait_native_job(client, args.timeout)
            require(job["status"] == "completed" and job["result"]["published"], job)
            data = probe.read_bounded(path, probe.MAX_BSP_BYTES)
            require_script_free(probe.bsp_lumps(data)[0][2])
            family = families[-1][0]
            verify_sky(data, family, "ldr_hdr", fixtures[family])
            entries = pak(data)
            reflection_manifest = json.loads(entries[reflections.MANIFEST])
            require(reflection_manifest["phase"] == "verified" and reflection_manifest["channel"] == "ldr_hdr" and reflection_manifest["samples"] and reflection_manifest["hdr_samples"], "Verified paired reflections did not coexist with the sky closure")
            live = game / "materials/skybox"
            offline = args.output / "offline-sky-sources"
            live.rename(offline)
            try:
                run = capture_run(client, args.output, path, data, family, "ldr_hdr", fixtures[family], "coexistence")
            finally:
                offline.rename(live)
            require(snapshot(client, source, ids) == before and asset_snapshot(game) == original_assets, "Combined build changed source or original assets")
            coexistence = {"bsp": str(path), "compile": job["result"], "run": run, "source_mount_removed_before_run": True, "content_epoch_reload_omitted_for_reflection_freshness": True}
        require_script_free(source.read_bytes())
        return {"source": str(source), "source_sha256": probe.digest(source.read_bytes()), "fixture_game": str(game), "original_assets": original_assets, "cases": cases, "coexistence": coexistence, "default_explicit_off_exact": True, "foreground_background_exact": True, "source_history_preserved": True, "script_free_source": True, "native_exercised": False}
    finally:
        try:
            job = client.call("get_compile_status")["job"]
            if job and job["status"] in ["running", "cancelling"]:
                if job["cancellable"]:
                    client.call("cancel_compile", {"token": job["token"]})
                wait_native_job(client, 240)
        finally:
            client.close()


def pixel_evidence(path, index, hdr):
    pixels = ppm(path)
    color = COLORS[(index + 2) % len(COLORS)] if hdr else COLORS[index]
    bright = {axis for axis, value in enumerate(color) if value > 128}
    patches = []
    for x, y in PATCHES:
        values = [pixels[(row * probe.WIDTH + column) * 3 + axis] for row in range(y - 8, y + 8) for column in range(x - 8, x + 8) for axis in range(3)]
        means = [sum(values[axis::3]) / 256 for axis in range(3)]
        require(min(means[axis] for axis in bright) > max((means[axis] for axis in range(3) if axis not in bright), default=0) + 12, f"Wrong {FACES[index]} {'HDR' if hdr else 'LDR'} sky face hue: {means}; expected {color}")
        patches.append({"center": [x, y], "rgb": means, "signal": sum(means[axis] for axis in bright) / len(bright)})
    signals = [row["signal"] for row in patches]
    require(all(right > left + 4 for left, right in zip(signals, signals[1:])), f"Sky {FACES[index]} quadrant orientation or luminance is wrong: {signals}")
    return {"face": FACES[index], "expected_rgb_signature": list(color), "patches": patches, "quadrant_order": "top-left < top-right < bottom-left < bottom-right", "face_and_orientation_verified": True}


def absent_runtime_sources(args, game):
    with patch.dict(os.environ, {
        "HAMMERR_RECENT_MAPS_DIR": str(args.output / "recent-maps"),
        "HAMMERR_BUILD_RECIPES": str(args.output / "build-recipes.json"),
        "HAMMERR_PREFABS_DIR": str(args.output / "prefabs"),
    }):
        client = Client(None, args.binary, args.output, game / "gameinfo.txt")
    try:
        set_sky(client, args.family)
        report = client.call("inspect_sky_assets")
        require(report["issues"] == [] and report["authored_name"] == args.family and report["authored_name_truncated"] is False and report["report_truncated"] is False, f"Runtime source absence requires a usable, complete GameFs inspection: {report}")
        require(not report["ldr_ready"] and not report["hdr_ready"] and len(report["faces"]) == 6 and all(face["source"] is None for face in report["faces"]), "A runtime search path still provides an original custom sky material")
        return {"gameinfo": (game / "gameinfo.txt").read_text(), "before_bsp_mount": report, "all_six_original_materials_absent": True}
    finally:
        client.close()


def native_diagnostics(text, manifest):
    resources = {asset["path"].removeprefix("materials/") for asset in manifest["assets"]}
    resources.update(str(Path(path).with_suffix("")) for path in list(resources))
    resource_pattern = re.compile(r"(?<![a-z0-9_./-])(?:materials/)?(?:" + "|".join(re.escape(path) for path in sorted(resources, key=len, reverse=True)) + r")(?![a-z0-9_./-])", re.IGNORECASE)
    warning_pattern = re.compile(r"\berror\b|\bwarning\b|\bfailed\b|\bmissing\b|\bunable\b|fall\s*back|could(?:n.t| not)|can(?:.t|not)|not (?:found|present)", re.IGNORECASE)
    warnings = sorted({line.strip() for line in text.splitlines() if warning_pattern.search(line)})
    sky = [line for line in warnings if resource_pattern.search(line.replace("\\", "/"))]
    return {"sky_resource_failures": sky, "other_native_warnings": [line for line in warnings if line not in sky]}


def worker(args):
    output = args.output
    require(not os.environ.get("LD_PRELOAD"), "Private sky validation must not inherit LD_PRELOAD")
    require(os.environ.get("WAYLAND_DISPLAY") and os.environ.get("XDG_RUNTIME_DIR") == str(output / "wayland"), "Sky validation requires its private compositor")
    require(re.fullmatch(r"[a-z0-9_-]{1,64}", args.map_name), "Invalid native map basename")
    require(probe.digest((args.game_dir / "bin/linux64/materialsystem.so").read_bytes()) == reflections.MATERIAL_SYSTEM_SHA256, "Material system changed; repeat native HDR interface evidence")
    require(probe.digest(probe.read_bounded(args.game_dir / "cstrike/bin/linux64/server.so", probe.MAX_BSP_BYTES)) == PRESERVATION_SERVER_SHA256, "Server module changed; repeat native observer round-preservation evidence")
    game = runtime.private_game(args.game_dir, output, engine_physics=True)
    require(not (game / "materials").exists(), "Native sky proof must have no custom source material mount")
    source_audit = absent_runtime_sources(args, game)
    data = probe.read_bounded(args.bsp, probe.MAX_BSP_BYTES)
    entries = pak(data)
    manifest = json.loads(entries[MANIFEST])
    require(manifest["family"] == args.family, "Native case family differs from the packed BSP")
    for asset in manifest["assets"]:
        require(asset["path"] in entries and probe.digest(entries[asset["path"]]) == asset["sha256"], "Native input packed asset hash mismatch")
    require_script_free(probe.bsp_lumps(data)[0][2])
    staged = game / "maps" / f"{args.map_name}.bsp"
    staged.write_bytes(data)
    (game / "scripts/vscripts" / (OBSERVER_NAME + ".nut")).write_text(private_observer(OBSERVER))
    mapspawn = observer_mapspawn()
    (game / "scripts/vscripts/mapspawn.nut").write_text(mapspawn)
    cfg = game / "cfg/hammerr_packed_sky_once.cfg"
    probe.write_cfg(cfg, "echo HAMMERR_PACKED_SKY_WAIT\n")
    controls = {"sv_lan": 1, "mat_hdr_level": args.hdr_level, "mat_fullbright": 0, **reflections.HDR_CONTROLS}
    post_map = "toggle sv_lan 1\ntoggle mat_fullbright 0\n" + "".join(f"help {name}\n" for name in controls)
    for name in ["server.cfg", "listenserver.cfg", "hammerr_packed_sky_postmap.cfg"]:
        (game / "cfg" / name).write_text(post_map)
    settings = output / "settings"
    for name in ["config", "cache", "data"]:
        (settings / name).mkdir(parents=True)
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    ready = output / "probe-ready"
    hardware_log = output / "hardware.jsonl"
    environment = {
        **os.environ, "LD_LIBRARY_PATH": str(args.game_dir / "bin/linux64"),
        "SteamAppId": "240", "SteamGameId": "240", "SDL_VIDEODRIVER": "x11",
        "XDG_CONFIG_HOME": str(settings / "config"), "XDG_CACHE_HOME": str(settings / "cache"),
        "XDG_DATA_HOME": str(settings / "data"), "LD_PRELOAD": str(args.probe_library),
        "HAMMERR_HDR_EXPECTED_EXE": str((args.game_dir / "cstrike_linux64").resolve(strict=True)),
        "HAMMERR_HDR_LIBRARY_DIR": str((args.game_dir / "bin/linux64").resolve(strict=True)),
        "HAMMERR_HDR_PRIVATE_GAME": str(game), "HAMMERR_HDR_READY": str(ready),
        "HAMMERR_HDR_REPORT": str(hardware_log),
    }
    flags = private_launch_options(args.game_dir, game, hdr_level=args.hdr_level, map_name=args.map_name, log_name="hammerr-packed-sky.log", controls={"servercfgfile": "hammerr_packed_sky_postmap.cfg", "mat_fullbright": 0, **reflections.HDR_CONTROLS})
    command = [str(args.game_dir / "cstrike_linux64"), "-game", str(game), "-windowed", "-w", str(probe.WIDTH), "-h", str(probe.HEIGHT), "-novid", *flags, "-nomaster", "-nojoy", "-console", "-port", str(args.port)]
    report = {"success": False, "map": args.map_name, "family": args.family, "hdr_level": args.hdr_level, "bsp_sha256": probe.digest(data), "probe_sha256": probe.digest(args.probe_library.read_bytes()), "original_custom_source_mounted": False, "runtime_source_audit": source_audit, "runtime_observer": {"injection": "private mapspawn SpawnEntityFromTable", "class": OBSERVER_CLASS, "round_preservation": {"server_sha256": PRESERVATION_SERVER_SHA256, "table_file_offset": "0xce2ce0", "table_elf_vaddr": "0xce3ce0", "cleanup_elf_vaddr": "0x817170"}, "scheduling": "one player_spawn callback AddThinkToEnt", "mapspawn_sha256": probe.digest(mapspawn.encode()), "script_sha256": probe.digest((game / "scripts/vscripts" / (OBSERVER_NAME + ".nut")).read_bytes()), "bsp_entity_lump_modified": False, "creation_marker": RUNTIME_OBSERVER_READY, "listener_marker": RUNTIME_OBSERVER_REGISTERED, "callback_marker": RUNTIME_OBSERVER_EVENT, "callback_userids": [], "scheduling_marker": RUNTIME_OBSERVER_SCHEDULED, "verified_created": False, "verified_listener": False, "verified_scheduled": False}, "script_free_bsp": True, "staged_bsp_bytes_exact": True, "published_bsp_loaded_exact": False, "desktop_input": False, "archive_verified": True, "native_exercised": False, "faces": [], "hardware": None}
    console = game / "hammerr-packed-sky.log"
    try:
        with private_native_process(command, args.game_dir, game, output, environment, report) as process:
            started = time.monotonic()
            captures = report["faces"]
            hardware = None
            while time.monotonic() - started < 110:
                text = probe.read_bounded(console, 2 * 1024**2).decode(errors="replace") if console.exists() else ""
                report["native_diagnostics"] = native_diagnostics(text, manifest)
                report["runtime_observer"]["verified_created"] = RUNTIME_OBSERVER_READY in text
                report["runtime_observer"]["verified_listener"] = RUNTIME_OBSERVER_REGISTERED in text
                report["runtime_observer"]["callback_userids"] = [int(value) for value in re.findall(r"^" + RUNTIME_OBSERVER_EVENT + r" ([0-9]+)\s*$", text, re.MULTILINE)]
                report["runtime_observer"]["verified_scheduled"] = RUNTIME_OBSERVER_SCHEDULED in text and RUNTIME_OBSERVER_READY in text and text.index(RUNTIME_OBSERVER_READY) < text.index(RUNTIME_OBSERVER_SCHEDULED)
                require(RUNTIME_OBSERVER_MISSING not in text, "Trusted player_spawn callback could not find the runtime observer controller")
                marker = f"HAMMERR_PACKED_SKY_FACE {len(captures)} 2 100"
                if not ready.exists() and "HAMMERR_PACKED_SKY_FACE 0 2 100" in text:
                    ready.write_text("post-map sky view ready\n")
                report["hardware_observations"] = read_probe(hardware_log)
                hardware = reflections.verify_hardware(report["hardware_observations"], args.hdr_level)
                if hardware:
                    report["hardware"] = hardware
                    report["native_exercised"] = True
                if len(captures) < 6 and marker in text and hardware:
                    report["private_launch"] = verify_private_launch(game, text)
                    require(RUNTIME_OBSERVER_READY in text and text.index(RUNTIME_OBSERVER_READY) < text.index("HAMMERR_PACKED_SKY_FACE 0 2 100"), "Trusted runtime observer creation was not observed before the first view")
                    require(RUNTIME_OBSERVER_REGISTERED in text and RUNTIME_OBSERVER_SCHEDULED in text and len(report["runtime_observer"]["callback_userids"]) == 1 and text.index(RUNTIME_OBSERVER_READY) < text.index(RUNTIME_OBSERVER_REGISTERED) < text.index(RUNTIME_OBSERVER_EVENT) < text.index(RUNTIME_OBSERVER_SCHEDULED), "One registered player_spawn callback with a userid was not observed between creation and scheduling")
                    require(RUNTIME_OBSERVER_SCHEDULED in text and text.index(RUNTIME_OBSERVER_READY) < text.index(RUNTIME_OBSERVER_SCHEDULED) < text.index("HAMMERR_PACKED_SKY_FACE 0 2 100"), "Trusted runtime observer scheduling was not observed after creation and before the first view")
                    report["runtime_observer"]["verified_created"] = True
                    report["runtime_observer"]["verified_scheduled"] = True
                    index = len(captures)
                    for extension in ["png", "ppm"]:
                        subprocess.run(["grim", *(["-t", "ppm"] if extension == "ppm" else []), str(output / f"{FACES[index]}.{extension}")], check=True, timeout=5)
                    captures.append(pixel_evidence(output / f"{FACES[index]}.ppm", index, args.hdr_level == 2))
                    report["published_bsp_loaded_exact"] = True
                    if len(captures) == 6:
                        probe.write_cfg(cfg, f"echo {FINISHED}\ndisconnect\nquit\n")
                    else:
                        pitch, yaw = ANGLES[len(captures)]
                        probe.write_cfg(cfg, f'setang {pitch} {yaw} 0\nent_fire {OBSERVER_NAME} RunScriptCode "view = {len(captures)}; stage = 2; ready_at = Time() + 1.5"\n')
                if process.poll() is not None:
                    break
                time.sleep(0.02)
            else:
                raise TimeoutError("Native six-face sky handshake exceeded 110 seconds")
            text = probe.read_bounded(console, 2 * 1024**2).decode(errors="replace")
            report["native_diagnostics"] = native_diagnostics(text, manifest)
            require(process.returncode == 0 and len(captures) == 6 and FINISHED in text, f"Native sky view failed; inspect {console}")
            positions = [text.index(f"HAMMERR_PACKED_SKY_FACE {index} 2 100") for index in range(6)]
            require(positions == sorted(positions) and positions[-1] < text.index(FINISHED), "Sky frames were taken outside the ordered handshake")
            for name, expected in controls.items():
                values = re.findall(r'"' + re.escape(name) + r'" = "([^"\n]+)"', text)
                require(values and float(values[-1]) == expected, f"Native {name} control differs from proof: {values}")
            require(not report["native_diagnostics"]["sky_resource_failures"], f'Engine reported a packed sky resource failure: {report["native_diagnostics"]["sky_resource_failures"]}')
            require(staged.read_bytes() == data and probe.digest(args.bsp.read_bytes()) == report["bsp_sha256"], "Native proof changed the published or staged BSP")
            report.update({"success": True, "hardware": hardware, "controls": controls, "faces": captures, "elapsed_seconds": time.monotonic() - started})
    except BaseException as error:
        report.update({"success": False, "error": str(error), "traceback": traceback.format_exc()})
        raise
    finally:
        (output / "runtime.json").write_text(json.dumps(report, indent=2) + "\n")


def native_views(args, fixture, report):
    views = report["views"]
    cases = fixture["cases"]
    requests = [
        ("published-ldr", cases[0]["bsp"], Path(cases[0]["bsp"]).stem, cases[0]["family"], 0),
        ("published-dual-ldr", cases[1]["bsp"], Path(cases[1]["bsp"]).stem, cases[1]["family"], 0),
        ("published-dual-hdr", cases[1]["bsp"], Path(cases[1]["bsp"]).stem, cases[1]["family"], 2),
        ("run-dual-hdr", cases[1]["run"]["bsp"], cases[1]["run"]["map"], cases[1]["family"], 2),
    ]
    if fixture["coexistence"]:
        combined = fixture["coexistence"]
        requests.append(("run-reflections-hdr", combined["run"]["bsp"], combined["run"]["map"], cases[1]["family"], 2))
    available = {row[0] for row in requests}
    selected = set(args.native_case) if args.native_case else available
    require(selected <= available, f"Native cases are unavailable in this fixture: {sorted(selected - available)}")
    requests = [row for row in requests if row[0] in selected]
    report["requested_native_cases"] = [row[0] for row in requests]
    for index, (name, bsp, namespace, family, level) in enumerate(requests):
        output = args.output / name
        command = [sys.executable, "-B", str(Path(__file__).resolve()), "--worker", "--binary", str(args.binary), "--game-dir", str(args.game_dir), "--output", str(output), "--bsp", bsp, "--map-name", namespace, "--family", family, "--hdr-level", str(level), "--port", str(args.port + index), "--probe-library", str(args.probe_library)]
        report["executed_native_cases"].append(name)
        try:
            runtime.private_compositor(command, output, timeout=145)
        finally:
            if (output / "runtime.json").is_file():
                views[name] = json.loads((output / "runtime.json").read_text())
        require(views[name]["success"], views[name])
        report["passed_native_cases"].append(name)
    require(len({view["hardware"]["native_pid"] for view in views.values()}) == len(views), "Native sky cases reused a process")
    return views


def main():
    parser = argparse.ArgumentParser(description="Validate portable raw sky packing through shared MCP compiles, guarded Run, and optional fresh private native six-face LDR/HDR views.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--game-dir", type=Path, default=Path.home() / ".local/share/Steam/steamapps/common/Counter-Strike Source")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--runtime", action="store_true")
    parser.add_argument("--native-case", action="append", choices=NATIVE_CASES, help="Run only this named native case; repeat to select several (default: all available).")
    parser.add_argument("--reflections", action="store_true")
    parser.add_argument("--steam-root", type=Path)
    parser.add_argument("--timeout", type=int, default=600)
    parser.add_argument("--port", type=int, default=27723)
    parser.add_argument("--probe-library", type=Path, default=DEFAULT_BINARY.parent / "hammerr_hdr_probe.so")
    parser.add_argument("--reuse-fixture", type=Path)
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--bsp", type=Path, help=argparse.SUPPRESS)
    parser.add_argument("--map-name", help=argparse.SUPPRESS)
    parser.add_argument("--family", help=argparse.SUPPRESS)
    parser.add_argument("--hdr-level", type=int, choices=[0, 2], default=0, help=argparse.SUPPRESS)
    args = parser.parse_args()
    args.output = args.output.resolve()
    args.binary = args.binary.resolve(strict=True)
    args.game_dir = args.game_dir.resolve(strict=True)
    require(60 <= args.timeout <= 900 and 1024 <= args.port <= 65525, "Invalid validation deadline or port")
    require(not args.reflections or args.runtime, "Reflection generation requires explicit --runtime")
    require(not args.native_case or args.runtime, "Native case selection requires explicit --runtime")
    require(not args.native_case or "run-reflections-hdr" not in args.native_case or args.reflections, "The reflection native case requires explicit --reflections")
    if args.runtime or args.worker:
        args.probe_library = args.probe_library.resolve(strict=True)
    if args.worker:
        require(args.bsp and args.map_name and args.family, "Native sky worker arguments are incomplete")
        worker(args)
        return
    require(not args.output.exists(), "Use a fresh absent output directory with a short path for native sockets")
    require(not os.environ.get("LD_PRELOAD"), "Sky validation must not inherit LD_PRELOAD")
    args.output.mkdir(parents=True, mode=0o700)
    before = reflections.protected_files(args, [])
    report = {"success": False, "native_exercised": False, "desktop_input": False, "native_case_scope": "selected" if args.native_case else "all_available", "requested_native_cases": args.native_case or [], "executed_native_cases": [], "passed_native_cases": [], "views": {}}
    try:
        if args.reuse_fixture:
            fixture = json.loads(args.reuse_fixture.read_text())
            for case in fixture["cases"]:
                require(probe.digest(Path(case["bsp"]).read_bytes()) == case["published"]["bsp_sha256"], "Reused published BSP changed")
                require(probe.digest(Path(case["run"]["bsp"]).read_bytes()) == case["run"]["verification"]["bsp_sha256"], "Reused Run BSP changed")
        else:
            fixture = author(args)
        report["fixture"] = fixture
        (args.output / "fixture.json").write_text(json.dumps(fixture, indent=2) + "\n")
        preserved = asset_snapshot(Path(fixture["fixture_game"]))
        require(preserved == fixture["original_assets"] and probe.digest(Path(fixture["source"]).read_bytes()) == fixture["source_sha256"], "Fixture source changed")
        if args.runtime:
            native_views(args, fixture, report)
            report["native_exercised"] = True
        require(asset_snapshot(Path(fixture["fixture_game"])) == preserved and probe.digest(Path(fixture["source"]).read_bytes()) == fixture["source_sha256"], "Validation changed original fixture assets or VMF")
        report.update({"success": True, "archive_verified": True, "source_and_installed_assets_preserved": True})
    except BaseException as error:
        report.update({"success": False, "error": str(error), "traceback": traceback.format_exc()})
        raise
    finally:
        report["native_exercised"] = any(view.get("native_exercised", False) for view in report["views"].values())
        report["installed_assets_preserved"] = reflections.protected_files(args, []) == before
        if not report["installed_assets_preserved"]:
            report["success"] = False
        (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
        require(report["installed_assets_preserved"], "Validation changed installed configuration or engine files")
    print(json.dumps({"success": True, "native_exercised": report["native_exercised"], "results": str(args.output / "results.json")}, indent=2))


if __name__ == "__main__":
    main()
