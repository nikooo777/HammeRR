#[allow(dead_code)]
mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use compiler::instances::{self, PreparedInstances, SourceContext};
use compiler::{Control, LightingMode, Options};
use glam::DVec3;
use map_core::{Document, ObjectId, edit};
use source_formats::bsp::{self, Bsp, Edge, Face};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::keyvalues::Node;
use source_formats::vmf::{DispInfo, Entity, Solid, UvAxis, Vmf, format_vec3};

type Bounds = ([f64; 3], [f64; 3]);
const SOURCE: [Bounds; 3] = [
    ([-40.0, -24.0, 16.0], [-8.0, 24.0, 48.0]),
    ([8.0, -16.0, 16.0], [40.0, 16.0, 32.0]),
    ([-16.0, 48.0, 8.0], [16.0, 64.0, 16.0]),
];
const LITERAL: [[Bounds; 3]; 2] = [
    [
        ([1016.0, 1080.0, 1160.0], [1064.0, 1112.0, 1192.0]),
        ([1024.0, 1080.0, 1112.0], [1056.0, 1096.0, 1144.0]),
        ([976.0, 1072.0, 1136.0], [992.0, 1080.0, 1168.0]),
    ],
    [
        ([888.0, 936.0, 1160.0], [936.0, 968.0, 1192.0]),
        ([896.0, 936.0, 1112.0], [928.0, 952.0, 1144.0]),
        ([848.0, 928.0, 1136.0], [864.0, 936.0, 1168.0]),
    ],
];
const LOCAL: [[f64; 3]; 4] = [
    [-12.0, -10.0, 17.0],
    [-12.0, 10.0, 9.0],
    [12.0, 10.0, 7.0],
    [12.0, -10.0, 13.0],
];

fn next_id(vmf: &Vmf) -> u32 {
    vmf.object_ids().chain(vmf.side_ids()).max().unwrap() + 1
}

fn solid_mut(vmf: &mut Vmf, id: u32) -> &mut Solid {
    vmf.world
        .solids
        .iter_mut()
        .chain(
            vmf.entities
                .iter_mut()
                .flat_map(|entity| &mut entity.solids),
        )
        .find(|solid| solid.id == id)
        .unwrap()
}

fn entity_mut(vmf: &mut Vmf, id: u32) -> &mut Entity {
    vmf.entities
        .iter_mut()
        .find(|entity| entity.id == id)
        .unwrap()
}

fn block(document: &mut Document, bounds: Bounds, detail: bool) -> u32 {
    document
        .edit("target", |tx| {
            let ObjectId::Solid(id) = edit::create_block(
                tx,
                DVec3::from_array(bounds.0),
                DVec3::from_array(bounds.1),
                "dev/dev_measuregeneric01",
            )?
            else {
                unreachable!()
            };
            for side in &mut tx.solid_mut(id)?.sides {
                side.lightmap_scale = 128;
            }
            if detail {
                edit::tie_to_entity(tx, &[id], "func_detail", &[])?;
            }
            Ok(id)
        })
        .unwrap()
}

fn room() -> Vmf {
    let mut document = Document::empty();
    for bounds in [
        ([752.0, 752.0, 1008.0], [1296.0, 1296.0, 1024.0]),
        ([752.0, 752.0, 1280.0], [1296.0, 1296.0, 1296.0]),
        ([752.0, 752.0, 1024.0], [768.0, 1296.0, 1280.0]),
        ([1280.0, 752.0, 1024.0], [1296.0, 1296.0, 1280.0]),
        ([768.0, 752.0, 1024.0], [1280.0, 768.0, 1280.0]),
        ([768.0, 1280.0, 1024.0], [1280.0, 1296.0, 1280.0]),
    ] {
        block(&mut document, bounds, false);
    }
    document
        .edit("players and light", |tx| {
            for (class, point) in [
                ("info_player_terrorist", [960.0, 1024.0, 1032.0]),
                ("info_player_counterterrorist", [1088.0, 1024.0, 1032.0]),
            ] {
                edit::create_entity(tx, class, Some(DVec3::from_array(point)), &[], Vec::new())?;
            }
            edit::create_entity(
                tx,
                "light",
                Some(DVec3::new(1100.0, 1000.0, 1232.0)),
                &[
                    ("_light".into(), "255 192 96 160".into()),
                    ("_lightHDR".into(), "255 192 96 160".into()),
                    ("_constant_attn".into(), "1".into()),
                    ("_quadratic_attn".into(), "0".into()),
                ],
                Vec::new(),
            )?;
            Ok(())
        })
        .unwrap();
    document.into_vmf()
}

