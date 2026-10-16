import argparse
import hashlib
import json
import os
from pathlib import Path
from unittest.mock import patch

from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_sky_preview import rgba_png
from validate_working_region import configure

SIZE = 401
SOURCE, START, END, ALIAS = "entity:1001", "entity:101", "entity:202", "entity:303"
SELF_LINE = "line(32 220 80, nodeid, aim)"
PAIR_LINE = "line(240 80 24, nodeid, StartNode, nodeid, EndNode)"
FGD = f'''
@BaseClass {SELF_LINE} {PAIR_LINE} = links [
    aim(node_dest) : "Target node" : 42
    StartNode(node_dest) : "Start node" : 0
    EndNode(node_dest) : "End node" : 42
    count(integer) : "Count" : 0
]
@PointClass base(links) size(8 8 8) color(100 100 100) = link []
@PointClass size(8 8 8) color(100 100 100)
    studio("models/hammerr_missing_node_line_guide.mdl") = node [
    nodeid(integer) readonly : "Node ID" : 42
]
@PointClass size(8 8 8) color(100 100 100) = ordinary []
'''


def entity(identifier, classname, origin, properties=()):
    pairs = [("id", str(identifier)), ("classname", classname), ("origin", origin), *properties]
    return "entity\n{\n" + "".join(f'\t"{key}" "{value}"\n' for key, value in pairs) + "}\n"


def fixture(end_id="42", *, extra_ids=(), alias=None, omit_id=False):
    text = 'world\n{\n"id" "1"\n"classname" "worldspawn"\n}\n'
    text += entity(1001, "link", "0 -80 16")
    text += entity(101, "node", "-120 80 -80", [("nodeid", "0")])
    text += entity(202, "node", "120 80 80", ([] if omit_id else [("nodeid", end_id)]) + list(extra_ids))
    text += entity(42, "ordinary", "320 320 320")
    if alias is not None:
        text += entity(303, "node", "160 112 96", [("nodeid", alias)])
    return text


