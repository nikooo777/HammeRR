import argparse
import hashlib
import json
import os
import time
from contextlib import ExitStack
from pathlib import Path

from validate_authored_planes import records
from validate_collision_topology import (
    digest,
    document_state,
    failure_details,
    identity,
    record_failure,
)
from validate_compile_jobs import ACTIVE_JOB_STATUSES
from validate_compile_regions import apply as apply_region
from validate_face_lighting import DEFAULT_BINARY, ROOT, Client, detail
from validate_instance_contents import refresh
from validate_material_albedo import fixture_texture, identities, payload, require
from validate_nested_brush_instances import assets, fixture_blocks, room
from validate_recovery_exit import require_clean_eof
from validate_visibility import bit, cluster_at, visibility

MATERIAL = "hammerr690/world"
PREVIOUS_BSP = b"private previous BSP\n"
PREVIOUS_LIN = b"13 17 19\n23 29 31\n"
PREVIEW_REASON = (
    "Child visibility helpers have no reference geometry or icons; "
    "helper bounds, picking and framing are unavailable."
)
GROUP = ([-250, -250, 8], [110, 250, 248])
LITERAL_MARKER = [4096, 0, 64]
SOURCE_BOUNDS = {
    "translation": ([-266, -242, -24], [94, 258, 216]),
    "noncommuting": ([-178, -136, -274], [322, 104, 86]),
}
POSES = {
    "translation": ([16, -8, 32], "0 0 0"),
    "noncommuting": ([16, -8, 128], "0 0 90"),
}
PHYSICAL_LUMPS = (1, 2, 3, 5, 6, 7, 8, 10, 12, 13, 14, 16, 17, 18, 19,
                  26, 27, 30, 31, 48, 51, 52, 53, 55, 56, 58)
PHYSICAL_STATS = (
    "brushes", "nodes", "leaves", "faces", "models", "clusters", "visibility_bytes",
    "visibility_portals", "visibility_solved_clusters", "viscluster_helpers",
    "viscluster_groups", "viscluster_grouped_leaves", "lighting_mode", "visibility_mode", "hdr",
)
GEOMETRY_FIELDS = ("vertices", "indices", "triangles", "line_vertices")
DEFAULT_FGD = (Path.home() / ".local/share/Steam/steamapps/common/SourceSDK"
               / "bin/source2009/bin/cstrike.fgd")


def reviewed_open(client, path):
    review = client.call("get_map_info")["replacement_review"]
    client.call("open_map", {"path": str(path), "expected_review": review})


def new_map(client):
    review = client.call("get_map_info")["replacement_review"]
    client.call("new_map", {"expected_review": review})


def save(client, path, custody):
    generation = client.call("get_map_info")["generation"]
    client.call("save_map", {"path": str(path), "expected_generation": generation})
    before = identity(path)
    reviewed_open(client, path)
    require(identity(path) == before, "Reopening a saved fixture changed its source identity")
    for suffix, data in ((".bsp", PREVIOUS_BSP), (".lin", PREVIOUS_LIN)):
        sibling = path.with_suffix(suffix)
        sibling.write_bytes(data)
        custody[str(sibling)] = identity(sibling)
    custody[str(path)] = before


def block(client, bounds, material=MATERIAL):
    return client.call("create_block", {
        "mins": bounds[0], "maxs": bounds[1], "material": material,
    })["id"]


def helper(client, bounds, marker=None, hidden_solid=False):
    brush = block(client, bounds, "tools/toolstrigger")
    shape = detail(client, [brush])[0]
    if hidden_solid:
        client.call("set_object_visibility", {"action": "hide", "ids": [brush]})
    owner = client.call("tie_to_entity", {
        "ids": [brush], "classname": "func_viscluster",
    })["id"]
    authored = dict(detail(client, [owner])[0]["properties"])
    definition = client.call("get_entity_class", {"classname": "func_viscluster"})
    require(authored == {"classname": "func_viscluster", **definition["defaults"]},
            "Normal FGD brush/tie authoring changed defaults or synthesized extra helper properties")
    if marker is not None:
        client.call("set_entity_properties", {
            "id": owner, "set": {"origin": " ".join(map(str, marker))},
        })
    return {"owner": owner, "solid": shape["solid"], "defaults": authored}


