import argparse
import hashlib
import json
import os
import re
import resource
import shutil
import struct
import subprocess
import sys
import time
from pathlib import Path

import validate_cubemap_capture_probe as probe
import validate_native_runtime as runtime
from private_runtime_config import (
    finish_private_launch,
    private_launch_options,
    private_observer,
    private_readonly_launch,
    verify_private_launch,
)
from validate_authored_planes import records
from validate_compile_jobs import stopped_on_exit, wait_job
from validate_cubemap_reflections import ppm
from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_visibility import ROOM

SMALL_BUSH = "models/props/de_inferno/bushgreensmall.mdl"
TRAIN_BUSH = "models/props/de_train/bush.mdl"
ON_READY = "HAMMERR_DETAIL_ON_READY 2 100"
OFF_READY = "HAMMERR_DETAIL_OFF_READY 2 100"
FINISHED = "HAMMERR_DETAIL_FINISHED"
OBSERVER = """
stage <- 0;
ready_at <- 0.0;
function HammeRRThink() {
    local player = Entities.FindByClassname(null, "player");
    if (player == null) return 0.25;
    if (stage == 0) {
        SendToConsole("sv_cheats 1; sv_lan 1; joystick 0; bot_quota 0; bot_kick; jointeam 2; joinclass 1; hidepanel info; hidepanel team; hidepanel class_ter; hidepanel class_ct; r_drawvgui 0; gameui_hide; cl_drawhud 0; r_drawviewmodel 0; r_drawothermodels 0; mat_fullbright 0; toggle con_notifytime 0; toggle r_drawdetailprops 1; help r_drawdetailprops");
        ready_at = Time() + 6.0;
        stage = 1;
    }
    if (stage == 1 && Time() > ready_at && player.GetTeam() == 2 && player.GetHealth() == 100) {
        SendToConsole("noclip; god; setpos 0 -192 64; setang 10 90 0; sv_lan; mat_hdr_level; mat_fullbright");
        ready_at = Time() + 4.0;
        stage = 2;
    }
    if (stage >= 2) {
        player.SetOrigin(Vector(0, -192, 64) - (player.EyePosition() - player.GetOrigin()));
        player.SetVelocity(Vector(0, 0, 0));
    }
    if (stage == 2 && Time() > ready_at) {
        printl("HAMMERR_DETAIL_ON_READY " + player.GetTeam() + " " + player.GetHealth());
        stage = 3;
    }
    if (stage == 4 && Time() > ready_at) {
        printl("HAMMERR_DETAIL_OFF_READY " + player.GetTeam() + " " + player.GetHealth());
        stage = 5;
    }
    if (stage == 3 || stage == 5) SendToConsole("exec hammerr_detail_once.cfg");
    return 0.25;
}
"""


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def detail_records(data, allow_sprites=False, allow_baked=False, sprite_orientation=2):
    offset, size = struct.unpack_from("<2i", data, 8 + 35 * 16)
    require(0 <= offset <= offset + size <= len(data) and size >= 4, "Invalid game-lump directory")
    count = struct.unpack_from("<i", data, offset)[0]
    require(0 <= count <= 64 and 4 + count * 16 <= size, "Invalid game-lump count")
    entries = [struct.unpack_from("<IHHii", data, offset + 4 + index * 16) for index in range(count)]
    matches = [row for row in entries if row[0] == int.from_bytes(b"dprp", "big")]
    require(len(matches) == 1 and matches[0][1:3] == (0, 4), "Expected one uncompressed dprp v4 lump")
    cursor, length = matches[0][3:]
    end = cursor + length
    require(0 <= cursor <= end <= len(data) and length >= 12, "Invalid dprp span")
    payload = data[cursor:end]
    models = struct.unpack_from("<i", data, cursor)[0]
    cursor += 4
    require(0 <= models <= 1024 and cursor + models * 128 + 8 <= end, "Invalid detail model dictionary")
    names = []
    for index in range(models):
        name = data[cursor + index * 128:cursor + (index + 1) * 128]
        require(b"\0" in name, "Unterminated detail model path")
        names.append(name.split(b"\0", 1)[0].decode("ascii"))
    cursor += models * 128
    sprites = struct.unpack_from("<i", data, cursor)[0]
    cursor += 4
    require(0 <= sprites <= 4096 and cursor + sprites * 32 + 4 <= end, "Invalid detail sprite dictionary")
    require(allow_sprites or sprites == 0, "Explicit model fixture unexpectedly emitted sprites")
    sprite_records = [list(struct.unpack_from("<8f", data, cursor + index * 32)) for index in range(sprites)]
    cursor += sprites * 32
    count = struct.unpack_from("<i", data, cursor)[0]
    cursor += 4
    require(0 <= count <= 16384 and cursor + count * 52 == end, "Invalid detail prop records")
    props = []
    for index in range(count):
        row = struct.unpack_from("<6fHH4BI12Bf", data, cursor + index * 52)
        require(allow_baked or row[8:12] == (255, 255, 255, 0), "Detail lighting must be explicit unbaked white")
        record = {"origin": list(row[:3]), "angles": list(row[3:6]), "leaf": row[7]}
        if allow_baked:
            record["lighting"] = [*row[8:11], row[11] if row[11] < 128 else row[11] - 256]
        if row[21] == 0:
            require(row[6] < len(names), "Invalid detail model index")
            require(row[12:25] == (0,) * 13 and row[25] == 1, "Unexpected model styles, orientation, padding or scale")
            record["model"] = names[row[6]]
        else:
            require(allow_sprites and row[21] == 1 and row[6] < len(sprite_records), "Invalid detail sprite type or dictionary index")
            require(row[12:17] == (0,) * 5 and row[17] == sprite_orientation and row[18:21] == (0,) * 3 and row[22:25] == (0,) * 3, "Unexpected sprite styles, sway, shape, orientation or padding")
            require(0 < row[25] <= 6, "Invalid detail sprite scale")
            record.update({"kind": 1, "sprite": row[6], "scale": row[25], "orientation": row[17]})
        props.append(record)
    require([row["leaf"] for row in props] == sorted(row["leaf"] for row in props), "Detail records are not sorted by leaf")
    return {"models": names, "sprites": sprite_records, "props": props, "payload_sha256": hashlib.sha256(payload).hexdigest()}