fn terrain(power: u32, index: usize, bounds: Bounds, literal: bool) -> DispInfo {
    let size = (1_usize << power) + 1;
    let normal = if literal {
        DVec3::new(-2.0, 3.0, -1.0)
    } else {
        DVec3::new(1.0, 2.0, 3.0)
    }
    .normalize();
    let offset_normal = if literal {
        DVec3::new(-1.0, 2.0, -3.0)
    } else {
        DVec3::new(3.0, 1.0, 2.0)
    }
    .normalize();
    let mut info = DispInfo {
        power,
        start_position: if literal {
            DVec3::new(bounds.1[0], bounds.1[1], bounds.1[2])
        } else {
            DVec3::new(bounds.0[0], bounds.0[1], bounds.1[2])
        },
        flags: Some(0),
        elevation: 0.5,
        subdivided: true,
        normals: Some(vec![vec![normal; size]; size]),
        distances: Some(
            (0..size)
                .map(|row| {
                    (0..size)
                        .map(|col| (1 + row * 2 + col + row * col % 5) as f64 / 16.0)
                        .collect()
                })
                .collect(),
        ),
        offsets: Some(
            (0..size)
                .map(|row| {
                    (0..size)
                        .map(|col| {
                            let x = (row + 1) as f64 / 32.0;
                            let y = (col + 1) as f64 / 64.0;
                            let z = (row + col + 1) as f64 / 128.0;
                            if literal {
                                DVec3::new(-y, z, -x)
                            } else {
                                DVec3::new(x, y, z)
                            }
                        })
                        .collect()
                })
                .collect(),
        ),
        offset_normals: Some(vec![vec![offset_normal; size]; size]),
        alphas: Some(
            (0..size)
                .map(|row| {
                    (0..size)
                        .map(|col| ((row * 11 + col * 7 + index * 23) % 256) as f64)
                        .collect()
                })
                .collect(),
        ),
        triangle_tags: Some(
            (0..size - 1)
                .map(|row| {
                    (0..2 * (size - 1))
                        .map(|col| {
                            18 | if row % 2 == 0 { 4 } else { 0 }
                                | if col % 2 == 0 { 32 } else { 0 }
                        })
                        .collect()
                })
                .collect(),
        ),
        allowed_verts: Some(vec![-1; 10]),
        extra_properties: Default::default(),
        extra: Vec::new(),
    };
    info.extra_properties.set("smooth", "32");
    info
}

fn texture(solid: &mut Solid, occurrence: Option<usize>) {
    for side in &mut solid.sides {
        side.u_axis = UvAxis {
            axis: if occurrence.is_some() {
                DVec3::new(-1.0, 1.0, -1.0)
            } else {
                DVec3::ONE
            }
            .normalize(),
            offset: occurrence.map_or(0.0, |index| [1128.0, 1144.0][index] / 3.0_f64.sqrt()),
            scale: 1.0,
        };
        side.v_axis = UvAxis {
            axis: if occurrence.is_some() {
                DVec3::new(-2.0, 3.0, -1.0)
            } else {
                DVec3::new(1.0, 2.0, 3.0)
            }
            .normalize(),
            offset: occurrence.map_or(0.0, |index| [40.0, 216.0][index] / 14.0_f64.sqrt()),
            scale: 1.0,
        };
    }
}

fn overlay(vmf: &mut Vmf, sides: &[u32], index: usize, occurrence: Option<usize>) -> u32 {
    let id = next_id(vmf);
    let mut entity = Entity::new(id, "info_overlay");
    entity.properties.set(
        "material",
        if index == 0 {
            "decals/mark_a"
        } else {
            "decals/mark_b"
        },
    );
    entity.properties.set(
        "sides",
        sides
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(" "),
    );
    let basis = occurrence.map_or(SOURCE[index].1, |which| {
        if index == 0 {
            [[1016.0, 1112.0, 1160.0], [888.0, 968.0, 1160.0]][which]
        } else {
            [[1024.0, 1096.0, 1112.0], [896.0, 952.0, 1112.0]][which]
        }
    });
    let axes = if occurrence.is_some() {
        [
            [1.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0],
            [2.0 / 3.0, 1.0 / 3.0, -2.0 / 3.0],
            [-2.0 / 3.0, 2.0 / 3.0, -1.0 / 3.0],
        ]
    } else {
        [
            [-2.0 / 3.0, -1.0 / 3.0, 2.0 / 3.0],
            [2.0 / 3.0, -2.0 / 3.0, 1.0 / 3.0],
            [1.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0],
        ]
    };
    for (key, value) in [
        ("BasisOrigin", basis),
        ("BasisNormal", axes[0]),
        ("BasisU", axes[1]),
        ("BasisV", axes[2]),
    ] {
        entity
            .properties
            .set(key, format_vec3(DVec3::from_array(value)));
    }
    for (corner, value) in LOCAL.into_iter().enumerate() {
        entity
            .properties
            .set(format!("uv{corner}"), format_vec3(DVec3::from_array(value)));
    }
    for (key, value) in [
        ("RenderOrder", "2"),
        ("StartU", "1"),
        ("EndU", "-0.5"),
        ("StartV", "-0.25"),
        ("EndV", "1.25"),
        ("fademindist", "16"),
        ("fademaxdist", "64"),
    ] {
        entity.properties.set(key, value);
    }
    vmf.entities.push(entity);
    id
}

