import argparse
import hashlib
import json
import subprocess
from pathlib import Path

import prop_bounce_reference as reference
import validate_cubemap_capture_probe as probe
from validate_prop_vertex_lighting import MANIFEST, pak_and_flags, read_vhv, require


ROOT = Path(__file__).resolve().parent.parent
LAYOUT = [{"lod": 0, "vertex_count": 6}]


def compile_map(binary, fixtures, output, depth, bake):
    command = [str(binary), str(fixtures / "depth-room.vmf"), "--game", str(fixtures / "gameinfo.txt"),
               "-o", str(output), "--lighting", "direct_ldr_hdr", "--sky-quality", "fast", "--bounces", str(depth)]
    if bake:
        command.append("--bake-static-prop-vertices")
    compiled = subprocess.run(command, capture_output=True, text=True, timeout=60)
    output.with_suffix(".log").write_text(compiled.stdout + compiled.stderr)
    require(compiled.returncode == 0 and output.is_file(), f"Compiler rejected {output.name}; see its log")
    return output.read_bytes()


def matches(color, interval):
    return all(low <= value <= high for value, (low, high) in zip(color, interval, strict=True))


def inspect(data, world, fixtures, depth, oracle):
    actual_lumps, world_lumps = probe.bsp_lumps(data), probe.bsp_lumps(world)
    require(all(actual == expected for index, (actual, expected) in enumerate(zip(actual_lumps, world_lumps, strict=True))
                if index not in (40, 59)), "Enabling prop reception changed world lighting, geometry or native prop placement")
    entries, flags = pak_and_flags(data)
    require(flags & 3 == 3 and set(entries) == {MANIFEST, "sp_0.vhv", "sp_hdr_0.vhv"}, "Incomplete or extra generated assets")
    manifest = json.loads(entries[MANIFEST])
    require(manifest["channels"] == ["ldr", "hdr"] and len(manifest["receivers"]) == 1, "Receiver/channel identity differs")
    receiver = manifest["receivers"][0]
    require(receiver["ordinal"] == 0 and receiver["checksum"] == 619 and receiver["meshes"] == LAYOUT
            and receiver["model"] == "models/oracle/receiver.mdl", "Wrong native ordinal, model or hardware layout")
    require(manifest["missing_sources"] == [], "Fixture source closure has unexpected missing assets")
    for source in manifest["sources"]:
        raw = (fixtures / "game" / source["path"]).read_bytes()
        require(len(raw) == source["bytes"] and hashlib.sha256(raw).hexdigest() == source["sha256"],
                f"Source bytes differ: {source['path']}")
    textures = [source["path"] for source in manifest["sources"] if source["path"].endswith(".vtf")]
    require(textures == (["materials/hammerr628/room.vtf"] if depth else []), "Reflectivity source identity scope differs")
    measurements = {}
    for channel in ("ldr", "hdr"):
        name = f"sp_{'hdr_' if channel == 'hdr' else ''}0.vhv"
        report, colors = read_vhv(entries[name], 619, LAYOUT)
        if depth:
            expected = oracle["expected"][f"N{depth}"]["hardware_vertices"][channel]
            require(all(matches(color, row["bgra_interval"]) for color, row in zip(colors, expected, strict=True)),
                    f"N{depth} {channel} colors do not match independent W{depth - 1} transfer: {colors}")
            wrong = oracle["wrong_final_source_predictions"][f"N{depth}"][channel]
            require(any(not matches(color, wrong[root]["bgra_interval"])
                        for color, root in zip(colors, reference.HARDWARE, strict=True)),
                    f"N{depth} {channel} does not distinguish the wrong final-world source")
        else:
            require(all(color == (0, 0, 0, 255) for color in colors), "Downward receiver unexpectedly has direct light")
        asset = next(row for row in manifest["assets"] if row["path"] == name)
        require(asset["sha256"] == report["sha256"] and asset["bytes"] == report["bytes"], "Generated asset identity differs")
        measurements[channel] = {"vhv": report, "bgra": colors}
    return {"sha256": hashlib.sha256(data).hexdigest(), "channels": measurements,
            "world_lumps_unchanged": True, "world_source_depth": max(0, depth - 1)}


def validate(args):
    args.output.mkdir(parents=True, exist_ok=False)
    fixtures = args.output / "fixtures"
    reference.fixtures(fixtures, "strong")
    files = json.loads((fixtures / "files.json").read_text())
    worlds = [args.output / f"W{depth}.bsp" for depth in range(5)]
    for depth, output in enumerate(worlds):
        compile_map(args.binary, fixtures, output, depth, False)
    oracle = reference.reference(worlds, fixtures / "contract.json", args.output / "reference.json")
    results = {}
    for depth, world in enumerate(worlds):
        output = args.output / f"P{depth}.bsp"
        data = compile_map(args.binary, fixtures, output, depth, True)
        results[f"N{depth}"] = inspect(data, world.read_bytes(), fixtures, depth, oracle)
        (args.output / "results.json").write_text(json.dumps(results, indent=2) + "\n")
    for name, expected in files.items():
        raw = (fixtures / name).read_bytes()
        require(expected == {"bytes": len(raw), "sha256": hashlib.sha256(raw).hexdigest()}, f"Compiler changed fixture input {name}")
    print("Passed all five prop depths in both channels, with independent transfer, wrong-final-source separation and exact world parity.")


def main():
    parser = argparse.ArgumentParser(description="Validate static-prop bounce depth against an independent floor-transfer reference.")
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/hammerr-compile")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.binary = args.binary.expanduser().resolve()
    args.output = args.output.expanduser().resolve()
    validate(args)


if __name__ == "__main__":
    main()
