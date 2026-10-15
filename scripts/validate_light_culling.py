import argparse
import json
from pathlib import Path

from validate_compile_jobs import wait_job
from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_visibility import ROOM


def validate(client, output):
    results = {}
    for connected in [False, True]:
        client.call("new_map", {"discard_changes": True})
        walls = (
            [([-16, -256, 0], [16, -64, 256]), ([-16, 64, 0], [16, 256, 256])]
            if connected
            else [([-16, -256, 0], [16, 256, 256])]
        )
        ids = []
        for mins, maxs in ROOM + walls:
            ids.append(
                client.call(
                    "create_block",
                    {
                        "mins": mins,
                        "maxs": maxs,
                        "material": "dev/dev_measuregeneric01",
                    },
                )["id"]
            )
        for classname, origin in [
            ("info_player_terrorist", [-128, -128, 8]),
            ("info_player_counterterrorist", [128, -128, 8]),
        ]:
            ids.append(
                client.call(
                    "create_entity", {"classname": classname, "origin": origin}
                )["id"]
            )
        solid = client.call(
            "create_block",
            {
                "mins": [64, 64, 16],
                "maxs": [112, 112, 64],
                "material": "dev/dev_measuregeneric01",
            },
        )["id"]
        ids.append(
            client.call(
                "tie_to_entity",
                {
                    "ids": [solid],
                    "classname": "func_brush",
                    "properties": {"origin": "90.5 91.25 30.125"},
                },
            )["id"]
        )
        for x, color in [(-128, "255 0 0 200"), (128, "0 0 255 200")]:
            for index in range(8):
                ids.append(
                    client.call(
                        "create_entity",
                        {
                            "classname": "light",
                            "origin": [x, index * 8 - 28, 128],
                            "properties": {
                                "_light": color,
                                "targetname": f"switch_{x}",
                                "_distance": "140",
                            },
                        },
                    )["id"]
                )
        name = "connected" if connected else "disconnected"
        source = output / f"{name}.vmf"
        client.call("save_map", {"path": str(source)})
        source_bytes = source.read_bytes()
        objects = detail(client, ids)
        history = client.call("get_map_info")["undo"]
        for visibility in ["fast", "detailed"]:
            client.call(
                "set_compile_settings",
                {"lighting": "direct_ldr", "physics": "none", "visibility": visibility},
            )
            target = output / f"{name}-{visibility}.bsp"
            result = client.call("compile_map", {"path": str(target)})
            assert result["success"], result
            stats = result["stats"]
            assert stats["lit_brush_models"] == 1
            assert stats["switchable_light_groups"] == 2
            assert stats["light_evaluations"] > 0 and stats["light_range_skipped"] > 0
            assert stats["light_culling_work"] > 0
            if not connected:
                assert stats["light_pvs_skipped"] > 0
            assert (
                stats["light_evaluations"]
                + stats["light_pvs_skipped"]
                + stats["light_range_skipped"]
                == stats["lightmap_samples"] * stats["world_lights"]
            )
            background = output / f"{name}-{visibility}-background.bsp"
            client.call("start_compile", {"path": str(background)})
            job = wait_job(client)
            assert job["status"] == "completed", job
            assert job["settings"]["visibility"] == visibility
            assert background.read_bytes() == target.read_bytes()
            assert all(
                job["result"]["stats"][key] == stats[key]
                for key in [
                    "light_evaluations",
                    "light_pvs_skipped",
                    "light_range_skipped",
                    "light_culling_work",
                    "lighting_work",
                ]
            )
            assert (
                detail(client, ids) == objects
                and client.call("get_map_info")["undo"] == history
            )
            assert source.read_bytes() == source_bytes
            results[f"{name}-{visibility}"] = stats
    return results


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-light-culling")
    )
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    client = Client(None, args.binary, args.output)
    try:
        results = validate(client, args.output)
    finally:
        client.close()
    (args.output / "results.json").write_text(json.dumps(results, indent=2) + "\n")
    print(
        json.dumps(
            {
                name: {
                    key: stats[key]
                    for key in [
                        "light_evaluations",
                        "light_pvs_skipped",
                        "light_range_skipped",
                        "light_culling_work",
                        "lighting_work",
                    ]
                }
                for name, stats in results.items()
            },
            indent=2,
        )
    )


if __name__ == "__main__":
    main()
