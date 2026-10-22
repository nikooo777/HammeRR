import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
from contextlib import ExitStack
from pathlib import Path

from validate_authored_planes import records
from validate_collision_topology import (
    document_state,
    failure_details,
    identity,
    record_failure,
)
from validate_face_lighting import ROOT, Client, detail
from validate_instance_contents import refresh
from validate_instance_reflections import (
    REASON,
    digest,
    material_fixture,
    property_state,
)
from validate_instanced_visclusters import (
    DEFAULT_FGD,
    GEOMETRY_FIELDS,
    PREVIOUS_BSP,
    PREVIOUS_LIN,
    authored_instance,
    close_client,
    decoded_error,
    instance,
    new_map,
    reviewed_open,
    save,
    wait_job,
)
from validate_material_albedo import fixture_texture, identities, payload, require
from validate_native_runtime import private_game
from validate_nested_brush_instances import fixture_blocks
from validate_visibility import ROOM

PREVIEW_REASON = (
    "Child cubemap samples have no reference geometry or icons; "
    "sample bounds, picking and framing are unavailable."
)
LITERAL = ((-96, 0, 64), (96, 0, 64))
SOURCE = {
    "translation": ((-112, 8, 32), (80, 8, 32)),
    "nested": ((72, -80, -120), (72, -80, 72)),
    "fractional": ((-112.875, 7.125, 32.875), (80.875, 7.125, 32.875)),
}
POSES = {"translation": ([16, -8, 32], "0 0 0"),
         "fractional": ([16, -8, 32], "0 0 0"),
         "nested": ([16, -8, 128], "0 0 90"), "identity": ([0, 0, 0], "0 0 0")}
PHYSICAL_LUMPS = (1, 2, 3, 4, 5, 6, 7, 8, 10, 12, 13, 14, 16, 17, 18, 19,
                  26, 27, 30, 31, 48, 51, 52, 53, 55, 56, 58)
COMPILE_STATS = ("brushes", "nodes", "leaves", "faces", "models", "clusters", "cubemap_samples",
                 "visibility_bytes", "visibility_portals", "lighting_mode", "hdr")
NATIVE_NAMES = ("747-ldr-child", "747-ldr-flat", "747-hdr-child", "747-hdr-flat")


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def snapshot(client):
    return {"session": document_state(client), "property_draft": property_state(client)}


def watch(custody, paths):
    for path in paths:
        custody[str(path)] = identity(path)


def cubemap(client, origin, size=6, sides=None):
    identifier = client.call("create_entity", {"classname": "env_cubemap", "origin": origin})["id"]
    fields = dict(detail(client, [identifier])[0]["properties"])
    definition = client.call("get_entity_class", {"classname": "env_cubemap"})
    require({key: value for key, value in fields.items() if key != "origin"}
            == {"classname": "env_cubemap", **definition["defaults"]}
            and fields["cubemapsize"] == "0" and "angles" not in fields and "sides" not in fields,
            "Normal installed-FGD placement did not retain its automatic, unoriented size0 defaults")
    changes = {}
    if size is not None:
        changes["cubemapsize"] = str(size)
    if sides is not None:
        changes["sides"] = sides
    if changes:
        client.call("set_entity_properties", {"id": identifier, "set": changes})
    return {"id": identifier, "normal_fgd_fields": fields}


def child(client, path, points, custody, size=6, sides=None, hidden=False, mixed=False):
    new_map(client)
    helpers = [cubemap(client, point, size=size, sides=sides if index == 0 else None)
               for index, point in enumerate(points)]
    if hidden:
        client.call("set_object_visibility", {"action": "hide", "ids": [helpers[-1]["id"]]})
    if mixed:
        client.call("create_block", {"mins": [-32, -32, 0], "maxs": [-16, -16, 16],
                                      "material": "hammerr_reflections/gray"})
        brush = client.call("create_block", {"mins": [16, 16, 0], "maxs": [32, 32, 16],
                                              "material": "hammerr_reflections/gray"})["id"]
        client.call("tie_to_entity", {"ids": [brush], "classname": "func_detail"})
    save(client, path, custody)
    fields = [dict(row["properties"]) for row in detail(client, [row["id"] for row in helpers])]
    require(all("angles" not in row for row in fields), "Saved child samples acquired synthesized orientation")
    return helpers


def middle(client, path, leaf, custody):
    new_map(client)
    instance(client, leaf, [8, 16, 64], "90 0 0")
    save(client, path, custody)