struct Leaf {
    vmf: Vmf,
    solids: [u32; 3],
    targets: [u32; 3],
    helpers: [u32; 2],
}

fn leaf(power: u32) -> Leaf {
    let mut document = Document::empty();
    let solids = std::array::from_fn(|index| block(&mut document, SOURCE[index], index == 1));
    let mut vmf = document.into_vmf();
    let targets = std::array::from_fn(|index| {
        let solid = solid_mut(&mut vmf, solids[index]);
        texture(solid, None);
        if index < 2 {
            solid.sides[0].smoothing_groups = Some(7);
            solid.sides[0].displacement = Some(terrain(power, index, SOURCE[index], false));
        }
        solid.sides[0].id
    });
    let helpers = [
        overlay(&mut vmf, &[targets[1], targets[2], targets[0]], 0, None),
        overlay(&mut vmf, &targets[1..2], 1, None),
    ];
    Leaf {
        vmf,
        solids,
        targets,
        helpers,
    }
}

fn write(directory: &Path, name: &str, vmf: &Vmf) -> Vec<u8> {
    let bytes = vmf.write(&Default::default()).into_bytes();
    std::fs::write(directory.join(name), &bytes).unwrap();
    bytes
}

fn instance(vmf: &mut Vmf, file: &str, origin: &str, angles: &str) -> u32 {
    let id = next_id(vmf);
    let mut entity = Entity::new(id, "func_instance");
    for (key, value) in [("file", file), ("origin", origin), ("angles", angles)] {
        entity.properties.set(key, value);
    }
    vmf.entities.push(entity);
    id
}

fn parent(directory: &Path, child: &Vmf, nested: bool) -> (Vmf, u32, SourceContext) {
    write(directory, "leaf.vmf", child);
    let mut root = room();
    let owner = if nested {
        let mut middle = Document::empty().into_vmf();
        instance(&mut middle, "leaf.vmf", "16 0 0", "90 0 0");
        instance(&mut middle, "leaf.vmf", "-128 128 0", "90 0 0");
        write(directory, "middle.vmf", &middle);
        instance(&mut root, "middle.vmf", "1040 1048 1152", "0 90 0")
    } else {
        instance(&mut root, "leaf.vmf", "1024 1024 1024", "0 0 0")
    };
    let source = context(directory, &root);
    (root, owner, source)
}

fn context(directory: &Path, root: &Vmf) -> SourceContext {
    write(directory, "parent.vmf", root);
    SourceContext::capture(&directory.join("parent.vmf"), &Control::default()).unwrap()
}

fn game(directory: &Path) -> GameFs {
    for (name, text) in [
        ("dev/dev_measuregeneric01", "LightmappedGeneric { }"),
        ("decals/mark_a", "LightmappedGeneric { $decal 1 }"),
        ("decals/mark_b", "LightmappedGeneric { $decal 1 }"),
        ("tools/toolsblack", "UnlitGeneric { }"),
    ] {
        let path = directory.join(format!("materials/{name}.vmt"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.to_owned()),
    });
    game
}

fn compile(
    root: &Vmf,
    game: &GameFs,
    source: &SourceContext,
    options: Options,
) -> compiler::Output {
    compiler::compile_with_source_controlled(
        root,
        game,
        options,
        None,
        Some(source),
        &Control::default(),
    )
}

fn successful(output: &compiler::Output) -> &Bsp {
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.leak.is_none());
    output.bsp.as_ref().unwrap()
}

