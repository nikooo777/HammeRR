import argparse
import json
import os
import re
import struct
import subprocess
import sys
from pathlib import Path

import validate_native_runtime as runtime
from validate_authored_planes import records
from validate_compile_jobs import wait_job
from validate_face_lighting import ROOT, Client, detail
from validate_visibility import ROOM

OBSERVER = runtime.OBSERVER.replace(
    '    local player = Entities.FindByClassname(null, "player");',
    """
    local crate = TraceLine(Vector(-47.875, 32.25, 20.375), Vector(112.125, 32.25, 20.375), null);
    local cabinet = TraceLine(Vector(16, 64, 32), Vector(176, 64, 32), null);
    local decorative = TraceLine(Vector(-176, 128, 20.375), Vector(-16, 128, 20.375), null);
    printl("HAMMERR_STATIC_TRACE " + crate + " " + cabinet + " " + decorative);
    local player = Entities.FindByClassname(null, "player");
""",
)


def static_records(data):
    offset, size = struct.unpack_from("<2i", data, 8 + 35 * 16)
    assert size >= 4
    count = struct.unpack_from("<i", data, offset)[0]
    entries = [
        struct.unpack_from("<IHHii", data, offset + 4 + index * 16)
        for index in range(count)
    ]
    entry = next(row for row in entries if row[0] == int.from_bytes(b"sprp", "big"))
    assert entry[1:3] == (0, 6)
    cursor, end = entry[3], entry[3] + entry[4]
    count = struct.unpack_from("<i", data, cursor)[0]
    cursor += 4
    models = [
        data[cursor + index * 128 : cursor + (index + 1) * 128]
        .split(b"\0", 1)[0]
        .decode()
        for index in range(count)
    ]
    cursor += count * 128
    count = struct.unpack_from("<i", data, cursor)[0]
    cursor += 4
    leaves = list(struct.unpack_from(f"<{count}H", data, cursor))
    cursor += count * 2
    count = struct.unpack_from("<i", data, cursor)[0]
    cursor += 4
    props = [
        struct.unpack_from("<6f3HBBi6fHH", data, cursor + index * 64)
        for index in range(count)
    ]
    assert cursor + count * 64 == end
    return {"models": models, "leaves": leaves, "props": props}


