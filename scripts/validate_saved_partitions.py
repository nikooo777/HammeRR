import argparse
import json
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client, detail


def validate(client, output, render=False):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def inspect():
        return client.call("get_brush_components")

    def saved():
        return client.call("get_saved_component_partitions")

    def resume(sources):
        return client.call(
            "resume_component_partitions",
            {"sources": sources, "snapshot": saved()["snapshot"]},
        )

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

    tools = {tool["name"] for tool in client.rpc("tools/list", {})["tools"]}
    check(
        {
            "get_saved_component_partitions",
            "store_component_partition",
            "resume_component_partitions",
            "remove_saved_component_partitions",
        }
        <= tools,
        "All saved-partition operations are discoverable",
    )
    for owned in [False, True]:
        for oblique in [False, True]:
            for direction in range(2):
                label = f"owned={owned} oblique={oblique} direction={direction}"
                client.call("new_map", {"discard_changes": True})
                object_id = client.call(
                    "create_block",
                    {"mins": [0.125, -7.25, 16.5], "maxs": [64.875, 48.125, 80.25]},
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
                source = detail(client, [object_id])
                brush = source[0]["brushes"][0] if owned else source[0]
                face = max(brush["faces"], key=lambda face: face["normal"][2])
                sources = [{"solid": solid, "side": face["side"]}]
                selected = client.call(
                    "select_brush_components",
                    {
                        "components": [{"solid": solid, "sides": [face["side"]]}],
                        "snapshot": inspect()["snapshot"],
                    },
                )
                pairs = [
                    {
                        "solid": solid,
                        "edges": selected["crease_choices"][0]["pairs"][direction][
                            "edges"
                        ],
                    }
                ]
                history = client.call("get_map_info")["undo"]
                client.call(
                    "set_component_picking",
                    {
                        "enabled": True,
                        "kind": "face",
                        "solids": [solid],
                        "snapshot": inspect()["snapshot"],
                    },
                )
                draft = client.call(
                    "preview_component_partition",
                    {"pairs": pairs, "snapshot": inspect()["snapshot"]},
                )["preview"]
                identity = membership(draft["partition"]["partitions"][0])
                stored = client.call(
                    "store_component_partition", {"token": draft["token"]}
                )
                check(stored["stored"] == 1, f"{label}: flat draft kept")
                check(inspect()["preview"] is None, f"{label}: Keep closes the draft")
                render_args = {
                    "surface": "flat",
                    "camera": {"position": [170, -200, 180], "look_at": [30, 20, 45]},
                }
                kept_view = (
                    client.render(
                        output / f"kept-{int(owned)}-{int(oblique)}-{direction}.png",
                        **render_args,
                    )
                    if render
                    else None
                )
                check(
                    detail(client, [object_id]) == source,
                    f"{label}: authored brush untouched",
                )
                check(
                    len(client.call("get_map_info")["undo"]) == len(history) + 1,
                    f"{label}: Keep is one undo step",
                )
                client.call("undo")
                check(not saved()["entries"], f"{label}: exact metadata undo")
                client.call("redo")
                check(saved()["entries"][0]["valid"], f"{label}: metadata redo")
                path = output / f"saved-{int(owned)}-{int(oblique)}-{direction}.vmf"
                client.call("save_map", {"path": str(path)})
                check(
                    "hammerr_partitions" in path.read_text(),
                    f"{label}: intent exported",
                )
                client.call("open_map", {"path": str(path), "discard_changes": True})
                check(
                    saved()["entries"][0]["valid"],
                    f"{label}: intent resolves after reopen",
                )
                restored = resume(sources)["preview"]
                check(
                    membership(restored["partition"]["partitions"][0]) == identity,
                    f"{label}: persistent piece identity",
                )
                saved_bytes = path.read_bytes()
                client.call("save_map", {"path": str(path)}, error=True)
                check(
                    path.read_bytes() == saved_bytes,
                    f"{label}: pending resume does not replace the saved file",
                )
                client.call(
                    "remove_saved_component_partitions",
                    {"sources": sources, "snapshot": saved()["snapshot"]},
                    error=True,
                )
                client.call("cancel_component_edit", {"token": restored["token"]})
                check(
                    saved()["entries"][0]["valid"],
                    f"{label}: Cancel retains saved intent",
                )
                if render:
                    cancelled_view = client.render(
                        output
                        / f"cancelled-{int(owned)}-{int(oblique)}-{direction}.png",
                        **render_args,
                    )
                    check(
                        cancelled_view == kept_view,
                        f"{label}: scoped saved guides survive reopen and resume cancellation",
                    )
                stale = saved()["snapshot"]
                client.call(
                    "transform_objects",
                    {
                        "ids": [object_id],
                        "reflection": [True, False, False],
                        "pivot": [0, 0, 0],
                    },
                )
                client.call(
                    "resume_component_partitions",
                    {"sources": sources, "snapshot": stale},
                    error=True,
                )
                check(
                    inspect()["preview"] is None,
                    f"{label}: stale snapshot rejects atomically",
                )
                reflected = resume(sources)["preview"]
                check(
                    membership(reflected["partition"]["partitions"][0]) == identity,
                    f"{label}: reflection retains piece membership",
                )
                client.call("store_component_partition", {"token": reflected["token"]})
                check(
                    inspect()["preview"] is None,
                    f"{label}: unchanged Keep still closes the draft",
                )
                reflected = resume(sources)["preview"]
                check(
                    membership(reflected["partition"]["partitions"][0]) == identity,
                    f"{label}: repeated Keep retains reflected piece identity",
                )
                raised = client.call(
                    "raise_component_partition",
                    {
                        "token": reflected["token"],
                        "rise": 16.125,
                        "roof_texture_lock": True,
                    },
                )["preview"]
                client.call(
                    "store_component_partition", {"token": raised["token"]}, error=True
                )
                check(
                    saved()["entries"][0]["valid"],
                    f"{label}: raising leaves saved source pending",
                )
                client.call("commit_component_edit", {"token": raised["token"]})
                check(
                    not saved()["entries"], f"{label}: Apply consumes the flat intent"
                )
                client.call("undo")
                check(
                    saved()["entries"][0]["valid"],
                    f"{label}: undo restores geometry and intent",
                )
                client.call("redo")
                check(
                    not saved()["entries"],
                    f"{label}: redo consumes geometry and intent together",
                )
                client.call("undo")
                client.call("delete_objects", {"ids": [object_id]})
                report = saved()
                check(
                    not report["entries"][0]["valid"] and report["entries"][0]["error"],
                    f"{label}: deleted source remains diagnosable",
                )
                client.call(
                    "resume_component_partitions",
                    {"sources": sources, "snapshot": report["snapshot"]},
                    error=True,
                )
                check(
                    inspect()["preview"] is None,
                    f"{label}: invalid resume preserves state",
                )
                client.call(
                    "remove_saved_component_partitions",
                    {"sources": sources, "snapshot": saved()["snapshot"]},
                )
                check(
                    not saved()["entries"],
                    f"{label}: stale source record can be removed",
                )
                client.call("undo")
                check(
                    not saved()["entries"][0]["valid"],
                    f"{label}: removal undo restores stale record",
                )
    client.call("new_map", {"discard_changes": True})
    sources = []
    for offset in [0, 128]:
        object_id = client.call(
            "create_block", {"mins": [offset, 0, 0], "maxs": [offset + 64, 64, 64]}
        )["id"]
        solid = int(object_id.split(":")[1])
        face = max(
            detail(client, [object_id])[0]["faces"], key=lambda face: face["normal"][2]
        )
        sources.append({"solid": solid, "side": face["side"]})
    selected = client.call(
        "select_brush_components",
        {
            "components": [
                {"solid": source["solid"], "sides": [source["side"]]}
                for source in sources
            ],
            "snapshot": inspect()["snapshot"],
        },
    )
    pairs = [
        {"solid": choice["component"]["solid"], "edges": choice["pairs"][0]["edges"]}
        for choice in selected["crease_choices"]
    ]
    token = client.call(
        "preview_component_partition",
        {"pairs": pairs, "snapshot": inspect()["snapshot"]},
    )["preview"]["token"]
    stored = client.call("store_component_partition", {"token": token})
    check(
        stored["stored"] == 2 and len(saved()["entries"]) == 2,
        "Batch Keep stores both sources",
    )
    before = saved()
    for name, incoming in [
        ("resume_component_partitions", [sources[0]] * 17),
        ("remove_saved_component_partitions", [sources[0]] * 65),
        (
            "remove_saved_component_partitions",
            [sources[0], {"solid": 9999, "side": 9999}],
        ),
        ("resume_component_partitions", [sources[0], {"solid": 9999, "side": 9999}]),
    ]:
        client.call(
            name, {"sources": incoming, "snapshot": before["snapshot"]}, error=True
        )
        check(
            saved() == before and inspect()["preview"] is None,
            f"{name}: raw bounds or missing batch source reject atomically",
        )
    restored = resume(sources)["preview"]
    check(
        len(restored["partition"]["partitions"]) == 2,
        "Batch resume restores both sources",
    )
    client.call("cancel_component_edit", {"token": restored["token"]})
    history = client.call("get_map_info")["undo"]
    removed = client.call(
        "remove_saved_component_partitions",
        {"sources": sources, "snapshot": saved()["snapshot"]},
    )
    check(
        removed["removed"] == 2 and not saved()["entries"],
        "Batch removal clears both records",
    )
    check(
        len(client.call("get_map_info")["undo"]) == len(history) + 1,
        "Batch removal adds one undo step",
    )
    client.call("undo")
    check(len(saved()["entries"]) == 2, "Batch removal undo restores both records")
    return checks


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-saved-partitions-validation")
    )
    parser.add_argument("--render", action="store_true")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    client = Client(None, args.binary, args.output)
    try:
        checks = validate(client, args.output, args.render)
        (args.output / "results.json").write_text(
            json.dumps({"checks": len(checks), "passed": checks}, indent=2) + "\n"
        )
        print(json.dumps({"checks": len(checks), "output": str(args.output)}))
    finally:
        client.close()


if __name__ == "__main__":
    main()
