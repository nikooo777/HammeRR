import argparse
import json
import os
import re
import sys
from contextlib import ExitStack
from pathlib import Path

from validate_authored_planes import records
from validate_collision_topology import failure_details, identity, record_failure
from validate_face_lighting import ROOT, Client, detail, face_ref
from validate_instance_contents import refresh
from validate_instance_reflections import REASON, digest
from validate_instanced_cubemaps import (
    audit_program,
    compile_pair,
    configure,
    cubemap,
    snapshot,
    watch,
)
from validate_instanced_visclusters import (
    DEFAULT_FGD,
    authored_instance,
    close_client,
    instance,
    new_map,
    reviewed_open,
    save,
)
from validate_material_albedo import fixture_texture, identities, payload, require
from validate_nested_brush_instances import assets, fixture_blocks
from validate_visibility import ROOM

PLAIN = "hammerr690/world"
REFLECTIVE = "hammerr750/reflective"
WORLD = ([-16, -16, -8], [16, 16, 8])
DETAIL = ([32, -16, -8], [64, 16, 8])
NEAR, FAR = ([0, 0, 24], [0, 0, 80])
U, V = ((2 / 3, -2 / 3, 1 / 3), (1 / 3, 2 / 3, 2 / 3))
SHIFT = (7.5, -3.25)
NESTED_OFFSET = (1024, 1024, 1024)
LITERALS = {
    "repeated": (
        {"world": ([-112, -16, 56], [-80, 16, 72]),
         "detail": ([-64, -16, 56], [-32, 16, 72]),
         "normal": [0, 0, 1], "u": [2 / 3, -2 / 3, 1 / 3], "v": [1 / 3, 2 / 3, 2 / 3],
         "shift": [1069 / 6, -551 / 12], "near": [-96, 0, 88], "far": [-96, 0, 144]},
        {"world": ([80, -16, 56], [112, 16, 72]),
         "detail": ([128, -16, 56], [160, 16, 72]),
         "normal": [0, 0, 1], "u": [2 / 3, -2 / 3, 1 / 3], "v": [1 / 3, 2 / 3, 2 / 3],
         "shift": [-2003 / 6, -3623 / 12], "near": [96, 0, 88], "far": [96, 0, 144]},
    ),
    "nested": (
        {"world": ([1040, 936, 1152], [1056, 968, 1184]),
         "detail": ([1040, 984, 1152], [1056, 1016, 1184]),
         "normal": [1, 0, 0], "u": [1 / 3, 2 / 3, -2 / 3], "v": [2 / 3, 1 / 3, 2 / 3],
         "shift": [-4883 / 6, -86183 / 12], "near": [1072, 952, 1168], "far": [1128, 952, 1168]},
        {"world": ([1040, 936, 1216], [1056, 968, 1248]),
         "detail": ([1040, 984, 1216], [1056, 1016, 1248]),
         "normal": [1, 0, 0], "u": [1 / 3, 2 / 3, -2 / 3], "v": [2 / 3, 1 / 3, 2 / 3],
         "shift": [-3859 / 6, -88231 / 12], "near": [1072, 952, 1232], "far": [1128, 952, 1232]},
    ),
}
NESTED_PLANES = (
    (
        (
            "(1056 936 1184) (1056 968 1184) (1056 968 1152)",
            "(1040 968 1184) (1040 936 1184) (1040 936 1152)",
            "(1056 936 1152) (1040 936 1152) (1040 936 1184)",
            "(1056 968 1184) (1040 968 1184) (1040 968 1152)",
            "(1056 936 1184) (1040 936 1184) (1040 968 1184)",
            "(1056 968 1152) (1040 968 1152) (1040 936 1152)",
        ),
        (
            "(1056 984 1184) (1056 1016 1184) (1056 1016 1152)",
            "(1040 1016 1184) (1040 984 1184) (1040 984 1152)",
            "(1056 984 1152) (1040 984 1152) (1040 984 1184)",
            "(1056 1016 1184) (1040 1016 1184) (1040 1016 1152)",
            "(1056 984 1184) (1040 984 1184) (1040 1016 1184)",
            "(1056 1016 1152) (1040 1016 1152) (1040 984 1152)",
        ),
    ),
    (
        (
            "(1056 936 1248) (1056 968 1248) (1056 968 1216)",
            "(1040 968 1248) (1040 936 1248) (1040 936 1216)",
            "(1056 936 1216) (1040 936 1216) (1040 936 1248)",
            "(1056 968 1248) (1040 968 1248) (1040 968 1216)",
            "(1056 936 1248) (1040 936 1248) (1040 968 1248)",
            "(1056 968 1216) (1040 968 1216) (1040 936 1216)",
        ),
        (
            "(1056 984 1248) (1056 1016 1248) (1056 1016 1216)",
            "(1040 1016 1248) (1040 984 1248) (1040 984 1216)",
            "(1056 984 1216) (1040 984 1216) (1040 984 1248)",
            "(1056 1016 1248) (1040 1016 1248) (1040 1016 1216)",
            "(1056 984 1248) (1040 984 1248) (1040 1016 1248)",
            "(1056 1016 1216) (1040 1016 1216) (1040 984 1216)",
        ),
    ),
)
NESTED_NORMALS = ([1, 0, 0], [-1, 0, 0], [0, -1, 0],
                  [0, 1, 0], [0, 0, 1], [0, 0, -1])
