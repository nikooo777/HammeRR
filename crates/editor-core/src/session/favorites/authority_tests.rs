use super::*;
use crate::material_favorites::{FILE_NAME, Persistence, Store};
use glam::DVec3;
use std::collections::BTreeSet;

#[test]
fn application_configuration_failure_is_bounded_and_reported_without_reconfiguration() {
    let authority = Authority::configured(Err(
        "x".repeat(crate::material_favorites::MAX_ERROR_BYTES * 2)
    ));
    let mut session = Session::with_material_favorites(Err("no assets".into()), authority);
    let status = session.material_favorite_status();
    let error = status.error.unwrap();
    assert!(error.starts_with("Material favorites could not load:"));
    assert!(error.len() <= crate::material_favorites::MAX_ERROR_BYTES);
    assert_eq!(session.recent_log().next().unwrap().text, error);
    let revision = session.material_favorites_serial();
    let directory = tempfile::tempdir().unwrap();
    session.configure_material_favorites(Ok(directory.path().to_owned()));
    assert_eq!(session.material_favorites_serial(), revision);
    assert_eq!(
        session.material_favorite_status().persistence,
        Persistence::Blocked
    );
    assert_eq!(
        session.material_favorite_status().error.as_deref(),
        Some(error.as_str())
    );
    assert!(!directory.path().join(FILE_NAME).exists());
}

#[test]
fn attached_sessions_share_favorites_errors_and_reload_without_rebinding_document_recovery() {
    let directory = tempfile::tempdir().unwrap();
    let shared = directory.path().join("shared");
    let authority = Authority::configured(Ok(shared.clone()));
    let mut first = Session::with_material_favorites(Err("no assets".into()), authority.clone());
    let mut second = Session::with_material_favorites(Err("no assets".into()), authority);
    first.configure_recovery(Ok(directory.path().join("first-recovery")));
    second.configure_recovery(Ok(directory.path().join("second-recovery")));
    first.favorite_material("one", true).unwrap();
    assert!(second.is_material_favorite("one"));
    assert_eq!(
        second.material_favorite_status().directory,
        Some(shared.clone())
    );
    let revision = first.material_favorites_serial();
    second.configure_recovery(Err("different recovery failure".into()));
    assert_eq!(first.material_favorites_serial(), revision);
    assert_eq!(second.material_favorites_serial(), revision);
    second.favorite_material("one", false).unwrap();
    assert!(!first.is_material_favorite("one"));
    Store {
        directory: shared.clone(),
    }
    .save(&BTreeSet::from(["external".into()]))
    .unwrap();
    second.reload_material_favorites().unwrap();
    assert!(first.is_material_favorite("external"));
    let path = shared.join(FILE_NAME);
    let valid = std::fs::read(&path).unwrap();
    std::fs::write(&path, b"broken").unwrap();
    first.reload_material_favorites().unwrap_err();
    assert_eq!(
        second.material_favorite_status().persistence,
        Persistence::Blocked
    );
    assert_eq!(
        first.material_favorite_status().error,
        second.material_favorite_status().error
    );
    let revision = first.material_favorites_serial();
    second.configure_recovery(Ok(directory.path().join("another-recovery")));
    assert_eq!(second.material_favorites_serial(), revision);
    assert!(second.favorite_material("blocked", true).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"broken");
    std::fs::write(&path, valid).unwrap();
    second.reload_material_favorites().unwrap();
    assert!(first.material_favorite_status().error.is_none());
    first.favorite_material("first", true).unwrap();
    assert!(second.is_material_favorite("first"));
    assert_eq!(
        first.material_favorites_serial(),
        second.material_favorites_serial()
    );
    assert!(
        !directory
            .path()
            .join("first-recovery")
            .join(FILE_NAME)
            .exists()
    );
    assert!(
        !directory
            .path()
            .join("another-recovery")
            .join(FILE_NAME)
            .exists()
    );
}

