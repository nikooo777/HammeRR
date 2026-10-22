import argparse
import hashlib
import json
import os
import re
from pathlib import Path
from unittest.mock import patch

from validate_authored_planes import records
from validate_compile_jobs import wait_job
from validate_direct_lighting import FACE_FORMAT
from validate_face_lighting import DEFAULT_BINARY, Client, detail, face_ref
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
MODEL_MATERIAL = "audit/model"
MODEL_SOURCE_BOUNDS = (((-48, -32, 16), (48, 32, 24)), ((-8, -32, 32), (8, 32, 40)))
MODEL_LITERAL_BOUNDS = (((32, 0, 16), (128, 64, 24)), ((72, 0, 32), (88, 64, 40)))


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


def brush_model(client, literal=False, shadow_caster=False):
    blocks = []
    for low, high in MODEL_LITERAL_BOUNDS if literal else MODEL_SOURCE_BOUNDS:
        identifier = client.call("create_block", {"mins": low, "maxs": high, "material": MODEL_MATERIAL})["id"]
        blocks.append(identifier)
        shape = detail(client, [identifier])[0]
        for face in shape["faces"]:
            normal = face["normal"]
            assert sum(abs(value) for value in normal) == 1 and set(normal) <= {-1, 0, 1}
            if normal[0]:
                u, v, shift = [0, 1, 0], [0, 0, -1], [-128, 0]
            elif normal[1]:
                u, v, shift = [1, 0, 0], [0, 0, -1], [-320, 0]
            else:
                u, v, shift = [1, 0, 0], [0, -1, 0], [-320, 128]
            client.call("edit_faces", {"faces": [face_ref(shape, face)], "u_axis": u, "v_axis": v,
                                       "scale_u": 0.25, "scale_v": 0.25, "shift_u": shift[0] if literal else 0,
                                       "shift_v": shift[1] if literal else 0, "lightmap_scale": 16})
    model = client.call("tie_to_entity", {"ids": blocks, "classname": "func_brush"})["id"]
    definition = client.call("get_entity_class", {"classname": "func_brush"})
    origin = next(row for row in definition["properties"] if row["name"] == "origin")
    assert origin["label"] == "Origin (X Y Z)" and not origin["read_only"] and origin["default"] is None
    authored = dict(detail(client, [model])[0]["properties"])
    assert authored == {"classname": "func_brush", **definition["defaults"]}
    assert authored["spawnflags"] == "2" and authored["Solidity"] == authored["StartDisabled"] == authored["InputFilter"] == "0"
    assert "origin" not in authored and "angles" not in authored
    client.call("set_entity_properties", {"id": model, "set": {"origin": "96 40 4" if literal else "16 8 4"}})
    if shadow_caster:
        shadows = next(row for row in definition["properties"] if row["name"] == "vrad_brush_cast_shadows")
        assert shadows["label"] == "Shadows" and not shadows["read_only"] and shadows["default"] == "0"
        assert [(choice["value"], choice["label"]) for choice in shadows["choices"]] == [("0", "No"), ("1", "Yes")]
        client.call("set_entity_properties", {"id": model, "set": {"vrad_brush_cast_shadows": "1"}})
    return model, blocks, definition