LOCAL_TARGET_ERROR = "must name a visible ordinary world or func_detail side in the same child"
HELPERS = (
    "validate_instanced_cubemap_bindings.py", "validate_instanced_cubemaps.py",
    "validate_instanced_visclusters.py", "validate_collision_topology.py",
    "validate_face_lighting.py", "validate_instance_contents.py",
    "validate_instance_reflections.py", "validate_nested_brush_instances.py",
    "validate_material_albedo.py", "validate_authored_planes.py",
)


def write_json(path, value):
    with path.open("x") as stream:
        json.dump(value, stream, indent=2, default=json_path)
        stream.write("\n")


def json_path(value):
    if isinstance(value, Path):
        return str(value)
    raise TypeError(f"Unsupported validation artifact value: {type(value).__name__}")


def declarations():
    cases = {"local": {"world": WORLD, "detail": DETAIL, "normal": [0, 0, 1],
                        "near": NEAR, "far": FAR}}
    cases.update({f"{placement}-{index}": row for placement, rows in LITERALS.items()
                  for index, row in enumerate(rows)})
    proofs = []
    for label, case in cases.items():
        axis = case["normal"].index(1)
        for role in ("world", "detail"):
            low, high = case[role]
            center = [(a + b) / 2 for a, b in zip(low, high, strict=True)]
            center[axis] = high[axis]
            distances = {name: sum((a - b) ** 2 for a, b in zip(case[name], center, strict=True))
                         for name in ("near", "far")}
            require(distances["far"] > distances["near"]
                    and all(case[name][axis] > center[axis] for name in ("near", "far")),
                    "Literal manual sample must be strictly farther than the automatic front-facing control")
            proofs.append({"case": label, "role": role, "face_center": center,
                           "near": case["near"], "far": case["far"], "distance_squared": distances})
    return proofs


def block(client, bounds, normal=(0, 0, 1), u=U, v=V, shift=SHIFT):
    identifier = client.call("create_block", {"mins": bounds[0], "maxs": bounds[1],
                                                "material": PLAIN})["id"]
    shape = detail(client, [identifier])[0]
    faces = [face_ref(shape, face) for face in shape["faces"]]
    client.call("edit_faces", {"faces": faces, "u_axis": u, "v_axis": v,
                              "scale_u": 0.25, "scale_v": 0.25,
                              "shift_u": shift[0], "shift_v": shift[1], "lightmap_scale": 64})
    selected = [face_ref(shape, face) for face in shape["faces"] if face["normal"] == list(normal)]
    require(len(selected) == 1, "One literal target face with the declared normal")
    client.call("edit_faces", {"faces": selected, "material": REFLECTIVE})
    return identifier, selected[0]


def assignments(report, faces, helper, manual):
    require(report["available"] is True and not report["diagnostics"],
            "Ordinary local binding inspection must be explicitly available without diagnostics")
    result = []
    for face in faces:
        selected = [row for row in report["assignments"] if row["face"] == face]
        require(len(selected) == 1 and selected[0]["helper"] == helper
                and selected[0]["manual"] is manual and selected[0]["material"] == REFLECTIVE,
                f"Literal farther/manual or nearer/automatic assignment differs: {selected}")
        result.append(selected[0])
    return result


