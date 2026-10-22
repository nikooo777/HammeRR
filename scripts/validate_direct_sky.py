import argparse
import json
import math
import re
import struct
from pathlib import Path

from validate_authored_planes import records
from validate_compile_jobs import wait_job
from validate_direct_lighting import center_sample, floor_samples, lightmaps
from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_visibility import ROOM


def sample(data, target):
    return next(
        color
        for point, color, _, _ in floor_samples(data)
        if all(abs(a - b) < 0.001 for a, b in zip(point, target, strict=True))
    )


def ambient_probes(data, stats):
    indices = records(data, 52, "<2H")
    samples = records(data, 56, "<24s4B", expected_version=1)
    leaves = records(data, 10, "<ihh6h4Hhh", expected_version=1)
    assert len(indices) == len(leaves)
    assert len(samples) == stats["ambient_samples"] > 0
    assert stats["ambient_bytes"] == len(indices) * 4 + len(samples) * 28
    colors = []
    for owner, (count, first) in enumerate(indices):
        if not count:
            assert first < len(leaves)
            continue
        assert (
            1 <= count <= 8 and first + count <= len(samples) and leaves[owner][1] >= 0
        )
        positions = []
        for cube, x, y, z, padding in samples[first : first + count]:
            assert padding == 0
            point = [
                leaves[owner][3 + axis]
                + (leaves[owner][6 + axis] - leaves[owner][3 + axis]) * fraction / 255
                for axis, fraction in enumerate([x, y, z])
            ]
            assert all(math.dist(point, earlier) > 128 for earlier in positions)
            positions.append(point)
            for red, green, blue, exponent in struct.iter_unpack("<3Bb", cube):
                colors.append([channel * 2**exponent for channel in (red, green, blue)])
    return colors


