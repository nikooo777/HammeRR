import argparse
import fcntl
import json
import tempfile
from contextlib import contextmanager
from pathlib import Path
from unittest.mock import patch

from validate_face_lighting import DEFAULT_BINARY, Client


@contextmanager
def launched(binary, output, stage, preferences=None):
    with patch.dict("os.environ", {"HAMMERR_RECENT_MAPS_DIR": str(preferences or output / "recent-map-state")}):
        client = Client(None, binary, output)
    try:
        yield client
    finally:
        client.close()
        (output / "editor.log").rename(output / f"editor-{stage}.log")


def map_state(client):
    info = client.call("get_map_info")
    return {
        **{key: info[key] for key in ["path", "unsaved_changes", "selection", "face_selection", "undo", "redo", "world_brushes", "worldspawn"]},
        "generation": info["recovery"]["generation"],
        "revision": info["recovery"]["revision"],
    }


def validate_conflicts(binary, output, check):
    first_output = output / "writer-a"
    second_output = output / "writer-b"
    first_output.mkdir()
    second_output.mkdir()
    preferences = output / "conflict-state"
    store = preferences / "recent-maps.json"
    first = output / "conflict-a.vmf"
    second = output / "conflict-b.vmf"
    with launched(binary, first_output, "a", preferences) as left:
        with launched(binary, second_output, "b", preferences) as right:
            check(not store.exists(), "Two writers inspect the same initially missing preference file")
            left.call("save_map", {"path": str(first)})
            saved = store.read_bytes()
            right.call("save_map", {"path": str(second)})
            listing = right.call("get_recent_maps")
            check(second.is_file() and map_state(right)["path"] == str(second), "Stale history publication does not fail a successful primary save")
            check(store.read_bytes() == saved and listing["entries"] == [], "A stale writer preserves the first editor's saved list and its own local list")
            check(listing["persistence"] == "blocked" and "Reload saved list" in listing["error"], "Stale writer conflict is visible and recoverable")
            measurement = right.call("get_measurements")
            right.call("set_measurement", {"action": "preview", "snapshot": measurement["snapshot"], "name": "Owned clearance", "start": [0, 0, 0], "end": [64, 0, 0]})
            owned = right.call("get_measurements")["draft"]
            before = map_state(right)
            reloaded = right.call("reload_recent_maps")
            check(reloaded["entries"][0]["path"] == str(first) and reloaded["error"] is None, "Explicit MCP Reload adopts the current saved list and clears conflict")
            check(map_state(right) == before and right.call("get_measurements")["draft"] == owned, "Reload preserves map/history/selection and the Agent-owned review")
            right.call("save_map")
            check([entry["path"] for entry in right.call("get_recent_maps")["entries"]] == [str(second), str(first)], "Saving after Reload retains both editors' map entries")
            left.call("reload_recent_maps")
            current = store.read_bytes()
            with (preferences / ".recent-maps.lock").open("r+b") as lock:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                left.call("clear_recent_maps", error=True)
                check(store.read_bytes() == current and "busy" in left.call("get_recent_maps")["error"], "Independent lock ownership fails promptly and preserves the saved list")
            left.call("clear_recent_maps")
            check(left.call("get_recent_maps")["entries"] == [], "A released lock permits retry without restarting")
            right.call("reload_recent_maps")
            valid = store.read_bytes()
            store.write_bytes(b"malformed preferences")
            right.call("reload_recent_maps", error=True)
            check(right.call("get_recent_maps")["persistence"] == "blocked" and store.read_bytes() == b"malformed preferences", "Invalid Reload blocks writes without overwriting malformed storage")
            store.write_bytes(valid)
            right.call("reload_recent_maps")
            check(right.call("get_recent_maps")["error"] is None, "Repair plus explicit Reload restores a usable preference list")


