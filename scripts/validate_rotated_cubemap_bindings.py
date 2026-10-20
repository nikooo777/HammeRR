import argparse
import hashlib
import json
import os
from pathlib import Path
from unittest.mock import patch

from validate_build_recipes import protected_state
from validate_bump_lighting import texture
from validate_face_lighting import DEFAULT_BINARY, ROOT, Client, detail, face_ref
from validate_recovery_exit import require_clean_eof
from validate_visibility import ROOM

PIVOT = [16.125, -32.25, 24.375]
MINS = [64.125, -44.25, 14.375]
MAXS = [96.125, -20.25, 34.375]
SOURCE_CENTER = [96.125, -32.25, 24.375]
LOCAL_CENTER = [80, 0, 0]
WORLD_CENTER = [16.125, 47.75, 24.375]
WORLD_CONTROL = [-160, -160, 48]
SAMPLES = {"A": [128, -16, 24], "B": [16, 128, 24]}
PLAIN = "hammerr683/plain"
REFLECTIVE = "hammerr683/reflective"
TARGET = "rotated_reflection_receiver"
MARKER = f'"targetname" "{TARGET}"'
COUNTERFACTUALS = {
    "correct": (WORLD_CENTER, [0, 1, 0], "B"),
    "ignored": (SOURCE_CENTER, [1, 0, 0], "A"),
    "position_only": (WORLD_CENTER, [1, 0, 0], "A"),
    "normal_only": (SOURCE_CENTER, [0, 1, 0], "A"),
    "world_control": (WORLD_CONTROL, [0, 0, 1], "A"),
}


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def identities(directory):
    return {str(path.relative_to(directory)): digest(path.read_bytes())
            for path in sorted(directory.rglob("*")) if path.is_file()}


def declarations():
    rows = {}
    for name, (center, normal, expected) in COUNTERFACTUALS.items():
        candidates = []
        for label, position in SAMPLES.items():
            delta = [a - b for a, b in zip(position, center, strict=True)]
            candidates.append({"sample": label, "distance_squared": sum(value * value for value in delta),
                               "facing": sum(a * b for a, b in zip(delta, normal, strict=True))})
        front = [candidate for candidate in candidates if candidate["facing"] >= 0]
        selected = min(front or candidates, key=lambda candidate: candidate["distance_squared"])["sample"]
        require(selected == expected, f"Frozen {name} geometry does not distinguish the intended sample")
        rows[name] = {"center": center, "normal": normal, "expected": expected, "candidates": candidates}
    return {"pivot": PIVOT, "source_bounds": [MINS, MAXS], "source_center": SOURCE_CENTER,
            "local_center": LOCAL_CENTER, "samples": SAMPLES, "cases": rows,
            "formula": "Literal yaw90 maps local (80,0,0) to (0,80,0) and +X to +Y; add fractional P once. Prefer nonnegative-facing samples, then least squared distance; preserve authored order for ties.",
            "acceptance": "Exact face/helper/manual/integer-position/material/alias; no fitted distance tolerance or native capture",
            "existing_ownership_coverage": "crates/editor-core/src/mcp/tools/cubemap_bindings/tests.rs and session/cubemap_bindings/tests.rs cover User/Agent ownership; this public runner checks Agent review, replaced tokens and selection staleness"}


def assets(game):
    folder = game / "materials/hammerr683"
    folder.mkdir(parents=True)
    (game / "gameinfo.txt").write_text('GameInfo { FileSystem { SearchPaths { Game "|gameinfo_path|." } } }\n')
    (folder / "white.vtf").write_bytes(texture(bytes([192, 192, 192, 255]) * 32 * 32))
    for name, extra in [("plain", ""), ("reflective", '$envmap "env_cubemap"')]:
        (folder / f"{name}.vmt").write_text(f'LightmappedGeneric {{ $basetexture "hammerr683/white" {extra} }}\n')


