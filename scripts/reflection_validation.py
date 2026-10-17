import re

import validate_cubemap_capture_probe as probe


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def protected_files(args, names):
    paths = [args.game_dir / "bin/linux64" / name for name in probe.MODULES]
    paths.extend(path for path in (args.game_dir / "cstrike/cfg").rglob("*") if path.is_file())
    for name in names:
        paths.append(args.game_dir / "cstrike/maps" / f"{name}.bsp")
        for prefix in [args.game_dir, args.game_dir / "cstrike"]:
            for folder in ["materials/maps", "materialsrc/maps"]:
                paths.extend(path for path in (prefix / folder / name).rglob("*") if path.is_file())
    return {
        str(path): probe.digest(probe.read_bounded(path, probe.MAX_BSP_BYTES)) if path.exists() else None
        for path in sorted(set(paths))
    }


def fake_install(game, output):
    executable = game.parent / "cstrike_linux64"
    program = """#!/usr/bin/env python3
import json
import shutil
import sys
from pathlib import Path
args = sys.argv[1:]
assert '-insecure' in args and args[args.index('+sv_lan') + 1] == '1', args
game = Path(args[args.index('-game') + 1]).resolve(strict=True)
name = args[args.index('+map') + 1]
assert name.startswith('hammerr_') and '/' not in name and '\\\\' not in name, name
source = game / 'maps' / (name + '.bsp')
assert source.stat().st_size <= 64 * 1024**2
output = Path(__file__).resolve().parent.parent
shutil.copyfile(source, output / 'run-staged.bsp')
(output / 'run-launch.json').write_text(json.dumps({'map':name,'game':str(game),'argv':args,'fake_executable':str(Path(__file__).resolve())},indent=2)+'\\n')
"""
    executable.write_text(program)
    executable.chmod(0o700)
    require(executable.resolve().parent == game.parent.resolve(), "Fake Run executable escaped the private fixture")
    return executable


def ppm(path, width=probe.WIDTH, height=probe.HEIGHT):
    data = probe.read_bounded(path, 4 * width * height)
    match = re.match(rb"P6\s+(\d+)\s+(\d+)\s+255\s", data)
    require(match is not None, "Invalid PPM screenshot")
    actual_width, actual_height = map(int, match.groups())
    pixels = data[match.end():]
    require((actual_width, actual_height) == (width, height) and len(pixels) == width * height * 3, "Native viewport differs from the verified dimensions")
    return pixels


def difference(first, second, region, width=probe.WIDTH):
    left, top, right, bottom = region
    a = b"".join(first[(y * width + left) * 3:(y * width + right) * 3] for y in range(top, bottom))
    b = b"".join(second[(y * width + left) * 3:(y * width + right) * 3] for y in range(top, bottom))
    delta = [abs(x - y) for x, y in zip(a, b, strict=True)]
    count = len(a) // 3
    return {
        "region": region,
        "mean_component_difference": sum(delta) / len(delta),
        "changed_fraction": sum(max(delta[index:index + 3]) > 8 for index in range(0, len(delta), 3)) / count,
        "first_rgb": [sum(a[axis::3]) / count for axis in range(3)],
        "second_rgb": [sum(b[axis::3]) / count for axis in range(3)],
    }
