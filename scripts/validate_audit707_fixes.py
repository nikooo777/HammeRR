import argparse
import hashlib
import json
import os
import re
from pathlib import Path

from validate_compile_jobs import wait_job
from validate_face_lighting import DEFAULT_BINARY, Client
from validate_material_albedo import require
from validate_recovery_exit import require_clean_eof


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def open_client(binary, directory, game, clients, recovery=None):
    directory.mkdir()
    temporary = directory / "tmp"
    temporary.mkdir()
    environment = {
        "XDG_CONFIG_HOME": str(directory / "config"),
        "XDG_STATE_HOME": str(directory / "state"),
        "HAMMERR_RECENT_MAPS_DIR": str(directory / "recent"),
        "HAMMERR_BUILD_RECIPES": str(directory / "recipes.json"),
        "HAMMERR_PREFABS_DIR": str(directory / "prefabs"),
        "TMPDIR": str(temporary),
    }
    if recovery is not None:
        environment["HAMMERR_RECOVERY_DIR"] = str(recovery)
    client = Client.__new__(Client)
    client.process, client.log = None, None
    clients.append((client, directory))
    Client.__init__(client, None, binary, directory, game / "gameinfo.txt", environment=environment)
    return client


def protected(client, source):
    info = client.call("get_map_info")
    require(not info["rendering"]["initialized"], "Validation initialized graphics")
    return {"source": digest(source), "history": client.call("get_history"),
            "revision": info["recovery"]["revision"],
            **{key: info[key] for key in ("generation", "unsaved_changes", "selection", "camera")}}


def compiler_case(binary, directory, game, clients):
    directory.mkdir()
    client = open_client(binary, directory / "client", game, clients)
    for low, high in [
        ([-2.125, -34, -2], [2.125, 34, 0]),
        ([-2.125, -34, 32], [2.125, 34, 34]),
        ([-2.125, -34, 0], [-0.125, 34, 32]),
        ([0.125, -34, 0], [2.125, 34, 32]),
        ([-0.125, -34, 0], [0.125, -32, 32]),
        ([-0.125, 32, 0], [0.125, 34, 32]),
    ]:
        client.call("create_block", {"mins": low, "maxs": high, "material": "audit/white", "snap": False})
    client.call("create_entity", {"classname": "info_player_terrorist", "origin": [0, 0, 8]})
    client.call("create_entity", {"classname": "light", "origin": [0, 0, 16],
                                  "properties": {"_light": "255 255 255 12"}})
    source = directory / "source.vmf"
    client.call("save_map", {"path": str(source)})
    target = directory / "map.bsp"
    client.call("set_compile_settings", {"lighting": "direct_ldr", "physics": "none", "bounces": 0,
                                         "generate_reflections": False, "pack_sky_assets": False,
                                         "bake_static_prop_vertices": False})
    require(client.call("compile_map", {"path": str(target)})["success"], "Ordinary thin-room control failed")
    retained = target.read_bytes()
    text, count = re.subn(r'"uaxis"\s+"[^"]+"', '"uaxis" "[64000 0 0 0] 0.25"',
                          source.read_text(), count=1)
    require(count == 1, "Missing floor U axis")
    text, count = re.subn(r'"vaxis"\s+"[^"]+"', '"vaxis" "[0 -0.001 0 6.4] 0.25"', text, count=1)
    require(count == 1, "Missing floor V axis")
    imported = directory / "finite-cut.vmf"
    imported.write_text(text)
    client.call("open_map", {"path": str(imported)})
    failures = []
    for lighting in ("fullbright", "direct_ldr"):
        client.call("set_compile_settings", {"lighting": lighting})
        before = protected(client, imported)
        pointfile = target.with_suffix(".lin")
        pointfile.write_bytes(b"retained pointfile\n")
        response = client.call("compile_map", {"path": str(target)}, error=True)
        result = json.loads(response["content"][0]["text"])
        require(not result["success"] and result["bsp"] is None, "Failed subdivision returned a BSP")
        errors = [row["message"] for row in result["diagnostics"] if row["level"] == "error"]
        require(any("lightmap" in error.lower() and "side:" in error for error in errors),
                f"Missing source-owned subdivision diagnostic: {errors}")
        require(target.read_bytes() == retained and pointfile.read_bytes() == b"retained pointfile\n",
                "Foreground refusal replaced prior output")
        client.call("start_compile", {"path": str(target)})
        job = wait_job(client)
        require(job["status"] == "failed" and not job["result"]["published"], "Background refusal published output")
        require(target.read_bytes() == retained and pointfile.read_bytes() == b"retained pointfile\n",
                "Background refusal replaced prior output")
        require(protected(client, imported) == before, "Compilation refusal changed document state")
        failures.append({"lighting": lighting, "diagnostics": errors, "job": job})
    return {"control_sha256": hashlib.sha256(retained).hexdigest(), "failures": failures}


