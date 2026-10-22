import argparse
import hashlib
import json
import os
import re
import resource
import struct
import subprocess
import sys
import time
import traceback
from pathlib import Path

import validate_cubemap_capture_probe as probe
import validate_native_runtime as runtime
from hdr_observer import HDR_CONTROLS, probe_environment, read_probe, verify_hardware
from private_runtime_config import (
    private_launch_options,
    private_native_process,
    private_observer,
    verify_private_launch,
)
from reflection_validation import difference, ppm
from validate_bump_lighting import texture
from validate_face_lighting import DEFAULT_BINARY, ROOT, Client, detail
from validate_hdr_reflections import MATERIAL_SYSTEM_SHA256, pak_entries
from validate_recovery_exit import require_clean_eof
from validate_visibility import ROOM
from validate_water_bottom_native import runtime_diagnostics, verify_runtime_materials

MANIFEST = "hammerr/packed-water.json"
EDGE = 32
REGION = (400, 200, 880, 520)
ON_READY = "HAMMERR_WATER_PACK_ON_READY 2 100"
OFF_READY = "HAMMERR_WATER_PACK_OFF_READY 2 100"
FINISHED = "HAMMERR_WATER_PACK_FINISHED"
VARIANTS = {"first": {"ldr": "green", "hdr": "blue"}, "second": {"ldr": "blue", "hdr": "green"}}
COLORS = {"green": (0, 1, 0), "blue": (0, 0, 1)}
CONTROLS = {
    "sv_lan": 1,
    "mat_fullbright": 0,
    "r_drawvgui": 0,
    "cl_drawhud": 0,
    "r_drawviewmodel": 0,
    "r_drawothermodels": 0,
    **HDR_CONTROLS,
}
OBSERVER = """
stage <- 0;
ready_at <- 0.0;
function HammeRRThink() {
    local player = Entities.FindByClassname(null, "player");
    if (player == null) return 0.25;
    if (stage == 0) {
        SendToConsole("sv_cheats 1; sv_lan 1; joystick 0; bot_quota 0; bot_kick; jointeam 2; joinclass 1; hidepanel info; hidepanel team; hidepanel class_ter; hidepanel class_ct; r_drawvgui 0; gameui_hide; cl_drawhud 0; r_drawviewmodel 0; r_drawothermodels 0; mat_fullbright 0; toggle con_notifytime 0; toggle mat_specular 1; toggle mat_fastspecular 1; toggle r_waterforceexpensive 0; toggle r_drawtranslucentworld 1; help mat_specular; help mat_fastspecular; help r_waterforceexpensive; help r_drawtranslucentworld");
        ready_at = Time() + 6.0;
        stage = 1;
    }
    if (stage == 1 && Time() > ready_at && player.GetTeam() == 2 && player.GetHealth() == 100) {
        SendToConsole("noclip; god; setang 75 0 0; sv_lan; mat_hdr_level; mat_fullbright");
        ready_at = Time() + 4.0;
        stage = 2;
    }
    if (stage >= 2) {
        player.SetOrigin(Vector(0, 0, 224) - (player.EyePosition() - player.GetOrigin()));
        player.SetVelocity(Vector(0, 0, 0));
    }
    if (stage == 2 && Time() > ready_at) {
        printl("HAMMERR_WATER_PACK_ON_READY " + player.GetTeam() + " " + player.GetHealth());
        stage = 3;
    }
    if (stage == 4 && Time() > ready_at) {
        printl("HAMMERR_WATER_PACK_OFF_READY " + player.GetTeam() + " " + player.GetHealth());
        stage = 5;
    }
    if (stage == 3 || stage == 5) {
        local eye = player.EyePosition();
        local angles = player.EyeAngles();
        printl("HAMMERR_WATER_PACK_POSE " + eye.x + " " + eye.y + " " + eye.z + " " + angles.x + " " + angles.y + " " + angles.z);
        SendToConsole("exec hammerr_water_pack_once.cfg");
    }
    return 0.25;
}
"""


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def cube(color, hdr, *, faces=7, version=4):
    header = bytearray(88)
    header[:4] = b"VTF\0"
    for offset, value in [(4, 7), (8, version), (12, 88), (20, 0x4000 | 0x0C), (52, 24 if hdr else 0), (57, 0xFFFFFFFF), (68, 1)]:
        struct.pack_into("<I", header, offset, value)
    for offset, value in [(16, EDGE), (18, EDGE), (24, 1), (26, 0 if faces == 7 else 65535), (63, 1)]:
        struct.pack_into("<H", header, offset, value)
    struct.pack_into("<3f", header, 32, *color)
    struct.pack_into("<f", header, 48, 1)
    header[56] = EDGE.bit_length()
    header[80:84] = bytes([0x30, 0, 0, 0])
    struct.pack_into("<I", header, 84, len(header))
    pixel = struct.pack("<4e", *color, 1) if hdr else bytes([*(value * 255 for value in color), 255])
    pixels = bytearray()
    for mip in reversed(range(header[56])):
        dimension = max(1, EDGE >> mip)
        pixels.extend(pixel * dimension * dimension * faces)
    return bytes(header + pixels)