def leaf(client, path, bounds, custody, marker=None, mixed=False, hidden=None):
    new_map(client)
    authored = helper(client, bounds, marker, hidden_solid=hidden == "solid")
    if mixed:
        block(client, ([-32, -32, 0], [-16, -16, 16]))
        physical = block(client, ([16, 16, 0], [32, 32, 16]))
        client.call("tie_to_entity", {"ids": [physical], "classname": "func_detail"})
    if hidden == "entity":
        client.call("set_object_visibility", {"action": "hide", "ids": [authored["owner"]]})
    save(client, path, custody)
    return authored


def instance(client, path, origin=(0, 0, 0), angles="0 0 0"):
    return client.call("create_entity", {
        "classname": "func_instance", "origin": origin,
        "properties": {"file": path.name, "angles": angles},
    })["id"]


def authored_instance(client, path, origin, angles):
    form = client.call("set_instance_reference", {"action": "begin"})
    token = form["token"]
    client.call("set_instance_reference", {
        "action": "update", "token": token, "file": path.name,
        "origin": " ".join(map(str, origin)), "angles": angles,
    })
    checked = client.call("set_instance_reference", {"action": "check", "token": token})
    require(checked["valid"] and checked["presence_checked"]
            and checked["contents_validation"] == "not_checked",
            "Ordinary instance authoring did not preserve its metadata-only admission contract")
    created = client.call("set_instance_reference", {"action": "apply", "token": token})
    return created["id"], {"checked": checked, "applied": created}


def middle(client, path, child, custody):
    new_map(client)
    instance(client, child, [8, 16, 64], "90 0 0")
    save(client, path, custody)


def physical_room(client):
    identifiers = room(client)
    client.call("set_entity_properties", {"id": identifiers[-1], "set": {"origin": "-160 -96 192"}})
    identifiers.append(client.call("create_entity", {
        "classname": "info_player_counterterrorist", "origin": [200, 0, 8],
    })["id"])
    for bounds in (([-72, -256, 0], [-56, -64, 256]),
                   ([-72, 64, 0], [-56, 256, 256]),
                   ([120, -256, 0], [136, 256, 256])):
        identifiers.append(block(client, bounds))
    return identifiers


def parent(client, path, custody, child=None, placement="translation", flat=False,
           region=False, repeated=1, root_helper=False, grouped=True):
    new_map(client)
    identifiers = physical_room(client)
    owners = []
    authored = None
    if flat:
        authored = helper(client, GROUP, LITERAL_MARKER if placement == "noncommuting" else None)
        identifiers.append(authored["owner"])
    elif child is not None:
        origin, angles = POSES[placement] if grouped else ([0, 0, 0], "0 0 0")
        if grouped and repeated == 1:
            client.call("save_map", {"path": str(path),
                                      "expected_generation": client.call("get_map_info")["generation"]})
        for index in range(repeated):
            shifted = [origin[0] + index * 24, origin[1], origin[2]]
            if grouped and repeated == 1:
                owner, authored = authored_instance(client, child, shifted, angles)
            else:
                owner = instance(client, child, shifted, angles)
            owners.append(owner)
        identifiers.extend(owners)
    if root_helper:
        identifiers.append(helper(client, ([900, 900, 900], [910, 910, 910]))["owner"])
    if region:
        apply_region(client, low=[-254, -254, 4], high=[254, 254, 252])
    save(client, path, custody)
    client.call("select", {"ids": owners or identifiers[:1]})
    return {"source": path, "owners": owners, "ids": identifiers, "authoring": authored}


def reference_form(client, control):
    before = document_state(client)
    owner = control["owners"][0]
    fields = dict(detail(client, [owner])[0]["properties"])
    state = client.call("set_instance_reference", {"action": "begin", "id": owner})
    token = state["token"]
    client.call("set_instance_reference", {
        "action": "update", "token": token, "file": fields["file"],
        "origin": fields["origin"], "angles": fields["angles"],
    })
    checked = client.call("set_instance_reference", {"action": "check", "token": token})
    require(checked["valid"] and checked["contents_validation"] == "not_checked",
            "Reference authoring claimed full helper-content validation")
    client.call("set_instance_reference", {"action": "cancel", "token": token})
    require(document_state(client) == before, "Reference authoring changed the saved map/history/selection")
    return checked


