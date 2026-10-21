import argparse
import hashlib
import json
import os
import tempfile
import time
from pathlib import Path

import validate_cubemap_capture_probe as probe
import validate_cubemap_reflections as reflections
import validate_native_runtime as runtime
from validate_brush_instances import source_state
from validate_bump_lighting import texture
from validate_compile_jobs import ACTIVE_JOB_STATUSES
from validate_face_lighting import DEFAULT_BINARY, ROOT, Client, detail
from validate_recovery_exit import require_clean_eof
from validate_visibility import ROOM

REASON = "Cubemap binding inspection and drafts are unavailable while the map contains instances; authored faces do not describe the assembled geometry"
CHILD_BOUNDS = ([-256, -248, -16], [288, 296, 0])
PRIOR_LIN = b"13 17 19\n23 29 31\n"
COLORS = {"gray": (160, 24, 4), "cyan": (16, 200, 200), "red": (220, 24, 16),
          "green": (16, 220, 32), "yellow": (220, 200, 16), "blue": (16, 32, 220)}


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def digest(path):
    require(path.is_file() and not path.is_symlink() and path.stat().st_size <= 1024**3, f"Unbounded or aliased source: {path}")
    before = path.stat()
    with path.open("rb") as stream:
        result = hashlib.file_digest(stream, "sha256").hexdigest()
    after = path.stat()
    require((before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns, before.st_ctime_ns)
            == (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns, after.st_ctime_ns), "Source changed during hashing")
    return result


def property_state(client):
    return client.call("get_property_edit") if client.call("get_map_info")["property_draft"] is not None else None


def state(client, source, child, game):
    return {"document": source_state(client, source), "draft": property_state(client),
            "child_sha256": digest(child), "assets": reflections.source_assets(game),
            "settings": client.call("get_compile_settings")}


def material_fixture(game):
    folder = game / "materials" / reflections.MATERIALS
    folder.mkdir(parents=True)
    for name, rgb in COLORS.items():
        (folder / f"{name}.vtf").write_bytes(texture(bytes([*rgb, 255]) * 32 * 32))
        shader = "LightmappedGeneric" if name == "gray" else "UnlitGeneric"
        extra = ' $envmap "env_cubemap" $envmaptint "[1 1 1]" unknown_metadata { retained "yes" }' if name == "gray" else ""
        (folder / f"{name}.vmt").write_text(f'{shader} {{ $basetexture "{reflections.MATERIALS}/{name}" $surfaceprop concrete{extra} }}\n')


def parent(client, path, child=None):
    client.call("new_map", {"discard_changes": True})
    ids = []
    for index, ((low, high), name) in enumerate(zip(ROOM, COLORS, strict=True)):
        if child is not None and index == 0:
            continue
        ids.append(client.call("create_block", {"mins": low, "maxs": high, "material": f"{reflections.MATERIALS}/{name}"})["id"])
    helper = None
    if child is not None:
        helper = client.call("create_entity", {"classname": "func_instance", "origin": [24, -16, 0],
                                               "properties": {"file": child.name, "angles": "0 90 0"}})["id"]
        ids.append(helper)
    for classname, origin, properties in [
        ("info_player_terrorist", [-192, -176, 8], {}),
        ("info_player_counterterrorist", [192, 176, 8], {}),
        ("light", [0, 0, 180], {"_light": "255 255 255 200", "_constant_attn": "1", "_linear_attn": "0", "_quadratic_attn": "0"}),
        *[("env_cubemap", list(origin), {"cubemapsize": str(probe.SIZE_ENUM)}) for origin in probe.SAMPLES],
    ]:
        ids.append(client.call("create_entity", {"classname": classname, "origin": origin, "properties": properties})["id"])
    client.call("save_map", {"path": str(path)})
    return ids, helper


def wait_job(client, timeout):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        job = client.call("get_compile_status")["job"]
        if job["status"] not in ACTIVE_JOB_STATUSES:
            return job
        time.sleep(0.1)
    raise TimeoutError("Native assembled reflection job did not complete within the declared timeout")


