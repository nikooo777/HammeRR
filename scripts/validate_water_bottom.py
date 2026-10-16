import argparse
import json
import struct
from pathlib import Path
from unittest.mock import patch

from validate_authored_planes import records
from validate_compile_jobs import wait_job
from validate_face_lighting import DEFAULT_BINARY, Client, detail
from validate_visibility import ROOM

TOP = "audit/water_top"
BOTTOM = "audit/water_below"
TOP_TEXT = "Patch { include materials/audit/water_base.vmt }\n"
SIDE = "audit/water_side"
SIDE_BOTTOM = "audit/water_side_below"
SIDE_TEXT = "Water { $abovewater 1 $bottommaterial audit/water_side_below }\n"


def fixture(output):
    game = output / "game"
    materials = game / "materials/audit"
    materials.mkdir(parents=True)
    for name, text in {
        "solid": "LightmappedGeneric {}",
        "nodraw": "UnlitGeneric { %compilenodraw 1 }",
        "water_base": 'Water { $abovewater 1 $bottommaterial "Materials\\audit\\water_below.vmt" $underwateroverlay effects/water_warp01 }',
        "water_top": TOP_TEXT,
        "water_below": "Patch { include materials/audit/water_below_base.vmt replace { $abovewater 0 } }",
        "water_below_base": "Water { $abovewater 1 }",
        "water_other_top": TOP_TEXT,
        "water_side": SIDE_TEXT,
        "water_side_below": "Water { $abovewater 0 }",
    }.items():
        (materials / f"{name}.vmt").write_text(text)
    gameinfo = game / "gameinfo.txt"
    gameinfo.write_text(
        'GameInfo { FileSystem { SearchPaths { Game "|gameinfo_path|." } } }\n'
    )
    return gameinfo, materials


def raw_lump(data, lump):
    offset, length = struct.unpack_from("<2i", data, 8 + lump * 16)
    return data[offset : offset + length]


def native_faces(data):
    infos = records(data, 6, "<16f2i")
    texdata = records(data, 2, "<3f5i")
    strings = raw_lump(data, 43)
    names = [
        strings[offset:].split(b"\0", 1)[0].decode().lower()
        for (offset,) in records(data, 44, "<i")
    ]
    faces = records(data, 7, "<HBBihhhh4Bif2i2ii2HI")
    materials = [names[texdata[infos[face[5]][17]][3]] for face in faces]
    return infos, faces, materials


