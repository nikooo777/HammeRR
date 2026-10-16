import argparse
import hashlib
import json
import math
import struct
from pathlib import Path
from unittest.mock import patch

from validate_authored_planes import records
from validate_compile_jobs import stopped_on_exit, wait_job
from validate_engine_physics import physics_records
from validate_face_lighting import DEFAULT_BINARY, Client, detail, face_ref
from validate_physics_surfaces import assets
from validate_terrain_lod import build
from validate_visibility import ROOM

LANE_CENTERS = (-96, 96)
POWER = 3
GRID_SIZE = (1 << POWER) + 1
TRIANGLES_PER_LANE = 2 * (GRID_SIZE - 1) ** 2
ALPHA_SUM_THRESHOLD = 382.5
GEOMETRY_LUMPS = (1, 3, 5, 7, 10, 12, 13, 14, 16, 17, 18, 19, 26, 48)


def lump(data, index):
    offset, size, version, compression = struct.unpack_from("<4i", data, 8 + index * 16)
    assert (
        version >= 0 and compression == 0 and 0 <= offset <= offset + size <= len(data)
    )
    return data[offset : offset + size]


def native_float(value):
    return struct.unpack("<f", struct.pack("<f", value))[0]


def materials(args):
    game = assets(args)
    folder = game / "materials/hammerr"
    for name, secondary in [
        ("blend", "rubber"),
        ("primary", None),
        ("empty", ""),
        ("unsafe", "unsafe/name"),
        ("unknown", "hammerr_missing_secondary_prop"),
    ]:
        prop = f' $surfaceprop2 "{secondary}"' if secondary is not None else ""
        (folder / f"terrain_{name}.vmt").write_text(
            "WorldVertexTransition { $basetexture dev/dev_measuregeneric01 "
            "$basetexture2 dev/dev_measuregeneric01 $surfaceprop ice" + prop + " }\n"
        )
    return game


def terrain(client, faces, preview=False):
    return client.call("get_terrain_grids", {"faces": faces, "preview": preview})[
        "faces"
    ]


def snapshot(client, ids, faces):
    return {
        "objects": detail(client, ids),
        "terrain": terrain(client, faces),
        "undo": client.call("get_map_info")["undo"],
    }


def blend(client, ids, faces, face, mode, value, center=None):
    before = snapshot(client, ids, faces)
    arguments = {"faces": [face], "mode": mode, "value": value}
    if center is not None:
        arguments.update({"center": center, "radius": 1, "strength": 1})
    draft = client.call("preview_terrain_blend", arguments)
    assert snapshot(client, ids, faces) == before
    preview = terrain(client, faces, preview=True)
    client.call("commit_face_preview", {"token": draft["state"]["preview_token"]})
    assert terrain(client, faces) == preview


def create_fixture(client):
    ids = [
        client.call(
            "create_block",
            {"mins": low, "maxs": high, "material": "hammerr/surface_default"},
        )["id"]
        for low, high in ROOM
    ]
    faces = []
    for center in LANE_CENTERS:
        parent = client.call(
            "create_block",
            {
                "mins": [-192, center - 64, 32],
                "maxs": [192, center + 64, 48],
                "material": "hammerr/surface_default",
            },
        )["id"]
        ids.append(parent)
        shape = detail(client, [parent])[0]
        top = next(face for face in shape["faces"] if face["normal"][2] > 0.99)
        face = face_ref(shape, top)
        faces.append(face)
        client.call(
            "edit_faces", {"faces": [face], "material": "hammerr/terrain_blend"}
        )
        client.call(
            "create_terrain", {"faces": [face], "power": POWER, "start_corner": 2}
        )
        client.call(
            "transform_objects",
            {"ids": [parent], "rotation": [0, 12, 0], "pivot": [0, center, 40]},
        )
    for classname, origin, properties in [
        ("info_player_terrorist", [-224, -208, 8], {}),
        ("info_player_counterterrorist", [224, 208, 8], {}),
        ("light", [0, 0, 180], {"_light": "255 255 255 120", "_constant_attn": "1"}),
    ]:
        ids.append(
            client.call(
                "create_entity",
                {"classname": classname, "origin": origin, "properties": properties},
            )["id"]
        )
    client.call("set_compile_settings", {"lighting": "direct_ldr", "physics": "engine"})
    grids = terrain(client, faces)
    assert len(grids) == len(faces) == len(LANE_CENTERS)
    assert all(grid["grid_size"] == GRID_SIZE for grid in grids)
    for grid, face in zip(grids, faces, strict=True):
        assert (grid["solid"], grid["side"]) == (face["solid"], face["side"])
    for first, second in zip(grids[0]["positions"], grids[1]["positions"], strict=True):
        assert math.isclose(first[0], second[0], abs_tol=1e-6)
        assert math.isclose(
            first[1] + LANE_CENTERS[1] - LANE_CENTERS[0],
            second[1],
            abs_tol=1e-6,
        )
        assert math.isclose(first[2], second[2], abs_tol=1e-6)
    assert (
        max(point[2] for point in grids[0]["positions"])
        - min(point[2] for point in grids[0]["positions"])
        > 70
    )
    return ids, faces


