import argparse
import hashlib
import json
import os
import struct
import time
from pathlib import Path
from unittest.mock import patch

from validate_authored_planes import records
from validate_brush_instances import geometry, sealed_parent, source_state
from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_normal_map_preview import fixture
from validate_sky_preview import rgba_png

CAMERA = {"position": [0, -80, 100], "look_at": [40, 48, 16], "fov": 50}
WIDTH, HEIGHT = 641, 401
GEOMETRY_LUMPS = (1, 2, 3, 6, 7, 12, 13, 14, 18, 19)


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def action(client, name, token=None, error=False, **fields):
    arguments = {"action": name, **fields}
    if token is not None:
        arguments["token"] = token
    return client.call("set_instance_extraction", arguments, error=error)


def wait(client, ready):
    deadline = time.monotonic() + 90
    while time.monotonic() < deadline:
        report = client.call("get_instance_extraction")
        if ready and report.get("ready"):
            return report
        if not ready and report["token"] is None and report.get("last", {}).get("published"):
            return report
        if report["status"] in ("failed", "blocked"):
            raise AssertionError(report)
        time.sleep(0.02)
    raise TimeoutError("Instance extraction worker")


def lump(data, index):
    offset, length, version, compression = struct.unpack_from("<4i", data, 8 + index * 16)
    assert compression == 0 and 0 <= offset <= offset + length <= len(data)
    return version, data[offset:offset + length]


def environment(output):
    return {"HAMMERR_RECENT_MAPS_DIR": str(output / "recent"),
            "HAMMERR_BUILD_RECIPES": str(output / "recipes.json"),
            "HAMMERR_PREFABS_DIR": str(output / "prefabs"),
            "XDG_CONFIG_HOME": str(output / "config"), "XDG_STATE_HOME": str(output / "state")}