def preview(client, control, count, mixed=False):
    before = document_state(client)
    ready = refresh(client, background=True)
    require(ready["state"] == "ready" and ready["stats"].get("visclusters", 0) == count,
            "Ready reference report has incorrect expanded helper usage")
    require(("visclusters" in ready["stats"]) == bool(count)
            and ("viscluster_preview" in ready) == bool(count),
            "Zero helper usage or its unavailable-preview field was not omitted")
    if count:
        require(ready["viscluster_preview"] == {"available": False, "reason": PREVIEW_REASON},
                "Visibility helpers did not explicitly report unavailable geometry/picking/framing")
    if mixed:
        require(ready["available"] and ready["stats"]["world_brushes"] == 1
                and ready["stats"]["detail_brushes"] == 1
                and ready["geometry"]["triangles"] > 0,
                "Mixed child helpers were counted as physical world/detail reference geometry")
    else:
        require(not ready["available"] and not ready["displayed"]
                and ready["stats"]["world_brushes"] == ready["stats"]["detail_brushes"] == 0
                and all(ready["geometry"][key] == 0 for key in GEOMETRY_FIELDS)
                and ready["face_provenance_total"] == 0
                and all(row["bounds"] is None for row in ready["owners"]),
                "Helper-only references fabricated physical meshes, provenance or bounds")
        client.call("set_instance_contents", {
            "action": "frame", "token": str(ready["token"]), "ids": control["owners"],
        }, error=True)
        pick = {"x": 256, "y": 256, "width": 513, "height": 513,
                "camera": {"projection": "top", "center": [-160, 100, 64], "ortho_height": 64}}
        ordinary = client.call("pick_at_pixel", {**pick, "instance_contents": False})
        referenced = client.call("pick_at_pixel", {**pick, "instance_contents": True})
        require(ordinary == referenced and referenced["object"] not in control["owners"]
                and referenced["solid"] is not None and referenced["side"] is not None,
                "Consumed helper geometry became a CPU pick surface in front of the ordinary floor")
    require({row["owner"] for row in ready["owners"]} == set(control["owners"]),
            "Reference report lost a root owner")
    require(document_state(client) == before and not client.call("get_map_info")["rendering"]["initialized"],
            "CPU reference inspection/framing changed the document or initialized graphics")
    return ready


def configure(client, mode):
    client.call("set_compile_settings", {
        "lighting": "fullbright" if mode == "fast" else "direct_ldr_hdr",
        "visibility": mode, "sky_quality": "fast", "physics": "none", "bounces": 0,
        "generate_reflections": False, "bake_static_prop_vertices": False,
        "pack_sky_assets": False, "pack_water_assets": False, "pack_static_prop_assets": False,
    })
    name = f"Visibility helpers {mode}"
    if not any(row["name"] == name for row in client.call("get_build_recipes")["recipes"]):
        client.call("save_build_recipe", {"name": name})


def wait_job(client, timeout):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        job = client.call("get_compile_status")["job"]
        if job["status"] not in ACTIVE_JOB_STATUSES:
            return job
        time.sleep(0.02)
    raise TimeoutError("Private visibility-helper compile did not finish")


def decoded_error(response):
    return json.loads(next(row["text"] for row in response["content"] if row["type"] == "text"))