def parent(client, path, custody, reference=None, placement="translation", literal=None,
           region=False, roots=()):
    new_map(client)
    names = ("gray", "cyan", "red", "green", "yellow", "blue")
    identifiers = []
    for bounds, name in zip(ROOM, names, strict=True):
        identifiers.append(client.call("create_block", {
            "mins": bounds[0], "maxs": bounds[1], "material": f"hammerr_reflections/{name}",
        })["id"])
    for classname, origin, properties in (
        ("info_player_terrorist", [-192, -176, 8], {}),
        ("info_player_counterterrorist", [192, 176, 8], {}),
        ("light", [0, 0, 180], {"_light": "255 255 255 200", "_constant_attn": "1",
                                 "_linear_attn": "0", "_quadratic_attn": "0"}),
    ):
        identifiers.append(client.call("create_entity", {
            "classname": classname, "origin": origin, "properties": properties,
        })["id"])
    owners, authoring = [], None
    if reference is not None:
        client.call("save_map", {"path": str(path),
                                  "expected_generation": client.call("get_map_info")["generation"]})
        owner, authoring = authored_instance(client, reference, *POSES[placement])
        owners.append(owner)
        identifiers.append(owner)
    for point in literal or ():
        identifiers.append(cubemap(client, point)["id"])
    for point in roots:
        identifiers.append(cubemap(client, point)["id"])
    if region:
        apply_bounds = {"snapshot": client.call("get_compile_region")["snapshot"], "enabled": True,
                        "mins": [-254, -254, 4], "maxs": [0, 254, 252]}
        draft = client.call("preview_compile_region", apply_bounds)
        client.call("apply_compile_region", {"token": draft["draft"]["token"]})
    save(client, path, custody)
    client.call("select", {"ids": owners or identifiers[:1]})
    return {"source": path, "owners": owners, "ids": identifiers, "authoring": authoring}


def configure(client, lighting="fullbright", capture=False):
    client.call("set_compile_settings", {
        "lighting": lighting, "physics": "none", "bounces": 0,
        "visibility": "fast", "sky_quality": "fast", "generate_reflections": capture,
        "bake_static_prop_vertices": False, "pack_sky_assets": False,
        "pack_water_assets": False, "pack_static_prop_assets": False,
    })


def inspection(client, control, count, mixed=False):
    before = snapshot(client)
    ready = refresh(client, background=True)
    require(ready["state"] == "ready" and ready["stats"].get("cubemaps", 0) == count
            and ("cubemaps" in ready["stats"]) == bool(count)
            and ("cubemap_preview" in ready) == bool(count),
            "Expanded cubemap usage or omit-zero reference reporting differs")
    if count:
        require(ready["cubemap_preview"] == {"available": False, "reason": PREVIEW_REASON},
                "Cubemap reference inspection lost its explicit unavailable preview reason")
    if mixed:
        require(ready["available"] and ready["stats"]["world_brushes"] == 1
                and ready["stats"]["detail_brushes"] == 1 and ready["geometry"]["triangles"] > 0,
                "Mixed cubemap references lost their actual physical world/detail geometry")
    else:
        require(not ready["available"] and not ready["displayed"]
                and all(ready["geometry"][key] == 0 for key in GEOMETRY_FIELDS)
                and ready["stats"]["world_brushes"] == ready["stats"]["detail_brushes"] == 0
                and ready["face_provenance_total"] == 0
                and all(row["bounds"] is None for row in ready["owners"]),
                "Sample-only references invented mesh data, provenance or bounds")
        client.call("set_instance_contents", {"action": "frame", "token": str(ready["token"]),
                                              "ids": control["owners"]}, error=True)
        pick = {"x": 256, "y": 256, "width": 513, "height": 513,
                "camera": {"projection": "top", "center": [-160, 100, 64], "ortho_height": 64}}
        ordinary = client.call("pick_at_pixel", {**pick, "instance_contents": False})
        referenced = client.call("pick_at_pixel", {**pick, "instance_contents": True})
        require(ordinary == referenced and referenced["object"] not in control["owners"]
                and referenced["solid"] is not None,
                "Child samples became CPU pick geometry in front of the ordinary floor")
    require({row["owner"] for row in ready["owners"]} == set(control["owners"]),
            "Reference inspection lost the root instance owner")
    bindings = client.call("get_cubemap_bindings")
    require(not bindings["available"] and bindings["total"] == 0 and bindings["assignments"] == []
            and bindings["diagnostics"] == [REASON] and bindings["state"]["context_error"] == REASON,
            "Authored root-face binding inspection falsely claimed assembled geometry coverage")
    face = detail(client, [control["ids"][0]])[0]
    refusal = client.call("set_cubemap_bindings", {
        "action": "preview", "helper": int(control["owners"][0].split(":")[-1]),
        "faces": [{"solid": face["solid"], "side": face["faces"][0]["side"]}],
        "operation": "bind", "snapshot": bindings["state"]["snapshot"],
    }, error=True)
    require(REASON in json.dumps(refusal), "Authored binding draft lost the shared instance-context refusal")
    require(snapshot(client) == before and not client.call("get_map_info")["rendering"]["initialized"],
            "Cubemap reference/binding inspection changed source/history/selection/settings or initialized graphics")
    return {"contents": ready, "bindings": bindings}