def snapshot(client, ids):
    return detail(client, ids), client.call("get_map_info")["undo"]


def compile_pair(client, output, name, ids):
    before = snapshot(client, ids)
    path = output / f"{name}.bsp"
    result = client.call("compile_map", {"path": str(path)})
    require(result["success"] and result["leak"] is None, result)
    data = probe.read_bounded(path, probe.MAX_BSP_BYTES)
    background = output / f"{name}-background.bsp"
    client.call("start_compile", {"path": str(background)})
    job = wait_job(client)
    require(job["status"] == "completed" and background.read_bytes() == data, job)
    require(snapshot(client, ids) == before, "Compiling changed source objects or Undo history")
    return path, data, result


def validate(args):
    client = Client(None, args.binary, args.output, gameinfo=args.game_dir / "cstrike/gameinfo.txt")
    try:
        with stopped_on_exit(client):
            client.call("load_entity_definitions", {"path": str(args.fgd)})
            definition = client.call("get_entity_class", {"classname": "prop_detail"})
            require(any(row["name"] == "model" and row["label"] == "World model" for row in definition["properties"]), definition)
            ids = [client.call("create_block", {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"})["id"] for low, high in ROOM]
            for classname, origin, properties in [
                ("info_player_terrorist", [-192, -176, 8], {}),
                ("info_player_counterterrorist", [192, 176, 8], {}),
                ("light", [0, 0, 180], {"_light": "255 255 255 180", "_constant_attn": "1", "_linear_attn": "0", "_quadratic_attn": "0"}),
                ("logic_script", [0, -176, 64], {"targetname": "hammerr_detail_observer", "vscripts": "hammerr_detail_observer.nut", "thinkfunction": "HammeRRThink"}),
            ]:
                ids.append(client.call("create_entity", {"classname": classname, "origin": origin, "properties": properties})["id"])
            client.call("set_compile_settings", {"lighting": "direct_ldr", "physics": "none", "bounces": 0})
            _, empty, empty_result = compile_pair(client, args.output, "empty", ids)
            empty_records = detail_records(empty)
            require(not empty_records["props"] and not empty_records["models"] and empty_result["stats"]["detail_props"] == 0, "Empty control emitted details")
            require(empty_records["payload_sha256"] == probe.digest(bytes(12)), "Empty control changed the established dprp placeholder")
            expected = [
                {"origin": [-64.125, 32.25, 20.375], "angles": [0.0, 37.5, 0.0], "model": SMALL_BUSH},
                {"origin": [64.0, 32.0, 0.375], "angles": [0.0, 0.0, 0.0], "model": TRAIN_BUSH},
                {"origin": [0.0, 112.0, 20.375], "angles": [0.0, 0.0, 0.0], "model": SMALL_BUSH},
            ]
            for index, row in enumerate(expected):
                properties = {"model": row["model"]}
                if index == 0:
                    properties["angles"] = "0 397.5 0"
                if index == 2:
                    properties.update({"skin": "0", "body": "0", "scale": "1", "modelscale": "1", "detailOrientation": "0", "rendercolor": "255 255 255", "renderamt": "255", "rendermode": "0", "renderfx": "0", "spawnflags": "0", "solid": "0", "disableshadows": "0", "fademindist": "-1", "fademaxdist": "0", "fadescale": "1"})
                ids.append(client.call("create_entity", {"classname": "detail_prop" if index == 1 else "prop_detail", "origin": row["origin"], "properties": properties})["id"])
            hidden = client.call("create_entity", {"classname": "prop_detail", "origin": [0, 0, 32], "properties": {"model": "models/hammerr_missing_detail.mdl", "scale": "2"}})["id"]
            ids.append(hidden)
            client.call("set_object_visibility", {"ids": [hidden], "action": "hide"})
            source = args.output / "detail-props.vmf"
            client.call("save_map", {"path": str(source)})
            authored = detail(client, ids)
            saved = source.read_bytes()
            client.call("open_map", {"path": str(source)})
            require(detail(client, ids) == authored, "VMF reopen changed authored detail entities")
            bsp, data, result = compile_pair(client, args.output, "detail-props", ids)
            parsed = detail_records(data, allow_baked=True)
            require(len(parsed["props"]) == result["stats"]["detail_props"] == 3, parsed)
            require(len(parsed["models"]) == result["stats"]["detail_prop_models"] == 2, parsed)
            require(result["stats"]["static_props"] == 0, "Detail fixture unexpectedly emitted static props")
            actual = [{key: row[key] for key in ["origin", "angles", "model"]} for row in parsed["props"]]
            require(sorted(actual, key=lambda row: row["origin"]) == sorted(expected, key=lambda row: row["origin"]), parsed)
            leaves = records(data, 10, "<ihh6h4HhH", expected_version=1)
            require(all(row["leaf"] < len(leaves) and leaves[row["leaf"]][0] & 1 == 0 for row in parsed["props"]), "Detail origins did not map to open BSP leaves")
            entity_text = probe.bsp_lumps(data)[0][2]
            require(b'"prop_detail"' not in entity_text and b'"detail_prop"' not in entity_text, "Compiled detail entities leaked into runtime entity text")
            diagnostic_text = json.dumps(result["diagnostics"])
            require("unbaked white lighting" in diagnostic_text and "non-colliding" in diagnostic_text, result["diagnostics"])
            require(source.read_bytes() == saved, "Compilation changed saved VMF bytes")
            rejected = {}
            destination = args.output / "rejected.bsp"
            destination.write_bytes(data)
            for name, classname, origin, changes in [
                ("missing-model", "prop_detail", [0, 0, 32], {"model": "models/hammerr_missing_detail.mdl"}),
                ("unsafe-model", "prop_detail", [0, 0, 32], {"model": "models/../hammerr_missing_detail.mdl"}),
                ("skin", "prop_detail", [0, 0, 32], {"skin": "1"}),
                ("scale", "prop_detail", [0, 0, 32], {"modelscale": "2"}),
                ("orientation", "prop_detail", [0, 0, 32], {"detailOrientation": "1"}),
                ("vertex-lit-material", "prop_detail", [0, 0, 32], {"model": "models/props_junk/wood_crate001a.mdl"}),
                ("solid-origin", "prop_detail", [0, 0, -8], {}),
                ("sprite", "prop_detail_sprite", [0, 0, 32], {}),
            ]:
                invalid = client.call("create_entity", {"classname": classname, "origin": origin, "properties": {"model": SMALL_BUSH, **changes}})["id"]
                before = snapshot(client, [*ids, invalid])
                response = client.call("compile_map", {"path": str(destination)}, error=True)
                failure = json.loads(response["content"][0]["text"])
                require(not failure["success"] and any(row.get("object") == invalid for row in failure["diagnostics"]), failure)
                require(destination.read_bytes() == data and snapshot(client, [*ids, invalid]) == before, "Rejected compile changed source/history or replaced the previous BSP")
                client.call("start_compile", {"path": str(destination)})
                job = wait_job(client)
                require(job["status"] == "failed" and destination.read_bytes() == data and snapshot(client, [*ids, invalid]) == before, job)
                rejected[name] = failure["diagnostics"]
                client.call("undo")
                require(detail(client, ids) == authored, "Undo failed to restore the fixture after a rejection")
            require(source.read_bytes() == saved and bsp.read_bytes() == data, "Rejection controls changed the published fixture")
            report = {"success": True, "stats": result["stats"], "records": parsed, "empty_stats": empty_result["stats"], "rejected": rejected, "saved_reopened": True, "foreground_background_exact": True, "source_history_preserved": True, "hidden_invalid_omitted": True, "native_exercised": False, "bsp": str(bsp), "bsp_sha256": probe.digest(data)}
            (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
            return bsp
    finally:
        client.close()


def frame_difference(first, second):
    region = (160, 180, 1120, 650)
    left, top, right, bottom = region
    delta = []
    for y in range(top, bottom):
        start, end = (y * probe.WIDTH + left) * 3, (y * probe.WIDTH + right) * 3
        delta.extend(abs(a - b) for a, b in zip(first[start:end], second[start:end], strict=True))
    pixels = len(delta) // 3
    return {"region": region, "mean_component_difference": sum(delta) / len(delta), "changed_fraction": sum(max(delta[index:index + 3]) > 12 for index in range(0, len(delta), 3)) / pixels}


def worker(args):
    output = args.output
    require(not os.environ.get("LD_PRELOAD"), "Native validation must not inherit LD_PRELOAD")
    require(os.environ.get("WAYLAND_DISPLAY") and os.environ.get("XDG_RUNTIME_DIR") == str(output / "wayland"), "Native view requires its private compositor")
    original = probe.read_bounded(args.bsp, probe.MAX_BSP_BYTES)
    parsed = detail_records(original, allow_sprites=args.expected_sprites > 0, allow_baked=args.baked_lighting, sprite_orientation=args.sprite_orientation)
    require(len(parsed["props"]) == args.expected_count, "Native input has an unexpected detail count")
    require(sum(row.get("kind") == 1 for row in parsed["props"]) == args.expected_sprites, "Native input has an unexpected sprite count")
    game = runtime.private_game(args.game_dir, output)
    if args.fixture_game is not None:
        shutil.copytree(args.fixture_game / "materials", game / "materials")
    staged = game / "maps/hammerr_detail_props.bsp"
    staged.write_bytes(original)
    observer = OBSERVER
    if args.view == "elevated":
        observer = observer.replace("0 -192 64", "0 -144 176").replace("10 90 0", "44 90 0").replace("Vector(0, -192, 64)", "Vector(0, -144, 176)")
    (game / "scripts/vscripts/hammerr_detail_observer.nut").write_text(private_observer(observer))
    cfg = game / "cfg/hammerr_detail_once.cfg"
    probe.write_cfg(cfg, "echo HAMMERR_DETAIL_WAIT\n")
    post_map = "toggle sv_lan 1\ntoggle mat_fullbright 0\nhelp sv_lan\nhelp mat_hdr_level\nhelp mat_fullbright\n"
    for name in ["server.cfg", "listenserver.cfg", "hammerr_detail_postmap.cfg"]:
        (game / "cfg" / name).write_text(post_map)
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    settings = output / "settings"
    for name in ["config", "cache", "data"]:
        (settings / name).mkdir(parents=True)
    environment = {**os.environ, "LD_LIBRARY_PATH": str(args.game_dir / "bin/linux64"), "SteamAppId": "240", "SteamGameId": "240", "SDL_VIDEODRIVER": "x11", "XDG_CONFIG_HOME": str(settings / "config"), "XDG_CACHE_HOME": str(settings / "cache"), "XDG_DATA_HOME": str(settings / "data")}
    flags = private_launch_options(args.game_dir, game, hdr_level=0, map_name="hammerr_detail_props", log_name="hammerr-detail.log", controls={"servercfgfile": "hammerr_detail_postmap.cfg", "mat_fullbright": 0})
    native = [str(args.game_dir / "cstrike_linux64"), "-game", str(game), "-windowed", "-w", str(probe.WIDTH), "-h", str(probe.HEIGHT), "-novid", *flags, "-nomaster", "-nojoy", "-console", "-port", str(args.port)]
    command, environment = private_readonly_launch(native, output, environment)
    report = {"success": False, "command": command, "bsp_sha256": probe.digest(original), "exact_compiled_bsp": True, "sprite_orientation": args.sprite_orientation, "view": args.view, "module_fingerprints": probe.module_fingerprints(args.game_dir), "filesystem_isolation": {"external_paths": "read_only", "writable_directory": str(output)}, "desktop_input": False}
    console = game / "hammerr-detail.log"
    process = None
    try:
        with (output / "launcher.log").open("w") as stream:
            process = subprocess.Popen(command, cwd=args.game_dir, env=environment, stdin=subprocess.DEVNULL, stdout=stream, stderr=stream)
            started = time.monotonic()
            on = off = False
            while time.monotonic() - started < 80:
                text = probe.read_bounded(console, 2 * 1024**2).decode(errors="replace") if console.exists() else ""
                if not on and ON_READY in text:
                    report["private_launch"] = verify_private_launch(game, text)
                    require(re.search(r'"r_drawdetailprops" = "1"', text, re.IGNORECASE), "Engine did not confirm detail rendering ON")
                    for extension in ["png", "ppm"]:
                        subprocess.run(["grim", *(["-t", "ppm"] if extension == "ppm" else []), str(output / f"details-on.{extension}")], check=True, timeout=5)
                    ppm(output / "details-on.ppm")
                    probe.write_cfg(cfg, 'toggle r_drawdetailprops 0\nhelp r_drawdetailprops\nent_fire hammerr_detail_observer RunScriptCode "stage = 4; ready_at = Time() + 4"\n')
                    on = True
                if on and not off and OFF_READY in text:
                    require(re.search(r'"r_drawdetailprops" = "0"', text, re.IGNORECASE), "Engine did not confirm detail rendering OFF")
                    for extension in ["png", "ppm"]:
                        subprocess.run(["grim", *(["-t", "ppm"] if extension == "ppm" else []), str(output / f"details-off.{extension}")], check=True, timeout=5)
                    off = True
                    probe.write_cfg(cfg, f"echo {FINISHED}\ndisconnect\nquit\n")
                if process.poll() is not None:
                    break
                time.sleep(0.02)
            else:
                raise TimeoutError("Native detail frame handshake exceeded 80 seconds")
            text = probe.read_bounded(console, 2 * 1024**2).decode(errors="replace")
            require(process.returncode == 0 and on and off and FINISHED in text, f"Native detail validation failed; inspect {console}")
            require(text.index(ON_READY) < text.index(OFF_READY) < text.index(FINISHED), "Frames were captured outside the ordered handshake")
            report["private_launch"] = verify_private_launch(game, text)
            for name, expected in {"sv_lan": 1, "mat_hdr_level": 0, "mat_fullbright": 0, "r_drawdetailprops": 0}.items():
                values = re.findall(r'"' + re.escape(name) + r'" = "([^"\n]+)"', text, re.IGNORECASE)
                require(values and float(values[-1]) == expected, f"Native {name} control differs: {values}")
            require(not re.search(r'(?:error|failed|not found|couldn.t)[^\n]*(?:\.mdl|detail prop)|Detail prop model[^\n]*using vertex-lit', text, re.IGNORECASE), "Engine reported a detail asset failure or substituted an error model")
            delta = frame_difference(ppm(output / "details-on.ppm"), ppm(output / "details-off.ppm"))
            require(delta["mean_component_difference"] > 0.5 and delta["changed_fraction"] > 0.002, f"No measurable detail model rendering: {delta}")
            require(staged.read_bytes() == original and args.bsp.read_bytes() == original, "Native validation changed the compiled BSP")
            report.update({"success": True, "detail_pixels": delta, "unbaked_white_lighting": not args.baked_lighting, "elapsed_seconds": time.monotonic() - started})
    except BaseException as error:
        report["error"] = str(error)
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
            report["preservation"] = finish_private_launch(game)
        except BaseException as error:
            report.update({"success": False, "preservation_error": str(error)})
            raise
        finally:
            (output / "runtime.json").write_text(json.dumps(report, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--fgd", type=Path, default=Path.home() / ".local/share/Steam/steamapps/common/SourceSDK/bin/source2009/bin/cstrike.fgd")
    parser.add_argument("--game-dir", type=Path, default=Path.home() / ".local/share/Steam/steamapps/common/Counter-Strike Source")
    parser.add_argument("--runtime", action="store_true")
    parser.add_argument("--runtime-bsp", type=Path, help="Render an existing checked detail BSP instead of creating the explicit fixture")
    parser.add_argument("--fixture-game", type=Path, help="Copy this private fixture's materials for an existing BSP")
    parser.add_argument("--expected-count", type=int, default=3)
    parser.add_argument("--expected-sprites", type=int, default=0)
    parser.add_argument("--baked-lighting", action="store_true", help="Permit actual compiler-baked detail base colors; the default requires white records")
    parser.add_argument("--sprite-orientation", type=int, choices=[0, 1, 2], default=2)
    parser.add_argument("--view", choices=["level", "elevated"], default="level")
    parser.add_argument("--port", type=int, default=27689)
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--bsp", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    args.output = args.output.resolve()
    args.binary = args.binary.resolve(strict=True)
    args.game_dir = args.game_dir.resolve(strict=True)
    require(1024 <= args.port <= 65533, "Invalid native port")
    require(1 <= args.expected_count <= 16384, "Invalid expected detail count")
    require(0 <= args.expected_sprites <= args.expected_count, "Invalid expected sprite count")
    if args.worker:
        require(args.bsp is not None, "Private worker requires the compiled BSP")
        worker(args)
        return
    require(not args.output.exists(), "Use a fresh absent output directory")
    args.output.mkdir(parents=True, mode=0o700)
    if args.runtime_bsp is None:
        require(args.fixture_game is None and args.expected_sprites == 0, "Fixture materials and sprites require --runtime-bsp")
        bsp = validate(args)
        args.baked_lighting = True
    else:
        require(args.runtime, "--runtime-bsp requires --runtime")
        bsp = args.runtime_bsp.resolve(strict=True)
        data = probe.read_bounded(bsp, probe.MAX_BSP_BYTES)
        parsed = detail_records(data, allow_sprites=args.expected_sprites > 0, allow_baked=args.baked_lighting, sprite_orientation=args.sprite_orientation)
        require(len(parsed["props"]) == args.expected_count, "Existing detail BSP count differs")
        require(sum(row.get("kind") == 1 for row in parsed["props"]) == args.expected_sprites, "Existing detail BSP sprite count differs")
        report = {"success": True, "native_exercised": False, "records": parsed, "bsp": str(bsp), "bsp_sha256": probe.digest(data), "source_validation": "external BSP; authored source checks are not performed in this mode"}
        (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    if args.runtime:
        output = args.output / "runtime"
        command = [sys.executable, "-B", str(Path(__file__).resolve()), "--worker", "--binary", str(args.binary), "--output", str(output), "--game-dir", str(args.game_dir), "--bsp", str(bsp), "--port", str(args.port), "--expected-count", str(args.expected_count), "--expected-sprites", str(args.expected_sprites), "--sprite-orientation", str(args.sprite_orientation), "--view", args.view]
        if args.baked_lighting:
            command.append("--baked-lighting")
        if args.fixture_game is not None:
            command += ["--fixture-game", str(args.fixture_game.resolve(strict=True))]
        runtime.private_compositor(command, output, timeout=110)
        report = json.loads((args.output / "results.json").read_text())
        report["native_exercised"] = True
        report["native"] = json.loads((output / "runtime.json").read_text())
        require(report["native"]["success"], report["native"])
        (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"success": True, "results": str(args.output / "results.json"), "native_exercised": args.runtime}))


if __name__ == "__main__":
    main()
