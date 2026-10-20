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


def require_builtins(catalog):
    expected = [
        ("fast", True, stored_options(0)),
        ("final", True, {"lighting": "direct_ldr", "physics": "engine", "sky_quality": "detailed",
                         "visibility": "detailed", "bounces": 4, "smoothing_angle": 45,
                         "generate_reflections": False}),
    ]
    require([(row["id"], row["builtin"], row["settings"]) for row in catalog["recipes"][:2]] == expected,
            "Path handling changed immutable Fast preview or Final lighting recipe settings")


def finish_path_case(clients, report, directory):
    errors = []
    report["cleanup"] = []
    for client in clients:
        if client.process is not None:
            process_id = client.process.pid
            try:
                report["cleanup"].append({"pid": process_id, "clean_eof": require_clean_eof(client)})
            except Exception as error:
                errors.append(f"{type(error).__name__}: {error}")
        elif client.log is not None and not client.log.closed:
            client.log.close()
    log = directory / "editor.log"
    if log.exists():
        report["stderr"] = fingerprint(log)
        if log.stat().st_size > 1024 * 1024:
            errors.append("Path case stderr exceeded its bounded fixture size")
        elif "panic" in log.read_text(errors="replace").lower():
            errors.append("Path case stderr contains a panic")
    if errors:
        report["status"], report["cleanup_errors"] = "failed", errors
        raise RuntimeError("; ".join(errors))