def validate(client, output):
    checks, images, measurements, cases = [], [], {}, []
    fgd = output / "node-lines.fgd"

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def definitions(text=FGD):
        fgd.write_text(text)
        client.call("load_entity_definitions", {"path": str(fgd)})

    def inspect():
        return client.call("get_entity_inspector", {"id": SOURCE})["helpers"]

    def rows():
        return inspect()["lines"]

    def snapshot():
        info = client.call("get_map_info")
        return {
            "objects": detail(client, [SOURCE, START, END]),
            "history": client.call("get_history"),
            "camera": client.call("get_camera"),
            **{key: info[key] for key in ("selection", "unsaved_changes", "view")},
        }

    def render(name, camera):
        png, metadata = client.render_result(width=SIZE, height=SIZE, camera=camera, selection_outlines=False)
        path = output / f"{name}.png"
        path.write_bytes(png)
        images.append({"file": path.name, "sha256": hashlib.sha256(png).hexdigest(), "camera": metadata["camera"]})
        return rgba_png(path)

    def guide_pixels(pixels, plain):
        green = orange = 0
        for offset in range(0, len(pixels), 4):
            red, green_channel, blue = pixels[offset:offset + 3]
            changed = max(abs(a - b) for a, b in zip(pixels[offset:offset + 3], plain[offset:offset + 3], strict=True)) > 8
            green += changed and green_channel > red + 40 and green_channel > blue + 40
            orange += changed and red > green_channel + 40 and green_channel > blue + 30
        return green, orange

    def open_fixture(name, text):
        path = output / f"{name}.vmf"
        path.write_text(text)
        definitions()
        client.call("open_map", {"path": str(path), "discard_changes": True})
        client.call("select", {"ids": [SOURCE]})
        return path

    source_path = open_fixture("authored-nodeids", fixture())
    client.call("set_spatial_settings", {"grid_visible": False})
    client.call("set_transform_tool", {"mode": "select"})
    first = rows()
    check(len(first) == 2 and all(row["state"] == "ready" and row["displayed"] for row in first), "Imported authored node IDs support both inherited node_dest line forms")
    check([[endpoint["entity"] for endpoint in row["endpoints"]] for row in first] == [[1001, 202], [101, 202]], "Node0 and node42 resolve independently of VMF IDs101/202 and the VMF-ID42 distractor")
    check(first[0]["endpoints"][0]["lookup_property"] is None and all(endpoint["lookup_property"] == "nodeid" for endpoint in [first[0]["endpoints"][1], *first[1]["endpoints"]]), "Endpoint metadata distinguishes nodeid lookup from the source origin")
    check([endpoint["property"] for endpoint in first[1]["endpoints"]] == ["StartNode", "EndNode"], "StartNode and EndNode retain declared node_dest field spelling")
    check(all(endpoint["value_source"] == "fgd_default" for endpoint in [first[0]["endpoints"][1], *first[1]["endpoints"]]) and "authored nodeid" in inspect()["status"], "Absent source references retain FGD-default provenance and explicit authored-ID human status")
    check(not client.call("get_map_info")["rendering"]["initialized"], "Imported ID inspection and definition loading stay CPU-only before rendering")
    before = snapshot()
    rejected = client.call("set_entity_properties", {"id": END, "set": {"nodeid": "43"}}, error=True)
    check("read-only" in json.dumps(rejected).lower() and snapshot() == before, "Readonly node IDs reject shared property edits without changing source or history")
    client.call("set_entity_properties", {"id": SOURCE, "set": {"aim": "0", "count": "invalid"}}, error=True)
    check(snapshot() == before, "An invalid shared property batch does not partially change a valid node reference")
    client.call("set_entity_properties", {"id": SOURCE, "set": {"aim": "42"}})
    check(rows()[0]["endpoints"][1]["value_source"] == "authored", "Editable node_dest references label authored overrides independently of endpoint IDs")
    client.call("undo")
    client.call("set_entity_properties", {"id": SOURCE, "set": {"aim": "202"}})
    check(rows()[0]["state"] == "missing" and not rows()[0]["displayed"], "A destination equal to an existing VMF entity ID does not infer a node association")
    client.call("undo")
    for value, state in [("-1", "no_association"), ("042", "unsupported_reference"), ("+42", "unsupported_reference"), (" 42", "unsupported_reference"), ("2147483647", "missing"), ("2147483648", "unsupported_reference")]:
        client.call("set_entity_properties", {"id": SOURCE, "set": {"aim": value, "EndNode": value}})
        check(all(row["state"] == state and not row["displayed"] and row["error"] for row in rows()), f"Node reference {value!r} reports {state} without a replacement endpoint")
        client.call("undo")
    body_ray = {"origin": [0, -200, 16], "direction": [0, 1, 0]}
    guide_ray = {"origin": [60, 0, 160], "direction": [0, 0, -1]}
    body_hits = client.call("get_object_hit_candidates", body_ray)
    bounds = detail(client, [SOURCE])[0]["bounds"]
    check(body_hits["total"] == 1 and client.call("get_object_hit_candidates", guide_ray)["total"] == 0, "Ordinary source box is pickable while node-line-only space is not")
    volume = client.call("select_volume", {"mins": [48, -12, 32], "maxs": [72, 12, 64], "rule": "intersecting", "preview": True})["preview"]
    check(not volume["objects"], "Volume selection ignores node line spans")
    client.call("cancel_volume_selection", {"token": volume["token"]})
    client.call("frame_selection", {"aspect": 1})
    framed = client.call("get_camera")
    baseline = snapshot()
    saved = source_path.read_bytes()
    cameras = {projection: {"projection": projection, "center": [0, 0, 16], "ortho_height": 400} for projection in ("top", "front", "side")}
    cameras["perspective"] = {"projection": "perspective", "position": [280, -360, 400], "look_at": [0, 0, 16]}
    shown = {projection: render(f"node-lines-{projection}", camera) for projection, camera in cameras.items()}
    definitions(FGD.replace(SELF_LINE, "").replace(PAIR_LINE, ""))
    check(not rows(), "Removing helper declarations keeps imported IDs and removes the guide report")
    for projection, camera in cameras.items():
        plain = render(f"no-node-lines-{projection}", camera)
        counts = guide_pixels(shown[projection], plain)
        check(min(counts) > 40, f"Both line-specific colors add actual pixels beyond identical ordinary geometry in {projection}")
        measurements[projection] = {"green_guide_pixels": counts[0], "orange_guide_pixels": counts[1]}
    client.call("frame_selection", {"aspect": 1})
    check(client.call("get_camera") == framed and snapshot() == baseline, "Removing declarations leaves ordinary framing, objects, camera and exact history unchanged")
    definitions()
    check(render("node-lines-restored", cameras["top"]) == shown["top"], "Definition reload restores exact original node guide pixels")
    client.call("transform_objects", {"ids": [SOURCE, END], "translation": [16, 24, 32], "preview": True})
    preview = rows()
    check([[endpoint["position"] for endpoint in row["endpoints"]] for row in preview] == [[[16, -56, 48], [136, 104, 112]], [[-120, 80, -80], [136, 104, 112]]], "Guide source and node endpoints follow displayed previews while the fixed Start node remains exact")
    check(render("node-lines-preview", cameras["top"]) != shown["top"] and detail(client, [SOURCE, START, END]) == baseline["objects"], "Actual preview pixels move without authoring readonly node IDs or committed positions")
    client.call("cancel_transform_preview")
    client.call("select", {"ids": [SOURCE]})
    client.call("set_object_visibility", {"action": "hide", "ids": [END]})
    check(all(row["state"] == "ready" and row["suppression_reason"] == "endpoint_hidden" for row in rows()), "Hidden canonical node retains intrinsic resolution and suppresses both guides")
    hidden = render("node-lines-endpoint-hidden", cameras["top"])
    definitions(FGD.replace(SELF_LINE, "").replace(PAIR_LINE, ""))
    check(render("no-lines-endpoint-hidden", cameras["top"]) == hidden, "Hidden endpoint contributes no guide pixels against the same hidden no-guide control")
    definitions()
    client.call("undo")
    configure(client, action="set", mins=[-16, -100, 0], maxs=[16, -60, 32])
    check(all(row["suppression_reason"] == "endpoint_outside_working_region" for row in rows()), "Working regions suppress outlying node endpoints while retaining the source box")
    configure(client, action="clear")
    client.call("select", {"ids": []})
    check(all(row["state"] == "ready" and row["suppression_reason"] == "not_selected" for row in rows()), "Unselected source explains suppression without losing authored node associations")
    client.call("select", {"ids": [SOURCE]})
    restored = snapshot()
    check(all(restored[key] == baseline[key] for key in restored if key != "history") and restored["history"]["undo_count"] == baseline["history"]["undo_count"] and source_path.read_bytes() == saved, "Reversible controls restore authored fields, source bytes, camera and Undo count")
    check(render("node-lines-final", cameras["top"]) == shown["top"] and rows() == first and snapshot() == restored, "Restored guide reads reproduce exact pixels and preserve the complete current Undo/Redo history")
    check(client.call("get_object_hit_candidates", body_ray) == body_hits and detail(client, [SOURCE])[0]["bounds"] == bounds, "All guide operations retain ordinary picks and source bounds")
    for name, text, state, matches, hide_alias in [
        ("duplicate-canonical-hidden", fixture(alias="42"), "ambiguous", 2, True),
        ("duplicate-alias-hidden", fixture(alias="042"), "ambiguous", 2, True),
        ("duplicate-plus-alias-hidden", fixture(alias="+42"), "ambiguous", 2, True),
        ("duplicate-padded-alias-hidden", fixture(alias=" 42 "), "ambiguous", 2, True),
        ("sole-alias", fixture("042"), "invalid_nodeid", 1, False),
        ("duplicate-keys", fixture(extra_ids=[("nodeid", "42")]), "duplicate_nodeid", 1, False),
        ("endpoint-default-only", fixture(omit_id=True), "missing", 0, False),
        ("out-of-range-endpoint", fixture("2147483648"), "missing", 0, False),
    ]:
        path = open_fixture(name, text)
        if hide_alias:
            client.call("set_object_visibility", {"action": "hide", "ids": [ALIAS]})
        state_before = snapshot()
        report = rows()
        check(all(row["state"] == state and not row["displayed"] and row["endpoints"][-1]["matches"] == matches for row in report), f"Imported {name} is {state}, including hidden numeric aliases without first-match selection")
        actual = render(name, cameras["top"])
        definitions(FGD.replace(SELF_LINE, "").replace(PAIR_LINE, ""))
        check(render(name + "-no-guide", cameras["top"]) == actual, f"Imported {name} adds no guide pixels against an identical no-guide fixture")
        check(snapshot() == state_before and path.read_text() == text, f"Read/render of {name} preserves imported readonly text, fields, camera and full history")
        cases.append({"case": name, "lines": report, "source_sha256": hashlib.sha256(text.encode()).hexdigest()})
    open_fixture("restored-canonical", fixture())
    for declaration in ("nodeid(integer)", "nodeid(string) readonly"):
        definitions(FGD.replace("nodeid(integer) readonly", declaration))
        check(all(row["state"] == "missing" and not row["displayed"] for row in rows()), f"Endpoint declaration {declaration} cannot establish readonly integer node ownership")
    definitions()
    check(render("canonical-after-errors", cameras["top"]) == shown["top"], "Restoring canonical imported IDs and declarations recovers the original image")
    (output / "results.json").write_text(json.dumps({"passed": len(checks), "checks": checks, "lines": first, "cases": cases, "measurements": measurements, "images": images, "source_sha256": hashlib.sha256(saved).hexdigest(), "identity_policy": "Canonical authored readonly FGD nodeid only; no generated IDs, VMF entity ID inference, runtime node indices or AI graph claim", "desktop_input": False, "native_exercised": False}, indent=2) + "\n")
    print(json.dumps({"passed": len(checks), "output": str(output)}))


def main():
    parser = argparse.ArgumentParser(description="Validate imported authored nodeid FGD lines through private MCP and actual paired guide pixels.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--gameinfo", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    with patch.dict(os.environ, {"HAMMERR_RECENT_MAPS_DIR": str(output / "recent"), "HAMMERR_BUILD_RECIPES": str(output / "recipes.json")}):
        client = Client(None, args.binary.resolve(), output, args.gameinfo)
    try:
        validate(client, output)
    finally:
        client.close()


if __name__ == "__main__":
    main()