def pair(client, args, game, label, control, sources, report, failure=None, child_ids=(),
         admission=False):
    report.update({"source": str(control["source"]), "dependencies": [str(path) for path in sources]})
    before = document_state(client)
    recipe = Path(client.call("get_build_recipes")["path"])
    protected_paths = [control["source"], recipe, *sources]
    protected = {str(path): identity(path) for path in protected_paths}
    mounted = identities(game)
    data = []
    for background in (False, True):
        name = "background" if background else "foreground"
        target = args.output / "builds" / f"{label}-{name}.bsp"
        target.write_bytes(PREVIOUS_BSP)
        target.with_suffix(".lin").write_bytes(PREVIOUS_LIN)
        previous = identity(target)
        for path in (target, target.with_suffix(".lin")):
            args.custody[str(path)] = identity(path)
        arguments = {"path": str(target), "expected_generation": before["document"]["generation"]}
        args.compile_calls += 1
        if background:
            started = client.call("start_compile", arguments)["job"]
            job = wait_job(client, args.timeout)
            report[name] = job
            require(job["token"] == started["token"] and job["native_runtime"] is None,
                    "Background job lost ownership or exercised native runtime")
            require(job["status"] == ("failed" if failure else "completed"),
                    f"Unexpected visibility-helper job status: {job}")
            compiled = job["result"]
            require(compiled["published"] == (failure is None), "Background publication flag differs from the outcome")
        else:
            response = client.call("compile_map", arguments, error=failure is not None)
            compiled = decoded_error(response) if failure else response
            report[name] = compiled
            require(compiled["game"] is None, "Foreground compile exercised a game")
        require(compiled["success"] == (failure is None), f"Unexpected compile outcome: {compiled}")
        if failure:
            errors = [row for row in compiled["diagnostics"] if row["level"] == "error"]
            require(any(failure in row["message"] for row in errors),
                    f"Missing contextual helper refusal {failure!r}: {errors}")
            for original in child_ids:
                source_id = (f"Child viscluster {original.split(':')[-1]}" if admission
                             else f"child object {original.split(':')[-1]}")
                require(any(row["object"] in control["owners"] and sources[0].name in row["message"]
                            and source_id in row["message"]
                            and failure in row["message"] for row in errors),
                        f"Helper refusal lost root owner, leaf filename or original child {original}: {errors}")
            require(identity(target) == previous, "Refused compile replaced or touched the previous BSP")
        else:
            require(compiled.get("leak") is None and target.read_bytes()[:4] == b"VBSP",
                    f"Successful compile leaked or produced no BSP: {compiled}")
        require(identity(target.with_suffix(".lin")) == args.custody[str(target.with_suffix(".lin"))],
                "Compile/refusal replaced or touched the previous pointfile")
        require(document_state(client) == before and identities(game) == mounted
                and {str(path): identity(path) for path in protected_paths} == protected,
                "Compile changed source dependencies, document/history/selection/settings or mounted fixtures")
        require(not client.call("get_map_info")["rendering"]["initialized"], "Compile initialized graphics")
        args.custody[str(target)] = identity(target)
        data.append(target.read_bytes())
    require(data[0] == data[1], "Foreground/background BSP bytes differ")
    report["bsp_sha256"] = hashlib.sha256(data[0]).hexdigest()
    return data[0]


def expanded_counts(receipt, sources, count, depth=1, occurrences=1, retained=None):
    expected = {"instances": occurrences, "files": len(sources), "max_expansion_depth": depth,
                "world_brushes": 0, "detail_brushes": 0}
    dependencies = [{"path": path.name, "bytes": path.stat().st_size, "sha256": digest(path)}
                    for path in sorted(sources)]
    for result in (receipt["foreground"], receipt["background"]["result"]):
        stats = result["stats"]["instances"]
        require(all(stats[key] == value for key, value in expected.items())
                and stats.get("visclusters", 0) == count and ("visclusters" in stats) == bool(count)
                and stats["dependencies"] == dependencies,
                "Expanded helper counts/dependencies differ before compile-region filtering")
        if retained is not None:
            require(result["stats"]["viscluster_helpers"] == retained,
                    "Retained compiler helper count was confused with expanded instance usage")


