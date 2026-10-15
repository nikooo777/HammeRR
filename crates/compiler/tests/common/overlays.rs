use glam::DVec3;
use map_core::{Document, edit};
use source_formats::vmf::Vmf;

pub fn add_overlay(vmf: Vmf, sides: &[u32]) -> Vmf {
    let mut document = Document::new(vmf);
    document
        .edit("overlay", |tx| {
            edit::create_entity(
                tx,
                "info_overlay",
                Some(DVec3::new(1000.0, 0.0, 32.0)),
                &[
                    ("targetname".into(), "floor_marking".into()),
                    (
                        "material".into(),
                        "materials/decals/decalgraffiti001a.vmt".into(),
                    ),
                    (
                        "sides".into(),
                        sides
                            .iter()
                            .map(u32::to_string)
                            .collect::<Vec<_>>()
                            .join(" "),
                    ),
                    ("BasisOrigin".into(), "0 0 32".into()),
                    ("BasisNormal".into(), "0 0 1".into()),
                    ("BasisU".into(), "1 0 0".into()),
                    ("BasisV".into(), "0 1 0".into()),
                    ("uv0".into(), "-120 -60 17".into()),
                    ("uv1".into(), "-120 60 9".into()),
                    ("uv2".into(), "120 60 7".into()),
                    ("uv3".into(), "120 -60 13".into()),
                ],
                Vec::new(),
            )
        })
        .unwrap();
    document.vmf().clone()
}
