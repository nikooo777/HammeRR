import argparse
import hashlib
import json
import math
from pathlib import Path

from validate_face_lighting import ROOT, Client, detail


def area(points):
    origin = points[0]
    vector = [0.0] * 3
    for index in range(1, len(points) - 1):
        a = [value - origin[axis] for axis, value in enumerate(points[index])]
        b = [value - origin[axis] for axis, value in enumerate(points[index + 1])]
        cross = [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
        vector = [value + cross[axis] for axis, value in enumerate(vector)]
    return math.sqrt(sum(value * value for value in vector)) * 0.5


def validate(client, output, render=False):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def inspect(solid=None):
        return client.call("get_brush_components", {"solids": [solid]} if solid else {})

    tools = {tool["name"] for tool in client.rpc("tools/list", {})["tools"]}
    check(
        {
            "preview_component_partition",
            "raise_component_partition",
            "select_component_partitions",
            "frame_component_partitions",
            "commit_component_edit",
            "cancel_component_edit",
        }
        <= tools,
        "All shared partition operations are discoverable",
    )
    for owned in [False, True]:
        for oblique in [False, True]:
            for direction in range(2):
                label = f"owned={owned} oblique={oblique} direction={direction}"
                client.call("new_map", {"discard_changes": True})
                object_id = client.call(
                    "create_block",
                    {
                        "mins": [0.125, -7.25, 16.5],
                        "maxs": [64.875, 48.125, 80.25],
                        "material": "dev/dev_measuregeneric01",
                    },
                )["id"]
                solid = int(object_id.split(":")[1])
                if oblique:
                    client.call(
                        "transform_objects",
                        {
                            "ids": [object_id],
                            "rotation": [17, -23, 31],
                            "scale": [1.125, 0.75, 1.5],
                            "pivot": [0, 0, 0],
                        },
                    )
                if owned:
                    object_id = client.call(
                        "tie_to_entity",
                        {"ids": [object_id], "classname": "func_detail"},
                    )["id"]
                path = output / f"source-{int(owned)}-{int(oblique)}-{direction}.vmf"
                client.call("save_map", {"path": str(path)})
                saved = hashlib.sha256(path.read_bytes()).hexdigest()
                source = detail(client, [object_id])
                history = client.call("get_map_info")["undo"]
                shape = inspect(solid)["brushes"][0]
                brush = source[0]["brushes"][0] if owned else source[0]
                face = max(brush["faces"], key=lambda face: face["normal"][2])
                selected = client.call(
                    "select_brush_components",
                    {
                        "components": [{"solid": solid, "sides": [face["side"]]}],
                        "snapshot": inspect()["snapshot"],
                    },
                )
                client.call(
                    "set_component_picking",
                    {
                        "enabled": True,
                        "kind": "face",
                        "solids": [solid],
                        "snapshot": inspect()["snapshot"],
                    },
                )
                choices = selected["crease_choices"][0]["pairs"]
                inputs = [{"solid": solid, "edges": choices[direction]["edges"]}]
                camera = {"position": [170, -200, 180], "look_at": [30, 20, 45]}
                render_args = {"surface": "flat", "camera": camera}
                if render:
                    baseline = client.render(
                        output / f"{label}-before.png", **render_args
                    )
                stamp = inspect()["snapshot"]
                draft = client.call(
                    "preview_component_partition", {"pairs": inputs, "snapshot": stamp}
                )
                preview = draft["preview"]
                token = preview["token"]
                part = preview["partition"]["partitions"][0]
                vertices = part["vertices"]
                check(
                    preview["operation"] == "partition"
                    and not preview["can_apply"]
                    and preview["partition"]["rise"] == 0,
                    f"{label}: temporary flat draft cannot be applied",
                )
                check(
                    part["owner"] == object_id
                    and part["plane"] == face["plane"]
                    and len(vertices) == 6
                    and len(part["loops"]) == 2,
                    f"{label}: source owner, authored plane and bounded topology",
                )
                check(
                    [vertex["id"] for vertex in vertices] == list(range(6))
                    and [piece["id"] for piece in part["loops"]] == [0, 1],
                    f"{label}: deterministic draft identifiers",
                )
                check(
                    all(
                        vertex["position"] == vertex["preview_position"]
                        for vertex in vertices
                    )
                    and all(
                        vertex["source"]["kind"] == "corner" for vertex in vertices[:4]
                    )
                    and all(
                        vertex["source"]["kind"] == "edge"
                        and vertex["source"]["parameter"] == 0.5
                        for vertex in vertices[4:]
                    ),
                    f"{label}: exact flat corner and midpoint incidence",
                )
                for vertex in vertices[4:]:
                    endpoints = [
                        next(
                            candidate["position"]
                            for candidate in shape["vertices"]
                            if candidate["sides"] == sides
                        )
                        for sides in vertex["source"]["corners"]
                    ]
                    midpoint = [(a + b) * 0.5 for a, b in zip(*endpoints, strict=True)]
                    check(
                        math.dist(midpoint, vertex["position"]) < 1e-8,
                        f"{label}: midpoint {vertex['id']} resolves its source endpoints",
                    )
                combined_area = sum(
                    area([vertices[index]["position"] for index in piece["vertices"]])
                    for piece in part["loops"]
                )
                check(
                    math.isclose(
                        combined_area,
                        area([vertex["position"] for vertex in vertices[:4]]),
                        rel_tol=1e-10,
                        abs_tol=1e-8,
                    ),
                    f"{label}: loops cover the complete source face",
                )
                check(
                    len(draft["brushes"][0]["faces"]) == 6
                    and len(draft["brushes"][0]["vertices"]) == 8
                    and detail(client, [object_id]) == source
                    and client.call("get_map_info")["undo"] == history,
                    f"{label}: flat topology leaves ordinary brush geometry and history unchanged",
                )
                for name, args in [
                    ("commit_component_edit", {"token": token}),
                    ("save_map", {"path": str(path)}),
                    ("compile_map", {"path": str(path.with_suffix(".bsp"))}),
                    ("new_map", {"discard_changes": False}),
                    (
                        "select_brush_components",
                        {"components": [], "snapshot": inspect()["snapshot"]},
                    ),
                    (
                        "preview_component_partition",
                        {"pairs": inputs, "snapshot": stamp},
                    ),
                ]:
                    client.call(name, args, error=True)
                check(
                    hashlib.sha256(path.read_bytes()).hexdigest() == saved
                    and inspect()["preview"]["token"] == token,
                    f"{label}: save, compile, discard and stale-input guards preserve the draft and file",
                )
                for rise in [-1, 0.01, 4097]:
                    client.call(
                        "raise_component_partition",
                        {"token": token, "rise": rise},
                        error=True,
                    )
                client.call(
                    "select_component_partitions",
                    {"token": token, "partitions": [{"solid": solid, "partition": 9}]},
                    error=True,
                )
                check(
                    inspect()["preview"]["token"] == token,
                    f"{label}: invalid heights and loop refs retain the valid draft",
                )
                if render:
                    flat_pixels = client.render(
                        output / f"{label}-flat.png", **render_args
                    )
                    check(
                        flat_pixels != baseline,
                        f"{label}: renderer displays the independent flat partition edge",
                    )
                parts = [{"solid": solid, "partition": 0}]
                selected = client.call(
                    "select_component_partitions", {"token": token, "partitions": parts}
                )
                check(
                    selected["preview"]["partition"]["selection"] == parts,
                    f"{label}: loop highlight selection is separate from source selection",
                )
                client.call(
                    "frame_component_partitions", {"token": token, "aspect": 0.6}
                )
                client.call(
                    "select_component_partitions",
                    {"token": token, "partitions": [], "mode": "replace"},
                )
                client.call("frame_component_partitions", {"token": token}, error=True)
                client.call(
                    "select_component_partitions",
                    {"token": token, "partitions": parts, "mode": "add"},
                )
                raised = client.call(
                    "raise_component_partition", {"token": token, "rise": 16.125}
                )
                raised_token = raised["preview"]["token"]
                raised_part = raised["preview"]["partition"]["partitions"][0]
                check(
                    raised_token != token
                    and raised["preview"]["can_apply"]
                    and len(raised["brushes"][0]["vertices"]) == 10
                    and len(raised["brushes"][0]["faces"]) == 7,
                    f"{label}: checked ridge materializes real convex brush topology",
                )
                check(
                    raised_part["plane"] == part["plane"]
                    and [vertex["position"] for vertex in raised_part["vertices"]]
                    == [vertex["position"] for vertex in vertices]
                    and raised["preview"]["partition"]["selection"] == parts,
                    f"{label}: raising preserves immutable source and highlight selection",
                )
                for old, new in zip(
                    vertices[4:], raised_part["vertices"][4:], strict=True
                ):
                    expected = [
                        old["position"][axis] + face["normal"][axis] * 16.125
                        for axis in range(3)
                    ]
                    check(
                        math.dist(expected, new["preview_position"]) < 1e-8,
                        f"{label}: raised midpoint {old['id']} follows the source normal",
                    )
                client.call(
                    "raise_component_partition", {"token": token, "rise": 0}, error=True
                )
                client.call("commit_component_edit", {"token": token}, error=True)
                flat = client.call(
                    "raise_component_partition", {"token": raised_token, "rise": 0}
                )
                check(
                    not flat["preview"]["can_apply"]
                    and len(flat["brushes"][0]["vertices"]) == 8
                    and detail(client, [object_id]) == source,
                    f"{label}: zero rise restores the flat draft without editing the source",
                )
                cancelled = client.call(
                    "cancel_component_edit", {"token": flat["preview"]["token"]}
                )
                check(
                    cancelled["source_selection"] == selected["source_selection"]
                    and detail(client, [object_id]) == source
                    and client.call("get_map_info")["undo"] == history,
                    f"{label}: cancel preserves source component selection and history",
                )
                if render:
                    check(
                        client.render(output / f"{label}-cancelled.png", **render_args)
                        == baseline,
                        f"{label}: cancel restores exact original pixels",
                    )
                staged = client.call(
                    "preview_component_partition",
                    {"pairs": inputs, "snapshot": inspect()["snapshot"]},
                )
                raised = client.call(
                    "raise_component_partition",
                    {"token": staged["preview"]["token"], "rise": 16.125},
                )
                check(
                    raised["preview"]["crease"]["changes"][0]["new_side"]
                    == max(candidate["side"] for candidate in brush["faces"]) + 1,
                    f"{label}: flat and cancelled drafts do not consume face IDs",
                )
                if render:
                    raised_pixels = client.render(
                        output / f"{label}-raised.png", **render_args
                    )
                if render:
                    check(
                        raised_pixels != flat_pixels,
                        f"{label}: renderer displays the raised ridge",
                    )
                client.call(
                    "commit_component_edit", {"token": raised["preview"]["token"]}
                )
                applied = detail(client, [object_id])
                check(
                    inspect(solid)["brushes"] == raised["brushes"],
                    f"{label}: applied topology matches the complete raised draft",
                )
                check(
                    client.call("get_map_info")["undo"][:1]
                    == ["raise flat face partitions"]
                    and len(client.call("get_map_info")["undo"]) == len(history) + 1,
                    f"{label}: apply creates one named undo step",
                )
                if render:
                    check(
                        client.render(output / f"{label}-applied.png", **render_args)
                        != baseline,
                        f"{label}: applied raised geometry remains visible after partition guides close",
                    )
                client.call("undo")
                check(
                    detail(client, [object_id]) == source,
                    f"{label}: undo restores exact source metadata and planes",
                )
                client.call("redo")
                check(
                    detail(client, [object_id]) == applied,
                    f"{label}: redo restores exact generated geometry",
                )
                client.call("save_map", {"path": str(path)})
                client.call("open_map", {"path": str(path), "discard_changes": True})
                check(
                    detail(client, [object_id]) == applied,
                    f"{label}: ordinary VMF save/reopen retains raised geometry and ownership",
                )
    client.call("new_map", {"discard_changes": True})
    ids = [
        client.call(
            "create_block",
            {
                "mins": [x, 0, 0],
                "maxs": [x + 64, 64, 64],
                "material": "dev/dev_measuregeneric01",
            },
        )["id"]
        for x in [0, 128]
    ]
    solids = [int(value.split(":")[1]) for value in ids]
    components = []
    pairs = []
    for index, solid in enumerate(solids):
        shape = inspect(solid)["brushes"][0]
        top = next(
            face
            for face in shape["faces"]
            if all(
                abs(shape["vertices"][vertex]["position"][2] - 64) < 1e-8
                for vertex in face["vertices"]
            )
        )
        selected = client.call(
            "select_brush_components",
            {
                "components": [{"solid": solid, "sides": [top["side"]]}],
                "snapshot": inspect()["snapshot"],
            },
        )
        components.append({"solid": solid, "sides": [top["side"]]})
        pairs.append(
            {
                "solid": solid,
                "edges": selected["crease_choices"][0]["pairs"][index]["edges"],
            }
        )
    selected = client.call(
        "select_brush_components",
        {"components": components, "snapshot": inspect()["snapshot"]},
    )
    source = detail(client, ids)
    history = client.call("get_map_info")["undo"]
    staged = client.call(
        "preview_component_partition",
        {"pairs": pairs, "snapshot": selected["snapshot"]},
    )
    check(
        len(staged["preview"]["partition"]["partitions"]) == 2
        and len(staged["preview"]["partition"]["selection"]) == 4,
        "Batch retains separate loop IDs in each source brush",
    )
    raised = client.call(
        "raise_component_partition",
        {"token": staged["preview"]["token"], "rise": 16.125},
    )
    check(
        len({change["new_side"] for change in raised["preview"]["crease"]["changes"]})
        == 2
        and detail(client, ids) == source,
        "Batch allocates distinct draft side IDs without changing either source",
    )
    client.call("commit_component_edit", {"token": raised["preview"]["token"]})
    check(
        len(client.call("get_map_info")["undo"]) == len(history) + 1,
        "Batch Apply creates one undo step",
    )
    client.call("undo")
    check(detail(client, ids) == source, "Batch undo restores both sources exactly")
    selected = client.call(
        "select_brush_components",
        {"components": components, "snapshot": inspect()["snapshot"]},
    )
    staged = client.call(
        "preview_component_partition",
        {"pairs": pairs, "snapshot": selected["snapshot"]},
    )
    token = staged["preview"]["token"]
    client.call("transform_objects", {"ids": [ids[1]], "translation": [0, 16, 0]})
    client.call("raise_component_partition", {"token": token, "rise": 16}, error=True)
    check(
        inspect()["preview"] is None and detail(client, [ids[0]])[0] == source[0],
        "Source edits invalidate the draft without changing unrelated brushes",
    )
    return checks


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-partition-validation")
    )
    parser.add_argument("--render", action="store_true")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    client = Client(None, args.binary, args.output)
    try:
        checks = validate(client, args.output, args.render)
        (args.output / "results.json").write_text(
            json.dumps(
                {"checks": len(checks), "passed": checks, "render": args.render},
                indent=2,
            )
            + "\n"
        )
        print(json.dumps({"checks": len(checks), "output": str(args.output)}))
    finally:
        client.close()


if __name__ == "__main__":
    main()
