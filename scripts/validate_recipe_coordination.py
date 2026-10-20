import argparse
import copy
import fcntl
import json
import os
from pathlib import Path
from unittest.mock import patch

from validate_face_lighting import DEFAULT_BINARY, ROOT, Client
from validate_material_albedo import require
from validate_material_read_limits import fingerprint, fingerprints
from validate_preference_conflicts import source_state
from validate_recovery_exit import require_clean_eof

LOCK_NAME = ".build-recipes.lock"
OPTIONS = (
    {"lighting": "fullbright", "physics": "none", "sky_quality": "fast", "visibility": "fast",
     "bounces": 0, "smoothing_angle": 0, "generate_reflections": False, "pack_sky_assets": False,
     "bake_static_prop_vertices": False},
    {"lighting": "direct_ldr_hdr", "physics": "none", "sky_quality": "balanced", "visibility": "detailed",
     "bounces": 1, "smoothing_angle": 30, "generate_reflections": False, "pack_sky_assets": True,
     "bake_static_prop_vertices": True},
)
HELPERS = (
    "validate_build_recipes.py", "validate_face_lighting.py", "validate_material_albedo.py",
    "validate_material_read_limits.py", "validate_preference_conflicts.py", "validate_recovery_exit.py",
)


def stored_options(index):
    return {key: value for key, value in OPTIONS[index].items()
            if key not in {"pack_sky_assets", "bake_static_prop_vertices"} or value}


def open_process(binary, directory, store, game, clients):
    directory.mkdir()
    temporary = directory / "tmp"
    temporary.mkdir()
    environment = {
        "HAMMERR_BUILD_RECIPES": str(store), "HAMMERR_RECENT_MAPS_DIR": str(directory / "recent"),
        "HAMMERR_PREFABS_DIR": str(directory / "prefabs"), "XDG_CONFIG_HOME": str(directory / "config"),
        "XDG_STATE_HOME": str(directory / "state"), "TMPDIR": str(temporary),
    }
    client = Client.__new__(Client)
    client.process, client.log = None, None
    clients.append(client)
    with patch.dict(os.environ, environment):
        Client.__init__(client, None, binary, directory, game / "gameinfo.txt")
    return client


def fixture(client, directory, index):
    block = client.call("create_block", {"mins": [0, 0, 0], "maxs": [64, 64, 64],
                                         "material": "hammerr698/probe"})["id"]
    source = directory / "source.vmf"
    client.call("save_map", {"path": str(source)})
    client.call("move_objects", {"ids": [block], "offset": [8, 0, 0]})
    client.call("select", {"ids": [block]})
    client.call("set_compile_settings", OPTIONS[index])
    if index == 0:
        token = client.call("begin_property_edit", {"world": True})["draft"]["token"]
        client.call("update_property_edit", {"token": token, "changes": [
            {"action": "set", "key": "message", "value": "Pending coordinated recipe review"}]})
    else:
        client.call("set_measurement", {"action": "preview", "snapshot": client.call("get_measurements")["snapshot"],
                                         "name": "Owned recipe clearance", "start": [0, 0, 0], "end": [64, 0, 0]})
    return source, [block]


def owned(client, source, ids):
    state = source_state(client, ids)
    info = client.call("get_map_info")
    require(not state["rendering"]["initialized"] and not any(state["running"]),
            "Recipe coordination initialized graphics, compilation or a game")
    return {"state": state, "history": client.call("get_history"), "view": info["view"],
            "property_draft": None if info["property_draft"] is None else client.call("get_property_edit"),
            "source": fingerprint(source)}


def read_store(path):
    if not path.exists():
        return None
    require(path.is_file() and path.stat().st_size <= 65536, "Recipe store exceeded its existing bounded format")
    data = path.read_bytes()
    require(len(data) <= 65536, "Recipe store changed beyond its existing byte bound")
    return data


def text_error(response):
    return "\n".join(row["text"] for row in response["content"] if row["type"] == "text")


