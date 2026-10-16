import argparse
import hashlib
import json
import os
import re
import resource
import shutil
import subprocess
import sys
import time
from pathlib import Path
from unittest.mock import patch

import validate_native_runtime as runtime
from private_runtime_config import (
    finish_private_launch,
    private_launch_options,
    private_observer,
    verify_private_launch,
)
from validate_authored_planes import records
from validate_bump_lighting import texture
from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_visibility import ROOM
from validate_water_bottom import native_faces, raw_lump

TOP = "hammerr/water_surface"
BOTTOMS = {"red": "hammerr/water_below_red", "blue": "hammerr/water_below_blue"}
SURFACE = 160
VIEWS = {"above": (224, 75), "below": (112, -75), "away": (112, 75)}
CONTROLS = {
    "sv_lan": 1,
    "mat_hdr_level": 0,
    "mat_fullbright": 0,
    "r_waterforceexpensive": 1,
    "r_drawtranslucentworld": 1,
    "r_drawvgui": 0,
    "cl_drawhud": 0,
    "r_drawviewmodel": 0,
    "r_drawothermodels": 0,
}
REGION = (400, 200, 880, 520)
COLLISION_LUMPS = (1, 3, 5, 10, 12, 13, 14, 18, 19, 29, 36, 46)
WATER_PARAMETERS = """
    %tooltexture hammerr/water_white
    $normalmap hammerr/water_flat_normal
    $reflecttexture _rt_WaterReflection
    $reflecttint "[0 0 0]"
    $reflectamount 0
    $refractamount 0
    $forceexpensive 1
    $forcecheap 0
    $fogenable 1
    $fogcolor "[0.015 0.015 0.015]"
    $fogstart 0
    $fogend 4096
    $surfaceprop water
"""
OBSERVER = """
stage <- 0;
ready_at <- 0.0;
phase <- -1;
phase_at <- 0.0;
function HammeRRThink() {
    local player = Entities.FindByClassname(null, "player");
    if (player == null) return 0.5;
    if (stage == 0) {
        SendToConsole("sv_cheats 1; sv_lan 1; joystick 0; bot_quota 0; bot_kick; jointeam 2; joinclass 1; hidepanel info; hidepanel team; hidepanel class_ter; hidepanel class_ct; r_drawvgui 0; gameui_hide; cl_drawhud 0; r_drawviewmodel 0; r_drawothermodels 0; mat_fullbright 0; toggle r_waterforceexpensive 1; toggle r_drawtranslucentworld 1; toggle con_notifytime 0");
        ready_at = Time() + 6.0;
        stage = 1;
    }
    if (stage == 1 && Time() > ready_at && player.GetTeam() == 2 && player.GetHealth() == 100) {
        SendToConsole("noclip; god");
        ready_at = Time() + 1.0;
        stage = 2;
    }
    if (stage == 2 && Time() > ready_at) {
        local elapsed = Time() - ready_at;
        local next = elapsed >= 20.0 ? 2 : elapsed >= 10.0 ? 1 : 0;
        if (next != phase) {
            phase = next;
            phase_at = Time();
            SendToConsole(phase == 1 ? "setang -75 0 0" : "setang 75 0 0");
            SendToConsole("sv_lan; mat_hdr_level; mat_fullbright; help r_waterforceexpensive; help r_drawtranslucentworld; r_drawvgui; cl_drawhud; r_drawviewmodel; r_drawothermodels; help mat_dxlevel");
        }
        local eye = player.EyePosition();
        local origin = player.GetOrigin();
        local desired = Vector(0, 0, phase == 0 ? 224 : 112);
        player.SetOrigin(desired - (eye - origin));
        player.SetVelocity(Vector(0, 0, 0));
        if (Time() > phase_at + 3.0) {
            eye = player.EyePosition();
            local angles = player.EyeAngles();
            local wet = NetProps.HasProp(player, "m_nWaterLevel") ? NetProps.GetPropInt(player, "m_nWaterLevel") : -1;
            printl("HAMMERR_WATER_VIEW " + phase + " " + Time() + " " + player.GetTeam() + " " + player.GetHealth() + " " + wet + " " + eye.x + " " + eye.y + " " + eye.z + " " + angles.x + " " + angles.y + " " + angles.z);
        }
    }
    return 0.5;
}
"""


