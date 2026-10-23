use super::*;
use crate::session::SelectMode;
use glam::{DVec2, DVec3};
use map_core::ObjectId;
use map_core::surface::{self, SurfaceEdit};

fn fixture(actor: Actor) -> (tempfile::TempDir, Session, FaceId) {
    let (directory, game) = crate::test_assets::game();
    let mut session = Session::new(Ok(game));
    let ObjectId::Solid(solid) = session
        .create_block(
            actor,
            DVec3::ZERO,
            DVec3::new(128.0, 64.0, 16.0),
            "test/checker",
            false,
        )
        .unwrap()
    else {
        unreachable!()
    };
    let face = surface::sample(
        session.document(),
        FaceId {
            solid,
            side: session.document().solid(solid).unwrap().sides[0].id,
        },
    )
    .unwrap()
    .face;
    session.select_faces(&[face], SelectMode::Replace).unwrap();
    session
        .edit_faces(
            actor,
            &[face],
            SurfaceEdit {
                rotation: Some(37.0),
                scale_u: Some(-0.5),
                lightmap_scale: Some(8),
                smoothing_groups: Some(1 << 3),
                ..SurfaceEdit::default()
            },
            false,
            false,
        )
        .unwrap();
    (directory, session, face)
}

fn bounds(session: &Session, face: FaceId, preview: bool) -> (DVec2, DVec2) {
    let sample = surface::sample(session.document(), face).unwrap();
    let side = if preview {
        session.preview_side(face).unwrap()
    } else {
        surface::side(session.document(), face).unwrap()
    };
    sample
        .vertices
        .iter()
        .map(|point| map_core::texel_uv(side, *point))
        .fold(
            (DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY)),
            |(lo, hi), uv| (lo.min(uv), hi.max(uv)),
        )
}

fn region() -> Rectangle {
    Rectangle {
        x: 16,
        y: 8,
        width: 32,
        height: 16,
    }
}

fn grouped_fixture() -> (tempfile::TempDir, Session, Vec<FaceId>) {
    let (directory, game) = crate::test_assets::game();
    let mut session = Session::new(Ok(game));
    let mut faces = Vec::new();
    for x in [0.0, 64.0, 128.0] {
        let ObjectId::Solid(solid) = session
            .create_block(
                Actor::Agent,
                DVec3::new(x, 0.0, 0.0),
                DVec3::new(x + 64.0, 32.0, 16.0),
                "test/checker",
                false,
            )
            .unwrap()
        else {
            unreachable!()
        };
        let side = session
            .document()
            .solid(solid)
            .unwrap()
            .sides
            .iter()
            .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal == DVec3::Z)
            .unwrap()
            .id;
        faces.push(FaceId { solid, side });
    }
    session
        .edit(Actor::Agent, "detail target", |tx| {
            map_core::edit::tie_to_entity(tx, &[faces[1].solid], "func_detail", &[])
        })
        .unwrap();
    for (index, face) in faces[..2].iter().enumerate() {
        session
            .edit_faces(
                Actor::Agent,
                &[*face],
                SurfaceEdit {
                    scale_u: Some(if index == 0 { -0.5 } else { 0.25 }),
                    lightmap_scale: Some(if index == 0 { 8 } else { 32 }),
                    smoothing_groups: Some(1 << index),
                    ..SurfaceEdit::default()
                },
                false,
                false,
            )
            .unwrap();
    }
    session
        .create_terrain(Actor::Agent, &faces[2..], 2, 0)
        .unwrap();
    session
        .select_faces(&faces[..2], SelectMode::Replace)
        .unwrap();
    (directory, session, faces)
}

