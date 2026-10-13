import argparse
import json
import os
import re
import subprocess
import sys
from pathlib import Path

import validate_native_runtime as runtime
from validate_authored_planes import records
from validate_compile_jobs import wait_job
from validate_direct_lighting import FACE_FORMAT, lightmaps
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


def displacement(flags, power=3):
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
    return f'dispinfo {{ "power" "{power}" "startposition" "[0 0 16]" "flags" "{flags}" "elevation" "0" "subdiv" "0" normals {{ {normals} }} distances {{ {distances} }} }}'


def check_lighting(data, info):
    face = records(data, 7, FACE_FORMAT)[info[9]]
    texinfo = records(data, 6, "<16f2i")[face[5]]
    assert face[16:18] == (15, 15) and face[8:12] == (0, 255, 255, 255)
    assert texinfo[16] & 0x400 == 0 and face[12] >= 4
    colors = lightmaps(data)[face[12] : face[12] + 256 * 4]
    assert len(colors) == 1024 and any(colors[index] for index in range(0, 1024, 4))
    samples = records(data, 34, "<4B")
    assert len(samples) == 256 and info[12] == 0
    assert all(
        index < 128 and sum(weights) in [253, 254, 255] for index, *weights in samples
    )


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
    client.call(
        "create_entity", {"classname": "info_player_terrorist", "origin": [-64, 0, 8]}
    )
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
    for name, flags in [
        ("terrain", 0),
        ("no-hull", 4),
        ("no-ray", 8),
        ("no-physics", 2),
    ]:
        vmf = output / f"{name}.vmf"
        contents, count = re.subn(
            pattern,
            lambda match, flags=flags: match[1] + displacement(flags) + "\n" + match[2],
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
        ) == (1, 81, 128)
        assert stats["brushes"] == 6 and stats["world_lights"] == 1
        assert stats["lighting_faces"] == 7
        assert stats["physics_mesh_triangles"] == (0 if flags & 2 else 128)
        assert stats["physics_solids"] == (1 if flags & 2 else 2)
        data = bsp.read_bytes()
        infos = records(data, 26, "<3f4ifi2H2i48s40s10I")
        assert (
            len(infos) == 1 and infos[0][5] == 3 and infos[0][6] == -2147483648 | flags
        )
        assert (
            len(records(data, 33, "<5f")) == 81 and len(records(data, 48, "<H")) == 128
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
    power_four = output / "power-four.vmf"
    contents, count = re.subn(
        pattern,
        lambda match: match[1] + displacement(0, power=4) + "\n" + match[2],
        text,
    )
    assert count == 1
    power_four.write_text(contents)
    client.call("open_map", {"path": str(power_four)})
    failed = client.call("compile_map", {"path": str(destination)}, error=True)
    diagnostics = json.loads(failed["content"][0]["text"])["diagnostics"]
    assert destination.read_bytes() == before and any(
        "triangle-511" in row["message"] for row in diagnostics
    )
    (output / "results.json").write_text(json.dumps(result, indent=2) + "\n")
    return result


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument("--output", type=Path, default=Path("/tmp/hammerr-terrain"))
    parser.add_argument("--runtime", action="store_true")
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
        args.ground_z = 0 if args.flags & 4 else 64
        runtime.worker(args)
        text = (args.output / "cstrike/hammerr-runtime.log").read_text(errors="replace")
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
