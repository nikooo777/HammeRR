import argparse
import base64
import hashlib
import json
import re
import struct
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client
from validate_static_props import static_records
from validate_terrain_lod import build
from validate_visibility import ROOM


def image(client, output, path, skin, body=0):
    result = client.rpc(
        "tools/call",
        {
            "name": "render_model_preview",
            "arguments": {"path": path, "skin": skin, "body": body},
        },
    )
    assert not result.get("isError"), result
    metadata = json.loads(
        next(c["text"] for c in result["content"] if c["type"] == "text")
    )
    pixels = base64.b64decode(
        next(c["data"] for c in result["content"] if c["type"] == "image")
    )
    output.write_bytes(pixels)
    assert metadata["asset"]["skin"] == skin
    assert metadata["asset"]["body"] == body
    assert (metadata["width"], metadata["height"]) == (640, 400)
    return hashlib.sha256(pixels).hexdigest()


def reference_checks(client, output, static_entity):
    paths = [
        "models/player/t_phoenix.mdl",
        "models/player/ct_urban.mdl",
        "models/weapons/w_rif_ak47.mdl",
    ]
    before = client.call("get_map_info")["undo"]
    camera = client.call("get_camera")
    selection = client.call("get_selection")
    assets = [client.call("inspect_model", {"path": path}) for path in paths]
    assert [asset["reference_bones"] for asset in assets] == [50, 50, 4]
    assert all(
        asset["pose"] == "reference" and not asset["static_prop"] for asset in assets
    )
    for index, path in enumerate(paths):
        image(client, output / f"reference-{index}.png", path, 0)
    token = client.call("begin_model_choice", {"id": static_entity})["token"]
    client.call("choose_model", {"token": token, "path": paths[0]}, error=True)
    assert client.call("get_model_choice", {"token": token})["asset"] is None
    client.call("cancel_model_choice", {"token": token})
    assert client.call("get_map_info")["undo"] == before
    assert client.call("get_camera") == camera
    assert client.call("get_selection") == selection
    ids = []
    for path, origin in zip(
        paths, [[-96, 96, 0], [80, 96, 0], [0, 16, 48]], strict=True
    ):
        entity = client.call(
            "create_entity",
            {
                "classname": "prop_dynamic",
                "origin": origin,
                "properties": {
                    "model": path,
                    "angles": "0 30 0",
                    "skin": "0",
                    "solid": "0",
                    "DefaultAnim": "idle",
                },
            },
        )["id"]
        assert client.call("inspect_entity_model", {"id": entity})["displayed"]
        ids.append(entity)
    token = client.call("begin_model_choice", {"id": ids[0]})["token"]
    client.call("choose_model", {"token": token, "path": paths[1]})
    client.call("apply_model_choice", {"token": token})
    assert (
        client.call("inspect_entity_model", {"id": ids[0]})["asset"]["path"] == paths[1]
    )
    client.call("undo")
    assert (
        client.call("inspect_entity_model", {"id": ids[0]})["asset"]["path"] == paths[0]
    )
    client.call("select", {"ids": []})
    client.render(
        output / "reference-models.png",
        camera={"position": [-220, -210, 148], "look_at": [0, 64, 44]},
    )
    return ids, assets


