#[path = "common/areas.rs"]
#[allow(dead_code)]
mod areas;
mod common;

use std::collections::BTreeSet;
use std::path::Path;

use compiler::instances::{self, PreparedInstances, SourceContext};
use compiler::{Control, LightingMode, Options, VisibilityMode};
use glam::DVec3;
use map_core::{BrushGeometry, Document, ObjectId, edit};
use source_formats::bsp::{self, Area, Bsp, Model, OCCLUDER_INACTIVE, Plane};
use source_formats::fgd::Catalog;
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::keyvalues::Node;
use source_formats::vmf::{Connection, Entity, Vmf};

type Bounds = ([f64; 3], [f64; 3]);
const ZXY_BLOCK_SIDE_ORDER: [usize; 6] = [3, 2, 5, 4, 0, 1];

fn game(directory: &Path) -> GameFs {
    let materials = directory.join("materials/tools");
    std::fs::create_dir_all(&materials).unwrap();
    for (name, flags) in [
        ("toolsoccluder", "$translucent 1 %compilenolight 1"),
        ("toolstrigger", "%compiletrigger 1"),
        ("toolsnodraw", "%compilenodraw 1"),
        ("toolsskip", "%compileskip 1"),
        ("toolshint", "%compilehint 1"),
        ("toolsorigin", "%compileorigin 1"),
        ("toolsareaportal", "%compileareaportal 1"),
        ("toolsblack", ""),
    ] {
        std::fs::write(
            materials.join(format!("{name}.vmt")),
            format!("UnlitGeneric {{ {flags} }}"),
        )
        .unwrap();
    }
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.to_owned()),
    });
    game
}

fn write_source(directory: &Path, name: &str, source: &Vmf) -> Vec<u8> {
    let bytes = source.write(&Default::default()).into_bytes();
    std::fs::write(directory.join(name), &bytes).unwrap();
    bytes
}

fn saved_parent(directory: &Path, source: &Vmf) -> SourceContext {
    write_source(directory, "parent.vmf", source);
    SourceContext::capture(&directory.join("parent.vmf"), &Control::default()).unwrap()
}

fn instance(source: &mut Vmf, file: &str, origin: &str, angles: &str) -> u32 {
    let id = source.object_ids().chain(source.side_ids()).max().unwrap() + 1;
    let mut entity = Entity::new(id, "func_instance");
    entity.properties.set("file", file);
    entity.properties.set("origin", origin);
    entity.properties.set("angles", angles);
    source.entities.push(entity);
    id
}

fn helper(document: &mut Document, bounds: &[Bounds], active: Option<&str>) -> u32 {
    let object = document
        .edit("occluder", |tx| {
            let mut solids = Vec::new();
            for &(mins, maxs) in bounds {
                let ObjectId::Solid(id) = edit::create_block(
                    tx,
                    DVec3::from_array(mins),
                    DVec3::from_array(maxs),
                    "tools/toolsoccluder",
                )?
                else {
                    unreachable!()
                };
                solids.push(id);
            }
            let properties = active
                .map(|value| vec![("StartActive".into(), value.into())])
                .unwrap_or_default();
            edit::tie_to_entity(tx, &solids, "func_occluder", &properties)
        })
        .unwrap();
    let ObjectId::Entity(id) = object else {
        unreachable!()
    };
    id
}

fn helper_file(bounds: &[Bounds], active: Option<&str>) -> Vmf {
    let mut document = Document::empty();
    helper(&mut document, bounds, active);
    document.into_vmf()
}

fn entity_mut(source: &mut Vmf, id: u32) -> &mut Entity {
    source
        .entities
        .iter_mut()
        .find(|entity| entity.id == id)
        .unwrap()
}

fn room() -> Vmf {
    let mut source = common::room();
    for solid in &mut source.world.solids {
        edit::translate_solid(solid, DVec3::splat(1024.0), true);
        for side in &mut solid.sides {
            side.lightmap_scale = 128;
        }
    }
    for entity in &mut source.entities {
        let origin = entity.origin().unwrap() + DVec3::splat(1024.0);
        entity
            .properties
            .set("origin", source_formats::vmf::format_vec3(origin));
    }
    let mut document = Document::new(source);
    document
        .edit("light", |tx| {
            edit::create_entity(
                tx,
                "light",
                Some(DVec3::new(960.0, 960.0, 1216.0)),
                &[
                    ("_light".into(), "255 160 80 180".into()),
                    ("_lightHDR".into(), "255 160 80 180".into()),
                    ("_constant_attn".into(), "1".into()),
                    ("_quadratic_attn".into(), "0".into()),
                ],
                Vec::new(),
            )
        })
        .unwrap();
    document.into_vmf()
}

fn region(source: &mut Vmf) {
    source.cordon = Some(Node::block(
        "cordon",
        vec![
            Node::string("active", "1"),
            Node::string("mins", "896 896 1028"),
            Node::string("maxs", "1152 1152 1276"),
        ],
    ));
}

fn compile_saved(
    source: &Vmf,
    game: &GameFs,
    options: Options,
    context: &SourceContext,
) -> compiler::Output {
    compiler::compile_with_source_controlled(
        source,
        game,
        options,
        None,
        Some(context),
        &Control::default(),
    )
}

fn successful(output: compiler::Output) -> Bsp {
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.leak.is_none());
    Bsp::parse(&output.bsp.unwrap().to_bytes()).unwrap()
}

fn resolve<'a>(source: &'a Vmf, context: &SourceContext) -> PreparedInstances<'a> {
    let mut report = compiler::Report::default();
    let prepared =
        instances::resolve(source, Some(context), &Control::default(), &mut report).unwrap();
    assert!(!report.has_errors(), "{report:?}");
    prepared
}

