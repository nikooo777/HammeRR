import argparse
import itertools
import json
import os
import re
import resource
import signal
import shutil
import subprocess
import sys
import time
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, ROOT, Client, detail
from validate_visibility import ROOM

OBSERVER = """
stage <- 0;
next_report <- 0.0;
function HammeRRThink() {
    local prop = Entities.FindByName(null, "physics_crate");
    if (prop != null) {
        local prop_pos = prop.GetOrigin();
        printl("HAMMERR_PROP " + prop_pos.x + " " + prop_pos.y + " " + prop_pos.z);
    }
    local player = Entities.FindByClassname(null, "player");
    if (player == null) return 1.0;
    if (stage == 0) {
        SendToConsole("sv_cheats 1; joystick 0; jointeam 2; joinclass 1; hidepanel info; hidepanel team; hidepanel class_ter; hidepanel class_ct; r_drawvgui 0; gameui_hide");
        stage = 1;
        next_report = Time() + 5.0;
    }
    if (Time() > next_report) {
        local pos = player.GetOrigin();
        printl("HAMMERR_PLAYER " + player.GetHealth() + " " + player.GetTeam() + " " + pos.x + " " + pos.y + " " + pos.z);
        if (stage == 1) {
            SendToConsole("setpos -64 -64 32; setang 25 45 0; mat_fullbright");
            stage = 2;
        }
        next_report = Time() + 3.0;
    }
    return 1.0;
}
"""


def private_game(root, output, bsp=None, *, engine_physics=False):
    game = output / "cstrike"
    if engine_physics:
        output.mkdir(parents=True, exist_ok=True)
        library = output / "bin"
        expected = (root / "bin").resolve(strict=True)
        if not library.exists():
            library.symlink_to(expected, target_is_directory=True)
        if library.resolve(strict=True) != expected:
            raise RuntimeError("Private physics fixture has an unexpected engine library path")
    for directory in [
        game / "maps",
        game / "cfg",
        game / "scripts/vscripts",
        game / "download",
    ]:
        directory.mkdir(parents=True, exist_ok=True)
    paths = [
        ("game", str(game)),
        ("mod+mod_write+default_write_path", "|gameinfo_path|."),
        ("game+game_write", str(game)),
        ("game+download", str(game / "download")),
    ]
    paths += [
        ("game+mod", str(root / "cstrike" / name))
        for name in ["cstrike_english.vpk", "cstrike_pak.vpk"]
    ]
    paths += [
        ("game", str(root / "hl2" / name))
        for name in [
            "hl2_textures.vpk",
            "hl2_sound_vo_english.vpk",
            "hl2_sound_misc.vpk",
            "hl2_misc.vpk",
        ]
    ]
    paths += [
        ("gamebin", str(root / "cstrike/bin")),
        ("game", str(root / "cstrike")),
        ("game", str(root / "hl2")),
        ("platform", str(root / "platform/platform_misc.vpk")),
        ("platform", str(root / "platform")),
    ]
    search = "\n".join(f'"{kind}" "{path}"' for kind, path in paths)
    (game / "gameinfo.txt").write_text(
        '"GameInfo" { game "HammeRR validation" title "Counter-Strike Source" type multiplayer_only nomodels 1 nohimodel 1 bots 1 nodegraph 0 FileSystem { SteamAppId 240 SearchPaths {\n'
        + search
        + "\n} } }\n"
    )
    for name in ["autoexec.cfg", "config.cfg"]:
        (game / "cfg" / name).write_text("")
    for name in ["listenserver.cfg", "server.cfg"]:
        (game / "cfg" / name).write_text("sv_lan 1\nsv_lan\n")
    (game / "motd.txt").write_text("HammeRR private validation\n")
    (game / "scripts/vscripts/mapspawn.nut").write_text('printl("HAMMERR_MAPSPAWN");\n')
    (game / "scripts/vscripts/hammerr_observer.nut").write_text(OBSERVER)
    if bsp is not None:
        shutil.copyfile(bsp, game / "maps/hammerr_runtime.bsp")
    (game / "hammerr-runtime.log").unlink(missing_ok=True)
    return game


