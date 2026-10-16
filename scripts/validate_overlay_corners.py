import argparse
import json
from pathlib import Path
from unittest.mock import patch

import validate_native_runtime as runtime
import validate_overlay_authoring as authoring
import validate_overlays as overlay_checks
from validate_authored_planes import records
from validate_compile_jobs import wait_job
from validate_face_lighting import DEFAULT_BINARY, Client, detail, face_ref
from validate_visibility import ROOM

CORNERS = [[-24.5, -16.25], [-20.0, 24.75], [28.5, 16.5], [32.25, -20.125]]
POINT_KEYS = {f"uv{index}" for index in range(4)}


def retained(entity):
    return [(key, value) for key, value in entity["properties"] if key.lower() not in POINT_KEYS]


def preview(client, overlay, corners=CORNERS, **changes):
    return client.call("preview_overlay_corners", {
        "snapshot": client.call("get_overlay_state")["snapshot"],
        "id": overlay, "corners": corners, **changes,
    })


def validate(client, output, render=False):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    for detail_owner in [False, True]:
        label = "detail" if detail_owner else "world"
        client.call("new_map", {"discard_changes": True})
        for mins, maxs in ROOM:
            client.call("create_block", {"mins": mins, "maxs": maxs, "material": "dev/dev_measuregeneric01"})
        plate = client.call("create_block", {"mins": [-64.125, -48.375, 64.625], "maxs": [63.875, 47.625, 80.625], "material": "dev/dev_measuregeneric01"})["id"]
        client.call("transform_objects", {"ids": [plate], "rotation": [17.125, -23.375, 8.625], "pivot": [0, 0, 64]})
        brush = detail(client, [plate])[0]
        source = face_ref(brush, max(brush["faces"], key=lambda face: face["normal"][2]))
        if detail_owner:
            client.call("tie_to_entity", {"ids": [plate], "classname": "func_detail"})
        state = authoring.preview(client, source)
        overlay = client.call("apply_overlay", {"token": state["draft"]["token"]})["created"]
        client.call("set_entity_properties", {"id": overlay, "set": {"StartU": "-00.2500", "RenderOrder": "02", "material": "Materials/Decals/decalgraffiti001a.VMT", "custom_option": "raw preserved"}})
        client.call("set_compile_settings", {"lighting": "fullbright", "physics": "none"})
        initial = client.call("get_overlay_state")
        check(initial["selected_corners"]["entity"] == overlay and initial["corners_error"] is None, f"{label}: inspect returns existing local corners and frame")
        before = detail(client, [overlay])[0]
        brushes = client.call("get_map_info")
        source_path = output / f"{label}-before.vmf"
        client.call("save_map", {"path": str(source_path)})
        source_bytes = source_path.read_bytes()
        baseline = output / f"{label}-baseline.bsp"
        client.call("compile_map", {"path": str(baseline)})
        initial = client.call("get_overlay_state")
        state = preview(client, overlay)
        token = state["draft"]["token"]
        check(state["draft"]["operation"] == "corners" and state["draft"]["local_corners"] == CORNERS and state["draft"]["source"] == source, f"{label}: explicit corner preview retains the source binding")
        check(state["draft"]["basis_origin"] == initial["selected_corners"]["basis_origin"] and state["draft"]["basis_u"] == initial["selected_corners"]["basis_u"] and state["draft"]["basis_v"] == initial["selected_corners"]["basis_v"], f"{label}: corner preview retains the authored frame")
        check(detail(client, [overlay])[0] == before and client.call("get_map_info")["undo"] == brushes["undo"], f"{label}: preview changes no source or history")
        if render:
            client.render(output / f"{label}-corners.png", width=1000, height=650, camera={"position": [180, -220, 220], "look_at": [0, 0, 80]})
        for invalid in [
            {"corners": [[0, 0]] * 4},
            {"corners": [[0, 0], [64, 64], [0, 64], [64, 0]]},
            {"corners": [[0, 0], [0, 64], [1e9, 64], [1e9, 0]]},
            {"corners": [[0, 0], [0, 64], [1e-8, 64], [1e-8, 0]]},
            {"corners": [[0, 0], [0, 64], [64, 64]]},
            {"source": source}, {"material": "decals/other"}, {"BasisU": [1, 0, 0]},
            {"snapshot": initial["snapshot"]},
        ]:
            client.call("preview_overlay_corners", {"snapshot": state["snapshot"], "id": overlay, "corners": CORNERS, **invalid}, error=True)
            check(client.call("get_overlay_state")["draft"] == state["draft"], f"{label}: rejected corner request preserves draft: {invalid}")
        pending = output / f"{label}-pending.bsp"
        client.call("compile_map", {"path": str(pending)})
        check(pending.read_bytes() == baseline.read_bytes() and client.call("get_overlay_state")["draft"]["valid"], f"{label}: pending corners do not affect compilation or stale the draft")
        client.call("cancel_overlay", {"token": token})
        check(detail(client, [overlay])[0] == before, f"{label}: cancellation preserves the exact overlay")
        state = preview(client, overlay)
        reviewed = client.call("review_overlay", {"token": state["draft"]["token"]})
        client.call("apply_overlay", {"token": state["draft"]["token"]}, error=True)
        client.call("cancel_overlay", {"token": token}, error=True)
        result = client.call("apply_overlay", {"token": reviewed["draft"]["token"]})
        after = detail(client, [overlay])[0]
        info = client.call("get_map_info")
        check(result["operation"] == "corners" and result["updated"] == overlay and "created" not in result, f"{label}: reviewed Apply updates the same entity")
        check(retained(before) == retained(after), f"{label}: raw binding, origin, basis, settings and custom properties are preserved")
        check(info["point_entities"] == brushes["point_entities"] and info["selection"] == [overlay] and info["undo"][0] == "Edit overlay corners" and len(info["undo"]) == len(brushes["undo"]) + 1, f"{label}: Apply creates one Undo step and no entity")
        edited = output / f"{label}-edited.bsp"
        client.call("compile_map", {"path": str(edited)})
        old_record = records(baseline.read_bytes(), 45, "<ihH64i22f")[0]
        new_record = records(edited.read_bytes(), 45, "<ihH64i22f")[0]
        point_slots = {71 + 3 * index + axis for index in range(4) for axis in range(2)}
        check(all(new_record[index] == value for index, value in enumerate(old_record) if index not in point_slots) and [[new_record[71 + 3 * index], new_record[72 + 3 * index]] for index in range(4)] == CORNERS, f"{label}: native records change only the four XY corner pairs")
        check(records(edited.read_bytes(), 60, "<2f") == records(baseline.read_bytes(), 60, "<2f"), f"{label}: native fades remain exact")
        background = output / f"{label}-background.bsp"
        client.call("start_compile", {"path": str(background)})
        job = wait_job(client)
        check(job["status"] == "completed" and background.read_bytes() == edited.read_bytes(), f"{label}: synchronous and controlled builds agree")
        check(source_path.read_bytes() == source_bytes, f"{label}: unapplied disk save is preserved")
        client.call("undo")
        check(detail(client, [overlay])[0] == before, f"{label}: Undo restores the exact previous projection")
        client.call("redo")
        check(detail(client, [overlay])[0] == after, f"{label}: Redo restores edited corners")
        saved = output / f"{label}-edited.vmf"
        client.call("save_map", {"path": str(saved)})
        client.call("open_map", {"path": str(saved), "discard_changes": True})
        client.call("select", {"ids": [overlay]})
        check(client.call("get_overlay_state")["selected_corners"]["local_corners"] == CORNERS, f"{label}: corners survive save/reopen in the original frame")
        stale = preview(client, overlay)
        owner = client.call("get_map_info")["selection"][0]
        client.call("set_entity_properties", {"id": owner, "set": {"custom_option": "changed after preview"}})
        client.call("apply_overlay", {"token": stale["draft"]["token"]}, error=True)
        client.call("review_overlay", {"token": stale["draft"]["token"]}, error=True)
        check(not client.call("get_overlay_state")["draft"]["valid"], f"{label}: source revision invalidates Apply and Review")
        client.call("cancel_overlay", {"token": stale["draft"]["token"]})
        check(client.call("get_overlay_state")["draft"] is None, f"{label}: owned stale cancellation succeeds")
    return checks


