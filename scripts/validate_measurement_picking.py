import argparse
import json
import math
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client, detail, face_ref


SIZE = 511


def state(client):
    return client.call("get_measurements")


def action(client, name, **arguments):
    return client.call("set_measurement", {"action": name, **arguments})


def preview(client, identifier=None):
    return action(
        client, "preview", id=identifier, name="Surface clearance",
        start=[-16.25, -24.5, 8.125], end=[64.5, 48.25, 32.75],
        snapshot=state(client)["snapshot"],
    )


def arm(client, endpoint, camera=None):
    report = state(client)
    return action(
        client, "begin_pick", token=report["draft"]["token"], endpoint=endpoint,
        width=SIZE, height=SIZE, snapshot=report["pick_snapshot"],
        **({"camera": camera} if camera else {}),
    )


def ray(client, origin, direction):
    return action(
        client, "pick_ray", token=state(client)["draft"]["token"],
        ray_origin=origin, ray_direction=direction,
    )


def cancel(client):
    action(client, "cancel", token=state(client)["draft"]["token"])


def reject(client, name, **arguments):
    before = state(client)
    client.call("set_measurement", {"action": name, **arguments}, error=True)
    assert state(client) == before


def validate(args):
    client = Client(None, args.binary, args.output)
    checks = {}
    try:
        client.call("new_map", {"discard_changes": True})
        floor = client.call("create_block", {
            "mins": [-256, -256, -16], "maxs": [256, 256, 0],
        })["id"]
        wall = client.call("create_block", {
            "mins": [-256, 128, 0], "maxs": [256, 144, 256],
        })["id"]
        source_path = args.output / "source.vmf"
        client.call("save_map", {"path": str(source_path)})
        source_bytes = source_path.read_bytes()
        original = client.call("get_objects", {"ids": [floor, wall]})
        history = client.call("get_map_info")["undo"]
        selection = client.call("get_selection")
        user_camera = client.call("get_map_info")["camera"]
        draft = preview(client)
        report = arm(client, "start", {
            "projection": "top", "center": [3.125, -5.25, 0], "ortho_height": 256,
        })
        token = report["draft"]["token"]
        assert token != draft["draft"]["token"]
        for name, arguments in [
            ("pick_ray", {"ray_origin": [0, 0, 96], "ray_direction": [0, 0, 0]}),
            ("pick_ray", {"ray_origin": [1000, 1000, 1000], "ray_direction": [0, 0, 1]}),
            ("pick_pixel", {"x": 255, "y": 255, "width": SIZE + 1, "height": SIZE}),
            ("pick_pixel", {"x": SIZE, "y": 255, "width": SIZE, "height": SIZE}),
            ("apply", {}),
        ]:
            reject(client, name, token=token, **arguments)
        report = action(client, "pick_pixel", token=token, x=255, y=255, width=SIZE, height=SIZE)
        assert report["draft"]["measurement"]["start"] == [3.125, -5.25, 0]
        assert report["draft"]["measurement"]["end"] == draft["draft"]["measurement"]["end"]
        assert report["draft"]["last_pick"]["target"] == floor
        assert report["draft"]["picking"] is None
        reject(client, "cancel", token=token)
        arm(client, "end")
        report = ray(client, [35.123456789012, 0, 93.987654321012], [0, 7, 0])
        assert report["draft"]["measurement"]["end"] == [35.123456789012, 128, 93.987654321012]
        assert report["draft"]["last_pick"]["target"] == wall
        assert client.call("get_objects", {"ids": [floor, wall]}) == original
        assert client.call("get_map_info")["undo"] == history
        assert client.call("get_selection") == selection
        assert client.call("get_map_info")["camera"] == user_camera
        assert source_path.read_bytes() == source_bytes
        checks["precise_ray_pixel_source_history_camera_selection"] = report
        if args.render:
            client.render(args.output / "measurement-surface-picked.png", camera={
                "position": [-160, -220, 180], "look_at": [12, 64, 40],
            })
        expected = report["draft"]["measurement"]
        action(client, "apply", token=report["draft"]["token"])
        assert state(client)["measurements"] == [expected]
        assert len(client.call("get_map_info")["undo"]) == len(history) + 1
        client.call("undo")
        assert state(client)["measurements"] == []
        client.call("redo")
        assert state(client)["measurements"] == [expected]
        saved_path = args.output / "measurement-picking.vmf"
        client.call("save_map", {"path": str(saved_path)})
        client.call("open_map", {"path": str(saved_path), "discard_changes": True})
        assert state(client)["measurements"] == [expected]
        checks["apply_undo_redo_save_reopen"] = expected

        preview(client, expected["id"])
        report = arm(client, "end")
        before = report["draft"]["measurement"]
        reviewed = action(client, "review", token=report["draft"]["token"])
        reject(client, "cancel_pick", token=report["draft"]["token"])
        cancelled = action(client, "cancel_pick", token=reviewed["draft"]["token"])
        assert cancelled["draft"]["picking"] is None
        assert cancelled["draft"]["measurement"] == before
        checks["review_cancel_pick_retains_endpoints"] = cancelled
        cancel(client)
        for change in ["camera", "view", "map"]:
            preview(client, expected["id"])
            report = arm(client, "end")
            token = report["draft"]["token"]
            if change == "camera":
                client.call("set_camera", {"position": [200, -180, 144], "look_at": [0, 0, 0]})
            elif change == "view":
                client.call("set_view", {"tool_textures": False})
            else:
                client.call("create_block", {"mins": [512, 512, 0], "maxs": [528, 528, 16]})
            assert state(client)["draft"]["picking"]["valid"] is False
            reject(client, "pick_ray", token=token, ray_origin=[0, 0, 96], ray_direction=[0, 0, -1])
            cancelled = action(client, "cancel_pick", token=token)
            assert cancelled["draft"]["measurement"] == report["draft"]["measurement"]
            cancel(client)
            checks[f"stale_{change}_and_owned_cleanup"] = True

        client.call("create_entity", {"classname": "prop_dynamic", "origin": [1000, 0, 32],
            "properties": {"model": "models/missing-measurement-fixture.mdl"}})
        preview(client, expected["id"])
        report = arm(client, "start")
        reject(client, "pick_ray", token=report["draft"]["token"],
               ray_origin=[1000, 0, 96], ray_direction=[0, 0, -1])
        cancel(client)
        checks["helper_only_target_has_no_plane_fallback"] = True

        terrain = client.call("create_block", {"mins": [-64, -64, 16], "maxs": [64, 64, 32]})["id"]
        shape = detail(client, [terrain])[0]
        face = face_ref(shape, next(face for face in shape["faces"] if face["normal"][2] > 0.99))
        client.call("select_faces", {"faces": [face]})
        client.call("create_terrain", {"power": 2})
        sculpt = client.call("preview_terrain_sculpt", {
            "mode": "raise", "center": [0, 0, 32], "radius": 100, "amount": 48,
        })
        client.call("commit_face_preview", {"token": sculpt["state"]["preview_token"]})
        preview(client, expected["id"])
        arm(client, "start")
        report = ray(client, [16, 24, 128], [0, 0, -1])
        assert report["draft"]["last_pick"]["target"] == terrain
        assert report["draft"]["measurement"]["start"][2] > 32
        assert 0 < report["draft"]["last_pick"]["normal"][2] < 0.99
        checks["terrain_triangle"] = report
        cancel(client)
        client.call("set_object_visibility", {"action": "hide", "ids": [terrain]})
        preview(client, expected["id"])
        arm(client, "start")
        report = ray(client, [16, 24, 128], [0, 0, -1])
        assert report["draft"]["last_pick"]["target"] == floor
        assert report["draft"]["measurement"]["start"][2] == 0
        checks["hidden_terrain_skipped"] = report
        cancel(client)

        if not args.skip_models:
            model = client.call("create_entity", {"classname": "prop_dynamic", "origin": [-96, -96, 0],
                "properties": {"model": "models/props_junk/wood_crate001a.mdl", "angles": "0 27 0"}})["id"]
            assert client.call("inspect_entity_model", {"id": model})["displayed"]
            preview(client, expected["id"])
            arm(client, "end")
            report = ray(client, [-96, -96, 128], [0, 0, -1])
            assert report["draft"]["last_pick"]["target"] == model
            assert math.isfinite(report["draft"]["measurement"]["end"][2])
            assert report["draft"]["measurement"]["end"][2] > 8
            checks["actual_model_mesh"] = report
            cancel(client)
            client.call("set_object_visibility", {"action": "hide", "ids": [model]})
            preview(client, expected["id"])
            arm(client, "end")
            report = ray(client, [-96, -96, 128], [0, 0, -1])
            assert report["draft"]["last_pick"]["target"] == floor
            checks["hidden_model_skipped"] = report
            cancel(client)

        preview(client, expected["id"])
        report = arm(client, "end")
        old_token = report["draft"]["token"]
        client.call("new_map", {"discard_changes": True})
        replacement = preview(client)
        reject(client, "cancel_pick", token=old_token)
        assert replacement["draft"]["token"] > old_token
        cancel(client)
        checks["new_map_replaced_tokens"] = True
        (args.output / "validation.json").write_text(json.dumps(checks, indent=2) + "\n")
    finally:
        client.close()
    return len(checks)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, default=Path("/tmp/hammerr-measurement-picking"))
    parser.add_argument("--skip-models", action="store_true")
    parser.add_argument("--render", action="store_true")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    count = validate(args)
    print(json.dumps({"measurement_picking": count, "output": str(args.output)}))


if __name__ == "__main__":
    main()