def worker(args):
    root = args.game_dir.resolve()
    output = args.output.resolve()
    game = private_game(root, output, args.bsp)
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    env = {
        **os.environ,
        "LD_LIBRARY_PATH": str(root / "bin/linux64"),
        "SteamAppId": "240",
        "SteamGameId": "240",
        "SDL_VIDEODRIVER": "x11",
    }
    command = [
        str(root / "cstrike_linux64"),
        "-game",
        str(game),
        "-windowed",
        "-w",
        "1280",
        "-h",
        "720",
        "-novid",
        "-insecure",
        "-nomaster",
        "-nojoy",
        "-console",
        "-port",
        "27583",
        "+con_logfile",
        "hammerr-runtime.log",
        "+sv_lan",
        "1",
        "+mat_hdr_level",
        "0",
        "+fps_max",
        "60",
        "+map",
        "hammerr_runtime",
    ]
    with (output / "launcher.log").open("w") as log:
        process = subprocess.Popen(
            command, cwd=root, env=env, stdin=subprocess.DEVNULL, stdout=log, stderr=log
        )
        try:
            started = time.monotonic()
            while time.monotonic() - started < getattr(args, "observe_seconds", 30):
                if process.poll() is not None:
                    raise RuntimeError(f"CS:S exited early: {process.returncode}")
                time.sleep(1)
            subprocess.run(["grim", str(output / "game.png")], check=True, timeout=10)
            if getattr(args, "capture_ppm", False):
                subprocess.run(
                    ["grim", "-t", "ppm", str(output / "game.ppm")],
                    check=True,
                    timeout=10,
                )
            text = (game / "hammerr-runtime.log").read_text(errors="replace")
            players = [
                list(map(float, match))
                for match in re.findall(
                    r"HAMMERR_PLAYER ([\d.-]+) ([\d.-]+) ([\d.eE+-]+) ([\d.eE+-]+) ([\d.eE+-]+)",
                    text,
                )
            ]
            props = [
                list(map(float, match))
                for match in re.findall(
                    r"HAMMERR_PROP ([\d.eE+-]+) ([\d.eE+-]+) ([\d.eE+-]+)", text
                )
            ]
            assert len(players) >= 3 and all(
                row[0] == 100 and row[1] == 2 for row in players
            ), players
            ground_z = getattr(args, "ground_z", 0.0)
            ground_tolerance = getattr(args, "ground_tolerance", 1.0)
            assert all(
                abs(row[4] - ground_z) < ground_tolerance for row in players[-3:]
            ), players
            assert '"mat_fullbright" = "0"' in text and "Level unlit" not in text
            assert props, "Physics prop did not enter the observer"
            report = {
                "players": players,
                "prop_positions": props,
                "display": os.environ.get("DISPLAY"),
                "wayland_display": os.environ.get("WAYLAND_DISPLAY"),
                "insecure": True,
                "ground_z": ground_z,
                "ground_tolerance": ground_tolerance,
            }
            (output / "runtime.json").write_text(json.dumps(report, indent=2) + "\n")
        finally:
            if process.poll() is None:
                process.kill()
            process.wait(timeout=10)


def compile_cases(args):
    client = Client(None, args.binary, args.output)
    try:
        for low, high in ROOM:
            client.call(
                "create_block",
                {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"},
            )
        if args.fluid:
            brush = client.call(
                "create_block",
                {
                    "mins": [-32, -32, 0],
                    "maxs": [224, 224, 64],
                    "material": "tools/toolsnodraw",
                },
            )["id"]
            source = detail(client, [brush])[0]
            top = next(face for face in source["faces"] if face["normal"] == [0, 0, 1])
            client.call(
                "select_faces",
                {"faces": [{"solid": source["solid"], "side": top["side"]}]},
            )
            client.call("edit_faces", {"material": "liquids/aztecwater"})
        client.call(
            "create_entity",
            {"classname": "info_player_terrorist", "origin": [-64, 0, 8]},
        )
        client.call(
            "create_entity",
            {
                "classname": "light",
                "origin": [0, 0, 100],
                "properties": {"_light": "255 0 0 200", "_quadratic_attn": "1"},
            },
        )
        client.call(
            "create_entity",
            {
                "classname": "logic_script",
                "origin": [0, 0, 32],
                "properties": {
                    "targetname": "hammerr_observer",
                    "vscripts": "hammerr_observer.nut",
                    "thinkfunction": "HammeRRThink",
                },
            },
        )
        client.call(
            "create_entity",
            {
                "classname": "prop_physics",
                "origin": [64, 64, 160],
                "properties": {
                    "targetname": "physics_crate",
                    "model": "models/props_junk/wood_crate001a.mdl",
                },
            },
        )
        client.call("save_map", {"path": str(args.output / "runtime.vmf")})
        cases = []
        for mode in ["none", "engine"]:
            client.call(
                "set_compile_settings", {"lighting": "direct_ldr", "physics": mode}
            )
            path = args.output / f"{mode}.bsp"
            compiled = client.call("compile_map", {"path": str(path)})
            assert compiled["success"]
            cases.append((mode, path, compiled["stats"]))
        return cases
    finally:
        client.close()


def check_positions(results, fluid):
    none, engine = results
    assert none["prop_positions"][-1][2] < -512, none
    if fluid:
        positions = engine["prop_positions"]
        assert 40 < positions[-1][2] < 96, engine
        assert all(
            -32 < point[0] < 224 and -32 < point[1] < 224 for point in positions[-3:]
        )
        assert any(b[2] - a[2] > 8 for a, b in itertools.pairwise(positions[1:])), (
            positions
        )
    else:
        assert 0 < engine["prop_positions"][-1][2] < 64, engine
    assert max(
        abs(a - b)
        for a, b in zip(engine["prop_positions"][-1], engine["prop_positions"][-2])
    ) < (2.0 if fluid else 0.1)



def private_compositor(command, output, timeout=65, extra_environment=None):
    output = output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    private_runtime = output / "wayland"
    private_runtime.mkdir(mode=0o700)
    environment = {
        **os.environ,
        "WLR_BACKENDS": "headless",
        "WLR_HEADLESS_OUTPUTS": "1",
        "WLR_RENDERER": "pixman",
        "XDG_RUNTIME_DIR": str(private_runtime),
        "SDL_AUDIODRIVER": "dummy",
        **(extra_environment or {}),
    }
    environment.pop("DISPLAY", None)
    environment.pop("WAYLAND_DISPLAY", None)
    with (output / "compositor.log").open("w") as stream:
        process = subprocess.Popen(["cage", "--", *command], cwd=ROOT, env=environment, stdout=stream, stderr=subprocess.STDOUT, start_new_session=True)
        try:
            result = process.wait(timeout=timeout)
        finally:
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGTERM)
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()
    if result:
        raise RuntimeError((output / "compositor.log").read_text())
    return output


