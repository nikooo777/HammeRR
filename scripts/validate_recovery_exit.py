import argparse
import hashlib
import json
import os
import re
import subprocess
from pathlib import Path
from unittest.mock import patch

from validate_face_lighting import Client, detail

ROOT = Path(__file__).resolve().parents[1]


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def close_with_status(client):
    process = client.process
    process.stdin.close()
    try:
        code = process.wait(timeout=15)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5)
        raise TimeoutError("Headless EOF failed to drain within 15 seconds")
    finally:
        client.log.close()
        client.process = None
    return code, process.stdout.read()


def validate_case(binary, output, fail):
    output.mkdir()
    game = output / "game"
    game.mkdir()
    gameinfo = game / "gameinfo.txt"
    gameinfo.write_text('GameInfo { FileSystem { SearchPaths { game "|gameinfo_path|." } } }\n')
    recovery = output / "recovery"
    env = {"HAMMERR_RECENT_MAPS_DIR": str(output / "recent"),
           "HAMMERR_BUILD_RECIPES": str(output / "recipes.json"),
           "HAMMERR_PREFABS_DIR": str(output / "prefabs"),
           "XDG_CONFIG_HOME": str(output / "config"), "XDG_STATE_HOME": str(output / "state")}
    client = None
    result = {"binary": str(binary), "binary_sha256": digest(binary), "storage_failure": fail}
    try:
        with patch.dict(os.environ, env):
            client = Client(None, binary, output, gameinfo=gameinfo)
        parent = output / "parent.vmf"
        client.call("save_map", {"path": str(parent)})
        source_sha256 = digest(parent)
        block = client.call("create_block", {"mins": [0, 0, 0], "maxs": [16, 16, 16],
            "material": "audit/no-assets-needed"})["id"]
        original = {"object": detail(client, [block]), "history": client.call("get_history"),
                    "camera": client.call("get_camera"), "selection": client.call("get_selection")}
        before = client.call("get_recovery")
        assert before["status"]["pending_changes"] and not before["status"]["busy"]
        assert before["status"]["settings"]["enabled"] and before["status"]["last_snapshot"] is None
        assert not client.call("get_map_info")["rendering"]["initialized"]
        result["before_eof"] = before
        if fail:
            assert not recovery.exists()
            recovery.write_text("Private deterministic storage obstruction.\n")
            rejected = client.call("create_recovery_snapshot", error=True)
            failed = client.call("get_recovery")
            assert failed["status"]["error"] and failed["status"]["pending_changes"]
            assert not failed["status"]["busy"] and failed["status"]["last_snapshot"] is None
            assert {"object": detail(client, [block]), "history": client.call("get_history"),
                    "camera": client.call("get_camera"), "selection": client.call("get_selection")} == original
            result["manual_failure"], result["failed_status"] = rejected, failed
        code, trailing = close_with_status(client)
        result["exit_code"], result["trailing_stdout"] = code, trailing
        log = (output / "editor.log").read_text()
        result["stderr_sha256"] = digest(output / "editor.log")
        assert digest(parent) == source_sha256
        assert not trailing.strip(), trailing
        if fail:
            assert code != 0 and "Exit recovery snapshot failed" in log, (code, log)
            assert recovery.is_file() and recovery.read_text() == "Private deterministic storage obstruction.\n"
        else:
            assert code == 0, log
            manifests = [path for path in recovery.glob("*.json") if path.name != "settings.json"]
            assert len(manifests) == 1, manifests
            entry = json.loads(manifests[0].read_text())
            snapshot = recovery / f"{entry['id']}.vmf"
            text = snapshot.read_text()
            assert entry["kind"] == "autosave" and entry["source_path"] == str(parent)
            assert entry["bytes"] == snapshot.stat().st_size
            assert len(re.findall(r'(?m)^\s*"?solid"?\s*$', text)) == 1
            assert len(re.findall(r'(?m)^\s*"material"\s+"AUDIT/NO-ASSETS-NEEDED"\s*$', text)) == 6
            assert "Exit recovery snapshot failed" not in log and "Could not drain recovery" not in log
            result["snapshot"], result["snapshot_sha256"] = entry, digest(snapshot)
        result["parent_sha256"] = source_sha256
        result["status"] = "passed"
    except Exception as error:
        result["status"] = "failed"
        result["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        if client is not None and client.process is not None:
            client.close()
        (output / "case.json").write_text(json.dumps(result, indent=2) + "\n")
    return result


def main():
    parser = argparse.ArgumentParser(description="Validate explicit recovery drain and process status on private MCP EOF, without GPU or game execution.")
    parser.add_argument("--headless-binary", type=Path, default=ROOT / "target/debug/hammerr-headless")
    parser.add_argument("--editor-binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    result = {"status": "running", "transport": "private_stdio_eof", "native_exercised": False,
              "desktop_input": False, "graphics_initialized": False, "cases": []}
    try:
        for label, binary in [("standalone", args.headless_binary), ("editor-headless", args.editor_binary)]:
            for fail in [False, True]:
                case = output / f"{label}-{'failure' if fail else 'success'}"
                result["cases"].append(validate_case(binary.resolve(), case, fail))
        result["cases"].append(validate_error_domains(args.headless_binary.resolve(), output / "error-domains"))
        result["status"] = "passed"
    except Exception as error:
        result["status"] = "failed"
        result["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        (output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps({"status": result["status"], "cases": len(result["cases"]), "output": str(output)}))


def validate_error_domains(binary, output):
    output.mkdir()
    game = output / "game"
    game.mkdir()
    gameinfo = game / "gameinfo.txt"
    gameinfo.write_text('GameInfo { FileSystem { SearchPaths { game "|gameinfo_path|." } } }\n')
    recovery = output / "recovery"
    recovery.mkdir()
    settings = recovery / "settings.json"
    settings.write_text("malformed settings")
    env = {"HAMMERR_RECENT_MAPS_DIR": str(output / "recent"),
           "HAMMERR_BUILD_RECIPES": str(output / "recipes.json"),
           "HAMMERR_PREFABS_DIR": str(output / "prefabs"),
           "XDG_CONFIG_HOME": str(output / "config"), "XDG_STATE_HOME": str(output / "state")}
    client = None
    result = {"binary": str(binary), "binary_sha256": digest(binary), "case": "error_domains"}
    try:
        with patch.dict(os.environ, env):
            client = Client(None, binary, output, gameinfo=gameinfo)
        parent = output / "parent.vmf"
        client.call("save_map", {"path": str(parent)})
        parent_digest = digest(parent)
        block = client.call("create_block", {"mins": [0, 0, 0], "maxs": [16, 16, 16], "material": "audit/no-assets-needed"})["id"]

        def protected():
            return {"object": detail(client, [block]), "history": client.call("get_history"),
                    "camera": client.call("get_camera"), "selection": client.call("get_selection")}

        original = protected()
        initial = client.call("get_recovery")["status"]
        assert initial["settings_error"] and not initial["settings"]["enabled"]
        client.call("create_recovery_snapshot")
        captured = client.call("get_recovery")["status"]
        assert captured["settings_error"] == initial["settings_error"]
        assert captured["error"] == initial["settings_error"] and captured["snapshot_error"] is None
        client.call("set_recovery_settings", {"enabled": True})
        assert client.call("get_recovery")["status"]["error"] is None
        retained = output / "retained-recovery"
        recovery.rename(retained)
        recovery.write_text("temporary directory obstruction")
        client.call("create_recovery_snapshot", error=True)
        failed = client.call("get_recovery")["status"]
        assert failed["snapshot_error"] and not failed["busy"]
        recovery.unlink()
        retained.rename(recovery)
        client.call("set_recovery_settings", {"interval_seconds": 60})
        repaired = client.call("get_recovery")["status"]
        assert repaired["settings_error"] is None and repaired["snapshot_error"] == failed["snapshot_error"]
        assert repaired["error"] == failed["snapshot_error"]
        saved_settings = {"enabled": False, "interval_seconds": 90, "keep_per_map": 3}
        settings.write_text(json.dumps(saved_settings))
        reloaded = client.call("reload_recovery_settings", {"expected_revision": repaired["settings_revision"]})
        assert reloaded["settings"] == saved_settings
        assert reloaded["snapshot_error"] == failed["snapshot_error"] and reloaded["settings_error"] is None
        client.call("reload_recovery_settings", {"expected_revision": repaired["settings_revision"]}, error=True)
        assert client.call("get_recovery")["status"]["settings_revision"] == reloaded["settings_revision"]
        for invalid in [b"malformed settings", b" " * 32_769]:
            settings.write_bytes(invalid)
            client.call("reload_recovery_settings", error=True)
            refused = client.call("get_recovery")["status"]
            assert refused["settings"] == saved_settings and refused["settings_revision"] == reloaded["settings_revision"]
            assert refused["settings_error"] and refused["snapshot_error"] == failed["snapshot_error"]
            assert settings.read_bytes() == invalid and protected() == original
        settings.write_text(json.dumps(saved_settings))
        client.call("reload_recovery_settings")
        assert client.call("get_recovery")["status"]["settings_error"] is None
        client.call("create_recovery_snapshot")
        final = client.call("get_recovery")["status"]
        assert all(final[field] is None for field in ["error", "settings_error", "snapshot_error", "retention_error", "drain_error"])
        assert protected() == original and digest(parent) == parent_digest
        assert not client.call("get_map_info")["rendering"]["initialized"]
        code, trailing = close_with_status(client)
        assert code == 0 and not trailing.strip(), (code, trailing)
        result.update({"status": "passed", "exit_code": code, "initial": initial,
                       "captured": captured, "failed": failed, "repaired": repaired,
                       "reloaded": reloaded, "reload_refused": refused, "final": final})
    except Exception as error:
        result.update({"status": "failed", "error": f"{type(error).__name__}: {error}"})
        raise
    finally:
        if client is not None and client.process is not None:
            client.close()
        (output / "case.json").write_text(json.dumps(result, indent=2) + "\n")
    return result


if __name__ == "__main__":
    main()
