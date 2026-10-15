import argparse
import json
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client, detail


def validate_grid_snap(client, check):
    client.call("new_map", {"discard_changes": True})
    client.call("set_spatial_settings", {"grid_size": 0.25, "snap": False})
    brush = client.call("create_block", {"mins": [-10.125, 3.0625, -0.375], "maxs": [5.375, 18.5625, 15.125], "material": "brick/test"})["id"]
    point = client.call("create_entity", {"classname": "info_target", "origin": [-32.125, -24.375, -16.0625]})["id"]
    ids = [brush, point]
    client.call("select", {"ids": ids})
    source = detail(client, ids)
    history = client.call("get_map_info")["undo"]
    for texture_lock in [False, True]:
        client.call("set_transform_tool", {"texture_lock": texture_lock})
        result = client.call("preview_snap_to_grid")
        snap = result["grid_snap"]
        draft = result["state"]["preview"]
        check(snap == {"grid": 0.25, "bounds_minimum": [-40.125, -32.375, -24.0625], "snapped_bounds_minimum": [-40.25, -32.5, -24.0], "delta": [-0.125, -0.125, 0.0625]}, "Grid snap names the world bounds-minimum anchor and rounds negative fractional coordinates")
        check(draft["grid_snap"] == snap and draft["translation"] == snap["delta"] and draft["texture_lock"] == texture_lock and draft["bounds"]["mins"] == snap["snapped_bounds_minimum"], "Grid snap exposes the normal transform draft with the shared texture policy")
        check(client.call("get_transform_handles")["state"]["preview"]["grid_snap"] == snap and "bounds minimum" in draft["display"]["summary"], "Agent and human transform inspection retain the named snap anchor")
        check(detail(client, ids) == source and client.call("get_map_info")["undo"] == history, "Snapping previews preserve source geometry and history")
        client.call("cancel_transform_preview")
        check(detail(client, ids) == source, "Cancelling grid snap preserves the mixed selection")
        draft = client.call("preview_snap_to_grid", {"ids": ids})["state"]["preview"]
        client.call("commit_transform_preview")
        check(detail(client, ids) == draft["objects"] and len(client.call("get_map_info")["undo"]) == len(history) + 1, "Applying grid snap matches its rigid mixed-selection draft in one edit")
        aligned = detail(client, ids)
        aligned_history = client.call("get_map_info")["undo"]
        check(client.call("preview_snap_to_grid")["grid_snap"]["delta"] == [0, 0, 0], "An aligned bounds minimum has zero snap delta")
        client.call("commit_transform_preview")
        check(detail(client, ids) == aligned and client.call("get_map_info")["undo"] == aligned_history, "Applying an aligned snap creates no history edit")
        client.call("undo")
        check(detail(client, ids) == source, "One Undo restores every snapped source object")
    client.call("set_spatial_settings", {"snap": True})
    client.call("transform_objects", {"ids": ids, "translation": [0.03125, 0, 0]})
    check(client.call("get_selection")["bounds"]["mins"] == [-40.09375, -32.375, -24.0625], "Typed transform offsets remain exact with automatic snapping enabled")
    source = detail(client, ids)
    history = client.call("get_map_info")["undo"]
    client.call("preview_snap_to_grid")
    reloaded = client.call("reload_model_assets")
    check(reloaded["snap_preview_cancelled"] and client.call("get_transform_handles")["state"]["preview"] is None, "Reloading model assets reports cancellation of the old snap anchor")
    client.call("commit_transform_preview", error=True)
    check(detail(client, ids) == source and client.call("get_map_info")["undo"] == history, "An asset reload cannot apply stale snap bounds or edit the map")
    draft = client.call("transform_objects", {"ids": ids, "translation": [0.03125, 0, 0], "preview": True})["state"]["preview"]
    reloaded = client.call("reload_model_assets")
    check(not reloaded["snap_preview_cancelled"] and client.call("get_transform_handles")["state"]["preview"] == draft, "Model reload retains ordinary exact transform drafts")
    client.call("cancel_transform_preview")


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
    validate_grid_snap(client, check)
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