#[test]
fn shared_authority_preserves_separate_documents_drafts_history_and_catalog_snapshots() {
    let authority = Authority::default();
    let mut first = Session::with_material_favorites(Err("no assets".into()), authority.clone());
    let mut second = Session::with_material_favorites(Err("no assets".into()), authority);
    let block = first
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(32.),
            "first/map",
            false,
        )
        .unwrap();
    first
        .preview_transform(
            Actor::User,
            &[block],
            map_core::edit::Transform {
                translation: DVec3::X * 16.,
                ..Default::default()
            },
            None,
        )
        .unwrap();
    second
        .begin_property_edit(crate::session::PropertyTarget::World)
        .unwrap();
    let first_source = first.document().vmf().clone();
    let first_preview = first.render_document().vmf().clone();
    let first_history = first.history_snapshot();
    let second_source = second.document().vmf().clone();
    let second_history = second.history_snapshot();
    let token = second.property_draft().unwrap().token;
    let first_camera = first.camera();
    let second_camera = second.camera();
    first.remember_material("first/recent");
    assert!(second.material_recent.is_empty());
    let serials = (
        first.scene_serial(),
        first.view_serial(),
        first.camera_serial(),
        second.scene_serial(),
        second.view_serial(),
        second.camera_serial(),
    );
    let old = second.material_favorites_snapshot();
    first.favorite_material("shared", true).unwrap();
    let frozen = second
        .material_catalog_with_favorites("", "favorites", 0, 36, &old)
        .unwrap();
    assert_eq!(frozen.total, 0);
    assert_eq!(frozen.favorites.count, 0);
    let catalog = second.material_catalog("", "favorites", 0, 36).unwrap();
    assert_eq!(catalog.total, 1);
    assert_eq!(catalog.favorites.count, 1);
    assert!(catalog.entries[0].favorite);
    assert!(first.favorite_material("../invalid", true).is_err());
    assert!(second.material_favorite_status().error.is_some());
    second.favorite_material("second", true).unwrap();
    assert!(first.material_favorite_status().error.is_none());
    assert_eq!(first.document().vmf(), &first_source);
    assert_eq!(first.render_document().vmf(), &first_preview);
    assert_eq!(first.history_snapshot(), first_history);
    assert_eq!(second.document().vmf(), &second_source);
    assert_eq!(second.history_snapshot(), second_history);
    assert_eq!(second.property_draft().unwrap().token, token);
    assert_eq!(first.camera(), first_camera);
    assert_eq!(second.camera(), second_camera);
    assert_eq!(
        (
            first.scene_serial(),
            first.view_serial(),
            first.camera_serial(),
            second.scene_serial(),
            second.view_serial(),
            second.camera_serial()
        ),
        serials
    );
    let mut independent = Session::new(Err("no assets".into()));
    independent.favorite_material("independent", true).unwrap();
    assert!(!independent.is_material_favorite("shared"));
    assert!(!first.is_material_favorite("independent"));
}

#[test]
fn two_document_workers_serialize_shared_writes_without_session_to_session_locks() {
    let directory = tempfile::tempdir().unwrap();
    let authority = Authority::configured(Ok(directory.path().to_owned()));
    let first = Session::with_material_favorites(Err("no assets".into()), authority.clone());
    let second = Session::with_material_favorites(Err("no assets".into()), authority.clone());
    let workers: Vec<_> = [first, second]
        .into_iter()
        .enumerate()
        .map(|(index, mut session)| {
            std::thread::spawn(move || {
                for entry in 0..8 {
                    session
                        .favorite_material(&format!("worker{index}/{entry}"), true)
                        .unwrap();
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
    let session = Session::with_material_favorites(Err("no assets".into()), authority);
    assert_eq!(session.material_favorite_status().count, 16);
    assert!(session.material_favorite_status().error.is_none());
    assert_eq!(
        Store {
            directory: directory.path().to_owned()
        }
        .load()
        .unwrap(),
        session.material_favorites_snapshot().materials
    );
}