#[test]
fn grouped_atlas_owns_validated_reference_and_preserves_targets_through_rejection_cancel_and_undo()
{
    let (directory, mut session, faces) = grouped_fixture();
    let selected = &faces[..2];
    let original = session.document().vmf().clone();
    let history = session.history_snapshot();
    let undo_count = session.document().undo_labels().count();
    let snapshot = session
        .inspect_material_atlas("test/checker")
        .unwrap()
        .report["snapshot"]
        .as_str()
        .unwrap()
        .to_owned();
    let options = FitOptions {
        region: FitRegion {
            rectangle: region(),
            padding: 2,
        },
        mode: FitMode::Together {
            reference: faces[0],
        },
    };
    let state = session
        .preview_atlas_fit_options(
            Actor::Agent,
            selected,
            "test/checker",
            options,
            &snapshot,
            None,
        )
        .unwrap();
    assert_eq!(state["preview"]["mode"], "together");
    assert_eq!(
        state["preview"]["reference"],
        json!({"solid":faces[0].solid,"side":faces[0].side})
    );
    let reference = session.preview_side(faces[0]).unwrap();
    let other = session.preview_side(faces[1]).unwrap();
    assert_eq!(reference.u_axis, other.u_axis);
    assert_eq!(reference.v_axis, other.v_axis);
    assert!(other.u_axis.scale.is_sign_negative());
    for face in selected {
        let preview = session.preview_side(*face).unwrap();
        let mut expected = surface::side(session.document(), *face).unwrap().clone();
        expected.u_axis = preview.u_axis;
        expected.v_axis = preview.v_axis;
        expected.rotation = preview.rotation;
        expected.material = preview.material.clone();
        assert_eq!(preview, &expected);
    }
    for (point, expected) in [
        (DVec3::new(0.0, 0.0, 16.0), DVec2::new(46.0, 22.0)),
        (DVec3::new(128.0, 32.0, 16.0), DVec2::new(18.0, 10.0)),
        (DVec3::new(64.0, 0.0, 16.0), DVec2::new(32.0, 22.0)),
    ] {
        assert!(map_core::texel_uv(reference, point).abs_diff_eq(expected, 1e-8));
        assert!(map_core::texel_uv(other, point).abs_diff_eq(expected, 1e-8));
    }
    let token = session.face_preview_token();
    let preview = session.render_document().vmf().clone();
    let mut invalid_targets = vec![vec![faces[0], faces[2]]];
    for normal in [DVec3::Y, -DVec3::Z] {
        let side = session
            .document()
            .solid(faces[0].solid)
            .unwrap()
            .sides
            .iter()
            .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal == normal)
            .unwrap()
            .id;
        invalid_targets.push(vec![
            faces[0],
            FaceId {
                solid: faces[0].solid,
                side,
            },
        ]);
    }
    for targets in invalid_targets {
        assert!(
            session
                .preview_atlas_fit_options(
                    Actor::Agent,
                    &targets,
                    "test/checker",
                    options,
                    &snapshot,
                    token,
                )
                .is_err()
        );
        assert_eq!(session.atlas_fit_state(), state);
        assert_eq!(session.render_document().vmf(), &preview);
    }
    assert!(
        session
            .preview_atlas_fit_options(
                Actor::Agent,
                selected,
                "test/checker",
                FitOptions {
                    mode: FitMode::Together {
                        reference: faces[2]
                    },
                    ..options
                },
                &snapshot,
                token,
            )
            .is_err()
    );
    assert!(session.commit_face_preview(Actor::User).is_err());
    assert!(
        session
            .cancel_owned_face_preview(Actor::User, token)
            .is_err()
    );
    let material_path = directory.path().join("materials/test/checker.vmt");
    let material_source = std::fs::read_to_string(&material_path).unwrap();
    std::fs::write(
        &material_path,
        "LightmappedGeneric { $basetexture test/checker $color \"[1 0 0]\" }",
    )
    .unwrap();
    assert!(session.commit_face_preview(Actor::Agent).is_err());
    assert_eq!(session.atlas_fit_state(), state);
    assert_eq!(session.render_document().vmf(), &preview);
    session
        .cancel_owned_face_preview(Actor::Agent, token)
        .unwrap();
    assert_eq!(session.render_document().vmf(), &original);
    assert_eq!(session.history_snapshot(), history);
    std::fs::write(material_path, material_source).unwrap();
    session
        .preview_atlas_fit_options(
            Actor::Agent,
            selected,
            "test/checker",
            options,
            &snapshot,
            None,
        )
        .unwrap();
    session.commit_face_preview(Actor::Agent).unwrap();
    assert_eq!(session.document().vmf(), &preview);
    assert_eq!(session.document().undo_labels().count(), undo_count + 1);
    session.undo(Actor::Agent).unwrap();
    assert_eq!(session.document().vmf(), &original);
    session.redo(Actor::Agent).unwrap();
    assert_eq!(session.document().vmf(), &preview);
}

