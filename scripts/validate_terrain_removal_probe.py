import argparse
import hashlib
import json
import struct
import sys
from pathlib import Path

import validate_native_runtime as runtime
from validate_face_lighting import ROOT


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--bsp",
        type=Path,
        required=True,
        help="Flat terrain_lod fixture BSP, with engine physics",
    )
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-terrain-removal-probe")
    )
    parser.add_argument(
        "--game-dir",
        type=Path,
        default=Path.home()
        / ".local/share/Steam/steamapps/common/Counter-Strike Source",
    )
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    original = args.bsp.resolve().read_bytes()
    assert original[:4] == b"VBSP"
    data = bytearray(original)
    start, length, version, _ = struct.unpack_from("<4i", data, 8 + 48 * 16)
    assert (
        version == 0
        and length > 0
        and length % 2 == 0
        and start >= 1036
        and start + length <= len(data)
    )
    for offset in range(start, start + length, 2):
        tag = struct.unpack_from("<H", data, offset)[0]
        assert tag & 32 == 0
        struct.pack_into("<H", data, offset, tag | 32)
    assert (
        original[:start] == data[:start]
        and original[start + length :] == data[start + length :]
    )
    patched = args.output.resolve() / "native-remove.bsp"
    patched.write_bytes(data)
    rows = []
    for name, bsp in [("baseline", args.bsp.resolve()), ("removed", patched)]:
        output = args.output.resolve() / name
        runtime.private_compositor(
            [
                sys.executable,
                "-B",
                str(ROOT / "scripts/validate_terrain_lod.py"),
                "--worker",
                "--view",
                "near",
                "--case",
                "flat",
                "--bsp",
                str(bsp),
                "--output",
                str(output),
                "--game-dir",
                str(args.game_dir),
            ],
            output,
            timeout=55,
        )
        rows.append(
            {
                "render_sha256": hashlib.sha256(
                    (output / "game.png").read_bytes()
                ).hexdigest(),
                "rays": json.loads((output / "seam-heights.json").read_text())[-9:],
                "players": json.loads((output / "runtime.json").read_text())["players"][
                    -3:
                ],
            }
        )
    assert rows[0] == rows[1], (
        "Native removal behavior changed; review this engine before changing compiler support"
    )
    result = {
        "native_removal_ignored": True,
        "all_triangles_tagged": length // 2,
        "render_rays_and_players_match": True,
        "baseline": rows[0],
        "removed": rows[1],
    }
    (args.output / "probe-results.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"native_removal_ignored": True, "output": str(args.output)}))


if __name__ == "__main__":
    main()
