import argparse
import json
import re
import struct
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client
from validate_terrain_lod import build
from validate_visibility import ROOM


def properties(client, entity):
    return dict(
        client.call("get_objects", {"ids": [entity]})["objects"][0]["properties"]
    )


def validate(args):
    client = Client(None, args.binary, args.output)
    try:
        path = "models/props_junk/wood_crate001a.mdl"
        ids = []
        for index, x in enumerate([-96, 0, 96]):
            ids.append(
                client.call(
                    "create_entity",
                    {
                        "classname": "prop_dynamic",
                        "origin": [x, 0, 0],
                        "properties": {
                            "model": path,
                            "rendercolor": "255 255 255 064",
                            "targetname": f"tint_{index}",
                            "solid": "0",
                            "rendermode": "0",
                            "renderamt": "255",
                        },
                    },
                )["id"]
            )
        client.call("select", {"ids": []})
        camera = {"position": [-210, -260, 150], "look_at": [0, 0, 30]}
        original_camera = client.call("get_camera")
        white = client.render(args.output / "white.png", camera=camera)
        colors = [[255, 96, 64], [96, 255, 96], [96, 128, 255]]
        for entity, color in zip(ids, colors, strict=True):
            inspector = client.call("get_entity_inspector", {"id": entity})
            field = next(
                field
                for field in inspector["properties"]
                if field["key"] == "rendercolor"
            )
            assert field["label"] == "Tint" and field["control"] == "color"
            assert not field["advanced"] and field["intensity"] is None
            client.call(
                "set_entity_color", {"id": entity, "key": "rendercolor", "color": color}
            )
            assert (
                properties(client, entity)["rendercolor"]
                == " ".join(map(str, color)) + " 064"
            )
            report = client.call("inspect_entity_model", {"id": entity})
            assert (
                report["displayed"]
                and report["tint"] == color
                and not report["opacity_preview"]
            )
        tinted = client.render(args.output / "model-tints.png", camera=camera)
        assert tinted != white and client.call("get_camera") == original_camera
        before = properties(client, ids[-1])
        history = client.call("get_map_info")["undo"]
        client.call(
            "set_entity_color",
            {"id": ids[-1], "key": "rendercolor", "color": [1, 2, 3], "intensity": 100},
            error=True,
        )
        assert (
            properties(client, ids[-1]) == before
            and client.call("get_map_info")["undo"] == history
        )
        client.call("undo")
        assert properties(client, ids[-1])["rendercolor"] == "255 255 255 064"
        client.call("redo")
        assert client.render(args.output / "redone.png", camera=camera) == tinted
        client.call(
            "set_entity_properties",
            {"id": ids[-1], "set": {"rendercolor": "256 0 0"}},
            error=True,
        )
        for low, high in ROOM:
            client.call("create_block", {"mins": low, "maxs": high})
        client.call(
            "create_entity",
            {"classname": "info_player_terrorist", "origin": [-160, -160, 8]},
        )
        client.call("set_compile_settings", {"lighting": "direct_ldr"})
        bsp, stats = build(client, args.output, "model-tints")
        data = bsp.read_bytes()
        offset, length = struct.unpack_from("<ii", data, 8)
        entities = data[offset : offset + length].decode().rstrip("\0")
        for index, color in enumerate(colors):
            block = next(
                block
                for block in re.findall(r"\{[^{}]*\}", entities)
                if f'"tint_{index}"' in block
            )
            assert re.search(
                r'"rendercolor"\s+"' + " ".join(map(str, color)) + r' 064"', block
            )
        static = client.call(
            "create_entity",
            {
                "classname": "prop_static",
                "origin": [160, 96, 0],
                "properties": {
                    "model": path,
                    "solid": "0",
                    "rendercolor": "255 128 64",
                },
            },
        )["id"]
        report = client.call("inspect_entity_model", {"id": static})
        assert not report["displayed"] and "Tinted static props" in report["error"]
        failed = client.call("compile_map", {"path": str(bsp)}, error=True)
        failure = json.loads(failed["content"][0]["text"])
        assert any(
            row["object"] == static and "Tinted static props" in row["message"]
            for row in failure["diagnostics"]
        )
        assert bsp.read_bytes() == data
        client.call("undo")
        vmf = args.output / "model-tints.vmf"
        client.call("save_map", {"path": str(vmf)})
        client.call("open_map", {"path": str(vmf), "discard_changes": True})
        for entity, color in zip(ids, colors, strict=True):
            assert client.call("inspect_entity_model", {"id": entity})["tint"] == color
            assert properties(client, entity)["rendercolor"].endswith(" 064")
        (args.output / "validation.json").write_text(
            json.dumps(
                {"colors": colors, "stats": stats, "fourth_component": "064"}, indent=2
            )
            + "\n"
        )
    finally:
        client.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, default=Path("/tmp/hammerr-model-tints"))
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    validate(args)
    print(json.dumps({"model_tints": True, "output": str(args.output)}))


if __name__ == "__main__":
    main()
