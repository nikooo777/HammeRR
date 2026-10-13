import argparse
import json
from pathlib import Path

from validate_compile_jobs import wait_job
from validate_direct_lighting import center_sample
from validate_face_lighting import ROOT, Client, detail
from validate_visibility import ROOM


def validate(client, output, reference):
    for low, high in ROOM:
        client.call(
            "create_block",
            {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"},
        )
    blocks = []
    for row in range(8):
        for column in range(8):
            x, y = -172 + column * 48, -172 + row * 48
            blocks.append(
                client.call(
                    "create_block",
                    {
                        "mins": [x, y, 64],
                        "maxs": [x + 8, y + 8, 80],
                        "material": "dev/dev_measuregeneric01",
                    },
                )["id"]
            )
    owner = client.call("tie_to_entity", {"ids": blocks, "classname": "func_detail"})[
        "id"
    ]
    client.call(
        "create_entity", {"classname": "info_player_terrorist", "origin": [-64, 0, 8]}
    )
    for classname, origin, properties in [
        ("light", [0, 0, 192], {"_light": "255 0 0 200"}),
        ("light", [128, 128, 192], {"_light": "0 0 255 200"}),
        (
            "light_spot",
            [-128, -128, 192],
            {"_light": "0 255 0 200", "pitch": "-90", "_cone": "45"},
        ),
    ]:
        client.call(
            "create_entity",
            {
                "classname": classname,
                "origin": origin,
                "properties": {
                    "_constant_attn": "1",
                    "_linear_attn": "0",
                    "_quadratic_attn": "0",
                    **properties,
                },
            },
        )
    vmf = output / "dense.vmf"
    bsp = output / "dense.bsp"
    client.call("save_map", {"path": str(vmf)})
    client.call("set_compile_settings", {"lighting": "direct_ldr", "physics": "none"})
    before = detail(client, [owner])
    history = client.call("get_map_info")["undo"]
    source = vmf.read_bytes()
    compiled = client.call("compile_map", {"path": str(bsp)})
    assert compiled["success"], compiled
    stats = compiled["stats"]
    assert stats["brushes"] == 70 and stats["world_lights"] == 3
    assert stats["lightmap_samples"] > 4000
    data = bsp.read_bytes()
    assert center_sample(data)[0] == 200
    client.call("start_compile", {"path": str(output / "background.bsp")})
    job = wait_job(client)
    assert job["status"] == "completed"
    assert (output / "background.bsp").read_bytes() == data
    assert before == detail(client, [owner])
    assert history == client.call("get_map_info")["undo"] and source == vmf.read_bytes()
    report = {"stats": stats}
    if reference:
        baseline = json.loads((reference / "results.json").read_text())["stats"]
        assert data == (reference / "dense.bsp").read_bytes()
        ratio = stats["lighting_work"] / baseline["lighting_work"]
        assert ratio < 0.5, ratio
        report.update({"linear_stats": baseline, "work_ratio": ratio})
    (output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    return report


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-shadow-index")
    )
    parser.add_argument("--reference", type=Path)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    client = Client(None, args.binary, args.output)
    try:
        print(json.dumps(validate(client, args.output, args.reference)))
    finally:
        client.close()


if __name__ == "__main__":
    main()
