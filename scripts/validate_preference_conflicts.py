import argparse
import copy
import fcntl
import json
from contextlib import contextmanager
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client, detail

STORES = {
    "grid": ("get_grid_defaults", "set_grid_defaults", "grid-defaults.json", ".grid-defaults.lock"),
    "preview": ("get_preview_defaults", "set_preview_defaults", "preview-defaults.json", ".preview-defaults.lock"),
    "navigation": ("get_navigation_settings", "set_navigation_defaults", "navigation-defaults.json", ".navigation-defaults.lock"),
    "shortcuts": ("get_keyboard_shortcuts", "set_keyboard_shortcuts", "shortcuts.json", ".shortcuts.lock"),
}


@contextmanager
def launched(binary, output):
    client = Client(None, binary, output)
    try:
        yield client
    finally:
        client.close()


def state(client, store):
    return client.call(STORES[store][0])


def change(client, store, action, bindings=None, error=False):
    arguments = {"action": action, "snapshot": state(client, store)["snapshot"]}
    if bindings is not None:
        arguments["bindings"] = bindings
    return client.call(STORES[store][1], arguments, error=error)


def source_state(client, ids):
    info = client.call("get_map_info")
    return {
        **{key: info[key] for key in [
            "path", "unsaved_changes", "world_brushes", "point_entities", "brush_entities",
            "worldspawn", "selection", "face_selection", "undo", "redo",
        ]},
        "generation": info["recovery"]["generation"],
        "revision": info["recovery"]["revision"],
        "objects": detail(client, ids),
        "camera": client.call("get_camera"),
        "measurement": client.call("get_measurements")["draft"],
        "rendering": info["rendering"],
        "running": [info[key] for key in ["compiling", "launching", "game_running"]],
    }


def custom_bindings(factory, key):
    bindings = copy.deepcopy(factory)
    bindings["undo"] = [{"key": key, "control": True, "shift": False}, None]
    bindings["duplicate_selection"] = [None, None]
    return bindings