def bind(client, source, near, far, faces, total):
    client.call("select_faces", {"faces": faces})
    original = snapshot(client)
    saved = identity(source)
    report = client.call("get_cubemap_bindings", {"limit": 256})
    require(report["total"] == total and len(report["assignments"]) == total,
            "Complete local reflective assignment count")
    automatic = assignments(report, faces, near, False)
    near_origin = detail(client, [f"entity:{near}"])[0]["origin"]
    far_origin = detail(client, [f"entity:{far}"])[0]["origin"]
    require(all(float(value).is_integer() for origin in (near_origin, far_origin) for value in origin),
            "Literal sample fixture positions must be exact integer coordinates")
    require(all(row["position"] == near_origin for row in automatic), "Actual nearer helper position differs")
    request = {"action": "preview", "helper": far, "faces": faces,
               "operation": "bind", "snapshot": report["state"]["snapshot"]}
    draft = client.call("set_cubemap_bindings", request)
    require(draft["draft"]["owner"] == "agent" and draft["draft"]["valid"] is True,
            "Shared child binding draft must be valid and owned by the requesting agent")
    preview = assignments(draft["draft"]["report"], faces, far, True)
    require(all(row["position"] == far_origin
                and row["alias"] == f'maps/{source.stem}/{REFLECTIVE}_{int(far_origin[0])}_{int(far_origin[1])}_{int(far_origin[2])}'
                for row in preview), "Actual farther manual helper position/alias differs from the literal local plan")
    require(snapshot(client) == original and identity(source) == saved,
            "Shared binding Preview changed source/history/selection/camera/settings")
    client.call("set_cubemap_bindings", {"action": "cancel", "token": draft["draft"]["token"]})
    require(snapshot(client) == original and identity(source) == saved,
            "Shared binding Cancel changed source or history")
    request["snapshot"] = client.call("get_cubemap_bindings")["state"]["snapshot"]
    draft = client.call("set_cubemap_bindings", request)
    old_token = draft["draft"]["token"]
    reviewed = client.call("set_cubemap_bindings", {"action": "review", "token": old_token})
    token = reviewed["draft"]["token"]
    require(token != old_token and reviewed["draft"]["valid"] is True
            and reviewed["draft"]["owner"] == "agent", "Review must replace the owned draft token")
    refused = client.call("set_cubemap_bindings", {"action": "apply", "token": old_token}, error=True)
    require(client.call("get_cubemap_bindings")["state"]["draft"] == reviewed["draft"]
            and snapshot(client) == original and identity(source) == saved,
            "Replaced binding token changed source or the reviewed draft")
    client.call("set_cubemap_bindings", {"action": "apply", "token": token})
    applied = snapshot(client)
    before_document, after_document = [value["session"]["document"] for value in (original, applied)]
    require(after_document["undo"][0] == "Edit cubemap face bindings"
            and len(after_document["undo"]) == len(before_document["undo"]) + 1,
            "Shared binding Apply must create its one named Undo transaction")
    changed = {"undo", "redo", "unsaved_changes", "generation", "revision"}
    require({key: value for key, value in before_document.items() if key not in changed}
            == {key: value for key, value in after_document.items() if key not in changed}
            and applied["session"]["settings"] == original["session"]["settings"]
            and applied["property_draft"] == original["property_draft"],
            "Binding Apply changed source-independent camera/selection/settings/draft state")
    expected_sides = " ".join(str(face["side"]) for face in sorted(faces, key=lambda face: face["side"]))
    for before, after in zip(original["session"]["objects"], applied["session"]["objects"], strict=True):
        if before["id"] == f"entity:{far}":
            require([value for key, value in after["properties"] if key == "sides"] == [expected_sides]
                    and {**after, "properties": [pair for pair in after["properties"] if pair[0] != "sides"]} == before,
                    "Binding Apply changed the helper beyond its exact local sides list")
        else:
            require(after == before, "Binding Apply changed unrelated source objects")
    require(identity(source) == saved, "Binding Apply implicitly saved source bytes")
    assignments(client.call("get_cubemap_bindings", {"limit": 256}), faces, far, True)
    client.call("undo")
    require(snapshot(client)["session"]["objects"] == original["session"]["objects"],
            "Undo did not restore the exact local source objects")
    client.call("redo")
    require(snapshot(client)["session"]["objects"] == applied["session"]["objects"],
            "Redo did not restore the exact reviewed manual sides")
    return {"automatic": automatic, "manual": preview, "replaced_token_refusal": refused,
            "raw_sides": expected_sides}


def verify_saved_bindings(client, bindings):
    for binding in bindings:
        entity = detail(client, [f'entity:{binding["far"]}'])[0]
        values = [value for key, value in entity["properties"] if key == "sides"]
        require(values == [binding["workflow"]["raw_sides"]],
                "Save/reopen or compilation rewrote the original local manual sides")
        assignments(client.call("get_cubemap_bindings", {"limit": 256}),
                    binding["faces"], binding["far"], True)


def leaf(client, path, args, helpers_first):
    new_map(client)
    world, world_face = block(client, WORLD)
    physical, detail_face = block(client, DETAIL)
    all_sides = [face["side"] for shape in detail(client, [world, physical]) for face in shape["faces"]]
    if not helpers_first:
        detail_owner = client.call("tie_to_entity", {"ids": [physical], "classname": "func_detail"})["id"]
    near = int(cubemap(client, NEAR, size=3)["id"].split(":")[-1])
    far = int(cubemap(client, FAR, size=3)["id"].split(":")[-1])
    if helpers_first:
        detail_owner = client.call("tie_to_entity", {"ids": [physical], "classname": "func_detail"})["id"]
    save(client, path, args.custody)
    faces = [world_face, detail_face]
    workflow = bind(client, path, near, far, faces, 2)
    save(client, path, args.custody)
    binding = {"near": near, "far": far, "faces": faces, "workflow": workflow}
    verify_saved_bindings(client, [binding])
    entities = fixture_blocks(path.read_text(), "entity")
    classes = [re.search(r'"classname"\s+"([^"]+)"', entity)[1] for entity in entities]
    require(classes == (["env_cubemap", "env_cubemap", "func_detail"] if helpers_first
                        else ["func_detail", "env_cubemap", "env_cubemap"]),
            "Child fixture did not retain the intended helper/detail source order")
    return {"source": path, "binding": binding, "world": world,
            "detail_owner": detail_owner, "all_sides": all_sides,
            "source_sha256": digest(path)}