fn assert_child_error(output: &compiler::Output, owner: u32, original: u32, reason: &str) {
    assert!(output.report.has_errors() && output.bsp.is_none() && output.leak.is_none());
    assert!(
        output.report.diagnostics.iter().any(|diagnostic| {
            diagnostic.object == Some(format!("entity:{owner}"))
                && diagnostic.message.contains("leaf.vmf")
                && diagnostic.message.contains(reason)
                && [
                    format!("child object {original}"),
                    format!("Child occluder {original}"),
                    format!("Child brush {original}"),
                    format!("Child side {original}"),
                ]
                .iter()
                .any(|identity| diagnostic.message.contains(identity))
        }),
        "{reason}: {:?}",
        output.report
    );
}

fn assert_preserved(output: &compiler::Output, directory: &Path) {
    let path = directory.join("previous.bsp");
    std::fs::write(&path, b"previous BSP").unwrap();
    std::fs::write(path.with_extension("lin"), b"previous LIN").unwrap();
    assert!(!output.write(&path).unwrap());
    assert_eq!(std::fs::read(path).unwrap(), b"previous BSP");
    assert_eq!(
        std::fs::read(directory.join("previous.lin")).unwrap(),
        b"previous LIN"
    );
}

fn map_generated_ids(literal: &mut Entity, generated: &Entity) {
    literal.id = generated.id;
    assert_eq!(literal.solids.len(), generated.solids.len());
    for (literal, generated) in literal.solids.iter_mut().zip(&generated.solids) {
        literal.id = generated.id;
        assert_eq!(literal.sides.len(), generated.sides.len());
        for (literal, generated) in literal.sides.iter_mut().zip(&generated.sides) {
            literal.id = generated.id;
        }
    }
}

fn assert_native_geometry(bsp: &Bsp, expected_polygons: &[i32], expected_bounds: &[Bounds]) {
    let data = bsp.occlusion().unwrap().unwrap();
    let planes = bsp.records::<Plane>().unwrap();
    let vertices = bsp.vertices().unwrap();
    data.validate(
        planes.len(),
        vertices.len(),
        bsp.records::<Area>().unwrap().len(),
    )
    .unwrap();
    assert_eq!(data.occluders.len(), expected_polygons.len());
    let mut first_polygon = 0;
    for ((record, &count), &(mins, maxs)) in data
        .occluders
        .iter()
        .zip(expected_polygons)
        .zip(expected_bounds)
    {
        assert_eq!(record.first_polygon, first_polygon);
        assert_eq!(record.polygon_count, count);
        assert_eq!(record.mins, mins.map(|value| value as f32));
        assert_eq!(record.maxs, maxs.map(|value| value as f32));
        assert!(record.area > 0);
        first_polygon += count;
    }
    for polygon in &data.polygons {
        let plane = planes[polygon.plane as usize];
        let normal = DVec3::from_array(plane.normal.map(f64::from));
        let points = data.vertex_indices[polygon.first_vertex_index as usize..]
            [..polygon.vertex_count as usize]
            .iter()
            .map(|&index| DVec3::from_array(vertices[index as usize].map(f64::from)))
            .collect::<Vec<_>>();
        assert!(
            points
                .iter()
                .all(|point| (normal.dot(*point) - f64::from(plane.dist)).abs() < 0.001)
        );
        assert!(
            (points[2] - points[0])
                .cross(points[1] - points[0])
                .dot(normal)
                > 0.0
        );
    }
}

fn assert_physical_unchanged(baseline: &Bsp, actual: &Bsp) {
    for (index, expected) in baseline.lumps.iter().enumerate() {
        if ![
            bsp::lump::ENTITIES,
            bsp::lump::PLANES,
            bsp::lump::VERTEXES,
            bsp::lump::OCCLUSION,
            bsp::lump::VERTNORMALS,
        ]
        .contains(&index)
        {
            assert_eq!(
                &actual.lumps[index], expected,
                "physical/visibility/lighting lump {index}"
            );
        }
    }
    for lump in [bsp::lump::PLANES, bsp::lump::VERTNORMALS] {
        assert_eq!(actual.lumps[lump].version, baseline.lumps[lump].version);
        assert_eq!(actual.lumps[lump].fourcc, baseline.lumps[lump].fourcc);
    }
    let old_normals = baseline.values::<[f32; 3]>(bsp::lump::VERTNORMALS).unwrap();
    let normals = actual.values::<[f32; 3]>(bsp::lump::VERTNORMALS).unwrap();
    for (label, bsp, normals) in [
        ("helper-free", baseline, &old_normals),
        ("with helpers", actual, &normals),
    ] {
        let planes = bsp.records::<Plane>().unwrap();
        assert_eq!(
            normals.len(),
            planes.len(),
            "{label} normal/plane table lengths"
        );
        for (index, (normal, plane)) in normals.iter().zip(&planes).enumerate() {
            assert!(normal.iter().all(|value| value.is_finite()) && plane.dist.is_finite());
            assert_eq!(
                normal.map(f32::to_bits),
                plane.normal.map(f32::to_bits),
                "{label} normal/plane slot {index}"
            );
        }
    }
    let normal_indices = actual.values::<u16>(bsp::lump::VERTNORMALINDICES).unwrap();
    assert!(!normal_indices.is_empty());
    let mut plane_indices = BTreeSet::new();
    for index in normal_indices {
        let index = usize::from(index);
        assert_eq!(
            normals[index].map(f32::to_bits),
            old_normals[index].map(f32::to_bits),
            "physical normal slot {index}"
        );
        plane_indices.insert(index);
    }
    plane_indices.extend(
        actual
            .records::<bsp::Node>()
            .unwrap()
            .iter()
            .map(|node| usize::try_from(node.plane).unwrap()),
    );
    plane_indices.extend(
        actual
            .records::<bsp::BrushSide>()
            .unwrap()
            .iter()
            .map(|side| usize::from(side.plane)),
    );
    plane_indices.extend(
        actual
            .records::<bsp::AreaPortal>()
            .unwrap()
            .iter()
            .map(|portal| usize::try_from(portal.plane).unwrap()),
    );
    for lump in [
        bsp::lump::FACES,
        bsp::lump::ORIGINALFACES,
        bsp::lump::FACES_HDR,
    ] {
        plane_indices.extend(
            actual
                .values::<bsp::Face>(lump)
                .unwrap()
                .iter()
                .map(|face| usize::from(face.plane)),
        );
    }
    for index in plane_indices {
        let first = index * size_of::<Plane>();
        let end = first + size_of::<Plane>();
        assert_eq!(
            &actual.lumps[bsp::lump::PLANES].data[first..end],
            &baseline.lumps[bsp::lump::PLANES].data[first..end],
            "physical plane slot {index}"
        );
    }
    assert_eq!(actual.records::<Model>().unwrap().len(), 1);
    let visibility = actual.visibility().unwrap().unwrap();
    let old_visibility = baseline.visibility().unwrap().unwrap();
    assert_eq!(visibility.clusters(), old_visibility.clusters());
    for cluster in 0..visibility.clusters() {
        for kind in [bsp::VisibilityKind::Visible, bsp::VisibilityKind::Audible] {
            assert_eq!(
                visibility.row(cluster, kind).unwrap(),
                old_visibility.row(cluster, kind).unwrap()
            );
        }
    }
    for kind in [bsp::LightingKind::Ldr, bsp::LightingKind::Hdr] {
        assert!(!actual.world_lights(kind).unwrap().is_empty());
        assert_eq!(
            actual.world_lights(kind).unwrap(),
            baseline.world_lights(kind).unwrap()
        );
        assert_eq!(
            actual.leaf_ambient(kind).unwrap(),
            baseline.leaf_ambient(kind).unwrap()
        );
    }
}