#[test]
fn atlas_fit_shared_preview_preserves_metadata_and_applies_exactly_one_undo_step() {
    for actor in [Actor::User, Actor::Agent] {
        let (_directory, mut session, face) = fixture(actor);
        let original = session.document().vmf().clone();
        let camera = session.camera();
        let history = session.history_snapshot();
        let undo_count = session.document().undo_labels().count();
        let side = surface::side(session.document(), face).unwrap().clone();
        let atlas = session.inspect_material_atlas("test/checker").unwrap();
        assert_eq!([atlas.image.width, atlas.image.height], [64, 32]);
        let snapshot = atlas.report["snapshot"].as_str().unwrap();
        session
            .preview_atlas_fit(actor, &[face], "test/checker", region(), snapshot, None)
            .unwrap();
        assert_eq!(session.document().vmf(), &original);
        assert_eq!(session.history_snapshot(), history);
        let preview = session.preview_side(face).unwrap();
        assert_eq!(preview.u_axis.axis, side.u_axis.axis);
        assert_eq!(preview.v_axis.axis, side.v_axis.axis);
        assert_eq!(preview.rotation, side.rotation);
        assert_eq!(preview.lightmap_scale, side.lightmap_scale);
        assert_eq!(preview.smoothing_groups, side.smoothing_groups);
        assert_eq!(preview.plane, side.plane);
        assert_eq!(preview.u_axis.scale.signum(), side.u_axis.scale.signum());
        let (lo, hi) = bounds(&session, face, true);
        assert!(lo.abs_diff_eq(DVec2::new(16.0, 8.0), 1e-8));
        assert!(hi.abs_diff_eq(DVec2::new(48.0, 24.0), 1e-8));
        let preview = session.face_preview_document().unwrap().vmf().clone();
        let token = session.face_preview_token();
        assert!(session.check_face_preview_token(None).is_err());
        session.check_face_preview_token(token).unwrap();
        session.commit_face_preview(actor).unwrap();
        assert_eq!(session.document().vmf(), &preview);
        assert_eq!(session.document().undo_labels().count(), undo_count + 1);
        session.undo(actor).unwrap();
        assert_eq!(session.document().vmf(), &original);
        session.redo(actor).unwrap();
        assert_eq!(session.document().vmf(), &preview);
        assert_eq!(session.camera(), camera);
        assert_eq!(session.selected_faces(), vec![face]);
    }
}