def compile_one(client, args, output, game, label, control, sources, background=False,
                failure=None, original_ids=(), admission=False):
    target = output / f"{label}.bsp"
    target.write_bytes(PREVIOUS_BSP)
    target.with_suffix(".lin").write_bytes(PREVIOUS_LIN)
    watch(args.custody, (target, target.with_suffix(".lin")))
    previous = identity(target)
    before = snapshot(client)
    protected_paths = [control["source"], *sources]
    protected = {str(path): identity(path) for path in protected_paths}
    mounted = identities(game / "materials")
    recipes = client.call("get_build_recipes")
    captures_before = set(args.native_tmpdir.glob("hammerr_capture_*"))
    recipe_path = Path(recipes["path"])
    recipe_identity = identity(recipe_path)
    request = {"path": str(target), "expected_generation": before["session"]["document"]["generation"]}
    args.compile_calls += 1
    if background:
        started = client.call("start_compile", request)["job"]
        job = wait_job(client, args.timeout)
        require(job["token"] == started["token"] and job["status"] == ("failed" if failure else "completed"),
                f"Unexpected cubemap background outcome: {job}")
        result = job["result"]
        require(result["published"] == (failure is None), "Cubemap background publication status differs")
    else:
        job = None
        response = client.call("compile_map", request, error=failure is not None)
        result = decoded_error(response) if failure else response
        require(result["game"] is None, "Compile launched ordinary Play")
    require(result["success"] == (failure is None), f"Unexpected cubemap compile outcome: {result}")
    if failure:
        diagnostics = [row for row in result["diagnostics"] if row["level"] == "error"]
        require(any(failure in row["message"] for row in diagnostics),
                f"Missing cubemap refusal {failure!r}: {diagnostics}")
        for identifier in original_ids:
            number = identifier.split(":")[-1]
            label_id = f"Child cubemap {number}" if admission else f"child object {number}"
            require(any(row["object"] in control["owners"] and sources[0].name in row["message"]
                        and failure in row["message"] and label_id in row["message"] for row in diagnostics),
                    f"Cubemap refusal lost root owner, leaf path or original helper {identifier}: {diagnostics}")
        require(identity(target) == previous, "Failed cubemap publication touched the previous BSP")
    else:
        require(result.get("leak") is None and payload(target.read_bytes(), 42),
                "Successful sample compile leaked or emitted no cubemap records")
        if job is not None:
            require(not job["stale"] and (not sources or job["instance_dependencies"]["current"]),
                    "Successful cubemap background publication lost current original dependencies")
        watch(args.custody, (target,))
    require(identity(target.with_suffix(".lin")) == args.custody[str(target.with_suffix(".lin"))],
            "Cubemap compile touched the previous pointfile")
    require(snapshot(client) == before and {str(path): identity(path) for path in protected_paths} == protected
            and identities(game / "materials") == mounted and identity(recipe_path) == recipe_identity
            and client.call("get_build_recipes") == recipes,
            "Cubemap compile changed source/dependencies/history/selection/draft/settings/recipes/assets")
    info = client.call("get_map_info")
    require(not info["rendering"]["initialized"] and not info["launching"] and not info["game_running"],
            "Cubemap compile initialized editor graphics or ordinary Play")
    if failure or not before["session"]["settings"]["generate_reflections"]:
        require(set(args.native_tmpdir.glob("hammerr_capture_*")) == captures_before,
                "Sample-only compilation or preflight refusal unexpectedly started native capture")
    return {"result": result, "job": job, "bsp": str(target), "bsp_sha256": digest(target)}


def compile_pair(client, args, output, game, label, control, sources, **options):
    results = {}
    for background in (False, True):
        key = "background" if background else "foreground"
        results[key] = compile_one(client, args, output, game, f"{label}-{key}", control, sources,
                                   background=background, **options)
    left, right = [Path(results[key]["bsp"]).read_bytes() for key in ("foreground", "background")]
    require(left == right, "Cubemap foreground/background BSP bytes differ")
    return results


def check_counts(receipt, sources, expanded, retained, depth=1, occurrences=1, root_samples=0):
    dependencies = [{"path": path.name, "bytes": path.stat().st_size, "sha256": digest(path)}
                    for path in sorted(sources)]
    for compiled in receipt.values():
        stats = compiled["result"]["stats"]
        usage = stats["instances"]
        require(usage.get("cubemaps", 0) == expanded and ("cubemaps" in usage) == bool(expanded)
                and usage["world_brushes"] == usage["detail_brushes"] == 0
                and usage["files"] == len(sources) and usage["instances"] == occurrences
                and usage["max_expansion_depth"] == depth and usage["dependencies"] == dependencies
                and stats["cubemap_samples"] == retained + root_samples,
                "Pre-region expanded cubemap usage was confused with retained global BSP samples")


def compare_compiler(actual, literal, expected):
    left, right = [Path(row["foreground"]["bsp"]).read_bytes() for row in (actual, literal)]
    native = records(left, 42, "<3iB3s")
    require(native == [(*point, 6, b"\0\0\0") for point in expected]
            and payload(left, 42) == payload(right, 42),
            "Transformed sample positions, native truncation, size enums or padding differ from literals")
    require(all(payload(left, index) == payload(right, index) for index in PHYSICAL_LUMPS),
            "Child samples changed physical geometry, visibility or LDR/HDR lighting arrays")
    a, b = [row["foreground"]["result"]["stats"] for row in (actual, literal)]
    require(all(a[key] == b[key] for key in COMPILE_STATS),
            "Child-sample compiler statistics differ from the independent literal control")
    require(b"env_cubemap" not in payload(left, 0) and b"func_instance" not in payload(left, 0),
            "Consumed sample/reference helpers escaped into runtime entities")
    return {"literal_samples": expected, "exact_sample_lump": 42,
            "exact_geometry_visibility_lighting_lumps": list(PHYSICAL_LUMPS)}