fn literal(
    root: &Vmf,
    child: &Leaf,
    owner: u32,
    power: u32,
    prepared: &PreparedInstances<'_>,
) -> Vmf {
    let mut base = root.clone();
    base.entities.retain(|entity| entity.id != owner);
    let mut document = Document::new(base);
    let solids: [[u32; 3]; 2] = std::array::from_fn(|occurrence| {
        std::array::from_fn(|index| block(&mut document, LITERAL[occurrence][index], index == 1))
    });
    let mut vmf = document.into_vmf();
    for (occurrence, ids) in solids.into_iter().enumerate() {
        for (index, id) in ids.into_iter().enumerate() {
            let solid = solid_mut(&mut vmf, id);
            let original = solid.sides.clone();
            solid.sides = [4, 5, 0, 1, 2, 3]
                .map(|side| original[side].clone())
                .to_vec();
            for (side, old) in solid.sides.iter_mut().zip(original) {
                side.id = old.id;
            }
            texture(solid, Some(occurrence));
            if index < 2 {
                solid.sides[0].smoothing_groups = Some(7);
                solid.sides[0].displacement =
                    Some(terrain(power, index, LITERAL[occurrence][index], true));
            }
        }
    }
    let helpers: [[u32; 2]; 2] = std::array::from_fn(|occurrence| {
        let targets = solids[occurrence].map(|id| solid_mut(&mut vmf, id).sides[0].id);
        [
            overlay(
                &mut vmf,
                &[targets[1], targets[2], targets[0]],
                0,
                Some(occurrence),
            ),
            overlay(&mut vmf, &targets[1..2], 1, Some(occurrence)),
        ]
    });
    let mut objects = BTreeMap::new();
    let mut sides = BTreeMap::new();
    let mut counts = [0; 3];
    for actual in prepared.generated_brushes() {
        let index = child
            .solids
            .iter()
            .position(|&id| id == actual.original)
            .unwrap();
        let occurrence = counts[index];
        counts[index] += 1;
        assert_eq!(
            (actual.owner, actual.file, actual.detail),
            (owner, "leaf.vmf", index == 1)
        );
        let id = solids[occurrence][index];
        if index == 1 {
            let old = vmf
                .entities
                .iter()
                .find(|entity| entity.solids.iter().any(|solid| solid.id == id))
                .unwrap()
                .id;
            let new = prepared
                .vmf()
                .entities
                .iter()
                .find(|entity| {
                    entity
                        .solids
                        .iter()
                        .any(|solid| solid.id == actual.solid.id)
                })
                .unwrap()
                .id;
            objects.insert(old, new);
        }
        objects.insert(id, actual.solid.id);
        for (offset, (expected, actual_side)) in solid_mut(&mut vmf, id)
            .sides
            .iter()
            .zip(&actual.solid.sides)
            .enumerate()
        {
            let original = child
                .vmf
                .solids()
                .find(|solid| solid.id == actual.original)
                .unwrap()
                .sides[offset]
                .id;
            assert_eq!(
                prepared.face_origin(actual_side.id),
                Some((owner, "leaf.vmf", actual.original, original))
            );
            sides.insert(expected.id, actual_side.id);
        }
    }
    assert_eq!(counts, [2; 3]);
    let generated = prepared
        .vmf()
        .entities
        .iter()
        .filter(|entity| {
            entity.classname() == Some("info_overlay")
                && prepared.root_entity(entity.id) == Some(owner)
        })
        .collect::<Vec<_>>();
    assert_eq!(generated.len(), 4);
    for (ordinal, actual) in generated.into_iter().enumerate() {
        let id = helpers[ordinal / 2][ordinal % 2];
        let expected = entity_mut(&mut vmf, id)
            .properties
            .get("sides")
            .unwrap()
            .split_whitespace()
            .map(|id| sides[&id.parse::<u32>().unwrap()])
            .collect::<Vec<_>>();
        assert_eq!(
            actual
                .properties
                .get("sides")
                .unwrap()
                .split_whitespace()
                .map(|id| id.parse::<u32>().unwrap())
                .collect::<Vec<_>>(),
            expected
        );
        let original = child
            .vmf
            .entities
            .iter()
            .find(|entity| entity.id == child.helpers[ordinal % 2])
            .unwrap();
        for key in [
            "uv0",
            "uv1",
            "uv2",
            "uv3",
            "material",
            "RenderOrder",
            "StartU",
            "EndU",
            "StartV",
            "EndV",
            "fademindist",
            "fademaxdist",
            "origin",
            "angles",
            "targetname",
        ] {
            assert_eq!(
                actual.properties.get(key),
                original.properties.get(key),
                "{key}"
            );
        }
        objects.insert(id, actual.id);
    }
    for solid in &mut vmf.world.solids {
        if let Some(id) = objects.get(&solid.id) {
            solid.id = *id;
        }
        for side in &mut solid.sides {
            if let Some(id) = sides.get(&side.id) {
                side.id = *id;
            }
        }
    }
    for entity in &mut vmf.entities {
        if let Some(id) = objects.get(&entity.id) {
            entity.id = *id;
        }
        for solid in &mut entity.solids {
            if let Some(id) = objects.get(&solid.id) {
                solid.id = *id;
            }
            for side in &mut solid.sides {
                if let Some(id) = sides.get(&side.id) {
                    side.id = *id;
                }
            }
        }
        if entity.classname() == Some("info_overlay") {
            let rewritten = entity
                .properties
                .get("sides")
                .unwrap()
                .split_whitespace()
                .map(|id| sides[&id.parse::<u32>().unwrap()].to_string())
                .collect::<Vec<_>>()
                .join(" ");
            entity.properties.set("sides", rewritten);
        }
    }
    vmf.validate_ids().unwrap();
    vmf
}

