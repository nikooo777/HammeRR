use super::*;
use crate::{edit, texel_uv};
use source_formats::{keyvalues::Node, vmf::Vmf};

fn fixture() -> (Document, Vec<FaceId>) {
    let mut document = Document::empty();
    let mut faces = Vec::new();
    for (index, owned) in [false, true].into_iter().enumerate() {
        let ObjectId::Solid(id) = document
            .edit("block", |tx| {
                edit::create_block(
                    tx,
                    DVec3::X * index as f64 * 256.0,
                    DVec3::new(128.0 + index as f64 * 256.0, 64.0, 16.0),
                    "brick/test",
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
        document
            .edit("metadata", |tx| {
                let side = tx
                    .solid_mut(id)?
                    .sides
                    .iter_mut()
                    .find(|side| side.id == face.side)
                    .unwrap();
                side.u_axis.scale = -0.5;
                side.v_axis.scale = 0.25;
                side.u_axis.offset = 13.125;
                side.v_axis.offset = -27.5;
                side.rotation = 37.0;
                side.lightmap_scale = 8;
                side.smoothing_groups = Some(0x8000_0005);
                side.extra_properties.set("custom", "keep");
                side.extra.push(Node::block(
                    "custom_data",
                    vec![Node::string("entry", "keep")],
                ));
                if owned {
                    side.v_axis.axis.x = 0.375;
                }
                Ok(())
            })
            .unwrap();
        if owned {
            document
                .edit("entity", |tx| {
                    edit::tie_to_entity(
                        tx,
                        &[id],
                        "func_brush",
                        &[("origin".into(), "300 16 8".into())],
                    )
                })
                .unwrap();
        } else {
            document
                .edit("terrain", |tx| {
                    crate::displacement::edit::create(tx, &[face], 2, 0)
                })
                .unwrap();
        }
        faces.push(face);
    }
    (document, faces)
}

#[test]
fn direct_vectors_skew_actual_texels_preserve_metadata_and_roundtrip_mixed_owners() {
    let (mut document, faces) = fixture();
    let before = document.vmf().clone();
    let history = document.undo_labels().count();
    let patch = SurfaceEdit {
        u_axis: Some([Some(2.0), Some(0.5), Some(0.125)]),
        v_axis: Some([None, Some(-1.5), None]),
        ..Default::default()
    };
    let old = faces
        .iter()
        .map(|face| side(&document, *face).unwrap().clone())
        .collect::<Vec<_>>();
    assert_eq!(
        document
            .edit("vectors", |tx| edit_faces(tx, &faces, &patch))
            .unwrap(),
        2
    );
    assert_eq!(document.undo_labels().count(), history + 1);
    for (face, original) in faces.iter().zip(&old) {
        let result = side(&document, *face).unwrap();
        let mut expected = original.clone();
        expected.u_axis.axis = DVec3::new(2.0, 0.5, 0.125);
        expected.v_axis.axis.y = -1.5;
        assert_eq!(result, &expected);
        for point in face_vertices(&document, *face).unwrap() {
            let expected = DVec2::new(
                (2.0 * point.x + 0.5 * point.y + 0.125 * point.z) / -0.5 + 13.125,
                (original.v_axis.axis.x * point.x - 1.5 * point.y
                    + original.v_axis.axis.z * point.z)
                    / 0.25
                    - 27.5,
            );
            assert!(texel_uv(result, point).abs_diff_eq(expected, 1e-9));
        }
    }
    let reopened = Document::new(Vmf::parse(&document.vmf().write(&Default::default())).unwrap());
    for face in &faces {
        assert_eq!(side(&document, *face), side(&reopened, *face));
    }
    let after = document.vmf().clone();
    document.undo().unwrap();
    assert_eq!(document.vmf(), &before);
    document.redo().unwrap();
    assert_eq!(document.vmf(), &after);
}

#[test]
fn direct_vectors_reject_zero_singular_face_collapses_nonfinite_and_conflicting_controls_atomically()
 {
    let (mut document, faces) = fixture();
    let before = document.vmf().clone();
    let revision = document.revision();
    let history = document.undo_labels().count();
    let patches = [
        SurfaceEdit {
            u_axis: Some([Some(0.0); 3]),
            ..Default::default()
        },
        SurfaceEdit {
            u_axis: Some([Some(1.0), Some(0.0), Some(0.0)]),
            v_axis: Some([Some(2.0), Some(0.0), Some(0.0)]),
            ..Default::default()
        },
        SurfaceEdit {
            u_axis: Some([Some(0.0), Some(0.0), Some(1.0)]),
            ..Default::default()
        },
        SurfaceEdit {
            u_axis: Some([Some(f64::NAN), None, None]),
            ..Default::default()
        },
        SurfaceEdit {
            v_axis: Some([Some(f64::INFINITY), None, None]),
            ..Default::default()
        },
        SurfaceEdit {
            u_axis: Some([Some(f64::MAX), Some(0.0), Some(0.0)]),
            ..Default::default()
        },
        SurfaceEdit {
            u_axis: Some([Some(1e-300), Some(0.0), Some(0.0)]),
            scale_u: Some(f64::MAX),
            ..Default::default()
        },
        SurfaceEdit {
            u_axis: Some([None; 3]),
            rotation: Some(0.0),
            ..Default::default()
        },
        SurfaceEdit {
            v_axis: Some([None; 3]),
            alignment: Some(Alignment::Face),
            ..Default::default()
        },
        SurfaceEdit {
            u_axis: Some([None; 3]),
            projection: Some(sample(&document, faces[0]).unwrap().projection),
            ..Default::default()
        },
    ];
    for patch in patches {
        assert!(
            document
                .edit("invalid vectors", |tx| edit_faces(tx, &faces, &patch))
                .is_err(),
            "{patch:?}"
        );
        assert_eq!(document.vmf(), &before);
        assert_eq!(document.revision(), revision);
        assert_eq!(document.undo_labels().count(), history);
    }
    let wall = FaceId {
        solid: faces[0].solid,
        side: solid(&document, faces[0].solid)
            .unwrap()
            .sides
            .iter()
            .find(|side| Plane::from_points(side.plane.0).unwrap().normal == DVec3::X)
            .unwrap()
            .id,
    };
    let patch = SurfaceEdit {
        u_axis: Some([Some(2.0), Some(0.5), Some(0.0)]),
        v_axis: Some([Some(0.25), Some(-1.0), Some(0.0)]),
        ..Default::default()
    };
    assert!(
        document
            .edit("one face collapses", |tx| edit_faces(
                tx,
                &[faces[0], wall],
                &patch
            ))
            .is_err()
    );
    assert_eq!(document.vmf(), &before);
}

#[test]
fn direct_vector_magnitudes_are_neither_normalized_nor_replaced_by_an_absolute_area_floor() {
    let (mut document, faces) = fixture();
    let patch = SurfaceEdit {
        u_axis: Some([Some(1e-10), Some(2e-11), Some(0.0)]),
        v_axis: Some([Some(3e-11), Some(-1e-10), Some(0.0)]),
        ..Default::default()
    };
    document
        .edit("tiny vectors", |tx| edit_faces(tx, &faces, &patch))
        .unwrap();
    for face in faces {
        assert_eq!(
            side(&document, face).unwrap().u_axis.axis,
            DVec3::new(1e-10, 2e-11, 0.0)
        );
        assert_eq!(
            side(&document, face).unwrap().v_axis.axis,
            DVec3::new(3e-11, -1e-10, 0.0)
        );
    }
}

#[test]
fn direct_vector_edits_keep_scale_shift_rotation_and_fitting_semantics() {
    let (mut document, faces) = fixture();
    let face = faces[0];
    document
        .edit("vectors", |tx| {
            edit_faces(
                tx,
                &[face],
                &SurfaceEdit {
                    u_axis: Some([Some(2.0), Some(0.5), Some(0.125)]),
                    v_axis: Some([Some(0.25), Some(-1.5), Some(0.375)]),
                    shift_u: Some(21.5),
                    scale_v: Some(-0.75),
                    ..Default::default()
                },
            )
        })
        .unwrap();
    let original = side(&document, face).unwrap().clone();
    document
        .edit("rotate", |tx| {
            edit_faces(
                tx,
                &[face],
                &SurfaceEdit {
                    rotation: Some(90.0),
                    ..Default::default()
                },
            )
        })
        .unwrap();
    let result = side(&document, face).unwrap();
    let turn = DQuat::from_axis_angle(DVec3::Z, 53.0_f64.to_radians());
    assert!(
        result
            .u_axis
            .axis
            .abs_diff_eq(turn * original.u_axis.axis, 1e-12)
    );
    assert!(
        result
            .v_axis
            .axis
            .abs_diff_eq(turn * original.v_axis.axis, 1e-12)
    );
    assert_eq!(result.u_axis.offset, 21.5);
    assert_eq!(result.u_axis.scale, -0.5);
    assert_eq!(result.v_axis.scale, -0.75);
    document.undo().unwrap();
    document
        .edit("fit vectors", |tx| {
            edit_faces(
                tx,
                &[face],
                &SurfaceEdit {
                    u_axis: Some([None; 3]),
                    fit: Some([128, 64]),
                    ..Default::default()
                },
            )
        })
        .unwrap();
    let result = side(&document, face).unwrap();
    assert_eq!(result.u_axis.axis, original.u_axis.axis);
    assert_eq!(result.v_axis.axis, original.v_axis.axis);
    let coordinates = face_vertices(&document, face)
        .unwrap()
        .into_iter()
        .map(|point| texel_uv(result, point))
        .collect::<Vec<_>>();
    let (mins, maxs) = coordinates
        .into_iter()
        .fold((DVec2::MAX, DVec2::MIN), |(mins, maxs), uv| {
            (mins.min(uv), maxs.max(uv))
        });
    assert!(mins.abs_diff_eq(DVec2::ZERO, 1e-10));
    assert!(maxs.abs_diff_eq(DVec2::new(128.0, 64.0), 1e-10));
}