def digest(data):
    return hashlib.sha256(data).hexdigest()


def material_hashes(game):
    return {
        str(path.relative_to(game)): digest(path.read_bytes())
        for path in sorted((game / "materials").rglob("*"))
        if path.is_file()
    }


def verify_runtime_materials(actual, expected, preservation):
    assert preservation["success"] is True, preservation
    assert {name: actual.get(name) for name in expected} == expected, (
        "Runtime fixture material hashes changed",
        actual,
        expected,
    )
    extras = {name: value for name, value in actual.items() if name not in expected}
    allowed = {f"materials/vgui/logos/spray.{suffix}" for suffix in ["vmt", "vtf"]}
    assert extras.keys() <= allowed, ("Unexpected runtime materials", extras)
    before = preservation["before"]
    remote = Path(before["steam"]) / "userdata" / before["account"] / "240/remote"
    imports = {}
    for name, value in extras.items():
        source = str(remote / name)
        assert before["files"].get(source) == value, ("Unverified Cloud import", name)
        assert preservation["after"]["files"].get(source) == value, (
            "Cloud import changed during validation",
            name,
        )
        imports[name] = {"sha256": value, "cloud_source": source}
    return imports


def assets(args):
    game = runtime.private_game(
        args.game_dir.resolve(), args.output / "source-game", engine_physics=True
    )
    folder = game / "materials/hammerr"
    folder.mkdir(parents=True)
    (folder / "water_white.vtf").write_bytes(texture(bytes([255] * 32 * 32 * 4)))
    (folder / "water_flat_normal.vtf").write_bytes(
        texture(bytes([128, 128, 255, 255] * 32 * 32), normal=True)
    )
    (folder / "water_receiver.vmt").write_text(
        "LightmappedGeneric { $basetexture hammerr/water_white }\n"
    )
    for name, color in {"red": [255, 3, 3, 255], "blue": [3, 3, 255, 255]}.items():
        (folder / f"water_signature_{name}.vtf").write_bytes(
            texture(bytes(color * 32 * 32))
        )
        (folder / f"water_below_{name}.vmt").write_text(
            "Water {\n    $abovewater 0\n"
            f"    $refracttexture hammerr/water_signature_{name}\n"
            + WATER_PARAMETERS
            + "}\n"
        )
    return game


def top_material(bottom):
    return (
        "Water {\n    $abovewater 1\n"
        "    $refracttexture _rt_WaterRefraction\n"
        f"    $bottommaterial {bottom}\n" + WATER_PARAMETERS + "}\n"
    )


def check_native(data, bottom):
    infos, faces, materials = native_faces(data)
    planes = records(data, 1, "<4fi")
    tops = [
        face for face, material in zip(faces, materials, strict=True) if material == TOP
    ]
    bottoms = [
        face
        for face, material in zip(faces, materials, strict=True)
        if material == bottom
    ]
    water = records(data, 36, "<ffhh")
    assert tops and bottoms and len(water) == 1
    assert (
        sum(face[13] for face in tops) == sum(face[13] for face in bottoms) == 320 * 320
    )
    assert all(planes[face[0]][2] == 1 for face in tops)
    assert all(planes[face[0]][2] == -1 for face in bottoms)
    assert (water[0][0], water[0][1]) == (SURFACE, 0)
    assert water[0][2] in [face[5] for face in tops]
    assert all(face[7] == 0 for face in tops + bottoms)
    assert all(infos[face[5]][:17] == infos[water[0][2]][:17] for face in bottoms)
    return {
        "top": TOP,
        "bottom": bottom,
        "top_faces": len(tops),
        "bottom_faces": len(bottoms),
        "water": [list(row) for row in water],
        "collision_sha256": {
            str(index): digest(raw_lump(data, index)) for index in COLLISION_LUMPS
        },
    }