def material(namespace, above):
    bottom = f'    $bottommaterial "{namespace}/bottom"\n' if above else ""
    return (
        "Water\n{\n"
        f'    %tooltexture "{namespace}/white"\n'
        f'    $normalmap "{namespace}/normal"\n'
        f'    $envmap "{namespace}/cube"\n'
        f"    $abovewater {int(above)}\n"
        + bottom
        + '    $reflecttexture "_rt_WaterReflection"\n'
        + '    $refracttexture "_rt_WaterRefraction"\n'
        + '    $reflecttint "[1 1 1]"\n'
        + "    $reflectamount 1\n"
        + "    $refractamount 0\n"
        + "    $forcecheap 1\n"
        + "    $forceexpensive 0\n"
        + "    $nofresnel 1\n"
        + '    $fogcolor "[0 0 0]"\n'
        + "    $fogenable 1\n"
        + "    $fogstart 0\n"
        + "    $fogend 4096\n"
        + "    $surfaceprop water\n"
        + "}\n"
    ).encode()


def originals(namespace, variant):
    prefix = f"materials/{namespace}"
    colors = VARIANTS[variant]
    return {
        f"{prefix}/surface.vmt": material(namespace, True),
        f"{prefix}/bottom.vmt": material(namespace, False),
        f"{prefix}/white.vtf": texture(bytes([255] * EDGE * EDGE * 4)),
        f"{prefix}/normal.vtf": texture(bytes([128, 128, 255, 255] * EDGE * EDGE), normal=True),
        f"{prefix}/cube.vtf": cube(COLORS[colors["ldr"]], False),
        f"{prefix}/cube.hdr.vtf": cube(COLORS[colors["hdr"]], True),
    }


def verify_packed(data, namespace, expected):
    entries, rows = pak_entries(probe.bsp_lumps(data)[40][2])
    require(MANIFEST in entries, "Published BSP has no Water packing manifest")
    manifest = json.loads(entries[MANIFEST])
    require(manifest["version"] == 1 and manifest["channels"] == "ldr_hdr", "Water packing manifest has the wrong contract")
    roots = manifest["roots"]
    known_roots = {f"materials/{namespace}/{name}.vmt" for name in ["surface", "bottom"]}
    require(roots == sorted(set(roots)) and f"materials/{namespace}/surface.vmt" in roots and set(roots) <= known_roots, "Water packing roots differ from the authored Water materials")
    receipts = [{"path": path, "sha256": probe.digest(payload)} for path, payload in sorted(expected.items())]
    require(manifest["assets"] == receipts, "Manifest does not name the independently generated original payloads")
    require(set(entries) == {*expected, MANIFEST}, f"Unregistered or missing PAK entries: {set(entries) ^ {*expected, MANIFEST}}")
    for path, payload in expected.items():
        require(entries[path] == payload, f"Packed original bytes changed: {path}")
    require(probe.bsp_lumps(data)[53][2] and probe.bsp_lumps(data)[58][2], "Fixture lacks both native lighting channels")
    return {"bsp_sha256": probe.digest(data), "manifest": manifest, "pak": rows, "original_bytes_exact": True}


