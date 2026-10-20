import argparse
import hashlib
import json
import math
import os
import re
import resource
import shutil
import struct
import subprocess
import sys
import time
from pathlib import Path
from unittest.mock import patch

import validate_cubemap_capture_probe as probe
import validate_hdr_lighting as hdr
import validate_native_runtime as runtime
from hdr_observer import ENGINE_SHA256, probe_environment, read_probe, verify_hardware
from reflection_validation import difference, ppm
from validate_build_recipes import protected_state
from validate_bump_lighting import texture
from validate_compile_jobs import stopped_on_exit, wait_job
from validate_face_lighting import DEFAULT_BINARY, ROOT, Client, detail
from validate_recovery_exit import require_clean_eof
from validate_visibility import ROOM

VALUES = {"low": "0.375", "high": "1.625"}
CONTROLLER = "hammerr_tonemap"
MATERIAL = "hammerr/tonemap_white"
ROI = (320, 400, 960, 640)
CONTROLS = {"mat_force_tonemap_scale": 0, "mat_autoexposure_min": 0.75, "mat_autoexposure_max": 0.75,
            "mat_dynamic_tonemapping": 1, "mat_hdr_uncapexposure": 0, "mat_disable_bloom": 1,
            "mat_bloomscale": 0, "mat_fullbright": 0, "sv_lan": 1}
SERVER_SHA256 = "7887c8cc24a3dec6b8739bfebcad48c00b95423ebcbfc647f49fef1ab904dbe2"
CLIENT_SHA256 = "cd2a89e99dc430a2f1a0687e88c456e535d535a53ac05a28b19f85585f6224ab"
GATES = {"region": ROI, "minimum_blue_ratio": 1.25, "minimum_blue_difference": 8,
         "minimum_blue_dominance": 20, "minimum_blue_dominant_fraction": 0.8,
         "maximum_same_case_mean_component_difference": 1.5,
         "maximum_restored_mean_component_difference": 2,
         "maximum_settled_readback_error": 0.001, "capture_interval_seconds": 1}
OBSERVER = r'''
stage <- 0;
ready_at <- 0.0;
after_reported <- false;
after_at <- 0.0;
function HammeRRTonemapReport(label) {
    if (!("NetProps" in getroottable())) { printl("HAMMERR_TONEMAP_ERROR missing_NetProps"); return false; }
    local controller = Entities.FindByClassname(null, "env_tonemap_controller");
    if (controller == null || controller.GetName() != "hammerr_tonemap" || Entities.FindByClassname(controller, "env_tonemap_controller") != null) {
        printl("HAMMERR_TONEMAP_ERROR controller_identity_or_count"); return false;
    }
    local names = ["m_bUseCustomAutoExposureMin", "m_bUseCustomAutoExposureMax", "m_flCustomAutoExposureMin", "m_flCustomAutoExposureMax"];
    foreach (index, name in names) {
        local expected = index < 2 ? "bool" : "float";
        if (!NetProps.HasProp(controller, name) || NetProps.GetPropType(controller, name) != expected) {
            printl("HAMMERR_TONEMAP_ERROR missing_or_wrong_type_" + name); return false;
        }
    }
    printl("HAMMERR_TONEMAP_" + label + " " + Time() + " " + (NetProps.GetPropBool(controller, names[0]) ? 1 : 0) + " " + (NetProps.GetPropBool(controller, names[1]) ? 1 : 0) + " " + NetProps.GetPropFloat(controller, names[2]) + " " + NetProps.GetPropFloat(controller, names[3]));
    return true;
}
function HammeRRThink() {
    if (stage == 0) {
        if (!HammeRRTonemapReport("BEFORE")) return 0.1;
        after_at = Time() + 2.0;
        stage = 1;
    }
    if (!after_reported && stage >= 1 && Time() >= after_at) after_reported = HammeRRTonemapReport("AFTER");
    local player = Entities.FindByClassname(null, "player");
    if (player == null) return 0.1;
    if (stage == 1) {
        SendToConsole("sv_cheats 1; sv_lan 1; joystick 0; bot_quota 0; bot_kick; jointeam 2; joinclass 1; hidepanel info; hidepanel team; hidepanel class_ter; hidepanel class_ct; r_drawvgui 0; gameui_hide; cl_drawhud 0; r_drawviewmodel 0; r_drawothermodels 0; mat_fullbright 0; mat_force_tonemap_scale 0; mat_autoexposure_min 0.75; mat_autoexposure_max 0.75; mat_dynamic_tonemapping 1; mat_hdr_uncapexposure 0; mat_disable_bloom 1; mat_bloomscale 0");
        ready_at = Time() + 6.0;
        stage = 2;
    }
    if (stage == 2 && Time() > ready_at && player.GetTeam() == 2 && player.GetHealth() == 100) {
        SendToConsole("noclip; setpos -192 -176 48; setang 35 42 0");
        ready_at = Time() + 4.0;
        stage = 3;
    }
    if (stage == 3 && Time() > ready_at && after_reported) {
        if (!HammeRRTonemapReport("SETTLED")) return 0.1;
        local origin = player.GetOrigin();
        local angles = player.EyeAngles();
        printl("HAMMERR_TONEMAP_CAMERA " + origin.x + " " + origin.y + " " + origin.z + " " + angles.x + " " + angles.y + " " + angles.z);
        SendToConsole("mat_hdr_level; mat_force_tonemap_scale; mat_autoexposure_min; mat_autoexposure_max; mat_dynamic_tonemapping; mat_hdr_uncapexposure; mat_disable_bloom; mat_bloomscale; mat_fullbright; sv_lan; help mat_hdr_tonemapscale; echo HAMMERR_TONEMAP_READY " + player.GetTeam() + " " + player.GetHealth());
        stage = 4;
    }
    return 0.1;
}
'''


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def identity(path):
    hasher = hashlib.sha256()
    with path.open("rb") as stream:
        while block := stream.read(1024 * 1024):
            hasher.update(block)
    return {"bytes": path.stat().st_size, "sha256": hasher.hexdigest()}


