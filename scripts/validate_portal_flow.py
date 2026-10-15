import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

import validate_native_runtime as runtime
import validate_visibility as vis
from validate_compile_jobs import wait_job
from validate_face_lighting import DEFAULT_BINARY, ROOT, Client, detail


def staggered():
    walls = []
    for x, opening in [(-160, (-224, -128)), (0, (128, 224)), (160, (-224, -128))]:
        walls.extend(
            [
                ([x - 8, -256, 0], [x + 8, opening[0], 256]),
                ([x - 8, opening[1], 0], [x + 8, 256, 256]),
            ]
        )
    return walls


def fixture(args):
    client = Client(None, args.binary, args.output)
    metrics = []
    try:
        settings = client.call("get_compile_settings")
        assert (
            settings["visibility"] == "fast"
            and len(settings["visibility_choices"]) == 2
        )
        client.call(
            "set_compile_settings",
            {
                "visibility": "invalid",
                "lighting": "direct_ldr",
                "physics": "engine",
                "sky_quality": "detailed",
            },
            error=True,
        )
        assert client.call("get_compile_settings") == settings
        for name, walls, spawns in [
            ("room", [], [[-64, -64, 8], [64, -64, 8]]),
            (
                "doorway",
                [([-8, -256, 0], [8, -32, 256]), ([-8, 32, 0], [8, 256, 256])],
                [[-64, -64, 8], [64, -64, 8]],
            ),
            (
                "narrow",
                [([-8, -256, 0], [8, -0.25, 256]), ([-8, 0.25, 0], [8, 256, 256])],
                [[-64, -64, 8], [64, -64, 8]],
            ),
            (
                "cycle",
                [([-64, -64, 0], [64, 64, 256])],
                [[-192, -192, 8], [192, 192, 8]],
            ),
            ("staggered", staggered(), [[-224, -176, 8], [224, -176, 8]]),
        ]:
            client.call("new_map", {"discard_changes": True})
            objects = []
            for low, high in vis.ROOM + walls:
                objects.append(
                    client.call(
                        "create_block",
                        {
                            "mins": low,
                            "maxs": high,
                            "material": "dev/dev_measuregeneric01",
                        },
                    )["id"]
                )
            for classname, origin in zip(
                ["info_player_terrorist", "info_player_counterterrorist"], spawns
            ):
                objects.append(
                    client.call(
                        "create_entity", {"classname": classname, "origin": origin}
                    )["id"]
                )
            if name == "staggered":
                for classname, origin, properties in [
                    (
                        "logic_script",
                        [-224, -176, 64],
                        {
                            "targetname": "hammerr_observer",
                            "vscripts": "hammerr_observer.nut",
                            "thinkfunction": "HammeRRThink",
                        },
                    ),
                    (
                        "prop_physics",
                        [-224, 128, 128],
                        {
                            "targetname": "physics_crate",
                            "model": "models/props_junk/wood_crate001a.mdl",
                        },
                    ),
                    (
                        "light",
                        [-208, -176, 180],
                        {
                            "_light": "255 255 255 200",
                            "_constant_attn": "1",
                            "_quadratic_attn": "0",
                        },
                    ),
                    (
                        "light",
                        [208, -176, 180],
                        {
                            "_light": "255 255 255 200",
                            "_constant_attn": "1",
                            "_quadratic_attn": "0",
                        },
                    ),
                ]:
                    objects.append(
                        client.call(
                            "create_entity",
                            {
                                "classname": classname,
                                "origin": origin,
                                "properties": properties,
                            },
                        )["id"]
                    )
            source = args.output / f"{name}.vmf"
            client.call("save_map", {"path": str(source)})
            before = detail(client, objects)
            history = client.call("get_map_info")["undo"]
            source_bytes = source.read_bytes()
            results = {}
            for mode in ["fast", "detailed"]:
                settings = {
                    "visibility": mode,
                    "lighting": "direct_ldr" if name == "staggered" else "fullbright",
                    "physics": "engine" if name == "staggered" else "none",
                }
                client.call("set_compile_settings", settings)
                path = args.output / f"{name}-{mode}.bsp"
                result = client.call("compile_map", {"path": str(path)})
                data = path.read_bytes()
                count, size, rows = vis.visibility(data)
                stats = result["stats"]
                assert (
                    result["success"]
                    and stats["visibility_mode"] == mode
                    and stats["clusters"] == count
                    and stats["visibility_bytes"] == size
                )
                assert stats["visibility_work"] > 0
                assert all(
                    vis.bit(pair[0], index) and vis.bit(pair[1], index)
                    for index, pair in enumerate(rows)
                )
                assert all(
                    vis.bit(rows[a][0], b) == vis.bit(rows[b][0], a)
                    for a in range(count)
                    for b in range(count)
                )
                assert all(
                    not vis.bit(pair[0], index) or vis.bit(pair[1], index)
                    for pair in rows
                    for index in range(count)
                )
                points = [
                    (x, y, z)
                    for x in [-224, -96, 96, 224]
                    for y in [-224, -176, -96, 0, 96, 176, 224]
                    for z in [16, 128, 240]
                ]
                clear = 0
                for start in points:
                    for end in points:
                        if not vis.blocked(start, end, vis.ROOM + walls):
                            clear += 1
                            assert vis.bit(
                                rows[vis.cluster_at(data, start)][0],
                                vis.cluster_at(data, end),
                            ), (name, mode, start, end)
                assert clear > 1000
                background = args.output / f"{name}-{mode}-background.bsp"
                client.call("start_compile", {"path": str(background)})
                job = wait_job(client)
                assert (
                    job["status"] == "completed"
                    and job["settings"]["visibility"] == mode
                    and background.read_bytes() == data
                )
                assert (
                    source.read_bytes() == source_bytes
                    and detail(client, objects) == before
                    and client.call("get_map_info")["undo"] == history
                )
                results[mode] = {
                    "stats": stats,
                    "rows": rows,
                    "data": data,
                    "clear": clear,
                }
            fast, detailed = results["fast"], results["detailed"]
            removed = sum(
                sum(byte.bit_count() for byte in row[0]) for row in fast["rows"]
            ) - sum(
                sum(byte.bit_count() for byte in row[0]) for row in detailed["rows"]
            )
            assert detailed["stats"]["visibility_removed_pairs"] == removed
            assert all(
                not vis.bit(detailed["rows"][a][0], b) or vis.bit(fast["rows"][a][0], b)
                for a in range(len(fast["rows"]))
                for b in range(len(fast["rows"]))
            )
            if name == "staggered":
                assert removed >= 2
                a, b = [
                    vis.cluster_at(detailed["data"], point)
                    for point in [[-224, -176, 64], [224, -176, 64]]
                ]
                assert vis.bit(fast["rows"][a][0], b) and not vis.bit(
                    detailed["rows"][a][0], b
                )
            client.call("set_compile_settings", {"visibility": "fast"})
            assert client.call("get_compile_status")["job"]["stale"]
            client.call("set_compile_settings", {"visibility": "detailed"})
            assert not client.call("get_compile_status")["job"]["stale"]
            metrics.append(
                {
                    "fixture": name,
                    "clusters": detailed["stats"]["clusters"],
                    "removed_pairs": removed,
                    "fast_work": fast["stats"]["visibility_work"],
                    "detailed_work": detailed["stats"]["visibility_work"],
                    "clear_sightlines": detailed["clear"],
                }
            )
        (args.output / "results.json").write_text(
            json.dumps(
                {
                    "metrics": metrics,
                    "shared_settings": True,
                    "exact_both_path_builds": True,
                    "source_and_history_preserved": True,
                },
                indent=2,
            )
            + "\n"
        )
        return [
            (mode, args.output / f"staggered-{mode}.bsp")
            for mode in ["fast", "detailed"]
        ]
    finally:
        client.close()


