import argparse
import json
from pathlib import Path
from unittest.mock import patch

import validate_native_runtime as runtime
import validate_overlays as overlay_checks
from validate_authored_planes import records
from validate_compile_jobs import wait_job
from validate_face_lighting import DEFAULT_BINARY, Client, detail, face_ref
from validate_visibility import ROOM


def preview(client, source, **changes):
    args = {
        "snapshot": client.call("get_overlay_state")["snapshot"],
        "source": source,
        "material": "decals/decalgraffiti001a",
        "targetname": "authored_overlay",
        "render_order": 2,
        "u": [-0.25, 1.75],
        "v": [0.125, 0.875],
        "fade_min": 64,
        "fade_max": 512,
        **changes,
    }
    return client.call("preview_overlay", args)


def validate(client, output, render=False):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    room_brushes = []
    for mins, maxs in ROOM:
        room_brushes.append(client.call("create_block", {"mins": mins, "maxs": maxs, "material": "dev/dev_measuregeneric01"})["id"])
    client.call("create_entity", {"classname": "info_player_terrorist", "origin": [-96, -96, 8]})
    client.call("create_entity", {"classname": "info_player_counterterrorist", "origin": [96, -96, 8]})
    plate = client.call("create_block", {
        "mins": [-64.125, -48.375, 64.625], "maxs": [63.875, 47.625, 80.625],
        "material": "dev/dev_measuregeneric01",
    })["id"]
    client.call("transform_objects", {"ids": [plate], "rotation": [17.125, -23.375, 8.625], "pivot": [0, 0, 64]})
    brush = detail(client, [plate])[0]
    top = max(brush["faces"], key=lambda face: face["normal"][2])
    source = face_ref(brush, top)
    client.call("select_faces", {"faces": [source]})
    client.call("set_compile_settings", {"lighting": "fullbright", "physics": "none"})
    initial = client.call("get_overlay_state")
    check(initial["selected_source"]["solid"] == source["solid"] and initial["selected_source"]["side"] == source["side"], "Shared eligibility identifies the explicit tilted quad source")
    check(initial["selection_error"] is None, "A visible tilted world quad is eligible")
    source_path = output / "before.vmf"
    client.call("save_map", {"path": str(source_path)})
    source_bytes = source_path.read_bytes()
    before = detail(client, [plate])
    info = client.call("get_map_info")
    baseline_path = output / "baseline.bsp"
    client.call("compile_map", {"path": str(baseline_path)})
    state = preview(client, source)
    token = state["draft"]["token"]
    check(state["draft"]["valid"] and state["draft"]["owner"] == "agent", "Preview creates a valid owned draft")
    check(abs(state["draft"]["basis_normal"][2]) < 0.99 and len(state["draft"]["corners"]) == 4, "The checked core definition fits the tilted source plane")
    check(detail(client, [plate]) == before and client.call("get_map_info")["undo"] == info["undo"] and client.call("get_map_info")["selection"] == info["selection"], "Preview preserves source geometry, selection and history")
    check(source_path.read_bytes() == source_bytes, "An unapplied draft leaves the saved VMF untouched")
    uncommitted = output / "uncommitted.bsp"
    client.call("compile_map", {"path": str(uncommitted)})
    check(uncommitted.read_bytes() == baseline_path.read_bytes(), "Unapplied overlay drafts do not affect compilation")
    if render:
        client.render(output / "overlay-outline.png", width=1000, height=650, camera={"position": [180, -220, 220], "look_at": [0, 0, 80]})
    for change in [{"material": "../outside"}, {"u": [1, 1]}, {"render_order": 4}, {"source": {"solid": source["solid"], "side": 4_000_000_000}}]:
        invalid = {
            "snapshot": client.call("get_overlay_state")["snapshot"],
            "source": source, "material": "decals/decalgraffiti001a", **change,
        }
        client.call("preview_overlay", invalid, error=True)
        check(client.call("get_overlay_state")["draft"] == state["draft"], f"Invalid preview preserves the valid draft: {next(iter(change))}")
    client.call("preview_overlay", {"snapshot": initial["snapshot"], "source": source, "material": "decals/decalgraffiti001a"}, error=True)
    replacement = preview(client, source)
    client.call("cancel_overlay", {"token": token}, error=True)
    check(client.call("get_overlay_state")["draft"]["token"] == replacement["draft"]["token"], "Stale cancellation preserves the replacement draft")
    reviewed = client.call("review_overlay", {"token": replacement["draft"]["token"]})
    check(reviewed["draft"]["token"] != replacement["draft"]["token"], "Explicit Review issues a fresh ownership token")
    client.call("cancel_overlay", {"token": reviewed["draft"]["token"]})
    check(client.call("get_overlay_state")["draft"] is None and detail(client, [plate]) == before, "Cancel discards the outline without editing the source")
    state = preview(client, source)
    result = client.call("apply_overlay", {"token": state["draft"]["token"]})
    overlay = result["created"]
    applied = client.call("get_map_info")
    check(applied["selection"] == [overlay] and applied["undo"][0] == "Create overlay", "Apply selects the new overlay in one Undo step")
    check(detail(client, [plate]) == before, "Apply leaves the source brush unchanged")
    source_path = output / "authored.vmf"
    client.call("save_map", {"path": str(source_path)})
    saved = source_path.read_bytes()
    client.call("undo")
    check(client.call("get_map_info")["undo"] == info["undo"], "Undo removes exactly the overlay creation step")
    client.call("redo")
    check(client.call("get_map_info")["selection"] == [], "Redo does not recreate a stale selection")
    client.call("set_entity_properties", {"id": overlay, "set": {"BasisU": "1 0 0"}}, error=True)
    check(source_path.read_bytes() == saved, "Generic derived-field editing remains read-only")
    path = output / "authored.bsp"
    built = client.call("compile_map", {"path": str(path)})
    rows = records(path.read_bytes(), 45, "<ihH64i22f")
    fades = records(path.read_bytes(), 60, "<2f")
    check(built["stats"]["overlays"] == len(rows) == 1 and rows[0][2] >> 14 == 2, "The authored definition emits a native overlay with its render order")
    check(rows[0][67:71] == (-0.25, 1.75, 0.125, 0.875) and fades == [(4096, 262144)], "Native output preserves U/V ranges and squared fade distances")
    background = output / "authored-background.bsp"
    client.call("start_compile", {"path": str(background)})
    job = wait_job(client)
    check(job["status"] == "completed" and background.read_bytes() == path.read_bytes(), "Synchronous and background overlay authoring builds are byte-identical")
    client.call("open_map", {"path": str(source_path), "discard_changes": True})
    check(dict(detail(client, [overlay])[0]["properties"])["material"] == "decals/decalgraffiti001a", "Authored overlay settings survive save and reopen")
    client.call("select_faces", {"faces": [source]})
    stale = preview(client, source)
    client.call("move_objects", {"ids": [plate], "offset": [1, 0, 0]})
    changed = detail(client, [plate])
    client.call("apply_overlay", {"token": stale["draft"]["token"]}, error=True)
    check(detail(client, [plate]) == changed and not client.call("get_overlay_state")["draft"]["valid"], "Source edits invalidate Apply without changing the edited source")
    client.call("cancel_overlay", {"token": stale["draft"]["token"]})
    client.call("select", {"ids": [overlay]})
    eligible = client.call("get_overlay_state")
    check(eligible["selected_overlay"]["entity"] == overlay and eligible["refit_error"] is None, "The selected existing overlay exposes its eligible original face binding")
    original_overlay = detail(client, [overlay])[0]
    refit_info = client.call("get_map_info")
    before_refit_bsp = output / "before-refit.bsp"
    client.call("compile_map", {"path": str(before_refit_bsp)})
    refit = client.call("preview_overlay_refit", {"snapshot": eligible["snapshot"]})
    check(refit["draft"]["operation"] == "refit" and refit["draft"]["entity"] == overlay and refit["draft"]["source"] == source, "Refit previews the same entity and its existing source without rebinding")
    check(refit["draft"]["corners"] != state["draft"]["corners"], "Refit follows the moved tilted source face")
    check(detail(client, [overlay])[0] == original_overlay and detail(client, [plate]) == changed and client.call("get_map_info")["undo"] == refit_info["undo"], "Refit preview preserves entities, source and history")
    for invalid in [{"id": plate}, {"id": "entity:4000000000"}, {"id": overlay, "source": source}]:
        client.call("preview_overlay_refit", {"snapshot": refit["snapshot"], **invalid}, error=True)
        check(client.call("get_overlay_state")["draft"] == refit["draft"], f"Rejected refit preserves the valid draft: {invalid}")
    refit_uncommitted = output / "refit-uncommitted.bsp"
    client.call("compile_map", {"path": str(refit_uncommitted)})
    check(refit_uncommitted.read_bytes() == before_refit_bsp.read_bytes() and client.call("get_overlay_state")["draft"]["valid"], "Uncommitted refit is excluded from compilation and keeps its token valid")
    client.call("cancel_overlay", {"token": refit["draft"]["token"]})
    check(detail(client, [overlay])[0] == original_overlay and client.call("get_map_info")["undo"] == refit_info["undo"], "Cancel refit preserves the original overlay and history")
    refit = client.call("preview_overlay_refit", {"snapshot": client.call("get_overlay_state")["snapshot"], "id": overlay})
    applied_refit = client.call("apply_overlay", {"token": refit["draft"]["token"]})
    check(applied_refit["operation"] == "refit" and applied_refit["updated"] == overlay and "created" not in applied_refit, "Apply refit explicitly updates the same entity")
    refitted = detail(client, [overlay])[0]
    projection = {"origin", "sides", "basisorigin", "basisnormal", "basisu", "basisv", "uv0", "uv1", "uv2", "uv3"}
    retained = lambda entity: [(key, value) for key, value in entity["properties"] if key.lower() not in projection]
    check(retained(refitted) == retained(original_overlay) and dict(refitted["properties"])["sides"] == dict(original_overlay["properties"])["sides"], "Refit preserves raw settings and the existing side binding")
    after_refit_info = client.call("get_map_info")
    check(after_refit_info["point_entities"] == refit_info["point_entities"] and after_refit_info["selection"] == [overlay] and after_refit_info["undo"][0] == "Refit overlay" and len(after_refit_info["undo"]) == len(refit_info["undo"]) + 1, "Refit creates no entity and records exactly one Undo step")
    check(detail(client, [plate]) == changed and source_path.read_bytes() == saved, "Refit leaves brush geometry and the saved VMF unchanged")
    refit_bsp = output / "refitted.bsp"
    client.call("compile_map", {"path": str(refit_bsp)})
    check(len(records(refit_bsp.read_bytes(), 45, "<ihH64i22f")) == 1 and records(refit_bsp.read_bytes(), 60, "<2f") == fades, "Refitted projection compiles as one native overlay with unchanged fades")
    client.call("undo")
    check(detail(client, [overlay])[0] == original_overlay and detail(client, [plate]) == changed, "Undo refit restores the exact prior overlay and keeps the moved source")
    client.call("redo")
    check(detail(client, [overlay])[0] == refitted, "Redo restores the refitted projection on the same entity")
    floor = detail(client, [room_brushes[0]])[0]
    destination = face_ref(floor, max(floor["faces"], key=lambda face: face["normal"][2]))
    client.call("select_faces", {"faces": [destination]})
    rebind_info = client.call("get_map_info")
    choices = client.call("browse_overlay_targets", {"query": "authored OVERLAY", "limit": 1})
    check(choices["total"] == 1 and choices["entries"][0]["entity"] == overlay and choices["entries"][0]["eligible"], "Searchable overlay choices identify an eligible existing binding without selecting its entity")
    check(client.call("browse_overlay_targets", {"offset": 1, "limit": 1})["entries"] == [] and client.call("get_map_info")["selection"] == rebind_info["selection"], "Overlay target paging leaves the destination face selection intact")
    before_rebind = output / "before-rebind.bsp"
    client.call("compile_map", {"path": str(before_rebind)})
    rebound = client.call("preview_overlay_rebind", {"snapshot": choices["snapshot"], "id": overlay, "source": destination})
    check(rebound["draft"]["operation"] == "rebind" and rebound["draft"]["previous_source"] == source and rebound["draft"]["source"] == destination, "Explicit rebind distinguishes the previous and destination bindings")
    check(detail(client, [overlay])[0] == refitted and client.call("get_map_info")["undo"] == rebind_info["undo"], "Rebind preview preserves the existing overlay and Undo history")
    for invalid in [{"source": source}, {"id": plate}, {"material": "decals/replacement"}, {"sides": str(destination["side"])}]:
        client.call("preview_overlay_rebind", {"snapshot": rebound["snapshot"], "id": overlay, "source": destination, **invalid}, error=True)
        check(client.call("get_overlay_state")["draft"] == rebound["draft"], f"Rejected rebind preserves the owned draft: {invalid}")
    pending_rebind = output / "uncommitted-rebind.bsp"
    client.call("compile_map", {"path": str(pending_rebind)})
    check(pending_rebind.read_bytes() == before_rebind.read_bytes(), "An unapplied new-face rebind does not change compiled output")
    client.call("cancel_overlay", {"token": rebound["draft"]["token"]})
    check(detail(client, [overlay])[0] == refitted and client.call("get_map_info")["undo"] == rebind_info["undo"], "Cancel rebind keeps the previous binding and history")
    rebound = client.call("preview_overlay_rebind", {"snapshot": client.call("get_overlay_state")["snapshot"], "id": overlay, "source": destination})
    old_token = rebound["draft"]["token"]
    reviewed = client.call("review_overlay", {"token": old_token})
    client.call("apply_overlay", {"token": old_token}, error=True)
    applied_rebind = client.call("apply_overlay", {"token": reviewed["draft"]["token"]})
    after_rebind = detail(client, [overlay])[0]
    rebind_after_info = client.call("get_map_info")
    check(applied_rebind["operation"] == "rebind" and applied_rebind["updated"] == overlay and "created" not in applied_rebind, "Reviewed rebind applies to the same overlay entity")
    check(retained(after_rebind) == retained(refitted) and dict(after_rebind["properties"])["sides"] == str(destination["side"]), "Rebinding updates the destination side while preserving raw authored settings")
    check(rebind_after_info["point_entities"] == rebind_info["point_entities"] and len(rebind_after_info["undo"]) == len(rebind_info["undo"]) + 1 and rebind_after_info["undo"][0] == "Rebind overlay", "Rebinding adds exactly one Undo step and creates no entity")
    rebound_bsp = output / "rebound.bsp"
    client.call("compile_map", {"path": str(rebound_bsp)})
    check(len(records(rebound_bsp.read_bytes(), 45, "<ihH64i22f")) == 1 and records(rebound_bsp.read_bytes(), 60, "<2f") == fades, "The new binding compiles as one native overlay with unchanged fades")
    rebound_async = output / "rebound-async.bsp"
    client.call("start_compile", {"path": str(rebound_async)})
    job = wait_job(client)
    check(job["status"] == "completed" and rebound_async.read_bytes() == rebound_bsp.read_bytes(), "Synchronous and controlled builds agree for the rebound overlay")
    check(detail(client, [plate]) == changed and source_path.read_bytes() == saved, "Rebinding preserves authored brush geometry and the saved source")
    client.call("undo")
    check(detail(client, [overlay])[0] == refitted, "Undo rebind restores the exact former binding and projection")
    client.call("redo")
    check(detail(client, [overlay])[0] == after_rebind, "Redo rebind restores the same entity on its destination")
    return checks


