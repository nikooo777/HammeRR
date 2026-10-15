import argparse
import json
import os
import re
import signal
import subprocess
import sys
import threading
import time
from pathlib import Path

import validate_native_runtime as runtime
from validate_face_lighting import ROOT, Client
from validate_occluders import native_occlusion, occluder
from validate_terrain_lod import build
from validate_visibility import ROOM

OBSERVER = (
    runtime.OBSERVER.replace("stage <- 0;", "stage <- 0;\nocclusion_start <- null;\nocclusion_stage <- 0;")
    .replace("sv_cheats 1; joystick 0;", "sv_cheats 1; joystick 0; bot_quota 0; bot_kick; r_drawviewmodel 0; cl_drawhud 0;")
    .replace("setpos -64 -64 32; setang 25 45 0; mat_fullbright", "noclip; setpos -160 0 64; setang 0 0 0; r_occlusion 1; r_occludeemaxarea 100; r_occluderminarea 0; r_occlusion; mat_fullbright")
    .replace("    return 1.0;", """
    if (stage == 2) {
        if (occlusion_start == null) occlusion_start = Time();
        local elapsed = Time() - occlusion_start;
        local next = elapsed >= 20 ? 4 : elapsed >= 15 ? 3 : elapsed >= 10 ? 2 : elapsed >= 5 ? 1 : 0;
        if (next > occlusion_stage) {
            local input = next == 1 ? "Deactivate" : next == 2 ? "Activate" : "Toggle";
            EntFire("visibility_screen", input);
            occlusion_stage = next;
        }
        local screen = Entities.FindByName(null, "visibility_screen");
        if (screen != null && NetProps.HasProp(screen, "m_bActive") && NetProps.HasProp(screen, "m_nOccluderIndex")) {
            printl("HAMMERR_OCCLUDER " + occlusion_stage + " " + (NetProps.GetPropBool(screen, "m_bActive") ? 1 : 0) + " " + NetProps.GetPropInt(screen, "m_nOccluderIndex"));
        } else printl("HAMMERR_OCCLUDER_PROP_MISSING");
    }
    return 1.0;
""")
)


