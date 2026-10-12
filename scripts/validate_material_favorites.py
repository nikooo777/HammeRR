import argparse
import json
import tempfile
from contextlib import contextmanager
from pathlib import Path

from validate_face_lighting import ROOT, Client, detail


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
    return checks


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument("--output", type=Path, default=Path("/tmp/hammerr-material-favorites"))
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    run = Path(tempfile.mkdtemp(prefix="run-", dir=args.output))
    checks = validate(args.binary.resolve(), run)
    (run / "results.json").write_text(json.dumps({"checks": checks}, indent=2))
    print(json.dumps({"checks": len(checks), "output": str(run)}))


if __name__ == "__main__":
    main()
