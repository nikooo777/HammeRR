import argparse
import json
import math
import os
import subprocess
import sys
from pathlib import Path

import validate_native_runtime as runtime
from validate_authored_planes import records
from validate_face_lighting import ROOT, Client, detail, face_ref
from validate_terrain_lod import build
from validate_terrain_seams import fixture, seam_colors


def commit(client, draft):
    client.call("commit_face_preview", {"token": draft["state"]["preview_token"]})


def validate(args):
    fixture(args)
    source = args.output / "sewn.vmf"
    text = source.read_text()
    old = "64.0 49.0 36.0 25.0 16.0 9.0 4.0 1.0 0.0"
    assert text.count(old) == 18
    source.write_text(text.replace(old, "64 56 48 40 32 24 16 8 0"))
    output = args.output / "angles"
    output.mkdir(exist_ok=True)
    client = Client(None, args.binary, output)
    try:
        client.call("open_map", {"path": str(source)})
        objects = client.call("get_object_report", {"filter": {"kind": "solid"}})[
            "objects"
        ]
        shapes = detail(client, [row["id"] for row in objects])
        faces = [
            face_ref(shape, face)
            for shape in shapes
            for face in shape["faces"]
            if face.get("displacement") is not None
        ]
        assert len(faces) == 2
        client.call("select_faces", {"faces": faces})
        client.call(
            "set_compile_settings", {"lighting": "direct_ldr", "physics": "engine"}
        )
        original = client.call("get_terrain_grids", {})
        assert all(grid["border_smoothing_angle"] is None for grid in original["faces"])
        automatic, automatic_stats = build(client, output, "automatic")
        assert automatic_stats["displacement_smoothing_overrides"] == 0
        auto_colors = seam_colors(automatic.read_bytes())
        assert auto_colors[0] == list(reversed(auto_colors[1]))
        assert all(
            abs(color[1] - 200 / math.sqrt(2)) < 1
            for colors in auto_colors
            for color in colors
        )
        history = client.call("get_map_info")["undo"]
        draft = client.call("preview_terrain_smoothing", {"angle": 0})
        preview = client.call("get_terrain_grids", {"preview": True})
        assert all(grid["border_smoothing_angle"] == 0 for grid in preview["faces"])
        for a, b in zip(original["faces"], preview["faces"], strict=True):
            assert (
                a["positions"] == b["positions"]
                and a["alphas"] == b["alphas"]
                and a["flags"] == b["flags"]
            )
        assert client.call("get_map_info")["undo"] == history
        client.call("preview_terrain_smoothing", {"angle": 181}, error=True)
        assert client.call("get_terrain_grids", {"preview": True}) == preview
        client.call("cancel_face_preview", {"token": draft["state"]["preview_token"]})
        assert client.call("get_terrain_grids", {}) == original
        commit(client, client.call("preview_terrain_smoothing", {"angle": 0}))
        hard, hard_stats = build(client, output, "hard")
        assert hard_stats["displacement_smoothing_overrides"] == 2
        hard_colors = seam_colors(hard.read_bytes())
        assert all(
            abs(a[1] - b[1]) > 100
            for a, b in zip(hard_colors[0], reversed(hard_colors[1]), strict=True)
        ), hard_colors
        client.render(
            output / "hard.png",
            camera={"position": [-240, -180, 170], "look_at": [0, 64, 32]},
        )
        variants = {"automatic": automatic, "hard": hard}
        colors = {"automatic": auto_colors, "hard": hard_colors}
        for angle in [52, 54]:
            commit(client, client.call("preview_terrain_smoothing", {"angle": angle}))
            path, stats = build(client, output, f"angle-{angle}")
            assert stats["displacement_smoothing_overrides"] == 2
            infos = records(path.read_bytes(), 26, "<3f4ifi2H2i48s40s10I")
            assert all(info[7] == angle for info in infos)
            value = seam_colors(path.read_bytes())
            assert value == (hard_colors if angle == 52 else auto_colors)
            variants[f"angle-{angle}"] = path
            colors[f"angle-{angle}"] = value
        commit(client, client.call("preview_terrain_smoothing", {}))
        restored, _ = build(client, output, "restored")
        assert restored.read_bytes() == automatic.read_bytes()
        client.render(
            output / "automatic.png",
            camera={"position": [-240, -180, 170], "look_at": [0, 64, 32]},
        )
        for path in variants.values():
            for lump in [33, 48, 29]:
                import struct

                data = path.read_bytes()
                old_data = automatic.read_bytes()
                start, length, _, _ = struct.unpack_from("<4i", data, 8 + lump * 16)
                old_start, old_length, _, _ = struct.unpack_from(
                    "<4i", old_data, 8 + lump * 16
                )
                assert (
                    data[start : start + length]
                    == old_data[old_start : old_start + old_length]
                )
        saved = output / "saved.vmf"
        commit(client, client.call("preview_terrain_smoothing", {"angle": 45.5}))
        client.call("save_map", {"path": str(saved)})
        client.call("open_map", {"path": str(saved)})
        client.call("select_faces", {"faces": faces})
        assert all(
            grid["border_smoothing_angle"] == 45.5
            for grid in client.call("get_terrain_grids", {})["faces"]
        )
        malformed = output / "malformed.vmf"
        malformed.write_text(
            saved.read_text().replace('"smooth" "45.5"', '"smooth" "invalid"')
        )
        client.call("open_map", {"path": str(malformed)})
        client.call("select_faces", {"faces": faces})
        assert all(
            grid["border_smoothing_error"]
            for grid in client.call("get_terrain_grids", {})["faces"]
        )
        preserved = output / "invalid-preserved.bsp"
        preserved.write_bytes(b"previous BSP")
        client.call("compile_map", {"path": str(preserved)}, error=True)
        assert preserved.read_bytes() == b"previous BSP"
        commit(client, client.call("preview_terrain_smoothing", {}))
        assert all(
            grid["border_smoothing_error"] is None
            for grid in client.call("get_terrain_grids", {})["faces"]
        )
        repaired, _ = build(client, output, "repaired")
        import struct

        old_data = automatic.read_bytes()
        repaired_data = repaired.read_bytes()
        for lump in range(64):
            start, length, _, _ = struct.unpack_from("<4i", old_data, 8 + lump * 16)
            new_start, new_length, _, _ = struct.unpack_from(
                "<4i", repaired_data, 8 + lump * 16
            )
            original = old_data[start : start + length]
            if lump == 0:
                original = original.replace(b'"mapversion" "1"', b'"mapversion" "2"')
            assert original == repaired_data[new_start : new_start + new_length], lump
        (args.output / "smoothing-results.json").write_text(
            json.dumps(
                {
                    "colors": colors,
                    "angle_boundary": True,
                    "source_geometry_and_physics_preserved": True,
                    "restored_exact_baseline": True,
                    "owned_preview_cancel": True,
                    "invalid_update_preserves_preview": True,
                    "save_reopen": True,
                },
                indent=2,
            )
            + "\n"
        )
        return automatic, hard
    finally:
        client.close()