def room(client, offset=(0, 0, 0)):
    ids = []
    for bounds in ROOM:
        shifted = [[coordinate + delta for coordinate, delta in zip(point, offset, strict=True)]
                   for point in bounds]
        identifier = client.call("create_block", {"mins": shifted[0], "maxs": shifted[1], "material": PLAIN})["id"]
        shape = detail(client, [identifier])[0]
        client.call("edit_faces", {"faces": [face_ref(shape, face) for face in shape["faces"]],
                                  "lightmap_scale": 64})
        ids.append(identifier)
    ids.append(client.call("create_entity", {"classname": "info_player_terrorist",
                                              "origin": [-192 + offset[0], -176 + offset[1], 8 + offset[2]]})["id"])
    ids.append(client.call("create_entity", {"classname": "light",
                "origin": [offset[0], offset[1], 192 + offset[2]],
                "properties": {"_light": "255 224 192 200", "_lightHDR": "192 224 255 200",
                               "_constant_attn": "1", "_linear_attn": "0", "_quadratic_attn": "0"}})["id"])
    return ids


def parent(client, path, args, child, placement, region=None):
    sources = [child["source"]]
    reference = child["source"]
    if placement == "nested":
        new_map(client)
        for origin in ([8, 16, 64], [8, 80, 64]):
            instance(client, child["source"], origin, "90 0 0")
        reference = path.with_name(path.stem + "-middle.vmf")
        save(client, reference, args.custody)
        sources.append(reference)
    new_map(client)
    ids = room(client, NESTED_OFFSET if placement == "nested" else (0, 0, 0))
    client.call("save_map", {"path": str(path),
                              "expected_generation": client.call("get_map_info")["generation"]})
    poses = (([-96, 0, 64], "0 0 0"), ([96, 0, 64], "0 0 0")) if placement == "repeated" else (([1040, 1016, 1152], "0 0 90"),)
    owners, authoring = [], []
    for origin, angles in poses:
        owner, form = authored_instance(client, reference, origin, angles)
        owners.append(owner)
        authoring.append(form)
    ids.extend(owners)
    if region is not None:
        draft = client.call("preview_compile_region", {"snapshot": client.call("get_compile_region")["snapshot"],
                    "enabled": True, "mins": region[0], "maxs": region[1]})
        client.call("apply_compile_region", {"token": draft["draft"]["token"]})
    save(client, path, args.custody)
    if placement == "nested":
        save(client, path, args.custody)
    client.call("select", {"ids": owners})
    root_sides = {face["side"] for shape in detail(client, ids[:6]) for face in shape["faces"]}
    require(root_sides.intersection(face["side"] for face in child["binding"]["faces"]),
            "The original local side IDs must actually collide with original root side IDs")
    return {"source": path, "owners": owners, "ids": ids, "authoring": authoring,
            "root_sides": sorted(root_sides)}, sources


def literal_solid(identifier, planes, literal):
    require(len(planes) == 6, "A literal control solid must declare exactly six ordered plane triples")
    axes = ["[" + " ".join(format(value, ".17g") for value in (*literal[key], literal["shift"][index]))
            + "] 0.25" for index, key in enumerate(("u", "v"))]
    sides = []
    for index, plane in enumerate(planes):
        material = REFLECTIVE if index == 0 else PLAIN
        sides.append(f'''side
{{
"id" "{identifier + index + 1}"
"plane" "{plane}"
"material" "{material.upper()}"
"uaxis" "{axes[0]}"
"vaxis" "{axes[1]}"
"rotation" "0"
"lightmapscale" "64"
"smoothing_groups" "0"
}}
''')
    return 'solid\n{\n"id" "' + str(identifier) + '"\n' + "".join(sides) + '}\n'