def material_identities(game):
    return {name: identity(game / f"materials/hammerr/{name}") for name in ("tonemap_white.vmt", "tonemap_white.vtf")}


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def snapshot(client, source, ids):
    return {"source": identity(source), "state": protected_state(client), "objects": detail(client, ids),
            "history": client.call("get_history"), "recipes": client.call("get_build_recipes")}


def connection(client, entity, input_name, parameter, index=None, cancel=False):
    arguments = {"id": entity, **({"index": index} if index is not None else {})}
    token = client.call("begin_connection_edit", arguments)["token"]
    result = client.call("update_connection_draft", {"token": token, "output": "OnMapSpawn", "target": CONTROLLER,
                         "input": input_name, "parameter": parameter, "delay": "1", "times": "1"})
    require(result["error"] is None, f"Shared float connection was rejected: {result}")
    client.call("cancel_connection_edit" if cancel else "commit_connection_edit", {"token": token})


def entity_records(data):
    payload = probe.bsp_lumps(data)[0][2]
    require(0 < len(payload) <= 65536 and payload.endswith(b"\0"), "Entity lump exceeds bounded fixture scope")
    text = payload[:-1].decode("utf-8")
    blocks = re.findall(r"\{\s*([^{}]*)\}", text)
    require(len(blocks) <= 16 and re.sub(r"\{\s*[^{}]*\}", "", text).strip() == "", "Unexpected entity syntax")
    records = []
    for block in blocks:
        pairs = re.findall(r'"([^"\r\n]*)"\s*"([^"\r\n]*)"', block)
        require(re.sub(r'"[^"\r\n]*"\s*"[^"\r\n]*"', "", block).strip() == "", "Unexpected entity field syntax")
        records.append(pairs)
    return records


