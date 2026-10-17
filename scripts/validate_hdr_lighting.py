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

import validate_native_runtime as runtime
from hdr_observer import (
    ENGINE_SHA256,
    HDR_CONTROLS,
    probe_environment,
    read_probe,
    verify_hardware,
)
from validate_bump_lighting import texture
from validate_compile_jobs import wait_job
from validate_direct_lighting import FACE_FORMAT
from validate_face_lighting import DEFAULT_BINARY, ROOT, Client, detail, face_ref
from validate_visibility import ROOM

CHANNELS = {"ldr": (7, 8, 15, 52, 56), "hdr": (58, 53, 54, 51, 55)}
CONTROLS = {**HDR_CONTROLS, "mat_fullbright": 0, "sv_lan": 1}
OBSERVER = """
stage <- 0;
ready_at <- 0.0;
function HammeRRThink() {
    local player = Entities.FindByClassname(null, "player");
    if (player == null) return 0.5;
    if (stage == 0) {
        SendToConsole("sv_cheats 1; sv_lan 1; joystick 0; bot_quota 0; bot_kick; jointeam 2; joinclass 1; hidepanel info; hidepanel team; hidepanel class_ter; hidepanel class_ct; r_drawvgui 0; gameui_hide; cl_drawhud 0; r_drawviewmodel 0; r_drawothermodels 0; mat_fullbright 0; mat_force_tonemap_scale 0.25; mat_autoexposure_min 0.25; mat_autoexposure_max 0.25; mat_disable_bloom 1; mat_bloomscale 0");
        ready_at = Time() + 6.0;
        stage = 1;
    }
    if (stage == 1 && Time() > ready_at && player.GetTeam() == 2 && player.GetHealth() == 100) {
        SendToConsole("noclip; setpos -192 -176 48; setang 35 42 0");
        ready_at = Time() + 3.0;
        stage = 2;
    }
    if (stage == 2 && Time() > ready_at) {
        SendToConsole("mat_hdr_level; mat_force_tonemap_scale; mat_autoexposure_min; mat_autoexposure_max; mat_disable_bloom; mat_bloomscale; mat_fullbright; sv_lan; echo HAMMERR_HDR_READY " + player.GetTeam() + " " + player.GetHealth());
        stage = 3;
    }
    return 0.5;
}
"""


def lump(data, index, versions=(0,)):
    assert data[:4] == b"VBSP" and len(data) >= 1036
    offset, size, version, compression = struct.unpack_from("<4i", data, 8 + index * 16)
    assert version in versions and compression == 0, (index, version, compression)
    assert 0 <= offset <= offset + size <= len(data), (index, offset, size)
    return data[offset:offset + size]


def array(data, index, layout, versions=(0,), maximum=65536):
    payload = lump(data, index, versions)
    stride = struct.calcsize(layout)
    assert len(payload) % stride == 0 and len(payload) // stride <= maximum
    return list(struct.iter_unpack(layout, payload))


def rgbexp(data):
    assert len(data) % 4 == 0
    return [[math.ldexp(component, exponent) for component in rgb]
            for *rgb, exponent in struct.iter_unpack("<3Bb", data)]