def geometry_cases(client, args, output, game, report):
    controls = {}
    for placement in ("translation", "nested", "fractional"):
        leaf = output / f"{placement}-child.vmf"
        helpers = child(client, leaf, SOURCE[placement], args.custody, sides="  ")
        reference, sources = leaf, [leaf]
        if placement == "nested":
            reference = output / "nested-middle.vmf"
            middle(client, reference, leaf, args.custody)
            sources.append(reference)
        control = parent(client, output / f"{placement}-parent.vmf", args.custody, reference, placement)
        case = report[placement] = {"authored": helpers, "parent_authoring": control["authoring"],
                                   "inspection": inspection(client, control, 2)}
        configure(client, "direct_ldr_hdr" if placement == "nested" else "fullbright")
        actual = compile_pair(client, args, output, game, placement, control, sources)
        check_counts(actual, sources, 2, 2, depth=2 if placement == "nested" else 1,
                     occurrences=2 if placement == "nested" else 1)
        if placement == "nested":
            controls = {"control": control, "sources": sources, "leaf": leaf, "helpers": helpers,
                        "prior": actual["background"]["bsp_sha256"]}
        literal = ((-96.875, -0.875, 64.875), (96.875, -0.875, 64.875)) if placement == "fractional" else LITERAL
        flat = parent(client, output / f"{placement}-flat.vmf", args.custody, literal=literal)
        configure(client, "direct_ldr_hdr" if placement == "nested" else "fullbright")
        flattened = compile_pair(client, args, output, game, f"{placement}-flat", flat, [])
        require("instances" not in flattened["foreground"]["result"]["stats"],
                "Independent literal control invented expanded instance usage")
        case.update({"actual": actual, "literal": flattened,
                     "comparison": compare_compiler(actual, flattened, LITERAL)})
    return controls


def omission_cases(client, args, output, game, report):
    defaults = output / "default0-child.vmf"
    helpers = child(client, defaults, SOURCE["translation"][:1], args.custody, size=None)
    control = parent(client, output / "default0-parent.vmf", args.custody, defaults)
    configure(client)
    result = compile_pair(client, args, output, game, "default0", control, [defaults])
    native = records(Path(result["foreground"]["bsp"]).read_bytes(), 42, "<3iB3s")
    require(native == [(*LITERAL[0], 0, b"\0\0\0")],
            "Untouched normal FGD size0 helper did not compile through ordinary sample collection")
    report["normal-fgd-default0"] = {"authored": helpers, "builds": result}
    leaf = output / "hidden-child.vmf"
    child(client, leaf, SOURCE["translation"], args.custody, hidden=True)
    control = parent(client, output / "hidden-parent.vmf", args.custody, leaf)
    inspected = inspection(client, control, 1)
    configure(client)
    result = compile_pair(client, args, output, game, "hidden", control, [leaf])
    check_counts(result, [leaf], 1, 1)
    report["hidden"] = {"inspection": inspected, "builds": result}
    region_leaf = output / "region-child.vmf"
    child(client, region_leaf, SOURCE["translation"], args.custody)
    control = parent(client, output / "region-parent.vmf", args.custody, region_leaf, region=True)
    inspected = inspection(client, control, 2)
    configure(client)
    result = compile_pair(client, args, output, game, "region", control, [region_leaf])
    check_counts(result, [region_leaf], 2, 1)
    report["region"] = {"inspection": inspected, "builds": result}

    mixed_leaf = output / "mixed-child.vmf"
    child(client, mixed_leaf, SOURCE["translation"], args.custody, mixed=True)
    control = parent(client, output / "mixed-parent.vmf", args.custody, mixed_leaf)
    mixed = inspection(client, control, 2, mixed=True)["contents"]
    reviewed_open(client, mixed_leaf)
    helper_ids = [row["id"] for row in client.call("find_objects", {"classname": "env_cubemap"})["objects"]]
    client.call("delete_objects", {"ids": helper_ids})
    physical = output / "physical-only-child.vmf"
    save(client, physical, args.custody)
    control = parent(client, output / "physical-only-parent.vmf", args.custody, physical)
    baseline = inspection(client, control, 0, mixed=True)["contents"]
    require(all(mixed["geometry"][key] == baseline["geometry"][key] for key in GEOMETRY_FIELDS)
            and mixed["face_provenance_total"] == baseline["face_provenance_total"]
            and [row["bounds"] for row in mixed["owners"]] == [row["bounds"] for row in baseline["owners"]],
            "Consumed samples changed mixed physical reference geometry/provenance/bounds")
    report["mixed"] = {"with_samples": mixed, "physical_only": baseline}