def check_bsp(data, value):
    records = entity_records(data)
    controllers = [record for record in records if ("classname", "env_tonemap_controller") in record]
    autos = [record for record in records if ("classname", "logic_auto") in record]
    require(len(controllers) == len(autos) == 1, "Expected exactly one controller and one logic_auto")
    controller, auto = controllers[0], autos[0]
    require(("targetname", CONTROLLER) in controller and not any("exposure" in key.lower() for key, unused in controller),
            "Controller must be named and initialized only by authored float inputs")
    outputs = [(key, content) for key, content in auto if key.startswith("On")]
    expected = [("OnMapSpawn", f"{CONTROLLER},{name},{value},1,1") for name in ("SetAutoExposureMin", "SetAutoExposureMax")]
    require(outputs == expected, f"Compiled delayed float parameter/delay/count differs: {outputs}")
    for channel, axis in (("ldr", 0), ("hdr", 2)):
        decoded = hdr.decode_channel(data, channel)
        require(decoded["styles"] == [0] and decoded["ambient_maximum"][axis] > 0
                and all(light[2 - axis] == 0 for light in decoded["world_intensities"]), "Native channel fixture differs")
    return {"entities": records, "controller_outputs": outputs, "bsp_sha256": probe.digest(data)}


def compile_case(client, args, source, ids, name, value):
    before = snapshot(client, source, ids)
    output = args.output / f"{name}.bsp"
    result = client.call("compile_map", {"path": str(output)})
    require(result["success"] and result["bsp"] == str(output) and result["leak"] is None, result)
    foreground = client.call("get_compile_status")["job"]
    require(foreground["status"] == "completed" and foreground["result"]["published"], foreground)
    data = probe.read_bounded(output, probe.MAX_BSP_BYTES)
    checked = check_bsp(data, value)
    background = args.output / f"{name}-background.bsp"
    submitted = client.call("start_compile", {"path": str(background)})["job"]
    job = wait_job(client)
    require(job["token"] == submitted["token"] and job["status"] == "completed" and job["result"]["published"]
            and not job["stale"] and background.read_bytes() == data, "Foreground/background publication differs")
    require(snapshot(client, source, ids) == before, "Compile changed source, history, selection, camera or recipes")
    return output, {**checked, "source": identity(source), "settings": client.call("get_compile_settings"),
                    "stats": result["stats"], "foreground_background_identical": True, "protected_state_unchanged": True}


