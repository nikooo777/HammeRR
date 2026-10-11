import argparse
import json
from pathlib import Path

from validate_face_lighting import ROOT, Client, detail, face_ref
from validate_material_replacement import preserved_map


def validate(client, output):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    with preserved_map(client):
        tools = {tool["name"]: tool for tool in client.rpc("tools/list", {})["tools"]}
        check("get_map_statistics" in tools, "Statistics query discoverable")
        empty = client.call("get_map_statistics")
        check(
            not any(empty["totals"].values()) and not empty["rows"],
            "Empty map has zero totals",
        )
        ids = []
        for index, material in enumerate(
            ["brick/brickwall001a", "dev/dev_measuregeneric01", "BRICK/BRICKWALL001A"]
        ):
            ids.append(
                client.call(
                    "create_block",
                    {
                        "mins": [index * 128, 0, 0],
                        "maxs": [index * 128 + 64, 64, 64],
                        "material": material,
                    },
                )["id"]
            )
        entity = client.call(
            "tie_to_entity", {"ids": [ids[1], ids[2]], "classname": "func_detail"}
        )["id"]
        client.call("set_object_visibility", {"action": "hide", "ids": [entity]})
        prop = client.call(
            "create_entity",
            {
                "classname": "prop_static",
                "origin": [0, 0, 128],
                "properties": {"model": "Models/Props/test.mdl"},
            },
        )["id"]
        client.call("create_entity", {"classname": "light", "origin": [64, 0, 128]})
        report = client.call("get_map_statistics")
        check(
            report["totals"]
            == {
                "objects": 4,
                "world_brushes": 1,
                "entity_brushes": 2,
                "brush_sides": 18,
                "point_entities": 2,
                "brush_entities": 1,
                "connections": 0,
                "hidden_brushes": 2,
                "hidden_entities": 1,
                "materials": 2,
                "entity_classes": 3,
                "model_paths": 1,
                "displacements": 0,
            },
            "Authored totals include nested brushes and inherited visibility",
        )
        check(
            report["rows"]
            == [
                {"name": "brick/brickwall001a", "count": 12},
                {"name": "dev/dev_measuregeneric01", "count": 6},
            ],
            "Material usage normalizes case and sorts by frequency",
        )
        first = client.call("get_map_statistics", {"limit": 1})
        last = client.call(
            "get_map_statistics", {"offset": first["next_offset"], "limit": 1}
        )
        check(
            first["rows"] + last["rows"] == report["rows"]
            and last["next_offset"] is None,
            "Pagination reconstructs complete usage",
        )
        check(
            client.call("get_map_statistics", {"offset": 999})["rows"] == [],
            "Offset past end gives an empty page",
        )
        for limit in [0, 201]:
            client.call("get_map_statistics", {"limit": limit}, error=True)
            checks.append(f"Reject page size {limit}")
        client.call("get_map_statistics", {"section": "unknown"}, error=True)
        checks.append("Reject unknown section")
        classes = client.call("get_map_statistics", {"section": "entity_classes"})
        check(
            [row["name"] for row in classes["rows"]]
            == ["func_detail", "light", "prop_static"],
            "Class ties sort deterministically by name",
        )
        check(
            client.call("get_map_statistics", {"section": "models"})["rows"]
            == [{"name": "models/props/test.mdl", "count": 1}],
            "External model references counted",
        )
        check(
            client.call("get_map_statistics", {"section": "lightmap_scales"})["rows"]
            == [{"name": "16", "count": 18}],
            "Lightmap density distribution counts sides",
        )
        check(
            client.call("get_map_statistics", {"section": "displacement_powers"})[
                "rows"
            ]
            == [],
            "No terrain produces an empty distribution",
        )
        before = client.call("get_map_info")
        brush = detail(client, [ids[0]])[0]
        face = face_ref(brush, brush["faces"][0])
        draft = client.call(
            "edit_faces",
            {"faces": [face], "material": "tools/toolsnodraw", "preview": True},
        )
        check(
            client.call("get_map_statistics") == report,
            "Face drafts do not change statistics or snapshot revision",
        )
        client.call("cancel_face_preview", {"token": draft["state"]["preview_token"]})
        after = client.call("get_map_info")
        check(
            all(
                before[key] == after[key]
                for key in ["selection", "camera", "undo", "redo", "unsaved_changes"]
            ),
            "Inspection leaves map history selection and camera unchanged",
        )
        client.call("edit_faces", {"faces": [face], "lightmap_scale": 8})
        changed = client.call("get_map_statistics", {"section": "lightmap_scales"})
        check(
            changed["rows"] == [{"name": "16", "count": 17}, {"name": "8", "count": 1}]
            and changed["revision"] != report["revision"],
            "Committed edits refresh distributions and snapshot revision",
        )
        client.call("undo")
        check(
            client.call("get_map_statistics")["totals"] == report["totals"],
            "Undo restores authored counts",
        )
        client.call("set_entity_properties", {"id": prop, "set": {"model": "*1"}})
        check(
            client.call("get_map_statistics")["totals"]["model_paths"] == 0,
            "Inline brush model references excluded from external assets",
        )
        client.call("undo")
        saved = output / "statistics.vmf"
        client.call("save_map", {"path": str(saved)})
        client.call("open_map", {"path": str(saved)})
        reopened = client.call("get_map_statistics")
        check(
            reopened["totals"] == report["totals"]
            and reopened["rows"] == report["rows"],
            "Saved map preserves statistics",
        )
        check(
            reopened["generation"] != report["generation"],
            "Map replacement exposes a new generation",
        )
        (output / "report.json").write_text(json.dumps(reopened, indent=2) + "\n")
    checks.append("Original map state restored")
    result = {
        "transport": "http" if client.url else "stdio",
        "tools": len(tools),
        "checks": len(checks),
        "passed": checks,
    }
    (output / "validation.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({key: value for key, value in result.items() if key != "passed"}))


def main():
    parser = argparse.ArgumentParser(
        description="Validate authored map statistics through MCP; HTTP requires a clean idle map with empty history."
    )
    parser.add_argument("--url")
    parser.add_argument("--binary", type=Path, default=ROOT / "target/debug/editor")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    client = Client(args.url, args.binary.resolve(), output)
    try:
        validate(client, output)
    finally:
        client.close()


if __name__ == "__main__":
    main()
