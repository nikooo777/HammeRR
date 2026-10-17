import argparse
import json
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client


def validate(binary, output):
    checks = []

    def check(condition, label):
        if not condition:
            raise AssertionError(label)
        checks.append(label)

    def change(client, action, error=False):
        state = client.call("get_preview_defaults")
        return client.call(
            "set_preview_defaults",
            {"action": action, "snapshot": state["snapshot"]},
            error=error,
        )

    saved = {
        "selection_outlines": False,
        "entity_images": False,
        "entity_model_guides": False,
        "rope_preview": True,
        "sky_preview": True,
    }
    path = output / "recovery/preview-defaults.json"
    client = Client(None, binary, output)
    try:
        state = client.call("get_preview_defaults")
        factory = state["factory_defaults"]
        check(state["ready"] and not state["stored"], "missing file reports factory defaults without writing")
        check(Path(state["path"]) == path, "preferences use private storage")
        client.call("create_block", {"mins": [0, 0, 0], "maxs": [64, 64, 64]})
        original = client.call("get_map_info")
        client.call("set_view", saved)
        check(not path.exists(), "ordinary view changes do not persist automatically")
        snapshot = client.call("get_preview_defaults")["snapshot"]
        client.call("set_view", factory)
        for action in ["save", "apply", "reload", "reset"]:
            client.call("set_preview_defaults", {"action": action, "snapshot": snapshot}, error=True)
        check(not path.exists(), "stale actions cannot write preferences")
        client.call("set_view", {**saved, "sky_channel": "hdr", "sky_exposure_ev": 1.75})
        state = change(client, "save")
        check(state["write_committed"] and state["saved"] == saved, "explicit save reports committed exact booleans")
        check(json.loads(path.read_text()) == {"version": 1, "preview": saved}, "only the five booleans are stored")
        stored_bytes = path.read_bytes()
        before_render = client.call("get_preview_defaults")
        image, metadata = client.render_result(width=257, height=193, **factory, camera={"focus": original["selection"]})
        (output / "temporary-factory-preview.png").write_bytes(image)
        check(all(metadata["view"][key] == value for key, value in factory.items()), "temporary render reports independent effective booleans")
        check(client.call("get_preview_defaults") == before_render and path.read_bytes() == stored_bytes, "temporary rendering preserves current defaults, snapshot and saved bytes")
        client.call("set_view", {**factory, "surface": "wireframe", "tool_textures": False})
        check(path.read_bytes() == stored_bytes, "ordinary toggles preserve saved bytes")
        change(client, "apply")
        view = client.call("get_map_info")["view"]
        check(all(view[key] == value for key, value in saved.items()), "apply restores the five saved settings")
        check(view["surface"] == "wireframe" and not view["tool_textures"] and view["sky_channel"] == "hdr" and view["sky_exposure_ev"] == 1.75, "apply preserves rendering mode, tool filter and sky tuning")
        client.call("draw_block", {"start": [0, 0], "end": [64, 64], "projection": "top", "preview": True})
        preview = client.call("get_map_info")["block_preview"]
        check(preview is not None, "fixture retains an actual geometry preview")
        change(client, "apply", error=True)
        check(client.call("get_map_info")["block_preview"] == preview, "apply rejects without canceling the active preview")
        change(client, "reload")
        check(client.call("get_map_info")["block_preview"] == preview, "reload preserves the active preview")
        client.call("cancel_block_preview")
        current = client.call("get_map_info")
        for key in ["path", "unsaved_changes", "world_brushes", "point_entities", "undo", "redo", "selection"]:
            check(current[key] == original[key], f"preferences preserve map {key}")
    finally:
        client.close()
    (output / "editor.log").rename(output / "first-startup.log")
    client = Client(None, binary, output)
    try:
        state = client.call("get_preview_defaults")
        view = client.call("get_map_info")["view"]
        check(state["current"] == saved and state["stored"], "restart loads saved preview defaults")
        check(view["sky_channel"] == "ldr" and view["sky_exposure_ev"] == 0 and view["surface"] == "textured", "restart leaves unrelated rendering settings at factory values")
        malformed = b'{"version":999,"preview":{}}'
        path.write_bytes(malformed)
        for action in ["save", "apply", "reset"]:
            change(client, action, error=True)
            state = client.call("get_preview_defaults")
            check(not state["ready"] and state["error"], f"outside replacement rejects {action} before reloading")
            check(path.read_bytes() == malformed, f"rejected {action} preserves outside bytes")
            if action != "reset":
                path.write_bytes(malformed + b" ")
                malformed = path.read_bytes()
        change(client, "reload", error=True)
        check(client.call("get_preview_defaults")["defaults"] is None, "malformed defaults stay inspectable without invented values")
        change(client, "save", error=True)
        change(client, "apply", error=True)
        check(path.read_bytes() == malformed, "malformed storage remains unchanged")
    finally:
        client.close()
    (output / "editor.log").rename(output / "second-startup.log")
    client = Client(None, binary, output)
    try:
        state = client.call("get_preview_defaults")
        check(not state["ready"] and state["error"] and path.read_bytes() == malformed, "malformed startup shows an error and preserves storage")
        client.call("set_view", saved)
        change(client, "reset")
        check(client.call("get_preview_defaults")["current"] == saved, "reset saves factory defaults without applying")
        change(client, "apply")
        check(client.call("get_preview_defaults")["current"] == factory, "factory defaults apply explicitly")
        path.write_text(json.dumps({"version": 1, "preview": saved}))
        change(client, "reload")
        check(client.call("get_preview_defaults")["current"] == factory, "reload never applies corrected defaults")
        change(client, "apply")
        check(client.call("get_preview_defaults")["current"] == saved, "reloaded corrected defaults apply on request")
        stored_bytes = path.read_bytes()
        path.chmod(0o444)
        try:
            change(client, "save", error=True)
            state = client.call("get_preview_defaults")
            check(not state["ready"] and state["error"] and path.read_bytes() == stored_bytes, "atomic write failure remains visible without a persistence fallback")
        finally:
            path.chmod(0o644)
        change(client, "reload")
        path.unlink()
        path.mkdir()
        change(client, "reset", error=True)
        check(path.is_dir(), "reset rejects nonregular preference storage")
        path.rmdir()
        change(client, "reload")
        change(client, "reset")
    finally:
        client.close()
    result = {"transport": "private stdio", "checks": len(checks), "passed": checks}
    (output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"transport": result["transport"], "checks": len(checks)}))


def main():
    parser = argparse.ArgumentParser(description="Validate explicit preview defaults in private headless processes.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True, help="New private directory for preferences and reports")
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    validate(args.binary.resolve(), output)


if __name__ == "__main__":
    main()
