import argparse
from functools import partial
import hashlib
import json
import re
import shutil
import struct
import sys
from pathlib import Path

import validate_hdr_lighting as hdr
import validate_native_runtime as runtime

PROP_CONTROLS = {
    "r_proplightingfromdisk": 1, "r_debugrandomstaticlighting": 0,
    "r_proplightingpooling": 0, "r_rootlod": 0, "mat_queue_mode": 0,
}
CAMERA = "setpos -192 -240 160; setang 25 50 0"
ENTITIES = b'''
{ "classname" "info_player_counterterrorist" "origin" "192 192 8" "angles" "0 180 0" }
{ "classname" "logic_script" "origin" "0 0 64" "targetname" "hammerr_observer" "vscripts" "hammerr_observer.nut" "thinkfunction" "HammeRRThink" }
'''
LOAD_ERRORS = re.compile(
    r"Error (?:Vertex|Index) File|Level unlit|(?:error|failed|unable|not found|couldn.t|can.t)[^\n]*"
    r"(?:\.mdl|\.vvd|\.vtx|\.vhv|\.vmt|\.vtf|static prop|material)|"
    r"(?:\.mdl|\.vvd|\.vtx|\.vhv)[^\n]*(?:error|failed|not found)", re.IGNORECASE,
)
STOCK_DIAGNOSTICS = {
    'Error: Material "___fillrate_0" uses unknown shader "FillRate"',
    'Error: Material "___debugnormalmap_1" uses unknown shader "DebugNormalMap"',
    'Error: Material "___debugdrawenvmapmask_2" uses unknown shader "DebugDrawEnvmapMask"',
    'Error: Material "___debugdepth_3" uses unknown shader "DebugDepth"',
    'Error: Material "___debugdepth_4" uses unknown shader "DebugDepth"',
    'Error: Material "debug/debugluxels" uses unknown shader "DebugLuxels"',
    'Error: Material "models/humans/female/pupil_r" uses unknown shader "Eyeball"',
    'Error: Material "models/humans/female/pupil_l" uses unknown shader "Eyeball"',
}
OPTIONAL_REFLECTION_DIAGNOSTIC = "##### CTexture::LoadTextureBitsFromFile couldn't find materials/maps/hammerr_runtime/cubemapdefault.vtf"


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def verify_owned_engine(process, records, engine_root):
    starts = [row for row in records if row["event"] == "start"]
    require(len(starts) == 1, "Hardware observer has no unique engine process")
    engine_pid = starts[0]["pid"]
    current, lineage = engine_pid, []
    for _ in range(16):
        lineage.append(current)
        if current == process.pid:
            break
        status = (Path("/proc") / str(current) / "stat").read_text()
        fields = status[status.rfind(")") + 2:].split()
        require(fields[0] not in {"Z", "X"}, "Hardware observer process is no longer live")
        current = int(fields[1])
        require(current > 0, "Hardware observer is outside the owned private process tree")
    require(lineage[-1] == process.pid, "Hardware observer is outside the owned private process tree")
    executable = (Path("/proc") / str(engine_pid) / "exe").resolve(strict=True)
    require(executable == (engine_root / "cstrike_linux64").resolve(strict=True), "Hardware observer has the wrong executable")
    return {"engine_pid": engine_pid, "launcher_pid": process.pid, "engine_in_owned_process_tree": True,
            "engine_executable": str(executable), "engine_lineage": lineage}


def stage_bsp(source, destination):
    require(1036 <= source.stat().st_size <= 512 * 1024**2, "Input BSP exceeds its size bound")
    original = source.read_bytes()
    entities = hdr.lump(original, 0)
    require(entities.endswith(b"\0") and b"\0" not in entities[:-1], "Invalid entity termination")
    require(b'"info_player_terrorist"' in entities, "Fixture requires its original T spawn")
    require(b"hammerr_observer" not in entities, "Fixture already has the private observer")
    require(struct.unpack("<I", hdr.lump(original, 59))[0] & 3 == 3, "Fixture requires both baked-prop map flags")
    require(b"hammerr/generated-prop-lighting.json" in hdr.lump(original, 40), "Fixture requires the compiler VHV manifest")
    payload = entities[:-1] + ENTITIES + b"\0"
    staged = bytearray(original)
    staged.extend(b"\0" * (-len(staged) % 4))
    offset = len(staged)
    staged.extend(payload)
    require(len(staged) <= 512 * 1024**2, "Private BSP exceeds its size bound")
    struct.pack_into("<4i", staged, 8, offset, len(payload), 0, 0)
    require(staged[:8] == original[:8] and staged[24:len(original)] == original[24:],
            "Entity-only staging changed another original byte or lump descriptor")
    destination.write_bytes(staged)
    return {"input_sha256": digest(original), "staged_sha256": digest(staged),
            "entity_only_append": True, "game_lump_absolute_offsets_preserved": True,
            "geometry_pak_and_vhv_manifest_bytes_preserved": True}


