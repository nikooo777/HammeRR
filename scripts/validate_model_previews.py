import argparse
import json
import math
from pathlib import Path

from validate_face_lighting import ROOT, Client
from validate_static_props import static_records
from validate_terrain_lod import build
from validate_visibility import ROOM


def selection_checks(client, ids, origins, reports):
    history = client.call("get_map_info")["undo"]
    for object_id, origin, report in zip(ids, origins, reports, strict=True):
        low, high = report["world_bounds"]
        details = client.call("get_objects", {"ids": [object_id]})["objects"][0]
        assert details["bounds"] == {"mins": low, "maxs": high}
        small = {
            "mins": [value - 9 for value in origin],
            "maxs": [value + 9 for value in origin],
        }
        common = {"selection_mode": "objects", "preview": True}
        contained = client.call(
            "select_volume", {**common, **small, "rule": "contained"}
        )["preview"]
        assert object_id not in contained["objects"]
        client.call("cancel_volume_selection", {"token": contained["token"]})
        center = [(a + b) / 2 for a, b in zip(low, high, strict=True)]
        remote = {
            "mins": [high[0] - 0.5, center[1] - 1, center[2] - 1],
            "maxs": [high[0] + 0.5, center[1] + 1, center[2] + 1],
        }
        crossing = client.call(
            "select_volume", {**common, **remote, "rule": "intersecting"}
        )["preview"]
        assert object_id in crossing["objects"]
        client.call("cancel_volume_selection", {"token": crossing["token"]})
        found = client.call(
            "find_objects",
            {
                "classname": "prop_static",
                "hidden": False,
                "within": [remote["mins"], remote["maxs"]],
            },
        )["objects"]
        matched = next(row for row in found if row["id"] == object_id)
        assert matched["bounds"] == {"mins": low, "maxs": high}
        camera = {"projection": "top", "center": center, "ortho_height": 128}
        common_area = {"width": 512, "height": 512, "camera": camera, "preview": True}
        area = client.call(
            "select_area",
            {
                **common_area,
                "start": [220, 220],
                "end": [292, 292],
                "rule": "contained",
            },
        )
        assert object_id not in area["matches"]["objects"]
        client.call("cancel_area_selection")
        x = 256 + (high[0] - center[0]) * 4
        area = client.call(
            "select_area",
            {
                **common_area,
                "start": [x - 2, 252],
                "end": [x + 2, 260],
                "rule": "intersecting",
            },
        )
        assert object_id in area["matches"]["objects"]
        client.call("cancel_area_selection")
    assert client.call("get_map_info")["undo"] == history