#[test]
fn repeated_nested_noncommuting_groups_match_literal_native_physics_visibility_and_lighting() {
    let directory = tempfile::tempdir().unwrap();
    let game = game(directory.path());
    let mut leaf = Document::empty();
    let islands = helper(
        &mut leaf,
        &[
            ([-64.0, -48.0, -96.0], [-32.0, 48.0, -80.0]),
            ([32.0, -48.0, -96.0], [64.0, 48.0, -80.0]),
        ],
        None,
    );
    let overlap = helper(
        &mut leaf,
        &[
            ([-64.0, -48.0, 0.0], [16.0, 48.0, 16.0]),
            ([-16.0, -48.0, 0.0], [64.0, 48.0, 16.0]),
        ],
        Some("0"),
    );
    let mut leaf = leaf.into_vmf();
    entity_mut(&mut leaf, islands)
        .properties
        .set("origin", "3 5 7");
    let leaf_bytes = write_source(directory.path(), "leaf.vmf", &leaf);
    let mut middle = Document::empty().into_vmf();
    instance(&mut middle, "leaf.vmf", "16 32 0", "0 90 0");
    let middle_bytes = write_source(directory.path(), "middle.vmf", &middle);
    let mut base = Document::new(room());
    let root = helper(
        &mut base,
        &[([880.0, 984.0, 1056.0], [896.0, 1064.0, 1152.0])],
        Some("1"),
    );
    let base = base.into_vmf();
    let mut parent = base.clone();
    let owners = [
        instance(&mut parent, "middle.vmf", "1104 1040 1152", "90 0 0"),
        instance(&mut parent, "middle.vmf", "1104 848 1152", "90 0 0"),
    ];
    let before = parent.clone();
    let context = saved_parent(directory.path(), &parent);
    let prepared = resolve(&parent, &context);
    let stats = prepared.stats().unwrap();
    assert_eq!(
        (stats.instances, stats.files, stats.max_expansion_depth),
        (4, 2, 2)
    );
    assert_eq!(
        (stats.occluders, stats.world_brushes, stats.detail_brushes),
        (4, 0, 0)
    );
    assert_eq!(prepared.generated_brushes().count(), 0);
    let generated = prepared
        .vmf()
        .entities
        .iter()
        .filter(|entity| entity.classname() == Some("func_occluder") && entity.id != root)
        .collect::<Vec<_>>();
    assert_eq!(generated.len(), 4);
    let mut object_ids = BTreeSet::new();
    let mut side_ids = BTreeSet::new();
    let mut flat = Document::new(base.clone());
    let mut literal_ids = Vec::new();
    for shift in [0.0, -192.0] {
        literal_ids.push(helper(
            &mut flat,
            &[
                (
                    [1008.0, 1008.0 + shift, 1088.0],
                    [1024.0, 1040.0 + shift, 1184.0],
                ),
                (
                    [1008.0, 1104.0 + shift, 1088.0],
                    [1024.0, 1136.0 + shift, 1184.0],
                ),
            ],
            None,
        ));
        literal_ids.push(helper(
            &mut flat,
            &[
                (
                    [1104.0, 1008.0 + shift, 1088.0],
                    [1120.0, 1088.0 + shift, 1184.0],
                ),
                (
                    [1104.0, 1056.0 + shift, 1088.0],
                    [1120.0, 1136.0 + shift, 1184.0],
                ),
            ],
            Some("0"),
        ));
    }
    let mut flat = flat.into_vmf();
    for (index, (&literal_id, actual)) in literal_ids.iter().zip(&generated).enumerate() {
        let source = leaf
            .entities
            .iter()
            .find(|entity| entity.id == [islands, overlap][index % 2])
            .unwrap();
        let shift = if index < 2 { 0.0 } else { -192.0 };
        assert_eq!(prepared.root_entity(actual.id), Some(owners[index / 2]));
        assert_eq!(actual.solids.len(), 2);
        assert_eq!(actual.properties.get("angles"), None);
        assert_eq!(actual.properties.get("targetname"), None);
        assert_eq!(
            actual.properties.get("StartActive"),
            if index % 2 == 0 { None } else { Some("0") }
        );
        if index % 2 == 0 {
            assert_eq!(
                actual.origin(),
                Some(DVec3::new(1111.0, 1075.0 + shift, 1141.0))
            );
        } else {
            assert_eq!(actual.properties.get("origin"), None);
        }
        assert!(object_ids.insert(actual.id));
        for (actual_solid, source_solid) in actual.solids.iter().zip(&source.solids) {
            assert!(object_ids.insert(actual_solid.id));
            let mut report = compiler::Report::default();
            report.diagnostics.push(compiler::Diagnostic::error(
                "solid provenance",
                Some(format!("solid:{}", actual_solid.id)),
                None,
            ));
            prepared.annotate(&mut report);
            assert_eq!(
                report.diagnostics[0].object,
                Some(format!("entity:{}", owners[index / 2]))
            );
            assert!(report.diagnostics[0].message.contains(&format!(
                "instance leaf.vmf, child object {}",
                source_solid.id
            )));
            for (actual_side, source_side) in actual_solid.sides.iter().zip(&source_solid.sides) {
                assert!(side_ids.insert(actual_side.id));
                assert_eq!(
                    prepared.face_origin(actual_side.id),
                    Some((
                        owners[index / 2],
                        "leaf.vmf",
                        source_solid.id,
                        source_side.id
                    ))
                );
                for (point, original) in actual_side.plane.0.iter().zip(source_side.plane.0) {
                    let expected = DVec3::new(
                        original.z + 1104.0,
                        original.x + 1072.0 + shift,
                        original.y + 1136.0,
                    );
                    assert!(
                        point.abs_diff_eq(expected, 1e-8),
                        "{point:?} != {expected:?}"
                    );
                    assert!(
                        map_core::texel_uv(actual_side, expected)
                            .abs_diff_eq(map_core::texel_uv(source_side, original), 1e-8)
                    );
                }
            }
        }
        let literal = entity_mut(&mut flat, literal_id);
        if index % 2 == 0 {
            literal
                .properties
                .set("origin", format!("1111 {} 1141", 1075.0 + shift));
        }
        for solid in &mut literal.solids {
            let sides = solid.sides.clone();
            solid.sides = ZXY_BLOCK_SIDE_ORDER
                .map(|side| sides[side].clone())
                .to_vec();
        }
        for (literal_solid, actual_solid) in literal.solids.iter().zip(&actual.solids) {
            let expected = BrushGeometry::from_solid(literal_solid).unwrap();
            let actual = BrushGeometry::from_solid(actual_solid).unwrap();
            assert!(actual.mins.abs_diff_eq(expected.mins, 1e-8));
            assert!(actual.maxs.abs_diff_eq(expected.maxs, 1e-8));
        }
        map_generated_ids(literal, actual);
    }
    for visibility in [VisibilityMode::Fast, VisibilityMode::Detailed] {
        let options = Options {
            visibility,
            lighting: LightingMode::DirectLdrHdr,
            ..Default::default()
        };
        let baseline = successful(compiler::compile_with_options(&room(), &game, options));
        let expected = successful(compiler::compile_with_options(&flat, &game, options));
        let output = compile_saved(&parent, &game, options, &context);
        assert!(!output.report.has_errors(), "{:?}", output.report);
        assert_eq!(output.stats.occluders, 5);
        assert_eq!(output.stats.instances.as_ref().unwrap().occluders, 4);
        let actual = successful(output);
        assert_eq!(actual.to_bytes(), expected.to_bytes());
        assert_physical_unchanged(&baseline, &actual);
        assert_native_geometry(
            &actual,
            &[6, 12, 10, 12, 10],
            &[
                ([880.0, 984.0, 1056.0], [896.0, 1064.0, 1152.0]),
                ([1008.0, 1008.0, 1088.0], [1024.0, 1136.0, 1184.0]),
                ([1104.0, 1008.0, 1088.0], [1120.0, 1136.0, 1184.0]),
                ([1008.0, 816.0, 1088.0], [1024.0, 944.0, 1184.0]),
                ([1104.0, 816.0, 1088.0], [1120.0, 944.0, 1184.0]),
            ],
        );
        let native = actual.occlusion().unwrap().unwrap();
        assert_eq!(
            native
                .occluders
                .iter()
                .map(|record| record.flags)
                .collect::<Vec<_>>(),
            [0, 0, OCCLUDER_INACTIVE, 0, OCCLUDER_INACTIVE]
        );
        let entities = actual.entities().unwrap();
        assert_eq!(entities.matches("\"occludernumber\"").count(), 5);
        for index in 0..5 {
            assert!(entities.contains(&format!("\"occludernumber\" \"{index}\"")));
        }
        assert!(
            !entities.contains("func_instance")
                && !entities.contains("func_detail")
                && !entities.contains("\"model\"")
        );
    }
    assert_eq!(parent, before);
    assert_eq!(
        std::fs::read(directory.path().join("leaf.vmf")).unwrap(),
        leaf_bytes
    );
    assert_eq!(
        std::fs::read(directory.path().join("middle.vmf")).unwrap(),
        middle_bytes
    );
    assert_eq!(
        std::fs::read(directory.path().join("parent.vmf")).unwrap(),
        before.write(&Default::default()).into_bytes()
    );
}