def decode_channel(data, kind):
    face_lump, sample_lump, world_lump, index_lump, ambient_lump = CHANNELS[kind]
    faces = array(data, face_lump, FACE_FORMAT, (0,) if kind == "ldr" else (1,))
    samples = lump(data, sample_lump, (1,))
    assert 0 < len(samples) <= 16 * 1024**2
    infos = array(data, 6, "<16f2i")
    planes = array(data, 1, "<4fi")
    floor = []
    lit_faces = 0
    sample_count = 0
    styled_faces = 0
    all_styles = set()
    for index, face in enumerate(faces):
        if face[12] < 0:
            continue
        lit_faces += 1
        styles = list(face[8:12])
        active = [style for style in styles if style != 255]
        assert styles == active + [255] * (4 - len(active)) and active[0] == 0
        assert len(active) == len(set(active))
        all_styles.update(active)
        styled_faces += len(active) > 1
        count = (face[16] + 1) * (face[17] + 1)
        sample_count += count
        maps = 4 if infos[face[5]][16] & 0x800 else 1
        assert count > 0 and face[12] % 4 == 0
        assert face[12] >= len(active) * 4
        assert face[12] + count * maps * len(active) * 4 <= len(samples)
        decoded = []
        for style_index, style in enumerate(active):
            directions = []
            for direction in range(maps):
                offset = face[12] + (style_index * maps + direction) * count * 4
                directions.append(rgbexp(samples[offset:offset + count * 4]))
            average_offset = face[12] - (style_index + 1) * 4
            average = rgbexp(samples[average_offset:average_offset + 4])[0]
            measured = [sum(color[axis] for color in directions[0]) / count for axis in range(3)]
            assert all(abs(a - b) <= max(2, abs(a) / 64) for a, b in zip(average, measured)), (kind, index, style, average, measured)
            decoded.append({"style": style, "average": average,
                            "maximum": [max(color[axis] for color in directions[0]) for axis in range(3)],
                            "maps": maps})
        plane = planes[face[0]]
        if plane[2] > 0.99 and abs(plane[3]) < 0.001:
            floor.append({"face": index, "styles": active, "offset": face[12], "channels": decoded})
    assert floor and lit_faces
    world = array(data, world_lump, "<9f3i7f3i", maximum=8192)
    assert world and all(all(math.isfinite(value) and value >= 0 for value in light[3:6]) for light in world)
    indices = array(data, index_lump, "<2H")
    ambient = lump(data, ambient_lump, (1,))
    assert ambient and len(ambient) % 28 == 0 and len(ambient) // 28 <= 65536
    leaves = array(data, 10, "<ihh6h4Hhh", (1,))
    assert len(indices) == len(leaves)
    for leaf, (count, first) in enumerate(indices):
        if count:
            assert first + count <= len(ambient) // 28
            continue
        seen = {leaf}
        while not count:
            assert first < len(indices)
            if first in seen:
                assert indices[first] == (0, first)
                break
            seen.add(first)
            count, first = indices[first]
        if count:
            assert first + count <= len(ambient) // 28
    cubes = [rgbexp(ambient[offset:offset + 24]) for offset in range(0, len(ambient), 28)]
    return {"faces": faces, "floor": floor, "lighting_faces": lit_faces,
            "lightmap_samples": sample_count, "lighting_bytes": len(samples),
            "world_lights": len(world), "world_intensities": [list(light[3:6]) for light in world],
            "world_styles": [light[11] for light in world], "styles": sorted(all_styles),
            "light_styles": len(all_styles), "styled_faces": styled_faces,
            "ambient_samples": len(cubes), "ambient_bytes": len(ambient) + len(indices) * 4,
            "ambient_maximum": [max(color[axis] for cube in cubes for color in cube) for axis in range(3)]}


def check_channels(path, stats=None):
    data = path.read_bytes()
    channels = {kind: decode_channel(data, kind) for kind in CHANNELS}
    ldr, hdr = channels["ldr"], channels["hdr"]
    assert len(ldr["faces"]) == len(hdr["faces"])
    assert all(a[:8] + a[13:] == b[:8] + b[13:] for a, b in zip(ldr["faces"], hdr["faces"], strict=True))
    assert ldr["styles"] == [0] and hdr["styles"] == [0, 32], channels
    assert any(a[8:13] != b[8:13] for a, b in zip(ldr["faces"], hdr["faces"]))
    for kind, base_axis in [("ldr", 0), ("hdr", 2)]:
        channel = channels[kind]
        floor_base = [face["channels"][0] for face in channel["floor"]]
        assert all(row["average"][base_axis] > 0 for row in floor_base)
        assert all(row["average"][2 - base_axis] == 0 for row in floor_base)
        assert any(row["maps"] == 4 for row in floor_base)
        assert channel["ambient_maximum"][base_axis] > 0 and channel["ambient_maximum"][2 - base_axis] == 0
        assert any(light[base_axis] > 0 for light in channel["world_intensities"])
        assert all(light[2 - base_axis] == 0 for light in channel["world_intensities"])
    assert max(face["channels"][0]["maximum"][2] for face in hdr["floor"]) > 255
    assert any(row["style"] == 32 and row["average"][1] > 0 for face in hdr["floor"] for row in face["channels"])
    if stats is not None:
        assert stats["hdr"]["map_data_present"] is True
        for kind, counters in [("ldr", stats), ("hdr", stats["hdr"])]:
            for key in ["lighting_faces", "lightmap_samples", "lighting_bytes", "world_lights", "light_styles", "styled_faces", "ambient_samples", "ambient_bytes"]:
                assert counters[key] == channels[kind][key], (kind, key, counters[key], channels[kind][key])
        assert 0 < stats["lighting_total_work"] <= 100_000_000
        assert stats["lighting_total_work"] >= stats["lighting_work"] + stats["hdr"]["lighting_work"]
    for channel in channels.values():
        del channel["faces"]
    return channels


