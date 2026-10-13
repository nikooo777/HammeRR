import argparse
import itertools
import json
import os
import re
import struct
import subprocess
import sys
from pathlib import Path

import validate_native_runtime as runtime
from validate_authored_planes import records
from validate_compile_jobs import wait_job
from validate_direct_lighting import FACE_FORMAT, floor_samples, lightmaps
from validate_face_lighting import ROOT, Client, detail
from validate_visibility import ROOM

OBSERVER = runtime.OBSERVER.replace(
    '    local player = Entities.FindByClassname(null, "player");',
    """
    local center = TraceLine(Vector(84, 84, 160), Vector(84, 84, -8), null);
    local first = TraceLine(Vector(28, 196, 160), Vector(28, 196, -8), null);
    local second = TraceLine(Vector(196, 28, 160), Vector(196, 28, -8), null);
    local terrain_prop = Entities.FindByName(null, "terrain_crate");
    if (terrain_prop != null) {
        local pos = terrain_prop.GetOrigin();
        printl("HAMMERR_TERRAIN_PROP " + pos.x + " " + pos.y + " " + pos.z);
    }
    printl("HAMMERR_TERRAIN_TRACE " + center + " " + first + " " + second);
    local player = Entities.FindByClassname(null, "player");
""",
).replace("setpos -64 -64 32", "setpos 84 84 96")


def displacement(flags, power=3, origin=(0, 0)):
    size = (1 << power) + 1
    rows = []
    for row in range(size):
        values = []
        for col in range(size):
            x, y = col * 224 / (size - 1), row * 224 / (size - 1)
            height = 64 if 56 <= x <= 168 and 56 <= y <= 168 else 16
            if (x, y) == (28, 196):
                height = 48
            if (x, y) == (196, 28):
                height = 32
            values.append(str(height - 16))
        rows.append(f'"row{row}" "{" ".join(values)}"')
    normals = "\n".join(
        f'"row{row}" "{" ".join(["0 0 1"] * size)}"' for row in range(size)
    )
    distances = "\n".join(rows)
    return f'dispinfo {{ "power" "{power}" "startposition" "[{origin[0]} {origin[1]} 16]" "flags" "{flags}" "elevation" "0" "subdiv" "0" normals {{ {normals} }} distances {{ {distances} }} }}'


def check_lighting(data, info, check_floor=True):
    face = records(data, 7, FACE_FORMAT)[info[9]]
    texinfo = records(data, 6, "<16f2i")[face[5]]
    extent = 57 if info[5] == 4 else 15
    assert face[16:18] == (extent, extent) and face[8:12] == (0, 255, 255, 255)
    assert texinfo[16] & 0x400 == 0 and face[12] >= 4
    byte_count = (extent + 1) ** 2 * 4
    colors = lightmaps(data)[face[12] : face[12] + byte_count]
    assert len(colors) == byte_count and any(
        colors[index] for index in range(0, byte_count, 4)
    )
    offset, length, _, _ = struct.unpack_from("<4i", data, 8 + 34 * 16)
    cursor = offset + info[12]
    missing = 0
    for _ in range((face[16] + 1) * (face[17] + 1)):
        index = data[cursor]
        cursor += 1
        if index == 255:
            index += data[cursor]
            cursor += 1
        weights = data[cursor : cursor + 3]
        cursor += 3
        assert cursor <= offset + length
        if index == 0 and weights == bytes(3):
            missing += 1
            assert info[5] == 4
        else:
            assert index < 2 * (1 << info[5]) ** 2 and sum(weights) in [253, 254, 255]
    if info[5] == 4:
        assert missing > 0
    if not check_floor:
        return
    floor = floor_samples(data)
    for target, shadowed in [([80, 80, 0], True), ([-64, -64, 0], False)]:
        color = next(
            color
            for point, color, _, _ in floor
            if all(abs(a - b) < 0.001 for a, b in zip(point, target, strict=True))
        )
        assert (color == [0, 0, 0]) == shadowed, (target, color)