def configuration_case(binary, directory, game, clients):
    directory.mkdir()
    raw = os.fsencode(directory) + b"/recovery-\xff"
    requested = Path(os.fsdecode(raw))
    client = open_client(binary, directory / "client", game, clients, requested)
    status = client.call("get_recovery")["status"]
    require(not status["available"] and status["directory"] is None
            and "UTF-8" in status["error"], "Invalid requested storage did not expose unavailable status")
    info = client.call("get_map_info")
    require(not info["rendering"]["initialized"], "Configuration inspection initialized graphics")
    for tool in ("get_grid_defaults", "get_preview_defaults", "get_navigation_settings"):
        report = client.call(tool)
        require(report["path"] is None and report["error"], f"{tool} did not report unavailable storage")
    require(not requested.exists() and not (directory / "recovery-\ufffd").exists(),
            "Rejected configuration created requested or substituted storage")
    return {"requested_bytes": raw.hex(), "status": status}


def recovery_case(binary, directory, game, clients):
    directory.mkdir()
    physical_store = directory / os.fsdecode(b"storage-\xff")
    physical_store.mkdir()
    storage = directory / "recovery"
    storage.symlink_to(physical_store, target_is_directory=True)
    physical = directory / os.fsdecode(b"physical-\xff.vmf")
    physical.write_text("world { id 1 classname worldspawn }\n")
    source = directory / "source.vmf"
    source.symlink_to(physical)
    client = open_client(binary, directory / "client", game, clients, storage)
    client.call("open_map", {"path": str(source)})
    client.call("create_block", {"mins": [0, 0, 0], "maxs": [16, 16, 16], "material": "audit/white"})
    manual = client.call("create_recovery_snapshot")["snapshot"]
    require(manual["source_path"] == str(source), "Recovery lost the usable authored alias")
    require(manual["document_key"] == "map-bytes:" + os.fsencode(physical).hex(),
            "Recovery did not retain lossless physical identity")
    client.call("save_map")
    listing = client.call("get_recovery")
    require({row["kind"] for row in listing["snapshots"]} == {"manual", "before_save"},
            "Manual or previous-save recovery entry missing")
    require(listing["status"]["error"] is None and source.is_symlink() and storage.is_symlink(),
            "Successful alias recovery retained an error or replaced an alias")
    client.call("create_block", {"mins": [32, 0, 0], "maxs": [48, 16, 16], "material": "audit/white"})
    exit_eof = require_clean_eof(client)
    entries = [json.loads(path.read_text()) for path in storage.glob("*.json") if path.name != "settings.json"]
    exit_entries = [row for row in entries if row.get("kind") == "autosave"]
    require(len(exit_entries) == 1 and exit_entries[0]["source_path"] == str(source),
            "EOF did not create recovery through the authored alias")
    second = open_client(binary, directory / "restore-client", game, clients, storage)
    second.call("restore_recovery_snapshot", {"id": exit_entries[0]["id"]})
    info = second.call("get_map_info")
    require(info["path"] is None and info["unsaved_changes"] and info["world_brushes"] == 2,
            "Recovered alias snapshot did not restore the committed map as an unnamed copy")
    require(not info["rendering"]["initialized"], "Recovery restore initialized graphics")
    return {"manual": manual, "exit": exit_entries[0], "exit_eof": exit_eof,
            "kinds": sorted(row["kind"] for row in entries)}


def main():
    parser = argparse.ArgumentParser(description="Validate corrected lightmap and recovery invariants via private headless MCP.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    game = output / "game"
    (game / "materials/audit").mkdir(parents=True)
    (game / "gameinfo.txt").write_text('GameInfo { FileSystem { SearchPaths { game "|gameinfo_path|." } } }\n')
    (game / "materials/audit/white.vmt").write_text("LightmappedGeneric { }\n")
    clients = []
    result = {"status": "failed", "binary": str(args.binary.resolve()), "binary_sha256": digest(args.binary), "cleanup": []}
    try:
        result["compiler"] = compiler_case(args.binary.resolve(), output / "compiler", game, clients)
        result["configuration"] = configuration_case(args.binary.resolve(), output / "configuration", game, clients)
        result["recovery"] = recovery_case(args.binary.resolve(), output / "recovery", game, clients)
        result["status"] = "passed"
    except Exception as error:
        result["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        cleanup_errors = []
        for client, directory in clients:
            try:
                if client.process is not None:
                    result["cleanup"].append({"directory": str(directory), "eof": require_clean_eof(client)})
                elif client.log is not None and not client.log.closed:
                    client.log.close()
                log = directory / "editor.log"
                require(not log.exists() or "panicked" not in log.read_text(errors="replace"), f"Panic in {log}")
            except Exception as error:
                cleanup_errors.append(f"{directory}: {error}")
        if cleanup_errors:
            result["status"], result["cleanup_errors"] = "failed", cleanup_errors
        (output / "results.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps({"status": result["status"], "output": str(output)}))
        if cleanup_errors:
            raise RuntimeError("; ".join(cleanup_errors))


if __name__ == "__main__":
    main()
