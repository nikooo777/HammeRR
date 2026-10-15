import argparse
import json
import shutil
import struct
import subprocess
from pathlib import Path

import validate_native_runtime as runtime
from validate_direct_lighting import floor_samples
from validate_face_lighting import DEFAULT_BINARY, ROOT, Client
from validate_visibility import ROOM


def texture(cutout):
    data = bytearray(80)
    data[:4] = b"VTF\0"
    struct.pack_into("<I", data, 4, 7)
    struct.pack_into("<I", data, 8, 2)
    struct.pack_into("<I", data, 12, 80)
    struct.pack_into("<HH", data, 16, 64, 64)
    struct.pack_into("<I", data, 20, 0x1001 if cutout else 1)
    struct.pack_into("<H", data, 24, 1)
    data[56] = 1
    struct.pack_into("<i", data, 57, -1)
    struct.pack_into("<H", data, 63, 1)
    for y in range(64):
        for x in range(64):
            alpha = 255 if not cutout or (x // 8 + y // 8) % 4 == 0 else 0
            data.extend([165, 190, 150, alpha] if cutout else [190, 190, 190, alpha])
    return data


def horizontal_pose(bounds):
    low, high = bounds
    center = [(a + b) / 2 for a, b in zip(low, high, strict=True)]
    axis = min(range(3), key=lambda i: high[i] - low[i])
    if axis == 0:
        angles, rotated = "90 0 0", [center[2], center[1], -center[0]]
    elif axis == 1:
        angles, rotated = "0 0 90", [center[0], -center[2], center[1]]
    else:
        angles, rotated = "0 0 0", center
    return angles, [-rotated[0], -rotated[1], 112 - rotated[2]]


def compile_cases(args):
    private = runtime.private_game(args.game_dir.resolve(), args.output / "fixture")
    metadata = json.loads(subprocess.check_output([
        str(args.fixture_helper.resolve()),
        str(args.game_dir.resolve() / "cstrike/gameinfo.txt"),
        args.model,
        str(private),
    ], text=True))
    (private / "materials/hammerr").mkdir(parents=True, exist_ok=True)
    (private / "materials/hammerr/prop_cutout.vtf").write_bytes(texture(True))
    (private / "materials/hammerr/receiver.vtf").write_bytes(texture(False))
    (private / "materials/hammerr/receiver.vmt").write_text("LightmappedGeneric { $basetexture hammerr/receiver }\n")
    angles, origin = horizontal_pose(metadata["bounds"])
    client = Client(None, args.binary.resolve(), args.output, private / "gameinfo.txt")
    try:
        client.call("new_map", {"discard_changes": True})
        for low, high in ROOM:
            client.call("create_block", {"mins": low, "maxs": high, "material": "hammerr/receiver"})
        for classname, origin_player in [("info_player_terrorist", [-160, -128, 8]), ("info_player_counterterrorist", [160, 128, 8])]:
            client.call("create_entity", {"classname": classname, "origin": origin_player})
        client.call("create_entity", {"classname": "light", "origin": [0, 0, 224], "properties": {"_light": "255 220 180 200", "_quadratic_attn": "1"}})
        client.call("create_entity", {"classname": "logic_script", "origin": [0, 0, 32], "properties": {"targetname": "hammerr_observer", "vscripts": "hammerr_observer.nut", "thinkfunction": "HammeRRThink"}})
        client.call("create_entity", {"classname": "prop_physics", "origin": [192, 192, 64], "properties": {"targetname": "physics_crate", "model": "models/props_junk/wood_crate001a.mdl"}})
        client.call("create_entity", {"classname": "prop_static", "origin": origin, "properties": {"model": args.model, "solid": "0", "angles": angles}})
        client.call("set_compile_settings", {"lighting": "direct_ldr", "physics": "engine"})
        vmf = args.output / "prop-cutouts.vmf"
        client.call("save_map", {"path": str(vmf)})
        before = vmf.read_bytes()
        history = client.call("get_map_info")["undo"]
        model_path = private / args.model
        flagged = model_path.read_bytes()
        opaque = bytearray(flagged)
        struct.pack_into("<I", opaque, 152, struct.unpack_from("<I", opaque, 152)[0] & ~0x00040000)
        cases = []
        for mode, model in [("opaque", opaque), ("cutout", flagged)]:
            model_path.write_bytes(model)
            bsp = args.output / f"{mode}.bsp"
            result = client.call("compile_map", {"path": str(bsp)})
            assert result["success"], result
            alpha_triangles = result["stats"]["prop_shadow_alpha_triangles"]
            assert (alpha_triangles > 0) == (mode == "cutout")
            cases.append((mode, bsp, result["stats"]))
        assert vmf.read_bytes() == before and client.call("get_map_info")["undo"] == history
        samples = [{tuple(point): color for point, color, _, _ in floor_samples(bsp.read_bytes())} for _, bsp, _ in cases]
        assert samples[0].keys() == samples[1].keys()
        transmitted = [point for point, color in samples[0].items() if color == [0, 0, 0] and samples[1][point][0] > 10]
        blocked = [point for point, color in samples[0].items() if color == [0, 0, 0] and samples[1][point] == color]
        assert len(transmitted) >= 2 and len(blocked) >= 2, {"transmitted": transmitted, "blocked": blocked}
        metadata.update({"angles": angles, "origin": origin, "transmitted_luxels": transmitted, "blocked_luxels": blocked, "cases": [{"mode": mode, "bsp": str(bsp), "stats": stats} for mode, bsp, stats in cases]})
        (args.output / "fixture.json").write_text(json.dumps(metadata, indent=2) + "\n")
        return private, cases, metadata
    finally:
        client.close()


def worker(args):
    fixture = args.fixture_game.resolve()
    private = runtime.private_game(args.game_dir.resolve(), args.output, args.bsp)
    for directory in ["models", "materials"]:
        shutil.copytree(fixture / directory, private / directory, dirs_exist_ok=True)
    args.observe_seconds = 24
    runtime.capture_worker(args, "setpos -160 -160 8; setang 15 45 0; mat_fullbright")
    text = (private / "hammerr-runtime.log").read_text(errors="replace")
    assert "sv_lan" in text and "= 1" in text, text[-4000:]
    assert "Error Vertex File" not in text and "Error Index File" not in text and "Error Material" not in text, text[-4000:]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--fixture-helper", type=Path, default=ROOT / "target/debug/examples/prop_shadow_fixture")
    parser.add_argument("--game-dir", type=Path, default=Path.home() / ".local/share/Steam/steamapps/common/Counter-Strike Source")
    parser.add_argument("--model", default="models/props_c17/fence01a.mdl")
    parser.add_argument("--output", type=Path, default=Path("/tmp/hammerr-prop-cutouts-native"))
    parser.add_argument("--runtime", action="store_true")
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--bsp", type=Path, help=argparse.SUPPRESS)
    parser.add_argument("--fixture-game", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    if args.worker:
        worker(args)
        return
    if not args.fixture_helper.is_file():
        raise RuntimeError("Build the fixture helper first: cargo build -p compiler --example prop_shadow_fixture")
    private, cases, metadata = compile_cases(args)
    if args.runtime:
        for mode, bsp, _ in cases:
            runtime.private_display(Path(__file__), args, bsp, extra_flags=["--fixture-game", str(private)], output=args.output / mode)
        difference = runtime.compare_frames(args.output, names=("opaque", "cutout"))
        assert difference["changed_pixels"] > 100, difference
        metadata["runtime_difference"] = difference
    (args.output / "results.json").write_text(json.dumps(metadata, indent=2) + "\n")
    print(json.dumps({"output": str(args.output), "transmitted_luxels": len(metadata["transmitted_luxels"]), "blocked_luxels": len(metadata["blocked_luxels"]), "runtime": args.runtime}))


if __name__ == "__main__":
    main()