def fixture(args):
    client = Client(None, args.binary, args.output)
    try:
        for low, high in ROOM:
            client.call("create_block", {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"})
        for classname, origin, properties in [
            ("info_player_terrorist", [-160, 0, 8], {}),
            ("info_player_counterterrorist", [192, -160, 8], {}),
            ("light", [0, 0, 160], {"_light": "255 255 255 200", "_constant_attn": "1", "_quadratic_attn": "0"}),
            ("prop_static", [96, 0, 8], {"model": "models/props_junk/wood_crate001a.mdl", "solid": "0"}),
            ("prop_physics", [-208, -208, 64], {"model": "models/props_junk/wood_crate001a.mdl", "targetname": "physics_crate"}),
            ("logic_script", [-128, -128, 64], {"targetname": "hammerr_observer", "vscripts": "hammerr_observer.nut", "thinkfunction": "HammeRRThink"}),
        ]:
            client.call("create_entity", {"classname": classname, "origin": origin, "properties": properties})
        occluder(client, [-2, -96, 0], [2, 96, 192], "1")
        client.call("set_compile_settings", {"lighting": "direct_ldr", "physics": "none"})
        client.call("save_map", {"path": str(args.output / "occluder-runtime.vmf")})
        path, stats = build(client, args.output, "occluder-runtime")
        records = native_occlusion(path.read_bytes())[0]
        assert len(records) == stats["occluders"] == 1 and records[0][0] == 0
        assert stats["static_props"] == 1 and stats["brushes"] == 6
        (args.output / "compile.json").write_text(json.dumps(stats, indent=2) + "\n")
        return path
    finally:
        client.close()


def capture_states(output, errors):
    try:
        captured = set()
        deadline = time.monotonic() + 43
        log = output / "cstrike/hammerr-runtime.log"
        expected = [1, 0, 1, 0, 1]
        while time.monotonic() < deadline and len(captured) < len(expected):
            if log.exists():
                states = re.findall(r"HAMMERR_OCCLUDER (\d+) (\d+) (\d+)", log.read_text(errors="replace"))
                if len(states) >= 3 and len(set(states[-3:])) == 1:
                    phase, active, index = map(int, states[-1])
                    if phase < len(expected) and active == expected[phase] and index == 0 and phase not in captured:
                        subprocess.run(["grim", str(output / f"phase-{phase}.png")], check=True, timeout=5)
                        subprocess.run(["grim", "-t", "ppm", str(output / f"phase-{phase}.ppm")], check=True, timeout=5)
                        captured.add(phase)
            time.sleep(0.15)
        assert captured == set(range(5)), captured
    except (OSError, subprocess.SubprocessError, AssertionError) as error:
        errors.append(str(error))


def pixels(path):
    data = path.read_bytes()
    header = re.match(rb"P6\s+(\d+)\s+(\d+)\s+255\s", data)
    assert header
    width, height = map(int, header.groups())
    body = data[header.end():]
    assert len(body) == width * height * 3
    return width, height, body


def difference(first, second):
    width, height, a = pixels(first)
    other_width, other_height, b = pixels(second)
    assert (width, height) == (other_width, other_height) == (1280, 720)
    changed = 0
    for y in range(280, 680):
        for x in range(400, 880):
            offset = (y * width + x) * 3
            changed += any(abs(a[offset + channel] - b[offset + channel]) > 12 for channel in range(3))
    return changed


def worker(args):
    runtime.OBSERVER = OBSERVER
    args.observe_seconds = 39
    args.ground_z = 64
    errors = []
    capture = threading.Thread(target=capture_states, args=(args.output, errors), daemon=True)
    capture.start()
    runtime.worker(args)
    capture.join(timeout=8)
    assert not errors, errors
    text = (args.output / "cstrike/hammerr-runtime.log").read_text(errors="replace")
    assert '"sv_lan" = "1"' in text, text[-6000:]
    assert '"r_occlusion" = "1"' in text, text[-6000:]
    active_repeat = difference(args.output / "phase-2.ppm", args.output / "phase-4.ppm")
    deactivated = difference(args.output / "phase-1.ppm", args.output / "phase-2.ppm")
    toggled = difference(args.output / "phase-3.ppm", args.output / "phase-4.ppm")
    assert deactivated > 300 and toggled > 300, (deactivated, toggled, active_repeat)
    assert active_repeat < min(deactivated, toggled) // 4, (active_repeat, deactivated, toggled)
    result = {"initial_active": True, "deactivate_activate_toggle": True, "runtime_index": 0, "occlusion_enabled": True, "deactivated_changed_pixels": deactivated, "toggled_changed_pixels": toggled, "active_repeat_changed_pixels": active_repeat, "insecure": True, "sv_lan": True}
    (args.output / "occlusion-runtime.json").write_text(json.dumps(result, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--game-dir", type=Path, default=Path.home() / ".local/share/Steam/steamapps/common/Counter-Strike Source")
    parser.add_argument("--runtime",action="store_true",help="Also check client culling in a private sv_lan 1 server")
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--bsp", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    args.binary = args.binary.resolve()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    if args.worker:
        worker(args)
        return
    bsp = fixture(args)
    if not args.runtime:
        print(json.dumps({"compiled_occluder_fixture":True,"output":str(args.output),"runtime_checked":False}))
        return
    output = args.output / "runtime"
    output.mkdir()
    private_runtime = output / "wayland"
    private_runtime.mkdir(mode=0o700)
    environment = {**os.environ, "WLR_BACKENDS": "headless", "WLR_HEADLESS_OUTPUTS": "1", "WLR_RENDERER": "pixman", "XDG_RUNTIME_DIR": str(private_runtime), "SDL_AUDIODRIVER": "dummy"}
    environment.pop("DISPLAY", None)
    environment.pop("WAYLAND_DISPLAY", None)
    command = ["cage", "--", sys.executable, "-B", str(Path(__file__).resolve()), "--worker", "--binary", str(args.binary), "--bsp", str(bsp), "--output", str(output), "--game-dir", str(args.game_dir)]
    with (output / "compositor.log").open("w") as stream:
        process = subprocess.Popen(command, cwd=ROOT, env=environment, stdout=stream, stderr=subprocess.STDOUT, start_new_session=True)
        try:
            result = process.wait(timeout=65)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGTERM)
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid,signal.SIGKILL)
                process.wait()
            raise
    if result:
        print((output / "compositor.log").read_text())
        raise SystemExit(result)
    print((output / "occlusion-runtime.json").read_text())


if __name__ == "__main__":
    main()
