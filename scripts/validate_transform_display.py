import argparse
import json
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client, detail


def validate(client):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    client.call("new_map", {"discard_changes": True})
    object_id = client.call(
        "create_block",
        {"mins": [0, 0, 0], "maxs": [64, 64, 64], "material": "brick/test"},
    )["id"]
    client.call("set_transform_tool", {"mode": "select"})
    before = detail(client, [object_id])
    history = client.call("get_map_info")["undo"]
    variants = [
        ({"translation": [12.125, -7.25, 3.5]}, "move"),
        ({"rotation": [17, -23, 31]}, "rotate"),
        ({"scale": [1.25, 0.75, 1.5]}, "scale"),
        ({"shear": {"axis": "x", "reference": "y", "factor": 0}}, "identity"),
        ({"rotation": [360, -720, 0]}, "identity"),
    ]
    for previous in [
        {"reflection": [True, False, False]},
        {"shear": {"axis": "x", "reference": "y", "factor": 0.5}},
    ]:
        for spec, operation in variants:
            client.call(
                "transform_objects", {"ids": [object_id], "preview": True, **previous}
            )
            args = {"ids": [object_id], "preview": True, **spec}
            draft = client.call("transform_objects", args)["state"]["preview"]
            display = draft["display"]
            check(display["primary"] == operation, f"{previous}/{spec}: correct mode")
            check(
                display["operations"]
                == ([] if operation == "identity" else [operation]),
                f"{previous}/{spec}: effective operations",
            )
            state = client.call("get_transform_handles")["state"]
            check(
                state["preview"]["display"] == display
                and state["settings"]["mode"] == "select",
                f"{previous}/{spec}: inspection preserves Select handles",
            )
            check(
                detail(client, [object_id]) == before
                and client.call("get_map_info")["undo"] == history,
                f"{previous}/{spec}: source and history unchanged",
            )
            client.call("cancel_transform_preview")
            check(detail(client, [object_id]) == before, f"{previous}/{spec}: cancel")
            client.call("transform_objects", args)
            client.call("commit_transform_preview")
            check(
                detail(client, [object_id]) == draft["objects"],
                f"{previous}/{spec}: apply matches draft",
            )
            if operation != "identity":
                client.call("undo")
    spec = {
        "translation": [8, 0, 0],
        "rotation": [0, 0, 15],
        "scale": [1.25, 1, 1],
        "reflection": [True, False, False],
        "shear": {"axis": "x", "reference": "y", "factor": 0.5},
    }
    draft = client.call(
        "transform_objects", {"ids": [object_id], "preview": True, **spec}
    )["state"]["preview"]
    check(
        draft["display"]["operations"]
        == ["move", "rotate", "scale", "mirror", "shear"],
        "Compound draft exposes every operation",
    )
    check(
        draft["display"]["summary"]
        == "Move: 8, 0, 0 units\nRotate: 0, 0, 15 degrees\nScale: 1.25, 1, 1 factors\nMirror: X\nShear: X by Y × 0.5",
        "Compound summary describes all values",
    )
    client.call("cancel_transform_preview")
    check(detail(client, [object_id]) == before, "Compound cancel preserves source")
    return checks


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-transform-display")
    )
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    client = Client(None, args.binary, args.output)
    try:
        checks = validate(client)
        (args.output / "results.json").write_text(
            json.dumps({"checks": len(checks), "passed": checks}, indent=2) + "\n"
        )
        print(json.dumps({"checks": len(checks), "output": str(args.output)}))
    finally:
        client.close()


if __name__ == "__main__":
    main()
