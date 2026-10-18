import argparse
import fcntl
import json
import os
from contextlib import ExitStack, contextmanager
from pathlib import Path
from unittest.mock import patch

from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_recovery_exit import close_with_status, digest


@contextmanager
def launched(binary, output, gameinfo, exits):
    environment = {
        "HAMMERR_RECENT_MAPS_DIR": str(output / "recent"),
        "HAMMERR_BUILD_RECIPES": str(output / "recipes.json"),
        "HAMMERR_PREFABS_DIR": str(output / "prefabs"),
        "XDG_CONFIG_HOME": str(output / "config"),
        "XDG_STATE_HOME": str(output / "state"),
    }
    with patch.dict(os.environ, environment):
        client = Client(None, binary, output, gameinfo=gameinfo)
    try:
        yield client
    finally:
        if client.process is not None:
            code, trailing = close_with_status(client)
            exits.append({"writer": output.name, "code": code, "trailing_stdout": trailing})
            assert code == 0 and not trailing.strip(), (code, trailing)


def status(client):
    return client.call("get_recovery")["status"]


def protected(client, block):
    return {
        "object": detail(client, [block]),
        "history": client.call("get_history"),
        "camera": client.call("get_camera"),
        "selection": client.call("get_selection"),
        "review": client.call("get_transform_handles"),
    }


def validate(binary, output, result):
    checks = result["checks"]

    def check(condition, label):
        assert condition, label
        checks.append(label)

    game = output / "game"
    game.mkdir()
    gameinfo = game / "gameinfo.txt"
    gameinfo.write_text('GameInfo { FileSystem { SearchPaths { game "|gameinfo_path|." } } }\n')
    shared = output / "shared"
    shared.mkdir()
    target = shared / "shared-settings.json"
    target.write_text(json.dumps({"enabled": False, "interval_seconds": 120, "keep_per_map": 10}))
    writers = [output / "writer-a", output / "writer-b"]
    for writer in writers:
        (writer / "recovery").mkdir(parents=True)
        (writer / "recovery/settings.json").symlink_to(target)

    with ExitStack() as stack:
        first, second = [
            stack.enter_context(launched(binary, writer, gameinfo, result["exits"]))
            for writer in writers
        ]
        block = second.call(
            "create_block",
            {"mins": [0, 0, 0], "maxs": [64, 64, 64], "material": "audit/no-assets-needed"},
        )["id"]
        second.call("transform_objects", {"ids": [block], "translation": [16, 0, 0], "preview": True})
        original = protected(second, block)
        initial = status(second)
        first.call("set_recovery_settings", {"interval_seconds": 60, "keep_per_map": 4})
        saved = target.read_bytes()
        second.call("set_recovery_settings", {"keep_per_map": 3}, error=True)
        conflicted = status(second)
        check(
            conflicted["settings"] == initial["settings"]
            and conflicted["settings_revision"] == initial["settings_revision"]
            and "Reload settings" in conflicted["settings_error"]
            and target.read_bytes() == saved,
            "A separate stale process preserves local settings/revision and saved bytes",
        )
        second.call("reload_recovery_settings", {"expected_revision": initial["settings_revision"]})
        repaired = second.call("set_recovery_settings", {"keep_per_map": 3})
        check(
            repaired["settings"] == {"enabled": False, "interval_seconds": 60, "keep_per_map": 3}
            and repaired["settings_error"] is None,
            "Explicit Reload and partial retry retain the other writer's fields",
        )
        with (shared / ".recovery-settings.lock").open("r+b") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
            saved = target.read_bytes()
            second.call("set_recovery_settings", {"interval_seconds": 90}, error=True)
            busy = status(second)
            check(
                "busy" in busy["settings_error"]
                and busy["settings_revision"] == repaired["settings_revision"]
                and target.read_bytes() == saved,
                "An independent physical-destination lock rejects alias writers without committing",
            )
        retried = second.call("set_recovery_settings", {"interval_seconds": 90})
        check(
            retried["settings"]["interval_seconds"] == 90 and retried["settings_error"] is None,
            "Lock contention can retry directly after the lock is released",
        )
        for invalid in [b"malformed settings", b" " * 32_769]:
            target.write_bytes(invalid)
            second.call("reload_recovery_settings", error=True)
            refused = status(second)
            check(
                refused["settings"] == retried["settings"]
                and refused["settings_revision"] == retried["settings_revision"]
                and target.read_bytes() == invalid,
                "Failed bounded Reload preserves effective settings and observed invalid bytes",
            )
            retried = second.call("set_recovery_settings", {"enabled": False})
            check(
                retried["settings_error"] is None
                and json.loads(target.read_text()) == retried["settings"],
                "Explicit validated Apply repairs an observed malformed or oversized source",
            )
        first.call("reload_recovery_settings")
        first_root = writers[0] / "recovery"
        first_root.rename(writers[0] / "retired-recovery")
        first_root.mkdir()
        (first_root / "settings.json").symlink_to(target)
        saved = target.read_bytes()
        first.call("set_recovery_settings", {"keep_per_map": 5}, error=True)
        check(
            "directory" in status(first)["settings_error"] and target.read_bytes() == saved,
            "Replacing an originally requested recovery directory cannot retarget a settings write",
        )
        first.call("reload_recovery_settings")
        first.call("set_recovery_settings", {"keep_per_map": 5})
        second.call("reload_recovery_settings")
        saved = target.read_bytes()
        shared.rename(output / "retired-shared")
        shared.mkdir()
        target.write_bytes(saved)
        second.call("set_recovery_settings", {"keep_per_map": 6}, error=True)
        check(
            "directory" in status(second)["settings_error"] and target.read_bytes() == saved,
            "Replacing the physical symlink-target directory cannot redirect publication",
        )
        second.call("reload_recovery_settings")
        second.call("set_recovery_settings", {"keep_per_map": 6})
        check(
            all((writer / "recovery/settings.json").is_symlink() for writer in writers)
            and json.loads(target.read_text())["keep_per_map"] == 6,
            "Reload admits the new directory explicitly while preserving regular-file symlinks",
        )
        check(
            protected(second, block) == original,
            "Conflicts, contention, repair and Reload preserve map/history/camera and the owned review",
        )
        check(
            all(not client.call("get_map_info")["rendering"]["initialized"] for client in [first, second]),
            "Recovery settings operations never initialize graphics",
        )
        result["final"] = [status(first), status(second)]


def main():
    parser = argparse.ArgumentParser(description="Validate coordinated recovery settings using two private stdio processes.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    binary = args.binary.resolve()
    result = {
        "status": "running",
        "binary": str(binary),
        "binary_sha256": digest(binary),
        "transport": "private_stdio",
        "desktop_input": False,
        "graphics_initialized": False,
        "checks": [],
        "exits": [],
    }
    try:
        validate(binary, output, result)
        result["status"] = "passed"
    except Exception as error:
        result.update({"status": "failed", "error": f"{type(error).__name__}: {error}"})
        raise
    finally:
        (output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps({"status": result["status"], "checks": len(result["checks"]), "output": str(output)}))


if __name__ == "__main__":
    main()