def stop_job(client, timeout):
    job = client.call("get_compile_status")["job"]
    if job and job["status"] in ACTIVE_JOB_STATUSES:
        if job["cancellable"]:
            client.call("cancel_compile", {"token": job["token"]})
        wait_job(client, timeout)


def patches(faces):
    return [[face[y * probe.CUBE_SIZE + x] for y in range(12, 20) for x in range(12, 20)] for face in faces]


def compare_patches(actual, expected):
    checks = []
    for actual_face, expected_face in zip(actual, expected, strict=True):
        delta = [abs(a - b) for pixel, reference in zip(actual_face, expected_face, strict=True) for a, b in zip(pixel, reference, strict=True)]
        mean = sum(delta) / len(delta)
        fraction = sum(max(delta[index:index + 3]) > 8 for index in range(0, len(delta), 3)) / 64
        require(mean < 3 and fraction < 0.02, f"Assembled child differs from literal flat native control: mean={mean}, fraction={fraction}")
        checks.append({"mean_component_difference": mean, "changed_fraction": fraction})
    return checks


def capture_evidence(data, result, verified):
    stats = result["stats"]["reflections"]
    directory = Path(stats["artifacts"])
    paths = [Path(path) for path in stats.get("capture_artifacts", [])]
    require(stats["channel"] == "ldr" and (not paths or paths == [directory]), "Expected exactly one real LDR capture process")
    require(directory.resolve().is_relative_to((ROOT / "target").resolve()), "Capture evidence escaped the on-disk target directory")
    request = json.loads(probe.read_bounded(directory / "request.json", 2 * 1024**2))
    require(request["channel"] == "ldr" and len(request["samples"]) == len(probe.SAMPLES), "Native request did not retain its LDR sample identities")
    receipts = {}
    for name in ["result.json", "supervisor-result.json"]:
        receipt = json.loads(probe.read_bounded(directory / name, 64 * 1024))
        require(receipt["success"] and receipt["verified_controls"], f"Native capture receipt failed: {name}")
        receipts[name] = receipt
    for observer in ["worker", "supervisor", "caller"]:
        audit = json.loads(probe.read_bounded(directory / f"private-preservation-{observer}.json", 16 * 1024**2))
        require(audit["success"], f"Native configuration/Cloud preservation failed: {observer}")
    cleanup = json.loads(probe.read_bounded(directory / "private-caller-cleanup.json", 64 * 1024))
    require(all(cleanup[key] for key in ["session_drained", "supervisor_reaped", "preservation_passed"]), "Native capture did not drain/preserve its owned session")
    supervisor_cleanup = json.loads(probe.read_bounded(directory / "private-cleanup.json", 64 * 1024))
    require(supervisor_cleanup["group_exited"] and supervisor_cleanup["audit_passed"], "Native supervisor group drain or preservation audit failed")
    native = json.loads(probe.read_bounded(directory / "native-process.json", 64 * 1024))
    console = probe.read_bounded(directory / "cstrike/hammerr-capture.log", 2 * 1024**2).decode(errors="replace")
    require(console.count("HAMMERR_CUBEMAP_CAPTURE_BEGIN") == 1
            and console.index("HAMMERR_CUBEMAP_CAPTURE_BEGIN") < console.index("HAMMERR_CUBEMAP_CAPTURE_RETURNED") < console.index("HAMMERR_CUBEMAP_DISCONNECTED"),
            "Native one-pass handshake is incomplete or repeated")
    for index in range(1, len(probe.SAMPLES) + 1):
        require(f"bounce: 1/1 sample: {index}/{len(probe.SAMPLES)}" in console, "Native one-pass sample progress is incomplete")
    staged, captured = [probe.read_bounded(directory / name, probe.MAX_BSP_BYTES) for name in ["staged.bsp", "captured.bsp"]]
    before, after = [probe.bsp_lumps(value) for value in [staged, captured]]
    require(all(left == right for index, (left, right) in enumerate(zip(before, after, strict=True)) if index not in (35, 40)), "Native capture changed non-PAK geometry/lighting/entities")
    probe.verify_game_lump_preservation(staged, captured)
    raw, _ = probe.zip_entries(after[40][2])
    published, _ = probe.zip_entries(probe.bsp_lumps(data)[40][2])
    requested = {tuple(sample["origin"]): sample for sample in request["samples"]}
    images = {}
    for sample in verified["manifest"]["samples"]:
        original = requested[tuple(sample["origin"])]
        payload = raw[original["texture"]]
        require(payload == published[sample["texture"]]
                and probe.digest(payload) != original["bootstrap_sha256"] == sample["bootstrap_sha256"]
                and original["dimension"] == sample["edge"], "Published sample differs from its actual native capture or retains bootstrap")
        metadata, faces = probe.decode_cube(payload)
        require(metadata["image_end"] == len(payload) and not sample["texture"].endswith(".hdr.vtf"), "Capture is not the complete registered native LDR payload")
        patch = patches(faces)
        floor = patch[5]
        means = [sum(pixel[axis] for pixel in floor) / len(floor) for axis in range(3)]
        require(means[0] > 30 and means[0] > means[1] * 1.5 and means[1] > means[2] * 1.5,
                f"Native down-face did not capture the orange child/literal floor: {means}")
        images[tuple(sample["origin"])] = patch
    files = ["request.json", "result.json", "supervisor-result.json", "native-process.json", "command.json", "staged.bsp", "captured.bsp",
             "cstrike/hammerr-capture.log", "private-cleanup.json", "private-caller-cleanup.json",
             *[f"private-preservation-{observer}.json" for observer in ["worker", "supervisor", "caller"]]]
    return {"directory": str(directory), "native_process": native, "receipts": receipts,
            "artifact_sha256": {name: digest(directory / name) for name in files},
            "channel_observer": "real capture request/control/progress and decoded registered native LDR payload; no HardwareConfig012 observation claimed",
            "sample_patches": {str(origin): patch for origin, patch in images.items()}}, images


