mod common;

use glam::DVec3;
use map_core::Document;
use map_core::measurements::{Measurement, Measurements};
use source_formats::vmf::Vmf;

#[test]
fn measurement_metadata_has_no_effect_on_bsp_bytes_or_entity_lump() {
    let mut document = Document::new(common::room());
    let game = common::empty_game();
    let before = compiler::compile(document.vmf(), &game)
        .bsp
        .unwrap()
        .to_bytes();
    document
        .edit("editor ruler", |tx| {
            Measurements {
                measurements: vec![Measurement {
                    id: 1,
                    name: "Editor only ruler".into(),
                    start: DVec3::new(-1024.25, 0.0, 8.0),
                    end: DVec3::new(1024.25, 64.0, 128.0),
                    extra: Vec::new(),
                }],
                extra: Vec::new(),
            }
            .write(tx)
        })
        .unwrap();
    let vmf = Vmf::parse(&document.vmf().write(&Default::default())).unwrap();
    assert_eq!(Measurements::parse(&vmf).unwrap().measurements.len(), 1);
    assert_eq!(
        compiler::compile(&vmf, &game).bsp.unwrap().to_bytes(),
        before
    );
}
