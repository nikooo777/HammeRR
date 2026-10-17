import argparse
import json
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client


def properties(client, entity):
    return dict(client.call("get_objects", {"ids": [entity]})["objects"][0]["properties"])


def set_properties(client, entity, **values):
    client.call("set_entity_properties", {"id": entity, "set": values})


def validate(args):
    client = Client(None, args.binary, args.output)
    try:
        camera = {"position": [-210, -260, 150], "look_at": [0, 0, 30]}
        original_camera = client.call("get_camera")
        ids = []
        amounts = [64, 128, 192]
        colors = ["255 96 64 032", "96 255 96 032", "96 128 255 032"]
        for x, amount, color in zip([-96, 0, 96], amounts, colors, strict=True):
            entity = client.call(
                "create_entity",
                {
                    "classname": "prop_dynamic",
                    "origin": [x, 0, 0],
                    "properties": {
                        "model": "models/props_junk/wood_crate001a.mdl",
                        "rendercolor": color,
                        "rendermode": "2",
                        "renderamt": str(amount),
                        "renderfx": "0",
                        "solid": "0",
                    },
                },
            )["id"]
            ids.append(entity)
            report = client.call("inspect_entity_model", {"id": entity})
            assert report["displayed"] and report["opacity_preview"]
            assert report["appearance"]["supported"]
            assert report["appearance"]["state"] == "texture"
            assert report["appearance"]["render_amount"] == amount
            assert abs(report["appearance"]["opacity"] - amount / 255) < 1e-6
            inspector = client.call("get_entity_inspector", {"id": entity})
            fields = {field["key"]: field for field in inspector["properties"]}
            assert fields["rendermode"]["control"] == "choices"
            assert [row["value"] for row in fields["rendermode"]["choices"]] == ["0", "2"]
            assert fields["renderamt"]["label"] == "Opacity"
            assert fields["renderamt"]["control"] == "slider"
            assert fields["renderamt"]["range"] == [0, 255]
            assert fields["rendercolor"]["label"] == "Tint"
        client.call("select", {"ids": []})
        image = client.render(args.output / "model-opacity.png", camera=camera)
        assert client.call("get_camera") == original_camera
        entity = ids[-1]
        original = properties(client, entity)
        bounds = client.call("inspect_entity_model", {"id": entity})["world_bounds"]
        set_properties(client, entity, rendermode="0", renderamt="0")
        normal = client.render(args.output / "normal-zero-amount.png", camera=camera)
        set_properties(client, entity, renderamt="255")
        assert client.render(args.output / "normal-full-amount.png", camera=camera) == normal
        client.call("undo")
        client.call("undo")
        assert properties(client, entity) == original
        assert client.render(args.output / "undone.png", camera=camera) == image
        set_properties(client, entity, renderamt="0")
        transparent = client.call("inspect_entity_model", {"id": entity})
        assert transparent["appearance"]["opacity"] == 0
        assert transparent["displayed"] and transparent["world_bounds"] == bounds
        assert client.render(args.output / "zero-opacity.png", camera=camera) != image
        client.call("undo")
        for key, value, error in [("rendermode", "5", "mode 5"), ("renderfx", "1", "FX 1")]:
            set_properties(client, entity, **{key: value})
            report = client.call("inspect_entity_model", {"id": entity})
            assert not report["displayed"] and not report["appearance"]["supported"]
            assert report["appearance"]["state"] == "unsupported"
            assert report["appearance"]["authored"][key] == value
            assert error in report["error"]
            client.call("undo")
        assert properties(client, entity) == original
        assert client.render(args.output / "restored.png", camera=camera) == image
        vmf = args.output / "model-opacity.vmf"
        client.call("save_map", {"path": str(vmf)})
        client.call("open_map", {"path": str(vmf), "discard_changes": True})
        for entity, amount, color in zip(ids, amounts, colors, strict=True):
            report = client.call("inspect_entity_model", {"id": entity})
            assert report["appearance"]["render_amount"] == amount
            assert properties(client, entity)["rendercolor"] == color
        (args.output / "validation.json").write_text(
            json.dumps({"model_opacity": True, "amounts": amounts, "colors": colors}, indent=2) + "\n"
        )
    finally:
        client.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, default=Path("/tmp/hammerr-model-opacity"))
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    validate(args)
    print(json.dumps({"model_opacity": True, "output": str(args.output)}))


if __name__ == "__main__":
    main()
