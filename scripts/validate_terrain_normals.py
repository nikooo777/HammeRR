import argparse
import json
import re
import struct
import sys
from pathlib import Path

import validate_native_runtime as runtime
from validate_authored_planes import records
from validate_compile_jobs import wait_job
from validate_direct_lighting import FACE_FORMAT, lightmaps
from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_visibility import ROOM


def fixture(args):
    client = Client(None, args.binary, args.output)
    try:
        for low, high in ROOM:
            client.call(
                "create_block",
                {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"},
            )
        terrain = client.call(
            "create_block",
            {
                "mins": [0, 0, 0],
                "maxs": [128, 128, 16],
                "material": "dev/dev_measuregeneric01",
            },
        )["id"]
        top = next(
            face
            for face in detail(client, [terrain])[0]["faces"]
            if face["normal"][2] > 0.99
        )
        client.call(
            "create_entity",
            {"classname": "info_player_terrorist", "origin": [-64, 0, 8]},
        )
        client.call(
            "create_entity",
            {
                "classname": "light",
                "origin": [64, 64, 180],
                "properties": {
                    "_light": "0 255 0 200",
                    "_constant_attn": "1",
                    "_quadratic_attn": "0",
                },
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
        vmf = args.output / "ridge.vmf"
        client.call("save_map", {"path": str(vmf)})
        normals = "\n".join(
            f'"row{row}" "{" ".join(["0 0 1"] * 5)}"' for row in range(5)
        )
        distances = "\n".join(f'"row{row}" "0 0 16 0 0"' for row in range(5))
        displacement = f'dispinfo {{ "power" "2" "startposition" "[0 0 16]" "flags" "0" "elevation" "0" "subdiv" "0" normals {{ {normals} }} distances {{ {distances} }} }}'
        pattern = rf'(\bside\s*\{{\s*"id"\s*"{top["side"]}"[^{{}}]*)(\}})'
        contents, count = re.subn(
            pattern,
            lambda match: (
                re.sub(r'("lightmapscale"\s+)"16"', r'\g<1>"18"', match[1])
                + displacement
                + "\n"
                + match[2]
            ),
            vmf.read_text(),
        )
        assert count == 1
        vmf.write_text(contents)
        client.call("open_map", {"path": str(vmf)})
        source = vmf.read_bytes()
        before = detail(client, [terrain])
        history = client.call("get_map_info")["undo"]
        client.call(
            "set_compile_settings", {"lighting": "fullbright", "physics": "engine"}
        )
        fullbright = args.output / "fullbright.bsp"
        assert client.call("compile_map", {"path": str(fullbright)})["success"]
        client.call("set_compile_settings", {"lighting": "direct_ldr"})
        bsp = args.output / "ridge.bsp"
        result = client.call("compile_map", {"path": str(bsp)})
        assert result["success"], result
        data = bsp.read_bytes()
        baseline = fullbright.read_bytes()
        for lump in [33, 48, 29]:
            offset, length, _, _ = struct.unpack_from("<4i", data, 8 + lump * 16)
            old_offset, old_length, _, _ = struct.unpack_from(
                "<4i", baseline, 8 + lump * 16
            )
            assert (
                data[offset : offset + length]
                == baseline[old_offset : old_offset + old_length]
            )
        disp = records(data, 26, "<3f4ifi2H2i48s40s10I")[0]
        face = records(data, 7, FACE_FORMAT)[disp[9]]
        assert face[16:18] == (8, 8)
        colors = lightmaps(data)
        color = struct.unpack_from("<3Bb", colors, face[12] + (4 * 9 + 4) * 4)
        assert color == (0, 200, 0, 0), color
        left = struct.unpack_from("<3Bb", colors, face[12] + (4 * 9 + 3) * 4)
        right = struct.unpack_from("<3Bb", colors, face[12] + (4 * 9 + 5) * 4)
        assert left == right and 0 < left[1] < color[1], (left, right)
        client.call("start_compile", {"path": str(args.output / "background.bsp")})
        job = wait_job(client)
        assert (
            job["status"] == "completed"
            and (args.output / "background.bsp").read_bytes() == data
        )
        assert (
            source == vmf.read_bytes()
            and before == detail(client, [terrain])
            and history == client.call("get_map_info")["undo"]
        )
        client.render(
            args.output / "viewport.png",
            width=800,
            height=600,
            surface="flat",
            camera={"position": [190, -190, 180], "look_at": [64, 64, 24]},
        )
        (args.output / "results.json").write_text(
            json.dumps(
                {
                    "stats": result["stats"],
                    "center": color,
                    "adjacent": left,
                    "geometry_and_physics_unchanged": True,
                },
                indent=2,
            )
            + "\n"
        )
        return bsp
    finally:
        client.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-terrain-normals")
    )
    parser.add_argument(
        "--game-dir",
        type=Path,
        default=Path.home()
        / ".local/share/Steam/steamapps/common/Counter-Strike Source",
    )
    parser.add_argument("--runtime", action="store_true")
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--bsp", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    if args.worker:
        runtime.OBSERVER = runtime.OBSERVER.replace(
            '    local player = Entities.FindByClassname(null, "player");',
            '    local peak = TraceLine(Vector(64, 64, 160), Vector(64, 64, -8), null);\n    printl("HAMMERR_RIDGE_TRACE " + peak);\n    local player = Entities.FindByClassname(null, "player");',
        )
        runtime.worker(args)
        text = (args.output / "cstrike/hammerr-runtime.log").read_text(errors="replace")
        heights = [
            160 - float(value) * 168
            for value in re.findall(r"HAMMERR_RIDGE_TRACE ([\d.eE+-]+)", text)
        ]
        assert len(heights) >= 3 and all(
            abs(height - 32) < 0.1 for height in heights
        ), heights
        return
    bsp = fixture(args)
    if args.runtime:
        output = args.output / "runtime"
        runtime.private_compositor(
            [
                sys.executable,
                "-B",
                str(Path(__file__).resolve()),
                "--worker",
                "--game-dir",
                str(args.game_dir),
                "--output",
                str(output),
                "--bsp",
                str(bsp),
            ],
            output,
            timeout=50,
        )
    print(json.dumps({"output": str(args.output), "runtime": args.runtime}))


if __name__ == "__main__":
    main()