def fixture(client, source):
    client.call("new_map", {"discard_changes": True})
    ids = [client.call("create_block", {"mins": low, "maxs": high, "material": PLAIN})["id"]
           for low, high in ROOM]
    control = client.call("create_block", {"mins": [-176, -176, 32], "maxs": [-144, -144, 48], "material": PLAIN})["id"]
    ids.append(control)
    shape = detail(client, [control])[0]
    world_face = face_ref(shape, next(face for face in shape["faces"] if face["normal"] == [0, 0, 1]))
    client.call("edit_faces", {"faces": [world_face], "material": REFLECTIVE})
    block = client.call("create_block", {"mins": MINS, "maxs": MAXS, "material": PLAIN})["id"]
    shape = detail(client, [block])[0]
    selected = face_ref(shape, next(face for face in shape["faces"] if face["normal"] == [1, 0, 0]))
    client.call("edit_faces", {"faces": [selected], "material": REFLECTIVE})
    owner = client.call("tie_to_entity", {"ids": [block], "classname": "func_brush", "properties": {
        "origin": "16.125 -32.25 24.375", "targetname": TARGET}})["id"]
    ids.append(owner)
    client.call("set_entity_properties", {"id": owner, "remove": ["angle", "angles"]})
    client.call("set_entity_properties", {"id": owner, "set": {"angles": "0 90 0"}})
    helpers = {}
    for label, position in SAMPLES.items():
        helper = client.call("create_entity", {"classname": "env_cubemap", "origin": position, "properties": {
            "cubemapsize": "3", "targetname": f"sample_{label.lower()}", "custom": "preserve"}})["id"]
        ids.append(helper)
        helpers[label] = int(helper.split(":")[-1])
    ids.append(client.call("create_entity", {"classname": "info_player_terrorist", "origin": [-192, -176, 8]})["id"])
    client.call("save_map", {"path": str(source)})
    client.call("open_map", {"path": str(source)})
    client.call("select_faces", {"faces": [selected]})
    return ids, owner, selected, world_face, helpers


def geometry(client, owner, selected):
    entity = detail(client, [owner])[0]
    require(entity["origin"] == PIVOT and [value for key, value in entity["properties"] if key == "angles"] == ["0 90 0"],
            "Loaded receiver origin/angles differ from the authored pose")
    require(len(entity["brushes"]) == 1, "Receiver must contain exactly one authored brush")
    brush = entity["brushes"][0]
    require(brush["solid"] == selected["solid"] and brush["bounds"] == {"mins": MINS, "maxs": MAXS}, "Authored source bounds changed")
    face = next(face for face in brush["faces"] if face["side"] == selected["side"])
    require(face["normal"] == [1, 0, 0] and face["material"] == REFLECTIVE, "Authored reflective normal/material changed")
    topology = client.call("get_brush_components", {"solids": [selected["solid"]]})["brushes"][0]
    polygon = next(face for face in topology["faces"] if face["side"] == selected["side"])
    points = [topology["vertices"][index]["position"] for index in polygon["vertices"]]
    expected = {(MAXS[0], y, z) for y in [MINS[1], MAXS[1]] for z in [MINS[2], MAXS[2]]}
    require(len(points) == 4 and {tuple(point) for point in points} == expected, "Loaded source winding differs from literal corners")
    center = [sum(point[axis] for point in points) / 4 for axis in range(3)]
    require(center == SOURCE_CENTER and [a - b for a, b in zip(center, PIVOT, strict=True)] == LOCAL_CENTER,
            "Loaded source center does not establish the intended local center")
    return {"origin": entity["origin"], "source_center": center, "source_normal": face["normal"], "corners": points}