#[test]
fn atlas_padding_preserves_zero_state_and_owned_draft_through_rejection_cancel_and_undo() {
    let (directory, mut session, face) = fixture(Actor::Agent);
    let original = session.document().vmf().clone();
    let history = session.history_snapshot();
    let undo_count = session.document().undo_labels().count();
    let snapshot = session
        .inspect_material_atlas("test/checker")
        .unwrap()
        .report["snapshot"]
        .as_str()
        .unwrap()
        .to_owned();
    let zero_state = session
        .preview_atlas_fit(
            Actor::Agent,
            &[face],
            "test/checker",
            region(),
            &snapshot,
            None,
        )
        .unwrap();
    let zero_preview = session.render_document().vmf().clone();
    let mut explicit_zero_state = session
        .preview_atlas_fit_region(
            Actor::Agent,
            &[face],
            "test/checker",
            FitRegion {
                rectangle: region(),
                padding: 0,
            },
            &snapshot,
            session.face_preview_token(),
        )
        .unwrap();
    explicit_zero_state["preview"]["token"] = zero_state["preview"]["token"].clone();
    assert_eq!(explicit_zero_state, zero_state);
    assert_eq!(session.render_document().vmf(), &zero_preview);
    let padded = FitRegion {
        rectangle: region(),
        padding: 2,
    };
    let state = session
        .preview_atlas_fit_region(
            Actor::Agent,
            &[face],
            "test/checker",
            padded,
            &snapshot,
            session.face_preview_token(),
        )
        .unwrap();
    assert_eq!(state["preview"]["padding"], 2);
    assert_eq!(state["preview"]["rectangle"], rectangle_json(region()));
    assert_eq!(
        state["preview"]["usable_rectangle"],
        json!({"x":18,"y":10,"width":28,"height":12})
    );
    assert_eq!(
        state["preview"]["uv_bounds"],
        json!([[0.28125, 0.3125], [0.71875, 0.6875]])
    );
    let (low, high) = bounds(&session, face, true);
    assert!(low.abs_diff_eq(DVec2::new(18.0, 10.0), 1e-8));
    assert!(high.abs_diff_eq(DVec2::new(46.0, 22.0), 1e-8));
    let preview = session.render_document().vmf().clone();
    let token = session.face_preview_token();
    for invalid in [
        FitRegion {
            padding: 8,
            ..padded
        },
        FitRegion {
            padding: u32::MAX,
            ..padded
        },
        FitRegion {
            rectangle: Rectangle {
                x: 40,
                y: 0,
                width: 32,
                height: 32,
            },
            padding: 8,
        },
    ] {
        assert!(
            session
                .preview_atlas_fit_region(
                    Actor::Agent,
                    &[face],
                    "test/checker",
                    invalid,
                    &snapshot,
                    token,
                )
                .is_err()
        );
        assert_eq!(session.atlas_fit_state(), state);
        assert_eq!(session.face_preview_token(), token);
        assert_eq!(session.render_document().vmf(), &preview);
    }
    let material_path = directory.path().join("materials/test/checker.vmt");
    let material_source = std::fs::read_to_string(&material_path).unwrap();
    std::fs::write(
        &material_path,
        "LightmappedGeneric { $basetexture test/checker $color \"[1 0 0]\" }",
    )
    .unwrap();
    assert!(session.commit_face_preview(Actor::Agent).is_err());
    assert_eq!(session.atlas_fit_state(), state);
    assert_eq!(session.render_document().vmf(), &preview);
    session
        .cancel_owned_face_preview(Actor::Agent, token)
        .unwrap();
    assert_eq!(session.render_document().vmf(), &original);
    assert_eq!(session.history_snapshot(), history);
    std::fs::write(material_path, material_source).unwrap();
    session
        .preview_atlas_fit_region(
            Actor::Agent,
            &[face],
            "test/checker",
            padded,
            &snapshot,
            None,
        )
        .unwrap();
    session.commit_face_preview(Actor::Agent).unwrap();
    assert_eq!(session.document().vmf(), &preview);
    assert_eq!(session.document().undo_labels().count(), undo_count + 1);
    session.undo(Actor::Agent).unwrap();
    assert_eq!(session.document().vmf(), &original);
    session.redo(Actor::Agent).unwrap();
    assert_eq!(session.document().vmf(), &preview);
}