def bodygroup_checks(client, output):
    path = "models/kleiner.mdl"
    plain = client.call("inspect_model", {"path": path, "body": 0})
    clipboard = next(
        group for group in plain["bodygroups"] if group["name"] == "clipboard"
    )
    assert [variant["label"] for variant in clipboard["variants"]] == [
        "None",
        "Clipboard",
    ]
    first = image(client, output / "clipboard-off.png", path, 0)
    second = image(client, output / "clipboard-on.png", path, 0, 1)
    assert first != second
    entity = client.call(
        "create_entity",
        {
            "classname": "prop_dynamic",
            "origin": [0, 128, 0],
            "properties": {"model": "", "targetname": "bodygroup_demo", "solid": "0"},
        },
    )["id"]
    before = client.call("get_objects", {"ids": [entity]})
    history = client.call("get_map_info")["undo"]
    token = client.call("begin_model_choice", {"id": entity})["token"]
    client.call("choose_model", {"token": token, "path": path})
    assert client.call("get_model_choice")["token"] == token
    assert (
        client.call("begin_model_choice", {"id": entity, "resume": True})["token"]
        == token
    )
    chosen = client.call(
        "choose_model_bodygroup",
        {"token": token, "group": clipboard["index"], "variant": 1},
    )
    assert (
        chosen["asset"]["body"] == 1
        and chosen["asset"]["triangles"] > plain["triangles"]
    )
    client.call(
        "choose_model_bodygroup",
        {"token": token, "group": clipboard["index"], "variant": 99},
        error=True,
    )
    assert client.call("get_model_choice", {"token": token})["asset"]["body"] == 1
    assert client.call("get_objects", {"ids": [entity]}) == before
    assert client.call("get_map_info")["undo"] == history
    client.call("apply_model_choice", {"token": token})
    assert client.call("get_map_info")["undo"][1:] == history[:9]
    assert client.call("inspect_entity_model", {"id": entity})["asset"]["body"] == 1
    assert client.call("get_model_choice")["token"] is None
    client.call("undo")
    assert client.call("get_objects", {"ids": [entity]}) == before
    client.call("redo")
    token = client.call("begin_model_choice", {"id": entity})["token"]
    client.call("choose_model", {"token": token, "path": path, "body": 1})
    client.call(
        "set_entity_properties", {"id": entity, "set": {"targetname": "stale_body"}}
    )
    stale = client.call("get_model_choice")
    assert stale["token"] == token and not stale["valid"]
    client.call("cancel_model_choice", {"token": stale["token"]})
    client.call("undo")
    return entity