def fixture(args):
    game = runtime.private_game(args.game_dir, args.output / "source-game", engine_physics=True)
    directory = game / "materials/hammerr"
    directory.mkdir(parents=True, exist_ok=True)
    (directory / "tonemap_white.vtf").write_bytes(texture(bytes([255] * 32 * 32 * 4)))
    (directory / "tonemap_white.vmt").write_text(f'LightmappedGeneric {{ $basetexture "{MATERIAL}" }}\n')
    material_inputs = material_identities(game)
    environment = {"HAMMERR_RECENT_MAPS_DIR": str(args.output / "recent"), "HAMMERR_BUILD_RECIPES": str(args.output / "recipes.json"),
                   "HAMMERR_PREFABS_DIR": str(args.output / "prefabs"), "XDG_CONFIG_HOME": str(args.output / "config"),
                   "XDG_STATE_HOME": str(args.output / "state")}
    with patch.dict(os.environ, environment):
        client = Client(None, args.binary, args.output, game / "gameinfo.txt")
    try:
        with stopped_on_exit(client):
            ids = [client.call("create_block", {"mins": low, "maxs": high, "material": MATERIAL})["id"] for low, high in ROOM]
            for classname, origin, properties in [
                ("info_player_terrorist", [-192, -176, 8], {}),
                ("info_player_counterterrorist", [192, 176, 8], {}),
                ("light", [0, 0, 180], {"_light": "255 0 0 40", "_lightHDR": "0 0 255 160", "_constant_attn": "1", "_linear_attn": "0", "_quadratic_attn": "0"}),
                ("env_tonemap_controller", [0, 0, 64], {"targetname": CONTROLLER}),
                ("logic_auto", [0, 0, 80], {"targetname": "hammerr_tonemap_spawn", "spawnflags": "1"}),
                ("logic_script", [-192, -176, 64], {"targetname": "hammerr_observer", "vscripts": "hammerr_observer.nut", "thinkfunction": "HammeRRThink"}),
            ]:
                entity = client.call("create_entity", {"classname": classname, "origin": origin, "properties": properties})["id"]
                ids.append(entity)
                if classname == "logic_auto":
                    auto = entity
            choices = client.call("get_connection_choices", {"id": auto, "target": CONTROLLER})
            inputs = {row["name"]: row["type"] for row in choices["inputs"]}
            require(all(inputs.get(name) == "float" for name in ("SetAutoExposureMin", "SetAutoExposureMax"))
                    and any(row["name"] == "OnMapSpawn" for row in choices["outputs"]), "Loaded FGD lacks the exact float min/max recipe")
            for name in ("SetAutoExposureMin", "SetAutoExposureMax"):
                connection(client, auto, name, VALUES["low"])
            client.call("set_compile_settings", {"lighting": "direct_ldr_hdr", "physics": "none", "bounces": 0, "sky_quality": "fast"})
            seed = args.output / "seed.vmf"
            client.call("save_map", {"path": str(seed)})
            seed_identity = identity(seed)
            client.call("open_map", {"path": str(seed)})
            source = args.output / "low.vmf"
            client.call("save_map", {"path": str(source)})
            before = snapshot(client, source, ids)
            connection(client, auto, "SetAutoExposureMin", VALUES["high"], index=0, cancel=True)
            require(snapshot(client, source, ids) == before, "Cancel changed the map or history")
            low, low_report = compile_case(client, args, source, ids, "low", VALUES["low"])
            client.call("open_map", {"path": str(seed)})
            for index, name in enumerate(("SetAutoExposureMin", "SetAutoExposureMax")):
                connection(client, auto, name, VALUES["high"], index=index)
            high_source = args.output / "high.vmf"
            client.call("save_map", {"path": str(high_source)})
            high, high_report = compile_case(client, args, high_source, ids, "high", VALUES["high"])
            low_data, high_data = low.read_bytes(), high.read_bytes()
            low_lumps, high_lumps = probe.bsp_lumps(low_data), probe.bsp_lumps(high_data)
            require(all(left == right for index, (left, right) in enumerate(zip(low_lumps, high_lumps, strict=True)) if index != 0),
                    "Changing the controller recipe changed a non-entity native lump")
            offset, size = struct.unpack_from("<2i", low_data, 8)
            require(low_data[:offset] == high_data[:offset] and low_data[offset + size:] == high_data[offset + size:],
                    "Non-entity BSP bytes changed between equal-length exposure parameters")
            expected_high = high_lumps[0][2]
            for name in ("SetAutoExposureMin", "SetAutoExposureMax"):
                expected_high = expected_high.replace(f"{CONTROLLER},{name},{VALUES['high']},1,1".encode(),
                                                      f"{CONTROLLER},{name},{VALUES['low']},1,1".encode())
            require(expected_high == low_lumps[0][2], "Entity changes extend beyond the two exact exposure parameters")
            client.call("open_map", {"path": str(source)})
            restored, restored_report = compile_case(client, args, source, ids, "restored", VALUES["low"])
            require(restored.read_bytes() == low_data, "Save/reopen did not restore exact low-case BSP bytes")
            require(identity(seed) == seed_identity, "Saved fixture seed changed")
            require(material_identities(game) == material_inputs, "Compilation changed source material bytes")
            report = {"cases": {"low": low_report, "high": high_report, "restored": restored_report},
                      "non_entity_bsp_bytes_identical": True, "restored_low_bytes_identical": True,
                      "fixture_game": str(game), "material_inputs": material_inputs,
                      "same_save_count_seed": seed_identity,
                      "loaded_float_input_signatures": inputs, "native_gates_predeclared": GATES}
            write_json(args.output / "compile.json", report)
        require_clean_eof(client)
        return {"low": low, "high": high, "restored": restored}, game, report
    finally:
        client.close()