fn face_points(bsp: &Bsp, face: Face) -> Vec<[f32; 3]> {
    let vertices = bsp.vertices().unwrap();
    let edges = bsp.records::<Edge>().unwrap();
    let surfedges = bsp.values::<i32>(bsp::lump::SURFEDGES).unwrap();
    surfedges[face.first_edge as usize..face.first_edge as usize + face.edge_count as usize]
        .iter()
        .map(|&edge| {
            vertices[edges[edge.unsigned_abs() as usize].0[usize::from(edge < 0)] as usize]
        })
        .collect()
}

fn assert_bindings(output: &compiler::Output, child: &Leaf, owner: u32, power: u32) {
    let bsp = successful(output);
    let terrain = bsp.displacements().unwrap().unwrap();
    let faces = bsp.records::<Face>().unwrap();
    let overlays = bsp.overlay_data().unwrap().unwrap();
    let origins = output.face_origins.as_ref().unwrap();
    let assert_origin = |face: usize, solid, side, occurrence: usize| {
        let values = origins.contributors(face).unwrap().collect::<Vec<_>>();
        assert_eq!(values.len(), 1);
        let compiler::face_origins::Origin::Instance {
            instance,
            file,
            solid: original_solid,
            side: original_side,
            occurrence: path,
        } = values[0]
        else {
            panic!("expected captured instance face");
        };
        assert_eq!(
            (*instance, file.as_str(), *original_solid, *original_side),
            (owner, "leaf.vmf", solid, side)
        );
        assert_eq!(
            path.steps()
                .map(|step| (step.helper, step.file.as_str()))
                .collect::<Vec<_>>(),
            vec![(owner, "middle.vmf"), ([2, 3][occurrence], "leaf.vmf")]
        );
    };
    let size = (1_usize << power) + 1;
    assert_eq!(
        (
            terrain.infos.len(),
            terrain.vertices.len(),
            terrain.triangles.len()
        ),
        (4, 4 * size * size, 8 * (size - 1) * (size - 1))
    );
    let mut targets = [[0_i32; 2]; 2];
    let mut seen = BTreeSet::new();
    for (ordinal, info) in terrain.infos.iter().enumerate() {
        let index = usize::from(info.contents & bsp::contents::DETAIL != 0);
        let occurrence = (0..2)
            .find(|&which| info.start == LITERAL[which][index].1.map(|value| value as f32))
            .unwrap();
        assert!(seen.insert((occurrence, index)));
        assert_eq!(info.power, power as i32);
        assert_eq!(
            info.contents,
            bsp::contents::SOLID | if index == 1 { bsp::contents::DETAIL } else { 0 }
        );
        assert_eq!(info.flags(), Some(0));
        assert_eq!(info.smoothing_angle, 32.0);
        assert_eq!(info.allowed_vertices, [u32::MAX; 10]);
        assert_eq!(faces[info.face as usize].dispinfo, ordinal as i16);
        assert_origin(
            info.face as usize,
            child.solids[index],
            child.targets[index],
            occurrence,
        );
        targets[occurrence][index] = i32::from(info.face);
        let vertices =
            &terrain.vertices[info.vertex_start as usize..info.vertex_start as usize + size * size];
        for (vertex_index, vertex) in vertices.iter().enumerate() {
            assert_eq!(
                vertex.alpha,
                ((vertex_index / size * 11 + vertex_index % size * 7 + index * 23) % 256) as f32
            );
            assert!(
                vertex.distance > 0.0
                    && vertex
                        .direction
                        .into_iter()
                        .all(|value| value.is_finite() && value != 0.0)
            );
        }
        let triangles = &terrain.triangles[info.triangle_start as usize
            ..info.triangle_start as usize + 2 * (size - 1) * (size - 1)];
        for (triangle_index, triangle) in triangles.iter().enumerate() {
            let row = triangle_index / (2 * (size - 1));
            let col = triangle_index % (2 * (size - 1));
            assert_eq!(
                triangle.tags,
                1 | if row.is_multiple_of(2) { 2 } else { 0 }
                    | if col.is_multiple_of(2) { 4 } else { 0 }
            );
        }
    }
    assert_eq!(seen.len(), 4);
    assert_eq!(overlays.overlays.len(), 4);
    for (occurrence, target) in targets.into_iter().enumerate() {
        let bounds = LITERAL[occurrence][2];
        let ordinary = faces
            .iter()
            .enumerate()
            .filter_map(|(index, &face)| {
                if face.dispinfo >= 0 {
                    return None;
                }
                let points = face_points(bsp, face);
                points
                    .iter()
                    .all(|point| {
                        point[1] == bounds.1[1] as f32
                            && (0..3).all(|axis| {
                                point[axis] >= bounds.0[axis] as f32
                                    && point[axis] <= bounds.1[axis] as f32
                            })
                    })
                    .then_some(index as i32)
            })
            .collect::<Vec<_>>();
        assert!(!ordinary.is_empty());
        for face in &ordinary {
            assert_origin(
                *face as usize,
                child.solids[2],
                child.targets[2],
                occurrence,
            );
        }
        for (variant, &bounds) in LITERAL[occurrence][..2].iter().enumerate() {
            let ordinal = occurrence * 2 + variant;
            let record = overlays.overlays[ordinal];
            let mut expected = if variant == 0 {
                target.to_vec()
            } else {
                vec![target[1]]
            };
            if variant == 0 {
                expected.extend(&ordinary);
            }
            expected.sort_unstable();
            assert_eq!(&record.faces[..record.face_count()], expected);
            assert!(
                record.faces[record.face_count()..]
                    .iter()
                    .all(|&value| value == 0)
            );
            assert_eq!(record.id, ordinal as i32);
            assert_eq!(record.render_order(), 2);
            assert_eq!(
                record.origin,
                [bounds.0[0], bounds.1[1], bounds.0[2]].map(|value| value as f32)
            );
            assert_eq!(record.basis_normal, [1.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0]);
            assert_eq!(record.u, [1.0, -0.5]);
            assert_eq!(record.v, [-0.25, 1.25]);
            for (point, local) in record.uv_points.iter().zip(LOCAL) {
                assert_eq!([point[0], point[1]], [local[0] as f32, local[1] as f32]);
            }
            assert_eq!(
                record.uv_points.map(|point| point[2]),
                [2.0 / 3.0, 1.0 / 3.0, -2.0 / 3.0, 0.0]
            );
            assert_eq!(
                [
                    overlays.fades[ordinal].min_distance_squared,
                    overlays.fades[ordinal].max_distance_squared
                ],
                [256.0, 4096.0]
            );
        }
    }
    assert!(!bsp.entities().unwrap().contains("func_instance"));
    assert!(!bsp.entities().unwrap().contains("info_overlay_accessor"));
}