def compile_fixture(args):
    game = assets(args)
    top_file = game / f"materials/{TOP}.vmt"
    top_file.write_text(top_material(BOTTOMS["red"]))
    with patch.dict(
        "os.environ",
        {
            "HAMMERR_RECENT_MAPS_DIR": str(args.output / "recent"),
            "HAMMERR_BUILD_RECIPES": str(args.output / "recipes.json"),
        },
    ):
        client = Client(None, args.binary.resolve(), args.output, game / "gameinfo.txt")
    try:
        client.call("new_map", {"discard_changes": True})
        for mins, maxs in ROOM:
            client.call(
                "create_block",
                {"mins": mins, "maxs": maxs, "material": "hammerr/water_receiver"},
            )
        for classname, origin, properties in [
            ("info_player_terrorist", [-224, -192, 8], {}),
            ("info_player_counterterrorist", [224, -192, 8], {}),
            (
                "light",
                [0, 0, 224],
                {
                    "_light": "255 255 255 200",
                    "_constant_attn": "1",
                    "_quadratic_attn": "0",
                },
            ),
            (
                "logic_script",
                [-224, 0, 32],
                {
                    "targetname": "hammerr_observer",
                    "vscripts": "hammerr_observer.nut",
                    "thinkfunction": "HammeRRThink",
                },
            ),
        ]:
            client.call(
                "create_entity",
                {"classname": classname, "origin": origin, "properties": properties},
            )
        pool = client.call(
            "create_block",
            {
                "mins": [-160, -160, 0],
                "maxs": [160, 160, SURFACE],
                "material": "tools/toolsnodraw",
            },
        )["id"]
        brush = detail(client, [pool])[0]
        top = next(face for face in brush["faces"] if face["normal"] == [0, 0, 1])
        client.call(
            "select_faces", {"faces": [{"solid": brush["solid"], "side": top["side"]}]}
        )
        client.call("edit_faces", {"material": TOP})
        client.call(
            "set_compile_settings",
            {"lighting": "direct_ldr", "physics": "engine", "visibility": "fast"},
        )
        vmf = args.output / "source.vmf"
        client.call("save_map", {"path": str(vmf)})
        source = vmf.read_bytes()
        before = detail(client, [pool])
        history = client.call("get_map_info")["undo"]
        selection = client.call("get_selection")
        camera = client.call("get_camera")
        cases = {}
        for name, bottom in BOTTOMS.items():
            top_file.write_text(top_material(bottom))
            for material in [TOP, bottom]:
                preview = client.call("preview_material", {"material": material})
                assert not preview["missing"] and (
                    preview["width"],
                    preview["height"],
                ) == (32, 32), (material, preview)
            bsp = args.output / f"{name}.bsp"
            result = client.call("compile_map", {"path": str(bsp)})
            assert result["success"] and result["stats"]["water_regions"] == 1
            assert result["stats"]["physics_fluids"] == 1
            native = check_native(bsp.read_bytes(), bottom)
            fixture = args.output / "material-snapshots" / name
            shutil.copytree(game / "materials", fixture / "materials")
            cases[name] = {
                "bsp": str(bsp),
                "fixture_game": str(fixture),
                "stats": result["stats"],
                "native": native,
                "materials_sha256": material_hashes(fixture),
            }
            assert vmf.read_bytes() == source and detail(client, [pool]) == before
            assert client.call("get_map_info")["undo"] == history
            assert (
                client.call("get_selection") == selection
                and client.call("get_camera") == camera
            )
        assert (
            cases["red"]["native"]["collision_sha256"]
            == cases["blue"]["native"]["collision_sha256"]
        )
        assert {
            path: value
            for path, value in cases["red"]["materials_sha256"].items()
            if path != f"materials/{TOP}.vmt"
        } == {
            path: value
            for path, value in cases["blue"]["materials_sha256"].items()
            if path != f"materials/{TOP}.vmt"
        }
        report = {
            "cases": cases,
            "source_sha256": digest(source),
            "source_preserved": True,
            "history_preserved": True,
            "selection_preserved": True,
            "camera_preserved": True,
            "collision_preserved": True,
            "signature": "$refracttexture through the Water shader; top/fog/overlay unchanged",
        }
        (args.output / "compile.json").write_text(json.dumps(report, indent=2) + "\n")
        return report
    finally:
        client.close()


def controls(text):
    values = {}
    for name, expected in CONTROLS.items():
        matches = re.findall(r'"' + re.escape(name) + r'" = "([^"]+)"', text)
        assert matches and float(matches[-1]) == expected, (name, expected, matches)
        values[name] = float(matches[-1])
    matches = re.findall(r'"mat_dxlevel" = "([^"]+)"', text)
    assert matches and float(matches[-1]) >= 90, matches
    values["mat_dxlevel"] = float(matches[-1])
    return values


