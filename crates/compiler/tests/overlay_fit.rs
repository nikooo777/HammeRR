mod common;

use glam::{DQuat, DVec3};
use map_core::{Document, ObjectId, Plane, edit, overlay, surface::FaceId};
use source_formats::vmf::Vmf;

#[test]
fn core_fitted_world_and_detail_quads_use_the_native_overlay_pipeline() {
    for detail in [false, true] {
        let mut document = Document::new(common::room());
        let face = document
            .edit("tilted overlay target", |tx| {
                let ObjectId::Solid(id) = edit::create_block(
                    tx,
                    DVec3::new(-48.0, 32.0, 32.0),
                    DVec3::new(48.0, 128.0, 48.0),
                    "dev/dev_measuregeneric01",
                )?
                else {
                    unreachable!()
                };
                let turn = DQuat::from_rotation_x(0.2);
                let solid = tx.solid_mut(id)?;
                for side in &mut solid.sides {
                    for point in &mut side.plane.0 {
                        point.x *= 1.0 + point.y / 512.0;
                        *point = turn * *point;
                    }
                }
                let side = solid
                    .sides
                    .iter()
                    .find(|side| {
                        Plane::from_points(side.plane.0)
                            .unwrap()
                            .normal
                            .dot(turn * DVec3::Z)
                            > 0.99
                    })
                    .unwrap()
                    .id;
                if detail {
                    edit::tie_to_entity(tx, &[id], "func_detail", &[])?;
                }
                Ok(FaceId { solid: id, side })
            })
            .unwrap();
        let options = overlay::Options {
            material: "Materials/Decals/decalgraffiti001a.VMT".into(),
            targetname: Some("  fitted marking  ".into()),
            render_order: 2,
            u: [1.0, 0.0],
            fade_min: 16.0,
            fade_max: 64.0,
            ..Default::default()
        };
        let definition = overlay::fit(document.vmf(), face, options.clone()).unwrap();
        document
            .edit("Fit overlay to face", |tx| {
                overlay::create(tx, face, options)
            })
            .unwrap();
        let source = document.into_vmf();
        let before = source.clone();
        let output = compiler::compile(&source, &common::empty_game());
        assert!(!output.report.has_errors(), "{:?}", output.report);
        let bsp = output.bsp.unwrap();
        let data = bsp.overlay_data().unwrap().unwrap();
        assert_eq!(data.overlays.len(), 1);
        let record = data.overlays[0];
        assert!(record.face_count() > 0);
        assert_eq!(record.render_order(), 2);
        assert_eq!(record.u, definition.options.u);
        assert_eq!(record.v, definition.options.v);
        assert_eq!(
            record.origin,
            definition.basis_origin.map(|value| value as f32)
        );
        assert_eq!(
            record.basis_normal,
            definition.basis_normal.map(|value| value as f32)
        );
        for (index, point) in record.uv_points.iter().enumerate() {
            assert_eq!(
                [point[0], point[1]],
                [
                    definition.uv_points[index][0] as f32,
                    definition.uv_points[index][1] as f32
                ]
            );
        }
        assert_eq!(data.fades[0].min_distance_squared, 256.0);
        assert_eq!(data.fades[0].max_distance_squared, 4096.0);
        assert!(
            bsp.entities()
                .unwrap()
                .contains("\"targetname\" \"  fitted marking  \"")
        );
        assert_eq!(source, before);
        let reopened = Vmf::parse(&source.write(&Default::default())).unwrap();
        let repeated =
            compiler::compile_controlled(&reopened, &common::empty_game(), &Default::default());
        assert!(!repeated.report.has_errors(), "{:?}", repeated.report);
        assert_eq!(repeated.bsp.unwrap().to_bytes(), bsp.to_bytes());
    }
}
