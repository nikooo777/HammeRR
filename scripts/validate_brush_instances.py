import argparse
import hashlib
import json
import os
from pathlib import Path
from unittest.mock import patch

from validate_authored_planes import records
from validate_compile_jobs import wait_job
from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_normal_map_preview import fixture
from validate_visibility import ROOM

MATERIAL = "audit/surface"
CHILD_WORLD = ([-48, -24, 16], [-16, 24, 48])
CHILD_DETAIL = ([0, -16, 16], [24, 16, 40])
UNSAVED_PARENT = ([80, 64, 16], [112, 96, 48])
DETAIL_CONTENTS = 0x08000000
SOURCE_FIELDS = (
    "path", "unsaved_changes", "world_brushes", "point_entities",
    "brush_entities", "worldspawn", "undo", "redo", "selection",
    "face_selection", "camera", "view",
)


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def text(result):
    return next(item["text"] for item in result["content"] if item["type"] == "text")


def block(client, bounds):
    return client.call(
        "create_block", {"mins": bounds[0], "maxs": bounds[1], "material": MATERIAL}
    )["id"]


def sealed_parent(client):
    client.call("new_map", {"discard_changes": True})
    for bounds in ROOM:
        block(client, bounds)
    client.call(
        "create_entity", {"classname": "info_player_terrorist", "origin": [-192, -192, 8]}
    )


def source_state(client, path):
    info = client.call("get_map_info")
    ids = [row["id"] for row in client.call("find_objects", {"limit": 500})["objects"]]
    return {
        "map": {key: info[key] for key in SOURCE_FIELDS},
        "history": client.call("get_history"),
        "objects": detail(client, ids),
        "source_sha256": digest(path),
    }


def game_files(game):
    return {
        str(path.relative_to(game)): digest(path)
        for path in sorted(game.rglob("*")) if path.is_file()
    }


def geometry(path):
    data = path.read_bytes()
    planes = records(data, 1, "<4fi")
    sides = records(data, 19, "<Hhhh")
    brushes = sorted(
        (contents, sorted(planes[side[0]][:4] for side in sides[first:first + count]))
        for first, count, contents in records(data, 18, "<3i")
    )
    return {
        "brushes": brushes,
        "model_bounds": [row[:6] for row in records(data, 14, "<9f3i")],
    }