def nested_literal_geometry(client, path, args, ids):
    save(client, path, args.custody)
    text = path.read_text()
    world = fixture_blocks(text, "world")
    require(len(world) == 1 and text.count(world[0]) == 1,
            "Independent literal root world must be unambiguous")
    solids, entities, geometry = [], [], []
    for index, (planes, literal) in enumerate(zip(NESTED_PLANES, LITERALS["nested"], strict=True)):
        world_id, entity_id = 10000 + index * 100, 20000 + index * 100
        detail_id = entity_id + 1
        solids.append(literal_solid(world_id, planes[0], literal))
        entities.append('entity\n{\n"id" "' + str(entity_id) + '"\n"classname" "func_detail"\n'
                        + literal_solid(detail_id, planes[1], literal) + '}\n')
        geometry.append((f"solid:{world_id}", f"entity:{entity_id}",
                         {"solid": world_id, "side": world_id + 1},
                         {"solid": detail_id, "side": detail_id + 1}))
    replacement = world[0][:-1] + "\n" + "".join(solids) + "}"
    path.write_text(text.replace(world[0], replacement, 1) + "\n" + "".join(entities))
    reviewed_open(client, path)
    for (world_id, detail_owner, world_face, detail_face), literal in zip(geometry, LITERALS["nested"], strict=True):
        world_shape, entity = detail(client, [world_id, detail_owner])
        require(len(entity["brushes"]) == 1, "Literal func_detail control must retain its one declared solid")
        for shape, role, face in ((world_shape, "world", world_face),
                                  (entity["brushes"][0], "detail", detail_face)):
            require(shape["bounds"] == {"mins": literal[role][0], "maxs": literal[role][1]}
                    and [row["normal"] for row in shape["faces"]] == list(NESTED_NORMALS)
                    and [row["side"] for row in shape["faces"]] == list(range(face["side"], face["side"] + 6)),
                    "Opened literal planes lost their independent bounds, source side order or VMF winding")
        ids.extend((world_id, detail_owner))
    return geometry


def flat(client, path, args, placement):
    new_map(client)
    ids = room(client, NESTED_OFFSET if placement == "nested" else (0, 0, 0))
    geometry = nested_literal_geometry(client, path, args, ids) if placement == "nested" else None
    bindings = []
    for index, literal in enumerate(LITERALS[placement]):
        if geometry is None:
            options = {name: literal[name] for name in ("normal", "u", "v", "shift")}
            world, world_face = block(client, literal["world"], **options)
            physical, detail_face = block(client, literal["detail"], **options)
            detail_owner = client.call("tie_to_entity", {"ids": [physical], "classname": "func_detail"})["id"]
            ids.extend((world, detail_owner))
        else:
            world, detail_owner, world_face, detail_face = geometry[index]
        near = int(cubemap(client, literal["near"], size=3)["id"].split(":")[-1])
        far = int(cubemap(client, literal["far"], size=3)["id"].split(":")[-1])
        ids.extend((f"entity:{near}", f"entity:{far}"))
        bindings.append({"near": near, "far": far, "faces": [world_face, detail_face]})
    save(client, path, args.custody)
    for binding in bindings:
        binding["workflow"] = bind(client, path, binding["near"], binding["far"], binding["faces"], 4)
    save(client, path, args.custody)
    verify_saved_bindings(client, bindings)
    client.call("select", {"ids": ids[:1]})
    return {"source": path, "owners": [], "ids": ids, "bindings": bindings}


def inspection(client, control, cubemaps, brushes):
    before = snapshot(client)
    ready = refresh(client, background=True)
    require(ready["state"] == "ready" and ready["available"] is True
            and ready["stats"]["cubemaps"] == cubemaps
            and ready["stats"]["world_brushes"] == brushes
            and ready["stats"]["detail_brushes"] == brushes,
            "Reference inspection must report actual direct and nested geometry/sample usage")
    bindings = client.call("get_cubemap_bindings")
    require(bindings["available"] is False and bindings["total"] == 0
            and bindings["assignments"] == [] and bindings["diagnostics"] == [REASON]
            and bindings["state"]["context_error"] == REASON,
            "Root authored-face inspection must retain its explicit assembled-geometry refusal")
    root = detail(client, control["ids"][:1])[0]
    refused = client.call("set_cubemap_bindings", {"action": "preview",
        "helper": int(control["owners"][0].split(":")[-1]),
        "faces": [face_ref(root, root["faces"][0])], "operation": "bind",
        "snapshot": bindings["state"]["snapshot"]}, error=True)
    require(REASON in json.dumps(refused) and snapshot(client) == before,
            "Unavailable root binding draft changed source/history/camera/settings")
    return {"contents": ready, "root_binding_refusal": refused, "bindings": bindings}