#[test]
fn atlas_fit_failed_requests_foreign_actors_and_stale_tokens_preserve_valid_preview() {
    let (_directory, mut session, face) = fixture(Actor::User);
    let original = session.document().vmf().clone();
    let history = session.history_snapshot();
    let snapshot = session
        .inspect_material_atlas("test/checker")
        .unwrap()
        .report["snapshot"]
        .as_str()
        .unwrap()
        .to_owned();
    session
        .preview_atlas_fit(
            Actor::User,
            &[face],
            "test/checker",
            region(),
            &snapshot,
            None,
        )
        .unwrap();
    let token = session.face_preview_token();
    let preview = session.render_document().vmf().clone();
    session.cancel_face_preview();
    assert_eq!(session.face_preview_token(), token);
    assert!(
        session
            .edit_faces(
                Actor::User,
                &[face],
                SurfaceEdit {
                    shift_u: Some(123.0),
                    ..SurfaceEdit::default()
                },
                false,
                false
            )
            .is_err()
    );
    for rectangle in [
        Rectangle {
            width: 0,
            ..region()
        },
        Rectangle { x: 63, ..region() },
        Rectangle {
            x: u32::MAX,
            ..region()
        },
    ] {
        assert!(
            session
                .preview_atlas_fit(
                    Actor::User,
                    &[face],
                    "test/checker",
                    rectangle,
                    &snapshot,
                    token
                )
                .is_err()
        );
    }
    assert!(
        session
            .preview_atlas_fit(
                Actor::Agent,
                &[face],
                "test/checker",
                region(),
                &snapshot,
                token
            )
            .is_err()
    );
    assert!(
        session
            .preview_atlas_fit(
                Actor::User,
                &[face],
                "test/checker",
                region(),
                &snapshot,
                None
            )
            .is_err()
    );
    assert!(
        session
            .preview_atlas_fit(
                Actor::User,
                &[face],
                "test/checker",
                region(),
                "stale assets",
                token
            )
            .is_err()
    );
    assert!(
        session
            .preview_atlas_fit(
                Actor::User,
                &[
                    face,
                    FaceId {
                        solid: u32::MAX,
                        side: u32::MAX
                    }
                ],
                "test/checker",
                region(),
                &snapshot,
                token
            )
            .is_err()
    );
    assert!(session.commit_face_preview(Actor::Agent).is_err());
    assert!(
        session
            .cancel_owned_face_preview(Actor::Agent, token)
            .is_err()
    );
    assert!(
        session
            .cancel_owned_face_preview(Actor::User, token.map(|token| token + 1))
            .is_err()
    );
    assert_eq!(session.face_preview_token(), token);
    assert_eq!(session.render_document().vmf(), &preview);
    assert_eq!(session.document().vmf(), &original);
    assert_eq!(session.history_snapshot(), history);
    session
        .cancel_owned_face_preview(Actor::User, token)
        .unwrap();
    assert_eq!(session.render_document().vmf(), &original);
    session
        .preview_atlas_fit(
            Actor::User,
            &[face],
            "test/checker",
            region(),
            &snapshot,
            None,
        )
        .unwrap();
    let previous_atlas = session.face_preview_token();
    session
        .edit_faces(
            Actor::Agent,
            &[face],
            SurfaceEdit {
                shift_u: Some(123.0),
                ..SurfaceEdit::default()
            },
            false,
            true,
        )
        .unwrap();
    assert!(!session.has_atlas_face_preview());
    assert!(session.has_face_preview());
    assert!(
        session
            .cancel_owned_face_preview(Actor::User, previous_atlas)
            .is_err()
    );
    assert!(session.has_face_preview());
    session.cancel_face_preview();
    assert!(!session.has_face_preview());
    assert_eq!(session.document().vmf(), &original);
    assert_eq!(session.history_snapshot(), history);
}