def validate(client, output, materials, vertical_sides=False):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    client.call("new_map", {"discard_changes": True})
    for mins, maxs in ROOM:
        client.call(
            "create_block", {"mins": mins, "maxs": maxs, "material": "audit/solid"}
        )
    client.call(
        "create_entity", {"classname": "info_player_terrorist", "origin": [-192, 0, 8]}
    )
    client.call(
        "create_entity",
        {"classname": "info_player_counterterrorist", "origin": [192, 0, 8]},
    )
    pool = client.call(
        "create_block",
        {"mins": [-128, -128, 0], "maxs": [128, 128, 64], "material": "audit/nodraw"},
    )["id"]
    brush = detail(client, [pool])[0]
    top = next(face for face in brush["faces"] if face["normal"] == [0, 0, 1])
    client.call(
        "select_faces", {"faces": [{"solid": brush["solid"], "side": top["side"]}]}
    )
    client.call("edit_faces", {"material": TOP})
    client.call(
        "create_block",
        {"mins": [-16, -16, 0], "maxs": [16, 16, 96], "material": "audit/solid"},
    )
    for mins, maxs in [
        ([-32, -32, 64], [32, 32, 96]),
        ([64, 64, 64], [96, 96, 96]),
    ]:
        client.call(
            "create_block", {"mins": mins, "maxs": maxs, "material": "audit/solid"}
        )
    client.call(
        "set_compile_settings",
        {"lighting": "fullbright", "physics": "none", "visibility": "fast"},
    )
    source = output / "source.vmf"
    client.call("save_map", {"path": str(source)})
    saved = source.read_bytes()
    before = detail(client, [pool])
    history = client.call("get_map_info")["undo"]
    camera = client.call("get_camera")
    destination = output / "water-bottom.bsp"
    result = client.call("compile_map", {"path": str(destination)})
    check(
        result["success"] and result["stats"]["water_regions"] == 1,
        "Shared compile succeeds with one connected water region",
    )
    data = destination.read_bytes()
    infos, faces, names = native_faces(data)
    tops = [index for index, name in enumerate(names) if name == TOP]
    bottoms = [index for index, name in enumerate(names) if name == BOTTOM]
    planes = records(data, 1, "<4fi")
    water = records(data, 36, "<ffhh")
    check(
        bool(tops) and bool(bottoms),
        "Native faces bind top and patched bottom materials separately",
    )
    check(
        all(planes[faces[index][0]][2] == 1 for index in tops)
        and all(planes[faces[index][0]][2] == -1 for index in bottoms),
        "Top and underside have opposite native plane normals",
    )
    check(
        all(faces[index][7] == 0 for index in tops + bottoms),
        "Both water-facing materials retain the connected fog-volume ID",
    )
    check(
        all(
            infos[faces[index][5]][:17] == infos[water[0][2]][:17] for index in bottoms
        ),
        "Undersides preserve top texture/lightmap vectors and surface flags",
    )
    check(
        sum(faces[index][13] for index in tops)
        == sum(faces[index][13] for index in bottoms)
        == 256 * 256 - 64 * 64 - 32 * 32,
        "The column and touching centered/off-center covers clip both water surfaces equally",
    )
    check(
        (water[0][0], water[0][1]) == (64, 0)
        and water[0][2] in [faces[index][5] for index in tops],
        "Leaf-water metadata keeps original top texinfo and connected depth",
    )
    leaves = records(data, 10, "<ihh6h4Hhh", expected_version=1)
    leaf_faces = [index for (index,) in records(data, 16, "<H")]
    for selected, wet in [(tops, False), (bottoms, True)]:
        references = [
            leaf
            for leaf in leaves
            if set(selected).intersection(leaf_faces[leaf[9] : leaf[9] + leaf[10]])
        ]
        check(
            bool(references) and all(bool(leaf[0] & 32) == wet for leaf in references),
            f"{'Bottom' if wet else 'Top'} faces belong to {'submerged' if wet else 'dry'} leaves",
        )
    background = output / "water-bottom-background.bsp"
    client.call("start_compile", {"path": str(background)})
    job = wait_job(client)
    check(
        job["status"] == "completed" and background.read_bytes() == data,
        "Synchronous and background Session/MCP builds emit identical bytes",
    )
    check(
        source.read_bytes() == saved
        and detail(client, [pool]) == before
        and client.call("get_map_info")["undo"] == history
        and client.call("get_camera") == camera,
        "Water compilation preserves source, history, selection data and camera",
    )

    top_file = materials / "water_top.vmt"
    top_file.write_text("Water { $abovewater 1 }\n")
    one_sided = output / "one-sided.bsp"
    missing_key = client.call("compile_map", {"path": str(one_sided)})
    check(
        missing_key["success"]
        and any(
            "no $bottommaterial" in row["message"] for row in missing_key["diagnostics"]
        ),
        "Missing bottom key reports one-sided water without rejecting the existing workflow",
    )
    previous = one_sided.read_bytes()
    check(
        BOTTOM not in native_faces(previous)[2],
        "A missing bottom key produces no fallback underside material",
    )
    check(
        all(
            raw_lump(previous, lump) == raw_lump(data, lump)
            for lump in [18, 19, 36, 46]
        ),
        "Bottom faces leave brush collision, water depth and water distances unchanged",
    )

    for label, text in [
        ("missing", "Water { $abovewater 1 $bottommaterial audit/absent }"),
        ("malformed", "Water { $abovewater 1 $bottommaterial audit/broken }"),
    ]:
        top_file.write_text(text)
        (materials / "broken.vmt").write_text("Water { $abovewater }")
        failed = client.call("compile_map", {"path": str(destination)}, error=True)
        failed = json.loads(failed["content"][0]["text"])
        check(
            not failed["success"]
            and destination.read_bytes() == data
            and any(
                row["level"] == "error"
                and "$bottommaterial" in row["message"]
                and row["object"] == pool
                for row in failed["diagnostics"]
            ),
            f"Explicit {label} bottom asset fails with its water object and preserves old output",
        )
        client.call("start_compile", {"path": str(destination)})
        job = wait_job(client)
        check(
            job["status"] == "failed" and destination.read_bytes() == data,
            f"Background rejection of {label} bottom asset preserves old output",
        )

    top_file.write_text(TOP_TEXT)
    recovered = client.call("compile_map", {"path": str(output / "recovered.bsp")})
    check(
        recovered["success"] and (output / "recovered.bsp").read_bytes() == data,
        "Repairing the mounted asset restores deterministic output without source edits",
    )
    check(
        source.read_bytes() == saved
        and detail(client, [pool]) == before
        and client.call("get_map_info")["undo"] == history,
        "Rejected builds and asset repair preserve authored water and history",
    )
    if vertical_sides:
        checks.extend(validate_vertical_sides(client, output, materials))
    return checks


