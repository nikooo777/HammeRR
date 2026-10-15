mod common;

use glam::{DVec2, DVec3};
use map_core::{
    Document, ObjectId, Plane, edit,
    surface::{self, FaceId, SurfaceEdit},
    texel_uv,
};
use source_formats::{
    bsp::{Face, TexInfo},
    vmf::Vmf,
};

fn projected(rows: [[f32; 4]; 2], point: DVec3) -> DVec2 {
    DVec2::from_array(rows.map(|row| {
        point.dot(DVec3::new(
            f64::from(row[0]),
            f64::from(row[1]),
            f64::from(row[2]),
        )) + f64::from(row[3])
    }))
}

#[test]
fn authored_skew_vectors_preserve_known_texels_through_vmf_and_world_or_model_bsp() {
    for owned in [false, true] {
        let mut document = Document::new(common::room());
        let ObjectId::Solid(id) = document
            .edit("block", |tx| {
                edit::create_block(
                    tx,
                    DVec3::new(0.0, 0.0, 16.0),
                    DVec3::new(128.0, 64.0, 80.0),
                    "uv/authored_skew",
                )
            })
            .unwrap()
        else {
            unreachable!()
        };
        let face = FaceId {
            solid: id,
            side: document
                .solid(id)
                .unwrap()
                .sides
                .iter()
                .find(|side| Plane::from_points(side.plane.0).unwrap().normal == DVec3::Z)
                .unwrap()
                .id,
        };
        let pivot = if owned {
            DVec3::new(80.125, 48.25, 32.5)
        } else {
            DVec3::ZERO
        };
        if owned {
            document
                .edit("model", |tx| {
                    edit::tie_to_entity(
                        tx,
                        &[id],
                        "func_brush",
                        &[("origin".into(), "80.125 48.25 32.5".into())],
                    )
                })
                .unwrap();
        }
        document
            .edit("author vectors", |tx| {
                surface::edit_faces(
                    tx,
                    &[face],
                    &SurfaceEdit {
                        u_axis: Some([Some(2.0), Some(0.5), Some(0.125)]),
                        v_axis: Some([Some(0.25), Some(-1.5), Some(0.375)]),
                        scale_u: Some(-0.5),
                        scale_v: Some(0.25),
                        shift_u: Some(13.125),
                        shift_v: Some(-27.5),
                        ..Default::default()
                    },
                )
            })
            .unwrap();
        let authored = surface::side(&document, face).unwrap().clone();
        let known = DVec3::new(16.0, 24.0, 80.0);
        assert_eq!(texel_uv(&authored, known), DVec2::new(-94.875, -35.5));
        let reopened = Vmf::parse(&document.vmf().write(&Default::default())).unwrap();
        let document = Document::new(reopened);
        let source = surface::side(&document, face).unwrap();
        assert_eq!(source, &authored);
        let expected = [source.u_axis, source.v_axis].map(|axis| {
            let vector = axis.axis / axis.scale;
            [
                vector.x as f32,
                vector.y as f32,
                vector.z as f32,
                (axis.offset + pivot.dot(vector)) as f32,
            ]
        });
        let output = compiler::compile(document.vmf(), &common::empty_game());
        assert!(!output.report.has_errors(), "{:?}", output.report);
        assert!(output.leak.is_none());
        let bsp = output.bsp.unwrap();
        let infos = bsp.records::<TexInfo>().unwrap();
        let faces = bsp.records::<Face>().unwrap();
        let matches = infos
            .iter()
            .enumerate()
            .filter(|(_, info)| info.texture_vecs == expected)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        assert!(!matches.is_empty());
        assert!(
            faces
                .iter()
                .any(|face| matches.contains(&(face.texinfo as usize)))
        );
        for index in matches {
            for point in surface::face_vertices(&document, face)
                .unwrap()
                .into_iter()
                .chain([known])
            {
                assert!(
                    projected(infos[index].texture_vecs, point - pivot)
                        .abs_diff_eq(texel_uv(source, point), 1e-5)
                );
            }
        }
    }
}