#[test]
fn unrelated_normal_admission_preserves_the_reviewed_atlas_identity_and_apply() {
    let (directory, mut session, face) = fixture(Actor::Agent);
    std::fs::write(
        directory.path().join("materials/test/normal.vmt"),
        "LightmappedGeneric { $basetexture test/checker $bumpmap test/checker }",
    )
    .unwrap();
    let snapshot = session
        .inspect_material_atlas("test/checker")
        .unwrap()
        .report["snapshot"]
        .as_str()
        .unwrap()
        .to_owned();
    session
        .preview_atlas_fit(
            Actor::Agent,
            &[face],
            "test/checker",
            region(),
            &snapshot,
            None,
        )
        .unwrap();
    let token = session.face_preview_token();
    let preview = session.render_document().vmf().clone();
    let before = session.materials.generation();
    let id = session.materials.load(&session.game, "test/normal");
    assert!(
        session
            .materials
            .normal_map(&session.game, id)
            .image
            .is_some()
    );
    assert_ne!(session.materials.generation(), before);
    assert_eq!(
        session
            .inspect_material_atlas("test/checker")
            .unwrap()
            .report["snapshot"],
        snapshot
    );
    assert_eq!(session.face_preview_token(), token);
    session.commit_face_preview(Actor::Agent).unwrap();
    assert_eq!(session.document().vmf(), &preview);
}

#[test]
fn atlas_fit_generic_commit_rejects_changed_cached_material_and_asset_reload() {
    let (directory, mut session, face) = fixture(Actor::Agent);
    let snapshot = session
        .inspect_material_atlas("test/checker")
        .unwrap()
        .report["snapshot"]
        .as_str()
        .unwrap()
        .to_owned();
    session
        .preview_atlas_fit(
            Actor::Agent,
            &[face],
            "test/checker",
            region(),
            &snapshot,
            None,
        )
        .unwrap();
    let token = session.face_preview_token();
    let original = session.document().vmf().clone();
    let preview = session.render_document().vmf().clone();
    std::fs::write(
        directory.path().join("materials/test/checker.vmt"),
        "LightmappedGeneric { $basetexture test/checker $color \"[1 0 0]\" }",
    )
    .unwrap();
    assert!(
        session
            .inspect_material_atlas("test/checker")
            .unwrap_err()
            .contains("Reload material assets")
    );
    assert!(session.commit_face_preview(Actor::Agent).is_err());
    assert_eq!(session.face_preview_token(), token);
    assert_eq!(session.render_document().vmf(), &preview);
    session.reload_material_assets(Actor::Agent);
    assert!(session.commit_face_preview(Actor::Agent).is_err());
    assert_eq!(session.document().vmf(), &original);
    session
        .cancel_owned_face_preview(Actor::Agent, token)
        .unwrap();
}

#[test]
fn atlas_admission_rejects_substituted_transformed_animated_and_oversized_sources_without_cache_poisoning()
 {
    let (directory, mut session, face) = fixture(Actor::Agent);
    for (name, source) in [
        (
            "display",
            "LightmappedGeneric { $basetexture test/checker %tooltexture test/other }",
        ),
        (
            "transform",
            "LightmappedGeneric { $basetexture test/checker $basetexturetransform \"scale 2 1\" }",
        ),
        (
            "proxy",
            "LightmappedGeneric { $basetexture test/checker Proxies { AnimatedTexture {} } }",
        ),
        (
            "blend",
            "WorldVertexTransition { $basetexture test/checker $basetexture2 test/checker }",
        ),
    ] {
        std::fs::write(
            directory.path().join(format!("materials/test/{name}.vmt")),
            source,
        )
        .unwrap();
        assert!(
            session
                .inspect_material_atlas(&format!("test/{name}"))
                .is_err()
        );
    }
    let mut bytes = std::fs::read(directory.path().join("materials/test/checker.vtf")).unwrap();
    bytes[24..26].copy_from_slice(&0u16.to_le_bytes());
    std::fs::write(directory.path().join("materials/test/zero.vtf"), bytes).unwrap();
    std::fs::write(
        directory.path().join("materials/test/zero.vmt"),
        "UnlitGeneric { $basetexture test/zero }",
    )
    .unwrap();
    assert!(session.inspect_material_atlas("test/zero").is_err());
    let large = std::fs::File::create(directory.path().join("materials/test/large.vtf")).unwrap();
    large.set_len(MAX_TEXTURE_BYTES as u64 + 1).unwrap();
    std::fs::write(
        directory.path().join("materials/test/large.vmt"),
        "UnlitGeneric { $basetexture test/large }",
    )
    .unwrap();
    assert!(session.inspect_material_atlas("test/large").is_err());
    assert!(session.materials.find("test/large").is_none());
    session.create_terrain(Actor::Agent, &[face], 2, 0).unwrap();
    let snapshot = session
        .inspect_material_atlas("test/checker")
        .unwrap()
        .report["snapshot"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        session
            .preview_atlas_fit(
                Actor::Agent,
                &[face],
                "test/checker",
                region(),
                &snapshot,
                None
            )
            .unwrap_err()
            .contains("terrain")
    );
    assert!(!session.has_face_preview());
}