def verify_pair(cases):
    require(set(cases) == set(VARIANTS), "Fixture requires both original cube variants")
    first = probe.bsp_lumps(probe.read_bounded(Path(cases["first"]["bsp"]), probe.MAX_BSP_BYTES))
    second = probe.bsp_lumps(probe.read_bounded(Path(cases["second"]["bsp"]), probe.MAX_BSP_BYTES))
    changed = [index for index, (left, right) in enumerate(zip(first, second, strict=True)) if left != right]
    require(changed == [40], f"Cube variants changed non-PAK geometry, lighting or entities: {changed}")
    return {"changed_lumps": changed, "geometry_lighting_entities_identical": True}


def fixture(args):
    game = runtime.private_game(args.game_dir, args.output / "source-game")
    namespace = "hammerr_water_pack_" + hashlib.sha256(os.fsencode(args.output)).hexdigest()[:12]
    first = originals(namespace, "first")
    for path, payload in first.items():
        target = game / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(payload)
    client = Client(None, args.binary, args.output, game / "gameinfo.txt", environment={
        "HAMMERR_RECENT_MAPS_DIR": str(args.output / "recent"),
        "HAMMERR_BUILD_RECIPES": str(args.output / "recipes.json"),
    })
    try:
        client.call("new_map", {"discard_changes": True})
        ids = []
        for mins, maxs in ROOM:
            ids.append(client.call("create_block", {"mins": mins, "maxs": maxs, "material": "dev/dev_measuregeneric01"})["id"])
        for classname, origin, properties in [
            ("info_player_terrorist", [-224, -192, 8], {}),
            ("info_player_counterterrorist", [224, -192, 8], {}),
            ("light", [0, 0, 224], {"_light": "255 255 255 200", "_constant_attn": "1", "_quadratic_attn": "0"}),
            ("logic_script", [-224, 0, 32], {"targetname": "hammerr_observer", "vscripts": "hammerr_observer.nut", "thinkfunction": "HammeRRThink"}),
        ]:
            ids.append(client.call("create_entity", {"classname": classname, "origin": origin, "properties": properties})["id"])
        pool = client.call("create_block", {"mins": [-160, -160, 0], "maxs": [160, 160, 160], "material": "tools/toolsnodraw"})["id"]
        ids.append(pool)
        brush = detail(client, [pool])[0]
        top = next(face for face in brush["faces"] if face["normal"] == [0, 0, 1])
        client.call("select_faces", {"faces": [{"solid": brush["solid"], "side": top["side"]}]})
        client.call("edit_faces", {"material": f"{namespace}/surface"})
        client.call("set_compile_settings", {"lighting": "direct_ldr_hdr", "physics": "none", "visibility": "fast", "pack_water_assets": True})
        source = args.output / "source.vmf"
        client.call("save_map", {"path": str(source)})
        before = (source.read_bytes(), detail(client, ids), client.call("get_map_info")["undo"], client.call("get_selection"), client.call("get_camera"), client.call("get_compile_settings"))
        cases = {}
        for variant in VARIANTS:
            expected = originals(namespace, variant)
            for path, payload in expected.items():
                (game / path).write_bytes(payload)
                snapshot = args.output / "originals" / variant / path
                snapshot.parent.mkdir(parents=True, exist_ok=True)
                snapshot.write_bytes(payload)
            bsp = args.output / f"{variant}.bsp"
            result = client.call("compile_map", {"path": str(bsp)})
            require(result["success"] and result["stats"]["water_regions"] == 1, f"Water packing compile failed: {result}")
            packed = verify_packed(probe.read_bounded(bsp, probe.MAX_BSP_BYTES), namespace, expected)
            after = (source.read_bytes(), detail(client, ids), client.call("get_map_info")["undo"], client.call("get_selection"), client.call("get_camera"), client.call("get_compile_settings"))
            require(before == after, "Compile changed the authored source, history, selection, camera or build settings")
            cases[variant] = {"bsp": str(bsp), "compile": result, "packed": packed, "expected_channel_colors": VARIANTS[variant]}
        report = {"namespace": namespace, "source_game": str(game), "source": str(source), "source_sha256": probe.digest(before[0]), "cases": cases, "pair": verify_pair(cases), "source_and_session_preserved": True, "native_acceptance": "not_run"}
        write_json(args.output / "fixture.json", report)
        return report
    finally:
        require_clean_eof(client)