def private_display(script, args, bsp, extra_flags=(), output=None):
    output = (args.output / "runtime" if output is None else output).resolve()
    command = [
        sys.executable, "-B", str(script.resolve()), "--worker",
        "--binary", str(args.binary.resolve()), "--bsp", str(bsp.resolve()),
        "--output", str(output), "--game-dir", str(args.game_dir.resolve()),
        *extra_flags,
    ]
    return private_compositor(command, output)


def capture_worker(args, view_command, ground_z=0.0):
    global OBSERVER
    original = OBSERVER
    default_view = "setpos -64 -64 32; setang 25 45 0; mat_fullbright"
    if original.count(default_view) != 1:
        raise RuntimeError("Native observer must contain exactly one capture view")
    args.capture_ppm = True
    args.ground_z = ground_z
    OBSERVER = original.replace(default_view, view_command)
    try:
        worker(args)
    finally:
        OBSERVER = original


def compare_frames(output, names=("flat", "smooth")):
    frames = []
    for name in names:
        magic, dimensions, maximum, pixels = (output / name / "game.ppm").read_bytes().split(b"\n", 3)
        assert magic == b"P6" and maximum == b"255"
        width, height = map(int, dimensions.split())
        assert width > 0 and height > 0 and len(pixels) == width * height * 3
        frames.append(((width, height), pixels))
    assert len(frames) == 2 and frames[0][0] == frames[1][0]
    differences = [abs(a - b) for a, b in zip(frames[0][1], frames[1][1], strict=True)]
    changed = sum(max(pixel) > 3 for pixel in zip(*[iter(differences)] * 3))
    return {"changed_pixels": changed, "mean_component_difference": sum(differences) / len(differences)}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--game-dir",
        type=Path,
        default=Path.home()
        / ".local/share/Steam/steamapps/common/Counter-Strike Source",
    )
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-native-runtime-check")
    )
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument(
        "--fluid",
        action="store_true",
        help="Include a world pool and verify crate buoyancy",
    )
    parser.add_argument("--bsp", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    if args.worker:
        worker(args)
        return
    assert shutil.which("cage") and shutil.which("grim"), "Cage and grim are required"
    results = []
    for mode, bsp, stats in compile_cases(args):
        output = private_display(Path(__file__), args, bsp, output=args.output / mode)
        result = json.loads((output / "runtime.json").read_text())
        result.update({"mode": mode, "stats": stats})
        results.append(result)
    if args.fluid:
        assert results[1]["stats"]["physics_fluids"] == 1
    check_positions(results, args.fluid)
    (args.output / "results.json").write_text(json.dumps(results, indent=2) + "\n")
    print(json.dumps({"cases": len(results), "output": str(args.output)}))


if __name__ == "__main__":
    main()