#[test]
fn nonflat_terrain_and_mixed_bindings_match_complete_literals_at_powers_two_to_four() {
    for power in 2..=4 {
        let directory = tempfile::tempdir().unwrap();
        let game = game(directory.path());
        let child = leaf(power);
        let (root, owner, source) = parent(directory.path(), &child.vmf, true);
        let before = root.clone();
        let saved = ["parent.vmf", "middle.vmf", "leaf.vmf"]
            .map(|name| std::fs::read(directory.path().join(name)).unwrap());
        let mut report = compiler::Report::default();
        let prepared =
            instances::resolve(&root, Some(&source), &Control::default(), &mut report).unwrap();
        assert!(!report.has_errors(), "{report:?}");
        let stats = prepared.stats().unwrap();
        assert_eq!(
            (
                stats.instances,
                stats.files,
                stats.max_expansion_depth,
                stats.world_brushes,
                stats.detail_brushes,
                stats.terrain_patches,
                stats.overlays
            ),
            (3, 2, 2, 4, 2, 4, 4)
        );
        assert_eq!(prepared.generated_brushes().count(), 6);
        let expected = literal(&root, &child, owner, power, &prepared);
        for lighting in [LightingMode::Fullbright, LightingMode::DirectLdrHdr] {
            let options = Options {
                lighting,
                ..Default::default()
            };
            let actual = compile(&root, &game, &source, options);
            let literal = compiler::compile_with_options(&expected, &game, options);
            assert_eq!(
                successful(&actual).to_bytes(),
                successful(&literal).to_bytes(),
                "power{power} {lighting:?}: exact complete literal premise failed"
            );
            assert_eq!(
                (
                    actual.stats.models,
                    actual.stats.brushes,
                    actual.stats.overlays,
                    actual.stats.displacements
                ),
                (1, 8, 4, 4)
            );
            assert_bindings(&actual, &child, owner, power);
            if lighting != LightingMode::Fullbright {
                for kind in [bsp::LightingKind::Ldr, bsp::LightingKind::Hdr] {
                    assert!(!successful(&actual).world_lights(kind).unwrap().is_empty());
                }
            }
        }
        assert_eq!(root, before);
        for (name, bytes) in ["parent.vmf", "middle.vmf", "leaf.vmf"]
            .into_iter()
            .zip(saved)
        {
            assert_eq!(std::fs::read(directory.path().join(name)).unwrap(), bytes);
        }
    }
}