def assets(args):
    game = runtime.private_game(args.game_dir.resolve(), args.output / "source-game", engine_physics=True)
    directory = game / "materials/hammerr"
    directory.mkdir(parents=True, exist_ok=True)
    (directory / "hdr_white.vtf").write_bytes(texture(bytes([255] * 32 * 32 * 4)))
    (directory / "hdr_normal.vtf").write_bytes(texture(bytes([128, 128, 255, 255] * 32 * 32), normal=True))
    (directory / "hdr_receiver.vmt").write_text('LightmappedGeneric { $basetexture "hammerr/hdr_white" $bumpmap "hammerr/hdr_normal" $surfaceprop concrete }\n')
    return game


def fixture(args):
    game = assets(args)
    client = Client(None, args.binary, args.output, game / "gameinfo.txt")
    try:
        ids = []
        for low, high in ROOM:
            ids.append(client.call("create_block", {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"})["id"])
        floor = detail(client, [ids[0]])[0]
        top = next(face for face in floor["faces"] if face["normal"] == [0, 0, 1])
        client.call("edit_faces", {"faces": [face_ref(floor, top)], "material": "hammerr/hdr_receiver", "lightmap_scale": 16})
        lights = []
        for classname, origin, properties in [
            ("info_player_terrorist", [-192, -176, 8], {}),
            ("info_player_counterterrorist", [192, 176, 8], {}),
            ("light", [0, 0, 180], {"_light": "255 0 0 80", "_lightHDR": "0 0 255 1600", "_constant_attn": "1", "_linear_attn": "0", "_quadratic_attn": "0"}),
            ("light", [96, 0, 180], {"targetname": "hdr_switch", "spawnflags": "1", "_light": "0 0 0 0", "_lightHDR": "0 255 0 160", "_constant_attn": "1", "_linear_attn": "0", "_quadratic_attn": "0"}),
            ("logic_script", [-192, -176, 64], {"targetname": "hammerr_observer", "vscripts": "hammerr_observer.nut", "thinkfunction": "HammeRRThink"}),
        ]:
            entity = client.call("create_entity", {"classname": classname, "origin": origin, "properties": properties})["id"]
            ids.append(entity)
            if classname == "light":
                lights.append(entity)
        source = args.output / "hdr.vmf"
        client.call("save_map", {"path": str(source)})
        before = source.read_bytes(), detail(client, ids), client.call("get_map_info")["undo"]
        client.call("set_compile_settings", {"lighting": "direct_ldr", "physics": "none", "bounces": 1})
        baseline = args.output / "ldr.bsp"
        ldr_result = client.call("compile_map", {"path": str(baseline)})
        assert ldr_result["success"] and ldr_result["stats"].get("hdr") is None, ldr_result
        selected = client.call("set_compile_settings", {"lighting": "direct_ldr_hdr"})
        assert client.call("get_compile_settings")["lighting"] == "direct_ldr_hdr", selected
        path = args.output / "hdr.bsp"
        result = client.call("compile_map", {"path": str(path)})
        assert result["success"], result
        channels = check_channels(path, result["stats"])
        for index in CHANNELS["ldr"]:
            versions = (0, 1)
            assert lump(baseline.read_bytes(), index, versions) == lump(path.read_bytes(), index, versions), index
        background = args.output / "background.bsp"
        client.call("start_compile", {"path": str(background)})
        job = wait_job(client)
        assert job["status"] == "completed" and background.read_bytes() == path.read_bytes(), job
        assert (source.read_bytes(), detail(client, ids), client.call("get_map_info")["undo"]) == before
        published = path.read_bytes()
        client.call("set_entity_properties", {"id": lights[0], "set": {"_lightHDR": "invalid HDR color"}}, error=True)
        assert detail(client, ids) == before[1]
        assert client.call("get_map_info")["undo"] == before[2]
        client.call("set_entity_properties", {"id": lights[0], "set": {"_lightscaleHDR": "-1"}})
        client.call("compile_map", {"path": str(path)}, error=True)
        assert path.read_bytes() == published
        report = {"stats": result["stats"], "channels": channels,
                  "ldr_channel_bytes_preserved": True, "background_bytes_identical": True,
                  "compile_preserves_source_and_history": True, "failed_hdr_preserves_output": True,
                  "installed_compatibility_material": "dev/dev_measuregeneric01",
                  "fixture_game": str(game), "bsp": str(path)}
        (args.output / "compile.json").write_text(json.dumps(report, indent=2) + "\n")
        return path, game
    finally:
        client.close()




def verify_controls(text, level):
    values = {}
    for name, expected in {**CONTROLS, "mat_hdr_level": level}.items():
        matches = re.findall(r'"' + re.escape(name) + r'" = "([^"]+)"', text)
        assert matches and float(matches[-1]) == expected, (name, expected, matches)
        values[name] = float(matches[-1])
    assert "HAMMERR_HDR_READY 2 100" in text
    assert "Level unlit" not in text
    return values


def worker(args):
    root = args.game_dir.resolve()
    output = args.output.resolve()
    assert not os.environ.get("LD_PRELOAD"), "Run the private worker without an inherited LD_PRELOAD"
    assert os.environ.get("WAYLAND_DISPLAY") and os.environ.get("XDG_RUNTIME_DIR") == str(output / "wayland")
    engine = root / "bin/linux64/engine.so"
    assert hashlib.sha256(engine.read_bytes()).hexdigest() == ENGINE_SHA256, "Playable engine changed; re-verify its hardware interface before querying"
    game = runtime.private_game(root, output, args.bsp)
    shutil.copytree(args.fixture_game / "materials", game / "materials", dirs_exist_ok=True)
    (game / "scripts/vscripts/hammerr_observer.nut").write_text(runtime.private_observer(OBSERVER))
    launch_options = runtime.private_launch_options(root, game, hdr_level=args.hdr_level, controls=CONTROLS)
    ready = output / "probe-ready"
    probe_log = output / "hardware.jsonl"
    assert not ready.exists() and not probe_log.exists(), "Use a fresh runtime output directory"
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    environment = {**os.environ, "LD_LIBRARY_PATH": str(root / "bin/linux64"), "SteamAppId": "240", "SteamGameId": "240", "SDL_VIDEODRIVER": "x11", **probe_environment(root, game, args.probe_library, ready, probe_log)}
    command = [str(root / "cstrike_linux64"), "-game", str(game), "-windowed", "-w", "1280", "-h", "720",
               "-novid", *launch_options, "-nomaster", "-nojoy", "-console", "-port", str(27584 + args.hdr_level)]
    result = {}
    try:
        with runtime.private_native_process(command, root, game, output, environment, result) as process:
            deadline = time.monotonic() + 48
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    raise RuntimeError(f"Private CS:S exited with {process.returncode}; inspect {output}")
                console = game / "hammerr-runtime.log"
                text = console.read_text(errors="replace") if console.exists() else ""
                if not ready.exists() and "HAMMERR_HDR_READY 2 100" in text:
                    verify_controls(text, args.hdr_level)
                    ready.write_text("post-map observer and queried controls verified\n")
                records = read_probe(probe_log)
                hardware = verify_hardware(records, args.hdr_level)
                if hardware is not None:
                    break
                time.sleep(0.25)
            else:
                raise TimeoutError(f"Private HDR observer did not complete; inspect {output}")
            samples = hardware["samples"]
            subprocess.run(["grim", str(output / "game.png")], check=True, timeout=10)
            subprocess.run(["grim", "-t", "ppm", str(output / "game.ppm")], check=True, timeout=10)
            console = (game / "hammerr-runtime.log").read_text(errors="replace")
            controls = verify_controls(console, args.hdr_level)
            private_launch = runtime.verify_private_launch(game, console)
            fingerprints = {path.name: hashlib.sha256(path.read_bytes()).hexdigest()
                            for path in (root / "bin/linux64").glob("*.so")
                            if path.name in ("engine.so", "materialsystem.so", "shaderapivk.so", "shaderapidx9.so")}
            result.update({"hdr_level": args.hdr_level, "controls": controls, "hardware": samples,
                      "private_launch": private_launch,
                      "acquisition": hardware["factory"], "probe_records": records, "pid": process.pid,
                      "engine_sha256": ENGINE_SHA256, "module_sha256": fingerprints,
                      "insecure": True, "post_map_sv_lan": True,
                      "display": os.environ.get("DISPLAY"), "wayland_display": os.environ.get("WAYLAND_DISPLAY")})
    finally:
        (output / "runtime.json").write_text(json.dumps(result, indent=2) + "\n")


def frame_color(path):
    magic, dimensions, maximum, pixels = path.read_bytes().split(b"\n", 3)
    assert magic == b"P6" and maximum == b"255"
    width, height = map(int, dimensions.split())
    assert (width, height) == (1280, 720) and len(pixels) == width * height * 3
    colors = [pixels[(y * width + x) * 3:(y * width + x) * 3 + 3]
              for y in range(height // 2, height * 9 // 10)
              for x in range(width // 4, width * 3 // 4)]
    return {"mean_rgb": [sum(color[axis] for color in colors) / len(colors) for axis in range(3)],
            "red_pixels": sum(color[0] > color[2] + 20 for color in colors),
            "blue_pixels": sum(color[2] > color[0] + 20 for color in colors)}


def native_cases(args, bsp, game):
    assert args.probe_library.is_file(), "Build scripts/hdr_runtime_probe.c as the supplied --probe-library first"
    assert shutil.which("cage") and shutil.which("grim"), "Cage and grim are required"
    results = {}
    for name, level in [("ldr", 0), ("hdr", 2)]:
        output = args.output / name
        command = [sys.executable, "-B", str(Path(__file__).resolve()), "--worker", "--bsp", str(bsp),
                   "--output", str(output), "--game-dir", str(args.game_dir.resolve()),
                   "--fixture-game", str(game), "--probe-library", str(args.probe_library.resolve()),
                   "--hdr-level", str(level)]
        runtime.private_compositor(command, output, timeout=62)
        result = json.loads((output / "runtime.json").read_text())
        result["frame_color"] = frame_color(output / "game.ppm")
        results[name] = result
    assert results["ldr"]["pid"] != results["hdr"]["pid"]
    ldr, hdr = results["ldr"]["frame_color"], results["hdr"]["frame_color"]
    assert ldr["red_pixels"] > 1000 and ldr["red_pixels"] > ldr["blue_pixels"] * 5, ldr
    assert hdr["blue_pixels"] > 1000 and hdr["blue_pixels"] > hdr["red_pixels"] * 5, hdr
    results["difference"] = runtime.compare_frames(args.output, ("ldr", "hdr"))
    assert results["difference"]["changed_pixels"] > 1000
    return results


def main():
    parser = argparse.ArgumentParser(description="Validate independent native LDR/HDR channels; optionally observe fresh private CS:S launches")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--game-dir", type=Path, default=Path.home() / ".local/share/Steam/steamapps/common/Counter-Strike Source")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--runtime", action="store_true")
    parser.add_argument("--probe-library", type=Path, default=ROOT / "target/debug/hammerr_hdr_probe.so")
    parser.add_argument("--runtime-bsp", type=Path, help="Reuse an already validated fixture BSP with --fixture-game")
    parser.add_argument("--fixture-game", type=Path)
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--bsp", type=Path, help=argparse.SUPPRESS)
    parser.add_argument("--hdr-level", type=int, choices=[0, 2], help=argparse.SUPPRESS)
    args = parser.parse_args()
    args.output = args.output.resolve()
    args.binary = args.binary.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    if args.worker:
        worker(args)
        return
    if args.runtime_bsp:
        assert args.runtime and args.fixture_game, "--runtime-bsp requires --runtime and --fixture-game"
        bsp, game = args.runtime_bsp.resolve(strict=True), args.fixture_game.resolve(strict=True)
        report = {"channels": check_channels(bsp), "bsp": str(bsp)}
    else:
        bsp, game = fixture(args)
        report = json.loads((args.output / "compile.json").read_text())
    report["runtime_checked"] = args.runtime
    if args.runtime:
        report["native"] = native_cases(args, bsp, game)
    (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"output": str(args.output), "independent_channels": True, "runtime_checked": args.runtime}))


if __name__ == "__main__":
    main()
