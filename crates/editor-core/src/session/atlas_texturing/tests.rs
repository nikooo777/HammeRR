use super::*;
use crate::session::SelectMode;
use glam::{DVec2, DVec3};
use map_core::ObjectId;

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