#[test]
fn translated_mixed_geometry_exposes_only_physical_brushes_and_preserves_per_side_ownership() {
    let directory = tempfile::tempdir().unwrap();
    let game = game(directory.path());
    let mut child = Document::empty();
    child
        .edit("physical brushes", |tx| {
            edit::create_block(
                tx,
                DVec3::new(-160.0, -160.0, 0.0),
                DVec3::new(-128.0, -128.0, 32.0),
                "dev/dev_measuregeneric01",
            )?;
            let ObjectId::Solid(id) = edit::create_block(
                tx,
                DVec3::new(128.0, 128.0, 0.0),
                DVec3::new(160.0, 160.0, 32.0),
                "dev/dev_measuregeneric01",
            )?
            else {
                unreachable!()
            };
            edit::tie_to_entity(tx, &[id], "func_detail", &[])
        })
        .unwrap();
    let screen = helper(
        &mut child,
        &[([-8.0, -64.0, 16.0], [8.0, 64.0, 192.0])],
        Some("1"),
    );
    let mut child = child.into_vmf();
    let helper = entity_mut(&mut child, screen);
    helper.solids[0].sides[0].material = "tools/toolstrigger".into();
    for side in &mut helper.solids[0].sides[1..] {
        side.material = "tools/toolsnodraw".into();
    }
    let original = child.clone();
    write_source(directory.path(), "leaf.vmf", &child);
    let mut parent = room();
    let owner = instance(&mut parent, "leaf.vmf", "1024 1024 1024", "0 0 0");
    let context = saved_parent(directory.path(), &parent);
    let prepared = resolve(&parent, &context);
    let brushes = prepared.generated_brushes().collect::<Vec<_>>();
    assert_eq!(brushes.len(), 2);
    assert_eq!(brushes.iter().filter(|brush| brush.detail).count(), 1);
    assert!(
        brushes
            .iter()
            .all(|brush| brush.owner == owner && brush.file == "leaf.vmf")
    );
    assert_eq!(
        (
            prepared.stats().unwrap().world_brushes,
            prepared.stats().unwrap().detail_brushes,
            prepared.stats().unwrap().occluders
        ),
        (1, 1, 1)
    );
    let helper = prepared
        .vmf()
        .entities
        .iter()
        .find(|entity| entity.classname() == Some("func_occluder"))
        .unwrap();
    let source_helper = original
        .entities
        .iter()
        .find(|entity| entity.id == screen)
        .unwrap();
    for (actual, source) in helper.solids[0]
        .sides
        .iter()
        .zip(&source_helper.solids[0].sides)
    {
        assert_eq!(actual.material, source.material);
        assert_eq!(
            prepared.face_origin(actual.id),
            Some((owner, "leaf.vmf", source_helper.solids[0].id, source.id))
        );
    }
    let actual = successful(compile_saved(&parent, &game, Options::default(), &context));
    let mut physical = prepared.vmf().clone();
    physical
        .entities
        .retain(|entity| entity.classname() != Some("func_occluder"));
    let baseline = successful(compiler::compile(&physical, &game));
    for index in [
        bsp::lump::BRUSHES,
        bsp::lump::BRUSHSIDES,
        bsp::lump::LEAFBRUSHES,
        bsp::lump::FACES,
        bsp::lump::PHYSCOLLIDE,
        bsp::lump::VISIBILITY,
    ] {
        assert_eq!(actual.lumps[index], baseline.lumps[index], "lump {index}");
    }
    assert_native_geometry(
        &actual,
        &[1],
        &[([1016.0, 960.0, 1216.0], [1032.0, 1088.0, 1216.0])],
    );
}