#[test]
fn atlas_thumbnail_bounds_nomip_textures_without_changing_original_uv_dimensions() {
    let (directory, game) = crate::test_assets::game();
    let path = directory.path().join("materials/test/checker.vtf");
    let mut bytes = std::fs::read(&path).unwrap();
    bytes.truncate(80);
    bytes[16..18].copy_from_slice(&512u16.to_le_bytes());
    bytes[18..20].copy_from_slice(&256u16.to_le_bytes());
    for y in 0..256 {
        for x in 0..512 {
            bytes.extend_from_slice(&[(x % 256) as u8, y as u8, 80, 255]);
        }
    }
    std::fs::write(&path, bytes).unwrap();
    let mut session = Session::new(Ok(game));
    let atlas = session.inspect_material_atlas("test/checker").unwrap();
    assert_eq!(
        (
            atlas.report["width"].as_u64(),
            atlas.report["height"].as_u64()
        ),
        (Some(512), Some(256))
    );
    assert_eq!((atlas.image.width, atlas.image.height), (256, 128));
    assert_eq!(&atlas.image.pixels[..8], &[0, 0, 80, 255, 2, 0, 80, 255]);
    assert_eq!(
        &atlas.image.pixels[atlas.image.pixels.len() - 4..],
        &[254, 254, 80, 255]
    );
}

fn effective_atlas_assets(directory: &std::path::Path, extra: &str) {
    let mut bytes = std::fs::read(directory.join("materials/test/checker.vtf")).unwrap();
    bytes.truncate(80);
    bytes[16..18].copy_from_slice(&128u16.to_le_bytes());
    bytes[18..20].copy_from_slice(&64u16.to_le_bytes());
    for y in 0..64 {
        for x in 0..128 {
            bytes.extend_from_slice(&[x as u8, y as u8, 177, 255]);
        }
    }
    std::fs::write(directory.join("materials/test/albedo.vtf"), bytes).unwrap();
    std::fs::write(directory.join("materials/test/effective.vmt"), format!("LightmappedGeneric {{ $basetexture test/checker $bumpmap test/missing_normal $albedo test/albedo {extra} }}")).unwrap();
}

