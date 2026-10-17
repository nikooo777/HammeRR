import argparse
import fcntl
import json
import tempfile
from contextlib import contextmanager
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client, detail


@contextmanager
def launched(binary, output, stage):
    client = Client(None, binary, output)
    try:
        yield client
    finally:
        client.close()
        (output / "editor.log").rename(output / f"editor-{stage}.log")


def map_state(client, ids):
    info = client.call("get_map_info")
    return {
        **{
            key: info[key]
            for key in [
                "path",
                "unsaved_changes",
                "world_brushes",
                "selection",
                "face_selection",
                "undo",
                "redo",
            ]
        },
        "generation": info["recovery"]["generation"],
        "revision": info["recovery"]["revision"],
        "objects": detail(client, ids),
    }


def validate_conflicts(binary, output, check):
    preferences = output / "shared-favorites"
    preferences.mkdir()
    first_output = output / "writer-a"
    second_output = output / "writer-b"
    for directory in [first_output, second_output]:
        directory.mkdir()
        (directory / "recovery").symlink_to(preferences, target_is_directory=True)
    store = preferences / "material-favorites.json"

    def favorite(client, material, enabled=True, error=False):
        return client.call(
            "set_material_favorite",
            {"material": material, "favorite": enabled},
            error=error,
        )

    def favorites(client):
        return client.call("browse_materials", {"group": "favorites"})

    with launched(binary, first_output, "a") as first:
        with launched(binary, second_output, "b") as second:
            check(not store.exists(), "Two editors load the same missing favorite store")
            favorite(first, "first")
            saved = store.read_bytes()
            favorite(second, "second", error=True)
            status = favorites(second)["favorites"]
            check(
                store.read_bytes() == saved
                and status["count"] == 0
                and status["persistence"] == "blocked"
                and "Reload favorites" in status["error"],
                "A stale writer preserves saved and local favorites with a visible conflict",
            )
            block = second.call(
                "create_block",
                {"mins": [0, 0, 0], "maxs": [64, 64, 64], "material": "test/base"},
            )["id"]
            second.call(
                "transform_objects",
                {"ids": [block], "translation": [16, 0, 0], "preview": True},
            )
            before = map_state(second, [block])
            owned = second.call("get_transform_handles")
            reloaded = second.call("reload_material_favorites")["favorites"]
            check(
                reloaded["count"] == 1
                and reloaded["error"] is None
                and reloaded["reload_available"],
                "Explicit Reload adopts another editor's saved favorites and clears conflict",
            )
            favorite(second, "second")
            favorite(first, "first", False, error=True)
            first.call("reload_material_favorites")
            favorite(first, "first", False)
            second.call("reload_material_favorites")
            check(
                {entry["name"] for entry in favorites(second)["entries"]} == {"second"},
                "Reload plus explicit retry retains concurrent additions and removals",
            )
            with (preferences / ".material-favorites.lock").open("r+b") as lock:
                fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
                saved = store.read_bytes()
                favorite(second, "third", error=True)
                status = favorites(second)["favorites"]
                check(
                    store.read_bytes() == saved
                    and status["persistence"] == "persistent"
                    and "busy" in status["error"],
                    "Independent lock contention preserves data and remains retryable",
                )
            favorite(second, "third")
            valid = store.read_bytes()
            store.write_bytes(b"malformed favorites")
            second.call("reload_material_favorites", error=True)
            check(
                favorites(second)["favorites"]["count"] == 2
                and favorites(second)["favorites"]["persistence"] == "blocked"
                and store.read_bytes() == b"malformed favorites",
                "Invalid Reload retains local favorites and cannot overwrite malformed storage",
            )
            store.write_bytes(valid)
            second.call("reload_material_favorites")
            check(
                favorites(second)["favorites"]["error"] is None,
                "Repair followed by Reload restores persistent editing",
            )
            check(
                map_state(second, [block]) == before
                and second.call("get_transform_handles") == owned,
                "Favorite conflict, writes and Reload preserve the owned transform and map/history",
            )
            check(
                not second.call("get_map_info")["rendering"]["initialized"],
                "Favorite persistence and Reload never initialize graphics",
            )