def worker(args):
    args.capture_ppm = True
    runtime.OBSERVER = runtime.OBSERVER.replace(
        "setpos -64 -64 32; setang 25 45 0; mat_fullbright",
        "setpos -224 -176 32; setang 20 0 0; r_drawothermodels 0; r_drawviewmodel 0; mat_fullbright",
    )
    runtime.worker(args)


def compare_captures(output, names=("fast", "detailed")):
    captures = []
    for mode in names:
        magic, dimensions, maximum, pixels = (
            (output / mode / "game.ppm").read_bytes().split(b"\n", 3)
        )
        assert magic == b"P6" and maximum == b"255"
        captures.append((dimensions, pixels))
    assert captures[0][0] == captures[1][0] and len(captures[0][1]) == len(
        captures[1][1]
    )
    differences = [abs(a - b) for a, b in zip(captures[0][1], captures[1][1])]
    mean = sum(differences) / len(differences)
    changed = sum(max(pixel) > 2 for pixel in zip(*[iter(differences)] * 3))
    assert mean < 2.0 and changed < len(differences) / 3 * 0.05, (mean, changed)
    return {"mean_component_difference": mean, "changed_pixels": changed}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, default=Path("/tmp/hammerr-portal-flow"))
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
        worker(args)
        return
    cases = fixture(args)
    if args.runtime:
        env = {
            **os.environ,
            "WLR_BACKENDS": "headless",
            "WLR_HEADLESS_OUTPUTS": "1",
            "WLR_RENDERER": "pixman",
        }
        for name, bsp in cases:
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
        report = compare_captures(args.output)
        (args.output / "render-results.json").write_text(
            json.dumps(report, indent=2) + "\n"
        )
    print(json.dumps({"output": str(args.output), "portal_flow": True}))


if __name__ == "__main__":
    main()