#[test]
fn fgd_cased_state_default_inactive_and_neutral_fields_survive_without_synthetic_pose() {
    let catalog = Catalog::parse(
        r#"
        @BaseClass = Targetname [ targetname(target_source) : "Name" ]
        @SolidClass base(Targetname) = func_occluder [
            StartActive(choices) : "Initial State" : 1 = [ 0 : "Inactive" 1 : "Active" ]
        ]
    "#,
    )
    .unwrap();
    let defaults = catalog.class("func_occluder").unwrap().defaults();
    assert_eq!(defaults.get("StartActive").map(String::as_str), Some("1"));
    assert!(!defaults.contains_key("targetname"));
    for active in [
        None,
        defaults.get("StartActive").map(String::as_str),
        Some("0"),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let game = game(directory.path());
        let mut child = helper_file(&[([-8.0, -32.0, 16.0], [8.0, 32.0, 128.0])], active);
        for (key, value) in [
            ("targetname", ""),
            ("parentname", ""),
            ("target", ""),
            ("angles", "0 0 0"),
            ("angle", "0"),
            ("spawnflags", "0"),
            ("mindxlevel", "0"),
            ("maxdxlevel", "0"),
        ] {
            child.entities[0].properties.set(key, value);
        }
        write_source(directory.path(), "leaf.vmf", &child);
        let mut parent = room();
        instance(&mut parent, "leaf.vmf", "1024 1024 1024", "0 90 0");
        let context = saved_parent(directory.path(), &parent);
        let prepared = resolve(&parent, &context);
        let generated = prepared
            .vmf()
            .entities
            .iter()
            .find(|entity| entity.classname() == Some("func_occluder"))
            .unwrap();
        assert_eq!(generated.properties.get("StartActive"), active);
        assert_eq!(generated.properties.get("origin"), None);
        assert_eq!(generated.properties.get("angles"), Some("0 0 0"));
        assert_eq!(
            generated
                .properties
                .iter()
                .any(|(key, _)| key == "StartActive"),
            active.is_some()
        );
        let bsp = successful(compile_saved(&parent, &game, Options::default(), &context));
        assert_eq!(
            bsp.occlusion().unwrap().unwrap().occluders[0].flags,
            if active == Some("0") {
                OCCLUDER_INACTIVE
            } else {
                0
            }
        );
    }
}