def validate_vertical_sides(client, output, materials):
    checks = []

    def check(condition, label):
        assert condition, label
        checks.append(label)

    client.call("new_map", {"discard_changes": True})
    for mins, maxs in ROOM:
        client.call(
            "create_block", {"mins": mins, "maxs": maxs, "material": "audit/solid"}
        )
    client.call(
        "create_entity", {"classname": "info_player_terrorist", "origin": [-192, 0, 8]}
    )
    client.call(
        "create_entity",
        {"classname": "info_player_counterterrorist", "origin": [192, 0, 8]},
    )
    pools = []
    for mins, maxs, material, inner in [
        ([-128, -128, 0], [0, 128, 64], TOP, [1, 0, 0]),
        ([0, -128, 0], [128, 128, 64], "audit/water_other_top", [-1, 0, 0]),
    ]:
        pool = client.call(
            "create_block", {"mins": mins, "maxs": maxs, "material": "audit/nodraw"}
        )["id"]
        pools.append(pool)
        brush = detail(client, [pool])[0]
        for normals, chosen in [([[0, 0, 1]], material), ([[0, -1, 0], inner], SIDE)]:
            selected = [
                {"solid": brush["solid"], "side": face["side"]}
                for face in brush["faces"]
                if face["normal"] in normals
            ]
            assert len(selected) == len(normals)
            client.call("select_faces", {"faces": selected})
            client.call("edit_faces", {"material": chosen})
    client.call(
        "set_compile_settings",
        {"lighting": "fullbright", "physics": "none", "visibility": "fast"},
    )
    source = output / "vertical-source.vmf"
    client.call("save_map", {"path": str(source)})
    saved = source.read_bytes()
    before = detail(client, pools)
    history = client.call("get_map_info")["undo"]
    camera = client.call("get_camera")
    destination = output / "water-vertical.bsp"
    result = client.call("compile_map", {"path": str(destination)})
    check(
        result["success"] and result["stats"]["water_regions"] == 2,
        "MCP-authored vertical water preserves two top-material regions",
    )
    data = destination.read_bytes()
    infos, faces, names = native_faces(data)
    inward = [i for i, name in enumerate(names) if name == SIDE_BOTTOM]
    outward = [i for i, name in enumerate(names) if name == SIDE]
    planes = records(data, 1, "<4fi")
    check(
        bool(inward) and all(planes[faces[i][0]][:3] == (0, 1, 0) for i in inward),
        "Vertical inward faces exclude the internal wet-to-wet boundary",
    )
    check(
        bool(outward) and all(planes[faces[i][0]][:3] == (0, -1, 0) for i in outward),
        "Outward vertical faces exclude the internal wet-to-wet boundary",
    )
    check(
        all(
            len({faces[i][7] for i in selected}) == 2
            and all(
                sum(faces[i][13] for i in selected if faces[i][7] == region) == 128 * 64
                for region in range(2)
            )
            for selected in [inward, outward]
        ),
        "Both directions retain their own wet region before merging",
    )
    side_infos = {
        tuple(infos[faces[i][5]][:17])
        for i in outward
        if planes[faces[i][0]][:3] == (0, -1, 0)
    }
    check(
        len(side_infos) == 1
        and all(tuple(infos[faces[i][5]][:17]) in side_infos for i in inward),
        "Vertical faces use the side material's own projection and flags",
    )
    leaves = records(data, 10, "<ihh6h4Hhh", expected_version=1)
    leaf_faces = [index for (index,) in records(data, 16, "<H")]
    for selected, wet in [(inward, True), (outward, False)]:
        for i in selected:
            references = [
                leaf for leaf in leaves if i in leaf_faces[leaf[9] : leaf[9] + leaf[10]]
            ]
            check(
                bool(references)
                and all(
                    bool(leaf[0] & 32) == wet and (not wet or leaf[13] == faces[i][7])
                    for leaf in references
                ),
                "Paired vertical faces retain dry/outside and checked submerged references",
            )
    background = output / "water-vertical-background.bsp"
    client.call("start_compile", {"path": str(background)})
    check(
        wait_job(client)["status"] == "completed" and background.read_bytes() == data,
        "Background vertical water compilation matches synchronous bytes",
    )
    side_file = materials / "water_side.vmt"
    side_file.write_text("Water { $abovewater 1 $bottommaterial audit/absent }\n")
    rejected = client.call("compile_map", {"path": str(destination)}, error=True)
    rejected = json.loads(rejected["content"][0]["text"])
    check(
        not rejected["success"]
        and destination.read_bytes() == data
        and any(
            row["level"] == "error"
            and "$bottommaterial" in row["message"]
            and row["object"] in pools
            for row in rejected["diagnostics"]
        ),
        "A bad side-specific bottom asset fails atomically at its authored water solid",
    )
    side_file.write_text(SIDE_TEXT)
    check(
        client.call("compile_map", {"path": str(destination)})["success"]
        and destination.read_bytes() == data,
        "Repairing the side asset restores deterministic output",
    )
    check(
        source.read_bytes() == saved
        and detail(client, pools) == before
        and client.call("get_map_info")["undo"] == history
        and client.call("get_camera") == camera,
        "Vertical builds and rejected assets preserve authored source, history, selection data and camera",
    )
    return checks


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument("--vertical-sides", action="store_true")
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-water-bottom")
    )
    args = parser.parse_args()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True)
    gameinfo, materials = fixture(args.output)
    with patch.dict(
        "os.environ",
        {
            "HAMMERR_RECENT_MAPS_DIR": str(args.output / "recent"),
            "HAMMERR_BUILD_RECIPES": str(args.output / "recipes.json"),
        },
    ):
        client = Client(None, args.binary.resolve(), args.output, gameinfo=gameinfo)
    try:
        checks = validate(client, args.output, materials, args.vertical_sides)
        report = {"checks": len(checks), "passed": checks, "desktop_input": False}
        (args.output / "results.json").write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps({"checks": len(checks), "output": str(args.output)}))
    finally:
        client.close()


if __name__ == "__main__":
    main()