def observed(text, expected):
    require("HAMMERR_TONEMAP_ERROR" not in text and "SCRIPT ERROR" not in text and "Level unlit" not in text,
            "Observer binding/property or native lighting failure; inspect full console")
    rows = {}
    for label in ("BEFORE", "AFTER", "SETTLED"):
        matches = re.findall(r"HAMMERR_TONEMAP_" + label + r" ([\d.eE+-]+) ([01]) ([01]) ([\d.eE+-]+) ([\d.eE+-]+)", text)
        require(len(matches) == 1, f"Missing/duplicate public controller observation: {label}")
        rows[label] = [float(value) for value in matches[0]]
        require(all(math.isfinite(value) for value in rows[label]), "Nonfinite public controller observation")
    require(rows["BEFORE"][1:3] == [0, 0], "Authored delayed inputs were already active before observation")
    require(rows["AFTER"][0] >= rows["BEFORE"][0] + 1 and rows["AFTER"][0] < rows["SETTLED"][0], "Controller observations are out of order")
    for label in ("AFTER", "SETTLED"):
        require(rows[label][1:] == [1, 1, expected, expected], "Actual controller custom flags/floats differ from the authored exact fraction")
    controls = {}
    for name, value in {**CONTROLS, "mat_hdr_level": 2}.items():
        found = re.findall(r'"' + re.escape(name) + r'" = "([^"]+)"', text)
        require(found and math.isfinite(float(found[-1])) and float(found[-1]) == value, f"Queried native control differs: {name}={found}")
        controls[name] = float(found[-1])
    scale = re.findall(r'"mat_hdr_tonemapscale" = "([^"]+)"', text)
    require(scale and math.isfinite(float(scale[-1])) and abs(float(scale[-1]) - expected) <= GATES["maximum_settled_readback_error"],
            f"Settled client console scale does not match fixed controller bounds: {scale}")
    camera = re.findall(r"HAMMERR_TONEMAP_CAMERA ([\d.eE+-]+) ([\d.eE+-]+) ([\d.eE+-]+) ([\d.eE+-]+) ([\d.eE+-]+) ([\d.eE+-]+)", text)
    require(len(camera) == 1 and all(math.isfinite(float(value)) and abs(float(value) - wanted) <= 0.01
            for value, wanted in zip(camera[0], (-192, -176, 48, 35, 42, 0), strict=True)), f"Actual fixed camera differs: {camera}")
    require("HAMMERR_TONEMAP_READY 2 100" in text, "Local player was not ready at the fixed camera")
    return {"server_public_netprops": rows, "queried_console_controls": controls,
            "settled_client_console_tonemapscale": float(scale[-1]), "camera": list(map(float, camera[0]))}


