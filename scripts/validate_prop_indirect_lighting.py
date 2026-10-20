import argparse
import hashlib
import json
import subprocess
from pathlib import Path

from prop_indirect_fixtures import EXPECTED, generate
from validate_hdr_lighting import decode_channel, lump, rgbexp
from validate_prop_vertex_lighting import MANIFEST, pak_and_flags, read_vhv, require


ROOT = Path(__file__).resolve().parent.parent
QUALITIES = {32: "fast", 128: "balanced", 512: "detailed"}


def floor_colors(data, channel):
    decoded = decode_channel(data, channel)
    samples = lump(data, 8 if channel == "ldr" else 53, (1,))
    colors = []
    for row in decoded["floor"]:
        face = decoded["faces"][row["face"]]
        count = (face[16] + 1) * (face[17] + 1)
        colors.extend(rgbexp(samples[face[12]:face[12] + count * 4]))
    require(colors, f"Missing {channel} source floor")
    return colors


def floor_expected(name, channel):
    if name == "a-red-blue":
        return EXPECTED["red_blue_N1"][f"floor_W0_{channel}"]
    if name == "b-sky-no-doublecount":
        return [0, 0, 64] if channel == "ldr" else [0, 128, 0]
    return EXPECTED["source"][f"white_floor_W0_{channel}"]


def expected_intervals(profile, bounce, channel):
    name = profile[f"N{bounce}"]
    if name == "black":
        return [[value, value] for value in EXPECTED["black"]]
    expected = EXPECTED[name]
    if name == "blocked_N1":
        values = expected["both_channels_bgra"]
    elif name in ("up_normal", "sky_control"):
        values = expected[f"{channel}_bgra"]
    else:
        return expected[channel]["bgra_interval"]
    return [[value, value] for value in values]


def inspect(path, fixtures, name, profile, bounce):
    data = path.read_bytes()
    entries, flags = pak_and_flags(data)
    require(flags & 3 == 3, "One-bounce fixture did not publish both native lighting bits")
    manifest = json.loads(entries[MANIFEST])
    require(manifest["channels"] == ["ldr", "hdr"], "Missing independent lighting channel")
    expected = EXPECTED["hardware"]
    require(len(manifest["receivers"]) == 1, "Opted-out blocker became a receiver")
    receiver = manifest["receivers"][0]
    layout = [{"lod": 0, "vertex_count": 6}]
    require(receiver["ordinal"] == expected["receiver_ordinal"] and receiver["checksum"] == expected["checksum"]
            and receiver["meshes"] == layout, "Fixture lost its literal duplicate hardware stream")
    require({row["path"] for row in manifest["assets"]} == {"sp_0.vhv", "sp_hdr_0.vhv"}, "Extra or missing generated sibling")
    require(manifest["missing_sources"] == [], "Fixture source closure has unexpected missing assets")
    for source in manifest["sources"]:
        raw = (fixtures / "game" / source["path"]).read_bytes()
        require(len(raw) == source["bytes"] and hashlib.sha256(raw).hexdigest() == source["sha256"],
                f"Source bytes differ: {source['path']}")
    vtf = {row["path"] for row in manifest["sources"] if row["path"].endswith(".vtf")}
    source_name = "black" if name == "b-sky-no-doublecount" else "reflector"
    require((f"materials/hammerr619/{source_name}.vtf" in vtf) == (bounce == 1),
            "Direct and reflected source identity scopes are not distinct")
    measurements = {}
    for channel in ("ldr", "hdr"):
        asset = f"sp_{'hdr_' if channel == 'hdr' else ''}0.vhv"
        report, colors = read_vhv(entries[asset], expected["checksum"], layout)
        intervals = expected_intervals(profile, bounce, channel)
        require(len(colors) == 6 and all(all(low <= value <= high for value, (low, high)
                in zip(color, intervals, strict=True)) for color in colors),
                f"{name} N{bounce} {channel}: {colors} differs from literal {intervals}")
        require(all(color == colors[0] for color in colors), "Duplicate/root hardware colors disagree in a constant field")
        floor = floor_colors(data, channel)
        baseline = floor_expected(name, channel)
        if bounce == 0:
            require(all(color == baseline for color in floor),
                    f"{name} {channel} W0 is not the independently required uniform source field")
        if name == "a-source-timing-fast" and bounce == 1:
            require(all(all(value > 2 * original for value, original in zip(color, baseline, strict=True)) for color in floor),
                    "Timing fixture W1 is not sufficiently above W0 to distinguish the source handoff")
        measurements[channel] = {"vhv": report, "unique_bgra": sorted(set(colors)),
                                 "floor_samples": len(floor), "floor_minimum": [min(c[axis] for c in floor) for axis in range(3)],
                                 "floor_maximum": [max(c[axis] for c in floor) for axis in range(3)]}
    return {"sha256": hashlib.sha256(data).hexdigest(), "channels": measurements}


def validate(args):
    args.output.mkdir(parents=True, exist_ok=False)
    fixtures = args.output / "fixtures"
    fixtures.mkdir()
    generate(fixtures)
    results = {}
    for name, profile in EXPECTED["profiles"].items():
        for count in profile["quality"]:
            quality = QUALITIES[count]
            for bounce in (0, 1):
                label = f"{name}-{quality}-N{bounce}"
                output = args.output / f"{label}.bsp"
                command = [str(args.binary), str(fixtures / f"{name}.vmf"), "--game", str(fixtures / "gameinfo.txt"),
                           "-o", str(output), "--lighting", "direct_ldr_hdr", "--bounces", str(bounce),
                           "--sky-quality", quality, "--bake-static-prop-vertices"]
                compiled = subprocess.run(command, capture_output=True, text=True, timeout=120)
                (args.output / f"{label}.log").write_text(compiled.stdout + compiled.stderr)
                require(compiled.returncode == 0 and output.is_file(), f"Compiler rejected {label}; see its log")
                results[label] = inspect(output, fixtures, name, profile, bounce)
                (args.output / "results.json").write_text(json.dumps(results, indent=2) + "\n")
    print(f"Passed {len(results)} compiler cases: independent energy, channels, blockers, height, normals, source timing and nearby sources.")


def main():
    parser = argparse.ArgumentParser(description="Check one-bounce prop lighting against literal numeric compiler fixtures.")
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/hammerr-compile")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.binary = args.binary.expanduser().resolve()
    args.output = args.output.expanduser().resolve()
    validate(args)


if __name__ == "__main__":
    main()
