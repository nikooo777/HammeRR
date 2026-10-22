#[allow(dead_code)]
mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use compiler::instances::{self, PreparedInstances, SourceContext};
use compiler::{Control, LightingMode, Options};
use glam::DVec3;
use map_core::{BrushGeometry, Document, ObjectId, edit};
use source_formats::bsp::{self, Bsp, Edge, Face, Plane, TexData, TexInfo};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::keyvalues::Node;
use source_formats::vmf::{Connection, Entity, Solid, UvAxis, Vmf, format_vec3};

type Bounds = ([f64; 3], [f64; 3]);
const LOCAL_CORNERS: [[f64; 3]; 4] = [
    [-12.0, -10.0, 17.0],
    [-12.0, 10.0, 9.0],
    [12.0, 10.0, 7.0],
    [12.0, -10.0, 13.0],
];
const LITERAL_BOUNDS: [[Bounds; 2]; 2] = [
    [
        ([1016.0, 1080.0, 1160.0], [1064.0, 1112.0, 1192.0]),
        ([1024.0, 1080.0, 1112.0], [1056.0, 1096.0, 1144.0]),
    ],
    [
        ([888.0, 936.0, 1160.0], [936.0, 968.0, 1192.0]),
        ([896.0, 936.0, 1112.0], [928.0, 952.0, 1144.0]),
    ],
];

fn next_id(source: &Vmf) -> u32 {
    source.object_ids().chain(source.side_ids()).max().unwrap() + 1
}

fn instance(source: &mut Vmf, file: &str, origin: &str, angles: &str) -> u32 {
    let id = next_id(source);
    let mut entity = Entity::new(id, "func_instance");
    for (key, value) in [("file", file), ("origin", origin), ("angles", angles)] {
        entity.properties.set(key, value);
    }
    source.entities.push(entity);
    id
}

fn write_source(directory: &Path, name: &str, source: &Vmf) -> Vec<u8> {
    let bytes = source.write(&Default::default()).into_bytes();
    std::fs::write(directory.join(name), &bytes).unwrap();
    bytes
}

fn saved_parent(directory: &Path, parent: &Vmf) -> SourceContext {
    write_source(directory, "parent.vmf", parent);
    SourceContext::capture(&directory.join("parent.vmf"), &Control::default()).unwrap()
}

fn game(directory: &Path) -> GameFs {
    for folder in ["dev", "tools", "decals"] {
        std::fs::create_dir_all(directory.join("materials").join(folder)).unwrap();
    }
    for (name, text) in [
        ("dev/dev_measuregeneric01", "LightmappedGeneric { }"),
        ("tools/toolsblack", "UnlitGeneric { }"),
        (
            "tools/toolsnodraw",
            "LightmappedGeneric { %compilenodraw 1 }",
        ),
        (
            "tools/toolsoccluder",
            "UnlitGeneric { $translucent 1 %compilenolight 1 }",
        ),
        ("decals/mark_a", "LightmappedGeneric { $translucent 1 }"),
        ("decals/mark_b", "LightmappedGeneric { $decal 1 }"),
    ] {
        std::fs::write(directory.join(format!("materials/{name}.vmt")), text).unwrap();
    }
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.to_owned()),
    });
    game
}

fn solid_mut(source: &mut Vmf, id: u32) -> &mut Solid {
    source
        .world
        .solids
        .iter_mut()
        .chain(
            source
                .entities
                .iter_mut()
                .flat_map(|entity| &mut entity.solids),
        )
        .find(|solid| solid.id == id)
        .unwrap()
}

fn helper_mut(source: &mut Vmf, id: u32) -> &mut Entity {
    source
        .entities
        .iter_mut()
        .find(|entity| entity.id == id)
        .unwrap()
}