def refusal_cases(client, args, output, game, report):
    leaf = output / "manual-child.vmf"
    helpers = child(client, leaf, SOURCE["translation"], args.custody, sides="1")
    control = parent(client, output / "manual-parent.vmf", args.custody, leaf)
    configure(client)
    report["manual-child-sides"] = compile_pair(client, args, output, game, "manual-sides", control, [leaf],
        failure="sides", original_ids=(helpers[0]["id"],), admission=True)

    leaf = output / "size9-child.vmf"
    helpers = child(client, leaf, SOURCE["translation"][:1], args.custody, size=9)
    control = parent(client, output / "size9-parent.vmf", args.custody, leaf)
    configure(client)
    ordinary = compile_pair(client, args, output, game, "size9-sample-only", control, [leaf])
    require(records(Path(ordinary["foreground"]["bsp"]).read_bytes(), 42, "<3iB3s")[0][3] == 9,
            "Ordinary sample-only compilation lost its supported size9 enum")
    configure(client, "direct_ldr", capture=True)
    refused = compile_pair(client, args, output, game, "size9-capture", control, [leaf],
        failure="cube edges up to 128", original_ids=(helpers[0]["id"],))
    report["size9"] = {"sample_only": ordinary, "capture_refusal": refused}

    leaf = output / "count32-child.vmf"
    helpers = child(client, leaf, [(index * 4 - 120, 0, 64) for index in range(32)], args.custody)
    control = parent(client, output / "count33-parent.vmf", args.custody, leaf,
                     placement="identity", roots=((-128, 0, 64),))
    configure(client)
    ordinary = compile_pair(client, args, output, game, "count33-sample-only", control, [leaf])
    check_counts(ordinary, [leaf], 32, 32, root_samples=1)
    configure(client, "direct_ldr", capture=True)
    refused = compile_pair(client, args, output, game, "count33-capture", control, [leaf],
        failure="1–32", original_ids=(helpers[-1]["id"],))
    report["root-plus32"] = {"sample_only": ordinary, "capture_refusal": refused}

    leaf = output / "collision-child.vmf"
    helpers = child(client, leaf, ((-112.125, 7.875, 32.125), (-112.875, 7.125, 32.875)), args.custody)
    control = parent(client, output / "collision-parent.vmf", args.custody, leaf)
    configure(client)
    ordinary = compile_pair(client, args, output, game, "collision-sample-only", control, [leaf])
    native = records(Path(ordinary["foreground"]["bsp"]).read_bytes(), 42, "<3iB3s")
    require([row[:3] for row in native] == [LITERAL[0], LITERAL[0]],
            "Fractional source controls did not actually collide after native integer truncation")
    configure(client, "direct_ldr", capture=True)
    refused = compile_pair(client, args, output, game, "collision-capture", control, [leaf],
        failure="quantization", original_ids=tuple(row["id"] for row in helpers))
    report["quantized-collision"] = {"sample_only": ordinary, "capture_refusal": refused}

    leaf = output / "nonfinite-child.vmf"
    helpers = child(client, leaf, SOURCE["translation"][:1], args.custody)
    text = leaf.read_text()
    entities = fixture_blocks(text, "entity")
    require(len(entities) == 1, "Nonfinite import must contain one original helper")
    changed, total = re.subn(r'"origin"\s+"[^"]*"', '"origin" "NaN 8 32"', entities[0], count=1)
    require(total == 1, "Nonfinite imported fixture did not change one origin")
    leaf.write_text(text.replace(entities[0], changed))
    watch(args.custody, (leaf,))
    control = parent(client, output / "nonfinite-parent.vmf", args.custody, leaf)
    configure(client)
    report["nonfinite-source"] = compile_pair(client, args, output, game, "nonfinite", control, [leaf],
        failure="origin", original_ids=(helpers[0]["id"],), admission=True)


def dependency_case(client, args, output, game, controls, report):
    control, leaf, sources = controls["control"], controls["leaf"], controls["sources"]
    reviewed_open(client, control["source"])
    configure(client, "direct_ldr_hdr")
    pair = compile_pair(client, args, output, game, "before-drift", control, sources)
    inspection(client, control, 2)
    before, original = snapshot(client), leaf.read_bytes()
    outputs = {path: identity(Path(path)) for path in args.custody
               if Path(path).is_relative_to(output) and Path(path).suffix in (".bsp", ".lin")}
    try:
        leaf.write_bytes(original + b"\n")
        stale = client.call("get_compile_status")
        ready = client.call("inspect_instance_contents")
        require(stale["job"]["stale"] and not stale["job"]["instance_dependencies"]["current"]
                and not stale["launch"]["ready"] and ready["state"] == "stale"
                and "cubemap_preview" not in ready and not ready["available"],
                "Transitive sample drift did not stale retained publication and reference reports")
        require(snapshot(client) == before, "Transitive staleness inspection changed Session state")
        leaf.unlink()
        failed = compile_pair(client, args, output, game, "missing-transitive", control, sources,
                              failure=leaf.name)
        report.update({"stale": stale, "stale_contents": ready, "missing_refusal": failed})
    finally:
        leaf.write_bytes(original)
        watch(args.custody, (leaf,))
    require({path: identity(Path(path)) for path in outputs} == outputs,
            "Transitive drift changed previously retained BSP/LIN artifacts")
    repaired = compile_pair(client, args, output, game, "repaired", control, sources)
    require(repaired["background"]["bsp_sha256"] == pair["background"]["bsp_sha256"] == controls["prior"],
            "Repaired transitive sample source did not restore the exact original BSP")
    report["repaired"] = repaired


def run_observer(args, output, action):
    require(digest(args.native_observer) == args.native_observer_sha256,
            "Reviewed private native observer changed")
    command = [sys.executable, "-B", str(args.native_observer), "--output", str(output), *action]
    result = subprocess.run(command, capture_output=True, text=True, timeout=args.timeout,
                            env={**os.environ, "PYTHONDONTWRITEBYTECODE": "1", "PYTHONPATH": ""})
    output.with_suffix(".stdout.log").write_text(result.stdout)
    output.with_suffix(".stderr.log").write_text(result.stderr)
    try:
        result.check_returncode()
    except subprocess.CalledProcessError as error:
        raise RuntimeError(f"Native observer {action[0]} failed; inspect {output.with_suffix('.stderr.log')}: "
                           f"{result.stderr[-4096:]}") from error
    require(digest(args.native_observer) == args.native_observer_sha256,
            "Private native observer changed during observation")
    report = json.loads(output.read_text())
    require(report["success"], "Native observer did not qualify fresh747 evidence")
    return report