def model_parent(client, path, child, literal=False, shadow_caster=False):
    sealed_parent(client)
    client.call("create_entity", {"classname": "light", "origin": [80, 32, 160], "properties": {
        "_light": "255 0 0 200" if shadow_caster else "255 128 64 200",
        "_lightHDR": "0 0 255 400" if shadow_caster else "64 255 128 400", "_constant_attn": "1",
        "_linear_attn": "0", "_quadratic_attn": "0"}})
    client.call("save_map", {"path": str(path)})
    token = client.call("set_instance_reference", {"action": "begin"})["token"]
    client.call("set_instance_reference", {"action": "update", "token": token, "file": child.name,
                                            "origin": "80 32 0", "angles": "0 0 0"})
    checked = client.call("set_instance_reference", {"action": "check", "token": token})
    assert checked["valid"] and checked["presence_checked"] and checked["contents_validation"] == "not_checked"
    owner = client.call("set_instance_reference", {"action": "apply", "token": token})["id"]
    assert owner == "entity:10", "Independent model identity reservation changed"
    if literal:
        model, blocks, _ = brush_model(client, literal=True, shadow_caster=shadow_caster)
        assert model == "entity:13" and blocks == ["solid:11", "solid:12"]
        client.call("delete_objects", {"ids": [owner]})
    client.call("save_map", {"path": str(path)})
    if literal:
        raw = path.read_bytes()
        assert raw.count(b'\n\t"id" "13"') == 1
        raw = raw.replace(b'\n\t"id" "13"', b'\n\t"id" "11"')
        raw, count = re.subn(rb'(\n\t\t"id" ")(11|12)(")',
                             lambda match: match[1] + {b"11": b"12", b"12": b"13"}[match[2]] + match[3], raw)
        assert count == 2, "Literal object-ID alignment did not cover exactly two model solids"
        path.write_bytes(raw)
    saved = path.read_bytes()
    client.call("open_map", {"path": str(path)})
    assert path.read_bytes() == saved
    return owner, client.call("get_map_info")["worldspawn"]


