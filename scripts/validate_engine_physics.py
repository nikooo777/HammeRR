import argparse
import json
import struct
from pathlib import Path

from validate_authored_planes import records
from validate_compile_jobs import wait_job
from validate_face_lighting import ROOT, Client, detail
from validate_visibility import ROOM


def physics_records(data):
    offset, size, version, compression = struct.unpack_from("<4i", data, 8 + 29 * 16)
    assert version == compression == 0 and 16 <= size <= 64 * 1024**2
    cursor, end = offset, offset + size
    models = []
    while cursor < end:
        index, body_size, key_size, count = struct.unpack_from("<4i", data, cursor)
        cursor += 16
        if index == -1:
            assert (body_size, key_size, count) == (-1, 0, 0) and cursor == end
            return models
        body_end = cursor + body_size
        assert 0 <= index < len(records(data, 14, "<9f3i"))
        assert 0 < count <= 32767 and 0 < key_size <= 65535
        solids = []
        for _ in range(count):
            length = struct.unpack_from("<i", data, cursor)[0]
            cursor += 4
            assert 28 <= length <= 16 * 1024**2 and cursor + length <= body_end
            magic, blob_version, kind, embedded = struct.unpack_from(
                "<4sHHI", data, cursor
            )
            assert (magic, blob_version, kind) == (b"VPHY", 0x100, 0)
            assert embedded == length - 28
            solids.append(data[cursor : cursor + length])
            cursor += length
        assert cursor == body_end and cursor + key_size <= end
        keys = data[cursor : cursor + key_size]
        assert keys[-1:] == b"\0" and b"\0" not in keys[:-1]
        cursor += key_size
        models.append((index, solids, keys))
    raise AssertionError("Missing physics end marker")


def validate(client, output):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    initial = client.call("get_compile_settings")
    check(initial["physics"] == "none", "Physics bodies remain optional by default")
    client.call(
        "set_compile_settings",
        {"lighting": "direct_ldr", "physics": "unknown"},
        error=True,
    )
    check(
        client.call("get_compile_settings") == initial,
        "Invalid combined settings preserve every previous field",
    )
    client.call("set_compile_settings", {"lighting": "direct_ldr", "physics": "engine"})
    for label in ["world", "clips_and_pivot", "hidden"]:
        client.call("new_map", {"discard_changes": True})
        ids = [
            client.call(
                "create_block",
                {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"},
            )["id"]
            for low, high in ROOM
        ]
        ids.append(
            client.call(
                "create_entity",
                {"classname": "info_player_terrorist", "origin": [-64, 0, 8]},
            )["id"]
        )
        ids.append(
            client.call(
                "create_entity",
                {
                    "classname": "light",
                    "origin": [0, 0, 100],
                    "properties": {"_light": "255 100 40 200", "_quadratic_attn": "1"},
                },
            )["id"]
        )
        if label == "clips_and_pivot":
            ids.append(
                client.call(
                    "create_block",
                    {
                        "mins": [-16, -16, 0],
                        "maxs": [16, 16, 48],
                        "material": "tools/toolsclip",
                    },
                )["id"]
            )
            brush = client.call(
                "create_block",
                {
                    "mins": [80, 80, 16],
                    "maxs": [112, 112, 48],
                    "material": "dev/dev_measuregeneric01",
                },
            )["id"]
            ids.append(
                client.call(
                    "tie_to_entity",
                    {
                        "ids": [brush],
                        "classname": "func_brush",
                        "properties": {
                            "origin": "96 96 32",
                            "targetname": "physics_pivot",
                        },
                    },
                )["id"]
            )
        if label == "hidden":
            brush = client.call(
                "create_block",
                {
                    "mins": [-16, -16, 0],
                    "maxs": [16, 16, 48],
                    "material": "dev/dev_measuregeneric01",
                },
            )["id"]
            ids.append(brush)
            client.call("set_object_visibility", {"ids": [brush], "action": "hide"})
        vmf = output / f"{label}.vmf"
        client.call("save_map", {"path": str(vmf)})
        client.call("open_map", {"path": str(vmf)})
        before = detail(client, ids)
        history = client.call("get_map_info")["undo"]
        saved = vmf.read_bytes()
        bsp = output / f"{label}.bsp"
        compiled = client.call("compile_map", {"path": str(bsp)})
        check(
            compiled["success"],
            f"{label}: shared Direct LDR/engine physics compile succeeds",
        )
        data = bsp.read_bytes()
        parsed = physics_records(data)
        stats = compiled["stats"]
        check(
            stats["physics_mode"] == "engine"
            and stats["lighting_mode"] == "direct_ldr",
            f"{label}: build result reports both frozen modes",
        )
        check(
            stats["physics_models"]
            == len(parsed)
            == (2 if label == "clips_and_pivot" else 1),
            f"{label}: native model references match shared counts",
        )
        check(
            stats["physics_solids"]
            == sum(len(solids) for _, solids, _ in parsed)
            == (4 if label == "clips_and_pivot" else 1),
            f"{label}: native body counts match contents groups",
        )
        check(
            stats["physics_convexes"] == (9 if label == "clips_and_pivot" else 6),
            f"{label}: counts include world/clip/local bodies and omit hidden brushes",
        )
        check(
            stats["lighting_faces"] > 0 and stats["world_lights"] == 1,
            f"{label}: physics preserves real baked lighting",
        )
        if label == "clips_and_pivot":
            model = records(data, 14, "<9f3i")[1]
            check(
                model[:3] == (-16, -16, -16) and model[3:6] == (16, 16, 16),
                "Pivot brush has checked local-coordinate model bounds",
            )
            check(
                b'"65536"' in parsed[0][2]
                and b'"131072"' in parsed[0][2]
                and b"materialtable" in parsed[0][2],
                "World key data retains separate player/NPC clipping and default surfaces",
            )
            check(
                b'"mass" "1"' in parsed[1][2]
                and b'"surfaceprop" "default"' in parsed[1][2],
                "Brush-model key data states the current mass/surface fallback",
            )
        background = output / f"{label}-background.bsp"
        client.call("start_compile", {"path": str(background)})
        job = wait_job(client)
        check(
            job["status"] == "completed"
            and job["settings"] == {"lighting": "direct_ldr", "physics": "engine"},
            f"{label}: background job freezes both shared options",
        )
        check(
            background.read_bytes() == data,
            f"{label}: synchronous and background native payloads agree",
        )
        client.call("set_compile_settings", {"physics": "none"})
        check(
            client.call("get_compile_settings")["lighting"] == "direct_ldr"
            and client.call("get_compile_status")["job"]["stale"],
            f"{label}: partial settings preserve lighting and invalidate previous physics builds",
        )
        client.call("set_compile_settings", {"physics": "engine"})
        check(
            not client.call("get_compile_status")["job"]["stale"],
            f"{label}: restoring options restores snapshot freshness",
        )
        check(
            detail(client, ids) == before
            and history == client.call("get_map_info")["undo"]
            and vmf.read_bytes() == saved,
            f"{label}: native physics preserves source and undo history",
        )
    return checks


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-engine-physics")
    )
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    client = Client(None, args.binary, args.output)
    try:
        checks = validate(client, args.output)
        (args.output / "results.json").write_text(
            json.dumps({"checks": len(checks), "passed": checks}, indent=2) + "\n"
        )
        print(json.dumps({"checks": len(checks), "output": str(args.output)}))
    finally:
        client.close()


if __name__ == "__main__":
    main()