def view_states(text):
    rows = re.findall(r"HAMMERR_WATER_VIEW ([^\r\n]+)", text)
    return [list(map(float, row.split())) for row in rows if len(row.split()) == 11]


def valid_pose(row):
    phase, _, team, health, wet, x, y, z, pitch, yaw, roll = row
    if phase not in range(len(VIEWS)):
        return False
    eye, angle = list(VIEWS.values())[int(phase)]
    return (
        team == 2
        and health == 100
        and max(abs(x), abs(y), abs(z - eye)) < 1
        and abs(pitch - angle) < 1
        and abs(yaw) < 1
        and abs(roll) < 1
        and (0 <= wet < 3 if phase == 0 else wet == 3)
    )


def runtime_diagnostics(text):
    rows = []
    failures = []
    for number, line in enumerate(text.splitlines(), 1):
        lower = line.lower().replace("\\", "/")
        if not re.search(
            r"\berror\b|\bwarning\b|\bfailed\b|\bmissing\b|not found|not present|couldn.t find|not a texture|\bcannot\b",
            lower,
        ):
            continue
        material_diagnostic = bool(
            re.search(
                r"\b(?:materials?|shaders?|textures?|render[-_ ]?targets?)\b|\bctexture::",
                lower,
            )
        )
        fixture = (
            "hammerr/" in lower
            or bool(re.search(r"\b_rt_water[a-z0-9_]*\b", lower))
            or (
                material_diagnostic
                and bool(re.search(r"\bwater(?:_dx[a-z0-9_]*)?\b", lower))
            )
        )
        fatal = fixture or any(
            message in lower
            for message in [
                "error vertex file",
                "error index file",
                "script error",
                "an error has occurred",
            ]
        )
        row = {
            "line": number,
            "message": line,
            "fixture_asset_or_water_shader": fixture,
            "fatal": fatal,
        }
        rows.append(row)
        if fatal:
            failures.append(row)
    return {
        "all": rows,
        "fixture_failures": failures,
        "unrelated": [row for row in rows if not row["fatal"]],
    }