def validate(binary, output):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def browse(client):
        return client.call("browse_materials", {"group": "favorites"})

    def favorite(client, material, enabled=True, error=False):
        return client.call(
            "set_material_favorite",
            {"material": material, "favorite": enabled},
            error=error,
        )

    store = output / "recovery" / "material-favorites.json"
    with launched(binary, output, "save") as client:
        tools = {tool["name"] for tool in client.rpc("tools/list", {})["tools"]}
        check("set_material_favorite" in tools, "Favorite operation is discoverable")
        check("reload_material_favorites" in tools, "Favorite Reload is discoverable")
        initial = browse(client)
        check(initial["total"] == 0, "Temporary store starts empty")
        status = initial["favorites"]
        check(status["persistence"] == "persistent", "Configured storage is reported")
        check(
            status["max_favorites"] == 512
            and status["max_material_bytes"] == 256
            and status["max_file_bytes"] == 192 * 1024,
            "Storage bounds are inspectable",
        )
        object_id = client.call(
            "create_block",
            {
                "mins": [0, 0, 0],
                "maxs": [64, 64, 64],
                "material": "Materials\\BRICK\\WALL.VMT",
            },
        )["id"]
        ids = [object_id]
        client.call("set_material", {"ids": ids, "material": "wood/floor"})
        client.call("undo")
        before = map_state(client, ids)
        result = favorite(client, " Materials\\BRICK\\WALL.VMT ")
        check(result["material"] == "brick/wall", "Favorite paths are canonicalized")
        original = store.read_bytes()
        favorite(client, "BRICK/WALL")
        check(store.read_bytes() == original, "Repeated favorite is a no-op")
        favorite(client, "custom/unmounted")
        catalog = browse(client)
        check(
            catalog["total"] == 2
            and catalog["favorites"]["count"] == 2
            and all(entry["favorite"] for entry in catalog["entries"]),
            "Favorites and counts include unmounted materials",
        )
        check(
            any(entry["name"] == "brick/wall" and entry["used"] for entry in catalog["entries"]),
            "Canonical favorites merge with imported map material paths",
        )
        for invalid in [
            "../wall",
            "/brick/wall",
            "C:\\brick\\wall",
            "brick/*",
            "bad\n",
            "x" * 257,
            "materials/materials/brick/wall",
            "brick/wall.vmt.vmt",
            "materials/ brick/wall",
            "brick/wall .vmt",
        ]:
            original = store.read_bytes()
            favorite(client, invalid, error=True)
            check(
                store.read_bytes() == original,
                f"Invalid path preserves preferences: {invalid!r}",
            )
        original = store.read_bytes()
        mode = store.stat().st_mode
        store.chmod(0o444)
        try:
            favorite(client, "wood/floor", error=True)
            check(store.read_bytes() == original, "Failed save preserves previous file")
            failed = browse(client)
            check(failed["total"] == 2, "Failed save preserves Session favorites")
            check(
                "could not save" in failed["favorites"]["error"],
                "Save failure is inspectable",
            )
            check(
                map_state(client, ids) == before,
                "Favorite actions preserve map and undo/redo",
            )
        finally:
            store.chmod(mode)
        favorite(client, "wood/floor")
        check(
            browse(client)["favorites"]["error"] is None,
            "Successful retry clears failure",
        )
        check(
            map_state(client, ids) == before,
            "Successful retry preserves map and history",
        )

    with launched(binary, output, "restart") as client:
        catalog = browse(client)
        check(catalog["total"] == 3, "Favorites survive a process restart")
        check(
            {entry["name"] for entry in catalog["entries"]}
            == {"brick/wall", "custom/unmounted", "wood/floor"},
            "Restart retains canonical favorites",
        )
        check(
            client.call("browse_materials", {"group": "recent"})["total"] == 0,
            "Recent applications remain session-local",
        )
        favorite(client, "Materials/BRICK/WALL.vmt", False)
        check(browse(client)["total"] == 2, "Canonical removal persists")

    for stage, contents in [
        ("malformed", b"broken preferences"),
        ("version", b'{"version":2,"materials":[]}'),
        ("oversized", b" " * (192 * 1024 + 1)),
    ]:
        store.write_bytes(contents)
        with launched(binary, output, stage) as client:
            catalog = browse(client)
            check(
                catalog["favorites"]["persistence"] == "blocked",
                f"{stage}: failed load is reported",
            )
            check(
                bool(catalog["favorites"]["error"]),
                f"{stage}: failure includes a reason",
            )
            before = map_state(client, [])
            favorite(client, "brick/wall", error=True)
            check(
                store.read_bytes() == contents,
                f"{stage}: editing cannot overwrite rejected file",
            )
            check(
                map_state(client, []) == before,
                f"{stage}: rejection preserves map and history",
            )
            store.write_text('{"version":1,"materials":["repaired"]}')
            repaired = client.call("reload_material_favorites")["favorites"]
            check(
                repaired["count"] == 1 and repaired["error"] is None,
                f"{stage}: repaired preferences can be reloaded without restarting",
            )

    store.write_text(
        json.dumps({"version": 1, "materials": [f"test/{index}" for index in range(512)]})
    )
    with launched(binary, output, "limit") as client:
        check(browse(client)["total"] == 512, "Maximum favorite count loads")
        original = store.read_bytes()
        favorite(client, "test/0")
        favorite(client, "extra/wall", error=True)
        check(store.read_bytes() == original, "Count limit preserves existing file")
        favorite(client, "test/0", False)
        favorite(client, "extra/wall")
        check(browse(client)["total"] == 512, "Removal frees capacity")

    (output / "recovery" / "settings.json").write_text("broken recovery settings")
    with launched(binary, output, "independent") as client:
        check(
            browse(client)["total"] == 512,
            "Corrupt recovery settings do not block favorites",
        )
        favorite(client, "test/1", False)
        check(
            browse(client)["total"] == 511,
            "Favorites still save with corrupt recovery settings",
        )
    validate_conflicts(binary, output, check)
    return checks


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, default=Path("/tmp/hammerr-material-favorites"))
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    run = Path(tempfile.mkdtemp(prefix="run-", dir=args.output))
    checks = validate(args.binary.resolve(), run)
    (run / "results.json").write_text(json.dumps({"checks": checks}, indent=2))
    print(json.dumps({"checks": len(checks), "output": str(run)}))


if __name__ == "__main__":
    main()