def worker(args):
    root, output = args.game_dir, args.output
    require(not os.environ.get("LD_PRELOAD") and os.environ.get("WAYLAND_DISPLAY")
            and os.environ.get("XDG_RUNTIME_DIR") == str(output / "wayland"), "Worker requires the isolated private compositor")
    require(identity(root / "bin/linux64/engine.so")["sha256"] == ENGINE_SHA256, "Pinned engine identity changed")
    require(identity(root / "cstrike/bin/linux64/server.so")["sha256"] == SERVER_SHA256, "Pinned server identity changed")
    require(identity(root / "cstrike/bin/linux64/client.so")["sha256"] == CLIENT_SHA256, "Pinned client identity changed")
    source_identity = identity(args.bsp)
    material_inputs = material_identities(args.fixture_game)
    probe_identity = identity(args.probe_library)
    check_bsp(probe.read_bounded(args.bsp, probe.MAX_BSP_BYTES), args.value)
    game = runtime.private_game(root, output, args.bsp)
    shutil.copytree(args.fixture_game / "materials", game / "materials", dirs_exist_ok=True)
    (game / "scripts/vscripts/hammerr_observer.nut").write_text(runtime.private_observer(OBSERVER))
    server_config = "hammerr_tonemap_server.cfg"
    (game / "cfg" / server_config).write_text("toggle sv_lan 1\nhelp sv_lan\n")
    options = runtime.private_launch_options(root, game, hdr_level=2, controls={**CONTROLS, "servercfgfile": server_config})
    ready, probe_log = output / "probe-ready", output / "hardware.jsonl"
    environment = {**os.environ, "LD_LIBRARY_PATH": str(root / "bin/linux64"), "SteamAppId": "240", "SteamGameId": "240",
                   "SDL_VIDEODRIVER": "x11", **probe_environment(root, game, args.probe_library, ready, probe_log)}
    command = [str(root / "cstrike_linux64"), "-game", str(game), "-windowed", "-w", "1280", "-h", "720", "-novid",
               *options, "-nomaster", "-nojoy", "-console", "-port", "27589"]
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    result = {"success": False, "predeclared_gates": GATES, "bsp_input": source_identity,
              "material_inputs": material_inputs, "probe_library": probe_identity}
    try:
        with runtime.private_native_process(command, root, game, output, environment, result) as process:
            deadline = time.monotonic() + 48
            while time.monotonic() < deadline:
                require(process.poll() is None, f"Private CS:S exited: {process.returncode}")
                console = game / "hammerr-runtime.log"
                text = console.read_text(errors="replace") if console.exists() else ""
                require("HAMMERR_TONEMAP_ERROR" not in text and "SCRIPT ERROR" not in text, "Public observer failed; inspect console")
                if not ready.exists() and "HAMMERR_TONEMAP_READY 2 100" in text:
                    result.update(observed(text, float(args.value)))
                    ready.write_text("Actual public controller state, camera and queried controls verified\n")
                records = read_probe(probe_log)
                hardware = verify_hardware(records, 2)
                if hardware is not None:
                    break
                time.sleep(0.1)
            else:
                raise TimeoutError("Private tonemap observation did not settle within 48 seconds")
            for name in ("a", "b"):
                subprocess.run(["grim", "-t", "ppm", str(output / f"game-{name}.ppm")], check=True, timeout=10)
                if name == "a":
                    subprocess.run(["grim", str(output / "game.png")], check=True, timeout=10)
                    time.sleep(GATES["capture_interval_seconds"])
            text = (game / "hammerr-runtime.log").read_text(errors="replace")
            result.update(observed(text, float(args.value)))
            result.update({"hardware_hdr": hardware, "probe_records": records, "private_launch": runtime.verify_private_launch(game, text),
                           "pid": process.pid, "insecure": True, "post_map_sv_lan": True, "desktop_input": False,
                           "engine_sha256": ENGINE_SHA256, "server_sha256": SERVER_SHA256, "client_sha256": CLIENT_SHA256,
                           "observer_sha256": identity(game / "scripts/vscripts/hammerr_observer.nut")["sha256"]})
            require(identity(args.bsp) == source_identity and identity(game / "maps/hammerr_runtime.bsp") == source_identity,
                    "Native launch changed original/staged BSP input")
            require(material_identities(args.fixture_game) == material_inputs and material_identities(game) == material_inputs
                    and identity(args.probe_library) == probe_identity, "Native launch changed source/staged material or observer library bytes")
        result["success"] = True
    finally:
        write_json(output / "runtime.json", result)


def frame_measurement(path):
    pixels = ppm(path)
    left, top, right, bottom = ROI
    selected = b"".join(pixels[(y * 1280 + left) * 3:(y * 1280 + right) * 3] for y in range(top, bottom))
    count = len(selected) // 3
    return {"mean_rgb": [sum(selected[axis::3]) / count for axis in range(3)],
            "blue_dominant_fraction": sum(selected[index + 2] >= max(selected[index], selected[index + 1]) + GATES["minimum_blue_dominance"]
                                          for index in range(0, len(selected), 3)) / count}


