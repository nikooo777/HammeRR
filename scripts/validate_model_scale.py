import argparse
import hashlib
import json
import math
import struct
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client
from validate_visibility import ROOM

CRATE = "models/props_junk/wood_crate001a.mdl"
REFERENCE = "models/kleiner.mdl"
ORIGIN = [100, 120, 20]


def close(actual, expected):
    assert all(
        math.isclose(a, b, rel_tol=1e-6, abs_tol=1e-5)
        for a, b in zip(actual, expected, strict=True)
    ), (actual, expected)


def properties(client, entity):
    return dict(client.call("get_objects", {"ids": [entity]})["objects"][0]["properties"])


def set_properties(client, entity, **values):
    client.call("set_entity_properties", {"id": entity, "set": values})


def state(client, ids):
    return {
        "properties": {entity: properties(client, entity) for entity in ids},
        "history": client.call("get_history"),
        "selection": client.call("get_selection"),
        "camera": client.call("get_camera"),
    }


def report(client, entity):
    return client.call("inspect_entity_model", {"id": entity})


def check_bounds(client, entity, asset, scale):
    low, high = asset["local_bounds"]
    expected = [
        [ORIGIN[0] - scale * high[1], ORIGIN[1] + scale * low[0], ORIGIN[2] + scale * low[2]],
        [ORIGIN[0] - scale * low[1], ORIGIN[1] + scale * high[0], ORIGIN[2] + scale * high[2]],
    ]
    inspection = report(client, entity)
    assert inspection["displayed"] and inspection["model_scale"]["applied"]
    for actual, wanted in zip(inspection["world_bounds"], expected, strict=True):
        close(actual, wanted)
    bounds = client.call("get_objects", {"ids": [entity]})["objects"][0]["bounds"]
    close(bounds["mins"], expected[0])
    close(bounds["maxs"], expected[1])
    return inspection


def captures(client, output, label, camera):
    return {
        surface: client.render(
            output / f"{label}-{surface}.png", width=512, height=512,
            camera=camera, surface=surface,
        )
        for surface in ["textured", "flat", "wireframe"]
    }


def first_hit(client, entity, bounds):
    low, high = bounds
    for x, y in [(0.5, 0.5), (0.25, 0.25), (0.75, 0.25), (0.25, 0.75)]:
        origin = [low[0] + x * (high[0] - low[0]), low[1] + y * (high[1] - low[1]), high[2] + 256]
        hit = client.call("raycast", {"origin": origin, "direction": [0, 0, -7]})
        if hit.get("object") == entity:
            return origin, hit
    raise AssertionError("No actual crate triangle under the independent top-down probes")