def vpk_namespace_absence(path, namespace):
    observed, missing = [], []
    for index in [path, path.with_name(path.stem + "_dir.vpk")]:
        try:
            stream = index.open("rb")
        except FileNotFoundError:
            missing.append(str(index))
            continue
        with stream:
            header = stream.read(28)
            require(len(header) == 28, f"Truncated mounted VPK: {index}")
            magic, version, size = struct.unpack_from("<3I", header)
            require(magic == 0x55AA1234 and version in (1, 2) and 0 < size <= 64 * 1024**2, f"Unsupported mounted VPK index: {index}")
            stream.seek(12 if version == 1 else 28)
            tree = stream.read(size)
        require(len(tree) == size and namespace.encode() not in tree.lower(), f"Authored namespace already appears in mounted VPK: {index}")
        observed.append({"index": str(index), "tree_sha256": probe.digest(tree), "tree_bytes": size})
    return {"requested": str(path), "indexes": observed, "absent_candidates": missing, "namespace_absent": True}


def source_absence(root, game, namespace, source_game):
    info = (game / "gameinfo.txt").read_text()
    require(str(source_game) not in info and str(source_game.parent) not in info, "Authored source game remains mounted")
    loose = []
    for mount in [game, game / "download", root / "cstrike", root / "hl2", root / "platform"]:
        materials = mount / "materials"
        matches = [str(path) for path in materials.iterdir() if path.name.lower() == namespace] if materials.is_dir() else []
        require(not matches, f"Authored namespace is present as mounted loose files: {matches}")
        loose.append(str(materials / namespace))
    vpks = [(root / "cstrike" / name) for name in ["cstrike_english.vpk", "cstrike_pak.vpk"]]
    vpks += [(root / "hl2" / name) for name in ["hl2_textures.vpk", "hl2_sound_vo_english.vpk", "hl2_sound_misc.vpk", "hl2_misc.vpk"]]
    vpks.append(root / "platform/platform_misc.vpk")
    declared = re.findall(r'"[^"\n]+"\s+"([^"\n]+\.vpk)"', info)
    require(sorted(declared) == sorted(map(str, vpks)), "Private VPK search paths changed; verify the complete mount list")
    return {"source_game": str(source_game), "gameinfo_sha256": probe.digest(info.encode()), "source_game_unmounted": True, "loose_namespace_absent": loose, "vpk_trees": [vpk_namespace_absence(path, namespace) for path in vpks]}


def control_values(text, level, specular):
    controls = {**CONTROLS, "mat_hdr_level": level, "mat_specular": 1, "mat_fastspecular": specular, "r_waterforceexpensive": 0, "r_drawtranslucentworld": 1}
    result = {}
    for name, expected in controls.items():
        values = re.findall(r'"' + re.escape(name) + r'" = "([^"\n]+)"', text)
        require(values and float(values[-1]) == expected, f"Native {name} differs from the controlled fixture: {values}")
        result[name] = float(values[-1])
    return result


def verify_pose(text):
    rows = re.findall(r"HAMMERR_WATER_PACK_POSE ([^\r\n]+)", text)
    require(rows, "Native Water view pose was not reported")
    pose = list(map(float, rows[-1].split()))
    require(len(pose) == 6 and max(abs(a - b) for a, b in zip(pose, [0, 0, 224, 75, 0, 0], strict=True)) < 1, f"Native Water view moved: {pose}")
    return pose


def capture(output, name):
    for extension in ["png", "ppm"]:
        subprocess.run(["grim", *(["-t", "ppm"] if extension == "ppm" else []), str(output / f"{name}.{extension}")], check=True, timeout=5)
    return ppm(output / f"{name}.ppm")


def water_diagnostics(text, namespace):
    diagnostics = runtime_diagnostics(text)
    for row in diagnostics["all"]:
        if namespace in row["message"].lower() and not row["fatal"]:
            row["fixture_asset_or_water_shader"] = True
            row["fatal"] = True
            diagnostics["fixture_failures"].append(row)
    diagnostics["unrelated"] = [row for row in diagnostics["all"] if not row["fatal"]]
    return diagnostics


