import argparse
import json
from pathlib import Path
from unittest.mock import patch

from validate_face_lighting import DEFAULT_BINARY, Client


def validate(client, output):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    entity = client.call("create_entity", {
        "classname": "info_particle_system", "origin": [0, 0, 32],
        "properties": {"targetname": "particle_demo", "effect_name": "", "start_active": "0"},
    })["id"]
    inspector = client.call("get_entity_inspector", {"id": entity})
    field = next(row for row in inspector["properties"] if row["key"] == "effect_name")
    check(field["control"] == "particle" and not field["advanced"], "Inspector exposes a readable particle browser field")
    first = client.call("browse_particles", {"limit": 1})
    second = client.call("browse_particles", {"offset": 1, "limit": 1})
    check(first["total"] > 100 and first["entries"][0] != second["entries"][0], "Active manifest provides paginated parsed system names")
    check(first["manifest"] == "particles/particles_manifest.txt", "Catalog names its global active manifest")
    catalog = client.call("browse_particles", {"query": "fire_medium_01", "limit": 100})
    check(any(row["name"] == "fire_medium_01" for row in catalog["entries"]), "Search finds a real installed named system")
    folders = [row for row in catalog["groups"] if row["id"].startswith("folder:")]
    check(bool(folders), "Readable particle folder groups are available")
    fire = client.call("inspect_particle", {"name": "fire_medium_01"})
    check(fire["source"] == "particles/fire_01.pcf" and fire["encoding_version"] == 2, "Inspection resolves the active binary2 PCF definition")
    check(bool(fire["material"]) and bool(fire["functions"]["renderers"]), "Inspection returns parsed material and renderer metadata")
    check("no animation" in fire["limits"], "Inspection explicitly states the definition-only limit")
    muzzle = client.call("inspect_particle", {"name": "muzzle_pistols"})
    check(muzzle["precache"], "Manifest precache marker is preserved")
    file_group = client.call("browse_particles", {"group": "file:particles/muzzleflashes.pcf", "limit": 100})
    check(all(row["source"] == "particles/muzzleflashes.pcf" for row in file_group["entries"]), "Particle file groups preserve source paths")
    for tool, arguments in [
        ("browse_particles", {"limit": 101}), ("browse_particles", {"typo": True}),
        ("browse_particles", {"group": "unknown"}), ("inspect_particle", {"name": "unlisted_guessed_system"}),
    ]:
        client.call(tool, arguments, error=True)
    before = client.call("get_objects", {"ids": [entity]})
    history = client.call("get_map_info")["undo"]
    camera = client.call("get_camera")
    selection = client.call("get_map_info")["selection"]
    token = client.call("begin_particle_choice", {"id": entity})["token"]
    check(client.call("begin_particle_choice", {"id": entity, "resume": True})["token"] == token, "Human/agent handoff resumes the owned same-target choice")
    client.call("choose_particle", {"token": token, "name": "fire_medium_01"})
    client.call("choose_particle", {"token": token, "name": "missing_effect"}, error=True)
    check(client.call("get_particle_choice")["particle"]["name"] == "fire_medium_01", "Invalid choice preserves the reviewed system")
    check(client.call("get_objects", {"ids": [entity]}) == before, "Choosing a particle leaves source unchanged")
    info = client.call("get_map_info")
    check(info["undo"] == history and info["selection"] == selection and client.call("get_camera") == camera, "Inspection preserves history, selection and camera")
    check(not info["rendering"]["initialized"], "Particle definition exploration needs no graphics")
    client.call("apply_particle_choice", {"token": token})
    applied = client.call("get_objects", {"ids": [entity]})
    check(dict(applied["objects"][0]["properties"])["effect_name"] == "fire_medium_01", "Apply sets the native effect_name property")
    check(client.call("browse_particles", {"group": "used"})["total"] == 1, "Used in this map includes the committed system")
    client.call("undo")
    check(client.call("get_objects", {"ids": [entity]}) == before, "One Undo restores the exact prior entity")
    client.call("redo")
    old = client.call("begin_particle_choice", {"id": entity})["token"]
    current = client.call("begin_particle_choice", {"id": entity})["token"]
    client.call("cancel_particle_choice", {"token": old}, error=True)
    check(client.call("get_particle_choice", {"token": current})["valid"], "Foreign stale Cancel preserves the replacement choice")
    client.call("choose_particle", {"token": current, "name": "muzzle_pistols"})
    client.call("transform_objects", {"ids": [entity], "translation": [8, 0, 0], "preview": True})
    client.call("apply_particle_choice", {"token": current}, error=True)
    check(not client.call("get_particle_choice")["valid"], "Geometry previews invalidate particle Apply")
    client.call("cancel_particle_choice", {"token": current})
    client.call("cancel_transform_preview")
    token = client.call("begin_particle_choice", {"id": entity})["token"]
    client.call("choose_particle", {"token": token, "name": "muzzle_pistols"})
    client.call("reload_particle_assets")
    check(not client.call("get_particle_choice")["valid"], "Shared particle reload invalidates a captured choice")
    client.call("cancel_particle_choice", {"token": token})
    check(client.call("browse_particles", {"query": "fire_medium_01"})["total"] == catalog["total"], "Search reloads active definitions after explicit reload")
    path = output / "particles.vmf"
    client.call("save_map", {"path": str(path)})
    client.call("open_map", {"path": str(path), "discard_changes": True})
    check(dict(client.call("get_objects", {"ids": [entity]})["objects"][0]["properties"])["effect_name"] == "fire_medium_01", "Save/reopen preserves the chosen native name")
    (output / "results.json").write_text(json.dumps({"checks": len(checks), "passed": checks, "catalog": first, "definition": fire}, indent=2) + "\n")
    return checks


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--output", type=Path, default=Path("/tmp/hammerr-particles"))
    args = parser.parse_args()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=True)
    with patch.dict("os.environ", {
        "HAMMERR_RECENT_MAPS_DIR": str(args.output / "recent"),
        "HAMMERR_BUILD_RECIPES": str(args.output / "recipes.json"),
    }):
        client = Client(None, args.binary, args.output)
    try:
        checks = validate(client, args.output)
        print(json.dumps({"checks": len(checks), "output": str(args.output)}))
    finally:
        client.close()


if __name__ == "__main__":
    main()