def native_pair(client, args, output, game, controls, channel, background, report):
    leaf, control, sources = controls["leaf"], controls["control"], controls["sources"]
    prefix = "747-ldr" if channel == "ldr" else "747-hdr"
    observations = {}
    for role in ("child", "flat"):
        if role == "child":
            reviewed_open(client, control["source"])
            client.call("select", {"ids": control["owners"]})
            current, dependencies = control, sources
            report["inspection"] = inspection(client, control, 2)
        else:
            current = parent(client, output / f"{prefix}-flat.vmf", args.custody, literal=LITERAL)
            dependencies = []
        configure(client, "direct_ldr" if channel == "ldr" else "direct_ldr_hdr", capture=True)
        token = client.call("begin_property_edit", {"world": True})["draft"]["token"]
        client.call("update_property_edit", {"token": token, "changes": [
            {"action": "set", "key": "message", "value": "Retain this cubemap capture review"}]})
        try:
            built = compile_one(client, args, output, game, f"{prefix}-{role}", current, dependencies,
                                background=background)
            if role == "child":
                usage = built["result"]["stats"]["instances"]
                expected = [{"path": path.name, "bytes": path.stat().st_size, "sha256": digest(path)}
                            for path in sorted(dependencies)]
                require(usage["cubemaps"] == 2 and usage["world_brushes"] == usage["detail_brushes"] == 0
                        and usage["max_expansion_depth"] == usage["instances"] == usage["files"] == 2
                        and usage["dependencies"] == expected,
                        "Native nested sample capture lost helper-only expanded provenance")
            compile_receipt = output / f"{prefix}-{role}-compile.json"
            write_json(compile_receipt, built["result"])
            observation = output / f"{prefix}-{role}-observation.json"
            report[role] = {"build": built, "observation_path": str(observation)}
            observed = run_observer(args, observation, ["observe", "--bsp", built["bsp"],
                "--compile-result", str(compile_receipt), "--channel", channel])
            report[role]["observation"] = observed
            evidence = observed["capture"]
            for capture in evidence.get("channels", {"ldr": evidence}).values():
                directory = Path(capture["directory"])
                for name, expected in capture["artifact_sha256"].items():
                    path = directory / name
                    require(digest(path) == expected, "Fresh native evidence changed after observation")
                    watch(args.custody, (path,))
            observations[role] = observation
        finally:
            client.call("cancel_property_edit", {"token": token})
    report["parity"] = run_observer(args, output / f"{prefix}-parity.json", ["compare",
        "--actual", str(observations["child"]), "--literal", str(observations["flat"])])
    require(digest(leaf) == args.custody[str(leaf)]["sha256"], "Native observation changed child source")


def audit_program(args, game, material_before, output, report):
    actual = {path: identity(Path(path)) for path in args.custody if Path(path).is_relative_to(output)}
    expected = {path: value for path, value in args.custody.items() if Path(path).is_relative_to(output)}
    report["custody_after_eof"] = actual
    require(actual == expected and identities(game / "materials") == material_before,
            "Post-EOF private source/BSP/LIN/material custody differs")


def run_program(binary, args, output, native_channel, report):
    output.mkdir()
    temporary = Path(tempfile.mkdtemp(prefix="n747-", dir=ROOT / "target"))
    args.native_tmpdir = temporary
    maximum_socket = temporary / ("hammerr_capture_" + "0" * 16) / "wayland/wayland-0"
    require(len(os.fsencode(maximum_socket)) < 108 and str(temporary) == str(temporary).lower(),
            "Native TMPDIR must be lowercase and leave room for the complete Wayland sun_path")
    report.update({"binary": str(binary), "binary_sha256": digest(binary), "native_tmpdir": str(temporary),
                   "maximum_native_socket_bytes": len(os.fsencode(maximum_socket)),
                   "compiler": {}, "omission": {}, "refusals": {}, "dependency": {}, "native": {}})
    with ExitStack() as lifecycle:
        lifecycle.enter_context(record_failure(args.errors))
        game = private_game(args.game_dir, output / "source-game", engine_physics=True)
        material_fixture(game)
        tools = game / "materials/tools"
        tools.mkdir(exist_ok=True)
        (tools / "toolsblack.vmt").write_text('UnlitGeneric { $basetexture "tools/toolsblack" }\n')
        (tools / "toolsblack.vtf").write_bytes(fixture_texture(2, 2, (0, 0, 0), (0, 0, 0, 255)))
        material_before = identities(game / "materials")
        watch(args.custody, [game / "gameinfo.txt",
                            *(path for path in (game / "cfg").rglob("*") if path.is_file()),
                            *(path for path in (game / "scripts").rglob("*") if path.is_file())])
        lifecycle.callback(audit_program, args, game, material_before, output, report)
        lifecycle.enter_context(record_failure(args.errors))
        client_output = output / "client"
        client_output.mkdir()
        environment = {"TMPDIR": str(temporary), "HAMMERR_RECENT_MAPS_DIR": str(output / "recent"),
                       "HAMMERR_BUILD_RECIPES": str(output / "recipes.json"),
                       "HAMMERR_PREFABS_DIR": str(output / "prefabs"),
                       "XDG_CONFIG_HOME": str(output / "config"), "XDG_STATE_HOME": str(output / "state"),
                       "XDG_CACHE_HOME": str(output / "cache")}
        client = Client(None, binary, client_output, game / "gameinfo.txt",
                        environment=environment, request_timeout=args.timeout)
        report["mcp_pid"] = client.process.pid
        lifecycle.callback(close_client, client, report, args.timeout)
        lifecycle.enter_context(record_failure(args.errors))
        process_stat = Path(f"/proc/{client.process.pid}/stat").read_text()
        report["mcp_start"] = process_stat[process_stat.rindex(")") + 2:].split()[19]
        definitions = client.call("load_entity_definitions", {"path": str(args.fgd)})
        for path in definitions["sources"]:
            actual = identity(Path(path))
            if path in args.custody:
                require(actual == args.custody[path], "Original FGD identity changed before another Session loaded it")
            else:
                args.custody[path] = actual
        report["fgd"] = client.call("get_entity_class", {"classname": "env_cubemap"})
        report["native_runtime"] = client.call("set_native_capture_runtime", {
            "engine_root": str(args.game_dir),
            **({"steam_root": str(args.steam_root)} if args.steam_root else {}),
        })
        require(report["native_runtime"]["ready"], "Installed native capture runtime is not ready")
        controls = geometry_cases(client, args, output, game, report["compiler"])
        omission_cases(client, args, output, game, report["omission"])
        refusal_cases(client, args, output, game, report["refusals"])
        dependency_case(client, args, output, game, controls, report["dependency"])
        if args.native_observer:
            native_pair(client, args, output, game, controls, native_channel,
                        background=native_channel == "ldr_hdr", report=report["native"])