def validate(client, binary, output, gameinfo, result):
    checks = result["checks"]

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def block(low, high):
        return client.call("create_block", {"mins": low, "maxs": high, "material": "audit/surface"})["id"]

    def capture(name, enabled):
        png, metadata = client.render_result(width=WIDTH, height=HEIGHT, camera=CAMERA,
            instance_contents=enabled, entity_images=False, entity_model_guides=False,
            selection_outlines=False, fog_preview=False, surface="flat")
        path = output / f"{name}.png"
        path.write_bytes(png)
        (output / f"{name}.metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
        result["images"].append({"file": path.name, "sha256": hashlib.sha256(png).hexdigest()})
        return rgba_png(path), metadata

    def pick(enabled):
        return client.call("pick_at_pixel", {"x": WIDTH // 2, "y": HEIGHT // 2,
            "width": WIDTH, "height": HEIGHT, "camera": CAMERA, "instance_contents": enabled})

    schemas = {tool["name"]: tool for tool in client.rpc("tools/list", {})["tools"]}
    assert schemas["set_instance_extraction"]["inputSchema"]["additionalProperties"] is False
    client.call("set_compile_settings", {"lighting": "fullbright", "physics": "none",
        "bounces": 0, "pack_sky_assets": False, "generate_reflections": False})
    sealed_parent(client)
    world = block([32, 40, 8], [48, 56, 24])
    brush = detail(client, [world])[0]
    client.call("edit_faces", {"faces": [{"solid": brush["solid"], "side": face["side"]}
        for face in brush["faces"]], "u_axis": [1, 0.5, 0], "v_axis": [0, 1, 0.25],
        "scale_u": -0.25, "scale_v": 0.5, "shift_u": 7.5, "shift_v": -3.25})
    a = block([64, 40, 8], [72, 56, 24])
    b = block([80, 40, 8], [88, 56, 24])
    owner = client.call("tie_to_entity", {"ids": [a, b], "classname": "func_detail"})["id"]
    client.call("create_entity", {"classname": "info_target", "origin": [0, 0, 64]})
    selected = [world, owner]
    client.call("set_spatial_settings", {"grid_visible": False})
    client.call("set_view", {"instance_contents": False, "entity_images": False,
        "entity_model_guides": False, "selection_outlines": False})
    client.call("set_camera", CAMERA)
    parent = output / "parent.vmf"
    client.call("save_map", {"path": str(parent)})
    (output / "flattened-control.vmf").write_bytes(parent.read_bytes())
    parent.with_suffix(".bsp").write_bytes(b"previous parent BSP")
    parent.with_suffix(".lin").write_bytes(b"previous parent LIN")
    parts = output / "parts"
    parts.mkdir()
    client.call("select", {"ids": selected})
    original = source_state(client, parent)
    result["source_before"] = original

    token = action(client, "prepare", file="parts/cancel.vmf")["token"]
    assert client.call("get_instance_extraction")["contents_validation"] == "passed"
    assert source_state(client, parent) == original
    action(client, "cancel", token)
    check(not (parts / "cancel.vmf").exists() and source_state(client, parent) == original
          and not client.call("get_map_info")["rendering"]["initialized"],
          "Prepare/cancel use real serialized compiler validation without source/history/camera edits or graphics startup")
    existing = parts / "existing.vmf"
    existing.write_bytes(b"existing destination owner")
    action(client, "prepare", file="parts/existing.vmf", error=True)
    assert existing.read_bytes() == b"existing destination owner" and source_state(client, parent) == original

    token = action(client, "prepare", file="parts/directory-race.vmf")["token"]
    stages = list(parts.glob(".hammerr-extract-*.vmf"))
    assert len(stages) == 1
    stage_name = stages[0].name
    moved = output / "moved-parts"
    parts.rename(moved)
    parts.mkdir()
    replacement = parts / stage_name
    replacement.write_bytes(b"unrelated directory replacement")
    action(client, "extract", token, error=True)
    assert not (moved / stage_name).exists()
    assert replacement.read_bytes() == b"unrelated directory replacement"
    assert not (parts / "directory-race.vmf").exists()
    assert source_state(client, parent) == original
    action(client, "cancel", token)
    replacement.unlink()
    check(True, "Existing destinations and renamed target directories reject, clean only owned staging, and preserve replacement files plus parent state")

    face = detail(client, [world])[0]["faces"][0]["side"]
    client.call("create_entity", {"classname": "env_cubemap", "origin": [0, 0, 64],
        "properties": {"sides": str(face)}})
    client.call("select", {"ids": selected})
    bound = source_state(client, parent)
    action(client, "prepare", file="parts/bound.vmf", error=True)
    assert source_state(client, parent) == bound and not (parts / "bound.vmf").exists()
    client.call("undo")
    client.call("select", {"ids": selected})
    token = action(client, "prepare", file="parts/stale.vmf")["token"]
    client.call("create_entity", {"classname": "info_target", "origin": [120, 0, 32]})
    edited = source_state(client, parent)
    action(client, "extract", token, error=True)
    assert source_state(client, parent) == edited and not (parts / "stale.vmf").exists()
    assert not list(parts.glob(".hammerr-extract-*.vmf"))
    action(client, "cancel", token)
    client.call("undo")
    client.call("select", {"ids": selected})
    original = source_state(client, parent)
    check(True, "Inbound selected-face bindings and late document edits reject without stripping metadata, changing another edit, or retaining stale staging")

    manual_path = output / "flattened-control.bsp"
    manual = client.call("compile_map", {"path": str(manual_path)})
    assert manual["success"] and "instances" not in manual["stats"]
    assert source_state(client, parent) == original
    assert any(row[:8] == (-4.0, -2.0, 0.0, 7.5, 0.0, 2.0, 0.5, -3.25)
               for row in records(manual_path.read_bytes(), 6, "<16f2i"))
    before_pixels, _ = capture("original-brushes", False)
    assert pick(False)["object"] == world
    token = action(client, "prepare", file="parts/assembly.vmf", background=True)["token"]
    review = wait(client, True)
    assert review["token"] == token and review["selected"] == selected
    assert [review[key] for key in ("objects", "world_brushes", "detail_brushes", "sides")] == [2, 1, 2, 18]
    assert review["pivot"] == [0, 0, 0] and review["contents_validation"] == "passed"
    for name, fields in [("extract", {"file": "other.vmf"}), ("cancel", {"background": False})]:
        action(client, name, token, error=True, **fields)
    assert source_state(client, parent) == original and not (parts / "assembly.vmf").exists()
    action(client, "extract", token, background=True)
    completed = wait(client, False)["last"]
    helper, child = completed["id"], parts / "assembly.vmf"
    assert completed["published"] and completed["undo_retains_child"]
    assert completed["bytes"] == len(child.read_bytes()) and completed["sha256"] == digest(child) == review["sha256"]
    props = dict(detail(client, [helper])[0]["properties"])
    assert props["file"] == "parts/assembly.vmf" and props["origin"] == props["angles"] == "0 0 0"
    after = client.call("get_map_info")
    assert after["selection"] == [helper] and after["unsaved_changes"]
    assert client.call("get_history")["undo_count"] == original["history"]["undo_count"] + 1
    assert digest(parent) == original["source_sha256"]
    assert after["camera"] == original["map"]["camera"] and after["view"] == original["map"]["view"]
    assert action(client, "cancel", token)["status"] == "completed" and child.exists()
    check(True, "Background Prepare/Extract publish one exact child and one parent Undo step with correct path, identity pivot, frozen IDs/hash and late-Cancel completion")
    result["review"], result["publication"] = review, completed

    child_output = output / "child-inspection"
    child_output.mkdir()
    with patch.dict(os.environ, environment(child_output)):
        child_client = Client(None, binary, child_output, gameinfo=gameinfo)
    try:
        child_client.call("open_map", {"path": str(child)})
        assert detail(child_client, selected) == [row for row in original["objects"] if row["id"] in selected]
        assert dict(child_client.call("get_map_info")["worldspawn"]) == {"classname": "worldspawn"}
    finally:
        child_client.close()
    check(True, "A second private Session reads the final child with exact original solid/side/detail-owner IDs, planes, signed oblique UVs and metadata")

    refreshed = client.call("set_instance_contents", {"action": "refresh"})
    assert refreshed["state"] == "ready" and refreshed["owners"][0]["owner"] == helper
    assert refreshed["owners"][0]["bounds"] == [[32.0, 40.0, 8.0], [88.0, 56.0, 24.0]]
    off, _ = capture("extracted-guide-off", False)
    on, metadata = capture("extracted-guide-on", True)
    assert on != off and before_pixels != off and metadata["instance_contents"]["displayed"]
    hit = pick(True)
    assert hit["object"] == helper and hit["solid"] is None and hit["side"] is None
    assert pick(False)["object"] != helper
    assert client.call("get_map_info")["view"] == original["map"]["view"]
    check(True, "Explicit compiler-backed Refresh shows independently expected world/detail bounds; real GPU pixels and pixel picking expose only the root reference owner")
    result["contents"] = refreshed

    compiled_path = output / "extracted.bsp"
    compiled = client.call("compile_map", {"path": str(compiled_path)})
    assert compiled["success"]
    stats = compiled["stats"]["instances"]
    assert [stats[key] for key in ("instances", "files", "world_brushes", "detail_brushes")] == [1, 1, 1, 2]
    assert stats["dependencies"] == [{"path": "parts/assembly.vmf", "bytes": len(child.read_bytes()), "sha256": digest(child)}]
    actual_bytes, manual_bytes = compiled_path.read_bytes(), manual_path.read_bytes()
    for index in GEOMETRY_LUMPS:
        assert lump(actual_bytes, index) == lump(manual_bytes, index), index
    assert geometry(compiled_path) == geometry(manual_path)
    check(True, "Actual Fullbright/physics-none instance compilation matches the unchanged manual control's geometry and texture-info lumps exactly, with correct child hash provenance")
    result["compiled"] = compiled
    child_bytes = child.read_bytes()
    client.call("undo")
    assert detail(client, [row["id"] for row in original["objects"]]) == original["objects"]
    assert child.read_bytes() == child_bytes and not client.call("get_map_info")["unsaved_changes"]
    client.call("redo")
    assert dict(detail(client, [helper])[0]["properties"]) == props and child.read_bytes() == child_bytes
    assert parent.with_suffix(".bsp").read_bytes() == b"previous parent BSP"
    assert parent.with_suffix(".lin").read_bytes() == b"previous parent LIN"
    assert digest(parent) == original["source_sha256"]
    assert not list(parts.glob(".hammerr-extract-*.vmf"))
    check(True, "Undo restores exact parent objects and Redo restores only the reference, retaining the exported child and previous parent VMF/BSP/LIN files")
    client.call("save_map", {"path": str(output / "extracted-parent.vmf")})
    result["parent_sha256"], result["child_sha256"] = digest(parent), digest(child)


def main():
    parser = argparse.ArgumentParser(description="Validate new-file brush-instance extraction with private stdio MCP and offscreen captures.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    binary, output = args.binary.resolve(), args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    gameinfo, materials = fixture(output)
    (materials / "surface.vmt").write_text("LightmappedGeneric { $basetexture audit/base }\n")
    (gameinfo.parent / "maps").mkdir()
    placeholder = gameinfo.parent.parent / "cstrike_linux64"
    placeholder.write_text("Private non-executable placeholder; no game is present.\n")
    assert placeholder.stat().st_mode & 0o111 == 0
    result = {"status": "running", "transport": "private_stdio", "binary_sha256": digest(binary),
              "native_exercised": False, "desktop_input": False, "checks": [], "images": []}
    client = None
    try:
        with patch.dict(os.environ, environment(output)):
            client = Client(None, binary, output, gameinfo=gameinfo)
        validate(client, binary, output, gameinfo, result)
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
