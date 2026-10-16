import argparse
import io
import json
import math
import os
import re
import resource
import shutil
import struct
import subprocess
import sys
import time
import traceback
import zipfile
from pathlib import Path

import validate_cubemap_capture_probe as probe
import validate_native_runtime as runtime
from private_runtime_config import (
    finish_private_launch,
    private_launch_options,
    private_observer,
    private_readonly_launch,
    verify_private_launch,
)
from validate_bump_lighting import texture
from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_hdr_lighting import HARDWARE_VERSION, read_probe
from validate_visibility import ROOM

MANIFEST = "hammerr/generated-cubemaps.json"
MATERIALS = "hammerr_hdr_reflections"
MATERIAL_SYSTEM_SHA256 = "2ac4db05ea8ccc0ce63627a37c6a0551b2f9d21faf0cfd6c7128bf3f39622928"
MAX_ENTRY_BYTES = 4 * 1024**2
HDR_CONTROLS = {
    "mat_force_tonemap_scale": 0.25,
    "mat_autoexposure_min": 0.25,
    "mat_autoexposure_max": 0.25,
    "mat_disable_bloom": 1,
    "mat_bloomscale": 0,
}
ON_READY = "HAMMERR_REFLECTION_ON_READY 2 100"
OFF_READY = "HAMMERR_REFLECTION_OFF_READY 2 100"
FINISHED = "HAMMERR_REFLECTION_FINISHED"
REGION = (320, 400, 960, 650)
OBSERVER = """
stage <- 0;
ready_at <- 0.0;
function HammeRRThink() {
    local player = Entities.FindByClassname(null, "player");
    if (player == null) return 0.25;
    if (stage == 0) {
        SendToConsole("sv_cheats 1; sv_lan 1; joystick 0; bot_quota 0; bot_kick; jointeam 2; joinclass 1; hidepanel info; hidepanel team; hidepanel class_ter; hidepanel class_ct; r_drawvgui 0; gameui_hide; cl_drawhud 0; r_drawviewmodel 0; r_drawothermodels 0; mat_fullbright 0; toggle con_notifytime 0; toggle mat_specular 1; help mat_specular");
        ready_at = Time() + 6.0;
        stage = 1;
    }
    if (stage == 1 && Time() > ready_at && player.GetTeam() == 2 && player.GetHealth() == 100) {
        SendToConsole("noclip; god; setpos -192 -176 80; setang 25 45 0; sv_lan; mat_hdr_level; mat_fullbright");
        ready_at = Time() + 4.0;
        stage = 2;
    }
    if (stage >= 2) {
        player.SetOrigin(Vector(-192, -176, 80) - (player.EyePosition() - player.GetOrigin()));
        player.SetVelocity(Vector(0, 0, 0));
    }
    if (stage == 2 && Time() > ready_at) {
        printl("HAMMERR_REFLECTION_ON_READY " + player.GetTeam() + " " + player.GetHealth());
        stage = 3;
    }
    if (stage == 4 && Time() > ready_at) {
        printl("HAMMERR_REFLECTION_OFF_READY " + player.GetTeam() + " " + player.GetHealth());
        stage = 5;
    }
    if (stage == 3 || stage == 5) SendToConsole("exec hammerr_reflection_once.cfg");
    return 0.25;
}
"""


def require(condition, message):
    probe.require(condition, message)


def pak_entries(payload):
    require(22 <= len(payload) <= probe.MAX_PAK_BYTES, "Invalid PAK size")
    with zipfile.ZipFile(io.BytesIO(payload)) as archive:
        infos = archive.infolist()
        names = [info.filename for info in infos]
        require(len(infos) <= 256 and len(set(name.lower() for name in names)) == len(names), "Invalid PAK entry count or duplicate name")
        require(all(not name.startswith(("/", "\\")) and "\\" not in name and ".." not in name.split("/") for name in names), "Unsafe PAK name")
        require(sum(info.file_size for info in infos) <= probe.MAX_PAK_BYTES, "PAK decompression budget exceeded")
        require(all(0 < info.file_size <= MAX_ENTRY_BYTES and not info.flag_bits & 1 and info.compress_type in (zipfile.ZIP_STORED, zipfile.ZIP_DEFLATED) for info in infos), "Invalid PAK entry")
        require(archive.testzip() is None, "PAK CRC check failed")
        entries = {info.filename: archive.read(info) for info in infos}
        records = [{"name": info.filename, "bytes": info.file_size, "sha256": probe.digest(entries[info.filename])} for info in infos]
    return entries, records