def native_processes(report):
    result = []
    for program in report["programs"]:
        if "native_tmpdir" not in program:
            continue
        for path in Path(program["native_tmpdir"]).glob("hammerr_capture_*/native-process.json"):
            require(path.stat().st_size <= 64 * 1024, "Oversized owned native process receipt")
            process = json.loads(path.read_text())
            result.append({**process, "directory": str(path.parent)})
    return result


def assert_drained(report):
    owned = native_processes(report)
    sessions = {(int(row["uid"]), int(row["session"])) for row in owned}
    uids = {uid for uid, _session in sessions}
    live = []
    for process in Path("/proc").iterdir():
        if not process.name.isdigit():
            continue
        try:
            uid = process.stat().st_uid
            if uid not in uids:
                continue
            raw = (process / "stat").read_text()
        except (FileNotFoundError, ProcessLookupError):
            continue
        fields = raw[raw.rindex(")") + 2:].split()
        if (uid, int(fields[3])) in sessions:
            live.append({"pid": int(process.name), "session": int(fields[3]), "state": fields[0]})
    require(not live, f"Owned native sessions survived all MCP EOF drains: {live}")
    for program in report["programs"]:
        if "mcp_start" not in program:
            continue
        try:
            raw = Path(f"/proc/{program['mcp_pid']}/stat").read_text()
        except (FileNotFoundError, ProcessLookupError):
            continue
        require(raw[raw.rindex(")") + 2:].split()[19] != program["mcp_start"],
                "A private production MCP process survived EOF")
    report["owned_native_sessions_drained"] = True
    report["native_capture_directories"] = [row["directory"] for row in owned]
    if report["native_requested"]:
        require(len(owned) == len({(row["pid"], row["start"]) for row in owned}) == 6,
                "Expected exactly six distinct fresh native channel processes")
    else:
        require(not owned, "Compiler-only validation unexpectedly launched native engines")
    report["fresh_native_channel_processes"] = len(owned)


def audit_global(args, report, installed):
    errors = []
    actual = {path: identity(Path(path)) for path in args.custody}
    report["global_custody_after_eof"] = actual
    if actual != args.custody:
        errors.append(AssertionError("Original FGD/binary/helper/source/output custody changed after EOF"))
    current = installed_files(args)
    report["installed_after_eof"] = current
    if current != installed:
        errors.append(AssertionError("Installed runtime configuration/assets changed"))
    if errors:
        raise ExceptionGroup("Instanced cubemap post-EOF custody failed", errors)


def audit_cloud(args, report):
    directories = [process["directory"] for process in native_processes(report)]
    if args.native_observer and directories:
        command = ["audit"]
        for directory in directories:
            command.extend(("--capture-dir", directory))
        report["post_eof_config_cloud"] = run_observer(args, args.output / "post-eof-config-cloud.json", command)


def installed_files(args):
    paths = [args.game_dir / "bin/linux64" / name for name in ("engine.so", "vtex_dll.so", "bsppack.so")]
    paths.extend(path for path in (args.game_dir / "cstrike/cfg").rglob("*") if path.is_file())
    for name in NATIVE_NAMES:
        paths.append(args.game_dir / "cstrike/maps" / f"{name}.bsp")
        for prefix in (args.game_dir, args.game_dir / "cstrike"):
            for folder in ("materials/maps", "materialsrc/maps"):
                paths.extend(path for path in (prefix / folder / name).rglob("*") if path.is_file())
    return {str(path): identity(path) for path in sorted(set(paths))}


def finish(args, report):
    report["success"] = not args.errors
    report["compile_calls"] = args.compile_calls
    report["errors"] = [failure_details(error) for error in args.errors]
    write_json(args.output / "results.json", report)
    print(json.dumps({"success": report["success"], "output": str(args.output),
                      "compile_calls": args.compile_calls,
                      "fresh_native_channel_processes": report.get("fresh_native_channel_processes", 0)}))
    if args.errors:
        raise BaseExceptionGroup("Instanced cubemap validation failed", args.errors)


