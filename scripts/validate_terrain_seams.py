import argparse
import json
import math
import os
import re
import struct
import subprocess
import sys
from pathlib import Path

import validate_native_runtime as runtime
from validate_authored_planes import records
from validate_compile_jobs import wait_job
from validate_direct_lighting import FACE_FORMAT, lightmaps
from validate_face_lighting import ROOT, Client, detail
from validate_visibility import ROOM


def displacement(right=False):
    size = 9
    start = [128, 128, 16] if right else [-128, 0, 16]
    normals = "\n".join(
        f'"row{row}" "{" ".join(["0 0 1"] * size)}"' for row in range(size)
    )
    heights = [
        (128 - col * 16 if right else -128 + col * 16) ** 2 / 256 for col in range(size)
    ]
    distances = "\n".join(
        f'"row{row}" "{" ".join(map(str, heights))}"' for row in range(size)
    )
    return f'dispinfo {{ "power" "3" "startposition" "[{" ".join(map(str, start))}]" "elevation" "0" "subdiv" "0" normals {{ {normals} }} distances {{ {distances} }} }}'


def seam_colors(data):
    faces = records(data, 7, FACE_FORMAT)
    infos = records(data, 26, "<3f4ifi2H2i48s40s10I")
    lighting = lightmaps(data)
    result = []
    for info in infos:
        face = faces[info[9]]
        width, height = [dimension + 1 for dimension in face[16:18]]
        edge = []
        for row in range(height):
            red, green, blue, exponent = struct.unpack_from(
                "<3Bb", lighting, face[12] + (row * width + width - 1) * 4
            )
            edge.append([value * 2**exponent for value in [red, green, blue]])
        result.append(edge)
    return result