def validate(client, output, gameinfo, result):
    checks = result["checks"]

    def check(condition, label):
        assert condition, label
        checks.append(label)

    settings = {
        "lighting": "fullbright", "physics": "none", "bounces": 0,
        "pack_sky_assets": False, "generate_reflections": False,
    }
    client.call("set_compile_settings", settings)
    child = output / "parts/child.vmf"
    child.parent.mkdir()
    block(client, CHILD_WORLD)
    brush = block(client, CHILD_DETAIL)
    client.call("tie_to_entity", {"ids": [brush], "classname": "func_detail"})
    token = client.call("begin_property_edit", {"world": True})["draft"]["token"]
    client.call("update_property_edit", {
        "token": token,
        "changes": [{"action": "set", "key": "skyname", "value": "audit_ignored_child"}],
    })
    client.call("commit_property_edit", {"token": token})
    client.call("save_map", {"path": str(child)})
    child_bytes = child.read_bytes()
    child_relative = child.relative_to(output).as_posix()

    sealed_parent(client)
    helper = client.call("create_entity", {
        "classname": "func_instance", "origin": [0, 0, 0],
        "properties": {"file": child_relative, "angles": "0 0 0"},
    })["id"]
    parent = output / "parent.vmf"
    client.call("save_map", {"path": str(parent)})
    extra = block(client, UNSAVED_PARENT)
    client.call("select", {"ids": [helper, extra]})
    client.call("set_camera", {"position": [-220, -210, 180], "look_at": [0, 0, 48]})
    original = source_state(client, parent)
    assert original["map"]["unsaved_changes"]
    result["parent_state_before"] = original
    shallow = client.call("run_map_checks")
    assert any(issue["severity"] == "info" and issue["object"] == helper
               and "dependencies" in issue["message"] for issue in shallow["issues"])

    blocking_path = output / "blocking.bsp"
    background_path = output / "background.bsp"
    previous_lin = b"7 8 9\n10 11 12\n"
    for path in [blocking_path, background_path]:
        path.write_bytes(b"previous BSP")
        path.with_suffix(".lin").write_bytes(previous_lin)
    built = client.call("compile_map", {"path": str(blocking_path)})
    stats = built["stats"]["instances"]
    assert built["success"] and built["game"] is None
    assert [stats[key] for key in ["instances", "files", "world_brushes", "detail_brushes"]] == [1, 1, 1, 1]
    assert stats["dependencies"] == [{
        "path": child_relative, "bytes": len(child_bytes),
        "sha256": hashlib.sha256(child_bytes).hexdigest(),
    }]
    assert built["stats"]["brushes"] == 9
    assert any(diagnostic["object"] == helper and child_relative in diagnostic["message"]
               and "child world settings are ignored" in diagnostic["message"]
               for diagnostic in built["diagnostics"])
    assert any(diagnostic["level"] == "info" and "not shown in the editor viewport" in diagnostic["message"]
               for diagnostic in built["diagnostics"])
    check(blocking_path.read_bytes().startswith(b"VBSP")
          and blocking_path.with_suffix(".lin").read_bytes() == previous_lin,
          "Blocking compilation publishes the unsaved snapshot with bounded hashes and root-helper diagnostic provenance")
    result["blocking"] = built

    started = client.call("start_compile", {"path": str(background_path)})["job"]
    completed = wait_job(client)
    assert completed["token"] == started["token"] and completed["status"] == "completed"
    assert completed["result"]["published"] and not completed["stale"]
    assert completed["result"]["stats"]["instances"] == stats
    assert completed["instance_dependencies"]["current"]
    assert background_path.read_bytes() == blocking_path.read_bytes()
    assert background_path.with_suffix(".lin").read_bytes() == previous_lin
    healthy = client.call("get_compile_status")
    baked = client.call("inspect_baked_lightmaps")
    assert healthy["launch"]["ready"] and baked["build"]["status"] == "current"
    assert source_state(client, parent) == original
    check(not client.call("get_map_info")["rendering"]["initialized"],
          "Background publication matches blocking bytes and preserves source, history, selection and camera without graphics startup")
    result["background"] = completed
    result["healthy"] = {"status": healthy, "baked": baked}

    child.write_bytes(child_bytes + b"\n// child-only dependency mutation\n")
    stale = client.call("get_compile_status")
    stale_baked = client.call("inspect_baked_lightmaps")
    assert stale["job"]["status"] == "completed" and stale["job"]["stale"]
    assert not stale["job"]["instance_dependencies"]["current"]
    assert not stale["launch"]["ready"] and "Instance source/dependencies changed" in stale["launch"]["reason"]
    assert stale_baked["build"]["status"] == "stale" and not stale_baked["build"]["current"]
    game_before = game_files(gameinfo.parent)
    rejected = client.call("launch_game", error=True)
    assert "Instance source/dependencies changed" in text(rejected)
    assert game_files(gameinfo.parent) == game_before
    info = client.call("get_map_info")
    assert not info["launching"] and not info["game_running"]
    assert source_state(client, parent) == original
    check(True, "A child-only change stales job, Play and baked provenance; Play rejects before touching the private game tree")
    result["stale_dependency"] = {"status": stale, "baked": stale_baked, "play_rejection": rejected}

    cases = output / "cases"
    cases.mkdir()
    failures = {}
    for name, content in [
        ("malformed", b"world {\n"),
        ("missing", None),
        ("unsupported", child_bytes + b'\nentity { "id" "9999" "classname" "info_target" "origin" "0 0 128" }\n'),
    ]:
        if content is None:
            child.unlink()
        else:
            child.write_bytes(content)
            (cases / f"{name}-child.vmf").write_bytes(content)
        previous_bsp = background_path.read_bytes()
        if name == "missing":
            client.call("start_compile", {"path": str(background_path)})
            failed_job = wait_job(client)
            assert failed_job["status"] == "failed"
            failed = failed_job["result"]
            failures[name] = failed_job
        else:
            failed = json.loads(text(client.call("compile_map", {"path": str(background_path)}, error=True)))
            failures[name] = failed
        assert not failed["success"] and any(
            diagnostic["level"] == "error" and diagnostic["object"] == helper
            for diagnostic in failed["diagnostics"]
        ), failed
        if name == "unsupported":
            assert any("info_target" in diagnostic["message"] and "unsupported" in diagnostic["message"]
                       for diagnostic in failed["diagnostics"])
        assert background_path.read_bytes() == previous_bsp
        assert background_path.with_suffix(".lin").read_bytes() == previous_lin
        assert source_state(client, parent) == original
    child.write_bytes(child_bytes)
    check(True, "Malformed, missing and unsupported children fail with selectable root provenance and preserve BSP/LIN plus parent state")
    result["failures"] = failures
    result["parent_state_after"] = source_state(client, parent)
    client.call("save_map", {"path": str(output / "parent-current-snapshot.vmf")})

    sealed_parent(client)
    block(client, UNSAVED_PARENT)
    block(client, CHILD_WORLD)
    detail_brush = block(client, CHILD_DETAIL)
    client.call("tie_to_entity", {"ids": [detail_brush], "classname": "func_detail"})
    flat_source = output / "flattened.vmf"
    client.call("save_map", {"path": str(flat_source)})
    flat_before = source_state(client, flat_source)
    flat_path = output / "flattened.bsp"
    flat = client.call("compile_map", {"path": str(flat_path)})
    flat_status = client.call("get_compile_status")
    assert flat["success"] and "instances" not in flat["stats"]
    assert "instance_dependencies" not in flat_status["job"] and not flat_status["job"]["stale"]
    assert flat["stats"]["brushes"] == built["stats"]["brushes"]
    actual_geometry, flat_geometry = geometry(background_path), geometry(flat_path)
    assert actual_geometry == flat_geometry
    assert sum(bool(contents & DETAIL_CONTENTS) for contents, _planes in actual_geometry["brushes"]) == 1
    assert source_state(client, flat_source) == flat_before
    final = client.call("get_map_info")
    result["graphics_initialized"] = final["rendering"]["initialized"]
    assert not final["rendering"]["initialized"] and not final["game_running"] and not final["launching"]
    check(True, "No-instance manual control omits optional instance metadata and matches compiled brush planes, DETAIL ownership and model bounds")
    result["flattened_control"] = {"result": flat, "status": flat_status, "geometry": actual_geometry}