def compare(actual, literal, placement):
    left, right = [Path(pair["foreground"]["bsp"]).read_bytes() for pair in (actual, literal)]
    expected = [(*point, 3, b"\0\0\0") for occurrence in LITERALS[placement]
                for point in (occurrence["near"], occurrence["far"])]
    require(records(left, 42, "<3iB3s") == expected == records(right, 42, "<3iB3s"),
            "Literal transformed sample order/positions/size/padding differ")
    require(left[:8] == right[:8] and left[1032:1036] == right[1032:1036],
            "Child and independently authored flat BSP headers differ")
    for index in range(64):
        require(payload(left, index) == payload(right, index),
                f"Child versus independent literal complete ordinary BSP payload differs: lump{index}")
    require(b"func_instance" not in payload(left, 0) and b"env_cubemap" not in payload(left, 0),
            "Consumed manual helpers escaped into runtime entities")
    for result in actual.values():
        warnings = [row for row in result["result"]["diagnostics"] if row["level"] == "warning"]
        require(any("sides list is unused in this build" in row["message"] for row in warnings),
                "Ordinary builds must explicitly retain the no-capture/manual-sides-unused boundary")
    return {"exact_logical_payloads": 64,
            "literal_samples": [{"origin": row[:3], "size": row[3], "padding_hex": row[4].hex()} for row in expected],
            "capture_executed": False, "pending_plans_covered_by": "compiler integration tests"}


def replace_sides(source, far, value):
    text = source.read_text()
    entities = [entity for entity in fixture_blocks(text, "entity")
                if re.search(r'"id"\s+"' + str(far) + '"', entity)
                and '"classname" "env_cubemap"' in entity]
    require(len(entities) == 1, "Exactly one original local manual helper in imported fixture")
    changed, count = re.subn(r'"sides"\s+"[^"]*"', '"sides" "' + value + '"', entities[0])
    require(count == 1 and changed != entities[0], "One exact manual list must change")
    source.write_text(text.replace(entities[0], changed))


def negative_cases(client, args, output, game, child, report):
    original = child["source"].read_bytes()
    cases = (("missing", "999999", LOCAL_TARGET_ERROR),
             ("duplicate", f'{child["binding"]["faces"][0]["side"]} {child["binding"]["faces"][0]["side"]}', "cubemap sides"),
             ("foreign-root", None, LOCAL_TARGET_ERROR))
    for name, value, token in cases:
        path = output / f"{name}-child.vmf"
        path.write_bytes(original)
        candidate = {**child, "source": path}
        control, sources = parent(client, output / f"{name}-parent.vmf", args, candidate, "repeated")
        if value is None:
            foreign = [side for side in control["root_sides"] if side not in child["all_sides"]]
            require(foreign, "Foreign-target fixture requires an actual root side absent from the child")
            value = str(foreign[-1])
        replace_sides(path, child["binding"]["far"], value)
        watch(args.custody, (path,))
        configure(client)
        refused = compile_pair(client, args, output, game, name, control, sources,
            failure=token, original_ids=(f'entity:{child["binding"]["far"]}',), admission=True)
        report[name] = {"sides": value, "builds": refused}
    reviewed_open(client, child["source"])
    client.call("set_object_visibility", {"action": "hide", "ids": [child["world"]]})
    hidden_target = output / "hidden-target-child.vmf"
    save(client, hidden_target, args.custody)
    candidate = {**child, "source": hidden_target}
    control, sources = parent(client, output / "hidden-target-parent.vmf", args, candidate, "repeated")
    configure(client)
    report["hidden-target"] = compile_pair(client, args, output, game, "hidden-target", control, sources,
        failure=LOCAL_TARGET_ERROR, original_ids=(f'entity:{child["binding"]["far"]}',), admission=True)


def omissions(client, args, output, game, child, report):
    reviewed_open(client, child["source"])
    client.call("set_entity_properties", {"id": f'entity:{child["binding"]["far"]}',
                                          "set": {"sides": "999999"}})
    client.call("set_object_visibility", {"action": "hide", "ids": [f'entity:{child["binding"]["far"]}']})
    hidden = output / "hidden-helper-child.vmf"
    save(client, hidden, args.custody)
    candidate = {**child, "source": hidden}
    control, sources = parent(client, output / "hidden-helper-parent.vmf", args, candidate, "repeated")
    configure(client)
    built = compile_pair(client, args, output, game, "hidden-helper", control, sources)
    expected = [(*row["near"], 3, b"\0\0\0") for row in LITERALS["repeated"]]
    require(records(Path(built["foreground"]["bsp"]).read_bytes(), 42, "<3iB3s") == expected,
            "Hidden invalid manual helper must be omitted before local-side admission")
    report["hidden-helper"] = built
    control, sources = parent(client, output / "region-parent.vmf", args, child, "repeated",
        region=([-250, -250, 4], [0, 250, 252]))
    configure(client)
    built = compile_pair(client, args, output, game, "region", control, sources)
    expected = [(*LITERALS["repeated"][0][name], 3, b"\0\0\0") for name in ("near", "far")]
    require(records(Path(built["foreground"]["bsp"]).read_bytes(), 42, "<3iB3s") == expected
            and built["foreground"]["result"]["stats"]["instances"]["cubemaps"] == 4
            and built["foreground"]["result"]["stats"]["cubemap_samples"] == 2,
            "Region selection must distinguish expanded manual helpers from retained samples")
    report["region"] = built