def fixture(args):
    client = Client(None, args.binary, args.output)
    try:
        walls = [
            client.call(
                "create_block",
                {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"},
            )["id"]
            for low, high in ROOM
        ]
        ceiling = detail(client, [walls[1]])[0]
        bottom = next(face for face in ceiling["faces"] if face["normal"][2] < -0.99)
        client.call(
            "select_faces",
            {"faces": [{"solid": ceiling["solid"], "side": bottom["side"]}]},
        )
        client.call("edit_faces", {"material": "tools/toolsskybox"})
        for classname, origin, properties in [
            ("info_player_terrorist", [-64, -64, 8], {}),
            (
                "light_environment",
                [-64, -64, 64],
                {
                    "angles": "0 0 0",
                    "pitch": "-45",
                    "_light": "0 255 0 200",
                    "_ambient": "255 255 255 0",
                },
            ),
            (
                "logic_script",
                [0, 0, 32],
                {
                    "targetname": "hammerr_observer",
                    "vscripts": "hammerr_observer.nut",
                    "thinkfunction": "HammeRRThink",
                },
            ),
            (
                "prop_physics",
                [160, -128, 128],
                {
                    "targetname": "physics_crate",
                    "model": "models/props_junk/wood_crate001a.mdl",
                },
            ),
        ]:
            client.call(
                "create_entity",
                {"classname": classname, "origin": origin, "properties": properties},
            )
        patches = []
        for low, high in [([-128, 0, 0], [0, 128, 16]), ([0, 0, 0], [128, 128, 16])]:
            patch = client.call(
                "create_block",
                {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"},
            )["id"]
            top = next(
                face
                for face in detail(client, [patch])[0]["faces"]
                if face["normal"][2] > 0.99
            )
            patches.append((patch, top["side"]))
        vmf = args.output / "sewn.vmf"
        client.call("save_map", {"path": str(vmf)})
        text = vmf.read_text()
        for index, (_, side) in enumerate(patches):
            pattern = rf'(\bside\s*\{{\s*"id"\s*"{side}"[^{{}}]*)(\}})'
            text, count = re.subn(
                pattern,
                lambda match, index=index: (
                    match[1] + displacement(index == 1) + "\n" + match[2]
                ),
                text,
            )
            assert count == 1
        vmf.write_text(text)
        client.call("open_map", {"path": str(vmf)})
        source = vmf.read_bytes()
        history = client.call("get_map_info")["undo"]
        client.call(
            "set_compile_settings", {"lighting": "fullbright", "physics": "engine"}
        )
        baseline = args.output / "fullbright.bsp"
        assert client.call("compile_map", {"path": str(baseline)})["success"]
        client.call("set_compile_settings", {"lighting": "direct_ldr"})
        path = args.output / "sewn.bsp"
        built = client.call("compile_map", {"path": str(path)})
        assert built["success"], built
        assert built["stats"]["displacement_neighbor_pairs"] == 1
        data = path.read_bytes()
        old = baseline.read_bytes()
        for lump in [33, 48, 29]:
            offset, length, _, _ = struct.unpack_from("<4i", data, 8 + lump * 16)
            old_offset, old_length, _, _ = struct.unpack_from("<4i", old, 8 + lump * 16)
            assert (
                data[offset : offset + length]
                == old[old_offset : old_offset + old_length]
            )
        colors = seam_colors(data)
        assert colors[0] == list(reversed(colors[1])), colors
        assert all(
            color[0] == color[2] == 0 and abs(color[1] - 200 / math.sqrt(2)) < 1
            for edge in colors
            for color in edge
        ), colors
        client.call("start_compile", {"path": str(args.output / "background.bsp")})
        job = wait_job(client)
        assert (
            job["status"] == "completed"
            and (args.output / "background.bsp").read_bytes() == data
        )
        assert (
            source == vmf.read_bytes()
            and history == client.call("get_map_info")["undo"]
        )
        client.render(
            args.output / "viewport.png",
            width=1000,
            height=700,
            surface="flat",
            camera={"position": [185, -210, 175], "look_at": [0, 64, 32]},
        )
        client.call("set_object_visibility", {"action": "hide", "ids": [patches[1][0]]})
        hidden = args.output / "hidden.bsp"
        one = client.call("compile_map", {"path": str(hidden)})
        assert one["success"] and one["stats"]["displacement_neighbor_pairs"] == 0, one
        unsewn = seam_colors(hidden.read_bytes())
        assert len(unsewn) == 1 and any(
            abs(color[1] - colors[0][0][1]) > 3 for color in unsewn[0]
        ), unsewn
        (args.output / "results.json").write_text(
            json.dumps(
                {
                    "stats": built["stats"],
                    "seam": colors,
                    "unlinked_seam": unsewn,
                    "geometry_and_physics_unchanged": True,
                },
                indent=2,
            )
            + "\n"
        )
        return path
    finally:
        client.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-terrain-seams")
    )
    parser.add_argument("--runtime", action="store_true")
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--bsp", type=Path, help=argparse.SUPPRESS)
    parser.add_argument(
        "--game-dir",
        type=Path,
        default=Path.home()
        / ".local/share/Steam/steamapps/common/Counter-Strike Source",
    )
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    if args.worker:
        runtime.OBSERVER = runtime.OBSERVER.replace(
            '    local player = Entities.FindByClassname(null, "player");',
            '    local left = TraceLine(Vector(-1,64,160), Vector(-1,64,-8), null);\n    local center = TraceLine(Vector(0,64,160), Vector(0,64,-8), null);\n    local right = TraceLine(Vector(1,64,160), Vector(1,64,-8), null);\n    printl("HAMMERR_SEWN_SEAM " + left + " " + center + " " + right);\n    local player = Entities.FindByClassname(null, "player");',
        )
        runtime.worker(args)
        report = json.loads((args.output / "runtime.json").read_text())
        assert all(
            abs(position[2] - 20) < 2 for position in report["prop_positions"][-3:]
        )
        text = (args.output / "cstrike/hammerr-runtime.log").read_text(errors="replace")
        heights = [
            [160 - float(value) * 168 for value in values.split()]
            for values in re.findall(r"HAMMERR_SEWN_SEAM ([\d.eE+ -]+)", text)
        ]
        assert len(heights) >= 3 and all(
            abs(row[1] - 16) < 0.1 and abs(row[0] - row[2]) < 0.1 for row in heights
        ), heights
        (args.output / "seam-heights.json").write_text(json.dumps(heights) + "\n")
        return
    bsp = fixture(args)
    if args.runtime:
        output = args.output / "runtime"
        output.mkdir(exist_ok=True)
        env = {
            **os.environ,
            "WLR_BACKENDS": "headless",
            "WLR_HEADLESS_OUTPUTS": "1",
            "WLR_RENDERER": "pixman",
        }
        with (output / "compositor.log").open("w") as log:
            subprocess.run(
                [
                    "cage",
                    "--",
                    sys.executable,
                    "-B",
                    str(Path(__file__).resolve()),
                    "--worker",
                    "--bsp",
                    str(bsp),
                    "--output",
                    str(output),
                    "--game-dir",
                    str(args.game_dir),
                ],
                cwd=ROOT,
                env=env,
                stdout=log,
                stderr=log,
                check=True,
                timeout=50,
            )
    print(json.dumps({"output": str(args.output), "sewn": True}))


if __name__ == "__main__":
    main()
