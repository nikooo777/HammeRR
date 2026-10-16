import hashlib
import itertools
import json
import math
import re
import shutil
from pathlib import Path

from private_runtime_config import reject_configuration_writes

MODULES = {
    "engine.so": (
        "bin/linux64/engine.so",
        "965b90157cbec58187c553ce8bb65907e186c990b08453cb4abd75e909ec794a",
        "a897e8fd69fff915bd84769509436d61cdd528c7",
    ),
    "server.so": (
        "cstrike/bin/linux64/server.so",
        "7887c8cc24a3dec6b8739bfebcad48c00b95423ebcbfc647f49fef1ab904dbe2",
        "87d084fa011e265ede4aa65eb02a04112499aa7c",
    ),
    "vphysics.so": (
        "bin/linux64/vphysics.so",
        "2676cdc02be4b4188f7121c201d5f5a681f9c1c7c924fe7f9159cefd5a718277",
        "ab4c2d0c4c56edf61af5a7501907d1b6aba51324",
    ),
}
INTERFACES = {
    "VSERVERTOOLS003": ("server.so", 0xB5BCC8),
    "ServerGameEnts001": ("server.so", 0xB54EF8),
    "VEngineServer023": ("engine.so", 0x8E1770),
    "VPhysics031": ("vphysics.so", 0x193C98),
}
FACTORIES = {"engine.so": 0x666B80, "server.so": 0x938070, "vphysics.so": 0x58B20}
STABLE = {"blocked_crate", "blocked_door", "clear_door"}
BRACES = {
    f"{door}_{kind}"
    for door in ("blocked_door", "clear_door")
    for kind in ("side_brace", "back_brace")
}
MAX_REPORT_BYTES = 32 * 1024 * 1024


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def prepare(library, release_mode, root, game, output, command, environment):
    assert not environment.get("LD_PRELOAD"), (
        "Contact observation cannot inherit LD_PRELOAD"
    )
    assert game == output / "cstrike" and not game.is_symlink()
    assert not game.is_relative_to(root) and not root.is_relative_to(game)
    assert command[0] == str(root / "cstrike_linux64")
    assert "-default" in command and "-insecure" in command
    assert command[command.index("-game") + 1] == str(game)
    fingerprints = {}
    for name, (relative, expected, _) in MODULES.items():
        path = (root / relative).resolve(strict=True)
        assert path.stat().st_size <= 32 * 1024 * 1024
        assert digest(path) == expected, (
            f"Installed {name} changed; repeat the ABI review"
        )
        fingerprints[name] = {"path": str(path), "sha256": expected}
    assert library.is_file() and 0 < library.stat().st_size <= 4 * 1024 * 1024
    plugin = game / "addons/hammerr_contact_observer.so"
    plugin.parent.mkdir(exist_ok=True)
    with library.open("rb") as source, plugin.open("xb") as destination:
        shutil.copyfileobj(source, destination)
    library_hash = digest(library)
    assert digest(plugin) == library_hash
    profile = game / "cfg/hammerr_private_launch.cfg"
    source = profile.read_text()
    marker = "echo HAMMERR_PRIVATE_PREMAP_READY\nmap hammerr_runtime\n"
    assert source.count(marker) == 1
    assert not re.search(r"[\"\\\r\n;]", str(plugin))
    assert "plugin_load" not in source
    profile.write_text(source.replace(marker, f'plugin_load "{plugin}"\n' + marker))
    reject_configuration_writes(profile)
    receipt = {
        "schema": 1,
        "release_mode": release_mode,
        "observation_only": True,
        "library": str(library),
        "private_plugin": str(plugin),
        "library_sha256": library_hash,
        "module_fingerprints": fingerprints,
        "original_profile_sha256": hashlib.sha256(source.encode()).hexdigest(),
        "instrumented_profile_sha256": digest(profile),
        "load_before_private_map": True,
    }
    (output / "contact-observer-launch.json").write_text(
        json.dumps(receipt, indent=2) + "\n"
    )
    return command, {
        **environment,
        "HAMMERR_CONTACT_EXPECTED_EXE": str(
            (root / "cstrike_linux64").resolve(strict=True)
        ),
        "HAMMERR_CONTACT_GAME_ROOT": str(root),
        "HAMMERR_CONTACT_PRIVATE_GAME": str(game),
        "HAMMERR_CONTACT_REPORT": str(output / "contact-observer.jsonl"),
    }


def pointer(value):
    assert isinstance(value, str) and re.fullmatch(r"0x[0-9a-f]+", value), value
    assert int(value, 16) > 0, value
    return value


def vector(value):
    assert isinstance(value, list) and len(value) == 3
    assert all(isinstance(item, (int, float)) and math.isfinite(item) for item in value)


