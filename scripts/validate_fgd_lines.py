import argparse
import json
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_sky_preview import rgba_png
from validate_working_region import configure

SIZE = 401
DOUBLE = "line(240 80 24, targetname, start, targetname, aim)"
FGD = f'''
@BaseClass line(32 220 80, targetname, aim) = line_base [
    aim(target_destination) : "Target" : "end"
]
@PointClass base(line_base) size(8 8 8) color(40 170 225) {DOUBLE}
    = line_sample [
    start(target_destination) : "Start" : "start"
    count(integer) : "Count" : 0
]
@PointClass size(8 8 8) = line_target []
@PointClass size(8 8 8) line(0 0 255, nodeid, aim) = line_unsupported [
    aim(string) : "Target" : "end"
]
'''


def validate(client, output):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def inspect(entity):
        return client.call("get_entity_inspector", {"id": entity})["helpers"]

    def numeric(entity):
        return int(entity.split(":")[-1])

    def author(entity, **values):
        client.call("set_entity_properties", {"id": entity, "set": values})

    def render(name, camera):
        png, metadata = client.render_result(width=SIZE, height=SIZE, camera=camera, selection_outlines=False)
        path = output / f"{name}.png"
        path.write_bytes(png)
        return rgba_png(path), metadata

    def colors(pixels):
        green = orange = 0
        for offset in range(0, len(pixels), 4):
            r, g, b = pixels[offset:offset + 3]
            green += g > r + 40 and g > b + 40
            orange += r > g + 40 and g > b + 30
        return green, orange

    fgd = output / "lines.fgd"
    fgd.write_text(FGD)
    client.call("new_map", {"discard_changes": True})
    client.call("load_entity_definitions", {"path": str(fgd)})
    client.call("set_spatial_settings", {"grid_visible": False})
    client.call("set_transform_tool", {"mode": "select"})
    start = client.call("create_entity", {"classname": "line_target", "origin": [-120, 80, 16], "properties": {"targetname": "start"}})["id"]
    end = client.call("create_entity", {"classname": "line_target", "origin": [120, 80, 16], "properties": {"targetname": "end"}})["id"]
    source = client.call("create_entity", {"classname": "line_sample", "origin": [0, -80, 16]})["id"]
    rows = inspect(source)["lines"]
    check(len(rows) == 2 and all(row["state"] == "ready" and row["displayed"] for row in rows), "Inherited single-pair and two-pair declarations share selected inspector state")
    check(rows[0]["color"] == [32, 220, 80] and rows[1]["color"] == [240, 80, 24], "Each declaration retains its literal RGB independent of entity render color")
    check([point["entity"] for point in rows[0]["endpoints"]] == [numeric(source), numeric(end)] and [point["entity"] for point in rows[1]["endpoints"]] == [numeric(start), numeric(end)], "Single pair means self-to-target; two pairs mean the two referenced entities")
    client.call("set_entity_properties", {"id": source, "remove": ["aim"]})
    check(inspect(source)["lines"][0]["endpoints"][1]["value_source"] == "fgd_default", "Absent reference uses a labeled FGD default without fabricating target names")
    author(source, aim="end")
    check(inspect(source)["lines"][0]["endpoints"][1]["value_source"] == "authored", "Authored reference overrides the definition default")
    check(client.call("get_map_info")["rendering"]["initialized"] is False, "Line resolution stays headless until actual pixels are requested")
    body_ray = {"origin": [0, -200, 16], "direction": [0, 1, 0]}
    line_ray = {"origin": [60, 0, 128], "direction": [0, 0, -1]}
    body_hits = client.call("get_object_hit_candidates", body_ray)
    bounds = detail(client, [source])[0]["bounds"]
    check(body_hits["total"] == 1 and client.call("get_object_hit_candidates", line_ray)["total"] == 0, "Ordinary helper boxes remain pickable and guide-only positions do not become objects")
    volume = client.call("select_volume", {"mins": [48, -16, 8], "maxs": [72, 16, 24], "rule": "intersecting", "preview": True})["preview"]
    check(volume["objects"] == [], "Volume selection ignores the line span")
    client.call("cancel_volume_selection", {"token": volume["token"]})
    client.call("frame_selection", {"aspect": 1})
    framed = client.call("get_camera")
    camera = {"projection": "top", "center": [0, 0, 16], "ortho_height": 360}
    author(source, aim="missing")
    empty, _ = render("missing-endpoint", camera)
    author(source, aim="end")
    shown, _ = render("fgd-lines", camera)
    changed = sum(max(abs(a - b) for a, b in zip(empty[offset:offset + 3], shown[offset:offset + 3], strict=True)) > 8 for offset in range(0, len(shown), 4))
    check(changed > 200 and all(value > 80 for value in colors(shown)), "Actual top-view pixels contain two distinct colored links beyond the unchanged boxes")
    client.call("frame_selection", {"aspect": 1})
    check(client.call("get_camera") == framed and detail(client, [source])[0]["bounds"] == bounds, "Line endpoints do not enlarge selection framing or authored helper bounds")
    history = client.call("get_history")["undo_count"]
    authored = detail(client, [source, start, end])
    fgd.write_text(FGD.replace(DOUBLE, ""))
    client.call("load_entity_definitions", {"path": str(fgd)})
    one, _ = render("single-self-to-target-line", camera)
    check(len(inspect(source)["lines"]) == 1 and colors(one)[0] > 80 and colors(one)[1] == 0, "Definition reload independently removes the two-target link while retaining the self link")
    check(detail(client, [source, start, end]) == authored and client.call("get_history")["undo_count"] == history, "Definition reload changes neither authored properties nor exact Undo count")
    fgd.write_text(FGD)
    client.call("load_entity_definitions", {"path": str(fgd)})
    check(render("restored-definition", camera)[0] == shown, "Reload restores identical actual line pixels")
    client.call("transform_objects", {"ids": [end], "translation": [0, 32, 0], "preview": True})
    check(inspect(source)["lines"][0]["endpoints"][1]["position"] == [120, 112, 16], "Named endpoints follow displayed transform-preview origins")
    preview, _ = render("endpoint-transform-preview", camera)
    check(preview != shown and detail(client, [source, start, end]) == authored, "Preview line pixels move while committed map data stays exact")
    client.call("cancel_transform_preview")
    check(render("cancelled-endpoint-preview", camera)[0] == shown and client.call("get_history")["undo_count"] == history, "Cancelling the preview restores exact pixels without history")
    for projection in ["front", "side", "perspective"]:
        view = {**camera, "projection": projection} if projection != "perspective" else {"projection": "perspective", "position": [260, -320, 240], "look_at": [0, 0, 16]}
        actual, _ = render(f"lines-{projection}", view)
        author(source, aim="missing")
        absent, _ = render(f"missing-{projection}", view)
        check(actual != absent, f"The shared line guides also render in {projection} projection")
        client.call("undo")
    source_path = output / "authored.vmf"
    client.call("save_map", {"path": str(source_path)})
    saved = source_path.read_bytes()
    authored = detail(client, [source, start, end])
    history = client.call("get_history")["undo_count"]
    camera_before = client.call("get_camera")
    client.call("set_entity_properties", {"id": source, "set": {"aim": "missing", "count": "invalid"}}, error=True)
    check(detail(client, [source, start, end]) == authored and client.call("get_history")["undo_count"] == history, "Malformed shared property batches preserve all references and exact history atomically")
    for value, state in [("missing", "missing"), ("END", "unsupported_case_match"), ("end*", "unsupported_reference"), ("!self", "unsupported_reference")]:
        author(source, aim=value)
        report = inspect(source)
        check(report["lines"][0]["state"] == state and not report["lines"][0]["displayed"] and report["lines"][0]["error"], f"Literal reference {value!r} reports {state} without an invented endpoint")
        client.call("undo")
    duplicate = client.call("create_entity", {"classname": "line_target", "origin": [200, 80, 16], "properties": {"targetname": "end"}})["id"]
    client.call("set_object_visibility", {"action": "hide", "ids": [duplicate]})
    client.call("select", {"ids": [source]})
    ambiguous = inspect(source)
    check(ambiguous["lines"][0]["state"] == "ambiguous" and ambiguous["lines"][0]["endpoints"][1]["matches"] == 2 and "ambiguous" in ambiguous["status"], "Hidden duplicate names remain explicitly ambiguous in shared metadata and human summary")
    client.call("undo")
    client.call("undo")
    client.call("select", {"ids": [source]})
    client.call("set_object_visibility", {"action": "hide", "ids": [end]})
    hidden = inspect(source)
    check(all(row["state"] == "ready" and row["suppression_reason"] == "endpoint_hidden" and not row["displayed"] for row in hidden["lines"]), "Hidden endpoints retain intrinsic resolution and explicitly suppress both lines")
    check(colors(render("hidden-endpoint", camera)[0]) == (0, 0), "Hidden endpoint links disappear from real pixels")
    client.call("undo")
    configure(client, action="set", mins=[-16, -100, 0], maxs=[16, -60, 32])
    check(all(row["suppression_reason"] == "endpoint_outside_working_region" for row in inspect(source)["lines"]), "Working-region endpoint filtering suppresses links even while the source box remains included")
    configure(client, action="clear")
    client.call("select", {"ids": []})
    check(all(row["state"] == "ready" and row["suppression_reason"] == "not_selected" for row in inspect(source)["lines"]), "Unselected source explains suppression without losing resolved endpoints")
    check(colors(render("unselected-lines", camera)[0]) == (0, 0), "Unselected guides disappear independently of ordinary entity geometry")
    client.call("select", {"ids": [source]})
    first = inspect(source)
    check(inspect(source) == first and source_path.read_bytes() == saved and detail(client, [source, start, end]) == authored, "Repeated resolution and visibility controls preserve saved bytes and authored data")
    check(client.call("get_history")["undo_count"] == history and client.call("get_camera") == camera_before, "Guide inspection and rendering preserve exact history and active camera")
    check(client.call("get_object_hit_candidates", body_ray) == body_hits and detail(client, [source])[0]["bounds"] == bounds, "All rendering controls retain ordinary picking and bounds")
    check(render("restored-lines", camera)[0] == shown, "Restored source and endpoint state reproduce identical actual pixels")
    unsupported = client.call("create_entity", {"classname": "line_unsupported", "origin": [300, 0, 16]})["id"]
    check(inspect(unsupported)["lines"][0]["state"] == "unsupported" and not inspect(unsupported)["lines"][0]["displayed"], "Numeric nodeid helper declarations remain explicitly unsupported")
    client.call("undo")
    (output / "results.json").write_text(json.dumps({"passed": len(checks), "checks": checks, "helpers": first, "ambiguous": ambiguous, "hidden": hidden, "desktop_input": False}, indent=2) + "\n")
    print(json.dumps({"passed": len(checks), "output": str(output)}))


def main():
    parser = argparse.ArgumentParser(description="Validate selected literal FGD line helpers through private MCP operations and actual PNG controls.")
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--gameinfo", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    client = Client(None, args.binary.resolve(), output, args.gameinfo)
    try:
        validate(client, output)
    finally:
        client.close()


if __name__ == "__main__":
    main()