def validate(args):
    client = Client(None, args.binary, args.output)
    try:
        paths = [
            "models/props_junk/wood_crate001a.mdl",
            "models/props/cs_office/file_cabinet1.mdl",
            "models/props_c17/oildrum001.mdl",
        ]
        initial = client.call("get_map_info")
        assets = [client.call("inspect_model", {"path": path}) for path in paths]
        assert all(
            asset["vertices"] > 24 and asset["triangles"] > 12 for asset in assets
        )
        assert [asset["skins"] for asset in assets] == [2, 1, 6]
        for invalid in [
            {"path": "../bad.mdl"},
            {"path": "/models/bad.mdl"},
            {"path": paths[0], "skin": 99},
            {"path": paths[0], "typo": True},
            {"path": "models/missing.mdl"},
        ]:
            client.call("inspect_model", invalid, error=True)
        assert client.call("get_map_info")["undo"] == initial["undo"]
        room = []
        for low, high in ROOM:
            room.append(
                client.call(
                    "create_block",
                    {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"},
                )["id"]
            )
        ids, origins, angles, skins = [], [], [], [0, 0, 5]
        for index, (path, origin, rotation) in enumerate(
            [
                (paths[0], [-96, -32, 0], [0, 30, 0]),
                (paths[1], [64, -24, 0], [0, -25, 0]),
                (paths[2], [0, 96, 0], [0, 10, 0]),
            ]
        ):
            origin = [
                origin[0],
                origin[1],
                -assets[index]["local_bounds"][0][2] + 0.125,
            ]
            ids.append(
                client.call(
                    "create_entity",
                    {
                        "classname": "prop_static",
                        "origin": origin,
                        "properties": {
                            "model": path,
                            "angles": " ".join(map(str, rotation)),
                            "skin": str(skins[index]),
                            "solid": "0",
                        },
                    },
                )["id"]
            )
            origins.append(origin)
            angles.append(rotation)
        client.call(
            "create_entity",
            {"classname": "info_player_terrorist", "origin": [-160, -160, 8]},
        )
        client.call(
            "create_entity",
            {
                "classname": "light",
                "origin": [0, -128, 192],
                "properties": {"_light": "255 245 225 300", "_constant_attn": "1"},
            },
        )
        missing = client.call(
            "create_entity",
            {
                "classname": "prop_static",
                "origin": [160, 96, 0],
                "properties": {"model": "models/missing.mdl"},
            },
        )["id"]
        client.call("inspect_entity_model", {"id": missing}, error=True)
        client.call("set_object_visibility", {"ids": [missing], "action": "hide"})
        client.call("set_object_visibility", {"ids": [room[1]], "action": "hide"})
        client.call("select", {"ids": []})
        history = client.call("get_map_info")["undo"]
        reports = [
            client.call("inspect_entity_model", {"id": object_id}) for object_id in ids
        ]
        assert all(
            report["displayed"] and report["error"] is None for report in reports
        )
        selection_checks(client, ids, origins, reports)
        for object_id, origin, report in zip(ids, origins, reports, strict=True):
            low, high = report["world_bounds"]
            center = [(a + b) / 2 for a, b in zip(low, high, strict=True)]
            span = max(high[0] - low[0], high[1] - low[1]) * 1.6
            camera = {"projection": "top", "center": center, "ortho_height": span}
            hits = []
            observations = []
            for x in [300, 500, 700]:
                for y in [240, 400, 560]:
                    picked = client.call(
                        "pick_at_pixel",
                        {
                            "width": 1000,
                            "height": 800,
                            "x": x,
                            "y": y,
                            "camera": camera,
                        },
                    )
                    observations.append(picked)
                    if picked.get("object") == object_id:
                        hits.append(picked)
            assert hits, (object_id, observations)
            assert any(
                abs(hit["point"][0] - origin[0]) > 9
                or abs(hit["point"][1] - origin[1]) > 9
                for hit in hits
            ), object_id
        scene_camera = {"position": [-210, -220, 160], "look_at": [-8, 24, 36]}
        client.render(args.output / "model-previews.png", camera=scene_camera)
        skin_camera = {
            "projection": "perspective",
            "position": [-180, -150, 96],
            "look_at": [-96, -32, 20],
        }
        first = client.render(args.output / "crate-skin-0.png", camera=skin_camera)
        client.call("set_entity_properties", {"id": ids[0], "set": {"skin": "1"}})
        second = client.render(args.output / "crate-skin-1.png", camera=skin_camera)
        assert first != second
        assert client.call("inspect_entity_model", {"id": ids[0]})["asset"]["skin"] == 1
        client.call("undo")
        assert (
            client.render(args.output / "crate-skin-restored.png", camera=skin_camera)
            == first
        )
        bounds = reports[0]["world_bounds"]
        client.call(
            "transform_objects",
            {"ids": [ids[0]], "translation": [12, 4, 8], "preview": True},
        )
        moved = client.call("inspect_entity_model", {"id": ids[0]})["world_bounds"]
        for a, b in zip(bounds, moved, strict=True):
            assert all(
                math.isclose(value + delta, result, abs_tol=1e-5)
                for value, delta, result in zip(a, [12, 4, 8], b, strict=True)
            )
        client.call("cancel_transform_preview")
        assert (
            client.call("inspect_entity_model", {"id": ids[0]})["world_bounds"]
            == bounds
        )
        client.call("select", {"ids": []})
        client.call("reload_model_assets")
        assert (
            client.render(args.output / "crate-reloaded.png", camera=skin_camera)
            == first
        )
        assert client.call("get_map_info")["undo"] == history
        client.call("set_entity_properties", {"id": ids[0], "set": {"modelscale": "2"}})
        assert not client.call("inspect_entity_model", {"id": ids[0]})["displayed"]
        client.call("undo")
        client.call("set_object_visibility", {"ids": [room[1]], "action": "show"})
        client.call("set_compile_settings", {"lighting": "direct_ldr"})
        bsp, stats = build(client, args.output, "models")
        props = static_records(bsp.read_bytes())
        assert stats["static_props"] == len(props["props"]) == 3
        for row, path, origin, rotation, skin in zip(
            props["props"], paths, origins, angles, skins, strict=True
        ):
            assert props["models"][row[6]] == path
            assert all(
                math.isclose(a, b, abs_tol=1e-5)
                for a, b in zip(row[:3], origin, strict=True)
            )
            assert list(row[3:6]) == rotation
            assert row[11] == skin
        source = args.output / "models.vmf"
        client.call("save_map", {"path": str(source)})
        client.call("open_map", {"path": str(source)})
        assert all(
            client.call("inspect_entity_model", {"id": object_id})["displayed"]
            for object_id in ids
        )
        labels = client.call("get_entity_inspector", {"id": ids[0]})
        (args.output / "validation.json").write_text(
            json.dumps(
                {
                    "model_previews": True,
                    "assets": assets,
                    "reports": reports,
                    "stats": stats,
                    "inspector": labels,
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
        "--output", type=Path, default=Path("/tmp/hammerr-model-previews")
    )
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    validate(args)
    print(json.dumps({"model_previews": True, "output": str(args.output)}))


if __name__ == "__main__":
    main()
