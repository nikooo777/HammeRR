import argparse
import json
import struct
from pathlib import Path

from validate_authored_planes import records
from validate_compile_jobs import wait_job
from validate_engine_physics import physics_records
from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_visibility import ROOM


def validate(client, output, physics="none"):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    client.call("set_compile_settings", {"physics": physics})

    def room():
        client.call("new_map", {"discard_changes": True})
        for mins, maxs in ROOM:
            client.call(
                "create_block",
                {"mins": mins, "maxs": maxs, "material": "dev/dev_measuregeneric01"},
            )
        for classname, origin in [
            ("info_player_terrorist", [-64, -64, 8]),
            ("info_player_counterterrorist", [64, -64, 8]),
        ]:
            client.call("create_entity", {"classname": classname, "origin": origin})

    def pool(mins, maxs, material="liquids/aztecwater"):
        body = client.call(
            "create_block",
            {"mins": mins, "maxs": maxs, "material": "tools/toolsnodraw"},
        )["id"]
        brush = detail(client, [body])[0]
        top = next(face for face in brush["faces"] if face["normal"] == [0, 0, 1])
        client.call(
            "select_faces", {"faces": [{"solid": brush["solid"], "side": top["side"]}]}
        )
        client.call("edit_faces", {"material": material})
        return body

    for material in ["liquids/aztecwater", "dev/dev_water2"]:
        check(
            not client.call("preview_material", {"material": material})["missing"],
            f"Installed CS:S liquid material resolves: {material}",
        )
    for label, expected in [
        ("pool", 1),
        ("connected", 1),
        ("height", 2),
        ("material", 2),
    ]:
        room()
        if label == "pool":
            ids = [pool([-128, -128, 0], [128, 128, 64])]
        else:
            ids = [pool([-128, -128, 0], [0, 128, 64])]
            ids.append(
                pool(
                    [0, -128, 32],
                    [128, 128, 96 if label == "height" else 64],
                    "dev/dev_water2" if label == "material" else "liquids/aztecwater",
                )
            )
        vmf = output / f"{label}.vmf"
        client.call("save_map", {"path": str(vmf)})
        client.call("open_map", {"path": str(vmf)})
        source = vmf.read_bytes()
        before = detail(client, ids)
        history = client.call("get_map_info")["undo"]
        bsp = output / f"{label}.bsp"
        result = client.call("compile_map", {"path": str(bsp)})
        check(
            result["success"]
            and result["stats"]["water_regions"] == expected
            and result["stats"]["water_leaves"] > 0,
            f"{label}: shared compile reports native fluid regions and leaves",
        )
        data = bsp.read_bytes()
        if physics == "engine":
            native = physics_records(data)
            keys = native[0][2].decode()
            check(
                result["stats"]["physics_fluids"] == expected
                and keys.count("fluid {") == expected,
                f"{label}: native fluid bodies match connected leaf regions",
            )
            check(
                keys.count('"surfaceprop" "water"') == expected
                and keys.count('"damping" "0.01"') == expected
                and keys.count('"currentvelocity" "0 0 0"') == expected,
                f"{label}: fluid scripts retain native surface, damping and velocity defaults",
            )
        water = records(data, 36, "<ffhh")
        leaves = records(data, 10, "<ihh6h4Hhh", expected_version=1)
        distances = records(data, 46, "<H")
        infos = records(data, 6, "<16f2i")
        faces = records(data, 7, "<HBBihhhh4Bif2i2ii2HI")
        check(
            len(water) == expected
            and all(
                surface >= bottom and padding == 0 and 0 <= texinfo < len(infos)
                for surface, bottom, texinfo, padding in water
            ),
            f"{label}: native twelve-byte records have valid surface/depth/texinfo references",
        )
        check(
            all(infos[texinfo][16] & 8 for _, _, texinfo, _ in water),
            f"{label}: fluid records reference warp material texinfos",
        )
        wet = [
            (leaf, distance[0])
            for leaf, distance in zip(leaves, distances, strict=True)
            if leaf[-2] >= 0
        ]
        check(
            len(wet) == result["stats"]["water_leaves"]
            and all(
                0 <= leaf[-2] < len(water)
                and leaf[0] & 32
                and not leaf[0] & 1
                and distance == 0
                for leaf, distance in wet
            ),
            f"{label}: submerged leaves use fluid records and zero distance",
        )
        water_faces = [face for face in faces if infos[face[5]][16] & 8]
        check(
            bool(water_faces)
            and all(0 <= face[7] < len(water) for face in water_faces),
            f"{label}: water surfaces carry native fog-volume IDs",
        )
        check(
            records(data, 27, "<HBBihhhh4Bif2i2ii2HI") == faces,
            f"{label}: original faces preserve the same fog references",
        )
        offset, length = struct.unpack_from("<2i", data, 8)
        entities = data[offset : offset + length].decode()
        check(
            entities.count('"classname" "water_lod_control"') == 1
            and '"cheapwaterstartdistance" "1000"' in entities,
            f"{label}: compiler supplies one native default LOD control",
        )
        check(
            any(
                "depth-specific material" in diagnostic["message"]
                for diagnostic in result["diagnostics"]
            ),
            f"{label}: diagnostics explain the remaining material and rendering work",
        )
        check(
            before == detail(client, ids)
            and history == client.call("get_map_info")["undo"]
            and source == vmf.read_bytes(),
            f"{label}: compilation preserves source and history",
        )
        background = output / f"{label}-background.bsp"
        client.call("start_compile", {"path": str(background)})
        job = wait_job(client)
        check(
            job["status"] == "completed"
            and job["result"]["stats"]["water_regions"] == expected
            and job["result"]["stats"]["water_leaves"] == len(wet),
            f"{label}: background Session/MCP metrics match synchronous output",
        )
        check(
            background.read_bytes() == data,
            f"{label}: both compile APIs emit identical fluid records",
        )
    room()
    body = pool([-128, -128, 16], [128, 128, 64])
    client.call("set_object_visibility", {"ids": [body], "action": "hide"})
    hidden = client.call("compile_map", {"path": str(output / "hidden.bsp")})
    check(
        hidden["stats"]["water_regions"] == 0 and hidden["stats"]["water_leaves"] == 0,
        "Hidden liquids receive no native regions",
    )
    if physics == "engine":
        check(
            hidden["stats"]["physics_fluids"] == 0,
            "Hidden liquids receive no physics fluid bodies",
        )
    client.call("undo")
    client.call("transform_objects", {"ids": [body], "rotation": [0, 15, 0]})
    destination = output / "sloped.bsp"
    destination.write_bytes(b"previous successful build")
    failed = client.call("compile_map", {"path": str(destination)}, error=True)
    diagnostics = json.loads(failed["content"][0]["text"])["diagnostics"]
    check(
        any(
            diagnostic["level"] == "error"
            and diagnostic["object"] == body
            and "horizontal top" in diagnostic["message"]
            for diagnostic in diagnostics
        ),
        "Unsupported sloped liquids fail with object diagnostics",
    )
    check(
        destination.read_bytes() == b"previous successful build",
        "Rejected liquid compilation retains previous output",
    )
    client.call("undo")
    recovered = client.call("compile_map", {"path": str(output / "recovered.bsp")})
    check(
        recovered["success"] and recovered["stats"]["water_regions"] == 1,
        "Undo restores successful native liquid compilation",
    )
    client.call("tie_to_entity", {"ids": [body], "classname": "func_brush"})
    model = client.call("compile_map", {"path": str(output / "submodel.bsp")})
    check(
        model["stats"]["water_regions"] == 0
        and any(
            "brush-model fluid" in diagnostic["message"]
            for diagnostic in model["diagnostics"]
        ),
        "Unsupported brush-model fluid metadata has an explicit diagnostic",
    )
    if physics == "engine":
        check(
            model["stats"]["physics_fluids"] == 0,
            "Brush-model liquids receive no placeholder fluid bodies",
        )
    return checks


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-water-metadata")
    )
    parser.add_argument("--physics", choices=["none", "engine"], default="none")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    client = Client(None, args.binary, args.output)
    try:
        checks = validate(client, args.output, args.physics)
        (args.output / "results.json").write_text(
            json.dumps({"checks": len(checks), "passed": checks}, indent=2) + "\n"
        )
        print(json.dumps({"checks": len(checks), "output": str(args.output)}))
    finally:
        client.close()


if __name__ == "__main__":
    main()