#[test]
fn atlas_names_and_fits_effective_albedo_then_rejects_stale_selection_after_reload() {
    let (directory, mut session, face) = fixture(Actor::Agent);
    effective_atlas_assets(directory.path(), "");
    let original = session.document().vmf().clone();
    let history = session.history_snapshot();
    let atlas = session.inspect_material_atlas("test/effective").unwrap();
    assert_eq!(atlas.report["texture"], "materials/test/albedo.vtf");
    assert_eq!((atlas.image.width, atlas.image.height), (128, 64));
    assert_eq!(&atlas.image.pixels[..4], &[0, 0, 177, 255]);
    assert_eq!(
        &atlas.image.pixels[atlas.image.pixels.len() - 4..],
        &[127, 63, 177, 255]
    );
    let snapshot = atlas.report["snapshot"].as_str().unwrap().to_owned();
    let rectangle = Rectangle {
        x: 80,
        y: 40,
        width: 32,
        height: 16,
    };
    session
        .preview_atlas_fit(
            Actor::Agent,
            &[face],
            "test/effective",
            rectangle,
            &snapshot,
            None,
        )
        .unwrap();
    let (low, high) = bounds(&session, face, true);
    assert!(low.abs_diff_eq(DVec2::new(80.0, 40.0), 1e-8));
    assert!(high.abs_diff_eq(DVec2::new(112.0, 56.0), 1e-8));
    assert_eq!(
        session
            .preview_side(face)
            .unwrap()
            .material
            .to_ascii_lowercase(),
        "test/effective"
    );
    assert_eq!(session.document().vmf(), &original);
    assert_eq!(session.history_snapshot(), history);
    let token = session.face_preview_token();
    let preview = session.render_document().vmf().clone();
    effective_atlas_assets(directory.path(), "$nodiffusebumplighting 1");
    assert!(
        session
            .inspect_material_atlas("test/effective")
            .unwrap_err()
            .contains("Reload material assets")
    );
    assert!(session.commit_face_preview(Actor::Agent).is_err());
    assert_eq!(session.render_document().vmf(), &preview);
    session
        .cancel_owned_face_preview(Actor::Agent, token)
        .unwrap();
    assert_eq!(session.reload_material_assets(Actor::Agent).failed, 0);
    let suppressed = session.inspect_material_atlas("test/effective").unwrap();
    assert_eq!(suppressed.report["texture"], "materials/test/checker.vtf");
    assert_eq!((suppressed.image.width, suppressed.image.height), (64, 32));
    assert_ne!(suppressed.report["snapshot"], snapshot);
    assert!(
        session
            .preview_atlas_fit(
                Actor::Agent,
                &[face],
                "test/effective",
                rectangle,
                &snapshot,
                None
            )
            .is_err()
    );
    assert!(!session.has_face_preview());
    assert_eq!(session.document().vmf(), &original);
    assert_eq!(session.history_snapshot(), history);
}

#[test]
fn atlas_effective_selection_keeps_tool_identity_and_rejects_missing_or_bad_albedo_atomically() {
    let (directory, mut session, face) = fixture(Actor::Agent);
    effective_atlas_assets(directory.path(), "%tooltexture test/checker");
    assert!(
        session
            .inspect_material_atlas("test/effective")
            .unwrap_err()
            .contains("%tooltexture differs")
    );
    effective_atlas_assets(directory.path(), "%tooltexture test/albedo");
    let accepted = session.inspect_material_atlas("test/effective").unwrap();
    assert_eq!(accepted.report["texture"], "materials/test/albedo.vtf");
    let snapshot = accepted.report["snapshot"].as_str().unwrap().to_owned();
    session
        .preview_atlas_fit(
            Actor::Agent,
            &[face],
            "test/effective",
            region(),
            &snapshot,
            None,
        )
        .unwrap();
    let original = session.document().vmf().clone();
    let preview = session.render_document().vmf().clone();
    let history = session.history_snapshot();
    let token = session.face_preview_token();
    std::fs::write(
        directory.path().join("materials/test/corrupt.vtf"),
        b"malformed VTF",
    )
    .unwrap();
    for (name, albedo) in [
        ("missing", "test/missing_albedo"),
        ("corrupt", "test/corrupt"),
        ("blank", " "),
    ] {
        std::fs::write(directory.path().join(format!("materials/test/{name}.vmt")), format!("LightmappedGeneric {{ $basetexture test/checker $bumpmap test/missing_normal $albedo \"{albedo}\" }}")).unwrap();
        assert!(
            session
                .inspect_material_atlas(&format!("test/{name}"))
                .is_err()
        );
        assert!(session.materials.find(&format!("test/{name}")).is_none());
        assert_eq!(session.face_preview_token(), token);
        assert_eq!(session.render_document().vmf(), &preview);
        assert_eq!(session.document().vmf(), &original);
        assert_eq!(session.history_snapshot(), history);
    }
    session.commit_face_preview(Actor::Agent).unwrap();
    assert_eq!(session.document().vmf(), &preview);
    assert!(directory.path().join("materials/test/checker.vtf").exists());
}
