import argparse
import json
from pathlib import Path

from validate_face_lighting import ROOT, detail, face_ref
from validate_spawn_clearance import block, issues, prepare_client, spawn


def commit(client, draft):
    client.call("commit_face_preview", {"token": draft["state"]["preview_token"]})


def validate(args):
    client = prepare_client(args.binary, args.output)
    try:
        terrorist = spawn(client)
        spawn(client, "info_player_counterterrorist", [192, 192, 0])
        parent = block(client, [-64, -64, -64], [64, 64, 80])
        shape = detail(client, [parent])[0]
        face = face_ref(
            shape, next(face for face in shape["faces"] if face["normal"][2] > 0.99)
        )
        client.call("select_faces", {"faces": [face]})
        client.call("create_terrain", {"power": 4})
        before = client.call("get_map_info")
        client.call("run_map_checks")
        assert not issues(client, "spawn_clearance")
        assert not issues(client, "compiler_displacement")
        after = client.call("get_map_info")
        assert all(
            before[key] == after[key]
            for key in ["undo", "redo", "selection", "camera", "unsaved_changes"]
        )
        for _ in range(3):
            draft = client.call(
                "preview_terrain_sculpt",
                {
                    "mode": "flatten",
                    "center": [0, 0, 32],
                    "radius": 8192,
                    "strength": 1,
                },
            )
            client.call("run_map_checks")
            if _ == 0:
                assert not issues(client, "spawn_clearance")
            commit(client, draft)
        client.call("run_map_checks")
        blocked = issues(client, "spawn_clearance")
        assert len(blocked) == 1 and blocked[0]["object"] == terrorist
        assert "terrain face" in blocked[0]["message"]
        commit(
            client,
            client.call("preview_terrain_collision", {"player_collision": False}),
        )
        client.call("run_map_checks")
        assert not issues(client, "spawn_clearance")
        commit(
            client,
            client.call(
                "preview_terrain_collision",
                {
                    "player_collision": True,
                    "physics_collision": False,
                    "ray_collision": False,
                },
            ),
        )
        client.call("run_map_checks")
        assert len(issues(client, "spawn_clearance")) == 1
        client.call("set_object_visibility", {"action": "hide", "ids": [parent]})
        client.call("run_map_checks")
        assert not issues(client, "spawn_clearance")
        client.call("set_object_visibility", {"action": "show", "ids": [parent]})
        client.call("edit_faces", {"material": "clearance/missing"})
        client.call("run_map_checks")
        assert (
            not issues(client, "spawn_clearance")
            and len(issues(client, "spawn_clearance_unchecked")) == 1
        )
        client.call("edit_faces", {"material": "clearance/solid"})
        path = args.output / "terrain.vmf"
        client.call("save_map", {"path": str(path)})
        text = path.read_text()
        assert text.count('"subdiv" "0"') == 1
        path.write_text(
            text.replace('"subdiv" "0"', '"subdiv" "0"\n"smooth" "invalid"')
        )
        client.call("open_map", {"path": str(path)})
        client.call("select_faces", {"faces": [face]})
        client.call("run_map_checks")
        assert len(issues(client, "displacement_smoothing")) == 1
        commit(client, client.call("preview_terrain_smoothing", {}))
        client.call("run_map_checks")
        assert not issues(client, "displacement_smoothing")
        assert len(issues(client, "spawn_clearance")) == 1
        entity = client.call(
            "tie_to_entity", {"ids": [parent], "classname": "func_detail"}
        )["id"]
        client.call("run_map_checks")
        assert (
            not issues(client, "compiler_displacement")
            and len(issues(client, "spawn_clearance")) == 1
        )
        client.call(
            "set_entity_properties",
            {"id": entity, "set": {"classname": "func_brush"}},
        )
        client.call("run_map_checks")
        owner = issues(client, "compiler_displacement")
        assert (
            len(owner) == 1
            and owner[0]["object"] == entity
            and "brush entities" in owner[0]["message"]
        )
        assert not issues(client, "spawn_clearance")
        (args.output / "results.json").write_text(
            json.dumps(
                {
                    "sheet_does_not_fill_parent": True,
                    "source_preview_excluded": True,
                    "standing_hull_intersection": True,
                    "player_flags_and_visibility": True,
                    "uncertain_material": True,
                    "malformed_angle_issue_and_repair": True,
                    "precise_unsupported_owner": True,
                    "checks_preserve_source_state": True,
                },
                indent=2,
            )
            + "\n"
        )
    finally:
        client.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-terrain-clearance")
    )
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    validate(args)
    print(json.dumps({"terrain_clearance": True, "output": str(args.output)}))


if __name__ == "__main__":
    main()
