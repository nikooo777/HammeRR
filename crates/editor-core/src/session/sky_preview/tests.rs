use super::*;
use crate::session::{Actor, PropertyTarget};

fn fixture() -> (tempfile::TempDir, Session) {
    let (directory, game) = crate::test_assets::game();
    std::fs::create_dir_all(directory.path().join("materials/skybox")).unwrap();
    let mut hdr =
        std::fs::read(directory.path().join("materials/test/checker.vtf")).unwrap()[..80].to_vec();
    hdr[52..56].copy_from_slice(&24u32.to_le_bytes());
    for _ in 0..64 * 32 {
        for value in [0x4400u16, 0x3c00, 0x3400, 0x3c00] {
            hdr.extend_from_slice(&value.to_le_bytes());
        }
    }
    std::fs::write(directory.path().join("materials/skybox/hdr.vtf"), hdr).unwrap();
    for family in ["first", "second"] {
        for suffix in render::sky::SUFFIXES {
            std::fs::write(
                directory
                    .path()
                    .join(format!("materials/skybox/{family}{suffix}.vmt")),
                "Sky { $basetexture test/checker $hdrbasetexture skybox/hdr }",
            )
            .unwrap();
        }
    }
    let mut session = Session::new(Ok(game));
    session
        .edit(Actor::Agent, "sky", |tx| {
            tx.world_properties_mut().set("skyname", "first");
            Ok(())
        })
        .unwrap();
    (directory, session)
}
#[test]
fn explicit_hdr_preview_is_atomic_and_keeps_source_history_foreign_draft_and_camera() {
    for actor in [Actor::User, Actor::Agent] {
        let (_directory, mut session) = fixture();
        session.begin_property_edit(PropertyTarget::World).unwrap();
        let draft = serde_json::to_value(session.property_draft()).unwrap();
        let source = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let camera = session.camera();
        let ldr = session.view();
        for (channel, exposure) in [
            ("bad", 0.0),
            ("hdr", -8.1),
            ("hdr", 8.1),
            ("hdr", f64::NAN),
            ("hdr", f64::INFINITY),
        ] {
            assert!(
                session
                    .sky_preview_options(ldr, Some(channel), Some(exposure))
                    .is_err()
            );
            assert_eq!(session.view(), ldr);
        }
        let mut hdr = session
            .sky_preview_options(ldr, Some("hdr"), Some(2.0))
            .unwrap();
        hdr.sky_preview = true;
        let serial = session.scene_serial();
        let temporary = session.build_scene_for_view(hdr);
        assert_eq!(temporary.skybox.unwrap().channel, Channel::Hdr);
        assert_eq!(session.scene_serial(), serial);
        assert_eq!(session.view(), ldr);
        let report = session.sky_preview_state(hdr, camera);
        assert_eq!(report["requested_channel"], "hdr");
        assert_eq!(report["exposure_ev"], 2.0);
        assert_eq!(report["tonemap"], "reinhard_per_channel");
        session.set_view(hdr);
        assert!(session.scene_serial() > serial);
        let scene_serial = session.scene_serial();
        let changed_exposure = session.sky_preview_options(hdr, None, Some(-2.0)).unwrap();
        session.set_view(changed_exposure);
        assert_eq!(session.scene_serial(), scene_serial);
        assert_eq!(session.document().vmf(), &source);
        assert_eq!(session.document().undo_labels().count(), history);
        assert_eq!(session.camera(), camera);
        assert_eq!(
            serde_json::to_value(session.property_draft()).unwrap(),
            draft
        );
        session.reload_material_assets(actor);
        assert_eq!(session.document().vmf(), &source);
    }
}
#[test]
fn duplicate_world_names_do_not_reuse_or_poison_valid_hdr_cache() {
    let (_directory, mut session) = fixture();
    let hdr = session
        .sky_preview_options(session.view(), Some("hdr"), None)
        .unwrap();
    let first = session.skybox_preview_for(hdr).unwrap();
    assert!(first.ready());
    session
        .edit(Actor::Agent, "duplicate sky", |tx| {
            tx.world_properties_mut()
                .0
                .push(("SKYNAME".into(), "second".into()));
            Ok(())
        })
        .unwrap();
    let duplicate = session.skybox_preview_for(hdr).unwrap();
    assert!(!duplicate.ready());
    assert!(
        duplicate.faces[0]
            .issue
            .as_ref()
            .unwrap()
            .contains("Duplicate world skyname")
    );
    session.undo(Actor::Agent).unwrap();
    let restored = session.skybox_preview_for(hdr).unwrap();
    assert!(Arc::ptr_eq(&first, &restored));
    session.game_error = Some("unavailable content".into());
    let unavailable = session.skybox_preview_for(hdr).unwrap();
    assert!(!unavailable.ready());
    assert!(
        unavailable
            .faces
            .iter()
            .all(|face| face.source.is_none() && face.texture_source.is_none())
    );
    session.game_error = None;
    assert!(Arc::ptr_eq(
        &first,
        &session.skybox_preview_for(hdr).unwrap()
    ));
    session
        .edit(Actor::Agent, "oversized UTF-8 family", |tx| {
            tx.world_properties_mut().set("skyname", "é".repeat(200));
            Ok(())
        })
        .unwrap();
    let report = session.sky_preview_state(hdr, session.camera());
    assert_eq!(report["name_truncated"], true);
    assert!(report["name"].as_str().unwrap().len() <= 256);
    assert!(
        report["faces"]
            .as_array()
            .unwrap()
            .iter()
            .all(|face| face["source"].is_null() && face["texture_source"].is_null())
    );
    session.undo(Actor::Agent).unwrap();
    assert!(Arc::ptr_eq(
        &first,
        &session.skybox_preview_for(hdr).unwrap()
    ));
}
#[test]
fn hdr_cache_eviction_and_reload_change_payload_identity_without_ldr_substitution() {
    let (directory, mut session) = fixture();
    let hdr = session
        .sky_preview_options(session.view(), Some("hdr"), None)
        .unwrap();
    let first = session.skybox_preview_for(hdr).unwrap();
    assert!(first.ready());
    session
        .edit(Actor::Agent, "other sky", |tx| {
            tx.world_properties_mut().set("skyname", "second");
            Ok(())
        })
        .unwrap();
    assert!(session.skybox_preview_for(hdr).unwrap().ready());
    session.undo(Actor::Agent).unwrap();
    let reloaded = session.skybox_preview_for(hdr).unwrap();
    assert_ne!(first.payload_id, reloaded.payload_id);
    std::fs::remove_file(directory.path().join("materials/skybox/hdr.vtf")).unwrap();
    assert!(session.skybox_preview_for(hdr).unwrap().ready());
    session.reload_material_assets(Actor::Agent);
    let failed = session.skybox_preview_for(hdr).unwrap();
    assert!(!failed.ready());
    assert!(
        failed
            .faces
            .iter()
            .all(|face| face.texture.is_none() && face.hdr_texture.is_none())
    );
    assert!(session.skybox_preview().unwrap().ready());
}