def native_fixture(client, output):
    client.call("new_map", {"discard_changes": True})
    brushes = [client.call("create_block", {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"})["id"] for low, high in ROOM]
    for classname, origin, properties in [
        ("info_player_terrorist", [-64, -64, 8], {}),
        ("info_player_counterterrorist", [128, 64, 8], {}),
        ("light", [0, 0, 180], {"_light": "255 255 255 200", "_constant_attn": "1", "_quadratic_attn": "0"}),
        ("logic_script", [0, 0, 64], {"targetname": "hammerr_observer", "vscripts": "hammerr_observer.nut", "thinkfunction": "HammeRRThink"}),
        ("prop_physics", [160, 128, 128], {"targetname": "physics_crate", "model": "models/props_junk/wood_crate001a.mdl"}),
    ]:
        client.call("create_entity", {"classname": classname, "origin": origin, "properties": properties})
    floor = detail(client, brushes[:1])[0]
    top = next(face for face in floor["faces"] if face["normal"] == [0, 0, 1])
    source = face_ref(floor, top)
    client.call("select_faces", {"faces": [source]})
    client.call("set_compile_settings", {"lighting": "direct_ldr", "physics": "engine"})
    baseline = output / "native-baseline.bsp"
    client.call("compile_map", {"path": str(baseline)})
    state = preview(client, source, targetname="floor_marking", u=[0, 1], v=[0, 1], fade_min=-1, fade_max=0)
    result = client.call("apply_overlay", {"token": state["draft"]["token"]})
    visible = output / "native-visible.bsp"
    client.call("compile_map", {"path": str(visible)})
    client.call("set_entity_properties", {"id": result["created"], "set": {"fademindist": "16", "fademaxdist": "32"}})
    faded = output / "native-faded.bsp"
    client.call("compile_map", {"path": str(faded)})
    return [("baseline", baseline), ("visible", visible), ("faded", faded)]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, default=Path("/tmp/hammerr-overlay-authoring"))
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
    with patch.dict("os.environ", {
        "HAMMERR_RECENT_MAPS_DIR": str(args.output / "recent"),
        "HAMMERR_BUILD_RECIPES": str(args.output / "recipes.json"),
    }):
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
        (args.output / "render-results.json").write_text(json.dumps(reports, indent=2) + "\n")
        print(json.dumps({"native_fitted_overlay": True, "insecure": True, "sv_lan": True}))


if __name__ == "__main__":
    main()