#[test]
fn whole_group_regions_hidden_omission_and_expanded_counts_remain_distinct() {
    let directory = tempfile::tempdir().unwrap();
    let game = game(directory.path());
    let mut base = room();
    region(&mut base);
    let baseline = successful(compiler::compile(&base, &game));
    for state in [
        "contained",
        "outside",
        "crossing",
        "islands-crossing",
        "hidden",
        "all-hidden",
    ] {
        let bounds = if state == "islands-crossing" {
            vec![
                ([-64.0, -32.0, 16.0], [-48.0, 32.0, 160.0]),
                ([160.0, -32.0, 16.0], [176.0, 32.0, 160.0]),
            ]
        } else if state == "crossing" {
            vec![([120.0, -32.0, 16.0], [136.0, 32.0, 160.0])]
        } else {
            vec![([-8.0, -32.0, 16.0], [8.0, 32.0, 160.0])]
        };
        let mut child = helper_file(&bounds, None);
        let original = child.entities[0].id;
        child.entities[0].properties.set("origin", "4096 4096 4096");
        if state == "hidden" {
            child.entities[0].hidden = true;
            child.entities[0].properties.set("StartActive", "bad");
        } else if state == "all-hidden" {
            child.entities[0].solids[0].hidden = true;
            child.entities[0].solids[0].sides.clear();
        }
        write_source(directory.path(), "leaf.vmf", &child);
        let mut parent = base.clone();
        let owner = instance(
            &mut parent,
            "leaf.vmf",
            if state == "outside" {
                "1600 1024 1024"
            } else {
                "1024 1024 1024"
            },
            "0 0 0",
        );
        let context = saved_parent(directory.path(), &parent);
        let output = compile_saved(&parent, &game, Options::default(), &context);
        if state.contains("crossing") {
            assert_child_error(&output, owner, original, "compile-region boundary");
            assert_preserved(&output, directory.path());
        } else {
            let expanded = usize::from(!["hidden", "all-hidden"].contains(&state));
            let stats = output.stats.instances.as_ref().unwrap();
            assert_eq!(stats.occluders, expanded, "{state}");
            assert_eq!(
                serde_json::to_value(stats)
                    .unwrap()
                    .get("occluders")
                    .is_none(),
                expanded == 0
            );
            assert_eq!(output.stats.occluders, usize::from(state == "contained"));
            let actual = successful(output);
            if state == "contained" {
                assert_native_geometry(
                    &actual,
                    &[6],
                    &[([1016.0, 992.0, 1040.0], [1032.0, 1056.0, 1184.0])],
                );
            } else {
                assert_eq!(actual.to_bytes(), baseline.to_bytes(), "{state}");
            }
        }
    }
}

#[test]
fn root_and_children_share_the_1024_retained_limit_after_whole_helper_region_omission() {
    let directory = tempfile::tempdir().unwrap();
    let game = game(directory.path());
    let leaf = helper_file(&[([-8.0, -32.0, 16.0], [8.0, 32.0, 128.0])], None);
    write_source(directory.path(), "leaf.vmf", &leaf);
    let mut parent = room();
    region(&mut parent);
    let mut document = Document::new(parent);
    helper(
        &mut document,
        &[([1016.0, 992.0, 1040.0], [1032.0, 1056.0, 1152.0])],
        Some("1"),
    );
    let mut parent = document.into_vmf();
    let template = parent
        .entities
        .iter()
        .find(|entity| entity.classname() == Some("func_occluder"))
        .unwrap()
        .clone();
    let mut next = parent.object_ids().chain(parent.side_ids()).max().unwrap() + 1;
    for _ in 1..1023 {
        let mut entity = template.clone();
        entity.id = next;
        next += 1;
        for solid in &mut entity.solids {
            solid.id = next;
            next += 1;
            for side in &mut solid.sides {
                side.id = next;
                next += 1;
            }
        }
        parent.entities.push(entity);
    }
    instance(&mut parent, "leaf.vmf", "1024 1024 1024", "0 0 0");
    for _ in 0..2 {
        instance(&mut parent, "leaf.vmf", "1600 1024 1024", "0 0 0");
    }
    let context = saved_parent(directory.path(), &parent);
    let output = compile_saved(&parent, &game, Options::default(), &context);
    assert_eq!(output.stats.instances.as_ref().unwrap().occluders, 3);
    assert_eq!(output.stats.occluders, 1024);
    let bsp = successful(output);
    assert_eq!(bsp.occlusion().unwrap().unwrap().occluders.len(), 1024);
    assert!(
        bsp.entities()
            .unwrap()
            .contains("\"occludernumber\" \"1023\"")
    );
    let overflow = instance(&mut parent, "leaf.vmf", "1024 1024 1024", "0 0 0");
    let context = saved_parent(directory.path(), &parent);
    let output = compile_saved(&parent, &game, Options::default(), &context);
    assert_child_error(&output, overflow, leaf.entities[0].id, "at most 1024");
    assert_eq!(output.stats.instances.as_ref().unwrap().occluders, 4);
    assert_preserved(&output, directory.path());
}

