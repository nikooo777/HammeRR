import argparse
import json
import math
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client


def close(actual, expected):
    assert all(
        math.isclose(a, b, abs_tol=1e-5) for a, b in zip(actual, expected, strict=True)
    ), (actual, expected)


def validate(args):
    client = Client(None, args.binary, args.output)
    try:
        path = "models/kleiner.mdl"
        asset = client.call("inspect_model", {"path": path})
        attachments = asset["attachments"]
        assert len(attachments) == 15
        mouth = next(row for row in attachments if row["name"] == "mouth")
        index = mouth["index"]
        assert index > 0 and mouth["bone"] >= 0
        entity = client.call(
            "create_entity",
            {
                "classname": "prop_dynamic",
                "origin": [100, 200, 20],
                "properties": {"model": path, "angles": "0 90 0"},
            },
        )["id"]
        report = client.call("inspect_entity_model", {"id": entity})
        point = next(row for row in report["attachments"] if row["index"] == index)
        local = mouth["position"]
        close(point["position"], [100 - local[1], 200 + local[0], 20 + local[2]])
        for world, local_axis in zip(point["axes"], mouth["axes"], strict=True):
            close(world, [-local_axis[1], local_axis[0], local_axis[2]])
        before = client.call("get_objects", {"ids": [entity]})
        history = client.call("get_map_info")["undo"]
        camera = client.call("get_camera")
        view = {"position": [5, 80, 120], "look_at": [100, 200, 50]}
        baseline = client.render(args.output / "before.png", camera=view)
        guide = client.call(
            "show_model_attachment", {"id": entity, "attachment": index}
        )
        token = guide["token"]
        assert guide["attachment"] == point
        assert client.call("get_model_attachment_guide")["valid"]
        assert client.render(args.output / "guide.png", camera=view) != baseline
        assert client.call("get_camera") == camera
        for invalid in [0, 999]:
            client.call(
                "show_model_attachment",
                {"id": entity, "attachment": invalid},
                error=True,
            )
            assert client.call("get_model_attachment_guide")["token"] == token
        replacement = client.call(
            "show_model_attachment", {"id": entity, "attachment": 1}
        )
        client.call("clear_model_attachment", {"token": token}, error=True)
        assert (
            client.call("get_model_attachment_guide")["token"] == replacement["token"]
        )
        client.call("clear_model_attachment", {"token": replacement["token"]})
        assert client.render(args.output / "cleared.png", camera=view) == baseline
        client.call(
            "frame_model_attachment", {"id": entity, "attachment": index, "aspect": 1.5}
        )
        assert client.call("get_camera") != camera
        assert client.call("get_objects", {"ids": [entity]}) == before
        assert client.call("get_map_info")["undo"] == history
        guide = client.call(
            "show_model_attachment", {"id": entity, "attachment": index}
        )
        client.call("set_entity_properties", {"id": entity, "set": {"angles": "0 0 0"}})
        assert not client.call("get_model_attachment_guide")["valid"]
        client.call("clear_model_attachment", {"token": guide["token"]})
        client.call("undo")
        client.call("set_object_visibility", {"ids": [entity], "action": "hide"})
        client.call(
            "show_model_attachment", {"id": entity, "attachment": index}, error=True
        )
        client.call("undo")
        guide = client.call(
            "show_model_attachment", {"id": entity, "attachment": index}
        )
        client.render(args.output / "framed-attachment.png")
        client.call(
            "transform_objects",
            {"ids": [entity], "translation": [8, 0, 0], "preview": True},
        )
        assert not client.call("get_model_attachment_guide")["valid"]
        client.call(
            "show_model_attachment", {"id": entity, "attachment": index}, error=True
        )
        client.call("cancel_transform_preview")
        client.call("clear_model_attachment", {"token": guide["token"]})
        assert client.call("get_objects", {"ids": [entity]}) == before
        vmf = args.output / "attachments.vmf"
        client.call("save_map", {"path": str(vmf)})
        client.call("open_map", {"path": str(vmf), "discard_changes": True})
        reopened = client.call("inspect_entity_model", {"id": entity})
        assert reopened["attachments"] == report["attachments"]
        assert client.call("get_model_attachment_guide")["token"] is None
        (args.output / "validation.json").write_text(
            json.dumps(
                {"attachments": attachments, "world": report["attachments"]}, indent=2
            )
            + "\n"
        )
    finally:
        client.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-model-attachments")
    )
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    validate(args)
    print(json.dumps({"model_attachments": True, "output": str(args.output)}))


if __name__ == "__main__":
    main()
