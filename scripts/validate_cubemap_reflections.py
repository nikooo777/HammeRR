import argparse
import json
import os
import re
import resource
import shutil
import struct
import subprocess
import sys
import time
import traceback
from pathlib import Path

import validate_cubemap_capture_probe as probe
import validate_native_runtime as runtime
from private_runtime_config import (
    finish_private_launch,
    private_launch_options,
    private_observer,
    verify_private_launch,
)
from validate_bump_lighting import texture
from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_visibility import ROOM

MANIFEST = "hammerr/generated-cubemaps.json"
MATERIALS = "hammerr_reflections"
ON_READY = "HAMMERR_REFLECTION_ON_READY 2 100"
OFF_READY = "HAMMERR_REFLECTION_OFF_READY 2 100"
FINISHED = "HAMMERR_REFLECTION_FINISHED"
REGION = (320, 400, 960, 650)
OBSERVER = """
stage <- 0;
ready_at <- 0.0;
function HammeRRThink() {
    local player = Entities.FindByClassname(null, "player");
    if (player == null) return 0.25;
    if (stage == 0) {
        SendToConsole("sv_cheats 1; sv_lan 1; joystick 0; bot_quota 0; bot_kick; jointeam 2; joinclass 1; hidepanel info; hidepanel team; hidepanel class_ter; hidepanel class_ct; r_drawvgui 0; gameui_hide; cl_drawhud 0; r_drawviewmodel 0; r_drawothermodels 0; mat_fullbright 0; toggle con_notifytime 0; toggle mat_specular 1; help mat_specular");
        ready_at = Time() + 6.0;
        stage = 1;
    }
    if (stage == 1 && Time() > ready_at && player.GetTeam() == 2 && player.GetHealth() == 100) {
        SendToConsole("noclip; god; setpos -192 -176 80; setang 25 45 0; sv_lan; mat_hdr_level; mat_fullbright");
        ready_at = Time() + 4.0;
        stage = 2;
    }
    if (stage >= 2) {
        player.SetOrigin(Vector(-192, -176, 80) - (player.EyePosition() - player.GetOrigin()));
        player.SetVelocity(Vector(0, 0, 0));
    }
    if (stage == 2 && Time() > ready_at) {
        printl("HAMMERR_REFLECTION_ON_READY " + player.GetTeam() + " " + player.GetHealth());
        stage = 3;
    }
    if (stage == 4 && Time() > ready_at) {
        printl("HAMMERR_REFLECTION_OFF_READY " + player.GetTeam() + " " + player.GetHealth());
        stage = 5;
    }
    if (stage == 3 || stage == 5) SendToConsole("exec hammerr_reflection_once.cfg");
    return 0.25;
}
"""


def require(condition, message):
    probe.require(condition, message)


def patch_lump(data, index, payload):
    result = bytearray(data)
    result.extend(b"\0" * (-len(result) % 4))
    version, compression, _ = probe.bsp_lumps(data)[index]
    struct.pack_into("<4i", result, 8 + index * 16, len(result), len(payload), version, compression)
    result.extend(payload)
    return bytes(result)


def source_assets(game):
    return {
        str(path.relative_to(game)): probe.digest(probe.read_bounded(path, probe.MAX_ENTRY_BYTES))
        for path in sorted((game / "materials" / MATERIALS).rglob("*"))
        if path.is_file()
    }


def protected_files(args, names):
    paths = [args.game_dir / "bin/linux64" / name for name in probe.MODULES]
    paths.extend(path for path in (args.game_dir / "cstrike/cfg").rglob("*") if path.is_file())
    for name in names:
        paths.append(args.game_dir / "cstrike/maps" / f"{name}.bsp")
        for prefix in [args.game_dir, args.game_dir / "cstrike"]:
            for folder in ["materials/maps", "materialsrc/maps"]:
                paths.extend(path for path in (prefix / folder / name).rglob("*") if path.is_file())
    return {
        str(path): probe.digest(probe.read_bounded(path, probe.MAX_BSP_BYTES)) if path.exists() else None
        for path in sorted(set(paths))
    }