def selected_triangles(client, faces, grids, secondary):
    triangles = client.call("get_terrain_triangles", {"faces": faces})["faces"]
    counts = []
    for grid, row in zip(grids, triangles, strict=True):
        assert (row["solid"], row["side"]) == (grid["solid"], grid["side"])
        assert len(row["triangles"]) == TRIANGLES_PER_LANE
        count = 0
        for triangle in row["triangles"]:
            alpha = 0.0
            for index in triangle["indices"]:
                alpha = native_float(alpha + native_float(grid["alphas"][index]))
            count += secondary and alpha > ALPHA_SUM_THRESHOLD
        counts.append(count)
    return counts


def compile_case(client, args, ids, faces, name, secondary=True):
    grids = terrain(client, faces)
    selected = selected_triangles(client, faces, grids, secondary)
    source = args.output / f"{name}.vmf"
    client.call("save_map", {"path": str(source)})
    saved = source.read_bytes()
    client.call("open_map", {"path": str(source)})
    reopened = terrain(client, faces)
    for old, new in zip(grids, reopened, strict=True):
        assert old["alphas"] == new["alphas"]
        assert all(
            math.isclose(a, b, abs_tol=1e-5)
            for first, second in zip(old["positions"], new["positions"], strict=True)
            for a, b in zip(first, second, strict=True)
        )
    before = snapshot(client, ids, faces)
    path, stats = build(client, args.output, name)
    assert snapshot(client, ids, faces) == before and source.read_bytes() == saved
    data = path.read_bytes()
    models = physics_records(data)
    assert len(models) == stats["physics_models"] == 1
    keys = models[0][2].decode()
    assert '"ice" "2"' in keys and '"default" "1"' in keys, keys
    uses_secondary = any(selected)
    assert stats["physics_surface_properties"] == (3 if uses_secondary else 2), stats
    assert ('"rubber" "3"' in keys) == uses_secondary, keys
    assert (
        stats["physics_mesh_triangles"]
        == stats["displacement_triangles"]
        == (len(faces) * TRIANGLES_PER_LANE)
    )
    vertices = records(data, 33, "<5f")
    assert len(vertices) == len(faces) * GRID_SIZE**2
    for index, grid in enumerate(reopened):
        emitted = vertices[index * GRID_SIZE**2 : (index + 1) * GRID_SIZE**2]
        assert [row[4] for row in emitted] == list(map(native_float, grid["alphas"]))
    return path, {
        "secondary_triangles_by_lane": selected,
        "stats": stats,
        "physics_sha256": hashlib.sha256(lump(data, 29)).hexdigest(),
        "saved_reopened": True,
        "foreground_background_exact": True,
        "source_history_preserved": True,
    }


def rejected_cases(client, args, ids, faces, valid):
    for face in faces:
        blend(client, ids, faces, face, "fill", 0)
    destination = args.output / "rejected.bsp"
    original = valid.read_bytes()
    destination.write_bytes(original)
    failures = {}
    for name, message in [
        ("empty", "must use 1–127 ASCII"),
        ("unsafe", "must use 1–127 ASCII"),
        ("unknown", "is not defined by the mounted surface manifest"),
    ]:
        client.call(
            "edit_faces", {"faces": faces, "material": f"hammerr/terrain_{name}"}
        )
        before = snapshot(client, ids, faces)
        failed = client.call("compile_map", {"path": str(destination)}, error=True)
        diagnostics = json.loads(failed["content"][0]["text"])["diagnostics"]
        assert any(
            "Terrain secondary physics surface property" in row["message"]
            and message in row["message"]
            for row in diagnostics
        ), diagnostics
        assert (
            destination.read_bytes() == original
            and snapshot(client, ids, faces) == before
        )
        client.call("start_compile", {"path": str(destination)})
        job = wait_job(client)
        assert job["status"] == "failed", job
        assert message in json.dumps(job, ensure_ascii=False), job
        assert (
            destination.read_bytes() == original
            and snapshot(client, ids, faces) == before
        )
        failures[name] = {
            "foreground_background_rejected": True,
            "previous_bsp_preserved": True,
            "zero_alpha_still_validates_secondary": True,
        }
    return failures


