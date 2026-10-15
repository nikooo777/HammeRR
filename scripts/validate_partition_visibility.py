import argparse
import json
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client, detail


def validate(client, output):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    for owned in [False, True]:
        client.call("new_map", {"discard_changes": True})
        object_id = client.call(
            "create_block",
            {"mins": [0, 0, 0], "maxs": [64, 64, 64], "material": "tools/toolsnodraw"},
        )["id"]
        solid = int(object_id.split(":")[1])
        if owned:
            object_id = client.call(
                "tie_to_entity", {"ids": [object_id], "classname": "func_detail"}
            )["id"]
        source = detail(client, [object_id])[0]
        brush = source["brushes"][0] if owned else source
        side = max(brush["faces"], key=lambda face: face["normal"][2])["side"]
        state = client.call("get_brush_components")
        selected = client.call(
            "select_brush_components",
            {
                "components": [{"solid": solid, "sides": [side]}],
                "snapshot": state["snapshot"],
            },
        )
        draft = client.call(
            "preview_component_partition",
            {
                "pairs": [
                    {
                        "solid": solid,
                        "edges": selected["crease_choices"][0]["pairs"][0]["edges"],
                    }
                ],
                "snapshot": selected["snapshot"],
            },
        )["preview"]
        client.call("set_view", {"tool_textures": False})
        client.call(
            "select_component_partitions",
            {"token": draft["token"], "partitions": [{"solid": solid, "partition": 0}]},
        )
        draft = client.call(
            "preview_partition_extrusion",
            {
                "token": draft["token"],
                "height": 32,
                "wall_material": "brick/brickwall001a",
            },
        )["preview"]
        arguments = {
            "surface": "flat",
            "tool_textures": False,
            "camera": {
                "projection": "perspective",
                "position": [170, -200, 180],
                "look_at": [32, 32, 64],
            },
        }
        highlighted = client.render(
            output / f"highlighted-{int(owned)}.png", **arguments
        )
        client.call(
            "select_component_partitions",
            {"token": draft["token"], "partitions": []},
        )
        unhighlighted = client.render(
            output / f"unhighlighted-{int(owned)}.png", **arguments
        )
        check(
            highlighted != unhighlighted,
            f"owned={owned}: visible walls retain colored piece guides",
        )
        client.call("cancel_component_edit", {"token": draft["token"]})
        check(
            detail(client, [object_id])[0] == source,
            f"owned={owned}: source remains untouched",
        )
    return checks


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-partition-visibility")
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