def fake_install(game, output):
    executable = game.parent / "cstrike_linux64"
    program = """#!/usr/bin/env python3
import json
import shutil
import sys
from pathlib import Path
args = sys.argv[1:]
assert '-insecure' in args and args[args.index('+sv_lan') + 1] == '1', args
game = Path(args[args.index('-game') + 1]).resolve(strict=True)
name = args[args.index('+map') + 1]
assert name.startswith('hammerr_') and '/' not in name and '\\\\' not in name, name
source = game / 'maps' / (name + '.bsp')
assert source.stat().st_size <= 64 * 1024**2
output = Path(__file__).resolve().parent.parent
shutil.copyfile(source, output / 'run-staged.bsp')
(output / 'run-launch.json').write_text(json.dumps({'map':name,'game':str(game),'argv':args,'fake_executable':str(Path(__file__).resolve())},indent=2)+'\\n')
"""
    executable.write_text(program)
    executable.chmod(0o700)
    require(executable.resolve().parent == game.parent.resolve(), "Fake Run executable escaped the private fixture")
    return executable


def author(args):
    game = runtime.private_game(args.game_dir, args.output / "source-game", engine_physics=True)
    folder = game / "materials" / MATERIALS
    folder.mkdir(parents=True)
    colors = {
        "gray": (40, 40, 40), "cyan": (16, 200, 200), "red": (220, 24, 16),
        "green": (16, 220, 32), "yellow": (220, 200, 16), "blue": (16, 32, 220),
    }
    for name, rgb in colors.items():
        (folder / f"{name}.vtf").write_bytes(texture(bytes([*rgb, 255]) * 32 * 32))
        shader = "LightmappedGeneric" if name == "gray" else "UnlitGeneric"
        extra = ' $envmap "env_cubemap" $envmaptint "[1 1 1]" unknown_metadata { retained "yes" }' if name == "gray" else ""
        (folder / f"{name}.vmt").write_text(f'{shader} {{ $basetexture "{MATERIALS}/{name}" $surfaceprop concrete{extra} }}\n')
    fake_install(game, args.output)
    client = Client(None, args.binary, args.output, game / "gameinfo.txt")
    ids = []
    try:
        for (low, high), material in zip(ROOM, colors, strict=True):
            ids.append(client.call("create_block", {"mins": low, "maxs": high, "material": f"{MATERIALS}/{material}"})["id"])
        for classname, origin, properties in [
            ("info_player_terrorist", [-192, -176, 8], {}),
            ("info_player_counterterrorist", [192, 176, 8], {}),
            ("light", [0, 0, 180], {"_light": "255 255 255 200", "_constant_attn": "1", "_linear_attn": "0", "_quadratic_attn": "0"}),
            *[("env_cubemap", list(origin), {"cubemapsize": str(probe.SIZE_ENUM)}) for origin in probe.SAMPLES],
        ]:
            ids.append(client.call("create_entity", {"classname": classname, "origin": origin, "properties": properties})["id"])
        source = args.output / "reflections.vmf"
        client.call("save_map", {"path": str(source)})
        floor = detail(client, [ids[0]])[0]
        top = next(face for face in floor["faces"] if face["normal"][2] > 0.99)
        face = {"solid": floor["solid"], "side": top["side"]}
        right_helper = int(ids[-1].split(":")[-1])
        before = detail(client, ids), client.call("get_map_info")["undo"], source.read_bytes()
        report = client.call("get_cubemap_bindings")
        require(not report["diagnostics"], report)
        request = {"action": "preview", "helper": right_helper, "faces": [face], "operation": "bind", "snapshot": report["state"]["snapshot"]}
        draft = client.call("set_cubemap_bindings", request)
        require((detail(client, ids), client.call("get_map_info")["undo"], source.read_bytes()) == before, "Binding preview changed the source or Undo history")
        client.call("set_cubemap_bindings", {"action": "cancel", "token": draft["draft"]["token"]})
        require((detail(client, ids), client.call("get_map_info")["undo"], source.read_bytes()) == before, "Binding Cancel changed the source or Undo history")
        report = client.call("get_cubemap_bindings")
        request["snapshot"] = report["state"]["snapshot"]
        draft = client.call("set_cubemap_bindings", request)
        client.call("set_cubemap_bindings", {"action": "apply", "token": draft["draft"]["token"]})
        applied = detail(client, ids)
        require(applied != before[0] and client.call("get_map_info")["undo"][0] == "Edit cubemap face bindings", "Binding Apply did not create its shared Undo transaction")
        client.call("undo")
        require(detail(client, ids) == before[0], "Binding Undo did not restore exact object metadata")
        client.call("redo")
        require(detail(client, ids) == applied, "Binding Redo did not restore the reviewed assignment")
        client.call("save_map", {"path": str(source)})
        before = source.read_bytes(), detail(client, ids), client.call("get_map_info")["undo"], source_assets(game)
        native = client.call("set_native_capture_runtime", {"engine_root": str(args.game_dir), **({"steam_root": str(args.steam_root)} if args.steam_root else {})})
        require(native["ready"], native)
        client.call("set_compile_settings", {"lighting": "direct_ldr", "physics": "none", "bounces": 0, "generate_reflections": False})
        bsp = args.output / "reflections.bsp"
        legacy = client.call("compile_map", {"path": str(bsp)})
        require(legacy["success"] and legacy["stats"]["lighting_bytes"] > 0, legacy)
        off_data = probe.read_bounded(bsp, probe.MAX_BSP_BYTES)
        off_entries, _ = probe.zip_entries(probe.bsp_lumps(off_data)[40][2])
        require(MANIFEST not in off_entries and probe.bsp_lumps(off_data)[7][0] == 0, "Generation OFF changed the legacy BSP contract")
        settings = client.call("get_compile_status")["settings"]
        client.call("set_compile_settings", {"lighting": "fullbright", "generate_reflections": True}, error=True)
        require(client.call("get_compile_status")["settings"] == settings, "Invalid combined reflection settings mutated shared settings")
        client.call("set_compile_settings", {"lighting": "direct_ldr", "generate_reflections": True})
        client.call("start_compile", {"path": str(bsp)})
        started = time.monotonic()
        job = None
        while time.monotonic() - started < args.timeout:
            job = client.call("get_compile_status")["job"]
            if job["status"] not in ["running", "cancelling"]:
                break
            time.sleep(0.2)
        require(job and job["status"] == "completed" and job["result"]["published"] is True, job)
        generated = job["result"]
        require(generated["stats"]["reflections"]["verified_samples"] == len(probe.SAMPLES), generated)
        require((source.read_bytes(), detail(client, ids), client.call("get_map_info")["undo"], source_assets(game)) == before, "Reflection build changed source metadata, Undo or mounted fixture assets")
        data = probe.read_bounded(bsp, probe.MAX_BSP_BYTES)
        published = verify_assets(data, bsp.stem, args.output / "published")
        require(any(row["manual"] and row["face_id"] == face["side"] and row["helper_id"] == right_helper for row in published["manifest"]["assignments"]), "Reviewed manual floor assignment was not compiled")
        launched = client.call("launch_game")
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            log = client.call("game_log")
            if log["status"] != "running":
                break
            time.sleep(0.05)
        require(log["status"] == "exited" and log["exit_code"] == 0, log)
        launch = json.loads((args.output / "run-launch.json").read_text())
        require(launch["map"] != bsp.stem and launch["map"] == launched["map"], "Run did not stage a fresh random namespace")
        staged = verify_assets(probe.read_bounded(args.output / "run-staged.bsp", probe.MAX_BSP_BYTES), launch["map"], args.output / "rebased")
        require(staged["manifest"]["logical_namespace"] == bsp.stem, "Run lost the original logical namespace")
        require(probe.bsp_lumps(data)[7] == probe.bsp_lumps((args.output / "run-staged.bsp").read_bytes())[7], "Run rebasing changed actual face records")
        entries, _ = probe.zip_entries(probe.bsp_lumps(data)[40][2])
        del entries[published["manifest"]["samples"][0]["texture"]]
        tampered = patch_lump(data, 40, probe.make_zip(entries))
        try:
            verify_assets(tampered, bsp.stem, args.output / "missing-sample")
        except AssertionError as error:
            require("Missing or foreign sample" in str(error), f"Missing-sample check failed for another reason: {error}")
        else:
            raise AssertionError("Independent validation accepted a missing generated sample")
        launch_before = (args.output / "run-launch.json").read_bytes()
        try:
            bsp.write_bytes(tampered)
            client.call("launch_game", error=True)
            require((args.output / "run-launch.json").read_bytes() == launch_before, "A BSP with a missing generated sample reached the Run executable")
        finally:
            bsp.write_bytes(data)
        require((source.read_bytes(), detail(client, ids), client.call("get_map_info")["undo"], source_assets(game)) == before, "Run mutated source metadata, Undo or fixture assets")
        return {"source": str(source), "bsp": str(bsp), "fixture_game": str(game), "compile": generated, "legacy_compile": legacy, "native_runtime": native, "binding_face": face, "manual_helper": right_helper, "published": published, "run": {**launch, "verification": staged}, "source_preserved": True, "missing_sample_launch_rejected": True}
    finally:
        try:
            status = client.call("get_compile_status")["job"]
            if status and status["status"] in ["running", "cancelling"]:
                if status["cancellable"]:
                    client.call("cancel_compile", {"token": status["token"]})
                deadline = time.monotonic() + 240
                while time.monotonic() < deadline:
                    if client.call("get_compile_status")["job"]["status"] not in ["running", "cancelling"]:
                        break
                    time.sleep(0.25)
                else:
                    raise TimeoutError("Native build did not finish cleanup after validation failure")
        finally:
            client.close()