def state(client, ids, source, game):
    info = client.call("get_map_info")
    require(not info["rendering"]["initialized"], "Cubemap inspection initialized graphics")
    require(not info["compiling"] and not info["launching"] and not info["game_running"], "Inspection started build or game work")
    settings = client.call("get_compile_settings")
    require(not settings["generate_reflections"], "Public oracle must never enable reflection capture")
    recipes = client.call("get_build_recipes")
    return {"map": protected_state(client), "history": client.call("get_history"), "objects": detail(client, ids),
            "settings": settings, "recipes": recipes, "recipe_bytes": digest(Path(recipes["path"]).read_bytes()),
            "source": digest(source.read_bytes()), "assets": identities(game)}


def unchanged(client, ids, source, game, operation):
    before = state(client, ids, source, game)
    result = operation()
    require(state(client, ids, source, game) == before, "Read-only inspection/draft operation mutated protected state")
    return result


def assignment(report, face, label, helpers, namespace, manual=False):
    require(not report["diagnostics"], report)
    matches = [row for row in report["assignments"] if row["face"] == face]
    require(len(matches) == 1, f"Missing or duplicate inspected assignment for {face}: {report}")
    row = matches[0]
    position = SAMPLES[label]
    alias = f'maps/{namespace}/{REFLECTIVE}_{position[0]}_{position[1]}_{position[2]}'
    require(row == {"face": face, "side": face["side"], "helper": helpers[label], "position": position,
                    "manual": manual, "material": REFLECTIVE, "alias": alias}, f"Assignment differs from literal {label}: {row}")
    return row


def inspect(client, ids, source, game, selected, world_face, helpers, label, manual=False):
    report = unchanged(client, ids, source, game, lambda: client.call("get_cubemap_bindings", {"limit": 256}))
    require(report["total"] == 2 and len(report["assignments"]) == 2 and not report["provisional_namespace"], report)
    assignment(report, selected, label, helpers, source.stem, manual)
    assignment(report, world_face, "A", helpers, source.stem)
    return report


def preview(client, ids, source, game, helper, selected, operation):
    report = client.call("get_cubemap_bindings")
    return unchanged(client, ids, source, game, lambda: client.call("set_cubemap_bindings", {
        "action": "preview", "helper": helper, "faces": [selected], "operation": operation,
        "snapshot": report["state"]["snapshot"]}))


def fixed_state(value):
    return {key: value[key] for key in ["settings", "recipes", "recipe_bytes", "source", "assets"]} | {
        "selection": value["map"]["selection"], "face_selection": value["map"]["face_selection"],
        "camera": value["map"]["camera"], "view": value["map"]["view"], "worldspawn": value["map"]["worldspawn"]}