def fixtures(client, output):
    for low, high in ROOM:
        client.call(
            "create_block",
            {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"},
        )
    terrain = client.call(
        "create_block",
        {
            "mins": [0, 0, 0],
            "maxs": [224, 224, 16],
            "material": "dev/dev_measuregeneric01",
        },
    )["id"]
    top = next(
        face
        for face in detail(client, [terrain])[0]["faces"]
        if face["normal"][2] > 0.99
    )
    spawn = client.call(
        "create_entity", {"classname": "info_player_terrorist", "origin": [-64, 0, 8]}
    )["id"]
    client.call(
        "create_entity",
        {
            "classname": "light",
            "origin": [0, 0, 128],
            "properties": {"_light": "255 170 100 300", "_quadratic_attn": "1"},
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
            "origin": [160, -128, 128],
            "properties": {
                "targetname": "physics_crate",
                "model": "models/props_junk/wood_crate001a.mdl",
            },
        },
    )
    base = output / "base.vmf"
    client.call(
        "create_entity",
        {
            "classname": "prop_physics",
            "origin": [140, 140, 144],
            "properties": {
                "targetname": "terrain_crate",
                "model": "models/props_junk/wood_crate001a.mdl",
            },
        },
    )
    client.call("save_map", {"path": str(base)})
    text = base.read_text()
    pattern = rf'(\bside\s*\{{\s*"id"\s*"{top["side"]}"[^{{}}]*)(\}})'
    result = []
    for name, flags, power in [
        ("terrain", 0, 3),
        ("no-hull", 4, 3),
        ("no-ray", 8, 3),
        ("no-physics", 2, 3),
        ("power-four", 0, 4),
    ]:
        vmf = output / f"{name}.vmf"
        contents, count = re.subn(
            pattern,
            lambda match, flags=flags, power=power: (
                (
                    re.sub(r'("lightmapscale"\s+)"16"', r'\g<1>"4"', match[1])
                    if power == 4
                    else match[1]
                )
                + displacement(flags, power)
                + "\n"
                + match[2]
            ),
            text,
        )
        assert count == 1
        vmf.write_text(contents)
        client.call("open_map", {"path": str(vmf)})
        client.call(
            "set_compile_settings", {"lighting": "direct_ldr", "physics": "engine"}
        )
        before = detail(client, [terrain])
        history = client.call("get_map_info")["undo"]
        source = vmf.read_bytes()
        bsp = output / f"{name}.bsp"
        compiled = client.call("compile_map", {"path": str(bsp)})
        assert compiled["success"], compiled
        stats = compiled["stats"]
        assert (
            stats["displacements"],
            stats["displacement_vertices"],
            stats["displacement_triangles"],
        ) == (1, ((1 << power) + 1) ** 2, 2 * (1 << power) ** 2)
        assert stats["brushes"] == 6 and stats["world_lights"] == 1
        assert stats["lighting_faces"] == 7
        assert stats["physics_mesh_triangles"] == (
            0 if flags & 2 else 2 * (1 << power) ** 2
        )
        assert stats["physics_solids"] == (1 if flags & 2 else 2)
        data = bsp.read_bytes()
        infos = records(data, 26, "<3f4ifi2H2i48s40s10I")
        assert (
            len(infos) == 1
            and infos[0][5] == power
            and infos[0][6] == -2147483648 | flags
        )
        assert (
            len(records(data, 33, "<5f")) == ((1 << power) + 1) ** 2
            and len(records(data, 48, "<H")) == 2 * (1 << power) ** 2
        )
        check_lighting(data, infos[0])
        background = output / f"{name}-background.bsp"
        client.call("start_compile", {"path": str(background)})
        job = wait_job(client)
        assert job["status"] == "completed" and background.read_bytes() == data, job
        assert (
            before == detail(client, [terrain])
            and history == client.call("get_map_info")["undo"]
            and source == vmf.read_bytes()
        )
        result.append({"name": name, "flags": flags, "bsp": str(bsp), "stats": stats})
    invalid = output / "unsupported.vmf"
    contents, count = re.subn(
        pattern, lambda match: match[1] + displacement(16) + "\n" + match[2], text
    )
    assert count == 1
    invalid.write_text(contents)
    client.call("open_map", {"path": str(invalid)})
    destination = output / "terrain.bsp"
    before = destination.read_bytes()
    failed = client.call("compile_map", {"path": str(destination)}, error=True)
    diagnostics = json.loads(failed["content"][0]["text"])["diagnostics"]
    assert destination.read_bytes() == before and any(
        "unknown displacement flags" in row["message"] for row in diagnostics
    )
    client.call("open_map", {"path": str(output / "power-four.vmf")})
    additions = []
    for power, low, high in [
        (2, [-224, -224, 0], [-16, -128, 16]),
        (3, [-224, -112, 0], [-16, -16, 16]),
    ]:
        patch = client.call(
            "create_block",
            {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"},
        )["id"]
        face = next(
            face
            for face in detail(client, [patch])[0]["faces"]
            if face["normal"][2] > 0.99
        )
        additions.append((power, face["side"], low[:2]))
    client.call("set_entity_properties", {"id": spawn, "set": {"origin": "160 -192 8"}})
    mixed = output / "mixed.vmf"
    client.call("save_map", {"path": str(mixed)})
    contents = mixed.read_text()
    for power, side, origin in additions:
        pattern = rf'(\bside\s*\{{\s*"id"\s*"{side}"[^{{}}]*)(\}})'
        contents, count = re.subn(
            pattern,
            lambda match, power=power, origin=origin: (
                match[1] + displacement(0, power, origin) + "\n" + match[2]
            ),
            contents,
        )
        assert count == 1
    mixed.write_text(contents)
    client.call("open_map", {"path": str(mixed)})
    source = mixed.read_bytes()
    history = client.call("get_map_info")["undo"]
    destination = output / "mixed.bsp"
    compiled = client.call("compile_map", {"path": str(destination)})
    assert compiled["success"], compiled
    stats = compiled["stats"]
    assert stats["displacements"] == 3 and stats["displacement_triangles"] == 672
    assert stats["physics_mesh_triangles"] == 672 and stats["lighting_faces"] == 9
    data = destination.read_bytes()
    infos = records(data, 26, "<3f4ifi2H2i48s40s10I")
    assert sorted(info[5] for info in infos) == [2, 3, 4]
    check_lighting(
        data, next(info for info in infos if info[5] == 4), check_floor=False
    )
    starts = [info[12] for info in infos]
    assert starts[0] == 0 and all(a < b for a, b in itertools.pairwise(starts))
    background = output / "mixed-background.bsp"
    client.call("start_compile", {"path": str(background)})
    job = wait_job(client)
    assert job["status"] == "completed" and background.read_bytes() == data, job
    assert (
        source == mixed.read_bytes() and history == client.call("get_map_info")["undo"]
    )
    result.append(
        {"name": "mixed", "flags": 0, "bsp": str(destination), "stats": stats}
    )
    (output / "results.json").write_text(json.dumps(result, indent=2) + "\n")
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument("--output", type=Path, default=Path("/tmp/hammerr-terrain"))
    parser.add_argument("--runtime", action="store_true")
    parser.add_argument(
        "--runtime-cases", nargs="+", help="Limit private runtime checks to named cases"
    )
    parser.add_argument(
        "--game-dir",
        type=Path,
        default=Path.home()
        / ".local/share/Steam/steamapps/common/Counter-Strike Source",
    )
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--bsp", type=Path, help=argparse.SUPPRESS)
    parser.add_argument("--flags", type=int, default=0, help=argparse.SUPPRESS)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    if args.worker:
        runtime.OBSERVER = OBSERVER
        if args.bsp.stem == "mixed":
            runtime.OBSERVER = runtime.OBSERVER.replace(
                "    local center = TraceLine",
                "    local first_patch = TraceLine(Vector(-120, -176, 160), Vector(-120, -176, -8), null);\n"
                "    local second_patch = TraceLine(Vector(-120, -64, 160), Vector(-120, -64, -8), null);\n"
                '    printl("HAMMERR_MIXED_TRACE " + first_patch + " " + second_patch);\n'
                "    local center = TraceLine",
            )
        args.ground_z = 0 if args.flags & 4 else 64
        runtime.worker(args)
        text = (args.output / "cstrike/hammerr-runtime.log").read_text(errors="replace")
        if args.bsp.stem == "mixed":
            mixed_traces = [
                list(map(float, values.split()))
                for values in re.findall(r"HAMMERR_MIXED_TRACE ([\d.eE+ -]+)", text)
            ]
            assert len(mixed_traces) >= 3 and all(
                all(abs(160 - value * 168 - 64) < 0.1 for value in row)
                for row in mixed_traces
            ), mixed_traces
            (args.output / "mixed-heights.json").write_text(
                json.dumps(mixed_traces, indent=2) + "\n"
            )
        traces = [
            list(map(float, values.split()))
            for values in re.findall(r"HAMMERR_TERRAIN_TRACE ([\d.eE+ -]+)", text)
        ]
        expected = [0, 0, 0] if args.flags & 8 else [64, 48, 32]
        assert len(traces) >= 3, text[-4000:]
        heights = [[160 - fraction * 168 for fraction in row] for row in traces]
        assert all(
            all(
                abs(actual - height) < 0.1
                for actual, height in zip(row, expected, strict=True)
            )
            for row in heights
        ), heights
        (args.output / "terrain-heights.json").write_text(
            json.dumps(heights, indent=2) + "\n"
        )
        positions = [
            list(map(float, values.split()))
            for values in re.findall(r"HAMMERR_TERRAIN_PROP ([\d.eE+ -]+)", text)
        ]
        assert len(positions) >= 3, text[-4000:]
        expected_z = 20 if args.flags & 2 else 84
        assert all(abs(row[2] - expected_z) < 2 for row in positions[-3:]), positions
        assert all(
            56 <= row[0] <= 168 and 56 <= row[1] <= 168 for row in positions[-3:]
        ), positions
        (args.output / "terrain-prop.json").write_text(
            json.dumps(positions, indent=2) + "\n"
        )
        return
    client = Client(None, args.binary, args.output)
    try:
        cases = fixtures(client, args.output)
    finally:
        client.close()
    if args.runtime:
        env = {
            **os.environ,
            "WLR_BACKENDS": "headless",
            "WLR_HEADLESS_OUTPUTS": "1",
            "WLR_RENDERER": "pixman",
        }
        for case in cases:
            if args.runtime_cases and case["name"] not in args.runtime_cases:
                continue
            output = args.output / case["name"]
            output.mkdir(exist_ok=True)
            command = [
                "cage",
                "--",
                sys.executable,
                "-B",
                str(Path(__file__).resolve()),
                "--worker",
                "--game-dir",
                str(args.game_dir),
                "--output",
                str(output),
                "--bsp",
                case["bsp"],
                "--flags",
                str(case["flags"]),
            ]
            with (output / "compositor.log").open("w") as log:
                subprocess.run(
                    command,
                    cwd=ROOT,
                    env=env,
                    stdout=log,
                    stderr=log,
                    check=True,
                    timeout=50,
                )
    print(json.dumps({"output": str(args.output), "cases": len(cases)}))


if __name__ == "__main__":
    main()
