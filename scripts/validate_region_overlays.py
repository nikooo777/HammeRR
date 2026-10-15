import argparse
import json
from pathlib import Path
from unittest.mock import patch

import validate_native_runtime as runtime
import validate_overlays as overlay_checks
from validate_compile_jobs import wait_job
from validate_compile_regions import apply
from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_terrain_lod import build


LOW = [-224, -224, -16]
HIGH = [224, 224, 272]


def fixture(args):
    imported_args = argparse.Namespace(**vars(args))
    imported_args.output = args.output / "imported-overlay"
    imported_args.output.mkdir(exist_ok=True)
    overlay_checks.fixture(imported_args)
    client = Client(None, args.binary, args.output)
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    def preserved():
        ids = [item["id"] for item in client.call("find_objects", {"limit": 500})["objects"]]
        info = client.call("get_map_info")
        return detail(client, ids), info["undo"], info["selection"]

    def reject(source, expected, path, previous):
        client.call("open_map", {"path": str(source), "discard_changes": True})
        source_bytes = source.read_bytes()
        before = preserved()
        rejected = client.call("compile_map", {"path": str(path)}, error=True)
        check(expected in json.dumps(rejected) and path.read_bytes() == previous,
              f"{source.stem}: synchronous rejection preserves previous output")
        client.call("start_compile", {"path": str(path)})
        job = wait_job(client)
        check(job["status"] == "failed" and path.read_bytes() == previous,
              f"{source.stem}: background rejection preserves previous output")
        check(preserved() == before and source.read_bytes() == source_bytes,
              f"{source.stem}: both failed builds preserve imported source and history")

    try:
        imported = imported_args.output / "overlays.vmf"
        client.call("open_map", {"path": str(imported), "discard_changes": True})
        helper = client.call("find_objects", {"classname": "info_overlay"})["objects"][0]
        helper_id = helper["id"]
        properties = dict(detail(client, [helper_id])[0]["properties"])
        source_sides = properties["sides"]
        check(helper["origin"] == [1000, 0, 0], "Imported editor marker remains outside the region")
        client.call("set_entity_properties", {"id": helper_id, "set": {"BasisNormal": "0 0 1"}}, error=True)
        apply(client, low=LOW, high=HIGH)
        source = args.output / "region-overlays.vmf"
        client.call("save_map", {"path": str(source)})
        source_bytes = source.read_bytes()
        apply(client, clear=True)
        full_path = args.output / "full-map-same-version.bsp"
        full = client.call("compile_map", {"path": str(full_path)})
        client.call("undo")
        before = preserved()
        path, stats = build(client, args.output, "region")
        previous = path.read_bytes()
        rows, _ = overlay_checks.overlays(previous, stats)
        full_rows, _ = overlay_checks.overlays(full_path.read_bytes(), full["stats"])
        check(stats["overlays"] == 1 and stats["overlay_face_refs"] > 0,
              "The region binds the complete imported floor projection")
        check(rows[0][67:] == full_rows[0][67:], "Region preprocessing preserves every native projection field")
        check(helper_id in stats["compile_region"]["included"], "The complete outside-origin overlay is retained")
        check(preserved() == before and source.read_bytes() == source_bytes,
              "Identical synchronous and background builds preserve source, history and selection")
        apply(client, enabled=False, low=LOW, high=HIGH)
        disabled = args.output / "disabled.bsp"
        client.call("compile_map", {"path": str(disabled)})
        check(disabled.read_bytes() == full_path.read_bytes(),
              "Disabled region metadata preserves exact full-map overlay bytes")
        client.call("open_map", {"path": str(source), "discard_changes": True})
        client.call("set_object_visibility", {"action": "hide", "ids": [helper_id]})
        baseline, hidden_stats = build(client, args.output, "region-hidden")
        check(hidden_stats["overlays"] == 0, "Hidden imported overlays produce no region records")
        client.call("undo")
        client.call("set_entity_properties", {"id": helper_id, "set": {"fademindist": "16", "fademaxdist": "32"}})
        faded, faded_stats = build(client, args.output, "region-faded")
        check(overlay_checks.overlays(faded.read_bytes(), faded_stats)[1] == [(256, 1024)],
              "Region output preserves authored overlay fades")
        client.call("open_map", {"path": str(source), "discard_changes": True})
        floor = next(
            item for item in detail(client, [row["id"] for row in client.call("find_objects", {"kind": "solid"})["objects"]])
            if item["bounds"]["mins"][2] == -16 and item["bounds"]["maxs"][2] == 0
        )
        bottom = next(face["side"] for face in floor["faces"] if face["normal"] == [0, 0, -1])
        marker = f'"sides" "{source_sides}"'
        text = source.read_text()
        assert text.count(marker) == 1
        for name, sides, message in [
            ("non-emitted-target", f"{source_sides} {bottom}", "has no emitted world face"),
            ("missing-target", f"{source_sides} 4294967295", "is absent, hidden"),
            ("malformed-sides", "oops", "integer VMF side IDs"),
        ]:
            control = args.output / f"{name}.vmf"
            control.write_text(text.replace(marker, f'"sides" "{sides}"', 1))
            reject(control, message, path, previous)
        client.call("open_map", {"path": str(source), "discard_changes": True})
        outside = client.call("create_block", {
            "mins": [512, 32, 0], "maxs": [544, 64, 32], "material": "dev/dev_measuregeneric01",
        })["id"]
        outside_side = next(face["side"] for face in detail(client, [outside])[0]["faces"] if face["normal"] == [0, 0, 1])
        omitted = args.output / "omitted-target.vmf"
        client.call("save_map", {"path": str(omitted)})
        text = omitted.read_text()
        assert text.count(marker) == 1
        omitted.write_text(text.replace(marker, f'"sides" "{source_sides} {outside_side}"', 1))
        reject(omitted, f"Overlay side {outside_side} is absent", path, previous)
        check(source.read_bytes() == source_bytes, "All controls preserve the original imported region VMF")
        (args.output / "results.json").write_text(json.dumps({"checks": checks, "stats": stats}, indent=2) + "\n")
        return [("baseline", baseline), ("visible", path), ("faded", faded)]
    finally:
        client.close()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, default=Path("/tmp/hammerr-region-overlays"))
    parser.add_argument("--runtime", action="store_true")
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--bsp", type=Path, help=argparse.SUPPRESS)
    parser.add_argument("--game-dir", type=Path, default=Path.home() / ".local/share/Steam/steamapps/common/Counter-Strike Source")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    if args.worker:
        overlay_checks.worker(args)
        return
    with patch.dict("os.environ", {
        "HAMMERR_RECENT_MAPS_DIR": str(args.output / "recent"),
        "HAMMERR_BUILD_RECIPES": str(args.output / "recipes.json"),
    }):
        cases = fixture(args)
    if args.runtime:
        reports = {}
        for name, bsp in cases:
            output = runtime.private_display(Path(__file__), args, bsp, output=args.output / name)
            reports[name] = json.loads((output / "render.json").read_text())
        overlay_checks.check_rendering(reports)
        (args.output / "render-results.json").write_text(json.dumps(reports, indent=2) + "\n")
    print(json.dumps({"output": str(args.output), "region_overlays": True}))


if __name__ == "__main__":
    main()