def comparison(actual, literal, actual_stats, literal_stats, grouped=True):
    require(all(payload(actual, index) == payload(literal, index) for index in PHYSICAL_LUMPS),
            "Native physical geometry, UVs, LEAF records or LDR/HDR lighting arrays differ from the literal control")
    require(payload(actual, 4) == payload(literal, 4) and visibility(actual) == visibility(literal),
            "Native physical-leaf PVS/PAS differ from the literal control")
    require(all(actual_stats[key] == literal_stats[key] for key in PHYSICAL_STATS),
            "Native physical/visibility/grouping/lighting statistics differ from the literal control")
    require(b"func_instance" not in payload(actual, 0) and b"func_viscluster" not in payload(actual, 0),
            "Compiler-only helpers escaped into runtime entity text")
    leaves = records(actual, 10, "<ihh6h4Hhh", expected_version=1)
    require(len(leaves) == actual_stats["leaves"], "Physical LEAF count differs from compiler statistics")
    if grouped:
        require(actual_stats["viscluster_helpers"] == actual_stats["viscluster_groups"] == 1
                and actual_stats["viscluster_grouped_leaves"] >= 2
                and actual_stats["clusters"] < actual_stats["visibility_solved_clusters"],
                "Literal fixture did not actually merge multiple physical leaves")
        left = cluster_at(actual, [-200, 0, 64])
        middle_cluster = cluster_at(actual, [0, 0, 64])
        right = cluster_at(actual, [200, 0, 64])
        require(left == middle_cluster and left != right, "Helper grouping differs at independent interior probe points")
        rows = visibility(actual)[2]
        require(all(not bit(rows[left][kind], right) and not bit(rows[right][kind], left) for kind in (0, 1)),
                "Helper grouping lost unaffected PVS/PAS occlusion")
    return {"exact_physical_lumps": list(PHYSICAL_LUMPS), "exact_visibility_lump": 4,
            "physical_leaves": len(leaves), "clusters": actual_stats["clusters"],
            "literal_bsp_sha256": hashlib.sha256(literal).hexdigest()}


def positive_cases(client, author, args, game, custody, report):
    controls = {}
    for placement in POSES:
        child = args.output / f"{placement}-child.vmf"
        marker = [72, -80, 4072] if placement == "noncommuting" else None
        authored = leaf(author, child, SOURCE_BOUNDS[placement], custody, marker=marker)
        reference, sources = child, [child]
        if placement == "noncommuting":
            reference = args.output / "noncommuting-middle.vmf"
            middle(author, reference, child, custody)
            sources.append(reference)
        control = parent(client, args.output / f"{placement}-parent.vmf", custody,
                         reference, placement=placement)
        form = reference_form(client, control)
        inspected = preview(client, control, 1)
        flat = parent(author, args.output / f"{placement}-literal.vmf", custody,
                      placement=placement, flat=True)
        for mode in ("fast", "detailed"):
            label = f"{placement}-{mode}"
            case = report["positive"][label] = {"authored_child": authored,
                "parent_authoring": control["authoring"],
                "reference_form": form, "preview": inspected, "literal_bounds": GROUP,
                "literal_marker": LITERAL_MARKER if marker else None}
            reviewed_open(client, control["source"])
            client.call("select", {"ids": control["owners"]})
            configure(client, mode)
            actual = pair(client, args, game, label, control, sources, case)
            expanded_counts(case, sources, 1, depth=2 if placement == "noncommuting" else 1,
                            occurrences=2 if placement == "noncommuting" else 1, retained=1)
            reviewed_open(client, flat["source"])
            configure(client, mode)
            literal_receipt = case["literal"] = {}
            literal = pair(client, args, game, f"{label}-literal", flat, [], literal_receipt)
            require("instances" not in literal_receipt["foreground"]["stats"],
                    "Literal flattened control invented instance statistics")
            case["comparison"] = comparison(actual, literal, case["foreground"]["stats"],
                                             literal_receipt["foreground"]["stats"])
            if placement == "noncommuting" and mode == "detailed":
                controls = {"control": control, "sources": sources, "child": child, "prior": actual}
    return controls