def compile_case(client, output, label, background, source, child, game, timeout):
    path = output / f"{label}.bsp"
    path.write_bytes(b"Retain prior output until guarded native publication.\n")
    path.with_suffix(".lin").write_bytes(PRIOR_LIN)
    before = state(client, source, child, game)
    if background:
        started = client.call("start_compile", {"path": str(path)})["job"]
        job = wait_job(client, timeout)
        require(job["token"] == started["token"] and job["status"] == "completed" and job["result"]["published"] and not job["stale"], "Background native publication failed")
        result = job["result"]
        require(job["instance_dependencies"]["current"], "Assembled dependencies became stale during native capture")
    else:
        job = None
        result = client.call("compile_map", {"path": str(path)})
    require(result["success"] and (background or result["game"] is None), "Native reflection build failed or launched ordinary Play")
    game_status = client.call("game_log", error=True)
    require("no game has been launched by HammeRR" in "\n".join(row.get("text", "") for row in game_status["content"]),
            "Native reflection build created ordinary Play state")
    require(state(client, source, child, game) == before and path.with_suffix(".lin").read_bytes() == PRIOR_LIN,
            "Native reflection build changed source/history/draft/child/assets/settings or prior LIN")
    data = probe.read_bounded(path, probe.MAX_BSP_BYTES)
    verified = reflections.verify_assets(data, path.stem, output / f"{label}-verified")
    evidence, images = capture_evidence(data, result, verified)
    return {"compile": result, "job": job, "verified": verified, "capture": evidence, "bsp": str(path)}, images