def validate(client, output, reference=None):
    results = []
    for name in [
        "clear",
        "shade",
        "enclosed",
        "tilted",
        "terrain-shade",
        "hidden",
        "spread",
        "duplicate",
        "ambient",
        "ambient-shade",
        "ambient-enclosed",
        "ambient-only",
        "ambient-fallback",
    ]:
        client.call("new_map", {"discard_changes": True})
        walls = [
            client.call(
                "create_block",
                {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"},
            )["id"]
            for low, high in ROOM
        ]
        if name not in ["enclosed", "ambient-enclosed"]:
            ceiling = detail(client, [walls[1]])[0]
            bottom = next(
                face for face in ceiling["faces"] if face["normal"][2] < -0.99
            )
            client.call(
                "select_faces",
                {"faces": [{"solid": ceiling["solid"], "side": bottom["side"]}]},
            )
            client.call("edit_faces", {"material": "tools/toolsskybox"})
        client.call(
            "create_entity",
            {"classname": "info_player_terrorist", "origin": [-64, 0, 8]},
        )
        client.call(
            "create_entity",
            {
                "classname": "logic_script",
                "origin": [0, 0, 32],
                "properties": {
                    "targetname": "hammerr_observer",
                    "vscripts": "hammerr_observer.nut",
                    "thinkfunction": "HammeRRThink",
                },
            },
        )
        client.call(
            "create_entity",
            {
                "classname": "prop_physics",
                "origin": [160, -128, 128],
                "properties": {
                    "targetname": "physics_crate",
                    "model": "models/props_junk/wood_crate001a.mdl",
                },
            },
        )
        properties = {
            "_light": "0 0 0 0" if name == "ambient-only" else "255 0 0 200",
            "_ambient": "0 0 255 100" if name.startswith("ambient") else "0 0 0 0",
            "SunSpreadAngle": "91" if name == "spread" else "0",
            "pitch": "-45" if name == "tilted" else "-90",
        }
        sun = client.call(
            "create_entity",
            {
                "classname": "light_environment",
                "origin": [0, 0, 128],
                "properties": properties,
            },
        )["id"]
        if name == "ambient-fallback":
            client.call("set_entity_properties", {"id": sun, "remove": ["_ambient"]})
        if name == "duplicate":
            client.call(
                "create_entity",
                {
                    "classname": "light_environment",
                    "origin": [64, 0, 128],
                    "properties": properties,
                },
            )
        if name == "hidden":
            client.call("set_object_visibility", {"ids": [sun], "action": "hide"})
        terrain_side = None
        if name in ["shade", "ambient-shade"]:
            client.call(
                "create_block",
                {
                    "mins": [-16, -16, 32],
                    "maxs": [16, 16, 48],
                    "material": "dev/dev_measuregeneric01",
                },
            )
        if name == "terrain-shade":
            terrain = client.call(
                "create_block",
                {
                    "mins": [0, 0, 0],
                    "maxs": [128, 128, 16],
                    "material": "dev/dev_measuregeneric01",
                },
            )["id"]
            terrain_side = next(
                face["side"]
                for face in detail(client, [terrain])[0]["faces"]
                if face["normal"][2] > 0.99
            )
        vmf = output / f"{name}.vmf"
        client.call("save_map", {"path": str(vmf)})
        if terrain_side:
            normals = "\n".join(
                f'"row{row}" "{" ".join(["0 0 1"] * 5)}"' for row in range(5)
            )
            distances = "\n".join(
                f'"row{row}" "{" ".join(["16"] * 5)}"' for row in range(5)
            )
            disp = f'dispinfo {{ "power" "2" "startposition" "[0 0 16]" "flags" "0" "elevation" "0" "subdiv" "0" normals {{ {normals} }} distances {{ {distances} }} }}'
            contents, count = re.subn(
                rf'(\bside\s*\{{\s*"id"\s*"{terrain_side}"[^{{}}]*)(\}})',
                lambda match, disp=disp: match[1] + disp + "\n" + match[2],
                vmf.read_text(),
            )
            assert count == 1
            vmf.write_text(contents)
        client.call("open_map", {"path": str(vmf)})
        client.call(
            "set_compile_settings", {"lighting": "direct_ldr", "physics": "engine"}
        )
        before = detail(client, [sun])
        history = client.call("get_map_info")["undo"]
        source = vmf.read_bytes()
        bsp = output / f"{name}.bsp"
        if name in ["spread", "duplicate"]:
            bsp.write_bytes(b"previous BSP")
            failed = client.call("compile_map", {"path": str(bsp)}, error=True)
            value = json.loads(failed["content"][0]["text"])
            assert not value["success"] and bsp.read_bytes() == b"previous BSP"
            assert any(
                row["level"] == "error"
                and any(word in row["message"].lower() for word in ["sky", "sun"])
                for row in value["diagnostics"]
            ), value
        else:
            compiled = client.call("compile_map", {"path": str(bsp)})
            assert compiled["success"], compiled
            data = bsp.read_bytes()
            cubes = ambient_probes(data, compiled["stats"])
            if name in ["enclosed", "ambient-enclosed", "hidden"]:
                assert all(color == [0, 0, 0] for color in cubes), (name, cubes)
            elif name.startswith("ambient"):
                channel = 0 if name == "ambient-fallback" else 2
                assert any(color[channel] > 0 for color in cubes), (name, cubes)
            expected = [
                0
                if name in ["shade", "enclosed", "terrain-shade", "hidden"]
                else 141
                if name == "tilted"
                else 200,
                0,
                0,
            ]
            ambient_expected = {
                "ambient": [200, 0, 60],
                "ambient-shade": [0, 0, 37.5],
                "ambient-enclosed": [0, 0, 0],
                "ambient-only": [0, 0, 60.75],
                "ambient-fallback": [260, 0, 0],
            }
            expected = ambient_expected.get(name, expected)
            assert center_sample(data) == expected, (name, center_sample(data))
            if reference and not name.startswith("ambient"):
                assert data == (reference / f"{name}.bsp").read_bytes(), (
                    f"{name}: zero-ambient BSP changed"
                )
            if name == "tilted":
                assert sample(data, [-128, 0, 0]) == [0, 0, 0]
                assert sample(data, [128, 0, 0]) == [141, 0, 0]
            elif name in ["shade", "terrain-shade"]:
                assert sample(data, [-128, 0, 0]) == [200, 0, 0]
            lights = records(data, 15, "<9f3i7f3i")
            assert (
                len(lights)
                == compiled["stats"]["world_lights"]
                == (0 if name == "hidden" else 2 if name.startswith("ambient") else 1)
            )
            if lights:
                light = lights[0]
                direction = (
                    [math.sqrt(0.5), 0, -math.sqrt(0.5)]
                    if name == "tilted"
                    else [0, 0, -1]
                )
                assert light[9:12] == (-1, 3, 0)
                assert all(
                    abs(a - b) < 1e-6
                    for a, b in zip(light[6:9], direction, strict=True)
                )
                assert (
                    abs(light[3] - (0 if name == "ambient-only" else 200 / 255)) < 1e-6
                )
                if name.startswith("ambient"):
                    assert lights[1][10] == 5 and lights[1][6:9] == (0, 0, 0)
            background = output / f"{name}-background.bsp"
            client.call("start_compile", {"path": str(background)})
            job = wait_job(client)
            assert job["status"] == "completed" and background.read_bytes() == data
            results.append(
                {"name": name, "stats": compiled["stats"], "center": expected}
            )
        assert (
            before == detail(client, [sun])
            and history == client.call("get_map_info")["undo"]
            and source == vmf.read_bytes()
        )
    (output / "results.json").write_text(json.dumps(results, indent=2) + "\n")


def quality_checks(client, output):
    client.call("open_map", {"path": str(output / "ambient.vmf")})
    client.call(
        "set_compile_settings", {"lighting": "direct_ldr", "sky_quality": "fast"}
    )
    settings = client.call("get_compile_settings")
    assert [choice["directions"] for choice in settings["sky_choices"]] == [
        32,
        128,
        512,
    ]
    client.call(
        "set_compile_settings",
        {"lighting": "fullbright", "sky_quality": "unknown"},
        error=True,
    )
    assert client.call("get_compile_settings") == settings
    client.call("set_compile_settings", {"sky_quality": "balanced"})
    client.call("set_compile_settings", {"lighting": "fullbright"})
    assert client.call("get_compile_settings")["sky_quality"] == "balanced"
    client.call("set_compile_settings", {"lighting": "direct_ldr"})
    source = (output / "ambient.vmf").read_bytes()
    history = client.call("get_map_info")["undo"]
    results = []
    for quality, rays in [("fast", 32), ("balanced", 128), ("detailed", 512)]:
        client.call("set_compile_settings", {"sky_quality": quality})
        destination = output / f"quality-{quality}.bsp"
        result = client.call("compile_map", {"path": str(destination)})
        assert result["success"], result
        data = destination.read_bytes()
        if quality == "fast":
            assert data == (output / "ambient.bsp").read_bytes()
        client.call(
            "start_compile", {"path": str(output / f"quality-{quality}-background.bsp")}
        )
        client.call("set_compile_settings", {"sky_quality": "balanced"}, error=True)
        job = wait_job(client)
        assert job["settings"]["sky_quality"] == quality
        assert (
            job["status"] == "completed"
            and (output / f"quality-{quality}-background.bsp").read_bytes() == data
        )
        results.append(
            {
                "quality": quality,
                "directions": rays,
                "center": center_sample(data),
                "work": result["stats"]["lighting_work"],
            }
        )
    assert (
        source == (output / "ambient.vmf").read_bytes()
        and history == client.call("get_map_info")["undo"]
    )
    assert results[0]["work"] < results[1]["work"] < results[2]["work"]
    client.call("set_compile_settings", {"sky_quality": "fast"})
    assert client.call("get_compile_status")["job"]["stale"]
    assert not client.call("get_compile_status")["launch"]["ready"]
    for low, high in [
        ([-256, -256, 240], [-32, 256, 256]),
        ([32, -256, 240], [256, 256, 256]),
        ([-32, -256, 240], [32, -32, 256]),
        ([-32, 32, 240], [32, 256, 256]),
    ]:
        client.call(
            "create_block", {"mins": low, "maxs": high, "material": "tools/toolsnodraw"}
        )
    aperture = []
    for quality in ["fast", "detailed"]:
        client.call("set_compile_settings", {"sky_quality": quality})
        destination = output / f"aperture-{quality}.bsp"
        compiled = client.call("compile_map", {"path": str(destination)})
        assert compiled["success"], compiled
        aperture.append(center_sample(destination.read_bytes()))
    assert (
        aperture[0] == [200, 0, 0] and aperture[1][0] == 200 and aperture[1][2] > 0
    ), aperture
    (output / "aperture-results.json").write_text(json.dumps(aperture) + "\n")
    (output / "quality-results.json").write_text(json.dumps(results, indent=2) + "\n")


def soft_sun_checks(client, output):
    results = []
    for name in ["clear", "shade", "terrain-shade", "enclosed"]:
        fixture = "clear" if name == "shade" else name
        client.call(
            "open_map",
            {"path": str(output / f"{fixture}.vmf"), "discard_changes": True},
        )
        sun = client.call("find_objects", {"classname": "light_environment"})[
            "objects"
        ][0]["id"]
        client.call(
            "set_entity_properties", {"id": sun, "set": {"SunSpreadAngle": "20"}}
        )
        if name == "shade":
            client.call(
                "create_block",
                {
                    "mins": [-32, -128, 96],
                    "maxs": [32, 128, 112],
                    "material": "dev/dev_measuregeneric01",
                },
            )
        vmf = output / f"soft-{name}.vmf"
        client.call("save_map", {"path": str(vmf)})
        original = vmf.read_bytes()
        source = detail(client, [sun])
        history = client.call("get_map_info")["undo"]
        last_work = 0
        for quality in ["fast", "balanced", "detailed"]:
            client.call("set_compile_settings", {"sky_quality": quality})
            destination = output / f"soft-{name}-{quality}.bsp"
            built = client.call("compile_map", {"path": str(destination)})
            assert built["success"], built
            data = destination.read_bytes()
            ambient_probes(data, built["stats"])
            center = center_sample(data)
            if name == "clear":
                assert center == [200, 0, 0], center
            elif name == "enclosed":
                assert center == [0, 0, 0], center
            else:
                assert 0 < center[0] < 200 and center[1:] == [0, 0], (
                    name,
                    quality,
                    center,
                )
            assert built["stats"]["lighting_work"] > last_work
            last_work = built["stats"]["lighting_work"]
            background = output / f"soft-{name}-{quality}-background.bsp"
            client.call("start_compile", {"path": str(background)})
            job = wait_job(client)
            assert job["status"] == "completed" and background.read_bytes() == data
            assert job["settings"]["sky_quality"] == quality
            assert (
                source == detail(client, [sun])
                and history == client.call("get_map_info")["undo"]
            )
            assert vmf.read_bytes() == original
            results.append(
                {"case": name, "quality": quality, "center": center, "work": last_work}
            )
    (output / "soft-sun-results.json").write_text(json.dumps(results, indent=2) + "\n")


def local_ambient_checks(client, output):
    client.call("set_compile_settings", {"lighting": "direct_ldr", "physics": "engine"})
    client.call(
        "open_map", {"path": str(output / "clear.vmf"), "discard_changes": True}
    )
    sun = client.call("find_objects", {"classname": "light_environment"})["objects"][0][
        "id"
    ]
    client.call("delete_objects", {"ids": [sun]})
    client.call(
        "create_entity",
        {
            "classname": "light",
            "origin": [0, 0, 100],
            "properties": {"_light": "255 0 0 200", "_quadratic_attn": "1"},
        },
    )
    vmf = output / "local-gradient.vmf"
    client.call("save_map", {"path": str(vmf)})
    source = vmf.read_bytes()
    history = client.call("get_map_info")["undo"]
    cases = []
    counts = []
    for quality in ["fast", "balanced", "detailed"]:
        client.call("set_compile_settings", {"sky_quality": quality})
        path = output / f"local-gradient-{quality}.bsp"
        built = client.call("compile_map", {"path": str(path)})
        assert built["success"], built
        data = path.read_bytes()
        colors = ambient_probes(data, built["stats"])
        counts.append(built["stats"]["ambient_samples"])
        cap = {"fast": 1, "balanced": 4, "detailed": 8}[quality]
        indices = records(data, 52, "<2H")
        assert all(count <= cap for count, _ in indices)
        assert any(color[0] > 0 for color in colors)
        background = output / f"local-gradient-{quality}-background.bsp"
        client.call("start_compile", {"path": str(background)})
        assert (
            wait_job(client)["status"] == "completed"
            and background.read_bytes() == data
        )
        assert (
            source == vmf.read_bytes()
            and history == client.call("get_map_info")["undo"]
        )
        cases.append(data)
    assert counts[0] < counts[1] <= counts[2], counts
    assert lightmaps(cases[0]) == lightmaps(cases[1]) == lightmaps(cases[2])
    samples = []
    for data in cases:
        offset, length, _, _ = struct.unpack_from("<4i", data, 8 + 56 * 16)
        samples.append(data[offset : offset + length])
    assert samples[0] != samples[1]



def refinement_control(args):
    import hashlib
    import shutil
    from unittest.mock import patch

    from validate_brush_instances import game_files, source_state, text
    from validate_material_albedo import payload

    output = args.output.resolve()
    assert "target" in output.parts, "Use a fresh output directory under target/"
    output.mkdir(parents=True, exist_ok=False)
    original_source, original_game = args.refine_ambient_control.resolve(), args.game.resolve()
    originals = [original_source, original_game, *sorted((original_game.parent / "materials").rglob("*"))]
    originals = {path: path.read_bytes() for path in originals if path.is_file()}
    source = output / "scene.vmf"
    source.write_bytes(originals[original_source])
    game = output / "game"
    game.mkdir()
    gameinfo = game / "gameinfo.txt"
    gameinfo.write_bytes(originals[original_game])
    shutil.copytree(original_game.parent / "materials", game / "materials")
    for path in [source, *game.rglob("*")]:
        if path.is_file():
            path.chmod(0o444)
    receipt = {"status": "running", "cases": {}, "native_exercised": False,
               "fixture_sha256": hashlib.sha256(source.read_bytes()).hexdigest(),
               "binary_sha256": hashlib.sha256(args.binary.resolve().read_bytes()).hexdigest()}
    client = None
    calls = None
    try:
        client = Client(None, args.binary.resolve(), output, gameinfo=gameinfo, environment={
            "HAMMERR_RECENT_MAPS_DIR": str(output / "recent"), "HAMMERR_BUILD_RECIPES": str(output / "recipes.json"),
            "XDG_CONFIG_HOME": str(output / "config"), "XDG_STATE_HOME": str(output / "state")})
        data_by_option = []
        with patch.object(client, "call", wraps=client.call) as calls:
            for enabled in (False, True):
                client.call("open_map", {"path": str(source)})
                settings = client.call("set_compile_settings", {"lighting": "direct_ldr_hdr", "physics": "none",
                    "sky_quality": "balanced", "bounces": 0, "refine_ambient_probes": enabled})
                assert settings["refine_ambient_probes"] is enabled and settings["ambient_probe_refinement"]["available"]
                assert settings["lighting"] == "direct_ldr_hdr" and settings["sky_quality"] == "balanced" and settings["physics"] == "none"
                token = client.call("begin_property_edit", {"world": True})["draft"]["token"]
                client.call("update_property_edit", {"token": token, "changes": [{"action": "set", "key": "message", "value": "Retained ambient review"}]})
                before = (source_state(client, source), client.call("get_compile_settings"), client.call("get_property_edit"), game_files(game))
                outputs, jobs = [], []
                try:
                    for background in (False, True):
                        path = output / f"{'on' if enabled else 'off'}{'-background' if background else ''}.bsp"
                        path.write_bytes(b"retained previous ambient BSP")
                        pointfile = path.with_suffix(".lin")
                        pointfile.write_bytes(b"retained ambient pointfile\n")
                        if background:
                            submitted = client.call("start_compile", {"path": str(path)})["job"]
                            job = wait_job(client)
                            assert job["token"] == submitted["token"]
                            built = job["result"]
                        else:
                            built = client.call("compile_map", {"path": str(path)})
                            job = client.call("get_compile_status")["job"]
                        assert built["success"] and job["result"]["published"] and built["leak"] is None and built["bsp"] == str(path)
                        assert job["status"] == "completed" and not job["stale"]
                        assert all(settings[key] == value for key, value in job["settings"].items()), "Frozen job settings differ"
                        assert job["settings"]["refine_ambient_probes"] is enabled
                        assert job["settings"]["lighting"] == built["stats"]["lighting_mode"] == "direct_ldr_hdr"
                        assert job["settings"]["sky_quality"] == "balanced" and job["settings"]["physics"] == "none"
                        assert pointfile.read_bytes() == b"retained ambient pointfile\n"
                        assert (source_state(client, source), client.call("get_compile_settings"), client.call("get_property_edit"), game_files(game)) == before
                        outputs.append(path.read_bytes())
                        jobs.append(job)
                    assert outputs[0] == outputs[1], "Foreground/background complete BSP mismatch"
                    if enabled:
                        accepted = {path: path.read_bytes() for pattern in ("*.bsp", "*.lin") for path in output.glob(pattern)}
                        for update in ({"lighting": "fullbright"}, {"sky_quality": "fast"}):
                            rejected = client.call("set_compile_settings", update, error=True)
                            assert "Balanced or Detailed" in text(rejected)
                            assert (source_state(client, source), client.call("get_compile_settings"), client.call("get_property_edit"), game_files(game)) == before
                            assert all(path.read_bytes() == raw for path, raw in accepted.items())
                    receipt["cases"]["on" if enabled else "off"] = jobs
                    data_by_option.append(outputs[0])
                finally:
                    client.call("cancel_property_edit", {"token": token})
            dispatches = [call.args[0] for call in calls.call_args_list if call.args[0] in ("compile_map", "start_compile")]
            assert dispatches == ["compile_map", "start_compile"] * 2
            receipt["compile_dispatches"] = dispatches
        baseline, refined = data_by_option
        assert baseline[:1036] == refined[:1036], "Refinement changed BSP header or lump descriptors"
        assert all(payload(baseline, slot) == payload(refined, slot) for slot in range(64) if slot not in (55, 56)), "Refinement changed a non-ambient lump"
        changed = {}
        for channel, index_slot, sample_slot in (("ldr", 52, 56), ("hdr", 51, 55)):
            indices = records(baseline, index_slot, "<2H")
            old = records(baseline, sample_slot, "<24s4B", expected_version=1)
            new = records(refined, sample_slot, "<24s4B", expected_version=1)
            assert len(old) == len(new) > 0
            differences = []
            for leaf, (count, first) in enumerate(indices):
                if not count:
                    continue
                assert 1 <= count <= 4 and first + count <= len(old)
                assert old[first:first + count - 1] == new[first:first + count - 1], "An earlier anchor changed"
                if old[first + count - 1] != new[first + count - 1]:
                    assert old[first + count - 1][1:4] != new[first + count - 1][1:4], "Changed color without a new final location"
                    differences.append(leaf)
            assert differences, f"The supplied contrast fixture did not exercise {channel} final-slot replacement"
            for name, records_ in (("off", old), ("on", new)):
                stats = receipt["cases"][name][0]["result"]["stats"]
                if channel == "hdr":
                    stats = stats["hdr"]
                assert stats["ambient_samples"] == len(records_)
            changed[channel] = differences
        receipt["changed_final_slot_leaves"] = changed
        assert not client.call("get_map_info")["rendering"]["initialized"]
        assert all(path.read_bytes() == raw for path, raw in originals.items())
        receipt["status"] = "passed"
    except BaseException as error:
        receipt["status"] = "failed"
        receipt["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        try:
            if client is not None:
                client.close()
        except BaseException as error:
            receipt["status"] = "failed"
            receipt["cleanup_error"] = f"{type(error).__name__}: {error}"
            raise
        finally:
            if calls is not None:
                receipt["compile_dispatches"] = [call.args[0] for call in calls.call_args_list if call.args[0] in ("compile_map", "start_compile")]
            receipt["artifacts"] = [{"path": str(path.relative_to(output)), "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
                                    for path in sorted(output.rglob("*")) if path.is_file() and path.suffix in (".vmf", ".bsp", ".lin")]
            (output / "validation.json").write_text(json.dumps(receipt, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, default=Path("/tmp/hammerr-direct-sky"))
    parser.add_argument(
        "--reference", type=Path, help="Compare zero-ambient BSPs against a prior run"
    )
    parser.add_argument(
        "--quality-tests",
        action="store_true",
        help="Check shared sky quality, frozen jobs and deterministic output",
    )
    parser.add_argument(
        "--soft-sun-tests",
        action="store_true",
        help="Check soft sun on brush and terrain shadows at every quality",
    )
    parser.add_argument(
        "--local-ambient-tests",
        action="store_true",
        help="Check local model-ambient quality on a point-light gradient without changing world lightmaps",
    )
    parser.add_argument("--refine-ambient-control", type=Path, metavar="VMF", help="Run only four No physics MCP compiles (off/on LDR+HDR Balanced, foreground/background) using a frozen contrast VMF that changes each channel's final probe slot. Requires --game and fresh --output under target/. Copies private inputs, checks exact non-ambient lumps/earlier anchors/counts and source/draft/output custody; does not rerun held-out quality proof or claim native pixels.")
    parser.add_argument("--game", type=Path, help="Private contrast fixture gameinfo.txt for --refine-ambient-control; its local materials tree is copied read-only into the output game.")
    args = parser.parse_args()
    if args.refine_ambient_control:
        if args.game is None or args.reference or args.quality_tests or args.soft_sun_tests or args.local_ambient_tests:
            parser.error("--refine-ambient-control requires --game and excludes the ordinary sky-test options")
        refinement_control(args)
        print(json.dumps({"output": str(args.output), "compile_calls": 4, "status": "passed"}))
        return
    if args.game:
        parser.error("--game is used only with --refine-ambient-control")
    args.output.mkdir(parents=True, exist_ok=True)
    client = Client(None, args.binary, args.output)
    try:
        validate(client, args.output, args.reference)
        if args.quality_tests:
            quality_checks(client, args.output)
        if args.soft_sun_tests:
            soft_sun_checks(client, args.output)
        if args.local_ambient_tests:
            local_ambient_checks(client, args.output)
    finally:
        client.close()
    print(json.dumps({"output": str(args.output), "cases": 13}))


if __name__ == "__main__":
    main()