def omission_cases(client, author, args, game, custody, report):
    for hidden in ("entity", "solid"):
        child = args.output / f"hidden-{hidden}-child.vmf"
        leaf(author, child, SOURCE_BOUNDS["translation"], custody, hidden=hidden)
        control = parent(client, args.output / f"hidden-{hidden}-parent.vmf", custody, child)
        case = report["positive"][f"hidden-{hidden}"] = {"preview": preview(client, control, 0)}
        configure(client, "fast")
        pair(client, args, game, f"hidden-{hidden}", control, [child], case)
        expanded_counts(case, [child], 0, retained=0)

    child = args.output / "mixed-child.vmf"
    leaf(author, child, SOURCE_BOUNDS["translation"], custody, mixed=True)
    control = parent(client, args.output / "mixed-parent.vmf", custody, child)
    mixed = preview(client, control, 1, mixed=True)
    no_helper = args.output / "mixed-physical-only.vmf"
    reviewed_open(author, child)
    helpers = [row["id"] for row in author.call("find_objects", {"classname": "func_viscluster"})["objects"]]
    require(len(helpers) == 1, "Mixed fixture lost its one authored visibility helper")
    author.call("delete_objects", {"ids": helpers})
    save(author, no_helper, custody)
    ordinary = parent(client, args.output / "mixed-physical-parent.vmf", custody, no_helper)
    baseline = refresh(client, background=True)
    require("visclusters" not in baseline["stats"] and "viscluster_preview" not in baseline
            and all(mixed["geometry"][key] == baseline["geometry"][key] for key in GEOMETRY_FIELDS)
            and mixed["face_provenance_total"] == baseline["face_provenance_total"]
            and [row["bounds"] for row in mixed["owners"]] == [row["bounds"] for row in baseline["owners"]],
            "Consumed helpers changed mixed physical mesh/provenance/bounds")
    report["positive"]["mixed-physical-reference"] = {"mixed": mixed, "physical_only": baseline,
                                                      "physical_parent": str(ordinary["source"])}

    exterior = args.output / "exterior-child.vmf"
    leaf(author, exterior, ([1000, 1000, 1000], [1010, 1010, 1010]), custody)
    for label, repeated, region, count in (("sixteen", 15, False, 16), ("region-omits-seventeen", 17, True, 0)):
        control = parent(client, args.output / f"{label}-parent.vmf", custody, exterior,
                         repeated=repeated, region=region, root_helper=not region, grouped=False)
        configure(client, "fast")
        case = report["positive"][label] = {"preview": preview(client, control, repeated)}
        pair(client, args, game, label, control, [exterior], case)
        expanded_counts(case, [exterior], repeated, occurrences=repeated, retained=count)
    control = parent(client, args.output / "seventeen-parent.vmf", custody, exterior,
                     repeated=16, root_helper=True, grouped=False)
    configure(client, "fast")
    case = report["negative"]["seventeen"] = {}
    pair(client, args, game, "seventeen", control, [exterior], case, failure="16-helper")


def refusal_cases(client, author, args, game, custody, report):
    child = args.output / "ambiguous-child.vmf"
    new_map(author)
    first = helper(author, SOURCE_BOUNDS["translation"])
    second = helper(author, SOURCE_BOUNDS["translation"])
    save(author, child, custody)
    control = parent(client, args.output / "ambiguous-parent.vmf", custody, child)
    configure(client, "fast")
    pair(client, args, game, "ambiguous", control, [child], report["negative"].setdefault("ambiguous", {}),
         failure="both qualify", child_ids=(first["owner"], second["owner"]))

    child = args.output / "crossing-child.vmf"
    authored = leaf(author, child, ([-272, -242, -24], [94, 258, 216]), custody)
    control = parent(client, args.output / "crossing-parent.vmf", custody, child, region=True)
    configure(client, "fast")
    pair(client, args, game, "crossing", control, [child], report["negative"].setdefault("crossing", {}),
         failure="crosses a compile-region boundary", child_ids=(authored["owner"],))

    child = args.output / "malformed-child.vmf"
    authored = leaf(author, child, SOURCE_BOUNDS["translation"], custody)
    original = child.read_text()
    entities = fixture_blocks(original, "entity")
    require(len(entities) == 1, "Malformed fixture must retain exactly one original helper entity")
    changed = entities[0][:-1] + '\n"angles" "NaN 0 0"\n}'
    child.write_text(original.replace(entities[0], changed))
    custody[str(child)] = identity(child)
    control = parent(client, args.output / "malformed-parent.vmf", custody, child)
    configure(client, "fast")
    case = report["negative"]["malformed"] = {}
    case["reference_form"] = reference_form(client, control)
    failed = client.call("set_instance_contents", {"action": "refresh"})
    require(failed["state"] == "failed" and not failed["available"],
            "Malformed helper input was silently accepted or displayed by reference refresh")
    case["preview"] = failed
    pair(client, args, game, "malformed", control, [child], case, failure="angles",
         child_ids=(authored["owner"],), admission=True)