def run_program(binary, output, args, flat_reference):
    output.mkdir()
    temporary = Path(tempfile.mkdtemp(prefix="n730-", dir=(ROOT / "target").resolve()))
    maximum_socket = temporary / ("hammerr_capture_" + "0" * 16) / "wayland/wayland-0"
    socket_bytes = len(os.fsencode(maximum_socket))
    require(socket_bytes < 108, f"Native capture Wayland socket would exceed sun_path: {socket_bytes} bytes at {maximum_socket}")
    game = runtime.private_game(args.game_dir, output / "source-game", engine_physics=True)
    material_fixture(game)
    environment = {"TMPDIR": str(temporary), "HAMMERR_RECENT_MAPS_DIR": str(output / "recent"),
                   "HAMMERR_BUILD_RECIPES": str(output / "recipes.json"), "HAMMERR_PREFABS_DIR": str(output / "prefabs"),
                   "XDG_CONFIG_HOME": str(output / "config"), "XDG_STATE_HOME": str(output / "state")}
    binary_sha256 = digest(binary)
    client = Client(None, binary, output, game / "gameinfo.txt", environment=environment, request_timeout=args.timeout)
    failure = None
    report = {"binary": str(binary), "binary_sha256": binary_sha256, "success": False,
              "native_tmpdir": str(temporary), "maximum_native_socket_path": str(maximum_socket),
              "maximum_native_socket_path_bytes": socket_bytes}
    try:
        child = output / "floor.vmf"
        client.call("create_block", {"mins": CHILD_BOUNDS[0], "maxs": CHILD_BOUNDS[1], "material": f"{reflections.MATERIALS}/gray"})
        client.call("save_map", {"path": str(child)})
        client.call("set_compile_settings", {"lighting": "direct_ldr", "physics": "none", "bounces": 0,
                                              "generate_reflections": True, "bake_static_prop_vertices": False})
        native = client.call("set_native_capture_runtime", {"engine_root": str(args.game_dir), **({"steam_root": str(args.steam_root)} if args.steam_root else {})})
        require(native["ready"], "Installed one-pass capture runtime is not ready")
        report["native_runtime"] = native
        if flat_reference is None:
            flat_source = output / "literal-flat.vmf"
            parent(client, flat_source)
            require(client.call("get_cubemap_bindings")["available"] is True, "Ordinary flat binding inspection became unavailable")
            flat, flat_reference = compile_case(client, output, "literal-flat", False, flat_source, child, game, args.timeout)
            report["literal_flat"] = flat
        source = output / "assembled.vmf"
        ids, owner = parent(client, source, child)
        token = client.call("begin_property_edit", {"world": True})["draft"]["token"]
        client.call("update_property_edit", {"token": token, "changes": [{"action": "set", "key": "message", "value": "Retain this assembled reflection draft"}]})
        original = state(client, source, child, game)
        inspection = client.call("get_cubemap_bindings")
        require(inspection["available"] is False and inspection["total"] == 0 and inspection["assignments"] == []
                and inspection["diagnostics"] == [REASON] and inspection["state"]["context_error"] == REASON,
                "Instance binding inspection falsely reported authored faces as complete")
        face = detail(client, [ids[0]])[0]
        refused = client.call("set_cubemap_bindings", {"action": "preview", "helper": int(ids[-1].split(":")[-1]),
                                                      "faces": [{"solid": face["solid"], "side": face["faces"][0]["side"]}],
                                                      "operation": "bind", "snapshot": inspection["state"]["snapshot"]}, error=True)
        require(REASON in "\n".join(row.get("text", "") for row in refused["content"]), "Binding preview lacked the common assembled-geometry refusal")
        require(state(client, source, child, game) == original, "Unavailable binding inspection/draft changed authoring state")
        report["inspection"] = inspection
        for label, background in [("assembled-blocking", False), ("assembled-background", True)]:
            built, images = compile_case(client, output, label, background, source, child, game, args.timeout)
            dependencies = built["compile"]["stats"]["instances"]["dependencies"]
            require(dependencies == [{"path": child.name, "bytes": child.stat().st_size, "sha256": digest(child)}]
                    and built["compile"]["stats"]["instances"]["world_brushes"] == 1, "Native build lost original transformed child provenance")
            require(images.keys() == flat_reference.keys(), "Assembled samples differ from literal native control")
            built["literal_native_parity"] = {str(origin): compare_patches(images[origin], flat_reference[origin]) for origin in images}
            report[label] = built
        require(state(client, source, child, game) == original and not client.call("get_map_info")["rendering"]["initialized"], "Native build changed source state or initialized editor graphics")
        saved_child = child.read_bytes()
        try:
            child.write_bytes(saved_child + b"\n// child-only dependency drift\n")
            status = client.call("get_compile_status")
            require(status["job"]["stale"] and not status["job"]["instance_dependencies"]["current"], "Child drift did not stale native assembly provenance")
            report["child_drift"] = status
        finally:
            child.write_bytes(saved_child)
        require(state(client, source, child, game) == original, "Child drift control did not restore original state")
        report.update({"success": True, "instance_owner": owner, "source_preserved": True})
        return report, flat_reference
    except BaseException as error:
        failure = error
        report.update({"success": False, "error": {"type": type(error).__name__, "message": str(error)}})
        raise
    finally:
        cleanup = []
        for action in [lambda: stop_job(client, args.timeout), lambda: require_clean_eof(client)]:
            try:
                action()
            except BaseException as error:
                cleanup.append(error)
        if cleanup:
            report.update({"success": False, "cleanup_errors": [str(error) for error in cleanup]})
        try:
            require(digest(binary) == report["binary_sha256"], "MCP program changed during validation")
        except BaseException as error:
            cleanup.append(error)
            report.update({"success": False, "cleanup_errors": [str(item) for item in cleanup]})
        write_json(output / "results.json", report)
        if cleanup:
            raise BaseExceptionGroup("MCP source or native cleanup failed", ([failure] if failure is not None else []) + cleanup)


