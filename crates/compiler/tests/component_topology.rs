mod common;

use glam::DVec3;
use map_core::{Document, ObjectId, edit, subobject};
use source_formats::bsp::{Brush, Face, Plane};

#[test]
fn welded_pyramid_compiles_inside_a_sealed_room_after_vmf_round_trip() {
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
    let plan = subobject::plan_weld(&document, &selected).unwrap();
    document
        .edit("weld", |tx| subobject::apply(tx, &plan.replacements))
        .unwrap();
    let vmf = source_formats::vmf::Vmf::parse(&document.vmf().write(&Default::default())).unwrap();
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