def hdr_cube(data, edge, canonical):
    require(88 <= len(data) <= MAX_ENTRY_BYTES and data[:4] == b"VTF\0", "HDR sample is not a bounded VTF")
    major, minor, header = struct.unpack_from("<3I", data, 4)
    width, height = struct.unpack_from("<2H", data, 16)
    flags = struct.unpack_from("<I", data, 20)[0]
    require((major, minor) == (7, 4) and (width, height) == (edge, edge), "HDR sample version or dimensions differ")
    require(edge > 0 and edge <= 256 and edge & (edge - 1) == 0, "Invalid HDR cube edge")
    require(struct.unpack_from("<2H", data, 24) == (1, 0) and struct.unpack_from("<H", data, 63)[0] == 1, "HDR cube frames, first frame or depth differ")
    require(struct.unpack_from("<I", data, 52)[0] == 24 and flags & 0x4000 and not flags & ~0x600c, "HDR cube is not a supported native RGBA16F environment map")
    require(data[56] == edge.bit_length(), "HDR cube lacks its complete mip chain")
    require(all(math.isfinite(value) and value >= 0 for value in struct.unpack_from("<3f", data, 32)), "HDR reflectivity is invalid")
    bump = struct.unpack_from("<f", data, 48)[0]
    require(math.isfinite(bump) and bump >= 0, "HDR bump metadata is invalid")
    require(struct.unpack_from("<I", data, 57)[0] == 0xffffffff and data[61:63] == bytes(2), "HDR cube unexpectedly has thumbnail data")
    count = struct.unpack_from("<I", data, 68)[0]
    require(1 <= count <= 2 and header == 80 + count * 8 and header <= len(data), "Invalid HDR resource directory")
    resources = [data[80 + index * 8:88 + index * 8] for index in range(count)]
    high = [row for row in resources if row[:4] == bytes([0x30, 0, 0, 0])]
    require(len(high) == 1 and struct.unpack_from("<I", high[0], 4)[0] == header, "HDR high-resolution data is misplaced")
    require(all(row in high or row[:4] == b"CRC\x02" for row in resources), "Unregistered HDR resource")
    if canonical:
        require(header == 88 and count == 1, "Canonical HDR data retained an unused resource")
        require(all(not any(data[start:end]) for start, end in [(28, 32), (44, 48), (65, 68), (72, 80)]), "Canonical HDR header padding is nonzero")
    cursor = header
    images = {}
    rgb_hashes = []
    for mip in reversed(range(data[56])):
        dimension = max(1, edge >> mip)
        size = dimension * dimension * 8
        for face in range(7):
            require(cursor + size <= len(data), "Truncated HDR face or mip")
            image = data[cursor:cursor + size]
            cursor += size
            images[mip, face] = image
            if face == 6:
                if canonical:
                    require(not any(image), "Canonical HDR sphere face retained native unused bytes")
                continue
            rgb = bytearray()
            for offset in range(0, size, 8):
                r, g, b, alpha = struct.unpack_from("<4H", image, offset)
                require(all(bits & 0x7c00 != 0x7c00 and (not bits & 0x8000 or bits & 0x7fff == 0) for bits in [r, g, b]), "HDR RGB is negative or nonfinite")
                if canonical:
                    require(alpha == 0x3c00, "Canonical HDR alpha is not one")
                rgb.extend(image[offset:offset + 6])
            rgb_hashes.append({"mip": mip, "face": face, "rgb_sha256": probe.digest(rgb)})
    require(cursor == len(data), "HDR cube contains trailing or missing image bytes")
    metadata = {"format": 24, "version": [7, 4], "edge": edge, "faces": 7, "mips": data[56], "header": header, "bytes": len(data), "rgb": rgb_hashes, "canonical": canonical}
    return metadata, images


def hdr_faces(images):
    faces = []
    for face in range(6):
        pixels = []
        image = images[0, face]
        for offset in range(0, len(image), 8):
            rgb = struct.unpack_from("<3e", image, offset)
            pixels.append(tuple(round(min(1.0, value) ** (1.0 / 2.2) * 255) for value in rgb))
        faces.append(pixels)
    return faces


def verify_capture_rgb(data, manifest, stats):
    entries, _ = pak_entries(probe.bsp_lumps(data)[40][2])
    paths = [Path(path) for path in stats["capture_artifacts"]]
    require(len(paths) == len(set(paths)) == 2 and str(paths[0]) == stats["artifacts"], "Dual capture artifacts are incomplete")
    result = {}
    for directory in paths:
        request = json.loads((directory / "request.json").read_text())
        channel = request["channel"]
        require(channel in ["ldr", "hdr"] and channel not in result, "Capture channel is duplicated or unknown")
        for name in ["result.json", "supervisor-result.json"]:
            receipt = json.loads((directory / name).read_text())
            require(receipt["success"] and receipt["verified_controls"], f"Incomplete native capture receipt: {directory / name}")
        for observer in ["worker", "supervisor", "caller"]:
            audit = json.loads((directory / f"private-preservation-{observer}.json").read_text())
            require(audit["success"], f"Failed native capture preservation: {directory}, {observer}")
        cleanup = json.loads((directory / "private-caller-cleanup.json").read_text())
        require(all(cleanup[key] for key in ["session_drained", "supervisor_reaped", "preservation_passed"]), "Native capture ownership cleanup failed")
        captured, _ = pak_entries(probe.bsp_lumps(probe.read_bounded(directory / "captured.bsp", probe.MAX_BSP_BYTES))[40][2])
        requested = {tuple(sample["origin"]): sample for sample in request["samples"]}
        samples = manifest["hdr_samples" if channel == "hdr" else "samples"]
        require(set(requested) == {tuple(sample["origin"]) for sample in samples}, "Captured sample positions differ from publication")
        proofs = []
        for sample in samples:
            raw_sample = requested[tuple(sample["origin"])]
            raw = captured[raw_sample["texture"]]
            published = entries[sample["texture"]]
            require(probe.digest(raw) != raw_sample["bootstrap_sha256"] == sample["bootstrap_sha256"], "Native capture retained its bootstrap or lost provenance")
            if channel == "hdr":
                raw_metadata, _ = hdr_cube(raw, sample["edge"], False)
                published_metadata, _ = hdr_cube(published, sample["edge"], True)
                require(raw_metadata["rgb"] == published_metadata["rgb"], "HDR canonicalization changed six-face RGB bits in a mip")
                require(raw[20:24] == published[20:24] and raw[32:44] == published[32:44] and raw[48:52] == published[48:52], "HDR canonicalization changed retained native metadata")
            else:
                require(raw == published, "LDR native capture bytes changed during dual publication")
            proofs.append({"texture": sample["texture"], "raw_sha256": probe.digest(raw), "published_sha256": probe.digest(published), "all_mips_rgb_exact": True})
        default = manifest.get("hdr_default_texture" if channel == "hdr" else "default_texture")
        if default:
            suffix = ".hdr.vtf" if channel == "hdr" else ".vtf"
            raw = captured[f"materials/maps/{request['map_name']}/cubemapdefault{suffix}"]
            published = entries[default["path"]]
            if channel == "ldr":
                require(raw == published, "HDR pass replaced the authoritative LDR default")
            else:
                edge = struct.unpack_from("<H", published, 16)[0]
                raw_metadata, _ = hdr_cube(raw, edge, False)
                published_metadata, _ = hdr_cube(published, edge, True)
                require(raw_metadata["rgb"] == published_metadata["rgb"], "HDR default RGB changed during canonicalization")
        native_defaults = {name: probe.digest(value) for name, value in captured.items() if "/cubemapdefault" in name}
        result[channel] = {"directory": str(directory), "samples": proofs, "native_defaults": native_defaults}
    require(set(result) == {"ldr", "hdr"}, "Both capture channels were not observed")
    return result


