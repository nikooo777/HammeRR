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
    assert release_mode in {"kill", "disable", "disable-wake", "disable-refresh"}
    intervention = release_mode == "disable-refresh"
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
        "observation_only": not intervention,
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
        "HAMMERR_CONTACT_INTERVENTION": "single-recheck" if intervention else "none",
    }


def pointer(value):
    assert isinstance(value, str) and re.fullmatch(r"0x[0-9a-f]+", value), value
    assert int(value, 16) > 0, value
    return value


def vector(value):
    assert isinstance(value, list) and len(value) == 3
    assert all(isinstance(item, (int, float)) and math.isfinite(item) for item in value)


def verify(output, release_mode):
    intervention = release_mode == "disable-refresh"
    report = {
        "schema": 1,
        "observation_only": not intervention,
        "success": False,
        "release_mode": release_mode,
    }
    try:
        receipt = json.loads((output / "contact-observer-launch.json").read_text())
        assert receipt["release_mode"] == release_mode
        assert receipt["observation_only"] is (not intervention)
        assert receipt["load_before_private_map"]
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
        assert grouped["start"][0]["observation_only"] is (not intervention)
        if not intervention:
            assert not any(name.startswith("refresh_") for name in grouped)
        assert len(grouped.get("loaded", [])) == 1 and grouped["loaded"][0]["success"]
        assert (
            len(grouped.get("level_init", [])) == len(grouped.get("activate", [])) == 1
        )
        order = [row["event"] for row in records]
        initial_shutdowns = grouped.get("initial_shutdown", [])
        assert len(initial_shutdowns) <= 1
        if initial_shutdowns:
            initial = initial_shutdowns[0]
            assert initial["frame"] == initial["sample"] == 0
            assert initial["before_target_init"] is True
            assert (
                order.index("loaded")
                < order.index("initial_shutdown")
                < order.index("level_init")
            )
        report["initial_shutdowns_before_tracking"] = len(initial_shutdowns)
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
        names = grouped.get("physics_name", [])
        assert len(names) == len(samples)
        assert [row["sample"] for row in names] == list(range(len(samples)))
        expected_name = "models/props_junk/wood_crate001a.mdl"
        for row, sample in zip(names, samples, strict=True):
            assert row["bounded_read_success"] and row["fixture_label_only"]
            assert row["name"] == expected_name
            assert row["bytes_read"] == len(expected_name.encode()) + 1 <= 256
            pointer(row["returned_pointer"])
            assert (row["object"], row["owner"], row["collide"]) == (
                sample["object"],
                sample["owner"],
                sample["collide"],
            )
        for row in samples:
            vector(row["velocity"])
            vector(row["angular_velocity"])
            assert 0 <= row["contacts"] <= 4096
        contacts = grouped.get("contact", [])
        refresh_contacts = grouped.get("refresh_contact", [])
        entities = grouped.get("entity", [])
        assert len(entities) == len(samples) * len(STABLE | BRACES)
        by_sample, contacts_by_sample = {}, {}
        for row in entities:
            rows = by_sample.setdefault(row["sample"], {})
            assert row["name"] not in rows
            rows[row["name"]] = row
        brace_solidity = grouped.get("brace_solid", [])
        expected_brace_rows = [
            row for row in entities if row["name"] in BRACES and row["present"]
        ]
        assert len(brace_solidity) == len(expected_brace_rows)
        solidity_by_sample = {}
        for row in brace_solidity:
            assert row["name"] in BRACES and 0 <= row["sample"] < len(samples)
            rows = solidity_by_sample.setdefault(row["sample"], {})
            assert row["name"] not in rows
            rows[row["name"]] = row
            identity = by_sample[row["sample"]][row["name"]]
            assert identity["present"]
            assert (row["owner"], row["edict"], row["index"]) == (
                identity["entity"],
                identity["edict"],
                identity["index"],
            )
            assert pointer(row["entity_handle"]) == row["owner"]
            pointer(row["collideable"])
            assert 0 <= row["solid_type"] < 7 and 0 <= row["solid_flags"] <= 65535
            assert row["not_solid"] is bool(row["solid_flags"] & 4)
            assert row["is_solid"] is (row["solid_type"] != 0 and not row["not_solid"])
        for sample in samples:
            assert solidity_by_sample.get(sample["sample"], {}).keys() == {
                name for name in BRACES if by_sample[sample["sample"]][name]["present"]
            }
        for name in BRACES:
            assert (
                len(
                    {
                        row["collideable"]
                        for row in brace_solidity
                        if row["name"] == name
                    }
                )
                == 1
            )
        for row in contacts + refresh_contacts:
            assert 0 <= row["sample"] < len(samples)
            if row["event"] == "contact":
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
            assert all(
                row["is_solid"] and not row["not_solid"] for row in brace_solidity
            )
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
            solid_samples = [
                row
                for row in samples
                if all(
                    solidity_by_sample[row["sample"]][name]["is_solid"]
                    for name in BRACES
                )
            ]
            disabled_samples = [
                row
                for row in samples
                if all(
                    solidity_by_sample[row["sample"]][name]["not_solid"]
                    and not solidity_by_sample[row["sample"]][name]["is_solid"]
                    for name in BRACES
                )
            ]
            assert solid_samples and len(disabled_samples) >= 20
            last_solid, first_disabled = solid_samples[-1], disabled_samples[0]
            assert last_solid["sample"] + 1 == first_disabled["sample"]
            assert solid_samples[0]["sample"] == 0
            assert disabled_samples[-1]["sample"] == samples[-1]["sample"]
            assert len(solid_samples) + len(disabled_samples) == len(samples)
            for name in BRACES:
                rows = [row for row in brace_solidity if row["name"] == name]
                assert all(
                    not row["not_solid"] for row in rows[: first_disabled["sample"]]
                )
                assert all(row["not_solid"] for row in rows[first_disabled["sample"] :])
            release = [
                row["time"]
                for row in observations["events"]
                if row["name"] == "release_braces" and row["output"] == "OnTrigger"
            ]
            assert len(release) == 1
            offset = [
                last_solid["simulation_time"] - release[0] - 0.03,
                first_disabled["simulation_time"] - release[0] + 0.03,
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
            )
            retry = next(time for time, phase in observations["phases"] if phase == 3)
            before_retry = [
                row
                for row in disabled_samples
                if release[0] < row["simulation_time"] - offset[1]
                and row["simulation_time"] - offset[0] < retry
            ]
            assert len(before_retry) >= 10
            released_ids = {row["sample"] for row in before_retry}
            retained = [
                row
                for row in contacts
                if row["sample"] in released_ids
                and row["name"]
                in {"blocked_door_side_brace", "blocked_door_back_brace"}
            ]
            report["clock_correlation"] = {
                "anchor": "same authored Disable and all-four native FSOLID_NOT_SOLID transition",
                "native_minus_script_elapsed_interval": offset,
                "uncertainty_includes_two_physics_ticks": True,
            }
            report["post_disable_before_retry"] = {
                "samples": len(before_retry),
                "brace_contacts": len(retained),
                "brace_partners": sorted({row["name"] for row in retained}),
                "asleep_samples": sum(not row["awake"] for row in before_retry),
                "awake_samples": sum(row["awake"] for row in before_retry),
            }
            report["obstruction_brace_contacts"] = len(definite)
            if intervention:
                arms = grouped.get("refresh_arm", [])
                calls = grouped.get("refresh_call", [])
                afters = grouped.get("refresh_after", [])
                assert len(arms) == len(afters) == 1 and len(calls) == 2
                arm, after = arms[0], afters[0]
                enter, returned = calls
                assert enter["stage"] == "enter" and returned["stage"] == "return"
                assert arm["sample"] == first_disabled["sample"]
                assert arm["frame"] == first_disabled["frame"]
                assert arm["last_solid_time"] == last_solid["simulation_time"]
                assert arm["first_disabled_time"] == first_disabled["simulation_time"]
                active_time = arm["last_active_brace_contact_time"]
                assert math.isfinite(active_time)
                assert any(
                    samples[row["sample"]]["simulation_time"] == active_time
                    and samples[row["sample"]]["awake"]
                    for row in definite
                ), "Refresh did not retain awake initial-obstruction contact proof"
                call_id = enter["sample"]
                assert first_disabled["sample"] < call_id < len(samples) - 2
                before, next_frame = samples[call_id : call_id + 2]
                assert enter["frame"] == before["frame"]
                assert enter["simulation_time"] == before["simulation_time"]
                assert 2.0 - 1e-6 <= (
                    enter["simulation_time"] - first_disabled["simulation_time"]
                ) <= 2.2 + 1e-6
                assert release[0] < enter["simulation_time"] - offset[1]
                assert enter["simulation_time"] - offset[0] < retry
                assert enter["slot"] == 28 and enter["target_elf"] == 0x38100
                assert enter["snapshot_destroyed"] and enter["in_simulation"] is False
                assert enter["asleep"] and before["awake"] is False
                settled = [
                    row for row in samples
                    if enter["settled_since"] <= row["simulation_time"] <= enter["simulation_time"]
                ]
                assert len(settled) >= 5
                assert enter["simulation_time"] - enter["settled_since"] >= 0.5
                assert settled[0]["simulation_time"] == enter["settled_since"]
                assert all(
                    row["awake"] is False
                    and all(abs(value) <= 0.05 for value in row["velocity"] + row["angular_velocity"])
                    for row in settled
                )
                before_contacts = contacts_by_sample[call_id]
                brace_names = {"blocked_door_side_brace", "blocked_door_back_brace"}
                before_braces = [row for row in before_contacts if row["name"] in brace_names]
                assert enter["contacts"] == before["contacts"] == len(before_contacts)
                assert enter["brace_contacts"] == len(before_braces) > 0
                assert next_frame["frame"] == before["frame"] + 1
                assert 0 < next_frame["simulation_time"] - before["simulation_time"] <= 0.03
                for row in [enter, returned, after]:
                    assert row["sample"] == call_id and row["frame"] == before["frame"]
                    assert (row["object"], row["owner"], row["collide"]) == (
                        before["object"], before["owner"], before["collide"]
                    )
                    assert math.isfinite(row["simulation_time"])
                    assert row["simulation_time"] == before["simulation_time"]
                assert enter["crate_index"] == before["crate_index"]
                assert after["snapshot_destroyed"] and after["in_simulation"] is False
                assert isinstance(after["awake"], bool)
                vector(after["velocity"])
                vector(after["angular_velocity"])
                assert all(row["sample"] == call_id and row["stage"] == "after" for row in refresh_contacts)
                assert after["contacts"] == len(refresh_contacts) <= 4096
                after_braces = [row for row in refresh_contacts if row["name"] in brace_names]
                assert after["brace_contacts"] == len(after_braces)
                assert records.index(arm) < records.index(before) < records.index(enter)
                assert records.index(enter) < records.index(returned) < records.index(after)
                assert all(records.index(returned) < records.index(row) < records.index(after) for row in refresh_contacts)
                assert records.index(after) < records.index(next_frame)
                report["experimental_contact_refresh"] = {
                    "direct_calls": 1,
                    "method": "IPhysicsObject::RecheckContactPoints",
                    "slot": 28,
                    "target_elf": 0x38100,
                    "simulation_time": enter["simulation_time"],
                    "script_elapsed_interval": [enter["simulation_time"] - bound for bound in reversed(offset)],
                    "seconds_after_first_native_disabled": enter["simulation_time"] - first_disabled["simulation_time"],
                    "same_owner_object_collide": True,
                    "asleep_settled_before_call": True,
                    "before_brace_contacts": len(before_braces),
                    "after_brace_contacts": len(after_braces),
                    "awake_after_call": after["awake"],
                    "next_frame_brace_contacts": sum(row["name"] in brace_names for row in contacts_by_sample.get(next_frame["sample"], [])),
                    "next_frame_awake": next_frame["awake"],
                    "all_snapshots_destroyed": True,
                    "limits": "The method requests waking; this does not isolate waking from contact reconciliation, establish recovery, or prove a compiler defect.",
                }
        report.update(
            {
                "success": True,
                "samples": len(samples),
                "contacts": len(contacts),
                "crate_index": samples[0]["crate_index"],
                "same_entity_object_collide_observed": True,
                "sleeping_and_awake_directly_observed": True,
                "snapshot_lifetime_checked": True,
                "native_brace_solid_type_flags_and_handle_checked": True,
                "fixture_physics_debug_name": expected_name,
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