fn block(document: &mut Document, bounds: Bounds, detail: bool) -> u32 {
    document
        .edit("target brush", |tx| {
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
        .edit("players and lighting", |tx| {
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

#[derive(Clone, Copy)]
struct Frame {
    basis: [f64; 3],
    normal: [f64; 3],
    u: [f64; 3],
    v: [f64; 3],
    marker: Option<[f64; 3]>,
}

fn source_frame(index: usize) -> Frame {
    Frame {
        basis: if index == 0 {
            [-24.0, 0.0, 48.0]
        } else {
            [24.0, 0.0, 32.0]
        },
        normal: [-2.0 / 3.0, -1.0 / 3.0, 2.0 / 3.0],
        u: [2.0 / 3.0, -2.0 / 3.0, 1.0 / 3.0],
        v: if index == 0 {
            [1.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0]
        } else {
            [-1.0 / 3.0, -2.0 / 3.0, -2.0 / 3.0]
        },
        marker: (index == 0).then_some([7.0, 9.0, 11.0]),
    }
}

fn literal_frame(occurrence: usize, index: usize) -> Frame {
    Frame {
        basis: [
            [[1040.0, 1112.0, 1176.0], [1040.0, 1096.0, 1128.0]],
            [[912.0, 968.0, 1176.0], [912.0, 952.0, 1128.0]],
        ][occurrence][index],
        normal: [1.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0],
        u: [2.0 / 3.0, 1.0 / 3.0, -2.0 / 3.0],
        v: if index == 0 {
            [-2.0 / 3.0, 2.0 / 3.0, -1.0 / 3.0]
        } else {
            [2.0 / 3.0, -2.0 / 3.0, 1.0 / 3.0]
        },
        marker: if index == 0 {
            Some([[1031.0, 1075.0, 1145.0], [903.0, 931.0, 1145.0]][occurrence])
        } else {
            None
        },
    }
}

fn overlay(source: &mut Vmf, sides: &[u32], frame: Frame, index: usize) -> u32 {
    let id = next_id(source);
    let mut entity = Entity::new(id, "info_overlay");
    entity.properties.set(
        "material",
        if index == 0 {
            "materials/decals/mark_a.vmt"
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
    for (key, value) in [
        ("BasisOrigin", frame.basis),
        ("BasisNormal", frame.normal),
        ("BasisU", frame.u),
        ("BasisV", frame.v),
    ] {
        entity
            .properties
            .set(key, format_vec3(DVec3::from_array(value)));
    }
    if let Some(marker) = frame.marker {
        entity
            .properties
            .set("origin", format_vec3(DVec3::from_array(marker)));
    }
    for (index, point) in LOCAL_CORNERS.into_iter().enumerate() {
        entity
            .properties
            .set(format!("uv{index}"), format_vec3(DVec3::from_array(point)));
    }
    for (key, a, b) in [
        ("RenderOrder", "2", "1"),
        ("StartU", " 1.0 ", "0"),
        ("EndU", "-0.5", "1"),
        ("StartV", "-0.25", "1"),
        ("EndV", "1.25", "0"),
        ("fademindist", "16", "-1"),
        ("fademaxdist", "64", "0"),
    ] {
        entity.properties.set(key, if index == 0 { a } else { b });
    }
    source.entities.push(entity);
    id
}

struct Leaf {
    vmf: Vmf,
    solids: [u32; 2],
    targets: [u32; 2],
    helpers: [u32; 2],
}

fn leaf(helpers_first: bool) -> Leaf {
    let mut document = Document::empty();
    let world = block(
        &mut document,
        ([-40.0, -24.0, 16.0], [-8.0, 24.0, 48.0]),
        false,
    );
    let detail = block(
        &mut document,
        ([8.0, -16.0, 16.0], [40.0, 16.0, 32.0]),
        true,
    );
    let mut vmf = document.into_vmf();
    let targets = [
        solid_mut(&mut vmf, world).sides[0].id,
        solid_mut(&mut vmf, detail).sides[0].id,
    ];
    for solid in &mut vmf.world.solids {
        for side in &mut solid.sides {
            side.u_axis = UvAxis {
                axis: DVec3::ONE.normalize(),
                offset: 0.0,
                scale: 1.0,
            };
            side.v_axis = UvAxis {
                axis: DVec3::new(1.0, 2.0, 3.0).normalize(),
                offset: 0.0,
                scale: 1.0,
            };
        }
    }
    for entity in &mut vmf.entities {
        for solid in &mut entity.solids {
            for side in &mut solid.sides {
                side.u_axis = UvAxis {
                    axis: DVec3::ONE.normalize(),
                    offset: 0.0,
                    scale: 1.0,
                };
                side.v_axis = UvAxis {
                    axis: DVec3::new(1.0, 2.0, 3.0).normalize(),
                    offset: 0.0,
                    scale: 1.0,
                };
            }
        }
    }
    let helpers = [
        overlay(&mut vmf, &targets, source_frame(0), 0),
        overlay(&mut vmf, &targets[1..], source_frame(1), 1),
    ];
    helper_mut(&mut vmf, helpers[0])
        .properties
        .set("sides", format!("{}\t {}", targets[1], targets[0]));
    for (key, value) in [
        ("angles", "0 0 0"),
        ("angle", "-0"),
        ("spawnflags", "0"),
        ("mindxlevel", "0"),
        ("maxdxlevel", "0"),
        ("targetname", ""),
        ("target", ""),
        ("parentname", ""),
        ("name", ""),
        ("globalname", ""),
        ("killtarget", ""),
        ("lightingorigin", ""),
    ] {
        helper_mut(&mut vmf, helpers[1]).properties.set(key, value);
    }
    if helpers_first {
        let detail = vmf.entities.remove(0);
        vmf.entities.push(detail);
    }
    Leaf {
        vmf,
        solids: [world, detail],
        targets,
        helpers,
    }
}

fn nested_parent(directory: &Path, child: &Vmf) -> (Vmf, u32) {
    write_source(directory, "leaf.vmf", child);
    let mut middle = Document::empty().into_vmf();
    instance(&mut middle, "leaf.vmf", "16 0 0", "90 0 0");
    instance(&mut middle, "leaf.vmf", "-128 128 0", "90 0 0");
    write_source(directory, "middle.vmf", &middle);
    let mut parent = room();
    let owner = instance(&mut parent, "middle.vmf", "1040 1048 1152", "0 90 0");
    (parent, owner)
}

fn compile_saved(
    parent: &Vmf,
    game: &GameFs,
    source: &SourceContext,
    options: Options,
) -> compiler::Output {
    compiler::compile_with_source_controlled(
        parent,
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

fn resolve<'a>(parent: &'a Vmf, source: &SourceContext) -> PreparedInstances<'a> {
    let mut report = compiler::Report::default();
    let prepared =
        instances::resolve(parent, Some(source), &Control::default(), &mut report).unwrap();
    assert!(!report.has_errors(), "{report:?}");
    prepared
}

fn child_error(output: &compiler::Output, owner: u32, original: Option<u32>, reason: &str) {
    assert!(output.bsp.is_none() && output.leak.is_none() && output.report.has_errors());
    assert!(
        output.report.diagnostics.iter().any(|diagnostic| {
            diagnostic.object == Some(format!("entity:{owner}"))
                && diagnostic.message.contains("leaf.vmf")
                && diagnostic.message.contains(reason)
                && original.is_none_or(|id| {
                    diagnostic.message.contains(&format!("Child overlay {id}"))
                        || diagnostic.message.contains(&format!("child object {id}"))
                })
        }),
        "{reason}: {:?}",
        output.report
    );
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

struct Literal {
    vmf: Vmf,
    solids: [[u32; 2]; 2],
    helpers: [[u32; 2]; 2],
}

fn literal_flat(base: &Vmf) -> Literal {
    let mut document = Document::new(base.clone());
    let mut solids = [[0; 2]; 2];
    for occurrence in 0..2 {
        for index in 0..2 {
            solids[occurrence][index] =
                block(&mut document, LITERAL_BOUNDS[occurrence][index], index == 1);
        }
    }
    let mut vmf = document.into_vmf();
    for occurrence in 0..2 {
        for id in solids[occurrence] {
            let solid = solid_mut(&mut vmf, id);
            let sides = solid.sides.clone();
            solid.sides = [4, 5, 0, 1, 2, 3].map(|side| sides[side].clone()).to_vec();
            for (side, original) in solid.sides.iter_mut().zip(&sides) {
                side.id = original.id;
                side.u_axis = UvAxis {
                    axis: DVec3::new(-1.0, 1.0, -1.0).normalize(),
                    offset: [1128.0, 1144.0][occurrence] / 3.0_f64.sqrt(),
                    scale: 1.0,
                };
                side.v_axis = UvAxis {
                    axis: DVec3::new(-2.0, 3.0, -1.0).normalize(),
                    offset: [40.0, 216.0][occurrence] / 14.0_f64.sqrt(),
                    scale: 1.0,
                };
            }
        }
    }
    let mut helpers = [[0; 2]; 2];
    for occurrence in 0..2 {
        let targets = solids[occurrence].map(|id| solid_mut(&mut vmf, id).sides[0].id);
        helpers[occurrence] = [
            overlay(
                &mut vmf,
                &[targets[1], targets[0]],
                literal_frame(occurrence, 0),
                0,
            ),
            overlay(&mut vmf, &targets[1..], literal_frame(occurrence, 1), 1),
        ];
    }
    Literal {
        vmf,
        solids,
        helpers,
    }
}

fn map_literal_ids(
    literal: &mut Literal,
    prepared: &PreparedInstances<'_>,
    child: &Leaf,
    owner: u32,
) {
    let mut objects = BTreeMap::new();
    let mut sides = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for brush in prepared.generated_brushes() {
        assert_eq!((brush.owner, brush.file), (owner, "leaf.vmf"));
        let index = usize::from(brush.detail);
        assert_eq!(brush.original, child.solids[index]);
        let geometry = BrushGeometry::from_solid(brush.solid).unwrap();
        let occurrence = LITERAL_BOUNDS
            .iter()
            .position(|bounds| {
                geometry
                    .mins
                    .abs_diff_eq(DVec3::from_array(bounds[index].0), 1e-8)
                    && geometry
                        .maxs
                        .abs_diff_eq(DVec3::from_array(bounds[index].1), 1e-8)
            })
            .unwrap();
        assert!(seen.insert((occurrence, index)));
        let original = child
            .vmf
            .solids()
            .find(|solid| solid.id == child.solids[index])
            .unwrap();
        if brush.detail {
            let literal_owner = literal
                .vmf
                .entities
                .iter()
                .find(|entity| {
                    entity
                        .solids
                        .iter()
                        .any(|solid| solid.id == literal.solids[occurrence][index])
                })
                .unwrap()
                .id;
            let actual_owner = prepared
                .vmf()
                .entities
                .iter()
                .find(|entity| entity.solids.iter().any(|solid| solid.id == brush.solid.id))
                .unwrap()
                .id;
            assert!(objects.insert(literal_owner, actual_owner).is_none());
        }
        let target = solid_mut(&mut literal.vmf, literal.solids[occurrence][index]);
        objects.insert(target.id, brush.solid.id);
        for ((actual, original), target) in brush
            .solid
            .sides
            .iter()
            .zip(&original.sides)
            .zip(&target.sides)
        {
            assert_eq!(
                prepared.face_origin(actual.id),
                Some((owner, "leaf.vmf", child.solids[index], original.id))
            );
            assert!(sides.insert(target.id, actual.id).is_none());
        }
    }
    assert_eq!(seen.len(), 4);
    for actual in prepared.vmf().entities.iter().filter(|entity| {
        prepared.root_entity(entity.id) == Some(owner) && entity.classname() == Some("info_overlay")
    }) {
        let (occurrence, index) = (0..2)
            .flat_map(|occurrence| (0..2).map(move |index| (occurrence, index)))
            .find(|&(occurrence, index)| {
                let point =
                    source_formats::vmf::parse_vec3(actual.properties.get("BasisOrigin").unwrap())
                        .unwrap();
                point.abs_diff_eq(
                    DVec3::from_array(literal_frame(occurrence, index).basis),
                    1e-8,
                )
            })
            .unwrap();
        assert!(
            objects
                .insert(literal.helpers[occurrence][index], actual.id)
                .is_none()
        );
    }
    assert_eq!(objects.len(), 10);
    for solid in &mut literal.vmf.world.solids {
        if let Some(id) = objects.get(&solid.id) {
            solid.id = *id;
        }
        for side in &mut solid.sides {
            if let Some(id) = sides.get(&side.id) {
                side.id = *id;
            }
        }
    }
    for entity in &mut literal.vmf.entities {
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
                .map(|value| {
                    let id = value.parse::<u32>().unwrap();
                    sides.get(&id).copied().unwrap_or(id).to_string()
                })
                .collect::<Vec<_>>()
                .join(" ");
            entity.properties.set("sides", rewritten);
        }
    }
}

fn face_points(bsp: &Bsp, face: Face) -> Vec<DVec3> {
    let vertices = bsp.vertices().unwrap();
    let edges = bsp.records::<Edge>().unwrap();
    let surfedges = bsp.values::<i32>(bsp::lump::SURFEDGES).unwrap();
    surfedges[face.first_edge as usize..face.first_edge as usize + face.edge_count as usize]
        .iter()
        .map(|&edge| {
            DVec3::from_array(
                vertices[usize::from(edges[edge.unsigned_abs() as usize].0[usize::from(edge < 0)])]
                    .map(f64::from),
            )
        })
        .collect()
}

fn eligible_faces(bsp: &Bsp, bounds: &[Bounds], normal: [f32; 3]) -> Vec<i32> {
    let planes = bsp.records::<Plane>().unwrap();
    bsp.records::<Face>()
        .unwrap()
        .into_iter()
        .enumerate()
        .filter_map(|(index, face)| {
            let plane = planes[face.plane as usize];
            if plane.normal != normal {
                return None;
            }
            let points = face_points(bsp, face);
            bounds
                .iter()
                .any(|&(mins, maxs)| {
                    points.iter().all(|point| {
                        point.cmpge(DVec3::from_array(mins)).all()
                            && point.cmple(DVec3::from_array(maxs)).all()
                            && point.dot(DVec3::from_array(normal.map(f64::from)))
                                == DVec3::from_array(maxs)
                                    .dot(DVec3::from_array(normal.map(f64::from)))
                    })
                })
                .then_some(index as i32)
        })
        .collect()
}

fn assert_record(
    bsp: &Bsp,
    ordinal: usize,
    frame: Frame,
    variant: usize,
    bounds: &[Bounds],
    normal: [f32; 3],
) {
    let data = bsp.overlay_data().unwrap().unwrap();
    let record = data.overlays[ordinal];
    assert_eq!(record.id, ordinal as i32);
    assert_eq!(record.origin, frame.basis.map(|value| value as f32));
    assert_eq!(record.basis_normal, frame.normal.map(|value| value as f32));
    assert_eq!(record.render_order(), if variant == 0 { 2 } else { 1 });
    assert_eq!(
        record.u,
        if variant == 0 {
            [1.0, -0.5]
        } else {
            [0.0, 1.0]
        }
    );
    assert_eq!(
        record.v,
        if variant == 0 {
            [-0.25, 1.25]
        } else {
            [1.0, 0.0]
        }
    );
    for (point, local) in record.uv_points.iter().zip(LOCAL_CORNERS) {
        assert_eq!([point[0], point[1]], [local[0] as f32, local[1] as f32]);
    }
    assert_eq!(
        record.uv_points.map(|point| point[2]),
        [
            frame.u[0] as f32,
            frame.u[1] as f32,
            frame.u[2] as f32,
            variant as f32
        ]
    );
    assert_eq!(
        [
            data.fades[ordinal].min_distance_squared,
            data.fades[ordinal].max_distance_squared
        ],
        if variant == 0 {
            [256.0, 4096.0]
        } else {
            [-1.0, 0.0]
        }
    );
    let infos = bsp.records::<TexInfo>().unwrap();
    let textures = bsp.records::<TexData>().unwrap();
    let names = bsp.texdata_strings().unwrap();
    assert_eq!(
        names[textures[infos[record.texinfo as usize].texdata as usize].name_index as usize],
        if variant == 0 {
            "DECALS/MARK_A"
        } else {
            "DECALS/MARK_B"
        }
    );
    let expected = eligible_faces(bsp, bounds, normal);
    assert!(!expected.is_empty());
    assert_eq!(&record.faces[..record.face_count()], expected);
    assert!(
        record.faces[record.face_count()..]
            .iter()
            .all(|&face| face == 0)
    );
}

#[test]
fn repeated_cousins_noncommuting_frames_and_local_bindings_match_complete_literal_bsps() {
    for helpers_first in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let game = game(directory.path());
        let child = leaf(helpers_first);
        let (mut parent, owner) = nested_parent(directory.path(), &child.vmf);
        let floor = parent.world.solids[0].sides[0].id;
        assert!(
            child
                .targets
                .iter()
                .all(|side| parent.side_ids().any(|root| root == *side))
        );
        let root_frame = Frame {
            basis: [1024.0, 1024.0, 1024.0],
            normal: [0.0, 0.0, 1.0],
            u: [1.0, 0.0, 0.0],
            v: [0.0, 1.0, 0.0],
            marker: None,
        };
        let root = overlay(&mut parent, &[floor], root_frame, 0);
        let mut base = parent.clone();
        base.entities.retain(|entity| entity.id != owner);
        let before = parent.clone();
        let source = saved_parent(directory.path(), &parent);
        let originals = ["parent.vmf", "middle.vmf", "leaf.vmf"]
            .map(|name| std::fs::read(directory.path().join(name)).unwrap());
        let prepared = resolve(&parent, &source);
        let stats = prepared.stats().unwrap();
        assert_eq!(
            (
                stats.instances,
                stats.files,
                stats.max_expansion_depth,
                stats.world_brushes,
                stats.detail_brushes,
                stats.overlays
            ),
            (3, 2, 2, 2, 2, 4)
        );
        assert_eq!(prepared.generated_brushes().count(), 4);
        assert_eq!(
            prepared
                .generated_brushes()
                .filter(|brush| brush.detail)
                .count(),
            2
        );
        let generated = prepared
            .vmf()
            .entities
            .iter()
            .filter(|entity| entity.classname() == Some("info_overlay") && entity.id != root)
            .collect::<Vec<_>>();
        assert_eq!(generated.len(), 4);
        for (ordinal, actual) in generated.iter().enumerate() {
            let occurrence = ordinal / 2;
            let index = ordinal % 2;
            let frame = literal_frame(occurrence, index);
            assert_eq!(prepared.root_entity(actual.id), Some(owner));
            assert_eq!(
                actual.properties.get("angles"),
                if index == 0 { None } else { Some("0 0 0") }
            );
            assert_eq!(
                actual.properties.get("targetname"),
                if index == 0 { None } else { Some("") }
            );
            assert!(actual.solids.is_empty());
            for (key, expected) in [
                ("BasisOrigin", frame.basis),
                ("BasisNormal", frame.normal),
                ("BasisU", frame.u),
                ("BasisV", frame.v),
            ] {
                let actual =
                    source_formats::vmf::parse_vec3(actual.properties.get(key).unwrap()).unwrap();
                assert!(
                    actual.abs_diff_eq(DVec3::from_array(expected), 1e-8),
                    "{key}: {actual:?} != {expected:?}"
                );
            }
            assert_eq!(
                actual.properties.get("origin").is_none(),
                frame.marker.is_none()
            );
            if let Some(marker) = frame.marker {
                assert!(
                    actual
                        .origin()
                        .unwrap()
                        .abs_diff_eq(DVec3::from_array(marker), 1e-8)
                );
            }
            let original = child
                .vmf
                .entities
                .iter()
                .find(|entity| entity.id == child.helpers[index])
                .unwrap();
            for key in [
                "uv0",
                "uv1",
                "uv2",
                "uv3",
                "material",
                "StartU",
                "EndU",
                "StartV",
                "EndV",
                "RenderOrder",
                "fademindist",
                "fademaxdist",
                "angles",
                "angle",
                "spawnflags",
                "mindxlevel",
                "maxdxlevel",
                "targetname",
                "target",
                "parentname",
                "name",
                "globalname",
                "killtarget",
                "lightingorigin",
            ] {
                assert_eq!(
                    actual.properties.get(key),
                    original.properties.get(key),
                    "{key}"
                );
            }
            let actual_ids = actual
                .properties
                .get("sides")
                .unwrap()
                .split_whitespace()
                .map(|side| side.parse::<u32>().unwrap())
                .collect::<Vec<_>>();
            let mut expected = Vec::new();
            let targets = if index == 0 {
                vec![child.targets[1], child.targets[0]]
            } else {
                vec![child.targets[1]]
            };
            for target in targets {
                let brush_index = usize::from(target == child.targets[1]);
                let brush = prepared
                    .generated_brushes()
                    .find(|brush| {
                        brush.original == child.solids[brush_index]
                            && BrushGeometry::from_solid(brush.solid)
                                .unwrap()
                                .mins
                                .abs_diff_eq(
                                    DVec3::from_array(LITERAL_BOUNDS[occurrence][brush_index].0),
                                    1e-8,
                                )
                    })
                    .unwrap();
                expected.push(brush.solid.sides[0].id);
            }
            assert_eq!(actual_ids, expected);
        }
        let mut literal = literal_flat(&base);
        map_literal_ids(&mut literal, &prepared, &child, owner);
        literal.vmf.validate_ids().unwrap();
        for lighting in [LightingMode::Fullbright, LightingMode::DirectLdrHdr] {
            let options = Options {
                lighting,
                ..Default::default()
            };
            let actual = compile_saved(&parent, &game, &source, options);
            let expected = compiler::compile_with_options(&literal.vmf, &game, options);
            let actual_bsp = successful(&actual);
            let expected_bsp = successful(&expected);
            assert_eq!(actual.stats.overlays, 5);
            assert_eq!(actual_bsp.to_bytes(), expected_bsp.to_bytes());
            let bsp = Bsp::parse(&actual_bsp.to_bytes()).unwrap();
            assert_record(
                &bsp,
                0,
                root_frame,
                0,
                &[([752.0, 752.0, 1008.0], [1296.0, 1296.0, 1024.0])],
                [0.0, 0.0, 1.0],
            );
            for (occurrence, occurrence_bounds) in LITERAL_BOUNDS.iter().enumerate() {
                for index in 0..2 {
                    let bounds = if index == 0 {
                        &occurrence_bounds[..]
                    } else {
                        &occurrence_bounds[1..]
                    };
                    assert_record(
                        &bsp,
                        1 + occurrence * 2 + index,
                        literal_frame(occurrence, index),
                        index,
                        bounds,
                        [0.0, 1.0, 0.0],
                    );
                }
            }
            if lighting != LightingMode::Fullbright {
                for kind in [bsp::LightingKind::Ldr, bsp::LightingKind::Hdr] {
                    assert!(!bsp.world_lights(kind).unwrap().is_empty());
                }
            }
            assert!(!bsp.entities().unwrap().contains("info_overlay_accessor"));
            assert!(!bsp.entities().unwrap().contains("func_instance"));
        }
        assert_eq!(parent, before);
        for (name, bytes) in ["parent.vmf", "middle.vmf", "leaf.vmf"]
            .into_iter()
            .zip(originals)
        {
            assert_eq!(std::fs::read(directory.path().join(name)).unwrap(), bytes);
        }
    }
}

#[test]
fn hidden_helpers_keep_exact_physical_outputs_and_omit_zero_expanded_counts() {
    let directory = tempfile::tempdir().unwrap();
    let game = game(directory.path());
    let mut child = leaf(true);
    for &id in &child.helpers {
        let entity = helper_mut(&mut child.vmf, id);
        entity.hidden = true;
        entity.properties.set("sides", "bad");
    }
    let (parent, _) = nested_parent(directory.path(), &child.vmf);
    let source = saved_parent(directory.path(), &parent);
    let prepared = resolve(&parent, &source);
    assert_eq!(prepared.generated_brushes().count(), 4);
    assert_eq!(prepared.stats().unwrap().overlays, 0);
    assert!(
        serde_json::to_value(prepared.stats().unwrap())
            .unwrap()
            .get("overlays")
            .is_none()
    );
    let output = compile_saved(&parent, &game, &source, Options::default());
    let literal = literal_flat(&room());
    let mut physical = literal.vmf;
    physical
        .entities
        .retain(|entity| entity.classname() != Some("info_overlay"));
    let expected = compiler::compile(&physical, &game);
    assert_eq!(
        successful(&output).to_bytes(),
        successful(&expected).to_bytes()
    );
    assert_eq!(output.stats.overlays, 0);
}

#[test]
fn strict_properties_side_grammar_and_projection_shapes_preserve_previous_outputs() {
    let directory = tempfile::tempdir().unwrap();
    let game = game(directory.path());
    let base = leaf(true);
    let side = base.targets[0].to_string();
    let mut cases = Vec::new();
    for (key, value) in [
        ("sides", "".to_owned()),
        ("sides", "0".into()),
        ("sides", "-1".into()),
        ("sides", "oops".into()),
        ("sides", format!("{side} {side}")),
        (
            "sides",
            (1..=65)
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(" "),
        ),
        ("sides", "1 ".repeat(257)),
        ("targetname", "named".into()),
        ("targetname", " ".into()),
        ("parentname", "\t".into()),
        ("target", "named".into()),
        ("lightingorigin", "probe".into()),
        ("angles", "0 90 0".into()),
        ("angle", "1".into()),
        ("spawnflags", "1".into()),
        ("mindxlevel", "90".into()),
        ("maxdxlevel", "90".into()),
        ("replace01", "value".into()),
        ("model", "*1".into()),
        ("unknown", "0".into()),
        ("material", "../secret".into()),
        ("material", "/absolute".into()),
        ("material", "".into()),
        ("BasisU", "0 0 0".into()),
        ("BasisV", "1 0 0".into()),
        ("BasisNormal", "0 0 2".into()),
        ("BasisOrigin", "NaN 0 0".into()),
        ("BasisOrigin", "17000 0 48".into()),
        ("origin", "17000 0 0".into()),
        ("uv1", "-12 -10 9".into()),
        ("uv0", "1e40 0 0".into()),
        ("RenderOrder", "4".into()),
        ("EndU", "1".into()),
        ("StartV", "NaN".into()),
        ("fademindist", "1e30".into()),
    ] {
        let mut child = base.vmf.clone();
        helper_mut(&mut child, base.helpers[0])
            .properties
            .set(key, value);
        cases.push(child);
    }
    for case in 0..5 {
        let mut child = base.vmf.clone();
        let entity = helper_mut(&mut child, base.helpers[0]);
        match case {
            0 => entity
                .properties
                .0
                .push(("basisorigin".into(), "0 0 48".into())),
            1 => entity.extra.push(Node::block("opaque", Vec::new())),
            2 => {
                entity.connections = Some(vec![Connection {
                    output: "OnUser1".into(),
                    value: "target,FireUser1,,0,-1".into(),
                }])
            }
            3 => {
                let mut solid = base.vmf.world.solids[0].clone();
                solid.id += 1000;
                for side in &mut solid.sides {
                    side.id += 1000;
                }
                entity.solids.push(solid);
            }
            4 => {
                entity
                    .properties
                    .0
                    .iter_mut()
                    .find(|(key, _)| key == "BasisOrigin")
                    .unwrap()
                    .0 = "Basisorigin".into()
            }
            _ => unreachable!(),
        }
        cases.push(child);
    }
    for child in cases {
        let (parent, owner) = nested_parent(directory.path(), &child);
        let source = saved_parent(directory.path(), &parent);
        let output = compile_saved(&parent, &game, &source, Options::default());
        child_error(&output, owner, Some(base.helpers[0]), "");
        preserved(&output, directory.path());
    }
}

#[test]
fn sixty_four_direct_sides_remap_in_authored_order_without_extra_reference_geometry() {
    let directory = tempfile::tempdir().unwrap();
    let mut document = Document::empty();
    for index in 0..11 {
        let x = f64::from(index) * 24.0 - 128.0;
        block(
            &mut document,
            ([x, -8.0, 16.0], [x + 16.0, 8.0, 32.0]),
            index % 2 != 0,
        );
    }
    let mut child = document.into_vmf();
    let sides = child.side_ids().take(64).collect::<Vec<_>>();
    let order = sides.iter().rev().copied().collect::<Vec<_>>();
    let original = overlay(&mut child, &order, source_frame(0), 0);
    write_source(directory.path(), "leaf.vmf", &child);
    let mut parent = room();
    let owner = instance(&mut parent, "leaf.vmf", "1024 1024 1024", "0 0 0");
    let source = saved_parent(directory.path(), &parent);
    let prepared = resolve(&parent, &source);
    assert_eq!(prepared.generated_brushes().count(), 11);
    assert_eq!(prepared.stats().unwrap().overlays, 1);
    let actual = prepared
        .vmf()
        .entities
        .iter()
        .find(|entity| entity.classname() == Some("info_overlay"))
        .unwrap();
    assert_eq!(prepared.root_entity(actual.id), Some(owner));
    let side_map = prepared
        .generated_brushes()
        .flat_map(|brush| {
            brush.solid.sides.iter().map(|side| {
                let (actual_owner, file, _, original) = prepared.face_origin(side.id).unwrap();
                assert_eq!((actual_owner, file), (owner, "leaf.vmf"));
                (original, side.id)
            })
        })
        .collect::<BTreeMap<_, _>>();
    let expected = order.iter().map(|id| side_map[id]).collect::<Vec<_>>();
    let raw = actual.properties.get("sides").unwrap();
    assert!(raw.len() <= instances::MAX_FIELD_BYTES);
    assert_eq!(
        raw.split_whitespace()
            .map(|value| value.parse::<u32>().unwrap())
            .collect::<Vec<_>>(),
        expected
    );
    let mut report = compiler::Report::default();
    report.diagnostics.push(compiler::Diagnostic::error(
        "overlay provenance",
        Some(format!("entity:{}", actual.id)),
        None,
    ));
    prepared.annotate(&mut report);
    assert_eq!(
        report.diagnostics[0].object,
        Some(format!("entity:{owner}"))
    );
    assert!(
        report.diagnostics[0]
            .message
            .contains(&format!("instance leaf.vmf, child object {original}"))
    );
}

#[test]
fn targets_must_be_direct_visible_ordinary_sides_of_the_exact_child_occurrence() {
    let directory = tempfile::tempdir().unwrap();
    let game = game(directory.path());
    for case in [
        "hidden-world",
        "hidden-detail",
        "hidden-solid",
        "terrain-solid",
        "consumed-helper",
        "runtime-model",
        "transition",
        "nodraw",
    ] {
        let mut child = leaf(true);
        match case {
            "hidden-world" => child.vmf.world.solids[0].hidden = true,
            "hidden-detail" => {
                child
                    .vmf
                    .entities
                    .iter_mut()
                    .find(|entity| entity.classname() == Some("func_detail"))
                    .unwrap()
                    .hidden = true
            }
            "hidden-solid" => solid_mut(&mut child.vmf, child.solids[1]).hidden = true,
            "terrain-solid" => {
                let mut document = Document::new(child.vmf);
                document
                    .edit("terrain target", |tx| {
                        map_core::displacement::edit::create(
                            tx,
                            &[map_core::surface::FaceId {
                                solid: child.solids[0],
                                side: child.targets[0],
                            }],
                            2,
                            0,
                        )
                    })
                    .unwrap();
                child.vmf = document.into_vmf();
                let bottom = solid_mut(&mut child.vmf, child.solids[0]).sides[1].id;
                helper_mut(&mut child.vmf, child.helpers[0])
                    .properties
                    .set("sides", format!("{} {bottom}", child.targets[1]));
            }
            "consumed-helper" => {
                let detail = child
                    .vmf
                    .entities
                    .iter_mut()
                    .find(|entity| entity.classname() == Some("func_detail"))
                    .unwrap();
                detail.properties.set("classname", "func_occluder");
            }
            "runtime-model" => child
                .vmf
                .entities
                .iter_mut()
                .find(|entity| entity.classname() == Some("func_detail"))
                .unwrap()
                .properties
                .set("classname", "func_brush"),
            "transition" => helper_mut(&mut child.vmf, child.helpers[0])
                .properties
                .set("classname", "info_overlay_transition"),
            "nodraw" => {
                solid_mut(&mut child.vmf, child.solids[0]).sides[0].material =
                    "tools/toolsnodraw".into()
            }
            _ => unreachable!(),
        }
        let (parent, owner) = nested_parent(directory.path(), &child.vmf);
        let source = saved_parent(directory.path(), &parent);
        let output = compile_saved(&parent, &game, &source, Options::default());
        child_error(
            &output,
            owner,
            if matches!(case, "runtime-model" | "transition") {
                None
            } else {
                Some(child.helpers[0])
            },
            if case == "runtime-model" {
                "func_brush"
            } else if case == "transition" {
                "info_overlay_transition"
            } else {
                ""
            },
        );
        preserved(&output, directory.path());
    }
    for scope in ["root", "descendant", "sibling"] {
        let original = leaf(false);
        let mut physical = original.vmf.clone();
        physical
            .entities
            .retain(|entity| entity.classname() != Some("info_overlay"));
        write_source(directory.path(), "physical.vmf", &physical);
        let mut child = Document::empty().into_vmf();
        child.entities.push(
            original
                .vmf
                .entities
                .iter()
                .find(|entity| entity.id == original.helpers[0])
                .unwrap()
                .clone(),
        );
        helper_mut(&mut child, original.helpers[0])
            .properties
            .set("sides", original.targets[0].to_string());
        if scope == "descendant" {
            instance(&mut child, "physical.vmf", "0 0 0", "0 0 0");
        }
        write_source(directory.path(), "leaf.vmf", &child);
        let mut parent = room();
        if scope == "sibling" {
            instance(&mut parent, "physical.vmf", "1024 1024 1024", "0 0 0");
        }
        let owner = instance(&mut parent, "leaf.vmf", "1024 1024 1024", "0 0 0");
        assert!(parent.side_ids().any(|id| id == original.targets[0]));
        let source = saved_parent(directory.path(), &parent);
        let output = compile_saved(&parent, &game, &source, Options::default());
        child_error(&output, owner, Some(original.helpers[0]), "same child");
        preserved(&output, directory.path());
    }
}

#[test]
fn projection_and_marker_bounds_are_checked_at_source_and_each_intermediate_pose() {
    let directory = tempfile::tempdir().unwrap();
    let game = game(directory.path());
    for marker in [false, true] {
        for intermediate in [false, true] {
            let mut child = leaf(false);
            let helper = helper_mut(&mut child.vmf, child.helpers[0]);
            helper.properties.set(
                if marker { "origin" } else { "BasisOrigin" },
                if intermediate {
                    "600 0 48"
                } else {
                    "17000 0 48"
                },
            );
            write_source(directory.path(), "leaf.vmf", &child.vmf);
            let mut middle = Document::empty().into_vmf();
            instance(
                &mut middle,
                "leaf.vmf",
                if intermediate { "16000 0 0" } else { "0 0 0" },
                "0 0 0",
            );
            write_source(directory.path(), "middle.vmf", &middle);
            let mut parent = room();
            let owner = instance(
                &mut parent,
                "middle.vmf",
                if intermediate { "-16000 0 0" } else { "0 0 0" },
                "0 0 0",
            );
            let source = saved_parent(directory.path(), &parent);
            let output = compile_saved(&parent, &game, &source, Options::default());
            child_error(
                &output,
                owner,
                Some(child.helpers[0]),
                if marker {
                    "origin exceeds finite Source compile bounds"
                } else {
                    "must fit"
                },
            );
            assert!(output.stats.compile_region.is_none());
            preserved(&output, directory.path());
        }
    }
}

#[test]
fn region_omission_never_drops_missing_targets_and_late_buried_faces_keep_source_ownership() {
    let directory = tempfile::tempdir().unwrap();
    let game = game(directory.path());
    for omitted in [false, true] {
        let mut child = leaf(false);
        helper_mut(&mut child.vmf, child.helpers[0])
            .properties
            .set("origin", "4096 4096 4096");
        if omitted {
            edit::translate_solid(
                solid_mut(&mut child.vmf, child.solids[1]),
                DVec3::X * 512.0,
                true,
            );
        }
        write_source(directory.path(), "leaf.vmf", &child.vmf);
        let mut parent = room();
        parent.cordon = Some(Node::block(
            "cordon",
            vec![
                Node::string("active", "1"),
                Node::string("mins", "896 896 1028"),
                Node::string("maxs", "1152 1152 1276"),
            ],
        ));
        let owner = instance(&mut parent, "leaf.vmf", "1024 1024 1024", "0 0 0");
        let source = saved_parent(directory.path(), &parent);
        let output = compile_saved(&parent, &game, &source, Options::default());
        assert_eq!(output.stats.instances.as_ref().unwrap().overlays, 2);
        if omitted {
            child_error(&output, owner, Some(child.helpers[0]), "absent");
            preserved(&output, directory.path());
        } else {
            let bsp = successful(&output);
            assert_eq!(bsp.overlay_data().unwrap().unwrap().overlays.len(), 2);
            assert!(!bsp.entities().unwrap().contains("info_overlay_accessor"));
        }
    }
    let mut child = leaf(false);
    let world = solid_mut(&mut child.vmf, child.solids[0]);
    for side in &mut world.sides {
        for point in &mut side.plane.0 {
            point.z -= 16.0;
        }
    }
    let bottom = world.sides[1].id;
    helper_mut(&mut child.vmf, child.helpers[0])
        .properties
        .set("sides", bottom.to_string());
    write_source(directory.path(), "leaf.vmf", &child.vmf);
    let mut parent = room();
    let owner = instance(&mut parent, "leaf.vmf", "1024 1024 1024", "0 0 0");
    let source = saved_parent(directory.path(), &parent);
    let output = compile_saved(&parent, &game, &source, Options::default());
    child_error(
        &output,
        owner,
        Some(child.helpers[0]),
        "has no emitted world face",
    );
    preserved(&output, directory.path());
    let prepared = resolve(&parent, &source);
    let root = compiler::compile(prepared.vmf(), &game);
    assert!(root.report.diagnostics.iter().any(|diagnostic| {
        diagnostic.message.contains("has no emitted world face")
            && diagnostic
                .object
                .as_ref()
                .is_some_and(|object| object.starts_with("entity:"))
            && !diagnostic.message.contains("instance leaf.vmf")
    }));
    preserved(&root, directory.path());
}

#[test]
fn root_and_child_overlays_share_retained_record_and_compiled_face_budgets() {
    let directory = tempfile::tempdir().unwrap();
    let game = game(directory.path());
    let child = leaf(false);
    write_source(directory.path(), "leaf.vmf", &child.vmf);
    let mut parent = room();
    let floor = parent.world.solids[0].sides[0].id;
    let root_frame = Frame {
        basis: [1024.0, 1024.0, 1024.0],
        normal: [0.0, 0.0, 1.0],
        u: [1.0, 0.0, 0.0],
        v: [0.0, 1.0, 0.0],
        marker: None,
    };
    for _ in 0..1022 {
        overlay(&mut parent, &[floor], root_frame, 0);
    }
    let owner = instance(&mut parent, "leaf.vmf", "1024 1024 1024", "0 0 0");
    let source = saved_parent(directory.path(), &parent);
    let output = compile_saved(&parent, &game, &source, Options::default());
    assert_eq!(
        successful(&output)
            .overlay_data()
            .unwrap()
            .unwrap()
            .overlays
            .len(),
        1024
    );
    assert_eq!(output.stats.instances.as_ref().unwrap().overlays, 2);
    overlay(&mut parent, &[floor], root_frame, 0);
    let source = saved_parent(directory.path(), &parent);
    let output = compile_saved(&parent, &game, &source, Options::default());
    child_error(
        &output,
        owner,
        Some(child.helpers[1]),
        "1024-record allocation budget",
    );
    preserved(&output, directory.path());
    let mut document = Document::empty();
    let solid = block(
        &mut document,
        ([-160.0, -160.0, 0.0], [160.0, 160.0, 16.0]),
        false,
    );
    let mut child = document.into_vmf();
    for side in &mut solid_mut(&mut child, solid).sides {
        side.lightmap_scale = 1;
    }
    let target = child.world.solids[0].sides[0].id;
    let original = overlay(&mut child, &[target], source_frame(0), 0);
    write_source(directory.path(), "leaf.vmf", &child);
    let mut parent = room();
    let owner = instance(&mut parent, "leaf.vmf", "1024 1024 1024", "0 0 0");
    let source = saved_parent(directory.path(), &parent);
    let output = compile_saved(&parent, &game, &source, Options::default());
    child_error(&output, owner, Some(original), "compiled-face budget");
    preserved(&output, directory.path());
}

#[test]
fn aggregate_expansion_work_budget_is_shared_across_overlay_occurrences() {
    let directory = tempfile::tempdir().unwrap();
    let mut document = Document::empty();
    block(&mut document, ([-8.0, -8.0, 16.0], [8.0, 8.0, 32.0]), false);
    let mut child = document.into_vmf();
    let target = child.world.solids[0].sides[0].id;
    for _ in 0..256 {
        overlay(&mut child, &[target], source_frame(0), 0);
    }
    write_source(directory.path(), "leaf.vmf", &child);
    let mut parent = room();
    for _ in 0..20 {
        instance(&mut parent, "leaf.vmf", "1024 1024 1024", "0 0 0");
    }
    let source = saved_parent(directory.path(), &parent);
    let output = compile_saved(&parent, &common::empty_game(), &source, Options::default());
    assert!(output.bsp.is_none() && output.leak.is_none());
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("geometry-work")),
        "{:?}",
        output.report
    );
    preserved(&output, directory.path());
}

#[test]
fn transitive_source_history_cancellation_and_late_publication_checks_preserve_custody() {
    let directory = tempfile::tempdir().unwrap();
    let game = game(directory.path());
    let child = leaf(true);
    let (parent, owner) = nested_parent(directory.path(), &child.vmf);
    let mut document = Document::new(parent);
    document
        .edit("authored instance name", |tx| {
            tx.entity_mut(owner)?
                .properties
                .set("targetname", "overlay_assembly");
            Ok(())
        })
        .unwrap();
    let before = document.vmf().clone();
    let history = document
        .undo_labels()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let source = saved_parent(directory.path(), document.vmf());
    let originals = ["parent.vmf", "middle.vmf", "leaf.vmf"]
        .map(|name| std::fs::read(directory.path().join(name)).unwrap());
    let output = compile_saved(document.vmf(), &game, &source, Options::default());
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
    let staged = output
        .prepare_write_controlled(&path, &Control::default())
        .unwrap()
        .unwrap();
    let mut changed = originals[2].clone();
    changed.extend_from_slice(b"\n");
    std::fs::write(directory.path().join("leaf.vmf"), changed).unwrap();
    assert!(staged.commit(&Control::default()).is_err());
    assert!(output.write(&path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"previous BSP");
    assert_eq!(
        std::fs::read(path.with_extension("lin")).unwrap(),
        b"previous LIN"
    );
    std::fs::write(directory.path().join("leaf.vmf"), &originals[2]).unwrap();
    let cancelled_output = compiler::compile_with_source_controlled(
        document.vmf(),
        &game,
        Options::default(),
        None,
        Some(&source),
        &cancelled,
    );
    assert!(cancelled_output.bsp.is_none() && cancelled_output.leak.is_none());
    assert!(
        cancelled_output
            .report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("cancelled"))
    );
    let mut changed_parent = originals[0].clone();
    changed_parent.extend_from_slice(b"\n");
    std::fs::write(directory.path().join("parent.vmf"), changed_parent).unwrap();
    let changed = compile_saved(document.vmf(), &game, &source, Options::default());
    assert!(changed.bsp.is_none() && changed.report.has_errors());
    preserved(&changed, directory.path());
    std::fs::write(directory.path().join("parent.vmf"), &originals[0]).unwrap();
    assert_eq!(document.vmf(), &before);
    assert_eq!(
        document.undo_labels().collect::<Vec<_>>(),
        history.iter().map(String::as_str).collect::<Vec<_>>()
    );
    assert_eq!(document.redo_labels().count(), 0);
    for (name, bytes) in ["parent.vmf", "middle.vmf", "leaf.vmf"]
        .into_iter()
        .zip(originals)
    {
        assert_eq!(std::fs::read(directory.path().join(name)).unwrap(), bytes);
    }
}