def native_fixture(client, output):
    cases = authoring.native_fixture(client, output)
    selected = client.call("get_overlay_state")["selected_corners"]
    overlay = selected["entity"]
    client.call("set_entity_properties", {"id": overlay, "set": {"fademindist": "-1", "fademaxdist": "0"}})
    corners = [[u * 0.5, v * 0.5] for u, v in selected["local_corners"]]
    state = preview(client, overlay, corners)
    client.call("apply_overlay", {"token": state["draft"]["token"]})
    path = output / "native-corners.bsp"
    client.call("compile_map", {"path": str(path)})
    return cases + [("corners", path)]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, default=Path("/tmp/hammerr-overlay-corners"))
    parser.add_argument("--render", action="store_true")
    parser.add_argument("--runtime", action="store_true")
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--bsp", type=Path, help=argparse.SUPPRESS)
    parser.add_argument("--game-dir", type=Path, default=Path.home() / ".local/share/Steam/steamapps/common/Counter-Strike Source")
    args = parser.parse_args()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    if args.worker:
        overlay_checks.worker(args)
        return
    with patch.dict("os.environ", {"HAMMERR_RECENT_MAPS_DIR": str(args.output / "recent"), "HAMMERR_BUILD_RECIPES": str(args.output / "recipes.json")}):
        client = Client(None, args.binary, args.output)
    try:
        checks = validate(client, args.output, args.render)
        cases = native_fixture(client, args.output) if args.runtime else []
        (args.output / "results.json").write_text(json.dumps({"checks": len(checks), "passed": checks}, indent=2) + "\n")
        print(json.dumps({"checks": len(checks), "output": str(args.output)}))
    finally:
        client.close()
    if args.runtime:
        reports = {}
        for name, bsp in cases:
            output = runtime.private_display(Path(__file__), args, bsp, output=args.output / f"native-{name}")
            reports[name] = json.loads((output / "render.json").read_text())
        overlay_checks.check_rendering(reports)
        assert reports["baseline"]["dark_floor_pixels"] + 1000 < reports["corners"]["dark_floor_pixels"] < reports["visible"]["dark_floor_pixels"] - 1000, reports
        assert reports["corners"]["accessor_ids"] and set(reports["corners"]["accessor_ids"]) == {0}, reports
        (args.output / "render-results.json").write_text(json.dumps(reports, indent=2) + "\n")
        print(json.dumps({"native_edited_corners": True, "insecure": True, "sv_lan": True}))


if __name__ == "__main__":
    main()
