import argparse
import base64
import hashlib
import json
from pathlib import Path

from validate_face_lighting import ROOT, Client
from validate_static_props import static_records
from validate_terrain_lod import build
from validate_visibility import ROOM


def image(client, output, path, skin):
    result = client.rpc(
        "tools/call",
        {
            "name": "render_model_preview",
            "arguments": {"path": path, "skin": skin},
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
    assert (metadata["width"], metadata["height"]) == (640, 400)
    return hashlib.sha256(pixels).hexdigest()


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
        client.call("set_compile_settings", {"lighting": "direct_ldr"})
        bsp, stats = build(client, args.output, "model-browser")
        props = static_records(bsp.read_bytes())
        assert stats["static_props"] == len(props["props"]) == 1
        assert props["models"] == [path]
        assert props["props"][0][11] == 5
        vmf = args.output / "model-browser.vmf"
        client.call("save_map", {"path": str(vmf)})
        client.call("open_map", {"path": str(vmf), "discard_changes": True})
        assert client.call("inspect_entity_model", {"id": entity})["asset"]["skin"] == 5
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
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-model-browser")
    )
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    validate(args)
    print(json.dumps({"model_browser": True, "output": str(args.output)}))


if __name__ == "__main__":
    main()