def verify_rebased_assets(before, after, original, rebased):
    before_lumps, after_lumps = probe.bsp_lumps(before), probe.bsp_lumps(after)
    left, _ = pak_entries(before_lumps[40][2])
    right, _ = pak_entries(after_lumps[40][2])
    for index, (old, new) in enumerate(zip(before_lumps, after_lumps, strict=True)):
        if index not in [35, 40, 43, 44]:
            require(old == new, f"Run changed unrelated geometry or lighting lump {index}")
    probe.verify_game_lump_preservation(before, after)
    for key in ["samples", "hdr_samples"]:
        require(len(original[key]) == len(rebased[key]), "Run lost a reflection channel entry")
        for old, new in zip(original[key], rebased[key], strict=True):
            for field in ["helper_id", "origin", "size_enum", "edge", "sha256", "bootstrap_sha256"]:
                require(old[field] == new[field], f"Run changed reflection {field}")
            require(left[old["texture"]] == right[new["texture"]], "Run changed captured reflection texture bytes")
    for key in ["default_texture", "hdr_default_texture"]:
        old, new = original.get(key), rebased.get(key)
        require((old is None) == (new is None), "Run lost a registered reflection default")
        if old:
            require(old["sha256"] == new["sha256"] and left[old["path"]] == right[new["path"]], "Run changed a registered reflection default")
    require(not any(name.startswith(f"materials/maps/{original['namespace']}/") for name in right), "Run retained foreign generated namespace entries")


def patch_lump(data, index, payload):
    result = bytearray(data)
    result.extend(b"\0" * (-len(result) % 4))
    version, compression, _ = probe.bsp_lumps(data)[index]
    struct.pack_into("<4i", result, 8 + index * 16, len(result), len(payload), version, compression)
    result.extend(payload)
    return bytes(result)


def source_assets(game):
    return {
        str(path.relative_to(game)): probe.digest(probe.read_bounded(path, probe.MAX_ENTRY_BYTES))
        for path in sorted((game / "materials" / MATERIALS).rglob("*"))
        if path.is_file()
    }


def protected_files(args, names):
    paths = [args.game_dir / "bin/linux64" / name for name in probe.MODULES]
    paths.extend(path for path in (args.game_dir / "cstrike/cfg").rglob("*") if path.is_file())
    for name in names:
        paths.append(args.game_dir / "cstrike/maps" / f"{name}.bsp")
        for prefix in [args.game_dir, args.game_dir / "cstrike"]:
            for folder in ["materials/maps", "materialsrc/maps"]:
                paths.extend(path for path in (prefix / folder / name).rglob("*") if path.is_file())
    return {
        str(path): probe.digest(probe.read_bounded(path, probe.MAX_BSP_BYTES)) if path.exists() else None
        for path in sorted(set(paths))
    }


def fake_install(game, output):
    executable = game.parent / "cstrike_linux64"
    program = """#!/usr/bin/env python3
import json
import shutil
import sys
from pathlib import Path
args = sys.argv[1:]
assert '-insecure' in args and args[args.index('+sv_lan') + 1] == '1', args
game = Path(args[args.index('-game') + 1]).resolve(strict=True)
name = args[args.index('+map') + 1]
assert name.startswith('hammerr_') and '/' not in name and '\\\\' not in name, name
source = game / 'maps' / (name + '.bsp')
assert source.stat().st_size <= 64 * 1024**2
output = Path(__file__).resolve().parent.parent
shutil.copyfile(source, output / 'run-staged.bsp')
(output / 'run-launch.json').write_text(json.dumps({'map':name,'game':str(game),'argv':args,'fake_executable':str(Path(__file__).resolve())},indent=2)+'\\n')
"""
    executable.write_text(program)
    executable.chmod(0o700)
    require(executable.resolve().parent == game.parent.resolve(), "Fake Run executable escaped the private fixture")
    return executable