def dependency_case(client, args, game, controls, custody, report):
    control, sources, child = controls["control"], controls["sources"], controls["child"]
    reviewed_open(client, control["source"])
    client.call("select", {"ids": control["owners"]})
    configure(client, "detailed")
    case = report["negative"]["transitive-stale"] = {}
    pair(client, args, game, "before-stale", control, sources, case)
    preview(client, control, 1)
    before = document_state(client)
    outputs = identities(args.output / "builds")
    original = child.read_bytes()
    try:
        child.write_bytes(original + b"\n")
        stale = client.call("get_compile_status")
        inspected = client.call("inspect_instance_contents")
        require(stale["job"]["stale"] and not stale["job"]["instance_dependencies"]["current"]
                and not stale["launch"]["ready"] and inspected["state"] == "stale"
                and not inspected["available"],
                "Transitive helper source mutation did not stale both build publication and CPU references")
        require(document_state(client) == before and identities(args.output / "builds") == outputs,
                "Stale dependency inspection changed the document/history/selection or BSP/LIN")
        case["stale_build"], case["stale_preview"] = stale, inspected
        child.unlink()
        pair(client, args, game, "missing-transitive", control, sources,
             case.setdefault("missing_publication_refusal", {}), failure=child.name)
    finally:
        child.write_bytes(original)
        custody[str(child)] = identity(child)
    repaired = case["repaired"] = {}
    actual = pair(client, args, game, "after-stale", control, sources, repaired)
    require(actual == controls["prior"] and not client.call("get_compile_status")["job"]["stale"],
            "Repairing a transitive helper dependency failed to recover exact previous BSP bytes")


def close_client(client, report, timeout):
    try:
        job = client.call("get_compile_status")["job"]
        if job and job["status"] in ACTIVE_JOB_STATUSES:
            if job["cancellable"]:
                client.call("cancel_compile", {"token": job["token"]})
            report["drained_job"] = wait_job(client, timeout)
    finally:
        report["eof_exit_code"] = require_clean_eof(client)


def check_custody(custody, game, mounted, report):
    actual = {path: identity(Path(path)) for path in custody}
    report["custody_after_eof"] = actual
    changed = [path for path, expected in custody.items() if actual[path] != expected]
    errors = [AssertionError(f"Source, previous BSP/LIN, binary or FGD identity changed: {path}") for path in changed]
    if identities(game) != mounted:
        errors.append(AssertionError("Private mounted assets changed"))
    if errors:
        raise ExceptionGroup("Visibility-helper source/output custody failed", errors)


def finish_report(args, report, errors):
    report["status"] = "failed" if errors else "passed"
    report["compile_calls"] = args.compile_calls
    report["errors"] = [failure_details(error) for error in errors]
    (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"status": report["status"], "output": str(args.output),
                      "compile_calls": args.compile_calls}))
    if errors:
        raise BaseExceptionGroup("Instanced visibility-helper validation failed", errors)


