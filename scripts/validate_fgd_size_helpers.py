import argparse
import json
from pathlib import Path

from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_working_region import configure


FGD = '''
@BaseClass size(-24 -12 0, 40 20 72) color(12 134 216) = shared []
@PointClass base(shared) sphere(radius) = helper_sample [
    angles(angle) : "Orientation" : "0 90 0"
    radius(float) : "Radius" : 64
]
'''


def validate(client, output):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def render(name, **options):
        image, metadata = client.render_result(width=401, height=401, **options)
        (output / f"{name}.png").write_bytes(image)
        return image, metadata

    fgd = output / "helpers.fgd"
    fgd.write_text(FGD)
    client.call("new_map", {"discard_changes": True})
    client.call("load_entity_definitions", {"path": str(fgd)})
    client.call("set_spatial_settings", {"grid_visible": False})
    client.call("set_transform_tool", {"mode": "select"})
    entity = client.call("create_entity", {"classname": "helper_sample", "origin": [0, 0, 0]})["id"]
    inspector = client.call("get_entity_inspector", {"id": entity})
    helper = inspector["helpers"]
    bounds = {"mins": [-24, -12, 0], "maxs": [40, 20, 72]}
    check(helper["bounds_source"] == "fgd_size" and helper["world_bounds"] == list(bounds.values()), "Shared inspector resolves inherited asymmetric size in Source units")
    check(helper["unsupported"] == ["sphere"] and helper["class_color"] == [12, 134, 216], "Unsupported radius semantics and class color are explicit")
    check(str(fgd) in helper["definition_sources"] and "file/line" in helper["limits"], "Inspection identifies loaded sources and precise provenance limits")
    check(detail(client, [entity])[0]["bounds"] == bounds, "Object inspection uses displayed helper bounds")
    source = output / "source.vmf"
    client.call("save_map", {"path": str(source)})
    saved = source.read_bytes()
    before = client.call("get_map_info")
    ray = {"origin": [30, -100, 64], "direction": [0, 1, 0]}
    candidates = client.call("get_object_hit_candidates", ray)
    check(candidates["total"] == 1 and candidates["candidates"][0]["point"] == [30, -12, 64], "Ordered ray picking hits the class box outside the generic marker")
    volume = client.call("select_volume", {"mins": [25, -20, 50], "maxs": [45, 25, 80], "rule": "intersecting", "preview": True})
    check(volume["preview"]["objects"] == [entity], "Volume selection intersects the displayed box away from the entity origin")
    client.call("cancel_volume_selection", {"token": volume["preview"]["token"]})
    camera = {"projection": "perspective", "position": [130, -190, 130], "look_at": [8, 4, 36]}
    selected, _ = render("selected-size-box", camera=camera)
    no_outline, _ = render("selected-size-box-no-outline", camera=camera, selection_outlines=False)
    check(selected != no_outline, "Existing selection-outline control gates the class box edges")
    client.call("select", {"ids": []})
    unselected, _ = render("class-color-size-box", camera=camera)
    check(unselected != selected, "Class color and selected appearance produce distinct shared renders")
    region = configure(client, action="set", mins=[25, -20, 50], maxs=[45, 25, 80])
    check(region["objects"][0]["included"] and region["objects"][0]["bounds_source"] == "fgd_size", "Working region uses the class size, including noncentral overlap")
    configure(client, action="set", mins=[100, 100, 100], maxs=[200, 200, 200])
    excluded, _ = render("excluded-size-box", camera=camera)
    check(client.call("get_object_hit_candidates", ray)["total"] == 0 and excluded != unselected, "Working-region exclusion affects both rendering and picking")
    fgd.write_text("@PointClass size(0 0 0) = helper_sample []")
    client.call("load_entity_definitions", {"path": str(fgd)})
    invalid = client.call("get_entity_inspector", {"id": entity})["helpers"]
    check(invalid["invalid_size"] and invalid["local_bounds"] is None and invalid["bounds_source"] == "invalid_fgd_size" and invalid["errors"], "Invalid size reload exposes failure rather than successful fabricated class bounds")
    invalid_region = client.call("get_working_region")["objects"][0]
    check(invalid_region["included"] and invalid_region["bounds"] is None and invalid_region["bounds_error"] and "unresolved_bounds" in invalid_region["reasons"], "Invalid class bounds retain an editable marker with unresolved working-region diagnostics")
    configure(client, action="clear")
    invalid_image, _ = render("invalid-size-amber-marker", camera=camera)
    check(invalid_image != unselected, "Invalid helper has a distinct diagnostic marker")
    fgd.write_text(FGD)
    client.call("load_entity_definitions", {"path": str(fgd)})
    check(render("restored-size-box", camera=camera)[0] == unselected, "Reload restores exact shared helper geometry and color")
    client.call("select", {"ids": [entity]})
    after = client.call("get_map_info")
    check(after["undo"] == before["undo"] and after["selection"] == before["selection"] and not after["unsaved_changes"], "Definition/render/filter inspection preserves history, selection and dirty state")
    check(source.read_bytes() == saved, "FGD helpers leave the saved VMF untouched")
    (output / "results.json").write_text(json.dumps({"checks": checks, "inspector": inspector, "invalid": invalid}, indent=2))
    print(json.dumps({"passed": len(checks), "output": str(output)}))


def main():
    parser = argparse.ArgumentParser(description="Validate shared FGD size helpers in a private stdio MCP process.")
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
