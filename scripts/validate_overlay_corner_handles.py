import argparse
import json
from pathlib import Path
from unittest.mock import patch

from validate_face_lighting import DEFAULT_BINARY, Client, detail, face_ref
from validate_overlay_authoring import preview as preview_overlay
from validate_overlay_corners import CORNERS, retained

WIDTH, HEIGHT = 1000, 760
CAMERA = {"position": [180, -220, 220], "look_at": [0, 0, 80]}


def inspect(client):
    return client.call("overlay_corner_handles", {"action": "inspect"})


def action(client, name, **arguments):
    return client.call("overlay_corner_handles", {"action": name, **arguments})


def begin(client):
    report = inspect(client)
    return action(
        client, "begin", token=report["draft"]["token"],
        snapshot=report["handle_snapshot"], width=WIDTH, height=HEIGHT, camera=CAMERA,
    )


def pointer(client, name, report, point, **arguments):
    return action(
        client, name, token=report["draft"]["token"],
        x=point[0], y=point[1], width=WIDTH, height=HEIGHT, **arguments,
    )


def reject(client, name, **arguments):
    before = inspect(client)
    client.call("overlay_corner_handles", {"action": name, **arguments}, error=True)
    assert inspect(client) == before


def validate(client, output, render):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    for is_detail in [False, True]:
        label = "detail" if is_detail else "world"
        client.call("new_map", {"discard_changes": True})
        plate = client.call("create_block", {
            "mins": [-64.125, -48.375, 64.625], "maxs": [63.875, 47.625, 80.625],
            "material": "dev/dev_measuregeneric01",
        })["id"]
        client.call("transform_objects", {
            "ids": [plate], "rotation": [17.125, -23.375, 8.625], "pivot": [0, 0, 64],
        })
        brush = detail(client, [plate])[0]
        source = face_ref(brush, max(brush["faces"], key=lambda face: face["normal"][2]))
        if is_detail:
            client.call("tie_to_entity", {"ids": [plate], "classname": "func_detail"})
        draft = preview_overlay(client, source)
        overlay = client.call("apply_overlay", {"token": draft["draft"]["token"]})["created"]
        properties = dict(detail(client, [overlay])[0]["properties"])
        uv0 = properties["uv0"].split()
        client.call("set_entity_properties", {"id": overlay, "set": {
            "StartU": "-00.2500", "RenderOrder": "02", "custom_option": "raw preserved",
            "uv0": f"{uv0[0]} {uv0[1]} 17.125",
        }})
        client.call("set_view", {"surface": "flat"})
        initial = client.call("preview_overlay_corners", {
            "id": overlay, "corners": CORNERS, "snapshot": client.call("get_overlay_state")["snapshot"],
        })
        before = detail(client, [overlay])[0]
        history = client.call("get_history")["undo_count"]
        original_camera = client.call("get_camera")
        source_path = output / f"{label}-before.vmf"
        client.call("save_map", {"path": str(source_path)})
        original_bytes = source_path.read_bytes()
        draft = begin(client)
        check(draft["draft"]["handles"]["valid"] and draft["draft"]["handles"]["snapping"] is False, f"{label}: typed shared viewport handles capture a valid unsnapped plane")
        reject(client, "pick", token=draft["draft"]["token"], x=1, y=1, width=WIDTH, height=HEIGHT)
        client.call("apply_overlay", {"token": draft["draft"]["token"]}, error=True)
        start = draft["draft"]["handles"]["pixels"][0]
        start = [start[0] + 2.25, start[1] - 1.5]
        draft = pointer(client, "pick", draft, start)
        draft = pointer(client, "move", draft, start)
        check(draft["draft"]["local_corners"] == CORNERS, f"{label}: offset picking and a stationary pointer never jump the corner")
        old_token = draft["draft"]["token"]
        end = [start[0] - 12.5, start[1] + 7.75]
        draft = pointer(client, "move", draft, end)
        check(draft["draft"]["local_corners"] != CORNERS and draft["draft"]["local_corners"][1:] == CORNERS[1:], f"{label}: only the picked corner moves")
        reject(client, "cancel", token=old_token)
        opposite = draft["draft"]["handles"]["pixels"][2]
        invalid = [opposite[axis] + (opposite[axis] - start[axis]) * 0.25 for axis in [0, 1]]
        reject(client, "move", token=draft["draft"]["token"], x=invalid[0], y=invalid[1], width=WIDTH, height=HEIGHT)
        reject(client, "move", token=draft["draft"]["token"], x=end[0], y=end[1], width=WIDTH + 1, height=HEIGHT)
        check(detail(client, [overlay])[0] == before and client.call("get_history")["undo_count"] == history and source_path.read_bytes() == original_bytes and client.call("get_camera") == original_camera, f"{label}: pending drag changes no map, history, saved bytes or shared camera")
        if render:
            client.render(output / f"{label}-corner-handles.png", width=WIDTH, height=HEIGHT, camera=CAMERA)
            check(inspect(client)["draft"]["handles"]["valid"], f"{label}: offscreen render retains the captured drag")
        cancelled = action(client, "cancel", token=draft["draft"]["token"])
        check(cancelled["draft"]["local_corners"] == initial["draft"]["local_corners"] and cancelled["draft"]["handles"] is None, f"{label}: Cancel handles restores the exact pre-handle draft")
        draft = begin(client)
        start = draft["draft"]["handles"]["pixels"][0]
        draft = pointer(client, "pick", draft, start)
        draft = pointer(client, "move", draft, [start[0] - 12.5, start[1] + 7.75])
        final = action(client, "finish", token=draft["draft"]["token"])
        check(final["draft"]["handles"] is None and final["draft"]["local_corners"] == draft["draft"]["local_corners"], f"{label}: release returns the valid result to ordinary Apply review")
        applied = client.call("apply_overlay", {"token": final["draft"]["token"]})
        after = detail(client, [overlay])[0]
        check(applied["updated"] == overlay and applied["operation"] == "corners" and retained(after) == retained(before) and dict(after["properties"])["uv0"].split()[2] == "17.125" and client.call("get_history")["undo_count"] == history + 1, f"{label}: Apply preserves raw settings and ignored Z in one Undo")
        client.call("undo")
        check(detail(client, [overlay])[0] == before, f"{label}: Undo exactly restores the source entity")
        client.call("redo")
        check(detail(client, [overlay])[0] == after, f"{label}: Redo exactly restores the applied corner")
        draft = client.call("preview_overlay_corners", {
            "id": overlay, "corners": CORNERS, "snapshot": client.call("get_overlay_state")["snapshot"],
        })
        draft = begin(client)
        start = draft["draft"]["handles"]["pixels"][0]
        draft = pointer(client, "pick", draft, start)
        client.call("set_camera", {"position": [256, -256, 256], "look_at": [0, 0, 80]})
        reject(client, "move", token=draft["draft"]["token"], x=start[0], y=start[1], width=WIDTH, height=HEIGHT)
        reject(client, "finish", token=draft["draft"]["token"])
        cancelled = action(client, "cancel", token=draft["draft"]["token"])
        check(cancelled["draft"]["local_corners"] == CORNERS and cancelled["draft"]["handles"] is None, f"{label}: stale camera rejects motion but owned cancellation restores the draft")
        client.call("cancel_overlay", {"token": cancelled["draft"]["token"]})
    return checks


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, default=Path("/tmp/hammerr-overlay-corner-handles"))
    parser.add_argument("--render", action="store_true")
    args = parser.parse_args()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    with patch.dict("os.environ", {"HAMMERR_RECENT_MAPS_DIR": str(args.output / "recent"), "HAMMERR_BUILD_RECIPES": str(args.output / "recipes.json")}):
        client = Client(None, args.binary, args.output)
    try:
        checks = validate(client, args.output, args.render)
        report = {"checks": len(checks), "passed": checks, "native_exercised": False}
        (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps({"checks": len(checks), "output": str(args.output)}))
    finally:
        client.close()


if __name__ == "__main__":
    main()