def native_cases(args, cases, game):
    require(args.probe_library.is_file() and shutil.which("cage") and shutil.which("grim"), "Existing HDR observer library, Cage and grim are required")
    results = {"success": False, "predeclared_gates": GATES, "cases": {}}
    try:
        for name in ("low", "high", "restored"):
            output = args.output / f"native-{name}"
            value = VALUES["low" if name == "restored" else name]
            command = [sys.executable, "-B", str(Path(__file__).resolve()), "--worker", "--output", str(output), "--game-dir", str(args.game_dir),
                       "--bsp", str(cases[name]), "--fixture-game", str(game), "--probe-library", str(args.probe_library), "--value", value]
            runtime.private_compositor(command, output, timeout=65)
            result = json.loads((output / "runtime.json").read_text())
            require(result["success"], "Private runtime did not report complete observation")
            result["frames"] = {label: frame_measurement(output / f"game-{label}.ppm") for label in ("a", "b")}
            result["same_case_noise"] = difference(ppm(output / "game-a.ppm"), ppm(output / "game-b.ppm"), ROI)
            results["cases"][name] = result
            write_json(args.output / "native.json", results)
        for name, result in results["cases"].items():
            require(result["same_case_noise"]["mean_component_difference"] <= GATES["maximum_same_case_mean_component_difference"],
                    f"Unstable fixed-camera capture: {name}")
            for frame in result["frames"].values():
                red, green, blue = frame["mean_rgb"]
                require(blue >= max(red, green) + GATES["minimum_blue_dominance"]
                        and frame["blue_dominant_fraction"] >= GATES["minimum_blue_dominant_fraction"], "HDR world region is not blue-dominant")
        low = results["cases"]["low"]["frames"]["a"]["mean_rgb"][2]
        high = results["cases"]["high"]["frames"]["a"]["mean_rgb"][2]
        results["blue_effect"] = {"low": low, "high": high, "ratio": high / low if low else None, "difference": high - low}
        results["restoration"] = difference(ppm(args.output / "native-low/game-a.ppm"), ppm(args.output / "native-restored/game-a.ppm"), ROI)
        write_json(args.output / "native.json", results)
        require(high >= low * GATES["minimum_blue_ratio"] and high - low >= GATES["minimum_blue_difference"],
                "Predeclared native controller image-effect gate failed; retain this profile without loosening gates")
        require(results["restoration"]["mean_component_difference"] <= GATES["maximum_restored_mean_component_difference"],
                "Restored low exposure differs beyond the predeclared image noise envelope")
        require(len({result["pid"] for result in results["cases"].values()}) == 3, "Native cases did not use fresh processes")
        require(len({result["observer_sha256"] for result in results["cases"].values()}) == 1,
                "Observer script differs between exposure cases")
        results["success"] = True
        return results
    finally:
        write_json(args.output / "native.json", results)


def main():
    parser = argparse.ArgumentParser(description="Validate existing shared tonemap-controller float I/O and optional private HDR consumption.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--game-dir", type=Path, default=Path.home() / ".local/share/Steam/steamapps/common/Counter-Strike Source")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--runtime", action="store_true")
    parser.add_argument("--probe-library", type=Path, default=ROOT / "target/debug/hammerr_hdr_probe.so")
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--bsp", type=Path, help=argparse.SUPPRESS)
    parser.add_argument("--fixture-game", type=Path, help=argparse.SUPPRESS)
    parser.add_argument("--value", choices=tuple(VALUES.values()), help=argparse.SUPPRESS)
    args = parser.parse_args()
    for name in ("binary", "game_dir", "output", "probe_library", "bsp", "fixture_game"):
        value = getattr(args, name)
        if value is not None:
            setattr(args, name, value.resolve())
    if args.worker:
        require(args.bsp is not None and args.fixture_game is not None and args.value is not None, "Private worker arguments are incomplete")
        worker(args)
        return
    require(args.output.is_relative_to(ROOT / "target"), "Keep retained validation artifacts on disk under this checkout's target/")
    args.output.mkdir(parents=True, exist_ok=False)
    write_json(args.output / "predeclared.json", {"values": VALUES, "controls": CONTROLS, "gates": GATES,
               "camera_origin": [-192, -176, 48], "camera_angles": [35, 42, 0], "light_ldr": [40, 0, 0], "light_hdr": [0, 0, 160],
               "scope": "Single controller initialized by delayed equal min/max float I/O; not general adaptation or VRAD parity"})
    binary_identity = identity(args.binary)
    cases, game, report = fixture(args)
    report["binary"] = {"path": str(args.binary), **binary_identity}
    report["runtime_checked"] = args.runtime
    if args.runtime:
        report["native"] = native_cases(args, cases, game)
    require(identity(args.binary) == binary_identity, "MCP binary changed during validation")
    write_json(args.output / "results.json", report)
    print(json.dumps({"output": str(args.output), "compiled_float_io": True, "runtime_checked": args.runtime}))


if __name__ == "__main__":
    main()