def validate(args):
    clients, fixtures, baselines, settings = [], [], [], []
    result = {"status": "failed", "binary": str(args.binary), "binary_fingerprint": fingerprint(args.binary),
              "script_fingerprint": fingerprint(Path(__file__)),
              "helper_fingerprints": {name: fingerprint(Path(__file__).parent / name) for name in HELPERS},
              "transport": "two separate private stdio MCP processes", "steps": [], "compiles": 0,
              "graphics": False, "native": False, "game": False}
    store = args.output / "shared/build-recipes.json"
    store.parent.mkdir()
    game = args.output / "game"
    (game / "materials/hammerr698").mkdir(parents=True)
    (game / "gameinfo.txt").write_text('GameInfo { FileSystem { SearchPaths { game "|gameinfo_path|." } } }\n')
    (game / "materials/hammerr698/probe.vmt").write_text("UnlitGeneric { %compilenodraw 1 }\n")
    asset_manifest = fingerprints(game)

    def preserved():
        require(fingerprints(game) == asset_manifest, "Recipe actions changed mounted assets")
        for index, client in enumerate(clients):
            current_state = owned(client, *fixtures[index])
            if current_state != baselines[index]:
                result["preservation_failure"] = {"process": index, "expected": baselines[index], "actual": current_state}
                raise AssertionError("Recipe action changed source/history/selection/camera/draft")
            current = client.call("get_compile_settings")
            expected = copy.deepcopy(settings[index])
            expected["recipe"] = client.call("get_build_recipes")["active"]
            require(current == expected, "Recipe action changed current compiler options or unrelated settings metadata")

    def record(label, **extra):
        preserved()
        result["steps"].append({"label": label, "store": None if not store.exists() else fingerprint(store), **extra})

    def refused(index, tool, arguments, tokens, publication=True):
        before = [client.call("get_build_recipes") for client in clients]
        before_settings = [client.call("get_compile_settings") for client in clients]
        disk = read_store(store)
        response = clients[index].call(tool, arguments, error=True)
        message = text_error(response)
        require(all(token.lower() in message.lower() for token in tokens), f"Wrong refusal for {tool}: {message}")
        for peer, client in enumerate(clients):
            expected = copy.deepcopy(before[peer])
            if peer == index and publication:
                expected["error"] = message
            require(client.call("get_build_recipes") == expected, "Refusal changed recipe catalog beyond its exact save error")
            require(client.call("get_compile_settings") == before_settings[peer], "Refusal changed active identity/options")
        require(read_store(store) == disk, "Refused recipe action changed shared saved bytes")
        record(f"refused-{index}-{tool}", diagnostic=message)

    def reload(index):
        before = clients[index].call("get_build_recipes")
        before_settings = clients[index].call("get_compile_settings")
        disk = read_store(store)
        loaded = clients[index].call("reload_build_recipes", {"revision": before["revision"]})
        require(loaded["error"] is None and loaded["custom_editable"] and loaded["persistence"] == "persistent",
                "Explicit Reload did not recover an editable persistent registry")
        require(loaded["settings"] == before["settings"] and loaded["active"] == before["active"],
                "Reload unexpectedly applied settings or changed the still-valid active identity")
        require(clients[index].call("get_compile_settings") == before_settings and read_store(store) == disk,
                "Reload wrote storage or changed document-local settings")
        record(f"reload-{index}")
        return loaded

    def save(index, name):
        before = clients[index].call("get_build_recipes")
        saved = clients[index].call("save_build_recipe", {"name": name, "revision": before["revision"]})
        require(saved["active"] == {"id": f"custom:{index + 1}", "name": name}, "Reload/retry reused or skipped an identifier")
        require(saved["error"] is None and saved["settings"] == stored_options(index), "Save altered current options or retained a failure")
        record(f"save-{index}", active=saved["active"])
        return saved

    try:
        for index in range(2):
            directory = args.output / f"process-{index}"
            client = open_process(args.binary, directory, store, game, clients)
            fixtures.append(fixture(client, directory, index))
            baselines.append(owned(client, *fixtures[index]))
            settings.append(client.call("get_compile_settings"))
            require(baselines[index]["state"]["unsaved_changes"] and baselines[index]["history"]["undo_count"] > 0,
                    "Fixture lacks genuine dirty source/history")
            require({key: settings[index][key] for key in OPTIONS[index]} == OPTIONS[index], "Fixture did not set exact options")
            catalog = client.call("get_build_recipes")
            require(catalog["path"] == str(store) and [row["id"] for row in catalog["recipes"]] == ["fast", "final"],
                    "Processes did not start from the same absent private store")
        result["process_ids"] = [client.process.pid for client in clients]
        result["protected_baselines"] = baselines
        result["initial_settings"] = settings
        require(len(set(result["process_ids"])) == 2, "Coordination used only one process")
        require(clients[0].call("get_property_edit")["draft"] is not None
                and clients[1].call("get_measurements")["draft"]["owner"] == "agent",
                "Fixture lacks its owned World draft and measurement review")
        record("two-process-initial")
        lock_path = store.parent / LOCK_NAME
        result["lock"] = {"path": str(lock_path), "mode": "Linux nonblocking exclusive flock"}
        with lock_path.open("a+b") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            for index in range(2):
                refused(index, "save_build_recipe", {"name": f"Locked {index}"}, ("Build recipes", "busy", "locked"))
            require(not store.exists(), "Held-lock Save created a recipe store")
        for index in range(2):
            reload(index)
        first = save(0, "First process")
        first_bytes = read_store(store)
        require(json.loads(first_bytes) == {"version": 1, "next_id": 2, "recipes": [
            {"id": "custom:1", "name": "First process", "settings": stored_options(0)}]}, "First publication differs from exact bounded v1 data")
        refused(1, "save_build_recipe", {"name": "Second process"}, ("changed", "Reload recipes"))
        require(read_store(store) == first_bytes, "Stale missing-file writer lost the first recipe")
        loaded = reload(1)
        require([row["id"] for row in loaded["recipes"]] == ["fast", "final", "custom:1"], "Reload did not adopt first process's recipe")
        second = save(1, "Second process")
        require(first["active"]["id"] != second["active"]["id"], "Separate successful saves reused a custom ID")
        for tool, arguments in (
            ("rename_build_recipe", {"id": "custom:1", "name": "First reviewed"}),
            ("delete_build_recipe", {"id": "custom:1"}),
            ("save_build_recipe", {"name": "First replaced", "replace_id": "custom:1"}),
        ):
            refused(0, tool, arguments, ("changed", "Reload recipes"))
        reload(0)
        renamed = clients[0].call("rename_build_recipe", {"id": "custom:1", "name": "First reviewed"})
        require(renamed["active"] == {"id": "custom:1", "name": "First reviewed"}, "Reload/retry rename lost the active identity")
        record("rename-after-reload")
        stale_revision = clients[1].call("get_build_recipes")["revision"]
        reload(1)
        refused(1, "save_build_recipe", {"name": "Stale form", "revision": stale_revision},
                ("changed", "reload"), publication=False)
        with lock_path.open("r+b") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            refused(1, "rename_build_recipe", {"id": "custom:2", "name": "Second reviewed"},
                    ("Build recipes", "busy", "locked"))
        reload(1)
        renamed = clients[1].call("rename_build_recipe", {"id": "custom:2", "name": "Second reviewed"})
        require(renamed["active"] == {"id": "custom:2", "name": "Second reviewed"}, "Released-lock retry did not retain its own recipe")
        record("rename-after-lock-release")
        for index in range(2):
            reload(index)
        expected_file = {"version": 1, "next_id": 3, "recipes": [
            {"id": "custom:1", "name": "First reviewed", "settings": stored_options(0)},
            {"id": "custom:2", "name": "Second reviewed", "settings": stored_options(1)},
        ]}
        require(json.loads(read_store(store)) == expected_file, "Final publication lost a recipe, setting or next_id")
        catalogs = [client.call("get_build_recipes") for client in clients]
        require(catalogs[0]["recipes"] == catalogs[1]["recipes"], "Final explicit Reload did not converge saved catalogs")
        require([row["active"]["id"] for row in catalogs] == ["custom:1", "custom:2"], "Peer writes changed document-local active recipes")
        require({path.name for path in store.parent.iterdir()} == {store.name, LOCK_NAME}, "Publication left unexpected store artifacts")
        result["final_store"] = {"path": str(store), **fingerprint(store), "value": expected_file}
        if args.reference_output is not None:
            reference = json.loads((args.reference_output / "results.json").read_text())
            require(reference["status"] == "accepted" and reference["script_fingerprint"] == result["script_fingerprint"]
                    and reference["helper_fingerprints"] == result["helper_fingerprints"]
                    and reference["binary_fingerprint"]["sha256"] != result["binary_fingerprint"]["sha256"],
                    "Cross-program reference is not the same accepted frozen oracle")
            previous = Path(reference["final_store"]["path"])
            require(fingerprint(previous)["sha256"] == reference["final_store"]["sha256"]
                    and previous.read_bytes() == read_store(store), "Production programs saved different final recipe bytes")
            result["cross_program_store_bytes_equal"] = True
        record("final-converged-catalogs")
        result["clean_eof"] = []
        for client in clients:
            result["clean_eof"].append(require_clean_eof(client))
        require(read_store(store) is not None and fingerprint(store)["sha256"] == result["final_store"]["sha256"]
                and fingerprints(game) == asset_manifest, "EOF changed accepted storage/assets")
        for index, (source, _) in enumerate(fixtures):
            require(fingerprint(source) == baselines[index]["source"], "EOF rewrote saved source")
        result["status"] = "accepted"
    except Exception as error:
        result["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        cleanup_errors = []
        result["cleanup"] = []
        try:
            for client in clients:
                if client.process is not None:
                    process_id = client.process.pid
                    try:
                        code = require_clean_eof(client)
                        result["cleanup"].append({"pid": process_id, "clean_eof": code})
                    except Exception as error:
                        cleanup_errors.append(f"{type(error).__name__}: {error}")
                elif client.log is not None and not client.log.closed:
                    client.log.close()
            if cleanup_errors:
                result["cleanup_errors"] = cleanup_errors
                result["status"] = "failed"
                raise RuntimeError("; ".join(cleanup_errors))
        finally:
            (args.output / "results.json").write_text(json.dumps(result, indent=2) + "\n")
    return result


def main():
    parser = argparse.ArgumentParser(description="Validate deterministic cross-process recipe publication through two private headless MCP sessions and held Linux locks.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--reference-output", type=Path)
    args = parser.parse_args()
    args.binary, args.output = args.binary.resolve(), args.output.resolve()
    if args.reference_output is not None:
        args.reference_output = args.reference_output.resolve()
    require(args.output.is_relative_to(ROOT / "target/validation"), "Retain private artifacts under target/validation")
    args.output.mkdir(parents=True, exist_ok=False)
    result = validate(args)
    print(json.dumps({"status": result["status"], "steps": len(result["steps"]), "output": str(args.output)}))


if __name__ == "__main__":
    main()