def main():
    parser = argparse.ArgumentParser(description="Validate private brush-instance compilation through stdio MCP without game or graphics execution.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    binary = args.binary.resolve()
    gameinfo, materials = fixture(output)
    (materials / "surface.vmt").write_text("LightmappedGeneric { $basetexture audit/base }\n")
    (gameinfo.parent / "maps").mkdir()
    placeholder = gameinfo.parent.parent / "cstrike_linux64"
    placeholder.write_text("Private non-executable installation placeholder; no game is present.\n")
    assert placeholder.stat().st_mode & 0o111 == 0
    result = {
        "status": "running", "transport": "stdio", "binary": str(binary),
        "binary_sha256": digest(binary), "gameinfo": str(gameinfo),
        "checks": [], "native_exercised": False, "graphics_initialized": None,
    }
    client = None
    try:
        with patch.dict(os.environ, {
            "HAMMERR_RECENT_MAPS_DIR": str(output / "recent"),
            "HAMMERR_BUILD_RECIPES": str(output / "recipes.json"),
            "HAMMERR_PREFABS_DIR": str(output / "prefabs"),
            "XDG_CONFIG_HOME": str(output / "config"),
            "XDG_STATE_HOME": str(output / "state"),
        }):
            client = Client(None, binary, output, gameinfo=gameinfo)
        validate(client, output, gameinfo, result)
        result["status"] = "passed"
    except Exception as error:
        result["status"] = "failed"
        result["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        if client is not None:
            client.close()
        result["artifacts"] = [
            {"path": str(path.relative_to(output)), "bytes": path.stat().st_size, "sha256": digest(path)}
            for path in sorted(output.rglob("*")) if path.is_file() and path.suffix in [".vmf", ".bsp", ".lin"]
        ]
        (output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps({"status": result["status"], "checks": len(result["checks"]), "output": str(output)}))


if __name__ == "__main__":
    main()