fn preserved(output: &compiler::Output, directory: &Path) {
    let path = directory.join("previous.bsp");
    std::fs::write(&path, b"previous BSP").unwrap();
    std::fs::write(path.with_extension("lin"), b"previous LIN").unwrap();
    assert!(!output.write(&path).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), b"previous BSP");
    assert_eq!(
        std::fs::read(path.with_extension("lin")).unwrap(),
        b"previous LIN"
    );
}

fn child_error(output: &compiler::Output, owner: u32, original: u32, reason: &str) {
    assert!(output.bsp.is_none() && output.leak.is_none() && output.report.has_errors());
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .any(
                |diagnostic| diagnostic.object == Some(format!("entity:{owner}"))
                    && diagnostic.message.contains("leaf.vmf")
                    && diagnostic.message.contains(reason)
                    && [
                        format!("Child overlay {original}"),
                        format!("Child cubemap {original}"),
                        format!("child object {original}")
                    ]
                    .iter()
                    .any(|text| diagnostic.message.contains(text))
            ),
        "{:?}",
        output.report
    );
}

#[test]
fn hidden_overlay_helpers_leave_exact_terrain_physics_and_lighting_payloads_unchanged() {
    let directory = tempfile::tempdir().unwrap();
    let game = game(directory.path());
    let mut child = leaf(3);
    for id in child.helpers {
        entity_mut(&mut child.vmf, id).hidden = true;
    }
    let (root, _, source) = parent(directory.path(), &child.vmf, true);
    let hidden = compile(&root, &game, &source, Options::default());
    assert_eq!(hidden.stats.instances.as_ref().unwrap().overlays, 0);
    assert!(
        !serde_json::to_value(hidden.stats.instances.as_ref().unwrap())
            .unwrap()
            .as_object()
            .unwrap()
            .contains_key("overlays")
    );
    let expected = successful(&hidden).to_bytes();
    child
        .vmf
        .entities
        .retain(|entity| !child.helpers.contains(&entity.id));
    write(directory.path(), "leaf.vmf", &child.vmf);
    let source = context(directory.path(), &root);
    let absent = compile(&root, &game, &source, Options::default());
    assert_eq!(expected, successful(&absent).to_bytes());
    assert_eq!(absent.stats.displacements, 4);
}

#[test]
fn displacement_admission_does_not_admit_cage_hidden_foreign_or_nested_targets_or_cubemaps() {
    for case in [
        "cage",
        "hidden-world",
        "hidden-detail",
        "foreign",
        "nested",
        "cubemap",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let game = game(directory.path());
        let mut child = leaf(2);
        let original = if case == "cubemap" {
            let id = next_id(&child.vmf);
            let mut entity = Entity::new(id, "env_cubemap");
            entity.properties.set("origin", "0 0 64");
            entity.properties.set("cubemapsize", "0");
            entity.properties.set("sides", child.targets[0].to_string());
            child.vmf.entities.push(entity);
            id
        } else {
            child.helpers[0]
        };
        let reason = if case == "cubemap" {
            "ordinary world or func_detail side"
        } else {
            "same child"
        };
        match case {
            "cage" => {
                let side = solid_mut(&mut child.vmf, child.solids[0]).sides[1].id;
                entity_mut(&mut child.vmf, original)
                    .properties
                    .set("sides", side.to_string());
            }
            "hidden-world" => solid_mut(&mut child.vmf, child.solids[0]).hidden = true,
            "hidden-detail" => {
                child
                    .vmf
                    .entities
                    .iter_mut()
                    .find(|entity| {
                        entity
                            .solids
                            .iter()
                            .any(|solid| solid.id == child.solids[1])
                    })
                    .unwrap()
                    .hidden = true
            }
            "foreign" => {
                let id = room()
                    .side_ids()
                    .find(|id| !child.vmf.side_ids().any(|local| local == *id))
                    .unwrap();
                entity_mut(&mut child.vmf, original)
                    .properties
                    .set("sides", id.to_string());
            }
            "nested" => {
                let index = child
                    .vmf
                    .world
                    .solids
                    .iter()
                    .position(|solid| solid.id == child.solids[0])
                    .unwrap();
                let mut nested = Document::empty().into_vmf();
                nested
                    .world
                    .solids
                    .push(child.vmf.world.solids.remove(index));
                write(directory.path(), "remote.vmf", &nested);
                instance(&mut child.vmf, "remote.vmf", "0 0 0", "0 0 0");
            }
            _ => {}
        }
        let before = child.vmf.clone();
        let (root, owner, source) = parent(directory.path(), &child.vmf, false);
        let output = compile(&root, &game, &source, Options::default());
        child_error(&output, owner, original, reason);
        preserved(&output, directory.path());
        assert_eq!(child.vmf, before);
    }
}