def validate(args):
    client = Client(None, args.binary, args.output)
    try:
        for low, high in ROOM:
            client.call("create_block", {"mins": low, "maxs": high})
        client.call(
            "create_entity",
            {"classname": "info_player_terrorist", "origin": [64, 64, 8]},
        )
        entity = client.call(
            "create_entity",
            {
                "classname": "prop_static",
                "origin": [0, 0, 0],
                "properties": {"model": "", "solid": "0", "angles": "0 30 0"},
            },
        )["id"]
        catalog = client.call("browse_models", {"query": "oil drum", "limit": 1})
        assert catalog["total"] > 0 and len(catalog["entries"]) == 1
        assert any(row["id"] == "folder:models/props_c17" for row in catalog["groups"])
        for invalid in [{"limit": 101}, {"group": "no"}, {"typo": True}]:
            client.call("browse_models", invalid, error=True)
        client.call(
            "set_entity_properties",
            {"id": entity, "properties": {"skin": "5"}},
            error=True,
        )
        source = client.call("get_map_info")
        camera = client.call("get_camera")
        selection = client.call("get_selection")
        baseline = client.render(args.output / "map-before.png")
        token = client.call("begin_model_choice", {"id": entity})["token"]
        path = "models/props_c17/oildrum001.mdl"
        client.call("choose_model", {"token": token, "path": path, "skin": 0})
        first = image(client, args.output / "model-skin-0.png", path, 0)
        client.call("choose_model", {"token": token, "path": path, "skin": 5})
        second = image(client, args.output / "model-skin-5.png", path, 5)
        assert first != second
        assert client.call("get_model_choice", {"token": token})["asset"]["skin"] == 5
        assert client.call("get_camera") == camera
        assert client.call("get_selection") == selection
        assert client.call("get_map_info")["undo"] == source["undo"]
        assert client.render(args.output / "map-after-preview.png") == baseline
        client.call("render_model_preview", {"path": path, "width": 4096}, error=True)
        client.call(
            "choose_model", {"token": token, "path": path, "skin": 99}, error=True
        )
        client.call("apply_model_choice", {"token": token})
        assert len(client.call("get_map_info")["undo"]) == len(source["undo"]) + 1
        assert client.call("inspect_entity_model", {"id": entity})["asset"]["skin"] == 5
        assert client.call("browse_models", {"group": "used"})["total"] == 1
        client.call("undo")
        assert client.render(args.output / "map-undone.png") == baseline
        client.call("redo")
        old = client.call("begin_model_choice", {"id": entity})["token"]
        replacement = client.call("begin_model_choice", {"id": entity})["token"]
        client.call("cancel_model_choice", {"token": old}, error=True)
        client.call("choose_model", {"token": old, "path": path}, error=True)
        client.call("choose_model", {"token": replacement, "path": path, "skin": 1})
        client.call(
            "set_entity_properties", {"id": entity, "set": {"targetname": "kept"}}
        )
        client.call("apply_model_choice", {"token": replacement}, error=True)
        client.call("cancel_model_choice", {"token": replacement})
        assert client.call("inspect_entity_model", {"id": entity})["asset"]["skin"] == 5
        token = client.call("begin_model_choice", {"id": entity})["token"]
        client.call("choose_model", {"token": token, "path": path})
        client.call(
            "transform_objects",
            {"ids": [entity], "translation": [16, 0, 0], "preview": True},
        )
        client.call("apply_model_choice", {"token": token}, error=True)
        client.call("cancel_model_choice", {"token": token})
        client.call("begin_model_choice", {"id": entity}, error=True)
        client.call("cancel_transform_preview")
        reference_ids, references = reference_checks(client, args.output, entity)
        body_entity = bodygroup_checks(client, args.output)
        client.call("set_compile_settings", {"lighting": "direct_ldr"})
        bsp, stats = build(client, args.output, "model-browser")
        props = static_records(bsp.read_bytes())
        assert stats["static_props"] == len(props["props"]) == 1
        assert props["models"] == [path]
        assert props["props"][0][11] == 5
        data = bsp.read_bytes()
        assert all(asset["path"].encode() in data for asset in references)
        offset, length = struct.unpack_from("<ii", data, 8)
        entities = data[offset : offset + length].decode("utf-8").rstrip("\0")
        body_entity_text = next(
            block
            for block in re.findall(r"\{[^{}]*\}", entities)
            if '"bodygroup_demo"' in block
        )
        assert '"models/kleiner.mdl"' in body_entity_text
        assert re.search(r'"body"\s+"1"', body_entity_text)
        client.call("set_entity_properties", {"id": entity, "set": {"body": "1"}})
        failed = client.call("compile_map", {"path": str(bsp)}, error=True)
        failure = json.loads(failed["content"][0]["text"])
        assert any(
            row["object"] == entity and "body 0" in row["message"]
            for row in failure["diagnostics"]
        )
        assert bsp.read_bytes() == data
        client.call("undo")
        client.call(
            "set_entity_properties",
            {"id": entity, "set": {"model": references[0]["path"], "skin": "0"}},
        )
        failed = client.call("compile_map", {"path": str(bsp)}, error=True)
        failure = json.loads(failed["content"][0]["text"])
        assert any(
            row["object"] == entity and "staticprop" in row["message"]
            for row in failure["diagnostics"]
        )
        assert bsp.read_bytes() == data
        client.call("undo")
        vmf = args.output / "model-browser.vmf"
        client.call("save_map", {"path": str(vmf)})
        client.call("open_map", {"path": str(vmf), "discard_changes": True})
        assert client.call("inspect_entity_model", {"id": entity})["asset"]["skin"] == 5
        for object_id in reference_ids:
            report = client.call("inspect_entity_model", {"id": object_id})
            assert report["displayed"] and report["asset"]["pose"] == "reference"
        body = client.call("inspect_entity_model", {"id": body_entity})
        assert body["displayed"] and body["asset"]["body"] == 1
        inspector = client.call("get_entity_inspector", {"id": entity})
        model = next(
            field for field in inspector["properties"] if field["key"] == "model"
        )
        assert model["control"] == "model" and not model["advanced"]
        (args.output / "validation.json").write_text(
            json.dumps(
                {
                    "model_browser": True,
                    "catalog": catalog,
                    "skins": [first, second],
                    "references": references,
                    "stats": stats,
                },
                indent=2,
            )
            + "\n"
        )
    finally:
        client.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-model-browser")
    )
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    validate(args)
    print(json.dumps({"model_browser": True, "output": str(args.output)}))


if __name__ == "__main__":
    main()
