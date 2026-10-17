import argparse
import hashlib
import json
import math
import os
from pathlib import Path
from unittest.mock import patch

from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_normal_map_preview import fixture


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def action(client, name, token=None, error=False, **fields):
    arguments = {"action": name, **fields}
    if token is not None:
        arguments["token"] = token
    return client.call("set_instance_reference", arguments, error=error)


def state(client, parent):
    info = client.call("get_map_info")
    ids = [row["id"] for row in client.call("find_objects", {"limit": 100})["objects"]]
    return {
        "objects": detail(client, ids), "history": client.call("get_history"),
        "selection": client.call("get_selection"), "camera": client.call("get_camera"),
        "view": info["view"], "dirty": info["unsaved_changes"],
        "parent_sha256": digest(parent),
    }


def properties(client, entity):
    return dict(detail(client, [entity])[0]["properties"])


def bounds_close(actual, expected):
    assert all(math.isclose(a, b, abs_tol=1e-5)
               for row, wanted in zip(actual, expected, strict=True)
               for a, b in zip(row, wanted, strict=True)), (actual, expected)


def validate(client, output, result):
    checks = result["checks"]

    def check(condition, label):
        assert condition, label
        checks.append(label)

    child = output / "parts/arch.vmf"
    child.parent.mkdir()
    client.call("create_block", {"mins": [-16, -8, 0], "maxs": [16, 8, 24], "material": "audit/surface"})
    brush = client.call("create_block", {"mins": [24, -8, 0], "maxs": [40, 8, 16], "material": "audit/surface"})["id"]
    client.call("tie_to_entity", {"ids": [brush], "classname": "func_detail"})
    client.call("save_map", {"path": str(child)})
    child_bytes = child.read_bytes()
    client.call("new_map", {"discard_changes": True})
    client.call("set_spatial_settings", {"grid_visible": False})
    client.call("set_view", {"instance_contents": False, "entity_images": False, "selection_outlines": False})
    parent = output / "parent.vmf"
    client.call("save_map", {"path": str(parent)})
    for suffix, payload in [(".bsp", b"previous BSP"), (".lin", b"previous LIN")]:
        parent.with_suffix(suffix).write_bytes(payload)
    original = state(client, parent)
    token = action(client, "begin")["token"]
    form = action(client, "update", token, file="parts/arch.vmf", origin="80.000 16 8", angles="0 90 0")
    assert form["valid"] and not form["presence_checked"]
    assert form["origin_components"] == ["80.000", "16", "8"]
    form = action(client, "file", token, selected_path=str(child))
    assert form["file"] == "parts/arch.vmf" and form["presence_checked"]
    assert form["contents_validation"] == "not_checked" and form["bytes"] == len(child_bytes)
    check(state(client, parent) == original and not client.call("get_map_info")["rendering"]["initialized"],
          "Begin, component updates and explicit file checks preserve source/history/selection/camera with lazy graphics")
    for name, fields in [
        ("file", {"selected_path": str(output.parent / "outside-instance.vmf")}),
        ("apply", {"file": "parts/arch.vmf"}),
    ]:
        action(client, name, token, error=True, **fields)
        assert state(client, parent) == original
    invalid = action(client, "update", token, angles="NaN 0 0")
    assert not invalid["valid"] and invalid["angles"] == "NaN 0 0" and invalid["angle_components"] is None
    action(client, "apply", token, error=True)
    assert state(client, parent) == original
    action(client, "update", token, angles="0 90 0")
    created = action(client, "apply", token)
    owner = created["id"]
    assert created["contents_validation"] == "not_checked"
    assert properties(client, owner)["origin"] == "80.000 16 8"
    assert client.call("get_map_info")["undo"][1:] == []
    assert [row["id"] for row in client.call("get_selection")["objects"]] == [owner]
    assert client.call("get_camera") == original["camera"]
    client.call("undo")
    assert not client.call("find_objects", {"classname": "func_instance"})["objects"]
    client.call("redo")
    assert properties(client, owner)["file"] == "parts/arch.vmf"
    check(True, "Create is one Undo/Redo transaction, selects one root helper and retains the authored transform token")
    ready = client.call("set_instance_contents", {"action": "refresh"})
    assert ready["state"] == "ready" and not ready["requested"] and not ready["displayed"]
    assert ready["stats"]["instances"] == ready["stats"]["files"] == 1
    assert ready["stats"]["world_brushes"] == ready["stats"]["detail_brushes"] == 1
    assert ready["owners"][0]["owner"] == owner
    bounds_close(ready["owners"][0]["bounds"], [[72, 0, 8], [88, 56, 32]])
    assert ready["stats"]["dependencies"] == [{"path": "parts/arch.vmf", "bytes": len(child_bytes), "sha256": hashlib.sha256(child_bytes).hexdigest()}]
    check(not client.call("get_map_info")["rendering"]["initialized"],
          "Only explicit Refresh resolves the saved relative child and its independent rotated world/detail bounds, with root-owner provenance")
    result["ready"] = ready
    source = state(client, parent)
    png, metadata = client.render_result(width=701, height=501,
        camera={"position": [-60, -120, 140], "look_at": [80, 28, 16]},
        instance_contents=True, selection_outlines=False)
    (output / "instance-authoring.png").write_bytes(png)
    (output / "instance-authoring.metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
    assert metadata["instance_contents"]["displayed"] and state(client, parent) == source
    token = action(client, "begin", id=owner)["token"]
    action(client, "update", token, origin="40 32 8", angles="0 0 0")
    action(client, "apply", token)
    assert properties(client, owner)["origin"] == "40 32 8"
    stale = client.call("inspect_instance_contents")
    assert stale["state"] == "stale" and not stale["available"]
    client.call("undo")
    assert properties(client, owner)["origin"] == "80.000 16 8"
    ready = client.call("set_instance_contents", {"action": "refresh"})
    bounds_close(ready["owners"][0]["bounds"], [[72, 0, 8], [88, 56, 32]])
    check(True, "Apply edits the same owner once, stales the old reference layer and Undo preserves raw previous fields")

    client.call("save_map", {"path": str(parent)})
    source = state(client, parent)
    token = action(client, "begin", id=owner)["token"]
    action(client, "update", token, file="parts/missing.vmf")
    action(client, "apply", token, error=True)
    assert state(client, parent) == source
    action(client, "cancel", token)
    token = action(client, "begin", id=owner)["token"]
    action(client, "check", token)
    child.write_bytes(child_bytes + b"\n// changed selected file\n")
    action(client, "apply", token, error=True)
    assert state(client, parent) == source
    action(client, "cancel", token)
    child.write_bytes(child_bytes)
    token = action(client, "begin", id=owner)["token"]
    client.call("save_map", {"path": str(parent)})
    saved_state = state(client, parent)
    action(client, "apply", token, error=True)
    assert state(client, parent) == saved_state
    action(client, "cancel", token)
    check(True, "Missing references, changed checked files and same-path Save reject Apply without changing the map or owned source state")

    malformed = output / "parts/malformed.vmf"
    malformed.write_text("world {\n")
    token = action(client, "begin", id=owner)["token"]
    action(client, "update", token, file="parts/malformed.vmf")
    admitted = action(client, "check", token)
    assert admitted["presence_checked"] and admitted["contents_validation"] == "not_checked"
    action(client, "apply", token)
    failed = client.call("set_instance_contents", {"action": "refresh"})
    assert failed["state"] == "failed" and not failed["available"]
    assert properties(client, owner)["file"] == "parts/malformed.vmf"
    client.call("undo")
    assert properties(client, owner)["file"] == "parts/arch.vmf"
    check(True, "A present malformed VMF is explicitly unchecked at authoring, then fails the real full resolver without silently repairing the reference")
    token = action(client, "begin", id=owner)["token"]
    action(client, "update", token, origin="0 0 0")
    prior = state(client, parent)
    action(client, "cancel", token)
    assert state(client, parent) == prior and client.call("get_instance_reference")["token"] is None
    assert parent.with_suffix(".bsp").read_bytes() == b"previous BSP"
    assert parent.with_suffix(".lin").read_bytes() == b"previous LIN"
    assert child.read_bytes() == child_bytes
    client.call("save_map", {"path": str(output / "authored-current.vmf")})
    result.update({"created": created, "malformed_presence": admitted, "full_resolution_failure": failed,
                   "parent_sha256": digest(parent), "child_sha256": digest(child),
                   "image_sha256": hashlib.sha256(png).hexdigest()})
    check(True, "Cancel and all authoring/preview actions preserve previous BSP/LIN plus child files, without native execution")


def main():
    parser = argparse.ArgumentParser(description="Validate owned instance reference authoring through private stdio MCP.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    binary, output = args.binary.resolve(), args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    gameinfo, materials = fixture(output)
    (materials / "surface.vmt").write_text("LightmappedGeneric { $basetexture audit/base }\n")
    result = {"status": "running", "transport": "private_stdio", "binary_sha256": digest(binary),
              "native_exercised": False, "desktop_input": False, "checks": []}
    client = None
    try:
        with patch.dict(os.environ, {"HAMMERR_RECENT_MAPS_DIR": str(output / "recent"),
            "HAMMERR_BUILD_RECIPES": str(output / "recipes.json"), "HAMMERR_PREFABS_DIR": str(output / "prefabs"),
            "XDG_CONFIG_HOME": str(output / "config"), "XDG_STATE_HOME": str(output / "state")}):
            client = Client(None, binary, output, gameinfo=gameinfo)
        validate(client, output, result)
        result["status"] = "passed"
    except Exception as error:
        result["status"] = "failed"
        result["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        if client is not None:
            client.close()
        (output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps({"status": result["status"], "checks": len(result["checks"]), "output": str(output)}))


if __name__ == "__main__":
    main()