def main():
    parser = argparse.ArgumentParser(
        description="Validate consumed child func_viscluster authoring, CPU reference reports and actual foreground/background native BSP builds through private production stdio MCP. The editor and hammerr-headless are each supported through --binary; run this script once for each executable. No game, render or desktop input is used.",
        epilog="Requires Python 3.11+, a production binary built from the corresponding source and an installed CS:S SDK cstrike.fgd with its includes. Creates bounded private material/game fixtures and VMFs; no original map is opened or modified. Run within the caller's aggregate 24 GiB budget and single build pipeline. The script performs no builds. Output and TMPDIR remain on disk under target/. Fast/Detailed grouping, physical LEAF/PVS/PAS and LDR/HDR array comparisons establish compiler equivalence to literal flattened controls; they do not establish native runtime speed, pixels, liquid/area behavior or player collision.",
    )
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY,
                        help="Production editor or hammerr-headless; launched only with --headless stdio MCP")
    parser.add_argument("--fgd", type=Path, default=DEFAULT_FGD,
                        help="Installed CS:S SDK root FGD; normal tie_to_entity defaults are retained")
    parser.add_argument("--output", type=Path, required=True,
                        help="Fresh artifact directory under target/; existing paths and symlinks are refused")
    parser.add_argument("--timeout", type=float, default=180,
                        help="Per-request and background-job timeout in seconds (default: 180)")
    args = parser.parse_args()
    if args.timeout <= 0:
        parser.error("--timeout must be positive")
    args.binary, args.fgd = args.binary.resolve(strict=True), args.fgd.resolve(strict=True)
    if not args.binary.is_file() or not os.access(args.binary, os.X_OK):
        parser.error("--binary must be an executable production binary")
    if not args.fgd.is_file():
        parser.error("--fgd must be an installed CS:S SDK FGD")
    if args.output.exists() or args.output.is_symlink():
        parser.error("--output must be fresh")
    args.output = args.output.resolve()
    if not args.output.is_relative_to(ROOT / "target"):
        parser.error("--output must remain on disk under this repository's target/")
    args.output.mkdir(parents=True, mode=0o700)
    args.compile_calls = 0
    errors, custody = [], {}
    args.custody = custody
    game = args.output / "game"
    assets(game)
    (game.parent / "cstrike_linux64").unlink()
    tools = game / "materials/tools"
    tools.mkdir()
    for name in ("toolsblack", "toolstrigger"):
        (tools / f"{name}.vmt").write_text(f'UnlitGeneric {{ $basetexture "tools/{name}" }}\n')
        (tools / f"{name}.vtf").write_bytes(fixture_texture(2, 2, (0, 0, 0), (0, 0, 0, 255)))
    mounted = identities(game)
    report = {"status": "running", "binary": str(args.binary), "binary_sha256": digest(args.binary),
              "script_sha256": digest(Path(__file__)), "transport": "private_stdio",
              "native_game_exercised": False, "desktop_input": False, "rendering_exercised": False,
              "positive": {}, "negative": {}, "clients": {},
              "literal_transform": {"translation": "(x+16, y-8, z+32)",
                                    "noncommuting": "(z+24, x-72, y+144)"}}
    with ExitStack() as lifecycle:
        lifecycle.callback(finish_report, args, report, errors)
        lifecycle.enter_context(record_failure(errors))
        lifecycle.callback(check_custody, custody, game, mounted, report)
        lifecycle.enter_context(record_failure(errors))
        custody[str(args.binary)] = identity(args.binary)
        (args.output / "builds").mkdir()
        temporary = args.output / "tmp"
        temporary.mkdir()
        clients = []
        for role in ("compiler", "author"):
            output = args.output / role
            output.mkdir()
            environment = {"TMPDIR": str(temporary), "HAMMERR_RECENT_MAPS_DIR": str(output / "recent"),
                           "HAMMERR_BUILD_RECIPES": str(output / "recipes.json"),
                           "HAMMERR_PREFABS_DIR": str(output / "prefabs"),
                           "XDG_CONFIG_HOME": str(output / "config"),
                           "XDG_CACHE_HOME": str(output / "cache"), "XDG_STATE_HOME": str(output / "state")}
            client = Client(None, args.binary, output, game / "gameinfo.txt",
                            environment=environment, request_timeout=args.timeout)
            clients.append(client)
            client_report = report["clients"][role] = {}
            lifecycle.callback(close_client, client, client_report, args.timeout)
            lifecycle.enter_context(record_failure(errors))
            client_report["definitions"] = client.call("load_entity_definitions", {"path": str(args.fgd)})
            class_info = client.call("get_entity_class", {"classname": "func_viscluster"})
            require(class_info["kind"] == "solid", "Installed FGD has no ordinary solid func_viscluster definition")
            client_report["viscluster_class"] = class_info
        for source in report["clients"]["author"]["definitions"]["sources"]:
            path = source["path"] if isinstance(source, dict) else source
            custody[str(Path(path))] = identity(Path(path))
        client, author = clients
        controls = positive_cases(client, author, args, game, custody, report)
        omission_cases(client, author, args, game, custody, report)
        refusal_cases(client, author, args, game, custody, report)
        dependency_case(client, args, game, controls, custody, report)


if __name__ == "__main__":
    main()