def verify(output, release_mode):
    report = {
        "schema": 1,
        "observation_only": True,
        "success": False,
        "release_mode": release_mode,
    }
    try:
        receipt = json.loads((output / "contact-observer-launch.json").read_text())
        assert receipt["release_mode"] == release_mode
        assert receipt["observation_only"] and receipt["load_before_private_map"]
        assert digest(Path(receipt["library"])) == receipt["library_sha256"]
        assert digest(Path(receipt["private_plugin"])) == receipt["library_sha256"]
        assert (
            digest(output / "cstrike/cfg/hammerr_private_launch.cfg")
            == receipt["instrumented_profile_sha256"]
        )
        for name, (_, expected, _) in MODULES.items():
            assert receipt["module_fingerprints"][name]["sha256"] == expected
            assert (
                digest(Path(receipt["module_fingerprints"][name]["path"])) == expected
            )
        path = output / "contact-observer.jsonl"
        assert 0 < path.stat().st_size <= MAX_REPORT_BYTES
        raw = path.read_bytes()
        report["raw_sha256"] = hashlib.sha256(raw).hexdigest()
        assert raw.endswith(b"\n")
        lines = raw.splitlines()
        assert all(0 < len(line) < 2048 for line in lines)
        records = [json.loads(line) for line in lines]
        grouped = {}
        for record in records:
            grouped.setdefault(record["event"], []).append(record)
        report["event_counts"] = {name: len(values) for name, values in grouped.items()}
        report["invalidations"] = grouped.get("invalidated", [])
        assert not report["invalidations"], report["invalidations"]
        assert not grouped.get("unload"), "Observer unloaded before capture termination"
        assert len(grouped.get("start", [])) == 1 and grouped["start"][0]["schema"] == 1
        assert grouped["start"][0]["observation_only"]
        assert len(grouped.get("loaded", [])) == 1 and grouped["loaded"][0]["success"]
        assert (
            len(grouped.get("level_init", [])) == len(grouped.get("activate", [])) == 1
        )
        order = [row["event"] for row in records]
        assert (
            order.index("start")
            < order.index("loaded")
            < order.index("level_init")
            < order.index("activate")
            < order.index("sample")
        )
        assert {row["name"]: row["build_id"] for row in grouped["module"]} == {
            name: values[2] for name, values in MODULES.items()
        }
        assert len(grouped["interface"]) == len(INTERFACES)
        assert {
            row["version"]: (row["module"], row["vtable_elf"])
            for row in grouped["interface"]
        } == INTERFACES
        assert all(
            row["factory_elf"] == FACTORIES[row["module"]]
            for row in grouped["interface"]
        )
        samples = grouped.get("sample", [])
        assert len(samples) >= 200, len(samples)
        assert [row["sample"] for row in samples] == list(range(len(samples)))
        assert len({row["thread"] for row in samples}) == 1 and samples[0]["thread"] > 0
        assert all(
            row["snapshot_destroyed"] and row["in_simulation"] is False
            for row in samples
        )
        assert all(
            0 < row["environments"] <= 16 and 0 < row["objects"] <= 65536
            for row in samples
        )
        times = [row["simulation_time"] for row in samples]
        assert all(math.isfinite(value) for value in times)
        assert all(0 < b - a <= 0.5 for a, b in itertools.pairwise(times)), times
        assert times[-1] - times[0] >= 30
        assert {row["awake"] for row in samples} == {False, True}, (
            "No directly observed sleeping/awake transition"
        )
        for field in ["object", "owner", "collide"]:
            assert len({pointer(row[field]) for row in samples}) == 1, field
        for row in samples:
            vector(row["velocity"])
            vector(row["angular_velocity"])
            assert 0 <= row["contacts"] <= 4096
        contacts = grouped.get("contact", [])
        entities = grouped.get("entity", [])
        assert len(entities) == len(samples) * len(STABLE | BRACES)
        by_sample, contacts_by_sample = {}, {}
        for row in entities:
            rows = by_sample.setdefault(row["sample"], {})
            assert row["name"] not in rows
            rows[row["name"]] = row
        for row in contacts:
            assert 0 <= row["sample"] < len(samples)
            contacts_by_sample.setdefault(row["sample"], []).append(row)
            assert pointer(row["source"]) == samples[row["sample"]]["object"]
            for field in ["other", "owner", "edict", "other_collide"]:
                pointer(row[field])
            assert isinstance(row["other_awake"], bool)
            assert row["index"] >= 0
            vector(row["point"])
            vector(row["normal"])
            if row["name"] in STABLE | BRACES:
                identity = by_sample[row["sample"]][row["name"]]
                assert identity["present"]
                assert (
                    row["owner"],
                    row["edict"],
                    row["index"],
                    row["other"],
                    row["other_collide"],
                ) == (
                    identity["entity"],
                    identity["edict"],
                    identity["index"],
                    identity["object"],
                    identity["collide"],
                )
        for sample in samples:
            rows = by_sample[sample["sample"]]
            assert rows.keys() == STABLE | BRACES
            assert (
                len(contacts_by_sample.get(sample["sample"], [])) == sample["contacts"]
            )
            for name in STABLE:
                assert rows[name]["present"] and rows[name]["object_count"] == 1
            crate = rows["blocked_crate"]
            assert crate["index"] == sample["crate_index"] > 0
            assert (crate["entity"], crate["object"], crate["collide"]) == (
                sample["owner"],
                sample["object"],
                sample["collide"],
            )
        for name in STABLE | BRACES:
            present = [
                row for row in entities if row["name"] == name and row["present"]
            ]
            assert present and all(row["object_count"] == 1 for row in present)
            assert (
                len(
                    {
                        tuple(
                            pointer(row[field])
                            for field in ["entity", "edict", "object", "collide"]
                        )
                        + (row["index"],)
                        for row in present
                    }
                )
                == 1
            )
        observations = json.loads((output / "observations.json").read_text())
        crate_bounds = [
            row for row in observations["bounds"] if row["name"] == "blocked_crate"
        ]
        assert {row["entity_index"] for row in crate_bounds} == {
            samples[0]["crate_index"]
        }
        freed = grouped.get("edict_freed", [])
        assert not any(row["name"] in STABLE for row in freed)
        if release_mode == "kill":
            assert len(freed) == 4 and {row["name"] for row in freed} == BRACES
            for row in freed:
                first = by_sample[0][row["name"]]
                assert row["edict"] == first["edict"] and row["index"] == first["index"]
            present_samples = [
                row
                for row in samples
                if all(by_sample[row["sample"]][name]["present"] for name in BRACES)
            ]
            absent_samples = [
                row
                for row in samples
                if all(not by_sample[row["sample"]][name]["present"] for name in BRACES)
            ]
            assert present_samples and len(absent_samples) >= 20
            last_present, first_absent = present_samples[-1], absent_samples[0]
            assert last_present["sample"] + 1 == first_absent["sample"]
            assert all(
                last_present["frame"] <= row["frame"] <= first_absent["frame"]
                for row in freed
            )
            release = [
                row["time"]
                for row in observations["events"]
                if row["name"] == "release_braces" and row["output"] == "OnTrigger"
            ]
            assert len(release) == 1
            offset = [
                last_present["simulation_time"] - release[0] - 0.03,
                first_absent["simulation_time"] - release[0] + 0.03,
            ]
            assert 0 < offset[1] - offset[0] <= 0.3
            phase1 = next(time for time, phase in observations["phases"] if phase == 1)
            definite = [
                row
                for row in contacts
                if row["name"] in {"blocked_door_side_brace", "blocked_door_back_brace"}
                and phase1 < samples[row["sample"]]["simulation_time"] - offset[1]
                and samples[row["sample"]]["simulation_time"] - offset[0] < release[0]
            ]
            assert definite, (
                "No native crate/brace contact resolved during genuine obstruction"
            )
            script_end = max(row["time"] for row in observations["samples"])
            native_end = samples[-1]["simulation_time"]
            assert (
                native_end - offset[1] - 0.5
                <= script_end
                <= native_end - offset[0] + 0.5
            ), "Contact stream ended before the retained script observations"
            report["clock_correlation"] = {
                "anchor": "same authored Kill and all-four native free/absence transition",
                "native_minus_script_elapsed_interval": offset,
                "uncertainty_includes_two_physics_ticks": True,
            }
            report["obstruction_brace_contacts"] = len(definite)
            report["post_removal_samples"] = len(absent_samples)
        else:
            assert not freed and all(row["present"] for row in entities)
            report["clock_correlation"] = None
        report.update(
            {
                "success": True,
                "samples": len(samples),
                "contacts": len(contacts),
                "crate_index": samples[0]["crate_index"],
                "same_entity_object_collide_observed": True,
                "sleeping_and_awake_directly_observed": True,
                "snapshot_lifetime_checked": True,
                "limits": "Sampled pointer continuity does not exclude physics allocator reuse within a step or establish unchanged hull bytes. A valid observer does not establish recovery or a contact-cache cause.",
            }
        )
    except BaseException as error:
        report["failure"] = {"type": type(error).__name__, "message": str(error)}
        raise
    finally:
        (output / "contact-observer-analysis.json").write_text(
            json.dumps(report, indent=2) + "\n"
        )
    return report