#[test]
fn behavioral_properties_opaque_blocks_duplicates_and_empty_or_excess_brushes_refuse() {
    let directory = tempfile::tempdir().unwrap();
    let game = game(directory.path());
    let source = helper_file(&[([-8.0, -32.0, 16.0], [8.0, 32.0, 128.0])], None);
    let mut cases = Vec::new();
    for (key, value) in [
        ("StartActive", "2"),
        ("startactive", "1"),
        ("targetname", "screen"),
        ("targetname", " "),
        ("parentname", "\t"),
        ("target", " "),
        ("parentname", "moving"),
        ("target", "other"),
        ("globalname", "global"),
        ("lightingorigin", "probe"),
        ("spawnflags", "1"),
        ("mindxlevel", "90"),
        ("maxdxlevel", "90"),
        ("angles", "0 90 0"),
        ("angle", "1"),
        ("model", "*4"),
        ("occludernumber", "1"),
        ("hammerid", "4"),
        ("replace01", "$screen value"),
        ("unknown", "0"),
        ("Origin", "0 0 0"),
    ] {
        let mut child = source.clone();
        child.entities[0].properties.set(key, value);
        cases.push((format!("{key}={value}"), child));
    }
    for case in 0..7 {
        let mut child = source.clone();
        let entity = &mut child.entities[0];
        match case {
            0 => entity.properties.0.extend([
                ("StartActive".into(), "1".into()),
                ("startactive".into(), "0".into()),
            ]),
            1 => {
                entity.connections = Some(vec![Connection {
                    output: "OnUser1".into(),
                    value: "screen,Toggle,,0,-1".into(),
                }])
            }
            2 => entity.extra.push(Node::block("opaque", Vec::new())),
            3 => entity.solids.clear(),
            4 => {
                let solid = entity.solids[0].clone();
                for index in 1..17 {
                    let mut solid = solid.clone();
                    solid.id += 100 * index;
                    for side in &mut solid.sides {
                        side.id += 100 * index;
                    }
                    entity.solids.push(solid);
                }
            }
            5 => entity
                .properties
                .set("targetname", "x".repeat(instances::MAX_FIELD_BYTES + 1)),
            6 => {
                let template = entity.solids[0].clone();
                entity.solids.clear();
                for index in 0..3 {
                    let mut solid = template.clone();
                    solid.id = 100 + index;
                    solid.sides = (0..86)
                        .map(|side| {
                            let mut value = template.sides[side % 6].clone();
                            value.id = 1000 + index * 100 + side as u32;
                            value
                        })
                        .collect();
                    entity.solids.push(solid);
                }
            }
            _ => unreachable!(),
        }
        cases.push((format!("structure {case}"), child));
    }
    for (label, child) in cases {
        let bytes = write_source(directory.path(), "leaf.vmf", &child);
        let mut parent = room();
        let owner = instance(&mut parent, "leaf.vmf", "1024 1024 1024", "0 0 0");
        let before = parent.clone();
        let context = saved_parent(directory.path(), &parent);
        let output = compile_saved(&parent, &game, Options::default(), &context);
        assert_child_error(&output, owner, child.entities[0].id, "");
        assert_preserved(&output, directory.path());
        assert_eq!(parent, before, "{label}");
        assert_eq!(
            std::fs::read(directory.path().join("leaf.vmf")).unwrap(),
            bytes,
            "{label}"
        );
    }
}

#[test]
fn source_and_intermediate_geometry_and_origins_are_checked_before_region_omission() {
    let directory = tempfile::tempdir().unwrap();
    let game = game(directory.path());
    for origin_only in [false, true] {
        for intermediate in [false, true] {
            let mut child = helper_file(&[([600.0, 600.0, 64.0], [608.0, 608.0, 128.0])], None);
            if origin_only {
                child.entities[0]
                    .properties
                    .set("origin", if intermediate { "600 0 0" } else { "17000 0 0" });
                if intermediate {
                    for side in &mut child.entities[0].solids[0].sides {
                        for point in &mut side.plane.0 {
                            point.x -= 600.0;
                        }
                    }
                }
            } else if !intermediate {
                for side in &mut child.entities[0].solids[0].sides {
                    for point in &mut side.plane.0 {
                        point.x += 17000.0;
                    }
                }
            }
            write_source(directory.path(), "leaf.vmf", &child);
            let mut middle = Document::empty().into_vmf();
            instance(
                &mut middle,
                "leaf.vmf",
                if intermediate { "16000 0 0" } else { "0 0 0" },
                "0 0 0",
            );
            write_source(directory.path(), "middle.vmf", &middle);
            let mut parent = room();
            region(&mut parent);
            let owner = instance(
                &mut parent,
                "middle.vmf",
                if intermediate { "-16000 0 0" } else { "0 0 0" },
                "0 0 0",
            );
            let context = saved_parent(directory.path(), &parent);
            let output = compile_saved(&parent, &game, Options::default(), &context);
            assert_child_error(&output, owner, child.entities[0].id, "compile bounds");
            if !origin_only && !intermediate {
                assert!(output.report.diagnostics.iter().any(|diagnostic| {
                    diagnostic
                        .message
                        .contains(&format!("Child brush {}", child.entities[0].solids[0].id))
                }));
            }
            assert!(output.stats.compile_region.is_none());
            assert_preserved(&output, directory.path());
        }
    }
}