def dependency(client, args, output, game, child, control, sources, report):
    reviewed_open(client, control["source"])
    configure(client, "direct_ldr_hdr")
    inspection(client, control, 4, 2)
    prior = compile_pair(client, args, output, game, "before-drift", control, sources)
    before = snapshot(client)
    saved = child["source"].read_bytes()
    outputs = {name: identity(Path(name)) for name in args.custody
               if Path(name).is_relative_to(output) and Path(name).suffix in (".bsp", ".lin")}
    try:
        child["source"].write_bytes(saved + b"\n")
        status = client.call("get_compile_status")
        contents = client.call("inspect_instance_contents")
        require(status["job"]["stale"] is True
                and status["job"]["instance_dependencies"]["current"] is False
                and status["launch"]["ready"] is False and contents["state"] == "stale"
                and contents["available"] is False and snapshot(client) == before,
                "Transitive child binding source drift must stale compilation and reference inspection")
        child["source"].unlink()
        refused = compile_pair(client, args, output, game, "missing-transitive", control, sources,
                               failure=child["source"].name)
        report.update({"stale": status, "stale_contents": contents, "missing_refusal": refused})
    finally:
        child["source"].write_bytes(saved)
        watch(args.custody, (child["source"],))
    require({name: identity(Path(name)) for name in outputs} == outputs,
            "Source drift/refusal changed previously retained BSP/LIN artifacts")
    repaired = compile_pair(client, args, output, game, "repaired", control, sources)
    require(repaired["background"]["bsp_sha256"] == prior["background"]["bsp_sha256"],
            "Explicitly restored local sides did not reproduce the original ordinary BSP")
    report["repaired"] = repaired


def run_program(binary, args, output, report):
    output.mkdir()
    args.native_tmpdir = output / "tmp"
    args.native_tmpdir.mkdir()
    game = output / "source-game"
    assets(game)
    materials = game / "materials/hammerr750"
    materials.mkdir()
    (materials / "reflective.vmt").write_text('LightmappedGeneric { $basetexture "hammerr690/base" $envmap "env_cubemap" }\n')
    tools = game / "materials/tools"
    tools.mkdir()
    (tools / "toolsblack.vmt").write_text('UnlitGeneric { $basetexture "tools/toolsblack" }\n')
    (tools / "toolsblack.vtf").write_bytes(fixture_texture(2, 2, (0, 0, 0), (0, 0, 0, 255)))
    material_before = identities(game / "materials")
    watch(args.custody, (game / "gameinfo.txt", game.parent / "cstrike_linux64"))
    report.update({"binary": str(binary), "binary_sha256": digest(binary), "positive": {},
                   "refusals": {}, "omissions": {}, "dependency": {}})
    with ExitStack() as lifecycle:
        lifecycle.enter_context(record_failure(args.errors))
        lifecycle.callback(audit_program, args, game, material_before, output, report)
        client_output = output / "client"
        client_output.mkdir()
        environment = {"TMPDIR": str(args.native_tmpdir), "HAMMERR_RECENT_MAPS_DIR": str(output / "recent"),
            "HAMMERR_BUILD_RECIPES": str(output / "recipes.json"), "HAMMERR_PREFABS_DIR": str(output / "prefabs"),
            "HAMMERR_RECOVERY_DIR": str(output / "recovery"), "XDG_CONFIG_HOME": str(output / "config"),
            "XDG_STATE_HOME": str(output / "state"), "XDG_CACHE_HOME": str(output / "cache")}
        client = Client(None, binary, client_output, game / "gameinfo.txt",
                        environment=environment, request_timeout=args.timeout)
        lifecycle.callback(close_client, client, report, args.timeout)
        lifecycle.enter_context(record_failure(args.errors))
        loaded = client.call("load_entity_definitions", {"path": str(args.fgd)})
        for path in loaded["sources"]:
            actual = identity(Path(path))
            if path in args.custody:
                require(actual == args.custody[path], "Original FGD identity changed before another Session loaded it")
            else:
                args.custody[path] = actual
        controls = {}
        for placement, helpers_first, lighting in (("repeated", True, "fullbright"),
                                                   ("nested", False, "direct_ldr_hdr")):
            child = leaf(client, output / f"{placement}-child.vmf", args, helpers_first)
            control, sources = parent(client, output / f"{placement}-parent.vmf", args, child, placement)
            inspected = inspection(client, control, 4, 2)
            configure(client, lighting)
            actual = compile_pair(client, args, output, game, placement, control, sources)
            literal = flat(client, output / f"{placement}-flat.vmf", args, placement)
            versions = [re.findall(r'"mapversion"\s+"([^"]+)"', fixture["source"].read_text())
                        for fixture in (control, literal)]
            require(versions[0] == versions[1] and len(versions[0]) == 2
                    and len(set(versions[0])) == 1,
                    "Saved authored parent and literal control must share versioninfo/world mapversion metadata")
            configure(client, lighting)
            flattened = compile_pair(client, args, output, game, f"{placement}-flat", literal, [])
            verify_saved_bindings(client, literal["bindings"])
            require("instances" not in flattened["foreground"]["result"]["stats"],
                    "Independent literal flat control invented instance dependencies")
            report["positive"][placement] = {"authored_child": child, "parent_authoring": control["authoring"],
                "inspection": inspected, "actual": actual, "literal": flattened,
                "saved_mapversion": versions[0][0],
                "comparison": compare(actual, flattened, placement)}
            controls[placement] = (child, control, sources)
        child, control, sources = controls["nested"]
        dependency(client, args, output, game, child, control, sources, report["dependency"])
        child = controls["repeated"][0]
        negative_cases(client, args, output, game, child, report["refusals"])
        omissions(client, args, output, game, child, report["omissions"])
        for child, _, _ in controls.values():
            reviewed_open(client, child["source"])
            verify_saved_bindings(client, [child["binding"]])
        require(not client.call("get_compile_settings")["generate_reflections"]
                and not any(args.native_tmpdir.glob("hammerr_capture_*")),
                "Compiler-composition validation must not request or start capture")
        report["success"] = True


