import argparse
import json
import math
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client


def state(client):
    return client.call("get_measurements")


def action(client, action_name, **arguments):
    return client.call("set_measurement", {"action": action_name, **arguments})


def token(client):
    return state(client)["draft"]["token"]


def preview(client, saved=None):
    report = state(client)
    measurement = saved or {
        "name": "Entity origin clearance", "start": [-1.125, 2.25, 3.5],
        "end": [128.25, 64.5, 96.75],
    }
    return action(
        client, "preview", id=measurement.get("id"), name=measurement["name"],
        start=measurement.get("start") or measurement["stored_start"],
        end=measurement.get("end") or measurement["stored_end"],
        snapshot=report["snapshot"],
    )


def reject(client, name, **arguments):
    before = state(client)
    client.call("set_measurement", {"action": name, **arguments}, error=True)
    assert state(client) == before


def metadata_bytes(path):
    data = path.read_bytes()
    return data[data.index(b"hammerr_measurements"):]


def validate(client, output):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    client.call("new_map", {"discard_changes": True})
    first_point = [16.125, -32.25, 64.5]
    first = client.call("create_entity", {"classname": "logic_relay", "origin": first_point})["id"]
    second = client.call("create_entity", {"classname": "logic_relay", "origin": [128.25, 64.5, 96.75]})["id"]
    brush = client.call("create_block", {"mins": [-64, -64, -16], "maxs": [64, 64, 0]})["id"]
    history = client.call("get_map_info")["undo"]
    preview(client)
    reject(client, "attach_origin", token=token(client), endpoint="start", object=brush)
    action(client, "attach_origin", token=token(client), endpoint="start", object=first)
    action(client, "attach_origin", token=token(client), endpoint="end", object=second)
    expected = state(client)["draft"]["measurement"]
    check(expected["start"] == first_point, "Origin attachment retains exact authored coordinates")
    reject(client, "attach_origin", token=token(client), endpoint="end", object=first)
    check(client.call("get_map_info")["undo"] == history, "Attachment previews leave map history unchanged")
    action(client, "apply", token=token(client))
    check(len(client.call("get_map_info")["undo"]) == len(history) + 1, "Apply records both origin references in one Undo step")
    source = output / "attached.vmf"
    client.call("save_map", {"path": str(source)})
    reference_bytes = metadata_bytes(source)
    client.call("open_map", {"path": str(source), "discard_changes": True})
    check(state(client)["measurements"] == [expected], "Save and reopen retain references and exact points")
    for arguments in [
        {"translation": [3.125, 5.25, 7.5]},
        {"rotation": [0, 0, 90], "pivot": [0, 0, 0]},
        {"scale": [2, 0.5, 1.5], "pivot": [8, 8, 8]},
    ]:
        before = state(client)["measurements"]
        count = len(client.call("get_map_info")["undo"])
        client.call("transform_objects", {"ids": [first], **arguments})
        authored = dict(client.call("get_objects", {"ids": [first]})["objects"][0]["properties"])
        point = [float(value) for value in authored["origin"].split()]
        actual = state(client)["measurements"][0]["start"]
        check(all(math.isclose(a, b, abs_tol=1e-12) for a, b in zip(point, actual, strict=True)), f"Origin follows transform {arguments}")
        check(len(client.call("get_map_info")["undo"]) == count + 1, "Following a transform adds no separate ruler history step")
        transformed = state(client)["measurements"]
        client.call("save_map", {"path": str(output / "transformed.vmf")})
        check(metadata_bytes(output / "transformed.vmf") == reference_bytes, "Transforms retain measurement metadata bytes")
        client.call("undo")
        check(state(client)["measurements"] == before, "Undo restores the exact origin-resolved ruler")
        client.call("redo")
        check(state(client)["measurements"] == transformed, "Redo restores the exact transformed ruler")
    duplicate = client.call("duplicate_objects", {"ids": [first], "offset": [100, 0, 0]})["copies"][0]["id"]
    check(state(client)["measurements"][0]["attachments"]["start"]["object"] == first, "Duplicate leaves the reference on its original entity")
    client.call("delete_objects", {"ids": [first]})
    missing = state(client)["measurements"][0]
    check(not missing["available"] and missing["start"] is None and missing["distance"] is None, "Deletion makes the ruler unavailable without a frozen fallback")
    check(client.call("get_objects", {"ids": [duplicate]})["objects"][0]["id"] == duplicate, "The duplicate cannot substitute for the deleted original")
    client.call("save_map", {"path": str(output / "missing.vmf")})
    check(metadata_bytes(output / "missing.vmf") == reference_bytes, "Missing references remain persisted without metadata changes")
    client.call("undo")
    check(state(client)["measurements"][0]["available"], "Undo deletion restores origin availability")
    client.call("redo")
    preview(client, missing)
    reject(client, "detach_origin", token=token(client), endpoint="start")
    reject(client, "apply", token=token(client))
    reject(client, "frame", token=token(client), snapshot=state(client)["snapshot"])
    action(client, "world_endpoint", token=token(client), endpoint="start", point=[-8.125, 2.25, 4.5])
    action(client, "cancel", token=token(client))
    check(state(client)["measurements"] == [missing], "Cancel discards explicit world repair and retains the missing reference")
    preview(client, missing)
    action(client, "world_endpoint", token=token(client), endpoint="start", point=[-8.125, 2.25, 4.5])
    count = len(client.call("get_map_info")["undo"])
    action(client, "apply", token=token(client))
    repaired = state(client)["measurements"][0]
    check(repaired["start"] == [-8.125, 2.25, 4.5] and repaired["attachments"]["start"] is None, "Explicit world replacement repairs only its chosen endpoint")
    check(repaired["attachments"]["end"]["object"] == second, "Repair retains the other endpoint origin reference")
    check(len(client.call("get_map_info")["undo"]) == count + 1, "World repair Apply adds one Undo step")
    client.call("undo")
    check(state(client)["measurements"] == [missing], "Undo repair restores the exact unavailable reference")
    return checks


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, default=Path("/tmp/hammerr-measurement-origins"))
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    client = Client(None, args.binary, args.output)
    try:
        checks = validate(client, args.output)
        (args.output / "validation.json").write_text(json.dumps(checks, indent=2) + "\n")
    finally:
        client.close()
    print(json.dumps({"measurement_origins": len(checks), "output": str(args.output)}))


if __name__ == "__main__":
    main()
