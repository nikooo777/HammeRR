mod common;

use glam::DVec3;
use map_core::{Document, ObjectId, edit, subobject};
use source_formats::bsp::{Brush, Face, Plane};

#[test]
fn welded_pyramid_compiles_inside_a_sealed_room_after_vmf_round_trip() {
    for target in [
        subobject::WeldTarget::Mean,
        subobject::WeldTarget::Position(DVec3::new(16.125, 24.25, 96.5)),
    ] {
        let mut document = Document::new(common::room());
        let ObjectId::Solid(id) = document
            .edit("block", |tx| {
                edit::create_block(
                    tx,
                    DVec3::ZERO,
                    DVec3::splat(64.0),
                    "dev/dev_measuregeneric01",
                )
            })
            .unwrap()
        else {
            unreachable!()
        };
        let shape = subobject::topology(document.solid(id).unwrap()).unwrap();
        let selected = shape
            .vertices
            .iter()
            .filter(|vertex| vertex.position.z == 64.0)
            .map(|vertex| subobject::Component {
                solid: id,
                sides: vertex.sides.clone(),
            })
            .collect::<Vec<_>>();
        let plan = subobject::plan_weld_target(&document, &selected, &target).unwrap();
        document
            .edit("weld", |tx| subobject::apply(tx, &plan.replacements))
            .unwrap();
        let vmf =
            source_formats::vmf::Vmf::parse(&document.vmf().write(&Default::default())).unwrap();
        let output = compiler::compile(&vmf, &common::empty_game());
        assert!(!output.report.has_errors(), "{:?}", output.report);
        assert!(output.leak.is_none());
        assert_eq!(output.stats.brushes, 7);
        let bsp = output.bsp.unwrap();
        assert!(
            bsp.records::<Brush>()
                .unwrap()
                .iter()
                .any(|brush| brush.side_count >= 5)
        );
        assert!(!bsp.records::<Face>().unwrap().is_empty());
        assert!(bsp.records::<Plane>().unwrap().iter().any(|plane| {
            plane
                .normal
                .iter()
                .filter(|value| value.abs() > 0.01)
                .count()
                == 2
        }));
    }
}

#[test]
fn split_face_and_edge_compile_inside_a_sealed_room_after_vmf_round_trip() {
    for edge in [false, true] {
        let mut document = Document::new(common::room());
        let ObjectId::Solid(id) = document
            .edit("block", |tx| {
                edit::create_block(
                    tx,
                    DVec3::ZERO,
                    DVec3::splat(64.0),
                    "dev/dev_measuregeneric01",
                )
            })
            .unwrap()
        else {
            unreachable!()
        };
        let shape = subobject::topology(document.solid(id).unwrap()).unwrap();
        let sides = if edge {
            shape.edges[0].sides.clone()
        } else {
            vec![shape.faces[0].side]
        };
        document
            .edit("split", |tx| {
                subobject::split(tx, &[subobject::Component { solid: id, sides }], 16.0)
            })
            .unwrap();
        let vmf =
            source_formats::vmf::Vmf::parse(&document.vmf().write(&Default::default())).unwrap();
        vmf.validate_ids().unwrap();
        let output = compiler::compile(&vmf, &common::empty_game());
        assert!(!output.report.has_errors(), "{:?}", output.report);
        assert!(output.leak.is_none());
        assert_eq!(output.stats.brushes, 7);
        let bsp = output.bsp.unwrap();
        assert!(!bsp.records::<Face>().unwrap().is_empty());
        assert!(bsp.records::<Plane>().unwrap().iter().any(|plane| {
            plane
                .normal
                .iter()
                .filter(|value| value.abs() > 0.01)
                .count()
                >= 2
        }));
    }
}

#[test]
fn carved_room_compiles_after_removing_the_retained_cutter_and_vmf_round_trip() {
    let mut document = Document::empty();
    let (outer, cutter) = document
        .edit("room volumes", |tx| {
            let outer = edit::create_block(
                tx,
                DVec3::new(-144.0, -144.0, -16.0),
                DVec3::splat(144.0),
                "dev/dev_measuregeneric01",
            )?;
            let cutter = edit::create_block(
                tx,
                DVec3::new(-128.0, -128.0, 0.0),
                DVec3::splat(128.0),
                "dev/dev_measuregeneric01",
            )?;
            Ok((outer, cutter))
        })
        .unwrap();
    let (ObjectId::Solid(outer), ObjectId::Solid(cutter)) = (outer, cutter) else {
        unreachable!()
    };
    let result = document
        .edit("carve room", |tx| {
            map_core::carve::carve(tx, cutter, &[outer])
        })
        .unwrap();
    assert_eq!(result.changes[0].fragments.len(), 6);
    document
        .edit("remove cutter and add spawn", |tx| {
            edit::delete(tx, &[ObjectId::Solid(cutter)])?;
            edit::create_entity(
                tx,
                "info_player_counterterrorist",
                Some(DVec3::new(0.0, 0.0, 64.0)),
                &[],
                Vec::new(),
            )
        })
        .unwrap();
    let vmf = source_formats::vmf::Vmf::parse(&document.vmf().write(&Default::default())).unwrap();
    vmf.validate_ids().unwrap();
    let output = compiler::compile(&vmf, &common::empty_game());
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.leak.is_none());
    assert_eq!(output.stats.brushes, 6);
    assert!(!output.bsp.unwrap().records::<Face>().unwrap().is_empty());
}

