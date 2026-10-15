import argparse
import json
import re
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client, detail


def membership(partition):
    vertices = partition["vertices"]
    return [
        sorted(
            vertices[index]["source"]["sides"]
            for index in piece["vertices"]
            if vertices[index]["source"]["kind"] == "corner"
        )
        for piece in partition["loops"]
    ]


def material_fields(path):
    return sorted(
        re.findall(
            r'"(top_material_[01]|future_surface)"\s+"([^"]*)"', path.read_text()
        )
    )


def without_save_counter(path):
    return re.sub(rb'("mapversion"\s+)"\d+"', rb'\1"VERSION"', path.read_bytes())


def validate(client, output):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def saved():
        return client.call("get_saved_component_partitions")

    def resume(sources):
        return client.call(
            "resume_component_partitions",
            {"sources": sources, "snapshot": saved()["snapshot"]},
        )["preview"]

    expected = [
        ("future_surface", "preserve"),
        ("top_material_0", "BRICK/PLANNED_RED"),
        ("top_material_1", "WOOD/PLANNED_BLUE"),
    ]
    for owned in [False, True]:
        for adjusted in [False, True]:
            for direction in range(2):
                label = f"owned={owned} adjusted={adjusted} direction={direction}"
                client.call("new_map", {"discard_changes": True})
                object_id = client.call(
                    "create_block",
                    {"mins": [0.125, -7.25, 16.5], "maxs": [64.875, 48.125, 80.25]},
                )["id"]
                solid = int(object_id.split(":")[1])
                if owned:
                    object_id = client.call(
                        "tie_to_entity",
                        {"ids": [object_id], "classname": "func_detail"},
                    )["id"]
                source = detail(client, [object_id])
                brush = source[0]["brushes"][0] if owned else source[0]
                side = max(brush["faces"], key=lambda face: face["normal"][2])["side"]
                selected = client.call(
                    "select_brush_components",
                    {
                        "components": [{"solid": solid, "sides": [side]}],
                        "snapshot": client.call("get_brush_components")["snapshot"],
                    },
                )
                draft = client.call(
                    "preview_component_partition",
                    {
                        "pairs": [
                            {
                                "solid": solid,
                                "edges": selected["crease_choices"][0]["pairs"][
                                    direction
                                ]["edges"],
                            }
                        ],
                        "snapshot": selected["snapshot"],
                    },
                )["preview"]
                if adjusted:
                    draft = client.call(
                        "update_component_partition_cut",
                        {"token": draft["token"], "edge_parameters": [0.2, 0.8]},
                    )["preview"]
                identity = membership(draft["partition"]["partitions"][0])
                client.call("store_component_partition", {"token": draft["token"]})
                path = (
                    output / f"materials-{int(owned)}-{int(adjusted)}-{direction}.vmf"
                )
                client.call("save_map", {"path": str(path)})
                contents, count = re.subn(
                    r'"kind"\s+"(opposing_midpoints|opposing_edges)"',
                    r'"kind" "\1_materials"\n'
                    r'"top_material_0" "BRICK/PLANNED_RED"\n'
                    r'"top_material_1" "WOOD/PLANNED_BLUE"\n'
                    r'"future_surface" "preserve"',
                    path.read_text(),
                )
                assert count == 1, "Fixture must have one partition intent"
                path.write_text(contents)
                client.call("open_map", {"path": str(path), "discard_changes": True})
                check(
                    saved()["entries"][0]["valid"], f"{label}: styled intent resolves"
                )
                check(
                    detail(client, [object_id]) == source, f"{label}: source unchanged"
                )
                sources = [{"solid": solid, "side": side}]
                normalized = resume(sources)
                client.call("store_component_partition", {"token": normalized["token"]})
                client.call("save_map", {"path": str(path)})
                canonical = without_save_counter(path)
                check(
                    material_fields(path) == expected,
                    f"{label}: planned fields retained",
                )
                draft = resume(sources)
                check(
                    membership(draft["partition"]["partitions"][0]) == identity,
                    f"{label}: original piece identity",
                )
                client.call("cancel_component_edit", {"token": draft["token"]})
                client.call("save_map", {"path": str(path)})
                check(
                    without_save_counter(path) == canonical,
                    f"{label}: Cancel preserves VMF apart from save counters",
                )
                draft = resume(sources)
                history = client.call("get_map_info")["undo"]
                client.call("store_component_partition", {"token": draft["token"]})
                check(
                    client.call("get_map_info")["undo"] == history,
                    f"{label}: unchanged Keep adds no history",
                )
                client.call(
                    "transform_objects",
                    {"ids": [object_id], "reflection": [True, False, False]},
                )
                draft = resume(sources)
                check(
                    membership(draft["partition"]["partitions"][0]) == identity,
                    f"{label}: reflection keeps original membership",
                )
                draft = client.call(
                    "update_component_partition_cut",
                    {"token": draft["token"], "edge_parameters": [0.3, 0.7]},
                )["preview"]
                client.call("store_component_partition", {"token": draft["token"]})
                client.call("save_map", {"path": str(path)})
                check(
                    material_fields(path) == expected,
                    f"{label}: adjusted Keep retains independent plans and unknown data",
                )
                client.call("undo")
                client.call("redo")
                client.call("save_map", {"path": str(path)})
                check(
                    material_fields(path) == expected, f"{label}: history retains plans"
                )
    return checks


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-partition-material-metadata")
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