def author(args):
    game = runtime.private_game(args.game_dir, args.output / "source-game", engine_physics=True)
    folder = game / "materials" / MATERIALS
    folder.mkdir(parents=True)
    colors = {
        "gray": (40, 40, 40), "cyan": (16, 200, 200), "red": (220, 24, 16),
        "green": (16, 220, 32), "yellow": (220, 200, 16), "blue": (16, 32, 220),
    }
    for name, rgb in colors.items():
        (folder / f"{name}.vtf").write_bytes(texture(bytes([*rgb, 255]) * 32 * 32))
        shader = "LightmappedGeneric" if name == "gray" else "UnlitGeneric"
        extra = ' $envmap "env_cubemap" $envmaptint "[1 1 1]" unknown_metadata { retained "yes" }' if name == "gray" else ""
        (folder / f"{name}.vmt").write_text(f'{shader} {{ $basetexture "{MATERIALS}/{name}" $surfaceprop concrete{extra} }}\n')
    fake_install(game, args.output)
    client = Client(None, args.binary, args.output, game / "gameinfo.txt")
    ids = []
    try:
        for (low, high), material in zip(ROOM, colors, strict=True):
            ids.append(client.call("create_block", {"mins": low, "maxs": high, "material": f"{MATERIALS}/{material}"})["id"])
        for classname, origin, properties in [
            ("info_player_terrorist", [-192, -176, 8], {}),
            ("info_player_counterterrorist", [192, 176, 8], {}),
            ("light", [0, 0, 180], {"_light": "255 255 255 200", "_constant_attn": "1", "_linear_attn": "0", "_quadratic_attn": "0"}),
            *[("env_cubemap", list(origin), {"cubemapsize": str(probe.SIZE_ENUM)}) for origin in probe.SAMPLES],
        ]:
            ids.append(client.call("create_entity", {"classname": classname, "origin": origin, "properties": properties})["id"])
        source = args.output / "hdr_reflections.vmf"
        client.call("save_map", {"path": str(source)})
        floor = detail(client, [ids[0]])[0]
        top = next(face for face in floor["faces"] if face["normal"][2] > 0.99)
        face = {"solid": floor["solid"], "side": top["side"]}
        right_helper = int(ids[-1].split(":")[-1])
        before = detail(client, ids), client.call("get_map_info")["undo"], source.read_bytes()
        report = client.call("get_cubemap_bindings")
        require(not report["diagnostics"], report)
        request = {"action": "preview", "helper": right_helper, "faces": [face], "operation": "bind", "snapshot": report["state"]["snapshot"]}
        draft = client.call("set_cubemap_bindings", request)
        require((detail(client, ids), client.call("get_map_info")["undo"], source.read_bytes()) == before, "Binding preview changed the source or Undo history")
        client.call("set_cubemap_bindings", {"action": "cancel", "token": draft["draft"]["token"]})
        require((detail(client, ids), client.call("get_map_info")["undo"], source.read_bytes()) == before, "Binding Cancel changed the source or Undo history")
        report = client.call("get_cubemap_bindings")
        request["snapshot"] = report["state"]["snapshot"]
        draft = client.call("set_cubemap_bindings", request)
        client.call("set_cubemap_bindings", {"action": "apply", "token": draft["draft"]["token"]})
        applied = detail(client, ids)
        require(applied != before[0] and client.call("get_map_info")["undo"][0] == "Edit cubemap face bindings", "Binding Apply did not create its shared Undo transaction")
        client.call("undo")
        require(detail(client, ids) == before[0], "Binding Undo did not restore exact object metadata")
        client.call("redo")
        require(detail(client, ids) == applied, "Binding Redo did not restore the reviewed assignment")
        client.call("save_map", {"path": str(source)})
        before = source.read_bytes(), detail(client, ids), client.call("get_map_info")["undo"], source_assets(game)
        native = client.call("set_native_capture_runtime", {"engine_root": str(args.game_dir), **({"steam_root": str(args.steam_root)} if args.steam_root else {})})
        require(native["ready"], native)
        client.call("set_compile_settings", {"lighting": "direct_ldr", "physics": "none", "bounces": 0, "generate_reflections": False})
        bsp = args.output / "hdr_reflections.bsp"
        legacy = client.call("compile_map", {"path": str(bsp)})
        require(legacy["success"] and legacy["stats"]["lighting_bytes"] > 0, legacy)
        off_data = probe.read_bounded(bsp, probe.MAX_BSP_BYTES)
        off_entries, _ = probe.zip_entries(probe.bsp_lumps(off_data)[40][2])
        require(MANIFEST not in off_entries and probe.bsp_lumps(off_data)[7][0] == 0, "Generation OFF changed the legacy BSP contract")
        settings = client.call("get_compile_status")["settings"]
        client.call("set_compile_settings", {"lighting": "fullbright", "generate_reflections": True}, error=True)
        require(client.call("get_compile_status")["settings"] == settings, "Invalid combined reflection settings mutated shared settings")
        client.call("set_compile_settings", {"lighting": "direct_ldr_hdr", "generate_reflections": True})
        client.call("start_compile", {"path": str(bsp)})
        started = time.monotonic()
        job = None
        while time.monotonic() - started < args.timeout:
            job = client.call("get_compile_status")["job"]
            if job["status"] not in ["running", "cancelling"]:
                break
            time.sleep(0.2)
        require(job and job["status"] == "completed" and job["result"]["published"] is True, job)
        generated = job["result"]
        require(generated["stats"]["reflections"]["verified_samples"] == len(probe.SAMPLES), generated)
        require(generated["stats"]["reflections"]["channel"] == "ldr_hdr", generated)
        require((source.read_bytes(), detail(client, ids), client.call("get_map_info")["undo"], source_assets(game)) == before, "Reflection build changed source metadata, Undo or mounted fixture assets")
        data = probe.read_bounded(bsp, probe.MAX_BSP_BYTES)
        published = verify_assets(data, bsp.stem, args.output / "published")
        captures = verify_capture_rgb(data, published["manifest"], generated["stats"]["reflections"])
        require(any(row["manual"] and row["face_id"] == face["side"] and row["helper_id"] == right_helper for row in published["manifest"]["assignments"]), "Reviewed manual floor assignment was not compiled")
        launched = client.call("launch_game")
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            log = client.call("game_log")
            if log["status"] != "running":
                break
            time.sleep(0.05)
        require(log["status"] == "exited" and log["exit_code"] == 0, log)
        launch = json.loads((args.output / "run-launch.json").read_text())
        require(launch["map"] != bsp.stem and launch["map"] == launched["map"], "Run did not stage a fresh random namespace")
        staged = verify_assets(probe.read_bounded(args.output / "run-staged.bsp", probe.MAX_BSP_BYTES), launch["map"], args.output / "rebased")
        verify_rebased_assets(data, (args.output / "run-staged.bsp").read_bytes(), published["manifest"], staged["manifest"])
        require(staged["manifest"]["logical_namespace"] == bsp.stem, "Run lost the original logical namespace")
        require(probe.bsp_lumps(data)[7] == probe.bsp_lumps((args.output / "run-staged.bsp").read_bytes())[7], "Run rebasing changed actual face records")
        entries, _ = pak_entries(probe.bsp_lumps(data)[40][2])
        del entries[published["manifest"]["hdr_samples"][0]["texture"]]
        tampered = patch_lump(data, 40, probe.make_zip(entries))
        try:
            verify_assets(tampered, bsp.stem, args.output / "missing-sample")
        except AssertionError as error:
            require("Missing or foreign sample" in str(error), f"Missing-sample check failed for another reason: {error}")
        else:
            raise AssertionError("Independent validation accepted a missing generated sample")
        launch_before = (args.output / "run-launch.json").read_bytes()
        try:
            bsp.write_bytes(tampered)
            client.call("launch_game", error=True)
            require((args.output / "run-launch.json").read_bytes() == launch_before, "A BSP with a missing generated sample reached the Run executable")
        finally:
            bsp.write_bytes(data)
        require((source.read_bytes(), detail(client, ids), client.call("get_map_info")["undo"], source_assets(game)) == before, "Run mutated source metadata, Undo or fixture assets")
        return {"source": str(source), "bsp": str(bsp), "fixture_game": str(game), "compile": generated, "capture_rgb_preservation": captures, "legacy_compile": legacy, "native_runtime": native, "binding_face": face, "manual_helper": right_helper, "published": published, "run": {**launch, "verification": staged}, "source_preserved": True, "missing_hdr_sample_launch_rejected": True}
    finally:
        try:
            status = client.call("get_compile_status")["job"]
            if status and status["status"] in ["running", "cancelling"]:
                if status["cancellable"]:
                    client.call("cancel_compile", {"token": status["token"]})
                deadline = time.monotonic() + 240
                while time.monotonic() < deadline:
                    if client.call("get_compile_status")["job"]["status"] not in ["running", "cancelling"]:
                        break
                    time.sleep(0.25)
                else:
                    raise TimeoutError("Native build did not finish cleanup after validation failure")
        finally:
            client.close()