def worker(args):
    root, output = args.game_dir.resolve(), args.output.resolve()
    assert not os.environ.get("LD_PRELOAD"), (
        "Private water validation rejects inherited LD_PRELOAD"
    )
    assert os.environ.get("WAYLAND_DISPLAY") and os.environ.get(
        "XDG_RUNTIME_DIR"
    ) == str(output / "wayland")
    game = runtime.private_game(root, output, args.bsp)
    shutil.copytree(args.fixture_game / "materials", game / "materials")
    assert (game / f"materials/{TOP}.vmt").read_text() == top_material(BOTTOMS["red"])
    check_native(args.bsp.read_bytes(), BOTTOMS[args.variant])
    (game / "scripts/vscripts/hammerr_observer.nut").write_text(
        private_observer(OBSERVER)
    )
    for name in ["listenserver.cfg", "server.cfg"]:
        assert re.search(r"(?m)^toggle sv_lan 1$", (game / "cfg" / name).read_text())
    launch_options = private_launch_options(
        root,
        game,
        controls={
            "servercfgfile": "server.cfg",
            **{
                name: value
                for name, value in CONTROLS.items()
                if name not in {"r_waterforceexpensive", "r_drawtranslucentworld"}
            },
        },
    )
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    environment = {
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
        *launch_options,
        "-nomaster",
        "-nojoy",
        "-console",
        "-port",
        "27589",
    ]
    captures = {}
    with (output / "launcher.log").open("w") as log:
        process = subprocess.Popen(
            command,
            cwd=root,
            env=environment,
            stdin=subprocess.DEVNULL,
            stdout=log,
            stderr=log,
        )
        try:
            deadline = time.monotonic() + 53
            while time.monotonic() < deadline and len(captures) < len(VIEWS) * 2:
                if process.poll() is not None:
                    raise RuntimeError(
                        f"Private CS:S exited {process.returncode}; inspect {output}"
                    )
                console = game / "hammerr-runtime.log"
                text = console.read_text(errors="replace") if console.exists() else ""
                rows = view_states(text)
                if (
                    len(rows) >= 3
                    and all(valid_pose(row) for row in rows[-3:])
                    and len({row[0] for row in rows[-3:]}) == 1
                ):
                    row = rows[-1]
                    phase = list(VIEWS)[int(row[0])]
                    previous = captures.get(f"{phase}-0")
                    repeat = 0 if previous is None else 1
                    key = f"{phase}-{repeat}"
                    if key not in captures and (
                        previous is None or row[1] - previous["state"][1] >= 1.5
                    ):
                        values = controls(text)
                        verify_private_launch(game, text)
                        subprocess.run(
                            ["grim", str(output / f"{key}.png")], check=True, timeout=5
                        )
                        subprocess.run(
                            ["grim", "-t", "ppm", str(output / f"{key}.ppm")],
                            check=True,
                            timeout=5,
                        )
                        captures[key] = {"state": row, "controls": values}
                time.sleep(0.15)
            assert len(captures) == len(VIEWS) * 2, {
                "captured": captures,
                "last_states": view_states(text)[-10:],
                "output": str(output),
            }
            assert "Level unlit" not in text
            diagnostics = runtime_diagnostics(text)
            (output / "runtime-diagnostics.json").write_text(
                json.dumps(diagnostics, indent=2) + "\n"
            )
            assert not diagnostics["fixture_failures"], diagnostics["fixture_failures"]
            report = {
                "diagnostics": diagnostics,
                "private_launch": verify_private_launch(game, text),
                "pid": process.pid,
                "captures": captures,
                "game": str(game),
                "insecure": True,
                "post_map_sv_lan": True,
                "desktop_input": False,
                "display": os.environ.get("DISPLAY"),
                "wayland_display": os.environ.get("WAYLAND_DISPLAY"),
                "engine_sha256": digest((root / "bin/linux64/engine.so").read_bytes()),
                "materials_sha256": material_hashes(game),
            }
            (output / "runtime.json").write_text(json.dumps(report, indent=2) + "\n")
        finally:
            if process.poll() is None:
                process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
            finish_private_launch(game)


def pixels(path):
    data = path.read_bytes()
    header = re.match(rb"P6\s+(\d+)\s+(\d+)\s+255\s", data)
    assert header
    width, height = map(int, header.groups())
    body = data[header.end() :]
    assert (width, height) == (1280, 720) and len(body) == width * height * 3
    left, top, right, bottom = REGION
    return [
        body[(y * width + x) * 3 : (y * width + x) * 3 + 3]
        for y in range(top, bottom)
        for x in range(left, right)
    ]


def frame_stats(path):
    colors = pixels(path)
    return {
        "pixels": len(colors),
        "mean_rgb": [
            sum(color[axis] for color in colors) / len(colors) for axis in range(3)
        ],
        "red": sum(
            color[0] > 50 and color[0] > color[1] + 30 and color[0] > color[2] + 30
            for color in colors
        ),
        "blue": sum(
            color[2] > 50 and color[2] > color[1] + 30 and color[2] > color[0] + 30
            for color in colors
        ),
    }


def compare(first, second):
    a, b = pixels(first), pixels(second)
    differences = [
        [abs(x - y) for x, y in zip(left, right, strict=True)]
        for left, right in zip(a, b, strict=True)
    ]
    return {
        "pixels": len(a),
        "changed": sum(max(row) > 12 for row in differences),
        "mean_difference": sum(sum(row) for row in differences) / (len(a) * 3),
    }


