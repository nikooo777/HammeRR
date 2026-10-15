import argparse
import json
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client, detail, face_ref


def validate(client, output):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    block = client.call(
        "create_block",
        {"mins": [70000, 0, 0], "maxs": [70016, 16, 16], "material": "audit"},
    )["id"]
    source = detail(client, [block])
    history = client.call("get_map_info")["undo"]
    destination = output / "bounds.bsp"
    destination.write_bytes(b"previous build")
    failed = client.call("compile_map", {"path": str(destination)}, error=True)
    diagnostics = json.loads(failed["content"][0]["text"])["diagnostics"]
    check(
        any(
            "16384" in diagnostic["message"] and diagnostic["object"] == block
            for diagnostic in diagnostics
        ),
        "Out-of-range compile returns an object-specific bounds diagnostic",
    )
    check(
        destination.read_bytes() == b"previous build", "Rejected compile preserves BSP"
    )
    check(
        detail(client, [block]) == source
        and client.call("get_map_info")["undo"] == history,
        "Rejected compile preserves geometry and undo",
    )
    check(
        not client.call("get_map_info")["compiling"], "Compile worker finishes cleanly"
    )

    client.call("new_map", {"discard_changes": True})
    for mins, maxs in [
        ([-272, -272, -16], [272, 272, 0]),
        ([-272, -272, 256], [272, 272, 272]),
        ([-272, -272, 0], [-256, 272, 256]),
        ([256, -272, 0], [272, 272, 256]),
        ([-256, -272, 0], [256, -256, 256]),
        ([-256, 256, 0], [256, 272, 256]),
    ]:
        client.call(
            "create_block",
            {"mins": mins, "maxs": maxs, "material": "dev/dev_measuregeneric01"},
        )
    client.call(
        "create_entity", {"classname": "info_player_terrorist", "origin": [0, 0, 8]}
    )
    compiled = client.call("compile_map", {"path": str(output / "room.bsp")})
    check(
        compiled["success"],
        "The same MCP session compiles a valid room after rejection",
    )

    duplicate = output / "duplicate.vmf"
    duplicate.write_text(
        "world { id 1 } entity { id 2 classname info_target } "
        "entity { id 2 classname light }"
    )
    before = client.call("get_map_info")
    rejected = client.call(
        "open_map", {"path": str(duplicate), "discard_changes": True}, error=True
    )
    check(
        "duplicate object ID 2" in json.dumps(rejected),
        "Duplicate import identifies the ID",
    )
    after = client.call("get_map_info")
    check(
        all(
            before[key] == after[key]
            for key in ["path", "undo", "redo", "camera", "selection"]
        ),
        "Rejected import preserves the open map, history, selection and camera",
    )

    extension = output / "extension.vmf"
    extension.write_text(
        "world { id 1 } visgroups { vendor_extension { data keep_me } vendor_value preserved }"
    )
    client.call("open_map", {"path": str(extension), "discard_changes": True})
    saved = output / "extension-saved.vmf"
    client.call("save_map", {"path": str(saved)})
    client.call("open_map", {"path": str(saved)})
    check(
        all(
            value in saved.read_text()
            for value in ["vendor_extension", "keep_me", "vendor_value", "preserved"]
        ),
        "Unknown root visgroup data survives save and reopen",
    )

    block = client.call(
        "create_block",
        {
            "mins": [0, 0, 0],
            "maxs": [64, 64, 64],
            "material": "dev/dev_measuregeneric01",
        },
    )["id"]
    for index in range(100):
        client.call(
            "transform_objects",
            {"ids": [block], "translation": [1 if index % 2 == 0 else -1, 0, 0]},
        )
    source = detail(client, [block])
    history = client.call("get_map_info")["undo"]
    face = face_ref(source[0], source[0]["faces"][0])
    client.call("edit_faces", {"faces": [face], "shift_u": 7, "preview": True})
    client.call(
        "transform_objects",
        {"ids": [block], "translation": [0, 16, 0], "preview": True},
    )
    check(
        not client.call("get_map_info")["face_selection"]["preview"],
        "Transform draft replaces the face draft",
    )
    hollow = client.call(
        "preview_hollow",
        {
            "ids": [block],
            "thickness": 8,
            "stamp": client.call("get_hollow_preview")["stamp"],
        },
    )["preview"]
    primitive = client.call(
        "create_primitive",
        {"shape": "wedge", "mins": [96, 0, 0], "maxs": [160, 64, 64], "preview": True},
    )["state"]["preview"]
    client.call("commit_hollow", {"token": hollow["token"]}, error=True)
    check(
        detail(client, [block]) == source
        and client.call("get_map_info")["undo"] == history,
        "Preview transitions and stale apply preserve source geometry and history",
    )
    client.call("cancel_primitive_preview", {"token": primitive["token"]})
    client.call("commit_primitive_preview", {"token": primitive["token"]}, error=True)
    check(
        detail(client, [block]) == source,
        "Cancelling the draft preserves source geometry",
    )
    client.call(
        "transform_objects",
        {"ids": [block], "translation": [0, 16, 0], "preview": True},
    )
    client.call("commit_transform_preview")
    client.call("undo")
    check(
        detail(client, [block]) == source,
        "Applying then undoing a draft restores exact source geometry",
    )
    return checks


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-audit-validation")
    )
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    client = Client(None, args.binary, args.output)
    try:
        checks = validate(client, args.output)
        result = {"passed": len(checks), "checks": checks}
        (args.output / "results.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps(result, indent=2))
    finally:
        client.close()


if __name__ == "__main__":
    main()