#[test]
fn retained_child_material_geometry_and_area_failures_keep_original_provenance() {
    let directory = tempfile::tempdir().unwrap();
    let game = game(directory.path());
    for material in [
        "missing/occluder",
        "tools/toolsskip",
        "tools/toolshint",
        "tools/toolsorigin",
        "tools/toolsnodraw",
    ] {
        let mut child = helper_file(&[([-8.0, -32.0, 16.0], [8.0, 32.0, 128.0])], None);
        for side in &mut child.entities[0].solids[0].sides {
            side.material = material.into();
        }
        write_source(directory.path(), "leaf.vmf", &child);
        let mut parent = room();
        let owner = instance(&mut parent, "leaf.vmf", "1024 1024 1024", "0 0 0");
        let context = saved_parent(directory.path(), &parent);
        let output = compile_saved(&parent, &game, Options::default(), &context);
        assert_child_error(&output, owner, child.entities[0].id, "Occluder");
        assert_preserved(&output, directory.path());
    }
    for case in [
        "terrain",
        "solid-metadata",
        "side-metadata",
        "texture-scale",
        "degenerate",
        "uncertain-contact",
    ] {
        let mut child = helper_file(
            &[
                ([-64.0, -32.0, 16.0], [-48.0, 32.0, 128.0]),
                ([48.0, -32.0, 16.0], [64.0, 32.0, 128.0]),
            ],
            None,
        );
        let entity = &mut child.entities[0];
        let mut original = entity.id;
        match case {
            "terrain" => {
                let mut document = Document::empty();
                document
                    .edit("terrain", |tx| {
                        let ObjectId::Solid(solid) = edit::create_block(
                            tx,
                            DVec3::new(-64.0, -32.0, 16.0),
                            DVec3::new(-48.0, 32.0, 128.0),
                            "tools/toolsoccluder",
                        )?
                        else {
                            unreachable!()
                        };
                        let side = tx.solid_mut(solid)?.sides[0].id;
                        map_core::displacement::edit::create(
                            tx,
                            &[map_core::surface::FaceId { solid, side }],
                            2,
                            0,
                        )
                    })
                    .unwrap();
                entity.solids[0].sides[0].displacement =
                    document.vmf().world.solids[0].sides[0].displacement.clone();
            }
            "solid-metadata" => {
                original = entity.solids[0].id;
                entity.solids[0]
                    .extra
                    .push(Node::block("opaque", Vec::new()));
            }
            "side-metadata" => {
                original = entity.solids[0].sides[0].id;
                entity.solids[0].sides[0]
                    .extra
                    .push(Node::block("opaque", Vec::new()));
            }
            "texture-scale" => {
                original = entity.solids[0].sides[0].id;
                entity.solids[0].sides[0].u_axis.scale = 0.0;
            }
            "degenerate" => {
                original = entity.solids[0].id;
                entity.solids[0].sides.clear();
            }
            "uncertain-contact" => {
                edit::translate_solid(&mut entity.solids[1], DVec3::new(-95.995, 0.0, 0.0), true)
            }
            _ => unreachable!(),
        }
        write_source(directory.path(), "leaf.vmf", &child);
        let mut parent = room();
        let owner = instance(&mut parent, "leaf.vmf", "1024 1024 1024", "0 0 0");
        let context = saved_parent(directory.path(), &parent);
        let output = compile_saved(&parent, &game, Options::default(), &context);
        assert_child_error(&output, owner, original, "");
        assert_preserved(&output, directory.path());
    }
    for two_areas in [false, true] {
        let child = if two_areas {
            helper_file(
                &[
                    ([-160.0, -32.0, 16.0], [-144.0, 32.0, 160.0]),
                    ([144.0, -32.0, 16.0], [160.0, 32.0, 160.0]),
                ],
                None,
            )
        } else {
            helper_file(&[([600.0, -32.0, 16.0], [616.0, 32.0, 160.0])], None)
        };
        write_source(directory.path(), "leaf.vmf", &child);
        let mut parent = if two_areas {
            areas::room(true).0
        } else {
            common::room()
        };
        let owner = instance(&mut parent, "leaf.vmf", "0 0 0", "0 0 0");
        let context = saved_parent(directory.path(), &parent);
        let output = compile_saved(&parent, &game, Options::default(), &context);
        assert_child_error(
            &output,
            owner,
            child.entities[0].id,
            if two_areas {
                "spans 2 visibility areas"
            } else {
                "Occluder"
            },
        );
        assert_preserved(&output, directory.path());
    }
}

#[test]
fn transitive_source_history_cancellation_and_dependency_changes_guard_publication() {
    let directory = tempfile::tempdir().unwrap();
    let game = game(directory.path());
    let child = helper_file(&[([-8.0, -32.0, 16.0], [8.0, 32.0, 128.0])], None);
    let child_bytes = write_source(directory.path(), "leaf.vmf", &child);
    let mut middle = Document::empty().into_vmf();
    instance(&mut middle, "leaf.vmf", "0 0 0", "0 0 0");
    let middle_bytes = write_source(directory.path(), "middle.vmf", &middle);
    let mut parent = room();
    let owner = instance(&mut parent, "middle.vmf", "1024 1024 1024", "0 0 0");
    let mut document = Document::new(parent);
    document
        .edit("authored instance name", |tx| {
            tx.entity_mut(owner)?
                .properties
                .set("targetname", "assembly");
            Ok(())
        })
        .unwrap();
    let before = document.vmf().clone();
    let history = document
        .undo_labels()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let context = saved_parent(directory.path(), document.vmf());
    let parent_bytes = std::fs::read(directory.path().join("parent.vmf")).unwrap();
    let output = compile_saved(document.vmf(), &game, Options::default(), &context);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    let stats = output.stats.instances.as_ref().unwrap();
    assert_eq!(
        (
            stats.files,
            stats.occluders,
            stats.world_brushes,
            stats.detail_brushes
        ),
        (2, 1, 0, 0)
    );
    assert_eq!(
        stats
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
    let mut changed = child_bytes.clone();
    changed.extend_from_slice(b"\n");
    std::fs::write(directory.path().join("leaf.vmf"), &changed).unwrap();
    assert!(staged.commit(&Control::default()).is_err());
    assert!(output.write(&path).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"previous BSP");
    assert_eq!(
        std::fs::read(path.with_extension("lin")).unwrap(),
        b"previous LIN"
    );
    std::fs::write(directory.path().join("leaf.vmf"), &child_bytes).unwrap();
    let cancelled_output = compiler::compile_with_source_controlled(
        document.vmf(),
        &game,
        Options::default(),
        None,
        Some(&context),
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
    assert_eq!(document.vmf(), &before);
    assert_eq!(
        document.undo_labels().collect::<Vec<_>>(),
        history.iter().map(String::as_str).collect::<Vec<_>>()
    );
    assert_eq!(document.redo_labels().count(), 0);
    assert_eq!(
        std::fs::read(directory.path().join("parent.vmf")).unwrap(),
        parent_bytes
    );
    assert_eq!(
        std::fs::read(directory.path().join("middle.vmf")).unwrap(),
        middle_bytes
    );
    assert_eq!(
        std::fs::read(directory.path().join("leaf.vmf")).unwrap(),
        child_bytes
    );
}