def native_cases(args, fixture):
    assert shutil.which("cage") and shutil.which("grim"), "Cage and grim are required"
    common = args.output / "runtime-materials"
    for case in fixture["cases"].values():
        assert material_hashes(Path(case["fixture_game"])) == case["materials_sha256"]
    shutil.copytree(
        Path(fixture["cases"]["red"]["fixture_game"]) / "materials",
        common / "materials",
    )
    (common / f"materials/{TOP}.vmt").write_text(top_material(BOTTOMS["red"]))
    expected_materials = material_hashes(common)
    results = {}
    for name, case in fixture["cases"].items():
        output = args.output / name
        command = [
            sys.executable,
            "-B",
            str(Path(__file__).resolve()),
            "--worker",
            "--bsp",
            case["bsp"],
            "--fixture-game",
            str(common),
            "--variant",
            name,
            "--output",
            str(output),
            "--game-dir",
            str(args.game_dir.resolve()),
        ]
        runtime.private_compositor(command, output, timeout=63)
        results[name] = json.loads((output / "runtime.json").read_text())
        assert material_hashes(Path(results[name]["game"])) == results[name][
            "materials_sha256"
        ]
        results[name]["cloud_material_imports"] = verify_runtime_materials(
            results[name]["materials_sha256"],
            expected_materials,
            json.loads((output / "private-preservation.json").read_text()),
        )
        results[name]["frames"] = {
            view: frame_stats(output / f"{view}-0.ppm") for view in VIEWS
        }
        results[name]["repeat"] = {
            view: compare(output / f"{view}-0.ppm", output / f"{view}-1.ppm")
            for view in VIEWS
        }
        assert all(
            row["mean_difference"] < 2 and row["changed"] < row["pixels"] * 0.01
            for row in results[name]["repeat"].values()
        ), results[name]
    assert results["red"]["pid"] != results["blue"]["pid"]
    assert results["red"]["materials_sha256"] == results["blue"]["materials_sha256"]
    differences = {
        view: compare(
            args.output / "red" / f"{view}-0.ppm",
            args.output / "blue" / f"{view}-0.ppm",
        )
        for view in VIEWS
    }
    for view in ["above", "away"]:
        assert (
            differences[view]["mean_difference"] < 2
            and differences[view]["changed"] < differences[view]["pixels"] * 0.01
        ), (view, differences)
    red, blue = results["red"]["frames"]["below"], results["blue"]["frames"]["below"]
    assert red["red"] > red["pixels"] * 0.6 and red["blue"] < red["pixels"] * 0.01, red
    assert (
        blue["blue"] > blue["pixels"] * 0.6 and blue["red"] < blue["pixels"] * 0.01
    ), blue
    assert differences["below"]["changed"] > differences["below"]["pixels"] * 0.6
    return {
        "cases": results,
        "fixture_materials_sha256": expected_materials,
        "runtime_materials_sha256": results["red"]["materials_sha256"],
        "runtime_top_bottom": BOTTOMS["red"],
        "runtime_materials_identical": True,
        "roi": REGION,
        "differences": differences,
        "bottom_material_dependent_pixels": True,
        "above_control_stable": True,
        "submerged_away_control_stable": True,
    }


def main():
    parser = argparse.ArgumentParser(
        description="Compile paired Water underside materials and optionally verify their private native pixels"
    )
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument(
        "--game-dir",
        type=Path,
        default=Path.home()
        / ".local/share/Steam/steamapps/common/Counter-Strike Source",
    )
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--runtime", action="store_true")
    parser.add_argument(
        "--reuse-fixture",
        type=Path,
        help="Reuse a successful compile.json with --runtime in a fresh output directory",
    )
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--bsp", type=Path, help=argparse.SUPPRESS)
    parser.add_argument("--fixture-game", type=Path, help=argparse.SUPPRESS)
    parser.add_argument("--variant", choices=list(BOTTOMS), help=argparse.SUPPRESS)
    args = parser.parse_args()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=args.worker)
    if args.worker:
        worker(args)
        return
    try:
        if args.reuse_fixture:
            assert args.runtime, "--reuse-fixture requires --runtime"
            fixture = json.loads(args.reuse_fixture.resolve(strict=True).read_text())
            for name, case in fixture["cases"].items():
                assert (
                    check_native(Path(case["bsp"]).read_bytes(), BOTTOMS[name])
                    == case["native"]
                )
        else:
            fixture = compile_fixture(args)
        report = {**fixture, "runtime_checked": args.runtime, "desktop_input": False}
        if args.runtime:
            report["native"] = native_cases(args, fixture)
        (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps({"output": str(args.output), "runtime_checked": args.runtime}))
    except Exception as error:
        (args.output / "failure.json").write_text(
            json.dumps(
                {
                    "error": type(error).__name__,
                    "message": str(error),
                    "runtime_requested": args.runtime,
                },
                indent=2,
            )
            + "\n"
        )
        raise


if __name__ == "__main__":
    main()