def main():
    parser = argparse.ArgumentParser(
        description="Validate automatic child env_cubemap helpers through both private production stdio MCP entrypoints, including foreground/background compilation, literal transforms, sample-only limits/refusals, CPU inspection and source/output/EOF custody. With a reviewed --native-observer, additionally run six fresh native channels: paired child/literal LDR foreground in headless and paired child/literal LDR+HDR background in editor.",
        epilog="Requires Python3.11+, matching production editor/hammerr-headless/native-runtime binaries, installed CS:S and its SDK FGD. Native mode also requires synchronized Steam Cloud and the separately reviewed pinned730 observer adapter with its exact hash. All game/recents/recipes/config/recovery and lowercase short native TMPDIR artifacts are private under target/. Production capture uses -insecure/sv_lan1 and no desktop focus/input; no normal Play/editor rendering or synthetic backend. Run under the caller's single eight-job pipeline and aggregate24GiB cap. Decoded directional patches retain730 thresholds and exact native-to-publication payload checks. No radiometric identity, runtime speed, arbitrary manual child bindings or in-flight cancellation/drift coverage is claimed; finalizer race tests remain separate. Root must independently verify its enclosing cgroup drains after this Python process exits.",
    )
    parser.add_argument("--binary", type=Path, action="append", required=True,
                        help="Specify both actual production executables: hammerr-headless and editor")
    parser.add_argument("--fgd", type=Path, default=DEFAULT_FGD)
    parser.add_argument("--game-dir", type=Path,
                        default=Path.home() / ".local/share/Steam/steamapps/common/Counter-Strike Source")
    parser.add_argument("--steam-root", type=Path)
    parser.add_argument("--output", type=Path, required=True, help="Fresh private artifact directory under repository target/")
    parser.add_argument("--timeout", type=int, default=600, help="Bound each MCP request/job/observer at60–600seconds")
    parser.add_argument("--native-observer", type=Path,
                        help="Reviewed analysis-only private747 adapter; omission runs compiler-only checks")
    parser.add_argument("--native-observer-sha256", help="Exact reviewed adapter hash; required with --native-observer")
    args = parser.parse_args()
    args.binary = [path.resolve(strict=True) for path in args.binary]
    if len(args.binary) != 2 or {path.name for path in args.binary} != {"editor", "hammerr-headless"}:
        parser.error("Specify each actual production executable once")
    args.binary.sort(key=lambda path: path.name != "hammerr-headless")
    args.fgd, args.game_dir = args.fgd.resolve(strict=True), args.game_dir.resolve(strict=True)
    if args.steam_root:
        args.steam_root = args.steam_root.resolve(strict=True)
    if not 60 <= args.timeout <= 600 or os.environ.get("LD_PRELOAD"):
        parser.error("Use a bounded60–600second timeout without inherited LD_PRELOAD")
    if bool(args.native_observer) != bool(args.native_observer_sha256):
        parser.error("--native-observer and its exact reviewed --native-observer-sha256 are required together")
    if args.native_observer:
        args.native_observer = args.native_observer.resolve(strict=True)
        if digest(args.native_observer) != args.native_observer_sha256:
            parser.error("Reviewed private observer hash differs")
    if args.output.exists() or args.output.is_symlink():
        parser.error("--output must be fresh")
    args.output = args.output.resolve()
    if not args.output.is_relative_to(ROOT / "target"):
        parser.error("--output must stay on disk under repository target/")
    args.output.mkdir(parents=True, mode=0o700)
    args.compile_calls, args.custody, args.errors = 0, {}, []
    watch(args.custody, [*args.binary, *(path.parent / "hammerr-native-runtime" for path in args.binary), args.fgd])
    if args.native_observer:
        watch(args.custody, [args.native_observer])
    installed = installed_files(args)
    report = {"success": False, "transport": "private_stdio", "script_sha256": digest(Path(__file__)),
              "native_requested": bool(args.native_observer), "desktop_input": False,
              "ordinary_play_exercised": False, "editor_rendering_exercised": False,
              "radiometric_identity_proven": False, "in_flight_dependency_or_cancellation_tested": False,
              "native_matrix": {"headless": "child/literal LDR foreground",
                                "editor": "child/literal LDR+HDR background"},
              "literal_transform": {"translation": "(x+16,y-8,z+32)",
                                    "nested": "(z+24,x-72,y+144)"},
              "installed_before": installed, "programs": []}
    with ExitStack() as lifecycle:
        lifecycle.callback(finish, args, report)
        lifecycle.enter_context(record_failure(args.errors))
        lifecycle.callback(audit_global, args, report, installed)
        lifecycle.enter_context(record_failure(args.errors))
        lifecycle.callback(assert_drained, report)
        lifecycle.enter_context(record_failure(args.errors))
        lifecycle.callback(audit_cloud, args, report)
        lifecycle.enter_context(record_failure(args.errors))
        for binary, channel in zip(args.binary, ("ldr", "ldr_hdr"), strict=True):
            program = {}
            report["programs"].append(program)
            run_program(binary, args, args.output / binary.name, channel, program)


if __name__ == "__main__":
    main()
