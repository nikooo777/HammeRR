import argparse
import json
import shlex
from pathlib import Path

from validate_face_lighting import ROOT, Client, detail


def prepare_client(binary, output):
    game = output / "game"
    materials = game / "materials/clearance"
    materials.mkdir(parents=True, exist_ok=True)
    for name, text in {
        "solid": "LightmappedGeneric {}",
        "playerclip": "LightmappedGeneric { %playerclip 1 }",
        "npcclip": "LightmappedGeneric { %compilenpcclip 1 }",
        "nonsolid": "LightmappedGeneric { %compilenonsolid 1 }",
        "water": "Water {}",
        "unsupported": "LightmappedGeneric { %compileunknown 1 }",
    }.items():
        (materials / f"{name}.vmt").write_text(text)
    gameinfo = game / "gameinfo.txt"
    gameinfo.write_text(
        'GameInfo { FileSystem { SearchPaths { Game "|gameinfo_path|." } } }\n'
    )
    launcher = output / "editor-with-content.sh"
    launcher.write_text(
        f"#!/bin/sh\nexec {shlex.quote(str(binary.resolve()))} "
        f'--game {shlex.quote(str(gameinfo))} "$@"\n'
    )
    launcher.chmod(0o700)
    return Client(None, launcher, output)


def spawn(client, classname="info_player_terrorist", origin=None):
    return client.call(
        "create_entity", {"classname": classname, "origin": origin or [0, 0, 0]}
    )["id"]


def block(client, mins, maxs, material="solid"):
    return client.call(
        "create_block",
        {"mins": mins, "maxs": maxs, "material": f"clearance/{material}"},
    )["id"]


def issues(client, code):
    report = client.call("get_map_checks", {"query": code, "limit": 200})
    return [issue for issue in report["issues"] if issue["code"] == code]