def invalid_requested_path(args, game, report):
    clients = []
    directory = args.output / "invalid-path-process"
    parent = args.output / "invalid-path-store"
    parent.mkdir()
    raw_name = b"recipes-\xff.json"
    requested = parent / os.fsdecode(raw_name)
    lossy = parent / raw_name.decode("utf-8", errors="replace")
    report.update({"status": "failed", "requested_path_hex": os.fsencode(requested).hex(),
                   "rejections": []})
    try:
        client = open_process(args.binary, directory, requested, game, clients)
        report["process_id"] = client.process.pid
        source, ids = fixture(client, directory, 0)
        baseline = owned(client, source, ids)
        settings = client.call("get_compile_settings")
        initial = client.call("get_build_recipes")
        require_builtins(initial)
        require(initial["path"] is None and initial["persistence"] == "blocked"
                and not initial["custom_editable"] and "UTF-8" in initial["error"]
                and "HAMMERR_BUILD_RECIPES" in initial["error"]
                and [row["id"] for row in initial["recipes"]] == ["fast", "final"],
                "Invalid requested recipe bytes did not produce a responsive explicit blocked report")
        require(baseline["state"]["unsaved_changes"] and baseline["history"]["undo_count"] > 0
                and baseline["property_draft"]["draft"] is not None,
                "Invalid-path case lacks genuine source/history and its owned World draft")
        report["initial_catalog"], report["protected_baseline"] = initial, baseline
        for tool, arguments, tokens in (
            ("save_build_recipe", {"name": "Unavailable path"}, ("did not load", "Reload recipes")),
            ("reload_build_recipes", {}, ("storage", "unavailable")),
        ):
            before = client.call("get_build_recipes")
            response = client.call(tool, arguments, error=True)
            message = text_error(response)
            require(all(token.lower() in message.lower() for token in tokens),
                    f"Wrong invalid-path refusal for {tool}: {message}")
            expected = copy.deepcopy(before)
            if tool == "save_build_recipe":
                expected["error"] = message
            require(client.call("get_build_recipes") == expected,
                    "Invalid-path refusal changed catalog metadata beyond its explicit error")
            require(owned(client, source, ids) == baseline
                    and client.call("get_compile_settings") == settings,
                    "Invalid-path refusal changed map/history/settings or its owned draft")
            require(not requested.exists() and not lossy.exists() and not list(parent.iterdir()),
                    "Invalid requested path created storage, a lock or a lossy substitute")
            report["rejections"].append({"tool": tool, "diagnostic": message})
        report["final_catalog"] = client.call("get_build_recipes")
        report["clean_eof"] = require_clean_eof(client)
        require(fingerprint(source) == baseline["source"] and not list(parent.iterdir()),
                "Invalid-path EOF changed saved source or the empty destination parent")
    except Exception as error:
        report["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        finish_path_case(clients, report, directory)
    report["status"] = "accepted"


def valid_alias_path(args, game, report):
    clients = []
    directory = args.output / "alias-path-process"
    parent = args.output / "alias-path-store"
    physical, requested_parent = parent / "physical", parent / "requested"
    physical.mkdir(parents=True)
    requested_parent.mkdir()
    raw_name = b"custom-recipes-\xff.store"
    destination = physical / os.fsdecode(raw_name)
    alias = requested_parent / "recipes.json"
    lossy = physical / raw_name.decode("utf-8", errors="replace")
    initial_bytes = b'{"version":1,"next_id":1,"recipes":[]}\n'
    destination.write_bytes(initial_bytes)
    alias.symlink_to(destination)
    report.update({"status": "failed", "requested_path": str(alias),
                   "physical_path_hex": os.fsencode(destination).hex()})
    try:
        client = open_process(args.binary, directory, alias, game, clients)
        report["process_id"] = client.process.pid
        source, ids = fixture(client, directory, 0)
        baseline = owned(client, source, ids)
        settings = client.call("get_compile_settings")
        initial = client.call("get_build_recipes")
        require_builtins(initial)
        require(initial["path"] == str(alias) and initial["persistence"] == "persistent"
                and initial["custom_editable"] and initial["error"] is None
                and [row["id"] for row in initial["recipes"]] == ["fast", "final"]
                and read_store(alias) == initial_bytes,
                "Valid UTF-8 alias did not load the exact non-UTF8 physical store")
        saved = client.call("save_build_recipe", {"name": "UTF-8 alias", "revision": initial["revision"]})
        require_builtins(saved)
        expected_file = {"version": 1, "next_id": 2, "recipes": [
            {"id": "custom:1", "name": "UTF-8 alias", "settings": stored_options(0)}]}
        require(saved["active"] == {"id": "custom:1", "name": "UTF-8 alias"}
                and saved["path"] == str(alias) and saved["error"] is None
                and saved["persistence"] == "persistent" and saved["custom_editable"]
                and [row["id"] for row in saved["recipes"]] == ["fast", "final", "custom:1"]
                and saved["settings"] == stored_options(0)
                and json.loads(read_store(destination)) == expected_file,
                "UTF-8 alias Save did not publish custom:1 to the exact byte-name target")
        expected_settings = copy.deepcopy(settings)
        expected_settings["recipe"] = saved["active"]
        saved_bytes = read_store(destination)
        loaded = client.call("reload_build_recipes", {"revision": saved["revision"]})
        require(loaded["recipes"] == saved["recipes"] and loaded["active"] == saved["active"]
                and loaded["path"] == str(alias) and loaded["error"] is None
                and loaded["custom_editable"] and loaded["persistence"] == "persistent"
                and read_store(alias) == saved_bytes,
                "UTF-8 alias Reload failed to retain the exact saved catalog")
        require(owned(client, source, ids) == baseline
                and client.call("get_compile_settings") == expected_settings,
                "Alias publication/reload changed map/history/options or its owned draft")
        require(alias.is_symlink() and os.fsencode(alias.readlink()) == os.fsencode(destination)
                and not lossy.exists()
                and {os.fsencode(path.name) for path in physical.iterdir()} == {raw_name, LOCK_NAME.encode()}
                and {path.name for path in requested_parent.iterdir()} == {alias.name},
                "Alias publication replaced its symlink or created a lossy/unrelated store")
        report["initial_catalog"], report["saved_catalog"], report["reloaded_catalog"] = initial, saved, loaded
        report["protected_baseline"] = baseline
        report["final_store"] = {**fingerprint(destination), "value": expected_file}
        report["clean_eof"] = require_clean_eof(client)
        require(fingerprint(source) == baseline["source"] and read_store(destination) == saved_bytes
                and alias.is_symlink() and not lossy.exists()
                and {os.fsencode(path.name) for path in physical.iterdir()} == {raw_name, LOCK_NAME.encode()}
                and {path.name for path in requested_parent.iterdir()} == {alias.name},
                "Alias EOF changed source, target bytes, symlink or destination entries")
    except Exception as error:
        report["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        finish_path_case(clients, report, directory)
    report["status"] = "accepted"


def validate(args):
    clients, fixtures, baselines, settings = [], [], [], []
    result = {"status": "failed", "binary": str(args.binary), "binary_fingerprint": fingerprint(args.binary),
              "script_fingerprint": fingerprint(Path(__file__)),
              "helper_fingerprints": {name: fingerprint(Path(__file__).parent / name) for name in HELPERS},
              "transport": "two coordinated writers plus two isolated path-case private stdio MCP processes",
              "steps": [], "compiles": 0,
              "graphics": False, "native": False, "game": False,
              "path_encoding": {"invalid_requested": {"status": "not_run"},
                                "valid_alias": {"status": "not_run"}}}
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
        record("final-converged-catalogs")
        result["clean_eof"] = []
        for client in clients:
            result["clean_eof"].append(require_clean_eof(client))
        require(read_store(store) is not None and fingerprint(store)["sha256"] == result["final_store"]["sha256"]
                and fingerprints(game) == asset_manifest, "EOF changed accepted storage/assets")
        for index, (source, _) in enumerate(fixtures):
            require(fingerprint(source) == baselines[index]["source"], "EOF rewrote saved source")
        require(len(result["steps"]) == 22 and result["clean_eof"] == [0, 0],
                "Original coordination workflow or its clean exits changed")
        invalid_requested_path(args, game, result["path_encoding"]["invalid_requested"])
        valid_alias_path(args, game, result["path_encoding"]["valid_alias"])
        require(all(case["status"] == "accepted" and case["clean_eof"] == 0
                    for case in result["path_encoding"].values())
                and fingerprints(game) == asset_manifest,
                "Path-encoding cases failed, did not exit cleanly or changed mounted assets")
        path_processes = [case["process_id"] for case in result["path_encoding"].values()]
        result["counts"] = {"coordination_steps": len(result["steps"]), "path_encoding_cases": 2,
                            "processes": len(result["process_ids"]) + len(path_processes),
                            "clean_eof": len(result["clean_eof"]) + sum(
                                case["clean_eof"] == 0 for case in result["path_encoding"].values())}
        require(result["counts"] == {"coordination_steps": 22, "path_encoding_cases": 2,
                                     "processes": 4, "clean_eof": 4},
                "Final report lacks the complete coordination and path-case exits")
        if args.reference_output is not None:
            reference = json.loads((args.reference_output / "results.json").read_text())
            require(reference["status"] == "accepted" and reference["script_fingerprint"] == result["script_fingerprint"]
                    and reference["helper_fingerprints"] == result["helper_fingerprints"]
                    and reference["binary_fingerprint"]["sha256"] != result["binary_fingerprint"]["sha256"]
                    and reference["counts"] == result["counts"]
                    and set(reference["path_encoding"]) == set(result["path_encoding"])
                    and all(case["status"] == "accepted" and case["clean_eof"] == 0
                            for case in reference["path_encoding"].values()),
                    "Cross-program reference lacks the same accepted frozen workflow and path cases")
            previous = Path(reference["final_store"]["path"])
            require(fingerprint(previous)["sha256"] == reference["final_store"]["sha256"]
                    and previous.read_bytes() == read_store(store), "Production programs saved different final recipe bytes")
            prior_alias = reference["path_encoding"]["valid_alias"]
            current_alias = result["path_encoding"]["valid_alias"]
            previous = Path(os.fsdecode(bytes.fromhex(prior_alias["physical_path_hex"])))
            current = Path(os.fsdecode(bytes.fromhex(current_alias["physical_path_hex"])))
            require(fingerprint(previous) == {key: prior_alias["final_store"][key] for key in ("bytes", "sha256")}
                    and previous.read_bytes() == read_store(current)
                    and prior_alias["final_store"]["value"] == current_alias["final_store"]["value"],
                    "Production programs saved different exact non-UTF8 alias target bytes")
            result["cross_program_store_bytes_equal"] = True
            result["cross_program_alias_store_bytes_equal"] = True
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
    parser = argparse.ArgumentParser(description="Validate coordinated recipe publication and requested-path encoding through private headless MCP sessions.")
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
    print(json.dumps({"status": result["status"], "steps": len(result["steps"]),
                      "path_cases": result["counts"]["path_encoding_cases"],
                      "clean_eof": result["counts"]["clean_eof"], "output": str(args.output)}))


if __name__ == "__main__":
    main()