#[test]
fn regions_keep_contained_terrain_refuse_crossing_and_preserve_missing_target_ownership() {
    for case in ["contained", "crossing", "omitted"] {
        let directory = tempfile::tempdir().unwrap();
        let game = game(directory.path());
        let mut child = leaf(2);
        entity_mut(&mut child.vmf, child.helpers[0])
            .properties
            .set("origin", "4096 4096 4096");
        if case == "omitted" {
            let solid = solid_mut(&mut child.vmf, child.solids[0]);
            for side in &mut solid.sides {
                side.plane.0 = side.plane.0.map(|point| point + DVec3::X * 512.0);
                if let Some(info) = &mut side.displacement {
                    info.start_position += DVec3::X * 512.0;
                }
            }
        }
        let (mut root, owner, _) = parent(directory.path(), &child.vmf, false);
        root.cordon = Some(Node::block(
            "cordon",
            vec![
                Node::string("active", "1"),
                Node::string("mins", "896 896 1028"),
                Node::string(
                    "maxs",
                    if case == "crossing" {
                        "1040 1152 1276"
                    } else {
                        "1152 1152 1276"
                    },
                ),
            ],
        ));
        let source = context(directory.path(), &root);
        let output = compile(&root, &game, &source, Options::default());
        assert_eq!(output.stats.instances.as_ref().unwrap().overlays, 2);
        match case {
            "contained" => {
                assert_eq!(
                    successful(&output)
                        .displacements()
                        .unwrap()
                        .unwrap()
                        .infos
                        .len(),
                    2
                );
                assert_eq!(output.stats.overlays, 2);
            }
            "omitted" => {
                child_error(&output, owner, child.helpers[0], "absent");
                preserved(&output, directory.path());
            }
            _ => {
                assert!(output.bsp.is_none() && output.report.has_errors());
                assert!(
                    output.report.diagnostics.iter().any(|diagnostic| {
                        diagnostic
                            .message
                            .contains("Terrain crosses a compile-region boundary")
                    }),
                    "{:?}",
                    output.report
                );
                preserved(&output, directory.path());
            }
        }
    }
}

#[test]
fn terrain_overlay_sources_remain_dependencies_through_cancellation_and_delayed_publication() {
    let directory = tempfile::tempdir().unwrap();
    let game = game(directory.path());
    let child = leaf(2);
    let (root, owner, _) = parent(directory.path(), &child.vmf, true);
    let mut document = Document::new(root);
    document
        .edit("assembly name", |tx| {
            tx.entity_mut(owner)?
                .properties
                .set("targetname", "terrain_marks");
            Ok(())
        })
        .unwrap();
    let before = document.vmf().clone();
    let history = document
        .undo_labels()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let source = context(directory.path(), document.vmf());
    let bytes = std::fs::read(directory.path().join("leaf.vmf")).unwrap();
    let output = compile(document.vmf(), &game, &source, Options::default());
    successful(&output);
    assert_eq!(
        output
            .stats
            .instances
            .as_ref()
            .unwrap()
            .dependencies
            .iter()
            .map(|dependency| dependency.path.as_str())
            .collect::<Vec<_>>(),
        ["leaf.vmf", "middle.vmf"]
    );
    let path = directory.path().join("previous.bsp");
    std::fs::write(&path, b"previous BSP").unwrap();
    std::fs::write(path.with_extension("lin"), b"previous LIN").unwrap();
    let cancelled = Control::default();
    cancelled.cancel().unwrap();
    assert!(output.write_controlled(&path, &cancelled).is_err());
    let pending = output
        .prepare_write_controlled(&path, &Control::default())
        .unwrap()
        .unwrap();
    let mut changed = bytes.clone();
    changed.extend_from_slice(b"\n");
    std::fs::write(directory.path().join("leaf.vmf"), changed).unwrap();
    assert!(pending.commit(&Control::default()).is_err());
    assert!(output.write(&path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"previous BSP");
    assert_eq!(
        std::fs::read(path.with_extension("lin")).unwrap(),
        b"previous LIN"
    );
    std::fs::write(directory.path().join("leaf.vmf"), bytes).unwrap();
    let cancelled = compiler::compile_with_source_controlled(
        document.vmf(),
        &game,
        Options::default(),
        None,
        Some(&source),
        &cancelled,
    );
    assert!(
        cancelled.bsp.is_none()
            && cancelled.leak.is_none()
            && cancelled
                .report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("cancelled"))
    );
    assert_eq!(document.vmf(), &before);
    assert_eq!(
        document.undo_labels().collect::<Vec<_>>(),
        history.iter().map(String::as_str).collect::<Vec<_>>()
    );
}