def validate(binary, output):
    checks = []

    def check(condition, label):
        if not condition:
            raise AssertionError(label)
        checks.append(label)

    preferences = output / "shared-preferences"
    other = output / "retargeted-preferences"
    preferences.mkdir()
    other.mkdir()
    directories = [output / "writer-a", output / "writer-b"]
    for directory in directories:
        directory.mkdir()
        (directory / "recovery").symlink_to(preferences, target_is_directory=True)

    with launched(binary, directories[0]) as first, launched(binary, directories[1]) as second:
        names = {tool["name"] for tool in second.rpc("tools/list", {})["tools"]}
        check(all(getter in names and setter in names for getter, setter, _, _ in STORES.values()),
              "All four preference stores expose their exact shared MCP tools")
        factories = {store: state(second, store)["factory_defaults"] for store in STORES}
        for store, (_, _, filename, _) in STORES.items():
            initial = state(second, store)
            check(initial["ready"] and not initial["stored"] and state(first, store)["ready"]
                  and not state(first, store)["stored"] and not (preferences / filename).exists(),
                  f"{store}: both editors start with a reviewed missing file")
            check(Path(initial["path"]) == directories[1] / "recovery" / filename,
                  f"{store}: storage stays in the private shared directory alias")

        first_values = {
            "grid": {"grid_size": 17.375, "snap": False, "grid_visible": True},
            "preview": {**factories["preview"], "entity_images": False},
            "navigation": {"fly_speed": 987.625},
            "shortcuts": custom_bindings(factories["shortcuts"], "F6"),
        }
        second_values = {
            "grid": {"grid_size": 23.5, "snap": True, "grid_visible": False},
            "preview": {**factories["preview"], "selection_outlines": False},
            "navigation": {"fly_speed": 1500.0},
            "shortcuts": custom_bindings(factories["shortcuts"], "F8"),
        }
        for client, values in [(first, first_values), (second, second_values)]:
            client.call("set_spatial_settings", values["grid"])
            client.call("set_view", values["preview"])
            client.call("set_navigation_settings", values["navigation"])

        block = second.call("create_block", {"mins": [0, 0, 0], "maxs": [64, 64, 64], "material": "tools/toolsnodraw"})["id"]
        second.call("set_measurement", {
            "action": "preview", "snapshot": second.call("get_measurements")["snapshot"],
            "name": "Owned preference audit", "start": [0, 0, 0], "end": [64, 0, 0],
        })
        original = source_state(second, [block])
        check(original["measurement"]["owner"] == "agent" and original["measurement"]["valid"],
              "The fixture retains a real Agent-owned measurement review")
        check(not original["rendering"]["initialized"] and not any(original["running"]),
              "Private preference checks start without graphics or native processes")

        alias = directories[1] / "recovery"
        alias.unlink()
        alias.symlink_to(other, target_is_directory=True)
        for store, (_, _, filename, _) in STORES.items():
            before = state(second, store)["current"]
            for _ in range(2):
                change(second, store, "reset", error=True)
                failed = state(second, store)
                check(not failed["ready"] and "directory" in failed["error"] and failed["current"] == before,
                      f"{store}: repeated Reset rejects a retargeted missing-file directory")
            check(not (preferences / filename).exists() and not (other / filename).exists(),
                  f"{store}: alias rejection writes to neither directory")
        check(source_state(second, [block]) == original,
              "Directory conflicts preserve source, history, selection, camera and the owned review")
        alias.unlink()
        alias.symlink_to(preferences, target_is_directory=True)
        for store in STORES:
            reloaded = change(second, store, "reload")
            check(reloaded["ready"] and not reloaded["stored"],
                  f"{store}: explicit Reload reviews the restored shared directory")

        for store, (_, _, filename, lockname) in STORES.items():
            path = preferences / filename
            before = state(second, store)
            saved = change(first, store, "save", first_values[store] if store == "shortcuts" else None)
            expected = first_values[store]
            check(saved["write_committed"] and saved["ready"] and json.loads(path.read_text()) == {
                "version": 1, "bindings" if store == "shortcuts" else store: expected,
            }, f"{store}: first editor explicitly publishes the exact bounded v1 preference")
            saved_bytes = path.read_bytes()
            check(state(second, store)["snapshot"] == before["snapshot"],
                  f"{store}: inspection does not implicitly Reload an outside write")
            change(second, store, "save", second_values[store] if store == "shortcuts" else None, error=True)
            failed = state(second, store)
            check(not failed["ready"] and "outside this session" in failed["error"]
                  and failed["current"] == before["current"] and path.read_bytes() == saved_bytes,
                  f"{store}: stale writer rejects and preserves saved bytes plus current values")
            check(source_state(second, [block]) == original,
                  f"{store}: stale publication preserves the map and owned review")
            reloaded = change(second, store, "reload")
            if store == "shortcuts":
                check(reloaded["current"] == first_values[store],
                      "shortcuts: valid Reload activates the other editor's saved bindings")
            else:
                check(reloaded["saved"] == first_values[store] and reloaded["current"] == before["current"],
                      f"{store}: Reload adopts saved defaults without applying them")
            if store == "navigation":
                applied = change(second, store, "apply")
                check(applied["current"] == first_values[store] and not applied["write_committed"]
                      and path.read_bytes() == saved_bytes,
                      "navigation: Apply restores saved speed without writing or changing the owned review")
                for speed in [0, 20001]:
                    snapshot = state(second, store)["snapshot"]
                    second.call("set_navigation_settings", {"fly_speed": speed}, error=True)
                    check(state(second, store)["snapshot"] == snapshot,
                          f"navigation: invalid fly speed {speed} preserves current settings and snapshot")
                second.call("set_navigation_settings", second_values[store])
                second.call("set_navigation_defaults", {"action": "save", "snapshot": applied["snapshot"]}, error=True)
                check(state(second, store)["current"] == second_values[store] and path.read_bytes() == saved_bytes,
                      "navigation: a form from before a current-speed change cannot publish")
                check(source_state(second, [block]) == original,
                      "navigation: Apply and temporary speed changes preserve the owned review and camera")

            retried = change(second, store, "save", second_values[store] if store == "shortcuts" else None)
            check(retried["write_committed"] and retried["ready"] and retried["current"] == second_values[store],
                  f"{store}: Reload followed by explicit retry publishes and retains intended current values")
            check(json.loads(path.read_text())["bindings" if store == "shortcuts" else store] == second_values[store],
                  f"{store}: successful retry persists the second editor's exact values")
            current = retried["current"]
            saved_bytes = path.read_bytes()
            with (preferences / lockname).open("r+b") as lock:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                change(second, store, "save", second_values[store] if store == "shortcuts" else None, error=True)
                blocked = state(second, store)
                check(not blocked["ready"] and "busy" in blocked["error"]
                      and blocked["current"] == current and path.read_bytes() == saved_bytes,
                      f"{store}: an independent same-target lock rejects promptly without mutation")
            change(second, store, "reload")
            check(change(second, store, "save", second_values[store] if store == "shortcuts" else None)["write_committed"],
                  f"{store}: released lock plus Reload permits explicit retry")
            reset = change(second, store, "reset")
            check(reset["write_committed"] and reset["ready"]
                  and reset["current"] == (factories[store] if store == "shortcuts" else current),
                  f"{store}: Reset activates only shortcuts and leaves current defaults settings intact")
            check(json.loads(path.read_text())["bindings" if store == "shortcuts" else store] == factories[store],
                  f"{store}: Reset persists exact factory values")
            check(source_state(second, [block]) == original,
                  f"{store}: Save, Reload, Apply, lock recovery and Reset preserve source/history/camera/review and GPU laziness")

    result = {"transport": "two private stdio processes", "checks": len(checks), "passed": checks}
    (output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"transport": result["transport"], "checks": len(checks), "output": str(output)}))


def main():
    parser = argparse.ArgumentParser(description="Validate four preference stores against real private MCP writers and locks.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True, help="New private directory for both processes, preferences and reports")
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    validate(args.binary.resolve(), output)


if __name__ == "__main__":
    main()