def manual_workflow(client, ids, source, game, owner, selected, world_face, helpers):
    original = state(client, ids, source, game)
    draft = preview(client, ids, source, game, helpers["A"], selected, "bind")
    require(draft["draft"]["owner"] == "agent" and draft["draft"]["valid"], draft)
    assignment(draft["draft"]["report"], selected, "A", helpers, source.stem, True)
    old_token = draft["draft"]["token"]
    reviewed = unchanged(client, ids, source, game, lambda: client.call("set_cubemap_bindings", {"action": "review", "token": old_token}))
    token = reviewed["draft"]["token"]
    require(token != old_token and reviewed["draft"]["owner"] == "agent" and reviewed["draft"]["valid"], reviewed)
    for action in ["apply", "cancel"]:
        unchanged(client, ids, source, game, lambda action=action: client.call("set_cubemap_bindings", {"action": action, "token": old_token}, error=True))
        require(client.call("get_cubemap_bindings")["state"]["draft"] == reviewed["draft"], "Replaced token changed the reviewed draft")
    client.call("set_cubemap_bindings", {"action": "apply", "token": token})
    bound = state(client, ids, source, game)
    require(fixed_state(bound) == fixed_state(original) and len(bound["map"]["undo"]) == len(original["map"]["undo"]) + 1
            and bound["map"]["undo"][0] == "Edit cubemap face bindings", "Binding Apply changed unrelated state or history granularity")
    for before, after in zip(original["objects"], bound["objects"], strict=True):
        if before["id"] == f'entity:{helpers["A"]}':
            require([value for key, value in after["properties"] if key == "sides"] == [str(selected["side"])], "Manual sides property differs")
            require({**after, "properties": [pair for pair in after["properties"] if pair[0] != "sides"]} == before,
                    "Binding Apply changed helper metadata beyond sides")
        else:
            require(after == before, "Binding Apply rewrote unrelated objects or authored geometry")
    manual = inspect(client, ids, source, game, selected, world_face, helpers, "A", True)
    removed = preview(client, ids, source, game, helpers["A"], selected, "remove")
    assignment(removed["draft"]["report"], selected, "B", helpers, source.stem)
    client.call("set_cubemap_bindings", {"action": "apply", "token": removed["draft"]["token"]})
    automatic = inspect(client, ids, source, game, selected, world_face, helpers, "B")
    unbound = state(client, ids, source, game)
    require(unbound["objects"] == original["objects"] and fixed_state(unbound) == fixed_state(original)
            and len(unbound["map"]["undo"]) == len(bound["map"]["undo"]) + 1
            and unbound["map"]["undo"][0] == "Edit cubemap face bindings", "Removal changed unrelated source or history granularity")
    client.call("undo")
    require(state(client, ids, source, game)["objects"] == bound["objects"], "Undo removal did not restore manual sides exactly")
    inspect(client, ids, source, game, selected, world_face, helpers, "A", True)
    client.call("undo")
    require(state(client, ids, source, game)["objects"] == original["objects"], "Undo binding did not restore original authored objects")
    stale = preview(client, ids, source, game, helpers["A"], selected, "bind")
    client.call("select_faces", {"faces": []})
    require(not client.call("get_cubemap_bindings")["state"]["draft"]["valid"], "Selection change did not invalidate draft")
    unchanged(client, ids, source, game, lambda: client.call("set_cubemap_bindings", {"action": "apply", "token": stale["draft"]["token"]}, error=True))
    unchanged(client, ids, source, game, lambda: client.call("set_cubemap_bindings", {"action": "cancel", "token": stale["draft"]["token"]}))
    client.call("select_faces", {"faces": [selected]})
    final = state(client, ids, source, game)
    require(fixed_state(final) == fixed_state(original) and final["objects"] == original["objects"], "Manual workflow did not preserve source/settings/selection/assets")
    geometry(client, owner, selected)
    inspect(client, ids, source, game, selected, world_face, helpers, "B")
    return {"manual": manual, "removed_automatic": automatic, "undo_restored": True, "reviewed_replaced_and_stale_tokens_rejected": True}