def worker(args):
    output = args.output
    require(not os.environ.get("LD_PRELOAD"), "Native validation rejects inherited LD_PRELOAD")
    require(os.environ.get("WAYLAND_DISPLAY") and os.environ.get("XDG_RUNTIME_DIR") == str(output / "wayland"), "Native Water view requires its private compositor")
    require(probe.digest((args.game_dir / "bin/linux64/materialsystem.so").read_bytes()) == MATERIAL_SYSTEM_SHA256, "Material system changed; repeat 704–706 native identity evidence")
    expected = originals(args.namespace, args.variant)
    data = probe.read_bounded(args.bsp, probe.MAX_BSP_BYTES)
    packed = verify_packed(data, args.namespace, expected)
    game = runtime.private_game(args.game_dir, output, args.bsp, engine_physics=True)
    staged = game / "maps/hammerr_runtime.bsp"
    require(probe.read_bounded(staged, probe.MAX_BSP_BYTES) == data, "Private staging changed the original published BSP")
    absence = source_absence(args.game_dir, game, args.namespace, args.source_game)
    (game / "scripts/vscripts/hammerr_observer.nut").write_text(private_observer(OBSERVER))
    cfg = game / "cfg/hammerr_water_pack_once.cfg"
    probe.write_cfg(cfg, "echo HAMMERR_WATER_PACK_WAIT\n")
    postmap = "toggle sv_lan 1\nhelp sv_lan\n" + "".join(f"help {name}\n" for name in CONTROLS if name != "sv_lan")
    for name in ["server.cfg", "listenserver.cfg", "hammerr_water_pack_postmap.cfg"]:
        (game / "cfg" / name).write_text(postmap)
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    settings = output / "settings"
    for name in ["config", "cache", "data"]:
        (settings / name).mkdir(parents=True)
    ready, hardware_log = output / "probe-ready", output / "hardware.jsonl"
    environment = {
        **os.environ, "LD_LIBRARY_PATH": str(args.game_dir / "bin/linux64"),
        "SteamAppId": "240", "SteamGameId": "240", "SDL_VIDEODRIVER": "x11",
        "XDG_CONFIG_HOME": str(settings / "config"), "XDG_CACHE_HOME": str(settings / "cache"),
        "XDG_DATA_HOME": str(settings / "data"),
        **probe_environment(args.game_dir, game, args.probe_library, ready, hardware_log),
    }
    flags = private_launch_options(args.game_dir, game, hdr_level=args.hdr_level, controls={"servercfgfile": "hammerr_water_pack_postmap.cfg", **CONTROLS})
    command = [str(args.game_dir / "cstrike_linux64"), "-game", str(game), "-windowed", "-w", str(probe.WIDTH), "-h", str(probe.HEIGHT), "-novid", *flags, "-nomaster", "-nojoy", "-console", "-port", str(args.port)]
    report = {"success": False, "variant": args.variant, "hdr_level": args.hdr_level, "packed": packed, "source_absence": absence, "desktop_input": False, "probe_sha256": probe.digest(args.probe_library.read_bytes()), "game": str(game)}
    console = game / "hammerr-runtime.log"
    try:
        with private_native_process(command, args.game_dir, game, output, environment, report) as process:
            started = time.monotonic()
            on = off = False
            while time.monotonic() - started < 90:
                text = probe.read_bounded(console, 2 * 1024**2).decode(errors="replace") if console.exists() else ""
                if not ready.exists() and ON_READY in text:
                    ready.write_text("post-map Water observer ready\n")
                hardware = verify_hardware(read_probe(hardware_log), args.hdr_level)
                if not on and ON_READY in text and hardware:
                    report["private_launch"] = verify_private_launch(game, text)
                    report["on_controls"] = control_values(text, args.hdr_level, 1)
                    report["on_pose"] = verify_pose(text)
                    capture(output, "specular-on")
                    probe.write_cfg(cfg, 'toggle mat_fastspecular 0\nhelp mat_fastspecular\nent_fire hammerr_observer RunScriptCode "stage = 4; ready_at = Time() + 4"\n')
                    on = True
                if on and not off and OFF_READY in text:
                    report["off_controls"] = control_values(text, args.hdr_level, 0)
                    report["off_pose"] = verify_pose(text)
                    capture(output, "specular-off")
                    off = True
                    probe.write_cfg(cfg, f"echo {FINISHED}\ndisconnect\nquit\n")
                if process.poll() is not None:
                    break
                time.sleep(0.05)
            else:
                raise TimeoutError("Native Water frame handshake exceeded 90 seconds")
            text = probe.read_bounded(console, 2 * 1024**2).decode(errors="replace")
            require(process.returncode == 0 and on and off and FINISHED in text, f"Native Water view failed; inspect {console}")
            require(text.index(ON_READY) < text.index(OFF_READY) < text.index(FINISHED), "Water captures were outside the ordered handshake")
            hardware = verify_hardware(read_probe(hardware_log), args.hdr_level)
            require(hardware is not None, "Live hardware HDR selection was not completed")
            diagnostics = water_diagnostics(text, args.namespace)
            require(not diagnostics["fixture_failures"], f"Native Water asset/shader diagnostic: {diagnostics['fixture_failures']}")
            require(not re.search(r"fall\s*back to a non-HDR version|failed to load.*\.hdr|error reading texture data.*\.hdr|(?:couldn.t|can.t) find.*\.hdr", text, re.IGNORECASE), "Native cube lookup used missing-HDR or LDR fallback")
            require("Level unlit" not in text, "Native fixture did not use its baked lighting")
            require(probe.read_bounded(staged, probe.MAX_BSP_BYTES) == data, "Native client changed the staged BSP")
            channel = "hdr" if args.hdr_level == 2 else "ldr"
            color = VARIANTS[args.variant][channel]
            delta = difference(ppm(output / "specular-on.ppm"), ppm(output / "specular-off.ppm"), REGION)
            require(delta["mean_component_difference"] > 8 and delta["changed_fraction"] > 0.05, f"Water has no measurable static reflection: {delta}")
            gains = [a - b for a, b in zip(delta["first_rgb"], delta["second_rgb"], strict=True)]
            axis = 1 if color == "green" else 2
            require(gains[axis] > 8 and all(gains[axis] > gain + 5 for index, gain in enumerate(gains) if index != axis), f"Water reflection does not select the expected original {channel} cube ({color}): {gains}")
            report.update({"success": True, "pid": process.pid, "hardware": hardware, "hardware_probe_records": read_probe(hardware_log), "diagnostics": diagnostics, "hdr_fallback_absent": True, "reflection_pixels": delta, "channel_gains": gains, "expected_color": color, "elapsed_seconds": time.monotonic() - started, "materials_sha256": runtime_material_hashes(game)})
    except BaseException as error:
        report.update({"success": False, "error": str(error), "traceback": traceback.format_exc()})
        raise
    finally:
        write_json(output / "runtime.json", report)