def validate(client, output, result):
    entity = client.call("create_entity", {
        "classname": "prop_dynamic", "origin": ORIGIN,
        "properties": {"model": CRATE, "angles": "0 90 0", "solid": "0"},
    })["id"]
    client.call("select", {"ids": []})
    asset = client.call("inspect_model", {"path": CRATE})
    baseline = check_bounds(client, entity, asset, 1)
    assert baseline["model_scale"]["authored"] is None
    fields = {row["key"]: row for row in client.call("get_entity_inspector", {"id": entity})["properties"]}
    assert fields["modelscale"]["label"] == "Model scale"
    assert fields["modelscale"]["control"] == "number"
    assert fields["modelscale"]["value"] == "1"
    assert fields["modelscale"]["range"] == [0.015625, 64]
    assert "modelscale" not in properties(client, entity)
    before = state(client, [entity])
    assert not client.call("get_map_info")["rendering"]["initialized"]
    camera = {"projection": "top", "center": ORIGIN, "ortho_height": 256}
    images = {"default": captures(client, output, "default", camera)}
    ray_origin, unit_hit = first_hit(client, entity, baseline["world_bounds"])
    assert unit_hit["solid"] is None and unit_hit["side"] is None
    for raw in ["1", "1.00000001"]:
        set_properties(client, entity, modelscale=raw)
        assert report(client, entity)["model_scale"]["effective"] == 1
        images[raw] = captures(client, output, f"unit-{raw}", camera)
        assert images[raw] == images["default"]
        client.call("undo")
        assert properties(client, entity) == before["properties"][entity]

    reports, hits = {}, {}
    for raw, scale in [("0.5000000001", 0.5), ("2", 2)]:
        history = client.call("get_map_info")["undo"]
        set_properties(client, entity, modelscale=raw)
        assert client.call("get_map_info")["undo"][1:] == history[:9]
        reports[raw] = check_bounds(client, entity, asset, scale)
        assert reports[raw]["model_scale"]["authored"] == raw
        assert reports[raw]["model_scale"]["effective"] == scale
        transformed_origin = [a + scale * (b - a) for a, b in zip(ORIGIN, ray_origin, strict=True)]
        hit = client.call("raycast", {"origin": transformed_origin, "direction": [0, 0, -7]})
        assert hit["object"] == entity and hit["solid"] is None and hit["side"] is None
        close(hit["point"], [a + scale * (b - a) for a, b in zip(ORIGIN, unit_hit["point"], strict=True)])
        assert math.isclose(hit["distance"], scale * unit_hit["distance"], rel_tol=1e-6, abs_tol=1e-5)
        hits[raw] = hit
        images[raw] = captures(client, output, f"scale-{scale}", camera)
        assert all(images[raw][surface] != images["default"][surface] for surface in images[raw])
        scaled = state(client, [entity])
        client.call("reload_model_assets")
        assert report(client, entity) == reports[raw]
        assert state(client, [entity]) == scaled
        assert captures(client, output, f"reloaded-{scale}", camera) == images[raw]
        client.call("undo")
        assert properties(client, entity) == before["properties"][entity]
        client.call("redo")
        assert properties(client, entity)["modelscale"] == raw
        client.call("undo")
    assert client.call("get_camera") == before["camera"]
    assert client.call("get_selection") == before["selection"]
    assert captures(client, output, "restored", camera) == images["default"]

    for raw in ["0", "NaN", "65", "1" * 65]:
        set_properties(client, entity, modelscale=raw)
        unsupported = report(client, entity)
        assert not unsupported["displayed"] and unsupported["helper"]
        assert not unsupported["model_scale"]["supported"]
        assert not unsupported["model_scale"]["applied"]
        assert unsupported["model_scale"]["authored"] == raw
        assert "Model scale" in unsupported["error"]
        fields = {row["key"]: row for row in client.call("get_entity_inspector", {"id": entity})["properties"]}
        assert fields["modelscale"]["control"] == "text" and fields["modelscale"]["value"] == raw
        client.call("undo")

    set_properties(client, entity, modelscale="0.5000000001")
    original = properties(client, entity)
    token = client.call("begin_model_choice", {"id": entity})["token"]
    client.call("choose_model", {"token": token, "path": CRATE, "skin": 1})
    assert properties(client, entity) == original
    client.call("apply_model_choice", {"token": token})
    assert properties(client, entity)["modelscale"] == "0.5000000001"
    client.call("undo")
    assert properties(client, entity) == original
    token = client.call("begin_model_choice", {"id": entity})["token"]
    client.call("choose_model", {"token": token, "path": CRATE, "skin": 1})
    set_properties(client, entity, modelscale="2")
    assert not client.call("get_model_choice")["valid"]
    client.call("apply_model_choice", {"token": token}, error=True)
    client.call("cancel_model_choice", {"token": token})
    client.call("undo")
    assert properties(client, entity) == original

    reference = client.call("create_entity", {
        "classname": "prop_dynamic", "origin": [-100, 0, 8],
        "properties": {"model": REFERENCE, "angles": "0 90 0", "modelscale": "0.5", "solid": "0"},
    })["id"]
    reference_asset = client.call("inspect_model", {"path": REFERENCE})
    reference_report = report(client, reference)
    assert reference_report["displayed"] and reference_report["asset"]["pose"] == "reference"
    assert reference_report["asset"]["reference_bones"] > 0
    mouth = next(row for row in reference_asset["attachments"] if row["name"] == "mouth")
    attachment = next(row for row in reference_report["attachments"] if row["index"] == mouth["index"])
    local = mouth["position"]
    close(attachment["position"], [-100 - 0.5 * local[1], 0.5 * local[0], 8 + 0.5 * local[2]])
    for index, (world, axis) in enumerate(zip(attachment["axes"], mouth["axes"], strict=True)):
        expected = [int(index == component) for component in range(3)] if mouth["world_aligned"] else [-0.5 * axis[1], 0.5 * axis[0], 0.5 * axis[2]]
        close(world, expected)
    source = state(client, [entity, reference])
    guide = client.call("show_model_attachment", {"id": reference, "attachment": mouth["index"]})
    close(guide["attachment"]["position"], attachment["position"])
    client.render(output / "model-scale.png", camera={"position": [-220, -240, 180], "look_at": [0, 40, 44]})
    client.call("clear_model_attachment", {"token": guide["token"]})
    assert state(client, [entity, reference]) == source

    for low, high in ROOM:
        client.call("create_block", {"mins": low, "maxs": high, "material": "dev/dev_measuregeneric01"})
    client.call("create_entity", {"classname": "info_player_terrorist", "origin": [-192, -192, 8]})
    static = client.call("create_entity", {
        "classname": "prop_static", "origin": [-96, 96, 8],
        "properties": {"model": CRATE, "solid": "0"},
    })["id"]
    client.call("set_compile_settings", {"lighting": "fullbright", "physics": "none", "bounces": 0, "pack_sky_assets": False, "generate_reflections": False})
    vmf, bsp = output / "model-scale.vmf", output / "model-scale.bsp"
    client.call("save_map", {"path": str(vmf)})
    source = state(client, [entity, reference, static])
    source_bytes = vmf.read_bytes()
    built = client.call("compile_map", {"path": str(bsp)})
    assert built["success"] and built["stats"]["static_props"] == 1 and built["game"] is None
    data = bsp.read_bytes()
    offset, size, version, compression = struct.unpack_from("<4i", data, 8)
    assert version == compression == 0 and offset >= 1036 and offset + size <= len(data)
    entities = data[offset:offset + size].decode().rstrip("\0")
    assert '"modelscale" "0.5000000001"' in entities
    assert '"modelscale" "0.5"' in entities
    assert state(client, [entity, reference, static]) == source and vmf.read_bytes() == source_bytes
    previous_lin = b"1 2 3\n4 5 6\n"
    bsp.with_suffix(".lin").write_bytes(previous_lin)
    set_properties(client, static, modelscale="2")
    static_report = report(client, static)
    assert not static_report["displayed"] and "Static-prop output" in static_report["error"]
    rejected = client.call("compile_map", {"path": str(bsp)}, error=True)
    rejection = json.loads(next(row["text"] for row in rejected["content"] if row["type"] == "text"))
    assert not rejection["success"] and any(
        row["level"] == "error" and "Static-prop output does not support modelscale" in row["message"]
        for row in rejection["diagnostics"]
    )
    assert bsp.read_bytes() == data and bsp.with_suffix(".lin").read_bytes() == previous_lin
    assert vmf.read_bytes() == source_bytes
    client.call("undo")
    assert properties(client, static) == source["properties"][static]
    client.call("open_map", {"path": str(vmf), "discard_changes": True})
    assert properties(client, entity)["modelscale"] == "0.5000000001"
    assert report(client, reference)["model_scale"]["effective"] == 0.5
    result.update({
        "model_scale": True, "images": images, "baseline": baseline,
        "scaled": reports, "unit_hit": unit_hit, "scaled_hits": hits,
        "reference": reference_report, "attachment": attachment,
        "compile": built, "static_rejection": rejection,
        "source_sha256": hashlib.sha256(source_bytes).hexdigest(),
        "bsp_sha256": hashlib.sha256(data).hexdigest(),
    })


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, default=Path("/tmp/hammerr-model-scale"))
    args = parser.parse_args()
    args.binary = args.binary.resolve()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    result = {"binary": str(args.binary), "binary_sha256": hashlib.sha256(args.binary.read_bytes()).hexdigest()}
    client = Client(None, args.binary, args.output)
    try:
        validate(client, args.output, result)
    finally:
        client.close()
        (args.output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"model_scale": True, "output": str(args.output)}))


if __name__ == "__main__":
    main()