def fixture(args, game):
    with patch.dict(
        "os.environ",
        {
            "HAMMERR_RECENT_MAPS_DIR": str(args.output / "recent-maps"),
            "HAMMERR_BUILD_RECIPES": str(args.output / "recipes.json"),
        },
    ):
        client = Client(None, args.binary, args.output, game / "gameinfo.txt")
    try:
        with stopped_on_exit(client):
            ids, faces = create_fixture(client)
            paths, results = {}, {}
            for name, alphas, material in [
                ("primary", [0, 0], "blend"),
                ("threshold", [127.5, 127.5], "blend"),
                ("rounded-threshold", [127.5000001, 127.5000001], "blend"),
                ("secondary", [255, 255], "blend"),
                ("above-threshold", [127.50001, 127.50001], "blend"),
                ("absent-secondary", [255, 255], "primary"),
                ("paired", [0, 255], "blend"),
                ("swapped", [255, 0], "blend"),
            ]:
                client.call(
                    "edit_faces",
                    {"faces": faces, "material": f"hammerr/terrain_{material}"},
                )
                for face, alpha in zip(faces, alphas, strict=True):
                    blend(client, ids, faces, face, "fill", alpha)
                grids = terrain(client, faces)
                assert all(
                    all(value == alpha for value in grid["alphas"])
                    for grid, alpha in zip(grids, alphas, strict=True)
                )
                paths[name], results[name] = compile_case(
                    client, args, ids, faces, name, secondary=material == "blend"
                )
                expected = [
                    TRIANGLES_PER_LANE
                    if material == "blend" and native_float(alpha) > 127.5
                    else 0
                    for alpha in alphas
                ]
                assert results[name]["secondary_triangles_by_lane"] == expected
            for face in faces:
                blend(client, ids, faces, face, "fill", 127.5)
            grids = terrain(client, faces)
            center_index = GRID_SIZE**2 // 2
            for face, grid, value in zip(faces, grids, [255, 0], strict=True):
                blend(
                    client,
                    ids,
                    faces,
                    face,
                    "paint",
                    value,
                    grid["positions"][center_index],
                )
            painted = terrain(client, faces)
            for old, new, value in zip(grids, painted, [255, 0], strict=True):
                assert old["positions"] == new["positions"]
                assert new["alphas"] == [
                    value if index == center_index else 127.5
                    for index in range(GRID_SIZE**2)
                ]
            paths["painted"], results["painted"] = compile_case(
                client, args, ids, faces, "painted"
            )
            first, second = results["painted"]["secondary_triangles_by_lane"]
            assert 0 < first < TRIANGLES_PER_LANE and second == 0
            data = {name: path.read_bytes() for name, path in paths.items()}
            baseline = records(data["primary"], 33, "<5f")
            for name, bsp in data.items():
                assert all(
                    lump(bsp, index) == lump(data["primary"], index)
                    for index in GEOMETRY_LUMPS
                ), name
                assert [row[:4] for row in records(bsp, 33, "<5f")] == [
                    row[:4] for row in baseline
                ], name
            assert (
                len(
                    {
                        lump(data[name], 29)
                        for name in [
                            "primary",
                            "threshold",
                            "rounded-threshold",
                            "absent-secondary",
                        ]
                    }
                )
                == 1
            )
            assert lump(data["secondary"], 29) == lump(data["above-threshold"], 29)
            assert (
                len(
                    {
                        lump(data[name], 29)
                        for name in [
                            "primary",
                            "secondary",
                            "paired",
                            "swapped",
                            "painted",
                        ]
                    }
                )
                == 5
            )
            failures = rejected_cases(client, args, ids, faces, paths["paired"])
            report = {
                "cases": results,
                "rejections": failures,
                "matching_geometry": True,
                "strict_f32_threshold": True,
                "missing_secondary_preserves_primary": True,
                "paint_preview_and_reopen": True,
                "native_mapping_verification": "Compiler checks both collision sides before and after native serialization/reload",
                "game_contact_exercised": False,
            }
            (args.output / "results.json").write_text(
                json.dumps(report, indent=2) + "\n"
            )
    finally:
        client.close()


def main():
    parser = argparse.ArgumentParser(
        description="Check terrain alpha-selected native physics through shared MCP builds; does not launch the game"
    )
    parser.add_argument("--binary", type=Path, default=DEFAULT_BINARY)
    parser.add_argument(
        "--output", type=Path, default=Path("/tmp/hammerr-terrain-physics-surfaces")
    )
    parser.add_argument(
        "--game-dir",
        type=Path,
        default=Path.home()
        / ".local/share/Steam/steamapps/common/Counter-Strike Source",
    )
    args = parser.parse_args()
    args.output, args.binary, args.game_dir = (
        args.output.resolve(),
        args.binary.resolve(),
        args.game_dir.resolve(),
    )
    args.output.mkdir(parents=True, exist_ok=True)
    game = materials(args)
    fixture(args, game)
    print(
        json.dumps(
            {
                "output": str(args.output),
                "terrain_secondary_surfaces": True,
                "game_contact_exercised": False,
            }
        )
    )


if __name__ == "__main__":
    main()