def validate(client, output):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    ids = []
    for mins, maxs in [
        ([-272, -272, -16], [272, 272, 0]),
        ([-272, -272, 72], [272, 272, 88]),
        ([-272, -272, 0], [-256, 272, 72]),
        ([256, -272, 0], [272, 272, 72]),
        ([-256, -272, 0], [256, -256, 72]),
        ([-256, 256, 0], [256, 272, 72]),
    ]:
        ids.append(block(client, mins, maxs))
    terrorist = spawn(client)
    ct = spawn(client, "info_player_counterterrorist", [128, 0, 0])
    ids.extend([terrorist, ct])
    before = client.call("get_map_info")
    geometry = detail(client, ids)
    report = client.call("run_map_checks")
    after = client.call("get_map_info")
    check(
        not issues(client, "spawn_clearance")
        and not issues(client, "missing_team_spawn"),
        "Clear room accepts both teams and touching floor/ceiling",
    )
    check(
        geometry == detail(client, ids)
        and all(
            before[key] == after[key]
            for key in ["undo", "redo", "selection", "camera", "unsaved_changes"]
        ),
        "Checking preserves map data, history, selection and camera",
    )
    check(
        any(
            "models" in note and "compiler LOD snapping" in note
            for note in report["notes"]
        )
        and len(issues(client, "spawn_clearance_scope")) == 1,
        "Shared report states unchecked collision and runtime limits",
    )
    block(client, [-1, -1, 35], [1, 1, 37])
    block(client, [100, -32, 71], [160, 32, 80])
    report = client.call("run_map_checks")
    blocked = issues(client, "spawn_clearance")
    check(
        {issue["object"] for issue in blocked} == {terrorist, ct},
        "Interior obstruction and tight ceiling identify the affected teams",
    )
    issue = next(issue for issue in blocked if issue["object"] == terrorist)
    client.call(
        "select_map_issue",
        {"report": report["report"], "issue": issue["id"], "frame": True},
    )
    check(
        client.call("get_map_info")["selection"] == [terrorist],
        "Current clearance issue selects and frames its spawn",
    )
    block(client, [500, 500, 500], [516, 516, 516])
    stale = client.call("get_map_checks", {"query": "spawn_clearance"})
    rejected = client.call(
        "select_map_issue",
        {"report": report["report"], "issue": issue["id"], "frame": True},
        error=True,
    )
    check(
        stale["stale"] and "changed" in json.dumps(rejected),
        "Stale clearance stays readable and rejects navigation",
    )
    client.call("run_map_checks")
    rejected = client.call(
        "select_map_issue",
        {"report": report["report"], "issue": issue["id"]},
        error=True,
    )
    check(
        "replaced" in json.dumps(rejected), "Replaced report token rejects navigation"
    )

    client.call("new_map", {"discard_changes": True})
    spawn(client)
    obstruction = block(client, [-8, -8, 8], [8, 8, 64], "playerclip")
    client.call("run_map_checks")
    check(
        len(issues(client, "spawn_clearance")) == 1, "Player clip blocks standing hull"
    )
    client.call("set_object_visibility", {"action": "hide", "ids": [obstruction]})
    client.call("run_map_checks")
    check(not issues(client, "spawn_clearance"), "Hidden world brush is excluded")
    client.call("set_object_visibility", {"action": "show", "ids": [obstruction]})
    entity = client.call(
        "tie_to_entity", {"ids": [obstruction], "classname": "func_detail"}
    )["id"]
    client.call("run_map_checks")
    check(
        len(issues(client, "spawn_clearance")) == 1, "Visible func_detail blocks hull"
    )
    client.call("set_object_visibility", {"action": "hide", "ids": [entity]})
    client.call("run_map_checks")
    check(
        not issues(client, "spawn_clearance"),
        "Hidden func_detail owner excludes brushes",
    )

    for material in ["water", "npcclip", "nonsolid"]:
        client.call("new_map", {"discard_changes": True})
        spawn(client)
        block(client, [-8, -8, 8], [8, 8, 64], material)
        client.call("run_map_checks")
        check(
            not issues(client, "spawn_clearance"), f"{material} is not a player blocker"
        )
    for classname in ["trigger_multiple", "func_door"]:
        client.call("new_map", {"discard_changes": True})
        spawn(client)
        obstruction = block(client, [-8, -8, 8], [8, 8, 64])
        client.call("tie_to_entity", {"ids": [obstruction], "classname": classname})
        client.call("run_map_checks")
        check(
            not issues(client, "spawn_clearance"),
            f"{classname} is outside static blockers",
        )
    for material in ["missing", "unsupported"]:
        client.call("new_map", {"discard_changes": True})
        spawn(client)
        block(client, [-8, -8, 8], [8, 8, 64], material)
        client.call("run_map_checks")
        check(
            not issues(client, "spawn_clearance")
            and len(issues(client, "spawn_clearance_unchecked")) == 1,
            f"{material} material reports uncertain collision without guessing",
        )

    malformed = output / "invalid-origins.vmf"
    malformed.write_text(
        "world { id 1 classname worldspawn } "
        "entity { id 2 classname info_player_terrorist } "
        'entity { id 3 classname info_player_counterterrorist origin "NaN 0 0" } '
        'entity { id 4 classname info_player_terrorist origin "0 0" } '
        'entity { id 5 classname info_player_terrorist origin "0 0 0" '
        'ORIGIN "64 0 0" }'
    )
    client.call("open_map", {"path": str(malformed), "discard_changes": True})
    client.call("run_map_checks")
    check(
        len(issues(client, "spawn_origin")) == 4
        and not issues(client, "spawn_clearance"),
        "Missing, non-finite, malformed and repeated origins are unchecked",
    )
    duplicate = output / "duplicate.vmf"
    duplicate.write_text(
        "world { id 1 classname worldspawn } "
        'entity { id 2 classname info_player_terrorist origin "0 0 0" } '
        'entity { id 2 classname info_player_counterterrorist origin "0 0 0" }'
    )
    before = client.call("get_map_info")
    rejected = client.call(
        "open_map", {"path": str(duplicate), "discard_changes": True}, error=True
    )
    after = client.call("get_map_info")
    check(
        "duplicate object ID 2" in json.dumps(rejected)
        and all(before[key] == after[key] for key in ["path", "undo", "redo"]),
        "Duplicate spawn import is rejected without replacing the map",
    )
    return checks


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-spawn-clearance-validation")
    )
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    client = prepare_client(args.binary, output)
    try:
        checks = validate(client, output)
        result = {"passed": len(checks), "checks": checks}
        (output / "results.json").write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps(result, indent=2))
    finally:
        client.close()


if __name__ == "__main__":
    main()