def main():
    parser = argparse.ArgumentParser(description="Actual one-pass LDR reflections for a transformed child floor and root cubemaps; requires installed CS:S, synchronized Steam Cloud and the native-runtime helper beside each binary. Uses at most five fresh captures for two programs, no input/focus, -insecure and sv_lan1. No synthetic capture backend.")
    parser.add_argument("--binary", type=Path, action="append", help="Repeat for at most two actual MCP programs; defaults to hammerr-headless")
    parser.add_argument("--game-dir", type=Path, default=Path.home() / ".local/share/Steam/steamapps/common/Counter-Strike Source")
    parser.add_argument("--steam-root", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--timeout", type=int, default=360, help="Bound each MCP request, native background job and cleanup wait in seconds (60..600)")
    args = parser.parse_args()
    args.binary = [path.resolve(strict=True) for path in (args.binary or [DEFAULT_BINARY])]
    args.game_dir, args.output = args.game_dir.resolve(strict=True), args.output.resolve()
    require(1 <= len(args.binary) <= 2 and len(set(args.binary)) == len(args.binary) and 60 <= args.timeout <= 600, "Use one/two distinct programs and a bounded timeout")
    require(args.output.is_relative_to((ROOT / "target").resolve()) and not args.output.exists() and not os.environ.get("LD_PRELOAD"), "Use a fresh on-disk target output without inherited LD_PRELOAD")
    args.output.mkdir(parents=True)
    before = reflections.protected_files(args, ["literal-flat", "assembled-blocking", "assembled-background"])
    report = {"success": False, "desktop_input": False, "programs": [], "capture_observer": "actual production one-pass native runtime; no HardwareConfig012 capture receipt claimed"}
    try:
        reference = None
        for index, binary in enumerate(args.binary):
            result, reference = run_program(binary, args.output / f"program-{index}", args, reference)
            report["programs"].append(result)
        captures = [value["capture"] for program in report["programs"] for key, value in program.items() if key in ["literal_flat", "assembled-blocking", "assembled-background"]]
        require(len(captures) == 1 + 2 * len(args.binary) and len({capture["native_process"]["pid"] for capture in captures}) == len(captures), "Capture processes are missing or reused")
        report.update({"success": True, "actual_fresh_captures": len(captures)})
    except BaseException as error:
        report.update({"success": False, "error": {"type": type(error).__name__, "message": str(error)}})
        raise
    finally:
        report["installed_assets_preserved"] = reflections.protected_files(args, ["literal-flat", "assembled-blocking", "assembled-background"]) == before
        if not report["installed_assets_preserved"]:
            report["success"] = False
        write_json(args.output / "results.json", report)
        require(report["installed_assets_preserved"], "Native validation changed installed assets")


if __name__ == "__main__":
    main()