def verify_assets(data, namespace, output):
    output.mkdir()
    lumps = probe.bsp_lumps(data)
    entries, records = probe.zip_entries(lumps[40][2])
    require(MANIFEST in entries, "Verified generated-asset manifest is absent")
    manifest = json.loads(entries[MANIFEST])
    require(manifest["version"] == 1 and manifest["phase"] == "verified" and manifest["channel"] == "ldr" and manifest["namespace"] == namespace, manifest)
    require(len(manifest["samples"]) == len(probe.SAMPLES) and manifest["aliases"] and manifest["assignments"], "Generated reflection manifest is incomplete")
    sample_reports = []
    for sample in manifest["samples"]:
        name = sample["texture"]
        require(name.startswith(f"materials/maps/{namespace}/") and name.endswith(".vtf") and name in entries, f"Missing or foreign sample: {sample}")
        pixels = entries[name]
        require(probe.digest(pixels) == sample["sha256"] != sample["bootstrap_sha256"], "Sample hash is stale or still names the bootstrap")
        metadata, faces = probe.decode_cube(pixels)
        require(metadata["image_end"] == len(pixels) and sample["edge"] == probe.CUBE_SIZE, "Generated sample has extra or missing image bytes")
        probe.save_faces(output / (Path(sample["texture"]).name + "-faces.png"), faces)
        sample_reports.append({"name": name, "sha256": sample["sha256"], "vtf": metadata, "pixels": probe.check_pixels(faces)})
    require(len({sample["sha256"] for sample in sample_reports}) == len(probe.SAMPLES), "Different sample positions produced identical captures")
    aliases = {alias["alias"] for alias in manifest["aliases"]}
    for alias in manifest["aliases"]:
        name = "materials/" + alias["alias"] + ".vmt"
        require(alias["alias"].startswith(f"maps/{namespace}/") and name in entries, "Missing or foreign generated material")
        require(probe.digest(entries[name]) == alias["sha256"], "Generated alias hash mismatch")
        text = entries[name].decode()
        envmaps = re.findall(r'"\$envmap"\s+"([^"\n]+)"', text, re.IGNORECASE)
        require(sorted(set(envmaps)) == alias["envmaps"] and "env_cubemap" not in text.lower(), "Generated material retained an unresolved cubemap reference")
        sample_texture = manifest["samples"][alias["sample"]]["texture"].removeprefix("materials/").removesuffix(".vtf")
        require(sample_texture in envmaps, "Alias does not select its registered sample")
        require("unknown_metadata" in text and "retained" in text, "Alias flattening discarded unknown authored material metadata")
    strings = lumps[43][2]
    offsets = [row[0] for row in struct.iter_unpack("<I", lumps[44][2])]
    names = []
    for row in struct.iter_unpack("<3f5i", lumps[2][2]):
        offset = offsets[row[3]]
        end = strings.index(0, offset)
        names.append(strings[offset:end].decode())
    infos = list(struct.iter_unpack("<16f2i", lumps[6][2]))
    bindings = []
    for index, row in enumerate(infos):
        alias = names[row[17]]
        if alias in aliases:
            bindings.append({"index": index, "alias": alias, "sha256": probe.digest(lumps[6][2][index * 72:(index + 1) * 72])})
    require(bindings and bindings == manifest["texinfo_bindings"], "Actual TexInfo vectors/materials differ from registered bindings")
    alias_infos = {row["index"] for row in bindings}
    consumers = []
    for lump, size, offset in [(7, 56, 10), (27, 56, 10), (19, 8, 2)]:
        payload = lumps[lump][2]
        require(len(payload) % size == 0, "Malformed reflection consumer lump")
        for index in range(len(payload) // size):
            texinfo = struct.unpack_from("<h", payload, index * size + offset)[0]
            if texinfo in alias_infos:
                consumers.append({"lump": lump, "index": index, "texinfo": texinfo})
    require(consumers and consumers == manifest["face_bindings"], "Actual face/brushside alias consumers differ from registered bindings")
    return {"manifest": manifest, "samples": sample_reports, "pak": records, "bound_texinfos": len(bindings), "bound_consumers": len(consumers), "bsp_sha256": probe.digest(data)}


def ppm(path):
    data = probe.read_bounded(path, 4 * probe.WIDTH * probe.HEIGHT)
    match = re.match(rb"P6\s+(\d+)\s+(\d+)\s+255\s", data)
    require(match is not None, "Invalid PPM screenshot")
    width, height = map(int, match.groups())
    pixels = data[match.end():]
    require((width, height) == (probe.WIDTH, probe.HEIGHT) and len(pixels) == width * height * 3, "Native viewport differs from the verified dimensions")
    return pixels


def difference(first, second):
    left, top, right, bottom = REGION
    a = b"".join(first[(y * probe.WIDTH + left) * 3:(y * probe.WIDTH + right) * 3] for y in range(top, bottom))
    b = b"".join(second[(y * probe.WIDTH + left) * 3:(y * probe.WIDTH + right) * 3] for y in range(top, bottom))
    delta = [abs(x - y) for x, y in zip(a, b, strict=True)]
    count = len(a) // 3
    return {
        "region": REGION,
        "mean_component_difference": sum(delta) / len(delta),
        "changed_fraction": sum(max(delta[index:index + 3]) > 8 for index in range(0, len(delta), 3)) / count,
        "first_rgb": [sum(a[axis::3]) / count for axis in range(3)],
        "second_rgb": [sum(b[axis::3]) / count for axis in range(3)],
    }


def worker(args):
    output = args.output
    require(not os.environ.get("LD_PRELOAD"), "Private native validation must not inherit LD_PRELOAD")
    require(os.environ.get("WAYLAND_DISPLAY") and os.environ.get("XDG_RUNTIME_DIR") == str(output / "wayland"), "Native view requires its private compositor")
    require(re.fullmatch(r"[a-z0-9_-]{1,64}", args.map_name), "Invalid private native map basename")
    game = runtime.private_game(args.game_dir, output, engine_physics=True)
    shutil.copytree(args.fixture_game / "materials" / MATERIALS, game / "materials" / MATERIALS)
    data = probe.read_bounded(args.bsp, probe.MAX_BSP_BYTES)
    entities = probe.bsp_lumps(data)[0][2]
    require(b'hammerr_reflection_observer' not in entities, "Observer name collides with authored source")
    observer = b'\n{\n"classname" "logic_script"\n"targetname" "hammerr_reflection_observer"\n"origin" "-192 -176 80"\n"vscripts" "hammerr_reflection_observer.nut"\n"thinkfunction" "HammeRRThink"\n}\n\0'
    staged = game / "maps" / f"{args.map_name}.bsp"
    staged.write_bytes(patch_lump(data, 0, entities.rstrip(b"\0") + observer))
    (game / "scripts/vscripts/hammerr_reflection_observer.nut").write_text(private_observer(OBSERVER))
    cfg = game / "cfg/hammerr_reflection_once.cfg"
    probe.write_cfg(cfg, "echo HAMMERR_REFLECTION_WAIT\n")
    post_map = "toggle sv_lan 1\ntoggle mat_fullbright 0\nhelp sv_lan\nhelp mat_hdr_level\nhelp mat_fullbright\n"
    for name in ["server.cfg", "listenserver.cfg", "hammerr_reflection_postmap.cfg"]:
        (game / "cfg" / name).write_text(post_map)
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    settings = output / "settings"
    for name in ["config", "cache", "data"]:
        (settings / name).mkdir(parents=True)
    environment = {
        **os.environ, "LD_LIBRARY_PATH": str(args.game_dir / "bin/linux64"),
        "SteamAppId": "240", "SteamGameId": "240", "SDL_VIDEODRIVER": "x11",
        "XDG_CONFIG_HOME": str(settings / "config"), "XDG_CACHE_HOME": str(settings / "cache"),
        "XDG_DATA_HOME": str(settings / "data"),
    }
    flags = private_launch_options(args.game_dir, game, hdr_level=0, map_name=args.map_name, log_name="hammerr-reflections.log", controls={"servercfgfile": "hammerr_reflection_postmap.cfg", "mat_fullbright": 0})
    command = [str(args.game_dir / "cstrike_linux64"), "-game", str(game), "-windowed", "-w", str(probe.WIDTH), "-h", str(probe.HEIGHT), "-novid", *flags, "-nomaster", "-nojoy", "-console", "-port", str(args.port)]
    report = {"command": command, "map": args.map_name, "bsp_sha256": probe.digest(data), "private_entity_adapter_only": True, "desktop_input": False, "success": False}
    console = game / "hammerr-reflections.log"
    process = None
    try:
        with (output / "launcher.log").open("w") as stream:
            process = subprocess.Popen(command, cwd=args.game_dir, env=environment, stdin=subprocess.DEVNULL, stdout=stream, stderr=stream)
            started = time.monotonic()
            on = off = False
            while time.monotonic() - started < 90:
                text = probe.read_bounded(console, 2 * 1024**2).decode(errors="replace") if console.exists() else ""
                if not on and ON_READY in text:
                    report["private_launch"] = verify_private_launch(game, text)
                    require(re.search(r'"mat_specular" = "1"', text), "Specular ON was not confirmed by the installed engine")
                    for extension in ["png", "ppm"]:
                        subprocess.run(["grim", *( ["-t", "ppm"] if extension == "ppm" else []), str(output / f"specular-on.{extension}")], check=True, timeout=5)
                    ppm(output / "specular-on.ppm")
                    probe.write_cfg(cfg, 'toggle mat_specular 0\nhelp mat_specular\nent_fire hammerr_reflection_observer RunScriptCode "stage = 4; ready_at = Time() + 4"\n')
                    on = True
                if on and not off and OFF_READY in text:
                    require(re.search(r'"mat_specular" = "0"', text), "Specular OFF was not confirmed by the installed engine")
                    for extension in ["png", "ppm"]:
                        subprocess.run(["grim", *( ["-t", "ppm"] if extension == "ppm" else []), str(output / f"specular-off.{extension}")], check=True, timeout=5)
                    off = True
                    probe.write_cfg(cfg, f"echo {FINISHED}\ndisconnect\nquit\n")
                if process.poll() is not None:
                    break
                time.sleep(0.02)
            else:
                raise TimeoutError("Native reflection frame handshake exceeded 90 seconds")
            text = probe.read_bounded(console, 2 * 1024**2).decode(errors="replace")
            require(process.returncode == 0 and on and off and FINISHED in text, f"Native reflection view failed: {report}; inspect {console}")
            require(text.index(ON_READY) < text.index(OFF_READY) < text.index(FINISHED), "Reflection frames were taken outside the ordered handshake")
            report["private_launch"] = verify_private_launch(game, text)
            controls = {}
            for name, expected in {"sv_lan": 1, "mat_hdr_level": 0, "mat_fullbright": 0, "mat_specular": 0}.items():
                values = re.findall(r'"' + re.escape(name) + r'" = "([^"\n]+)"', text)
                require(values and float(values[-1]) == expected, f"Native {name} control differs from the proof: {values}")
                controls[name] = float(values[-1])
            require(not re.search(r'(?:Error reading texture data "|couldn.t find )maps/' + re.escape(args.map_name) + r'/', text, re.IGNORECASE), "Native engine rejected a generated reflection texture")
            require("env_cubemap used on world geometry without rebuilding map" not in text, "The rendered surface retained an unbound placeholder")
            delta = difference(ppm(output / "specular-on.ppm"), ppm(output / "specular-off.ppm"))
            require(delta["mean_component_difference"] > 8 and delta["changed_fraction"] > 0.05, f"The actual floor did not display a measurable reflection: {delta}")
            gains = [on - off for on, off in zip(delta["first_rgb"], delta["second_rgb"], strict=True)]
            require(max(gains[1:]) > gains[0] + 5, f"The reflection lacks the expected green/blue directional signature: {gains}")
            delta["channel_gains"] = gains
            report.update({"success": True, "controls": controls, "reflection_pixels": delta, "elapsed_seconds": time.monotonic() - started})
    except BaseException as error:
        report.update({"error": str(error), "traceback": traceback.format_exc()})
        raise
    finally:
        if process is not None and process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
        try:
            report["configuration_preservation"] = finish_private_launch(game)
        except BaseException as error:
            report.update({"success": False, "preservation_error": str(error)})
            raise
        finally:
            report["exit_code"] = None if process is None else process.returncode
            (output / "runtime.json").write_text(json.dumps(report, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser(description="Prove shared MCP LDR reflection generation, owned face bindings, Run namespace rebasing and actual native reflected pixels in private displays.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--game-dir", type=Path, default=Path.home() / ".local/share/Steam/steamapps/common/Counter-Strike Source")
    parser.add_argument("--steam-root", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--timeout", type=int, default=360)
    parser.add_argument("--port", type=int, default=27683)
    parser.add_argument("--worker", action="store_true")
    parser.add_argument("--bsp", type=Path)
    parser.add_argument("--fixture-game", type=Path)
    parser.add_argument("--map-name")
    args = parser.parse_args()
    args.output = args.output.resolve()
    args.binary = args.binary.resolve(strict=True)
    args.game_dir = args.game_dir.resolve(strict=True)
    require(60 <= args.timeout <= 600 and 1024 <= args.port <= 65533, "Invalid validation time or port limit")
    if args.worker:
        require(args.bsp and args.fixture_game and args.map_name, "Private worker arguments are incomplete")
        worker(args)
        return
    require(not args.output.exists(), "Use a fresh absent validation output directory")
    require(not os.environ.get("LD_PRELOAD"), "Native validation must not inherit LD_PRELOAD")
    args.output.mkdir(parents=True)
    before = protected_files(args, ["reflections"])
    report = {"module_fingerprints": probe.module_fingerprints(args.game_dir), "success": False}
    try:
        fixture = author(args)
        report["fixture"] = fixture
        source = Path(fixture["source"])
        bsp = Path(fixture["bsp"])
        preserved = source.read_bytes(), bsp.read_bytes(), source_assets(Path(fixture["fixture_game"]))
        names = ["reflections", fixture["run"]["map"]]
        public_before = protected_files(args, names)
        require(protected_files(args, ["reflections"]) == before, "Generation changed installed source assets")
        views = {}
        for index, (name, path, namespace) in enumerate([
            ("published-view", bsp, "reflections"),
            ("run-view", args.output / "run-staged.bsp", fixture["run"]["map"]),
        ]):
            destination = args.output / name
            command = [sys.executable, "-B", str(Path(__file__).resolve()), "--worker", "--binary", str(args.binary), "--output", str(destination), "--game-dir", str(args.game_dir), "--bsp", str(path), "--fixture-game", fixture["fixture_game"], "--map-name", namespace, "--port", str(args.port + index)]
            runtime.private_compositor(command, destination, timeout=125)
            views[name] = json.loads((destination / "runtime.json").read_text())
            require(views[name]["success"] is True, views[name])
        parity = difference(ppm(args.output / "published-view/specular-on.ppm"), ppm(args.output / "run-view/specular-on.ppm"))
        require(parity["mean_component_difference"] < 3 and parity["changed_fraction"] < 0.02, f"Run rebasing visibly changed the reflective floor: {parity}")
        require((source.read_bytes(), bsp.read_bytes(), source_assets(Path(fixture["fixture_game"]))) == preserved, "Private native views changed the published BSP or source assets")
        require(protected_files(args, names) == public_before, "Private native views changed installed game assets")
        report.update({"success": True, "views": views, "run_pixel_parity": parity, "public_and_source_assets_preserved": True, "desktop_input": False})
    except BaseException as error:
        report.update({"error": str(error), "traceback": traceback.format_exc()})
        raise
    finally:
        report["initial_installed_assets_preserved"] = all(protected_files(args, ["reflections"]).get(path) == value for path, value in before.items())
        if not report["initial_installed_assets_preserved"]:
            report["success"] = False
        (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
        require(report["initial_installed_assets_preserved"], "Validation changed installed source assets")
    print(json.dumps({"success": True, "results": str(args.output / "results.json")}, indent=2))


if __name__ == "__main__":
    main()