def main():
    parser = argparse.ArgumentParser(description="Validate local manual child cubemap binding authoring/save/reopen and repeated/nested ordinary compilation through both actual stdio MCP programs. Compares all64 logical ordinary BSP payloads against independent literal geometry/samples and exercises local-target refusals, omission, source staleness and custody. No capture or native game is executed; compiler integration tests cover pending LDR/HDR binding plans.")
    parser.add_argument("--binary", type=Path, action="append", required=True,
                        help="Specify actual hammerr-headless and editor once each")
    parser.add_argument("--fgd", type=Path, default=DEFAULT_FGD)
    parser.add_argument("--output", type=Path, required=True, help="Fresh on-disk artifact directory under target/")
    parser.add_argument("--timeout", type=int, default=600, help="Bound each MCP request/background job to60–600seconds")
    args = parser.parse_args()
    args.binary = [path.resolve(strict=True) for path in args.binary]
    if len(args.binary) != 2 or {path.name for path in args.binary} != {"editor", "hammerr-headless"}:
        parser.error("Specify both actual MCP programs exactly once")
    if sys.flags.optimize or not 60 <= args.timeout <= 600 or os.environ.get("LD_PRELOAD"):
        parser.error("Use unoptimized Python,60–600seconds and no inherited LD_PRELOAD")
    args.binary.sort(key=lambda path: path.name != "hammerr-headless")
    args.fgd = args.fgd.resolve(strict=True)
    if args.output.exists() or args.output.is_symlink():
        parser.error("--output must be fresh")
    args.output = args.output.resolve()
    if not args.output.is_relative_to(ROOT / "target"):
        parser.error("--output must stay on disk under target/")
    args.output.mkdir(parents=True)
    args.compile_calls, args.custody, args.errors = 0, {}, []
    watch(args.custody, [*args.binary, args.fgd, *(ROOT / "scripts" / name for name in HELPERS)])
    report = {"schema": 750, "validator_revision": 3, "success": False, "transport": "private_stdio", "programs": [],
        "script_sha256": digest(Path(__file__)), "capture_executed": False, "native_games": 0,
        "farther_manual_controls": declarations(),
        "literal_transforms": {"repeated": "(x-96,y,z+64); (x+96,y,z+64)",
            "nested": "(z+1048,x+952,y+1168); (z+1048,x+952,y+1232)",
            "nested_global_offset": NESTED_OFFSET},
        "literal_projection": {"source_u": U, "source_v": V, "scale": 0.25,
                               "source_offsets": SHIFT, "occurrences": LITERALS,
                               "nested_side_normals": NESTED_NORMALS},
        "pending_binding_plans": "Covered by compiler-library integration tests, not this MCP runner",
        "limits": "Ordinary sample-only BSPs do not apply sides to reflection aliases or capture pixels. No new native reflection, radiometry, runtime binding or capture-format claim."}
    try:
        for binary in args.binary:
            program = {}
            report["programs"].append(program)
            run_program(binary, args, args.output / binary.name, program)
        require({path: identity(Path(path)) for path in args.custody} == args.custody,
                "Final actual executable/helper/FGD/source/BSP/LIN custody differs")
        require(not args.errors and all(program["success"] is True for program in report["programs"]),
                "Every actual MCP program and cleanup must pass")
        report["success"] = True
    except BaseException as error:
        report.update({"success": False, "error": failure_details(error)})
        raise
    finally:
        report.update({"compile_calls": args.compile_calls,
                       "errors": [failure_details(error) for error in args.errors]})
        write_json(args.output / "results.json", report)


if __name__ == "__main__":
    main()