#[test]
fn scaled_and_rotated_components_compile_after_vmf_round_trip() {
    for (spec, whole) in [
        (
            subobject::ComponentTransform::Scale(DVec3::new(0.5, 0.5, 1.0)),
            false,
        ),
        (
            subobject::ComponentTransform::Rotate(DVec3::new(30.0, 0.0, 0.0)),
            false,
        ),
        (
            subobject::ComponentTransform::Scale(DVec3::new(2.0, 0.5, 1.25)),
            true,
        ),
        (
            subobject::ComponentTransform::Rotate(DVec3::new(15.0, -20.0, 30.0)),
            true,
        ),
    ] {
        let mut document = Document::new(common::room());
        let ObjectId::Solid(id) = document
            .edit("block", |tx| {
                edit::create_block(
                    tx,
                    DVec3::ZERO,
                    DVec3::splat(64.0),
                    "dev/dev_measuregeneric01",
                )
            })
            .unwrap()
        else {
            unreachable!()
        };
        let shape = subobject::topology(document.solid(id).unwrap()).unwrap();
        let selected = shape
            .vertices
            .iter()
            .filter(|vertex| whole || vertex.position.z == 64.0)
            .map(|vertex| subobject::Component {
                solid: id,
                sides: vertex.sides.clone(),
            })
            .collect::<Vec<_>>();
        let plan = subobject::plan_transform(&document, &selected, &spec, None, whole).unwrap();
        document
            .edit("transform", |tx| subobject::apply(tx, &plan.replacements))
            .unwrap();
        let vmf =
            source_formats::vmf::Vmf::parse(&document.vmf().write(&Default::default())).unwrap();
        vmf.validate_ids().unwrap();
        let output = compiler::compile(&vmf, &common::empty_game());
        assert!(!output.report.has_errors(), "{:?}", output.report);
        assert!(output.leak.is_none());
        assert_eq!(output.stats.brushes, 7);
        assert!(!output.bsp.unwrap().records::<Face>().unwrap().is_empty());
    }
}

#[test]
fn multi_cutter_room_compiles_after_retaining_then_removing_both_cutters() {
    let mut document = Document::empty();
    let (outer, left, right) = document
        .edit("room inputs", |tx| {
            let outer = edit::create_block(
                tx,
                DVec3::new(-144.0, -144.0, -16.0),
                DVec3::splat(144.0),
                "dev/dev_measuregeneric01",
            )?;
            let left = edit::create_block(
                tx,
                DVec3::new(-128.0, -128.0, 0.0),
                DVec3::new(0.0, 128.0, 128.0),
                "dev/dev_measuregeneric01",
            )?;
            let right = edit::create_block(
                tx,
                DVec3::new(0.0, -128.0, 0.0),
                DVec3::splat(128.0),
                "dev/dev_measuregeneric01",
            )?;
            Ok((outer, left, right))
        })
        .unwrap();
    let (ObjectId::Solid(outer), ObjectId::Solid(left), ObjectId::Solid(right)) =
        (outer, left, right)
    else {
        unreachable!()
    };
    document
        .edit("carve room", |tx| {
            map_core::carve::carve_many(tx, &[left, right], &[outer])
        })
        .unwrap();
    document
        .edit("remove cutters and add spawn", |tx| {
            edit::delete(tx, &[ObjectId::Solid(left), ObjectId::Solid(right)])?;
            edit::create_entity(
                tx,
                "info_player_counterterrorist",
                Some(DVec3::new(0.0, 0.0, 64.0)),
                &[],
                Vec::new(),
            )
        })
        .unwrap();
    let vmf = source_formats::vmf::Vmf::parse(&document.vmf().write(&Default::default())).unwrap();
    vmf.validate_ids().unwrap();
    let output = compiler::compile(&vmf, &common::empty_game());
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.leak.is_none());
    assert!(output.stats.brushes >= 6);
}