def validate_brush_models(client, output, gameinfo, result, shadow_caster=False):
    from validate_instance_lighting import entity_rows

    retained = {path: path.read_bytes() for pattern in ("*.bsp", "*.lin") for path in output.rglob(pattern)}
    output = output / ("brush-casters" if shadow_caster else "brush-models")
    output.mkdir()
    material = gameinfo.parent / f"materials/{MODEL_MATERIAL}.vmt"
    assert not material.exists()
    material.write_text("LightmappedGeneric { $basetexture audit/base }\n")
    material_bytes = material.read_bytes()
    client.call("new_map", {"discard_changes": True})
    model, _, definition = brush_model(client)
    child = output / "model-child.vmf"
    client.call("save_map", {"path": str(child)})
    child_bytes = child.read_bytes()
    client.call("open_map", {"path": str(child)})
    assert child.read_bytes() == child_bytes
    parent, literal = output / "parent.vmf", output / "literal.vmf"
    owner, world = model_parent(client, parent, child, shadow_caster=shadow_caster)
    _, literal_world = model_parent(client, literal, child, literal=True, shadow_caster=shadow_caster)
    assert world == literal_world, "Normal-save world metadata differs between child and literal"
    receipt = {"cases": {}, "fgd": definition, "physics": "none", "native_exercised": False,
               "literal_origin": [96, 40, 4], "literal_local_bounds": [-64, -40, 12, 32, 24, 36],
               "identity_alignment": {"entity:13": "entity:11", "solid:11": "solid:12", "solid:12": "solid:13"}}
    result["brush_casters" if shadow_caster else "brush_models"] = receipt

    def preview():
        report = client.call("set_instance_contents", {"action": "refresh"})
        assert report["state"] == "ready" and report["stats"]["brush_models"] == 1
        assert report["stats"]["world_brushes"] == report["stats"]["detail_brushes"] == 0
        assert not report["available"] and not report["displayed"] and report["owners"][0]["bounds"] is None
        assert report["brush_model_preview"]["available"] is False and "material or native" in report["brush_model_preview"]["reason"]
        assert report["brushes_total"] == report["face_provenance_total"] == report["geometry"]["vertices"] == 0
        assert report["stats"]["dependencies"] == [{"path": child.name, "bytes": len(child_bytes), "sha256": hashlib.sha256(child_bytes).hexdigest()}]
        camera = client.call("get_camera")
        error = client.call("set_instance_contents", {"action": "frame", "token": str(report["token"]), "ids": [owner]}, error=True)
        assert "no visible geometry" in text(error) and client.call("get_camera") == camera
        return report

    def pair(source, label, mode, failure=False):
        client.call("set_compile_settings", {"lighting": mode, "physics": "none", "bounces": 0})
        settings = client.call("get_compile_settings")
        assert settings["lighting"] == mode and settings["physics"] == "none" and settings["bounces"] == 0
        token = client.call("begin_property_edit", {"world": True})["draft"]["token"]
        client.call("update_property_edit", {"token": token, "changes": [{"action": "set", "key": "message", "value": "Retained child-model review"}]})
        before = (source_state(client, source), client.call("get_compile_settings"), client.call("get_property_edit"), game_files(gameinfo.parent))
        outputs, jobs = [], []
        try:
            for background in (False, True):
                path = output / f"{label}{'-background' if background else ''}.bsp"
                if not path.exists():
                    path.write_bytes(b"previous child-model BSP")
                pointfile = path.with_suffix(".lin")
                pointfile.write_bytes(b"retained child-model pointfile\n")
                old = path.read_bytes()
                if background:
                    submitted = client.call("start_compile", {"path": str(path)})["job"]
                    job = wait_job(client)
                    assert job["token"] == submitted["token"]
                    built = job["result"]
                else:
                    response = client.call("compile_map", {"path": str(path)}, error=failure)
                    built = json.loads(text(response)) if failure else response
                    job = client.call("get_compile_status")["job"]
                assert built["success"] is not failure and job["status"] == ("failed" if failure else "completed")
                assert job["settings"]["lighting"] == mode and job["settings"]["physics"] == "none"
                assert job["result"]["published"] is not failure
                if failure:
                    assert path.read_bytes() == old and built["bsp"] is None
                    assert any(row["level"] == "error" and row["object"] == owner and ("Brush global shadows:" if shadow_caster else "Child brush-model material:") in row["message"] for row in built["diagnostics"]), built
                else:
                    assert not job["stale"] and built["bsp"] == str(path) and built["leak"] is None
                    assert built["stats"]["lighting_mode"] == mode
                    if shadow_caster:
                        expected_casters = 0 if mode == "fullbright" or label == "child-shadows-off" else 1
                        assert built["stats"]["brush_shadow_models"] == expected_casters
                        if mode == "fullbright":
                            assert any(row["level"] == "info" and row["message"] ==
                                       "Brush global shadows: 1 admitted Shadows Yes source request(s); not baked in Fullbright"
                                       for row in built["diagnostics"]), built
                assert pointfile.read_bytes() == b"retained child-model pointfile\n"
                assert (source_state(client, source), client.call("get_compile_settings"), client.call("get_property_edit"), game_files(gameinfo.parent)) == before
                assert child.read_bytes() == child_bytes
                outputs.append(path.read_bytes())
                jobs.append(job)
            assert outputs[0] == outputs[1], "Foreground/background complete model BSP differs"
            receipt["cases"][label + ("-refusal" if failure else "")] = jobs
            return outputs[0]
        finally:
            client.call("cancel_property_edit", {"token": token})

    try:
        with patch.object(client, "call", wraps=client.call) as calls:
            if shadow_caster:
                from validate_prop_shadows import brush_samples

                client.call("open_map", {"path": str(parent)})
                off = pair(parent, "child-shadows-off", "direct_ldr_hdr")
                off_samples = brush_samples(off, (80, 32))
                assert off_samples == {"ldr": ["c8000000"], "hdr": ["0000c801"]}, off_samples
                receipt["off_samples"] = off_samples
                (output / "model-child-off.vmf").write_bytes(child_bytes)
                client.call("open_map", {"path": str(child)})
                shadows = next(row for row in definition["properties"] if row["name"] == "vrad_brush_cast_shadows")
                assert shadows["label"] == "Shadows" and not shadows["read_only"] and shadows["default"] == "0"
                assert [(choice["value"], choice["label"]) for choice in shadows["choices"]] == [("0", "No"), ("1", "Yes")]
                client.call("set_entity_properties", {"id": model, "set": {"vrad_brush_cast_shadows": "1"}})
                client.call("save_map", {"path": str(child)})
                child_bytes = child.read_bytes()
            for mode in ("fullbright", "direct_ldr_hdr"):
                compiled = []
                for label, source in (("child", parent), ("literal", literal)):
                    client.call("open_map", {"path": str(source)})
                    if label == "child":
                        receipt["preview"] = preview()
                    data = pair(source, f"{label}-{mode}", mode)
                    stats = client.call("get_compile_status")["job"]["result"]["stats"]
                    if label == "child":
                        assert stats["instances"]["brush_models"] == 1
                    else:
                        assert "instances" not in stats
                    models = records(data, 14, "<9f3i")
                    assert len(models) == 2 and models[1][:9] == (-64, -40, 12, 32, 24, 36, 0, 0, 0)
                    rows = [row for row in entity_rows(data) if row.get("classname") == "func_brush"]
                    assert len(rows) == 1 and rows[0]["model"] == "*1" and rows[0]["origin"] == "96 40 4"
                    assert rows[0]["hammerid"] == "11" and "angles" not in rows[0]
                    faces, infos = records(data, 7, FACE_FORMAT), records(data, 6, "<16f2i")
                    first, count = models[1][10:12]
                    assert {infos[face[5]][:8] for face in faces[first:first + count]} == {
                        (0, 4, 0, 32, 0, 0, -4, -16), (4, 0, 0, 64, 0, 0, -4, -16), (4, 0, 0, 64, 0, -4, 0, -32)}
                    if shadow_caster:
                        assert rows[0]["vrad_brush_cast_shadows"] == "1"
                    if shadow_caster and mode == "direct_ldr_hdr":
                        samples = brush_samples(data, (80, 32))
                        assert samples == {"ldr": ["00000000"], "hdr": ["00000000"]}, samples
                        receipt["cases"][f"{label}-{mode}-world_samples"] = samples
                    compiled.append(data)
                assert compiled[0] == compiled[1], f"{mode}: complete BSP differs from independently authored translated compound model"
            client.call("open_map", {"path": str(parent)})
            material.write_text("LightmappedGeneric { $basetexture audit/base " +
                                ("$alphatest 1" if shadow_caster else "%compileorigin 1") + " }\n")
            receipt["material_is_not_certified_by_refresh"] = preview()
            pair(parent, "child-direct_ldr_hdr", "direct_ldr_hdr", failure=True)
            dispatches = [call.args[0] for call in calls.call_args_list if call.args[0] in ("compile_map", "start_compile")]
            assert dispatches == ["compile_map", "start_compile"] * (6 if shadow_caster else 5)
            receipt["compile_dispatches"] = dispatches
    finally:
        material.write_bytes(material_bytes)
    assert all(path.read_bytes() == data for path, data in retained.items())
    assert child.read_bytes() == child_bytes and not client.call("get_map_info")["rendering"]["initialized"]
    result["checks"].append("Compound child models preserve exact translated root BSP/local UVs, honest reference counts and material-refusal custody through both compile paths")
    if shadow_caster:
        result["checks"].append("Installed Shadows No/Yes changes exact world LDR/HDR luxels; actual baked counts and Fullbright admitted-source/not-baked status agree with complete child/root BSP parity")


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
    assert any(diagnostic["level"] == "info" and "Expanded 1 brush instances from 1 files" in diagnostic["message"]
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
    controls = parser.add_mutually_exclusive_group()
    controls.add_argument("--brush-model-control", action="store_true", help="Add ten compile calls for one installed-FGD compound func_brush child, explicit Origin, shared saved reference/Refresh with unavailable model preview, exact translated root-literal BSP and local UV parity in Fullbright and LDR+HDR, plus a custom-origin material refusal preserving output/source/draft. Uses No physics; no Engine or native pixel claim. Existing six-call controls remain unchanged.")
    controls.add_argument("--brush-caster-control", action="store_true", help="Add twelve compile calls for installed-FGD Shadows No/Yes on an ordinary compound func_brush child: exact red/blue world-luxel change, complete child/root literal BSP parity in Fullbright and LDR+HDR, actual baked count versus Fullbright not-baked status, and an alpha-material refusal preserving source/draft/BSP/LIN. No physics or native pixel claim. Original six-call controls remain unchanged; alternative to --brush-model-control.")
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
        if args.brush_model_control or args.brush_caster_control:
            validate_brush_models(client, output, gameinfo, result, shadow_caster=args.brush_caster_control)
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