def worker(args):
    runtime.OBSERVER = runtime.OBSERVER.replace(
        "setpos -64 -64 32; setang 25 45 0",
        "setpos -64 -96 32; setang 16 68 0; r_drawothermodels 0; r_drawviewmodel 0",
    )
    args.ground_z = 0.0
    args.capture_ppm = True
    runtime.worker(args)


def native_difference(first, second):
    header, dimensions, maximum, a = first.read_bytes().split(b"\n", 3)
    h2, d2, m2, b = second.read_bytes().split(b"\n", 3)
    assert (
        (header, dimensions, maximum) == (h2, d2, m2)
        and header == b"P6"
        and maximum == b"255"
    )
    width, height = map(int, dimensions.split())
    total = count = 0
    for y in range(height * 35 // 100, height * 60 // 100):
        for x in range(width * 35 // 100, width * 70 // 100):
            offset = (y * width + x) * 3
            for channel in range(3):
                total += abs(a[offset + channel] - b[offset + channel])
                count += 1
    return total / count


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-terrain-smoothing")
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
        worker(args)
        return
    paths = validate(args)
    if args.runtime:
        env = {
            **os.environ,
            "WLR_BACKENDS": "headless",
            "WLR_HEADLESS_OUTPUTS": "1",
            "WLR_RENDERER": "pixman",
        }
        outputs = []
        for name, bsp in zip(["automatic", "hard"], paths, strict=True):
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
                    timeout=55,
                )
            outputs.append(output)
        delta = native_difference(outputs[0] / "game.ppm", outputs[1] / "game.ppm")
        assert delta > 0.1, delta
        (args.output / "native-smoothing.json").write_text(
            json.dumps({"mean_absolute_difference": delta, "insecure": True}) + "\n"
        )
    print(json.dumps({"terrain_smoothing": True, "output": str(args.output)}))


if __name__ == "__main__":
    main()