def validate(binary, output):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def listing(client):
        return client.call("get_recent_maps")

    def paths(client):
        return [entry["path"] for entry in listing(client)["entries"]]

    def save(client, path=None, error=False):
        return client.call("save_map", {"path": str(path)} if path else {}, error=error)

    def reopen(client, path, discard=False, error=False):
        return client.call("open_recent_map", {"path": str(path), "discard_changes": discard}, error=error)

    first = output / "courtyard.vmf"
    second = output / "station.vmf"
    third = output / "saved-with-history-error.vmf"
    store = output / "recent-map-state" / "recent-maps.json"
    check(not store.exists(), "Validation starts with isolated empty storage")
    with launched(binary, output, "workflow") as client:
        names = {tool["name"] for tool in client.rpc("tools/list", {})["tools"]}
        check({"get_recent_maps", "reload_recent_maps", "open_recent_map", "remove_recent_map", "clear_recent_maps"} <= names, "Recent-map tools are discoverable")
        initial = listing(client)
        check(initial["entries"] == [], "Fresh recent list is empty")
        check(initial["persistence"] == "persistent" and initial["directory"] == str(store.parent), "Storage override is explicit and isolated")
        check(initial["max_maps"] == 20 and initial["max_path_bytes"] == 4096 and initial["max_file_bytes"] == 192 * 1024, "Persistence bounds are inspectable")
        save(client, first)
        client.call("set_compile_settings", {"lighting": "direct_ldr", "sky_quality": "balanced", "visibility": "detailed", "bounces": 2, "smoothing_angle": 35})
        retained = listing(client)["entries"][0]["build_settings"]
        check(retained["lighting"] == "direct_ldr" and retained["bounces"] == 2, "Current map build changes update retained context")
        save(client, second)
        client.call("set_compile_settings", {"lighting": "fullbright", "bounces": 0})
        check(paths(client) == [str(second), str(first)], "Successful save-as retains both maps in recency order")
        reopen(client, first)
        check(paths(client) == [str(first), str(second)], "Reopening moves the map to the front without duplicates")
        active = client.call("get_compile_settings")
        check(all(active[key] == value for key, value in retained.items()), "Reopen restores all compatible build values through shared settings")
        client.call("create_block", {"mins": [0, 0, 0], "maxs": [64, 64, 64], "material": "tools/toolsnodraw"})
        before = map_state(client)
        before_list = listing(client)
        reopen(client, second, error=True)
        check(map_state(client) == before and listing(client) == before_list, "Dirty reopen refusal preserves map, history and recent list")
        invalid = output / "invalid.vmf"
        invalid.write_text("world { ")
        client.call("open_map", {"path": str(invalid), "discard_changes": True}, error=True)
        check(map_state(client) == before and listing(client) == before_list, "Malformed open leaves map and list unchanged")
        save(client, output / "absent" / "failed.vmf", error=True)
        check(map_state(client) == before and listing(client) == before_list, "Failed primary save leaves path, map version, history and recent list unchanged")
        reopen(client, second, discard=True)
        check(map_state(client)["path"] == str(second) and not map_state(client)["unsaved_changes"], "Explicit discard reopens a clean map")
        first_bytes = first.read_bytes()
        first.unlink()
        missing = next(entry for entry in listing(client)["entries"] if entry["path"] == str(first))
        check(not missing["available"], "Missing files remain visible with availability false")
        before = map_state(client)
        before_paths = paths(client)
        reopen(client, first, error=True)
        check(map_state(client) == before and paths(client) == before_paths, "Missing reopen does not remove history or replace the current map")
        first.write_bytes(first_bytes)
        original = store.read_bytes()
        mode = store.stat().st_mode
        store.chmod(0o444)
        try:
            client.call("remove_recent_map", {"path": str(first)}, error=True)
            client.call("clear_recent_maps", error=True)
            check(store.read_bytes() == original and paths(client) == before_paths, "Failed remove and clear preserve stored and Session history")
            check(map_state(client) == before, "History maintenance failures preserve current document and undo")
            save(client, third)
            check(third.is_file() and not map_state(client)["unsaved_changes"], "A successful primary save remains successful when history cannot save")
            check(paths(client) == before_paths and store.read_bytes() == original, "Auxiliary save failure preserves previous recent list")
            check("could not save" in listing(client)["error"], "History save failure is inspectable")
        finally:
            store.chmod(mode)
        save(client)
        check(paths(client)[0] == str(third) and listing(client)["error"] is None, "Successful retry publishes history and clears the error")

    with launched(binary, output, "restart") as client:
        check(paths(client) == [str(third), str(second), str(first)], "Recent order survives a process restart")
        reopen(client, first)
        active = client.call("get_compile_settings")
        check(all(active[key] == value for key, value in retained.items()), "Per-map context survives process restart")
        before = map_state(client)
        client.call("remove_recent_map", {"path": str(first)})
        check(first.is_file() and map_state(client) == before, "Removing the open map only removes its list entry")
        client.call("set_compile_settings", {"lighting": "fullbright"})
        check(str(first) not in paths(client), "Changing settings does not re-add an explicitly removed entry")
        save(client)
        check(paths(client)[0] == str(first), "A later successful save can remember a removed map again")
        for index in range(23):
            save(client, output / f"bounded-{index}.vmf")
        bounded = paths(client)
        check(len(bounded) == 20 and len(set(bounded)) == 20, "History is bounded to twenty unique maps")
        check(bounded[0].endswith("bounded-22.vmf") and bounded[-1].endswith("bounded-3.vmf"), "History evicts only the least recent entries")
        alias = output / "map-alias.vmf"
        alias.symlink_to(bounded[0])
        client.call("open_map", {"path": str(alias)})
        check(paths(client) == bounded, "Canonical file aliases do not create duplicate entries")
        before = map_state(client)
        client.call("clear_recent_maps")
        check(paths(client) == [] and map_state(client) == before, "Clear only changes recent preferences")
        check(all(Path(path).is_file() for path in bounded), "Clearing does not delete any map file")
        save(client, third)

    valid = store.read_bytes()
    data = json.loads(valid)
    data["entries"][0]["game_directory"] = str(output / "different-game")
    store.write_text(json.dumps(data))
    with launched(binary, output, "incompatible") as client:
        entry = listing(client)["entries"][0]
        check(not entry["context_compatible"], "Different mounted game context is reported")
        before = client.call("get_compile_settings")
        reopen(client, third)
        check(client.call("get_compile_settings") == before, "Incompatible context keeps current build settings")
        check(listing(client)["entries"][0]["game_directory"] == str(output / "different-game"), "Opening incompatible content preserves retained context for the original game")

    for stage, damaged in [
        ("corrupt", b"broken recent maps"),
        ("version", json.dumps({"version": 999, "entries": []}).encode()),
        ("oversized", b" " * (192 * 1024 + 1)),
    ]:
        store.write_bytes(damaged)
        with launched(binary, output, stage) as client:
            check(listing(client)["persistence"] == "blocked" and listing(client)["error"], f"{stage} storage reports blocked status")
            client.call("clear_recent_maps", error=True)
            client.call("open_map", {"path": str(first)})
            save(client, third)
            check(store.read_bytes() == damaged, f"{stage} storage cannot be overwritten by opens, saves or clear")
    store.write_bytes(valid)
    with launched(binary, output, "repaired") as client:
        check(listing(client)["persistence"] == "persistent" and len(paths(client)) == 1, "Repair and restart recovers preserved history")

    validate_conflicts(binary, output, check)

    return {"checks": checks, "count": len(checks), "output": str(output)}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    output = args.output.resolve() if args.output else Path(tempfile.mkdtemp(prefix="hammerr-recent-maps-"))
    output.mkdir(parents=True, exist_ok=True)
    report = validate(args.binary.resolve(), output)
    (output / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
