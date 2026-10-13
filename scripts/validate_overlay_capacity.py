import argparse
import json
import os
import struct
import subprocess
import sys
from pathlib import Path

import validate_areaportals as areas
import validate_native_runtime as runtime
from validate_authored_planes import records
from validate_face_lighting import ROOT


def prepare(args):
    template = areas.fixture(args)
    original = template.read_bytes()
    data = bytearray(original)

    def raw(slot):
        offset, size, _, _ = struct.unpack_from("<4i", data, 8 + 16 * slot)
        return bytes(data[offset : offset + size])

    def replace(slot, payload):
        data.extend(bytes((-len(data)) % 4))
        struct.pack_into("<4i", data, 8 + 16 * slot, len(data), len(payload), 0, 0)
        data.extend(payload)

    texdata, infos, strings, names = [raw(slot) for slot in [2, 6, 43, 44]]
    material = b"DECALS/DECALGRAFFITI001A\0"
    info_index = len(infos) // 72
    texture_index = len(texdata) // 32
    replace(43, strings + material)
    replace(44, names + struct.pack("<i", len(strings)))
    replace(
        2,
        texdata
        + struct.pack("<3f5i", 0.5, 0.5, 0.5, len(names) // 4, 512, 512, 512, 512),
    )
    replace(
        6, infos + struct.pack("<16f2i", *([0, 0, 0, -99999] * 4), 0, texture_index)
    )
    planes = records(original, 1, "<4fi")
    faces = records(original, 7, "<HbBihhhh4Bif2i2iiHHI")
    floor = [
        index for index, face in enumerate(faces) if planes[face[0]][:4] == (0, 0, 1, 0)
    ]
    assert floor and len(floor) <= 64, floor
    overlays = []
    for index in range(args.count):
        targets = floor if index == args.count - 1 else []
        values = [index, info_index, len(targets)] + targets + [0] * (64 - len(targets))
        values += [
            0,
            1,
            0,
            1,
            -192,
            -192,
            1,
            -192,
            192,
            0,
            192,
            192,
            0,
            192,
            -192,
            0,
            0,
            0,
            0,
            0,
            0,
            1,
        ]
        overlays.append(struct.pack("<ihH64i22f", *values))
    replace(45, b"".join(overlays))
    replace(60, b"".join(struct.pack("<2f", -1, 0) for _ in overlays))
    decoded = records(data, 45, "<ihH64i22f")
    assert len(decoded) == args.count and decoded[-1][0] == args.count - 1
    assert all(row[2] == 0 for row in decoded[:-1]) and decoded[-1][2] == len(floor)
    path = args.output / "capacity.bsp"
    path.write_bytes(data)
    (args.output / "capacity.json").write_text(
        json.dumps(
            {
                "overlays": args.count,
                "only_live_record": args.count - 1,
                "floor_faces": floor,
            },
            indent=2,
        )
        + "\n"
    )
    return path


def worker(args):
    args.capture_ppm = True
    runtime.OBSERVER = runtime.OBSERVER.replace(
        "setang 25 45 0; mat_fullbright",
        "setang 25 45 0; r_drawothermodels 0; mat_fullbright",
    )
    runtime.worker(args)
    magic, dimensions, maximum, pixels = (
        (args.output / "game.ppm").read_bytes().split(b"\n", 3)
    )
    width, height = map(int, dimensions.split())
    assert magic == b"P6" and maximum == b"255" and len(pixels) == width * height * 3
    lower = pixels[width * (height // 2) * 3 :]
    purple = sum(
        blue > green * 1.5 + 10 for red, green, blue in struct.iter_unpack("3B", lower)
    )
    (args.output / "render.json").write_text(
        json.dumps(
            {"purple_floor_pixels": purple},
            indent=2,
        )
        + "\n"
    )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-overlay-capacity")
    )
    parser.add_argument("--count", type=int, default=1024)
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
    if not 1 <= args.count <= 1024:
        parser.error("--count must be between 1 and 1024")
    args.output.mkdir(parents=True, exist_ok=True)
    if args.worker:
        worker(args)
        return
    bsp = prepare(args)
    if args.runtime:
        env = {
            **os.environ,
            "WLR_BACKENDS": "headless",
            "WLR_HEADLESS_OUTPUTS": "1",
            "WLR_RENDERER": "pixman",
        }
        reports = []
        for name, path in [("baseline", args.output / "areas.bsp"), ("runtime", bsp)]:
            output = args.output / name
            output.mkdir(exist_ok=True)
            with (output / "compositor.log").open("w") as log:
                subprocess.run(
                    [
                        "cage",
                        "--",
                        sys.executable,
                        "-B",
                        str(Path(__file__).resolve()),
                        "--worker",
                        "--count",
                        str(args.count),
                        "--bsp",
                        str(path),
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
                    timeout=55,
                )
            reports.append(json.loads((output / "render.json").read_text()))
        assert (
            reports[1]["purple_floor_pixels"] > reports[0]["purple_floor_pixels"] + 1000
        ), reports
        (args.output / "render-results.json").write_text(
            json.dumps(
                {
                    "baseline": reports[0],
                    "capacity": reports[1],
                    "last_live_record": args.count - 1,
                },
                indent=2,
            )
            + "\n"
        )
    print(
        json.dumps({"output": str(args.output), "native_overlay_capacity": args.count})
    )


if __name__ == "__main__":
    main()