def runtime_material_hashes(game):
    directory = game / "materials"
    return {str(path.relative_to(game)): probe.digest(probe.read_bounded(path, probe.MAX_BSP_BYTES)) for path in directory.rglob("*") if path.is_file()}


def native_cases(args, fixtures):
    pair = verify_pair(fixtures["cases"])
    require(pair == fixtures["pair"], "Reused fixture pair contract changed")
    results = {}
    for index, (variant, case) in enumerate(fixtures["cases"].items()):
        expected = originals(fixtures["namespace"], variant)
        require(verify_packed(probe.read_bounded(Path(case["bsp"]), probe.MAX_BSP_BYTES), fixtures["namespace"], expected) == case["packed"], "Reused fixture identity changed")
        for channel, level in [("ldr", 0), ("hdr", 2)]:
            name = f"{variant}-{channel}"
            output = args.output / name
            command = [sys.executable, "-B", str(Path(__file__).resolve()), "--worker", "--binary", str(args.binary), "--output", str(output), "--game-dir", str(args.game_dir), "--bsp", case["bsp"], "--namespace", fixtures["namespace"], "--source-game", fixtures["source_game"], "--variant", variant, "--hdr-level", str(level), "--probe-library", str(args.probe_library), "--port", str(args.port + index * 2 + level // 2)]
            runtime.private_compositor(command, output, timeout=105)
            result = json.loads((output / "runtime.json").read_text())
            require(result["success"], f"Native Water case failed: {result}")
            require(runtime_material_hashes(Path(result["game"])) == result["materials_sha256"], "Runtime loose materials changed after shutdown")
            result["cloud_material_imports"] = verify_runtime_materials(result["materials_sha256"], {}, json.loads((output / "private-preservation.json").read_text()))
            result["source_absence_after"] = source_absence(args.game_dir, Path(result["game"]), fixtures["namespace"], Path(fixtures["source_game"]))
            require(result["source_absence_after"] == result["source_absence"], "Runtime gameinfo or mounted namespace/index identities changed")
            cleanup = json.loads((output / "private-cleanup.json").read_text())
            require(not cleanup["cleanup_failures"] and not cleanup["remaining_live_processes"], f"Native process cleanup failed: {cleanup}")
            results[name] = result
    require(len({row["hardware"]["native_pid"] for row in results.values()}) == 4, "Native cases reused a client process")
    controls = {}
    for channel in ["ldr", "hdr"]:
        delta = difference(ppm(args.output / f"first-{channel}/specular-off.ppm"), ppm(args.output / f"second-{channel}/specular-off.ppm"), REGION)
        require(delta["mean_component_difference"] < 2 and delta["changed_fraction"] < 0.01, f"Reflection-disabled Water control changed between original cube variants: {delta}")
        controls[channel] = delta
    return {"cases": results, "reflection_disabled_controls": controls, "source_mounts_absent": True, "original_pak_payloads_exact": True, "both_live_channels_discriminated": True, "scope": "Original static Water files produce distinct expected cheap-Water reflection colors in fresh LDR/HDR clients with source mounts absent; direct filesystem-winner observation and HDR radiance parity are not measured."}


def main():
    parser = argparse.ArgumentParser(description="Compile original static Water files and optionally verify source-mount-absent LDR/HDR native reflection controls. Requires a built MCP executable; --native also requires cage, grim, bwrap, the pinned CS:S install and a prebuilt scripts/hdr_runtime_probe.c observer library. Native runs use four fresh private clients, read-only external files, -insecure and sv_lan 1 without desktop input.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True, help="Fresh on-disk directory under target/validation; keep its path short enough for private Wayland sockets")
    parser.add_argument("--game-dir", type=Path, default=Path.home() / ".local/share/Steam/steamapps/common/Counter-Strike Source")
    parser.add_argument("--native", action="store_true", help="Run four private native cases and fixed visual/fallback gates after compilation")
    parser.add_argument("--reuse-fixture", type=Path, help="Read an existing fixture.json and verify its exact original packed payloads before native validation")
    parser.add_argument("--probe-library", type=Path, default=ROOT / "target/debug/hammerr_hdr_probe.so")
    parser.add_argument("--port", type=int, default=27630)
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--bsp", type=Path, help=argparse.SUPPRESS)
    parser.add_argument("--namespace", help=argparse.SUPPRESS)
    parser.add_argument("--source-game", type=Path, help=argparse.SUPPRESS)
    parser.add_argument("--variant", choices=VARIANTS, help=argparse.SUPPRESS)
    parser.add_argument("--hdr-level", type=int, choices=[0, 2], help=argparse.SUPPRESS)
    args = parser.parse_args()
    args.output = args.output.resolve()
    args.game_dir = args.game_dir.resolve(strict=True)
    args.binary = args.binary.resolve(strict=True)
    require(args.output.is_relative_to((ROOT / "target/validation").resolve()), "Output must be on disk under target/validation")
    require(1024 <= args.port <= 65531, "Native port range is invalid")
    if args.worker:
        require(args.variant is not None and args.hdr_level is not None and args.bsp is not None and args.source_game is not None and re.fullmatch(r"hammerr_water_pack_[a-f0-9]{12}", args.namespace or ""), "Worker requires the complete validated fixture identity")
        args.probe_library = args.probe_library.resolve(strict=True)
        worker(args)
        return
    require(not args.output.exists(), "Use a fresh output directory; failed receipts must remain intact")
    require(not os.environ.get("LD_PRELOAD"), "Validation rejects inherited LD_PRELOAD")
    args.output.mkdir(parents=True)
    report = {"success": False, "native_acceptance": "not_run"}
    try:
        fixtures = json.loads(args.reuse_fixture.read_text()) if args.reuse_fixture else fixture(args)
        report["fixture"] = fixtures
        if args.native:
            args.probe_library = args.probe_library.resolve(strict=True)
            report["native"] = native_cases(args, fixtures)
            report["native_acceptance"] = "passed"
        report["success"] = True
    except BaseException as error:
        report.update({"success": False, "error": str(error), "traceback": traceback.format_exc()})
        raise
    finally:
        write_json(args.output / "result.json", report)


if __name__ == "__main__":
    main()