def validate(args):
    game = args.output / "game"
    assets(game)
    environment = {"HAMMERR_RECENT_MAPS_DIR": str(args.output / "recent"), "HAMMERR_BUILD_RECIPES": str(args.output / "recipes.json"),
                   "HAMMERR_PREFABS_DIR": str(args.output / "prefabs"), "XDG_CONFIG_HOME": str(args.output / "config"), "XDG_STATE_HOME": str(args.output / "state")}
    with patch.dict(os.environ, environment):
        client = Client(None, args.binary, args.output, game / "gameinfo.txt")
    report = {"binary": str(args.binary), "binary_sha256": digest(args.binary.read_bytes()), "contract": declarations(), "success": False}
    try:
        source = args.output / "rotated.vmf"
        ids, owner, selected, world_face, helpers = fixture(client, source)
        client.call("save_build_recipe", {"name": "Cubemap inspection preservation"})
        report["authored_geometry"] = geometry(client, owner, selected)
        text = source.read_text()
        require(text.count(MARKER) == 1 and text.count('"angles" "0 90 0"') == 1
                and text.count('"origin" "16.125 -32.25 24.375"') == 1, "Source receiver pose is not uniquely identifiable")
        report["yaw90"] = inspect(client, ids, source, game, selected, world_face, helpers, "B")
        report["manual_workflow"] = manual_workflow(client, ids, source, game, owner, selected, world_face, helpers)
        report["zero_controls"] = []
        retained_sources = {str(source): digest(source.read_bytes())}
        for name, replacement in [("zero_absent", ""), ("zero_signed", '"angles" "-0 +0.0 0e-300"')]:
            path = args.output / f"{name}.vmf"
            path.write_text(text.replace('"angles" "0 90 0"', replacement))
            client.call("open_map", {"path": str(path), "discard_changes": True})
            client.call("select_faces", {"faces": [selected]})
            report["zero_controls"].append(inspect(client, ids, path, game, selected, world_face, helpers, "A"))
            retained_sources[str(path)] = digest(path.read_bytes())
        cases = [
            ("parented", text.replace(MARKER, MARKER + '\n"parentname" "unresolved_parent"'), "unparented"),
            ("unsupported_class", text.replace('"classname" "func_brush"', '"classname" "func_door"'), "func_brush"),
            ("ambiguous_angles", text.replace(MARKER, MARKER + '\n"angles#1" "0 90 0"'), "angle"),
            ("angle_out_of_bounds", text.replace('"angles" "0 90 0"', '"angles" "0 361 0"'), "360"),
            ("rotated_bounds", text.replace('"angles" "0 90 0"', '"angles" "0 180 0"').replace(
                '"origin" "16.125 -32.25 24.375"', '"origin" "16300 -32.25 24.375"'), "bounds"),
        ]
        report["refusals"] = []
        for name, content, token in cases:
            require(content != text, f"{name}: negative fixture did not change source")
            path = args.output / f"{name}.vmf"
            path.write_text(content)
            client.call("open_map", {"path": str(path), "discard_changes": True})
            client.call("select_faces", {"faces": [selected]})
            failed = unchanged(client, ids, path, game, lambda: client.call("get_cubemap_bindings"))
            require(failed["total"] == 0 and failed["assignments"] == [] and failed["diagnostics"], f"{name}: fabricated fallback assignments: {failed}")
            require(any(owner in message and f'side:{selected["side"]}' in message and token in message
                        and "automatic cubemap assignment" in message for message in failed["diagnostics"]),
                    f"{name}: missing actionable entity/side/pose diagnostic: {failed}")
            retained_sources[str(path)] = digest(path.read_bytes())
            report["refusals"].append({"case": name, "report": failed})
        before_eof = state(client, ids, path, game)
        report["eof_exit_code"] = require_clean_eof(client)
        require(all(digest(Path(path).read_bytes()) == value for path, value in retained_sources.items()), "Inspection/EOF changed a saved source fixture")
        require(identities(game) == before_eof["assets"] and digest(Path(before_eof["recipes"]["path"]).read_bytes()) == before_eof["recipe_bytes"],
                "EOF changed fixture assets or persisted recipes")
        require(not list(args.output.rglob("*.bsp")), "Inspection-only oracle produced a BSP")
        report.update({"success": True, "source_preserved": True, "assets_preserved": True, "graphics_initialized": False,
                       "compile_calls": 0, "native_capture": False, "fixture_sources": retained_sources})
    except BaseException as error:
        report["error"] = str(error)
        raise
    finally:
        if client.process is not None:
            client.close()
        (args.output / "results.json").write_text(json.dumps(report, indent=2, allow_nan=False) + "\n")
    return report


def main():
    parser = argparse.ArgumentParser(description="Headless public MCP proof of rotated automatic cubemap assignment and guarded manual binding drafts.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.binary = args.binary.resolve(strict=True)
    args.output = args.output.resolve()
    require(args.output.is_relative_to(ROOT / "target/validation"), "Keep receipts on disk below target/validation")
    args.output.mkdir(parents=True, exist_ok=False)
    report = validate(args)
    print(json.dumps({"success": report["success"], "results": str(args.output / "results.json")}))


if __name__ == "__main__":
    main()
