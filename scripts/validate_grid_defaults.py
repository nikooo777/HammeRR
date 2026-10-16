import argparse
import hashlib
import json
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client


def validate(binary, output):
    checks = []

    def check(condition, name):
        if not condition:
            raise AssertionError(name)
        checks.append(name)

    def change(client, action, error=False):
        state = client.call("get_grid_defaults")
        return client.call(
            "set_grid_defaults",
            {"action": action, "snapshot": state["snapshot"]},
            error=error,
        )

    path = output / "recovery/grid-defaults.json"
    saved = {"grid_size": 17.375, "snap": False, "grid_visible": False}
    client = Client(None, binary, output)
    try:
        state = client.call("get_grid_defaults")
        check(state["ready"] and not state["stored"], "missing file uses factory defaults without writing")
        check(Path(state["path"]) == path, "private storage path")
        original = client.call("get_map_info")
        client.call("set_spatial_settings", {**saved, "elevation": 19.25, "depth": 10.5})
        check(not path.exists(), "temporary MCP changes do not persist")
        stale = client.call("get_grid_defaults")["snapshot"]
        client.call("set_spatial_settings", {"grid_size": 64})
        for action in ["save", "apply", "reload", "reset"]:
            client.call("set_grid_defaults", {"action": action, "snapshot": stale}, error=True)
        check(not path.exists(), "stale actions cannot write preferences")
        client.call("set_spatial_settings", saved)
        change(client, "save")
        check(json.loads(path.read_text()) == {"version": 1, "grid": saved}, "save persists only the grid triple")
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        client.call("set_spatial_settings", {"grid_size": 4096, "snap": True, "grid_visible": True})
        check(hashlib.sha256(path.read_bytes()).hexdigest() == digest, "temporary updates preserve saved bytes")
        change(client, "apply")
        spatial = client.call("get_map_info")["spatial"]
        check(all(spatial[key] == value for key, value in saved.items()), "apply restores exact saved grid")
        check(spatial["elevation"] == 19.25 and spatial["depth"] == 10.5, "apply preserves transient workplane")
        current = client.call("get_map_info")
        for key in ["path", "unsaved_changes", "world_brushes", "point_entities", "undo", "redo"]:
            check(current[key] == original[key], f"grid actions preserve map {key}")
    finally:
        client.close()
    (output / "editor.log").rename(output / "first-startup.log")
    client = Client(None, binary, output)
    try:
        spatial = client.call("get_map_info")["spatial"]
        check(all(spatial[key] == value for key, value in saved.items()), "restart loads saved grid")
        check(spatial["elevation"] == 0 and spatial["depth"] == 128 and not spatial["draw_block"], "restart leaves transient settings at factory values")
        malformed = b'{"version":999,"grid":{"grid_size":16,"snap":true,"grid_visible":true}}'
        path.write_bytes(malformed)
        change(client, "reload", error=True)
        state = client.call("get_grid_defaults")
        check(not state["ready"] and state["error"] and state["defaults"] is None, "unsupported preferences remain inspectable")
        change(client, "save", error=True)
        change(client, "apply", error=True)
        check(path.read_bytes() == malformed, "failed actions preserve unsupported bytes")
    finally:
        client.close()
    (output / "editor.log").rename(output / "second-startup.log")
    client = Client(None, binary, output)
    try:
        state = client.call("get_grid_defaults")
        check(not state["ready"] and state["error"], "startup storage error stays visible")
        check(path.read_bytes() == malformed, "startup never repairs unsupported file")
        client.call("set_spatial_settings", {"grid_size": 32})
        change(client, "reset")
        check(client.call("get_map_info")["spatial"]["grid_size"] == 32, "reset saved defaults does not apply them")
        change(client, "apply")
        check(client.call("get_map_info")["spatial"]["grid_size"] == 16, "factory defaults apply explicitly")
        path.write_bytes(json.dumps({"version": 1, "grid": saved}).encode())
        change(client, "reload")
        check(client.call("get_map_info")["spatial"]["grid_size"] == 16, "reload does not apply file")
        change(client, "apply")
        check(client.call("get_map_info")["spatial"]["grid_size"] == 17.375, "retry loads corrected file")
        path.unlink()
        path.mkdir()
        change(client, "save", error=True)
        state = client.call("get_grid_defaults")
        check(not state["ready"] and state["error"] and path.is_dir(), "write failure has no persistence fallback")
        change(client, "reset", error=True)
        check(path.is_dir(), "explicit reset rejects nonregular storage")
        path.rmdir()
        change(client, "reload")
        change(client, "reset")
    finally:
        client.close()
    result = {"transport": "private stdio", "checks": len(checks), "passed": checks}
    (output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({key: value for key, value in result.items() if key != "passed"}))


def main():
    parser = argparse.ArgumentParser(description="Validate explicit grid defaults in isolated headless processes.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True, help="New private directory for preferences and reports")
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    validate(args.binary.resolve(), output)


if __name__ == "__main__":
    main()