def worker(args):
    controls = {**PROP_CONTROLS, "r_lod": args.lod, "r_proplightingfromdisk": args.disk}
    hdr.CONTROLS.update(controls)
    camera = "setpos -192 -176 48; setang 35 42 0"
    queries = "mat_hdr_level; mat_force_tonemap_scale;"
    require(hdr.OBSERVER.count(camera) == hdr.OBSERVER.count(queries) == 1, "HDR observer contract changed")
    hdr.OBSERVER = hdr.OBSERVER.replace(camera, CAMERA).replace(queries, "; ".join(controls) + "; " + queries)
    report = {"success": False, "draw_lod": args.lod, "root_lod": 0, "disk_lighting": args.disk, "camera": CAMERA}
    receipt = args.output / "runtime.json"
    try:
        original = args.bsp.read_bytes()
        game = runtime.private_game(args.game_dir, args.output, args.bsp)
        shutil.copytree(args.fixture_game / "models", game / "models", dirs_exist_ok=True)
        hdr.worker(args, process_observer=partial(verify_owned_engine, engine_root=args.game_dir))
        report.update(json.loads(receipt.read_text()))
        console = game / "hammerr-runtime.log"
        require(console.stat().st_size <= 2 * 1024**2, "Native console exceeds its size bound")
        messages = [line.strip() for line in console.read_text(errors="replace").splitlines() if LOAD_ERRORS.search(line)]
        stock = [line for line in messages if line in STOCK_DIAGNOSTICS]
        optional = [line for line in messages if line == OPTIONAL_REFLECTION_DIAGNOSTIC]
        failures = [line for line in messages if line not in STOCK_DIAGNOSTICS and line != OPTIONAL_REFLECTION_DIAGNOSTIC]
        require(not failures, f"Engine asset or lighting load errors: {failures}")
        require(args.bsp.read_bytes() == original and (game / "maps/hammerr_runtime.bsp").read_bytes() == original,
                "Native validation changed its staged BSP")
        starts = [row for row in report["probe_records"] if row["event"] == "start"]
        require(len(starts) == 1 and starts[0]["pid"] == report["engine_pid"] and report["engine_in_owned_process_tree"], "Hardware observer process identity differs")
        report.update({"success": True, "requested_asset_errors_absent": True,
                       "stock_engine_diagnostics": stock, "optional_reflection_diagnostics": optional,
                       "staged_bsp_sha256": digest(original), "frame_color": hdr.frame_color(args.output / "game.ppm")})
    except BaseException as error:
        report.update({"success": False, "error": str(error)})
        raise
    finally:
        existing = json.loads(receipt.read_text()) if receipt.exists() else {}
        existing.update(report)
        receipt.write_text(json.dumps(existing, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser(description="Private compiler VHV LDR/HDR and forced-LOD smoke validation")
    parser.add_argument("--bsp", type=Path, required=True)
    parser.add_argument("--fixture-game", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--game-dir", type=Path, default=Path.home() / ".local/share/Steam/steamapps/common/Counter-Strike Source")
    parser.add_argument("--probe-library", type=Path, default=hdr.ROOT / "target/debug/hammerr_hdr_probe.so")
    parser.add_argument("--disk-off-control", action="store_true", help="Add a fresh LDR/LOD 0 disk-off case paired with ldr-lod0")
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--hdr-level", type=int, choices=[0, 2], help=argparse.SUPPRESS)
    parser.add_argument("--lod", type=int, choices=[0, 1], help=argparse.SUPPRESS)
    parser.add_argument("--disk", type=int, choices=[0, 1], default=1, help=argparse.SUPPRESS)
    args = parser.parse_args()
    for name in ["bsp", "fixture_game", "game_dir", "probe_library"]:
        setattr(args, name, getattr(args, name).resolve(strict=True))
    args.output = args.output.resolve()
    require(args.output.is_relative_to((hdr.ROOT / "target/validation").resolve()), "Output must be on disk under ROOT/target/validation")
    require(all((args.fixture_game / name).is_dir() for name in ["models", "materials"]), "Fixture game requires local models and materials")
    if args.worker:
        require(args.lod in (0, 1) and args.hdr_level in (0, 2), "Worker requires channel and draw LOD")
        require(args.disk == 1 or (args.hdr_level == 0 and args.lod == 0), "Disk-off control requires LDR draw LOD 0")
        worker(args)
        return
    require(not args.output.exists(), "Use a fresh output directory")
    require(shutil.which("cage") and shutil.which("grim"), "Cage and grim are required")
    args.output.mkdir(parents=True)
    staged = args.output / "private.bsp"
    report = stage_bsp(args.bsp, staged)
    report.update({"success": False, "bsp": str(args.bsp), "fixture_game": str(args.fixture_game),
                   "scope": "Compiler dual-channel and forced draw-LOD smoke; frame colors alone do not prove VHV consumption",
                   "frame_color_roi": [320, 360, 960, 648], "pixel_dominance_asserted": False,
                   "disk_off_control_requested": args.disk_off_control, "cases": {}})
    cases = [(f"{channel}-lod{lod}", level, lod, 1) for channel, level in [("ldr", 0), ("hdr", 2)] for lod in (0, 1)]
    if args.disk_off_control:
        cases.append(("ldr-lod0-disk-off", 0, 0, 0))
    try:
        for name, level, lod, disk in cases:
            output = args.output / name
            command = [sys.executable, "-B", str(Path(__file__).resolve()), "--worker", "--bsp", str(staged),
                       "--fixture-game", str(args.fixture_game), "--output", str(output), "--game-dir", str(args.game_dir),
                       "--probe-library", str(args.probe_library), "--hdr-level", str(level), "--lod", str(lod), "--disk", str(disk)]
            runtime.private_compositor(command, output, timeout=65)
            result = json.loads((output / "runtime.json").read_text())
            require(result["success"], f"Native case failed: {name}")
            require(result["staged_bsp_sha256"] == report["staged_sha256"], "Native cases used different staged BSP bytes")
            report["cases"][name] = result
        require(len({result["pid"] for result in report["cases"].values()}) == len(cases), "Native cases reused a process")
        require(len({result["engine_pid"] for result in report["cases"].values()}) == len(cases), "Native cases reused an engine process")
        if args.disk_off_control:
            on, off = [report["cases"][name] for name in ["ldr-lod0", "ldr-lod0-disk-off"]]
            require(on["controls"]["r_proplightingfromdisk"] == 1 and off["controls"]["r_proplightingfromdisk"] == 0,
                    "Paired disk controls were not queried")
            require({key: value for key, value in on["controls"].items() if key != "r_proplightingfromdisk"}
                    == {key: value for key, value in off["controls"].items() if key != "r_proplightingfromdisk"},
                    "Paired queried controls differ beyond disk lighting")
            report["disk_control_pair"] = {"staged_bsp_sha256": report["staged_sha256"], "hdr_level": 0, "draw_lod": 0,
                "root_lod": 0, "camera": CAMERA, "model_roi_difference_measured": False,
                "planned_roi_max_pixels": 250000, "planned_high_magnitude_component_threshold": 64, "outputs": {}}
            for name in ["ldr-lod0", "ldr-lod0-disk-off"]:
                report["disk_control_pair"]["outputs"][name] = {
                    kind: {"path": str(args.output / name / file), "sha256": digest((args.output / name / file).read_bytes())}
                    for kind, file in [("receipt", "runtime.json"), ("png", "game.png"), ("ppm", "game.ppm")]}
        require(digest(args.bsp.read_bytes()) == report["input_sha256"], "Original compiler BSP changed")
        report["success"] = True
    except BaseException as error:
        report["error"] = str(error)
        raise
    finally:
        (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"output": str(args.output), "cases": len(cases), "success": True, "pixel_dominance_asserted": False}))


if __name__ == "__main__":
    main()