def fixture(client, output):
    ids = []
    for low, high in ROOM:
        client.call(
            "create_block",
            {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"},
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
    ids.append(
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
        )["id"]
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
    ids.append(
        client.call(
            "create_entity",
            {
                "classname": "info_lighting",
                "origin": [32.125, 64.25, 80.375],
                "properties": {"targetname": "sample_origin"},
            },
        )["id"]
    )
    for origin, properties in [
        (
            [32.125, 32.25, 20.375],
            {
                "model": "models/props_junk/wood_crate001a.mdl",
                "angles": "0 37.5 0",
                "solid": "6",
                "skin": "1",
                "fademaxdist": "1024",
                "lightingorigin": "sample_origin",
                "disableshadows": "1",
            },
        ),
        (
            [96, 64, 0.375],
            {"model": "models/props/cs_office/file_cabinet1.mdl", "solid": "2"},
        ),
        (
            [-96, 128, 20.375],
            {"model": "models/props_junk/wood_crate001a.mdl", "solid": "0"},
        ),
    ]:
        ids.append(
            client.call(
                "create_entity",
                {
                    "classname": "prop_static",
                    "origin": origin,
                    "properties": properties,
                },
            )["id"]
        )
    hidden = client.call(
        "create_entity",
        {
            "classname": "prop_static",
            "origin": [0, 0, 0],
            "properties": {"model": "models/missing.mdl"},
        },
    )["id"]
    client.call("set_object_visibility", {"ids": [hidden], "action": "hide"})
    ids.append(hidden)
    vmf = output / "static-props.vmf"
    client.call("save_map", {"path": str(vmf)})
    client.call("open_map", {"path": str(vmf)})
    client.call("set_compile_settings", {"lighting": "direct_ldr", "physics": "engine"})
    before = detail(client, ids)
    history = client.call("get_map_info")["undo"]
    bsp = output / "static-props.bsp"
    result = client.call("compile_map", {"path": str(bsp)})
    assert result["success"], result
    return {
        "bsp": bsp,
        "vmf": vmf,
        "ids": ids,
        "before": before,
        "history": history,
        "result": result,
    }


def validate(client, output):
    fixture_data = fixture(client, output)
    bsp = fixture_data["bsp"]
    data = bsp.read_bytes()
    source = fixture_data["vmf"].read_bytes()
    parsed = static_records(data)
    stats = fixture_data["result"]["stats"]
    assert stats["static_props"] == len(parsed["props"]) == 3
    assert stats["static_prop_models"] == len(parsed["models"]) == 2
    assert stats["static_prop_leaf_refs"] == len(parsed["leaves"])
    assert sorted(row[9] for row in parsed["props"]) == [0, 2, 6]
    prop = parsed["props"][0]
    assert prop[:6] == (32.125, 32.25, 20.375, 0, 37.5, 0)
    assert prop[10:18] == (1 | 2 | 16, 1, 1024, 1024, 32.125, 64.25, 80.375, 1)
    leaves = records(data, 10, "<ihh6h4HhH", expected_version=1)
    assert all(
        0 <= leaf < len(leaves) and leaves[leaf][0] & 1 == 0
        for leaf in parsed["leaves"]
    )
    assert stats["world_lights"] == 1 and stats["physics_solids"] == 1
    offset, length = struct.unpack_from("<2i", data, 8)
    entities = data[offset : offset + length]
    assert b'"prop_static"' not in entities and b'"info_lighting"' not in entities
    background = output / "static-props-background.bsp"
    client.call("start_compile", {"path": str(background)})
    job = wait_job(client)
    assert job["status"] == "completed" and background.read_bytes() == data, job
    assert detail(client, fixture_data["ids"]) == fixture_data["before"]
    assert client.call("get_map_info")["undo"] == fixture_data["history"]
    assert fixture_data["vmf"].read_bytes() == source
    missing = client.call(
        "create_entity",
        {
            "classname": "prop_static",
            "origin": [0, 0, 32],
            "properties": {"model": "models/missing.mdl"},
        },
    )["id"]
    response = client.call("compile_map", {"path": str(bsp)}, error=True)
    failure = json.loads(response["content"][0]["text"])
    assert not failure["success"] and bsp.read_bytes() == data
    assert any(
        row["object"] == missing and "was not found" in row["message"]
        for row in failure["diagnostics"]
    )
    client.call("undo")
    assert detail(client, fixture_data["ids"]) == fixture_data["before"]
    (output / "results.json").write_text(
        json.dumps(
            {"stats": stats, "records": parsed, "failed_asset": failure}, indent=2
        )
        + "\n"
    )
    return bsp


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-static-props")
    )
    parser.add_argument("--runtime", action="store_true")
    parser.add_argument(
        "--game-dir",
        type=Path,
        default=Path.home()
        / ".local/share/Steam/steamapps/common/Counter-Strike Source",
    )
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--bsp", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    if args.worker:
        runtime.OBSERVER = OBSERVER
        runtime.worker(args)
        text = (args.output / "cstrike/hammerr-runtime.log").read_text(errors="replace")
        traces = [
            list(map(float, values.split()))
            for values in re.findall(r"HAMMERR_STATIC_TRACE ([\d.eE+ -]+)", text)
        ]
        assert len(traces) >= 3, text[-4000:]
        assert all(
            0 < row[0] < 0.9 and 0 < row[1] < 0.9 and row[2] > 0.999 for row in traces
        ), traces
        (args.output / "static-traces.json").write_text(
            json.dumps(traces, indent=2) + "\n"
        )
        return
    client = Client(None, args.binary, args.output)
    try:
        bsp = validate(client, args.output)
        print(json.dumps({"bsp": str(bsp), "output": str(args.output)}))
    finally:
        client.close()
    if args.runtime:
        output = args.output / "game"
        output.mkdir(exist_ok=True)
        env = {
            **os.environ,
            "WLR_BACKENDS": "headless",
            "WLR_HEADLESS_OUTPUTS": "1",
            "WLR_RENDERER": "pixman",
        }
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
            str(bsp),
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


if __name__ == "__main__":
    main()