def verify_assets(data, namespace, output):
    output.mkdir()
    lumps = probe.bsp_lumps(data)
    entries, records = pak_entries(lumps[40][2])
    require(MANIFEST in entries, "Verified generated-asset manifest is absent")
    manifest = json.loads(entries[MANIFEST])
    require(manifest["version"] == 2 and manifest["phase"] == "verified" and manifest["channel"] == "ldr_hdr" and manifest["namespace"] == namespace, manifest)
    require(len(manifest["samples"]) == len(probe.SAMPLES) and manifest["aliases"] and manifest["assignments"], "Generated reflection manifest is incomplete")
    require(len(manifest["hdr_samples"]) == len(manifest["samples"]), "HDR sample pairs are incomplete")
    require(sorted(tuple(sample["origin"]) for sample in manifest["samples"]) == sorted(probe.SAMPLES), "Published cubemap origins differ from the authored fixture")
    sample_reports = []
    for sample in manifest["samples"]:
        name = sample["texture"]
        require(name.startswith(f"materials/maps/{namespace}/") and name.endswith(".vtf") and name in entries, f"Missing or foreign sample: {sample}")
        pixels = entries[name]
        require(probe.digest(pixels) == sample["sha256"] != sample["bootstrap_sha256"], "Sample hash is stale or still names the bootstrap")
        metadata, faces = probe.decode_cube(pixels)
        require(metadata["image_end"] == len(pixels) and sample["edge"] == probe.CUBE_SIZE, "Generated sample has extra or missing image bytes")
        probe.save_faces(output / (Path(sample["texture"]).name + "-faces.png"), faces)
        sample_reports.append({"name": name, "sha256": sample["sha256"], "vtf": metadata, "pixels": probe.check_pixels(faces)})
    require(len({sample["sha256"] for sample in sample_reports}) == len(probe.SAMPLES), "Different sample positions produced identical captures")
    hdr_reports = []
    for ldr, sample in zip(manifest["samples"], manifest["hdr_samples"], strict=True):
        require(all(sample[key] == ldr[key] for key in ["helper_id", "origin", "size_enum", "edge"]), "HDR/LDR sample pair metadata differs")
        name = sample["texture"]
        require(name == ldr["texture"].removesuffix(".vtf") + ".hdr.vtf" and name in entries, f"Missing or foreign sample: {sample}")
        pixels = entries[name]
        require(probe.digest(pixels) == sample["sha256"] != sample["bootstrap_sha256"], "HDR sample hash is stale or still names the bootstrap")
        metadata, images = hdr_cube(pixels, sample["edge"], True)
        faces = hdr_faces(images)
        probe.save_faces(output / (Path(name).name + "-faces.png"), faces)
        hdr_reports.append({"name": name, "sha256": sample["sha256"], "vtf": metadata, "pixels": probe.check_pixels(faces)})
    registered = {sample["texture"] for sample in [*manifest["samples"], *manifest["hdr_samples"]]}
    for key, suffix in [("default_texture", ".vtf"), ("hdr_default_texture", ".hdr.vtf")]:
        asset = manifest.get(key)
        if asset:
            name = asset["path"]
            require(name == f"materials/maps/{namespace}/cubemapdefault{suffix}" and name in entries and probe.digest(entries[name]) == asset["sha256"], "Registered reflection default is missing, foreign or stale")
            if key == "hdr_default_texture":
                hdr_cube(entries[name], struct.unpack_from("<H", entries[name], 16)[0], True)
            registered.add(name)
    generated_textures = {name for name in entries if name.startswith(f"materials/maps/{namespace}/") and name.endswith(".vtf")}
    require(generated_textures == registered, "Unregistered generated reflection textures remain in the published namespace")
    aliases = {alias["alias"] for alias in manifest["aliases"]}
    for alias in manifest["aliases"]:
        name = "materials/" + alias["alias"] + ".vmt"
        require(alias["alias"].startswith(f"maps/{namespace}/") and name in entries, "Missing or foreign generated material")
        require(probe.digest(entries[name]) == alias["sha256"], "Generated alias hash mismatch")
        text = entries[name].decode()
        envmaps = re.findall(r'"\$envmap"\s+"([^"\n]+)"', text, re.IGNORECASE)
        require(sorted(set(envmaps)) == alias["envmaps"] and "env_cubemap" not in text.lower(), "Generated material retained an unresolved cubemap reference")
        sample_texture = manifest["samples"][alias["sample"]]["texture"].removeprefix("materials/").removesuffix(".vtf")
        require(sample_texture in envmaps, "Alias does not select its registered sample")
        require(all(not name.endswith(".hdr") and not name.endswith(".vtf") for name in envmaps), "Alias bypassed native HDR/LDR suffix selection")
        require("unknown_metadata" in text and "retained" in text, "Alias flattening discarded unknown authored material metadata")
    strings = lumps[43][2]
    offsets = [row[0] for row in struct.iter_unpack("<I", lumps[44][2])]
    names = []
    for row in struct.iter_unpack("<3f5i", lumps[2][2]):
        offset = offsets[row[3]]
        end = strings.index(0, offset)
        names.append(strings[offset:end].decode().lower())
    infos = list(struct.iter_unpack("<16f2i", lumps[6][2]))
    bindings = []
    for index, row in enumerate(infos):
        alias = names[row[17]]
        if alias in aliases:
            bindings.append({"index": index, "alias": alias, "sha256": probe.digest(lumps[6][2][index * 72:(index + 1) * 72])})
    require(bindings and bindings == manifest["texinfo_bindings"], "Actual TexInfo vectors/materials differ from registered bindings")
    alias_infos = {row["index"] for row in bindings}
    consumers = []
    original_materials = {alias["material"] for alias in manifest["aliases"]}
    for lump, size, offset in [(7, 56, 10), (27, 56, 10), (19, 8, 2)]:
        payload = lumps[lump][2]
        require(len(payload) % size == 0, "Malformed reflection consumer lump")
        for index in range(len(payload) // size):
            texinfo = struct.unpack_from("<h", payload, index * size + offset)[0]
            if texinfo >= 0:
                require(texinfo < len(infos), "Reflection consumer has an invalid TexInfo index")
                require(names[infos[texinfo][17]] not in original_materials, "A geometry consumer still uses an unbound reflective material")
            if texinfo in alias_infos:
                consumers.append({"lump": lump, "index": index, "texinfo": texinfo})
    require(consumers and consumers == manifest["face_bindings"], "Actual face/brushside alias consumers differ from registered bindings")
    return {"manifest": manifest, "samples": sample_reports, "hdr_samples": hdr_reports, "pak": records, "bound_texinfos": len(bindings), "bound_consumers": len(consumers), "unused_original_materials": sorted(original_materials), "bsp_sha256": probe.digest(data)}


def ppm(path):
    data = probe.read_bounded(path, 4 * probe.WIDTH * probe.HEIGHT)
    match = re.match(rb"P6\s+(\d+)\s+(\d+)\s+255\s", data)
    require(match is not None, "Invalid PPM screenshot")
    width, height = map(int, match.groups())
    pixels = data[match.end():]
    require((width, height) == (probe.WIDTH, probe.HEIGHT) and len(pixels) == width * height * 3, "Native viewport differs from the verified dimensions")
    return pixels


def difference(first, second):
    left, top, right, bottom = REGION
    a = b"".join(first[(y * probe.WIDTH + left) * 3:(y * probe.WIDTH + right) * 3] for y in range(top, bottom))
    b = b"".join(second[(y * probe.WIDTH + left) * 3:(y * probe.WIDTH + right) * 3] for y in range(top, bottom))
    delta = [abs(x - y) for x, y in zip(a, b, strict=True)]
    count = len(a) // 3
    return {
        "region": REGION,
        "mean_component_difference": sum(delta) / len(delta),
        "changed_fraction": sum(max(delta[index:index + 3]) > 8 for index in range(0, len(delta), 3)) / count,
        "first_rgb": [sum(a[axis::3]) / count for axis in range(3)],
        "second_rgb": [sum(b[axis::3]) / count for axis in range(3)],
    }


def verify_hardware(records, level):
    failures = [row for row in records if row["event"].startswith("invalid_") or row["event"] in ["acquisition_failed", "thread_failed", "ready_timeout"]]
    require(not failures, f"Native hardware observer failed: {failures}")
    if not records or records[-1]["event"] != "complete":
        return None
    samples = [row for row in records if row["event"] == "sample"]
    require(len(samples) == 8 and all(row["dx_support_level"] >= 90 and row["hardware_hdr_type"] in (1, 2) for row in samples), "Hardware HDR capability was not measured")
    require(all(row["hdr_type"] in (0, 1, 2) and row["hdr_enabled"] == (level == 2) and (row["hdr_type"] != 0) == (level == 2) for row in samples), "Native hardware selected the wrong HDR state")
    require(len({(row["hdr_type"], row["hdr_enabled"]) for row in samples}) == 1, "Native HDR state changed during sampling")
    factory = [row for row in records if row["event"] == "hardware_factory" and row["present"] and row["code"] == 0 and row["version"] == HARDWARE_VERSION]
    require(factory, "The verified native hardware interface was not acquired")
    starts = [row for row in records if row["event"] == "start"]
    require(len(starts) == 1, "Native hardware observer process identity is missing")
    return {"samples": samples, "factory": factory[-1], "native_pid": starts[0]["pid"]}


def worker(args):
    output = args.output
    require(not os.environ.get("LD_PRELOAD"), "Private native validation must not inherit LD_PRELOAD")
    require(os.environ.get("WAYLAND_DISPLAY") and os.environ.get("XDG_RUNTIME_DIR") == str(output / "wayland"), "Native view requires its private compositor")
    require(re.fullmatch(r"[a-z0-9_-]{1,64}", args.map_name), "Invalid private native map basename")
    require(probe.digest((args.game_dir / "bin/linux64/materialsystem.so").read_bytes()) == MATERIAL_SYSTEM_SHA256, "Material system changed; repeat native HDR suffix selection evidence")
    game = runtime.private_game(args.game_dir, output, engine_physics=True)
    shutil.copytree(args.fixture_game / "materials" / MATERIALS, game / "materials" / MATERIALS)
    data = probe.read_bounded(args.bsp, probe.MAX_BSP_BYTES)
    bindings = verify_assets(data, args.map_name, output / "verified-assets")
    entities = probe.bsp_lumps(data)[0][2]
    require(b'hammerr_reflection_observer' not in entities, "Observer name collides with authored source")
    observer = b'\n{\n"classname" "logic_script"\n"targetname" "hammerr_reflection_observer"\n"origin" "-192 -176 80"\n"vscripts" "hammerr_reflection_observer.nut"\n"thinkfunction" "HammeRRThink"\n}\n\0'
    staged = game / "maps" / f"{args.map_name}.bsp"
    staged.write_bytes(patch_lump(data, 0, entities.rstrip(b"\0") + observer))
    (game / "scripts/vscripts/hammerr_reflection_observer.nut").write_text(private_observer(OBSERVER))
    cfg = game / "cfg/hammerr_reflection_once.cfg"
    probe.write_cfg(cfg, "echo HAMMERR_REFLECTION_WAIT\n")
    post_map = "toggle sv_lan 1\ntoggle mat_fullbright 0\nhelp sv_lan\nhelp mat_hdr_level\nhelp mat_fullbright\n" + "".join(f"help {name}\n" for name in HDR_CONTROLS)
    for name in ["server.cfg", "listenserver.cfg", "hammerr_reflection_postmap.cfg"]:
        (game / "cfg" / name).write_text(post_map)
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    settings = output / "settings"
    for name in ["config", "cache", "data"]:
        (settings / name).mkdir(parents=True)
    ready = output / "probe-ready"
    hardware_log = output / "hardware.jsonl"
    environment = {
        **os.environ, "LD_LIBRARY_PATH": str(args.game_dir / "bin/linux64"),
        "SteamAppId": "240", "SteamGameId": "240", "SDL_VIDEODRIVER": "x11",
        "XDG_CONFIG_HOME": str(settings / "config"), "XDG_CACHE_HOME": str(settings / "cache"),
        "XDG_DATA_HOME": str(settings / "data"),
        "LD_PRELOAD": str(args.probe_library),
        "HAMMERR_HDR_EXPECTED_EXE": str((args.game_dir / "cstrike_linux64").resolve(strict=True)),
        "HAMMERR_HDR_LIBRARY_DIR": str((args.game_dir / "bin/linux64").resolve(strict=True)),
        "HAMMERR_HDR_PRIVATE_GAME": str(game),
        "HAMMERR_HDR_READY": str(ready),
        "HAMMERR_HDR_REPORT": str(hardware_log),
    }
    flags = private_launch_options(args.game_dir, game, hdr_level=args.hdr_level, map_name=args.map_name, log_name="hammerr-reflections.log", controls={"servercfgfile": "hammerr_reflection_postmap.cfg", "mat_fullbright": 0, **HDR_CONTROLS})
    native = [str(args.game_dir / "cstrike_linux64"), "-game", str(game), "-windowed", "-w", str(probe.WIDTH), "-h", str(probe.HEIGHT), "-novid", *flags, "-nomaster", "-nojoy", "-console", "-port", str(args.port)]
    command, environment = private_readonly_launch(native, output, environment)
    report = {"command": command, "map": args.map_name, "hdr_level": args.hdr_level, "probe_sha256": probe.digest(args.probe_library.read_bytes()), "bsp_sha256": probe.digest(data), "private_entity_adapter_only": True, "filesystem_isolation": {"external_paths": "read_only", "writable_directory": str(output)}, "desktop_input": False, "success": False}
    console = game / "hammerr-reflections.log"
    process = None
    try:
        with (output / "launcher.log").open("w") as stream:
            process = subprocess.Popen(command, cwd=args.game_dir, env=environment, stdin=subprocess.DEVNULL, stdout=stream, stderr=stream)
            started = time.monotonic()
            on = off = False
            while time.monotonic() - started < 90:
                text = probe.read_bounded(console, 2 * 1024**2).decode(errors="replace") if console.exists() else ""
                if not ready.exists() and ON_READY in text:
                    ready.write_text("post-map reflection observer ready\n")
                hardware = verify_hardware(read_probe(hardware_log), args.hdr_level)
                if not on and ON_READY in text and hardware:
                    report["private_launch"] = verify_private_launch(game, text)
                    require(re.search(r'"mat_specular" = "1"', text), "Specular ON was not confirmed by the installed engine")
                    for extension in ["png", "ppm"]:
                        subprocess.run(["grim", *( ["-t", "ppm"] if extension == "ppm" else []), str(output / f"specular-on.{extension}")], check=True, timeout=5)
                    ppm(output / "specular-on.ppm")
                    probe.write_cfg(cfg, 'toggle mat_specular 0\nhelp mat_specular\nent_fire hammerr_reflection_observer RunScriptCode "stage = 4; ready_at = Time() + 4"\n')
                    on = True
                if on and not off and OFF_READY in text:
                    require(re.search(r'"mat_specular" = "0"', text), "Specular OFF was not confirmed by the installed engine")
                    for extension in ["png", "ppm"]:
                        subprocess.run(["grim", *( ["-t", "ppm"] if extension == "ppm" else []), str(output / f"specular-off.{extension}")], check=True, timeout=5)
                    off = True
                    probe.write_cfg(cfg, f"echo {FINISHED}\ndisconnect\nquit\n")
                if process.poll() is not None:
                    break
                time.sleep(0.02)
            else:
                raise TimeoutError("Native reflection frame handshake exceeded 90 seconds")
            text = probe.read_bounded(console, 2 * 1024**2).decode(errors="replace")
            require(process.returncode == 0 and on and off and FINISHED in text, f"Native reflection view failed: {report}; inspect {console}")
            require(text.index(ON_READY) < text.index(OFF_READY) < text.index(FINISHED), "Reflection frames were taken outside the ordered handshake")
            report["private_launch"] = verify_private_launch(game, text)
            controls = {}
            for name, expected in {"sv_lan": 1, "mat_hdr_level": args.hdr_level, "mat_fullbright": 0, "mat_specular": 0, **HDR_CONTROLS}.items():
                values = re.findall(r'"' + re.escape(name) + r'" = "([^"\n]+)"', text)
                require(values and float(values[-1]) == expected, f"Native {name} control differs from the proof: {values}")
                controls[name] = float(values[-1])
            require(not re.search(r'(?:Error reading texture data "|couldn.t find )maps/' + re.escape(args.map_name) + r'/', text, re.IGNORECASE), "Native engine rejected a generated reflection texture")
            require(not re.search(r"fall\s*back to a non-HDR version|failed to load.*\.hdr|error reading texture data.*\.hdr|(?:couldn.t|can.t) find.*\.hdr", text, re.IGNORECASE), "Native HDR reflection lookup used a missing-texture or LDR fallback")
            unused_warnings = sorted(set(re.findall(r"env_cubemap used on world geometry[^\r\n]*ignoring: ([^\r\n]+)", text)))
            require(set(unused_warnings).issubset(bindings["unused_original_materials"]), f"The engine reported an unverified unbound material: {unused_warnings}")
            report["unused_material_diagnostics"] = unused_warnings
            report["geometry_consumers_verified"] = bindings["bound_consumers"]
            delta = difference(ppm(output / "specular-on.ppm"), ppm(output / "specular-off.ppm"))
            require(delta["mean_component_difference"] > 8 and delta["changed_fraction"] > 0.05, f"The actual floor did not display a measurable reflection: {delta}")
            gains = [on - off for on, off in zip(delta["first_rgb"], delta["second_rgb"], strict=True)]
            require(max(gains[1:]) > gains[0] + 5, f"The reflection lacks the expected green/blue directional signature: {gains}")
            delta["channel_gains"] = gains
            report.update({"success": True, "controls": controls, "hardware": hardware, "hdr_fallback_absent": True, "reflection_pixels": delta, "elapsed_seconds": time.monotonic() - started})
    except BaseException as error:
        report.update({"error": str(error), "traceback": traceback.format_exc()})
        raise
    finally:
        if process is not None and process.poll() is None:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
        try:
            report["configuration_preservation"] = finish_private_launch(game)
        except BaseException as error:
            report.update({"success": False, "preservation_error": str(error)})
            raise
        finally:
            report["exit_code"] = None if process is None else process.returncode
            (output / "runtime.json").write_text(json.dumps(report, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser(description="Prove production LDR/HDR reflection publication, all-mip HDR RGB preservation, Run rebasing and actual private native reflection consumption.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--game-dir", type=Path, default=Path.home() / ".local/share/Steam/steamapps/common/Counter-Strike Source")
    parser.add_argument("--steam-root", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--timeout", type=int, default=600)
    parser.add_argument("--port", type=int, default=27683)
    parser.add_argument("--probe-library", type=Path, default=DEFAULT_BINARY.parent / "hammerr_hdr_probe.so")
    parser.add_argument("--reuse-fixture", type=Path)
    parser.add_argument("--worker", action="store_true")
    parser.add_argument("--bsp", type=Path)
    parser.add_argument("--fixture-game", type=Path)
    parser.add_argument("--map-name")
    parser.add_argument("--hdr-level", type=int, choices=[0, 2], default=0)
    args = parser.parse_args()
    args.output = args.output.resolve()
    args.binary = args.binary.resolve(strict=True)
    args.game_dir = args.game_dir.resolve(strict=True)
    args.probe_library = args.probe_library.resolve(strict=True)
    require(60 <= args.timeout <= 900 and 1024 <= args.port <= 65531, "Invalid validation time or port limit")
    if args.worker:
        require(args.bsp and args.fixture_game and args.map_name, "Private worker arguments are incomplete")
        worker(args)
        return
    require(not args.output.exists(), "Use a fresh absent validation output directory")
    require(not os.environ.get("LD_PRELOAD"), "Native validation must not inherit LD_PRELOAD")
    args.output.mkdir(parents=True, mode=0o700)
    before = protected_files(args, ["hdr_reflections"])
    report = {"module_fingerprints": probe.module_fingerprints(args.game_dir), "success": False}
    try:
        if args.reuse_fixture:
            fixture = json.loads(args.reuse_fixture.read_text())["fixture"]
            require(probe.digest(Path(fixture["bsp"]).read_bytes()) == fixture["published"]["bsp_sha256"], "Reused published BSP changed")
            require(probe.digest((Path(fixture["bsp"]).parent / "run-staged.bsp").read_bytes()) == fixture["run"]["verification"]["bsp_sha256"], "Reused Run BSP changed")
        else:
            fixture = author(args)
        report["fixture"] = fixture
        source = Path(fixture["source"])
        bsp = Path(fixture["bsp"])
        preserved = source.read_bytes(), bsp.read_bytes(), source_assets(Path(fixture["fixture_game"]))
        run_bsp = Path(fixture["bsp"]).parent / "run-staged.bsp"
        names = ["hdr_reflections", fixture["run"]["map"]]
        public_before = protected_files(args, names)
        require(protected_files(args, ["hdr_reflections"]) == before, "Generation changed installed source assets")
        views = {}
        for index, (name, path, namespace, level) in enumerate([
            ("published-ldr", bsp, "hdr_reflections", 0),
            ("published-hdr", bsp, "hdr_reflections", 2),
            ("run-ldr", run_bsp, fixture["run"]["map"], 0),
            ("run-hdr", run_bsp, fixture["run"]["map"], 2),
        ]):
            destination = args.output / name
            command = [sys.executable, "-B", str(Path(__file__).resolve()), "--worker", "--binary", str(args.binary), "--output", str(destination), "--game-dir", str(args.game_dir), "--bsp", str(path), "--fixture-game", fixture["fixture_game"], "--map-name", namespace, "--port", str(args.port + index), "--hdr-level", str(level), "--probe-library", str(args.probe_library)]
            runtime.private_compositor(command, destination, timeout=125)
            views[name] = json.loads((destination / "runtime.json").read_text())
            require(views[name]["success"] is True, views[name])
        require(len({view["hardware"]["native_pid"] for view in views.values()}) == 4, "Native channel views reused a process")
        parity = {}
        for channel in ["ldr", "hdr"]:
            delta = difference(ppm(args.output / f"published-{channel}/specular-on.ppm"), ppm(args.output / f"run-{channel}/specular-on.ppm"))
            require(delta["mean_component_difference"] < 3 and delta["changed_fraction"] < 0.02, f"Run rebasing visibly changed the {channel} reflective floor: {delta}")
            parity[channel] = delta
        require((source.read_bytes(), bsp.read_bytes(), source_assets(Path(fixture["fixture_game"]))) == preserved, "Private native views changed the published BSP or source assets")
        require(protected_files(args, names) == public_before, "Private native views changed installed game assets")
        report.update({"success": True, "views": views, "run_pixel_parity": parity, "public_and_source_assets_preserved": True, "desktop_input": False})
    except BaseException as error:
        report.update({"error": str(error), "traceback": traceback.format_exc()})
        raise
    finally:
        report["initial_installed_assets_preserved"] = all(protected_files(args, ["hdr_reflections"]).get(path) == value for path, value in before.items())
        if not report["initial_installed_assets_preserved"]:
            report["success"] = False
        (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
        require(report["initial_installed_assets_preserved"], "Validation changed installed source assets")
    print(json.dumps({"success": True, "results": str(args.output / "results.json")}, indent=2))


if __name__ == "__main__":
    main()
