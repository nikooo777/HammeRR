use super::*;
use crate::session::{SelectMode, measurements};
use source_formats::keyvalues::WriteOptions;

fn attached() -> (Session, Session, tempfile::TempDir, ObjectId) {
    let directory = tempfile::tempdir().unwrap();
    let authority = Authority::from_directory(Ok(directory.path().into()));
    let mut first = Session::with_prefabs(Err("No assets required".into()), authority.clone());
    let second = Session::with_prefabs(Err("No assets required".into()), authority);
    let brush = first
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(32.0),
            "brick/test",
            false,
        )
        .unwrap();
    first.select(&[brush], SelectMode::Replace).unwrap();
    (first, second, directory, brush)
}

fn stamp(session: &Session) -> String {
    session.prefab_report("", 0, 64, None)["snapshot"]
        .as_str()
        .unwrap()
        .into()
}

fn document_state(session: &Session) -> Value {
    json!({
        "source": session.document().vmf().write(&WriteOptions::default()),
        "render": session.render_document().vmf().write(&WriteOptions::default()),
        "history": session.history_snapshot(),
        "generation": session.document_generation(),
        "revision": session.document().revision(),
        "selection": format!("{:?}", session.selected_ids()),
        "camera": format!("{:?}", session.camera()),
        "serials": [session.scene_serial(),session.view_serial(),session.camera_serial()],
        "clipboard": session.clipboard_report(),
        "measurements": session.measurement_state(),
    })
}

#[test]
fn attached_catalog_writes_preserve_geometry_review_measurement_pick_and_render_stamps() {
    let (mut first, mut second, directory, brush) = attached();
    first
        .preview_transform(
            Actor::User,
            &[brush],
            map_core::edit::Transform {
                translation: DVec3::X * 12.0,
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let review = second
        .preview_measurement(
            Actor::User,
            measurements::Request {
                id: None,
                name: "Retained ruler".into(),
                start: DVec3::ZERO,
                end: DVec3::X * 64.0,
            },
            &second.measurement_stamp(),
        )
        .unwrap();
    second
        .begin_measurement_pick(
            Actor::User,
            review["draft"]["token"].as_u64().unwrap(),
            measurements::Endpoint::End,
            second.camera(),
            (640, 480),
            &second.measurement_pick_stamp(),
        )
        .unwrap();
    let first_state = document_state(&first);
    let second_state = document_state(&second);
    let revision = second.prefab_registry_serial();
    let old = second.prefab_report("", 0, 64, None);
    let saved = first
        .save_prefab(Actor::User, "Alpha", &[brush], &stamp(&first))
        .unwrap();
    assert_eq!(saved["write_committed"], true);
    assert!(second.prefab_registry_serial() > revision);
    assert_eq!(
        second.prefab_report("", 0, 64, None)["entries"][0]["name"],
        "Alpha"
    );
    assert_eq!(old["entries"], json!([]));
    first
        .rename_prefab(Actor::Agent, "Alpha", "Beta", &stamp(&first))
        .unwrap();
    assert_eq!(
        second.prefab_report("", 0, 64, Some("Beta"))["selected"]["objects"],
        1
    );
    second
        .delete_prefab(Actor::User, "Beta", &stamp(&second))
        .unwrap();
    assert_eq!(first.prefab_report("", 0, 64, None)["total"], 0);
    std::fs::create_dir(directory.path().join("Blocked.vmf")).unwrap();
    assert!(first.prefab_report("", 0, 64, None)["error"].is_string());
    assert!(second.prefab_report("", 0, 64, None)["error"].is_string());
    std::fs::remove_dir(directory.path().join("Blocked.vmf")).unwrap();
    assert!(first.prefab_report("", 0, 64, None)["error"].is_null());
    assert_eq!(document_state(&first), first_state);
    assert_eq!(document_state(&second), second_state);
    assert_eq!(
        second.measurement_state()["draft"]["picking"]["valid"],
        true
    );
}

#[test]
fn stable_scans_keep_frozen_actions_valid_and_peer_writes_reject_losing_guards() {
    let (mut first, mut second, directory, brush) = attached();
    let first_stamp = stamp(&first);
    let second_stamp = stamp(&second);
    let revision = first.prefab_registry_serial();
    for _ in 0..3 {
        assert_eq!(stamp(&first), first_stamp);
        assert_eq!(stamp(&second), second_stamp);
        assert_eq!(first.prefab_registry_serial(), revision);
    }
    assert!(
        first
            .save_prefab(Actor::User, "../invalid", &[brush], &first_stamp)
            .is_err()
    );
    assert_eq!(first.prefab_registry_serial(), revision);
    first
        .save_prefab(Actor::User, "Winner", &[brush], &first_stamp)
        .unwrap();
    let second_state = document_state(&second);
    assert!(
        second
            .delete_prefab(Actor::Agent, "Winner", &second_stamp)
            .unwrap_err()
            .contains("changed")
    );
    assert!(directory.path().join("Winner.vmf").exists());
    assert_eq!(document_state(&second), second_state);
}

#[test]
fn shared_unavailable_observation_clears_rows_and_repair_advances_identical_metadata() {
    let (mut first, second, directory, brush) = attached();
    first
        .save_prefab(Actor::User, "Alpha", &[brush], &stamp(&first))
        .unwrap();
    let healthy = second.prefab_report("", 0, 64, None);
    let revision = first.prefab_registry_serial();
    std::fs::create_dir(directory.path().join("Invalid.vmf")).unwrap();
    let unavailable = first.prefab_report("", 0, 64, None);
    assert!(unavailable["error"].is_string());
    assert_eq!(unavailable["entries"], json!([]));
    assert!(unavailable["snapshot"].is_null());
    assert!(second.prefab_registry_serial() > revision);
    assert_eq!(
        second.prefab_report("", 0, 64, None)["error"],
        unavailable["error"]
    );
    let failed_revision = second.prefab_registry_serial();
    assert_eq!(
        first.prefab_report("", 0, 64, None)["registry_revision"],
        failed_revision
    );
    std::fs::remove_dir(directory.path().join("Invalid.vmf")).unwrap();
    let repaired = second.prefab_report("", 0, 64, None);
    assert_eq!(repaired["entries"], healthy["entries"]);
    assert!(repaired["error"].is_null());
    assert!(second.prefab_registry_serial() > failed_revision);
    assert!(
        first
            .delete_prefab(Actor::User, "Alpha", healthy["snapshot"].as_str().unwrap())
            .is_err()
    );
}

#[test]
fn attached_recovery_configuration_cannot_rebind_prefabs() {
    let (mut first, mut second, directory, brush) = attached();
    let revision = first.prefab_registry_serial();
    first.configure_recovery(Ok(directory.path().join("first-recovery")));
    second.configure_recovery(Err("Second recovery unavailable".into()));
    assert_eq!(first.prefab_registry_serial(), revision);
    assert_eq!(second.prefab_registry_serial(), revision);
    first
        .save_prefab(Actor::Agent, "Fixed", &[brush], &stamp(&first))
        .unwrap();
    assert_eq!(
        second.prefab_report("", 0, 64, None)["directory"],
        directory.path().to_str().unwrap()
    );
    assert!(directory.path().join("Fixed.vmf").exists());
    assert!(
        !directory
            .path()
            .join("first-recovery/prefabs/Fixed.vmf")
            .exists()
    );
}

#[test]
fn attached_configuration_failure_remains_explicit_after_recovery_reconfiguration() {
    let authority = Authority::from_directory(Err("Prefab directory unavailable".into()));
    let mut first = Session::with_prefabs(Err("No assets required".into()), authority.clone());
    let second = Session::with_prefabs(Err("No assets required".into()), authority);
    let before = first.prefab_report("", 0, 64, None);
    let directory = tempfile::tempdir().unwrap();
    first.configure_recovery(Ok(directory.path().join("recovery")));
    let after = second.prefab_report("", 0, 64, None);
    assert_eq!(after["error"], "Prefab directory unavailable");
    assert_eq!(after["registry_revision"], before["registry_revision"]);
    assert_eq!(after["entries"], json!([]));
    assert!(after["directory"].is_null());
    assert!(!directory.path().join("recovery/prefabs").exists());
}

#[cfg(unix)]
#[test]
fn non_utf8_configuration_is_reported_without_panicking_during_json_serialization() {
    use std::os::unix::ffi::OsStringExt;
    let directory = PathBuf::from(std::ffi::OsString::from_vec(b"/tmp/prefab-\xff".to_vec()));
    let authority = Authority::from_directory(Ok(directory));
    let session = Session::with_prefabs(Err("No assets required".into()), authority);
    let report = session.prefab_report("", 0, 64, None);
    assert!(report["error"].as_str().unwrap().contains("UTF-8"));
    assert!(report["directory"].is_string());
    assert_eq!(report["entries"], json!([]));
}

#[test]
fn peer_source_changes_stale_only_relevant_paste_and_deletion_keeps_detached_payload() {
    let (mut first, mut second, _directory, brush) = attached();
    first
        .save_prefab(Actor::User, "Source", &[brush], &stamp(&first))
        .unwrap();
    let preview = second
        .preview_prefab(
            Actor::Agent,
            "Source",
            DVec3::X * 80.0,
            false,
            &stamp(&second),
        )
        .unwrap();
    let token = preview["paste"]["preview"]["token"].as_u64().unwrap();
    first
        .save_prefab(Actor::User, "Unrelated", &[brush], &stamp(&first))
        .unwrap();
    assert_eq!(second.clipboard_report()["preview"]["valid"], true);
    first
        .rename_prefab(Actor::User, "Source", "Renamed", &stamp(&first))
        .unwrap();
    assert!(
        second
            .apply_paste(Actor::User, token)
            .unwrap_err()
            .contains("renamed")
    );
    let preview = second
        .preview_prefab(
            Actor::Agent,
            "Renamed",
            DVec3::X * 80.0,
            false,
            &stamp(&second),
        )
        .unwrap();
    let token = preview["paste"]["preview"]["token"].as_u64().unwrap();
    first
        .delete_prefab(Actor::User, "Renamed", &stamp(&first))
        .unwrap();
    second.apply_paste(Actor::User, token).unwrap();
    assert_eq!(second.document().vmf().world.solids.len(), 1);
}

#[test]
fn decode_and_thumbnail_failures_stay_local_to_the_source_session() {
    let (mut first, second, directory, brush) = attached();
    first
        .save_prefab(Actor::User, "Good", &[brush], &stamp(&first))
        .unwrap();
    std::fs::write(directory.path().join("Malformed.vmf"), "not VMF").unwrap();
    let malformed = first.prefab_report("", 0, 64, Some("Malformed"));
    assert_eq!(malformed["selected_exists"], true);
    assert!(malformed["selection_error"].is_string());
    let revision = first.prefab_registry_serial();
    assert!(first.prefab_thumbnail_report("Malformed")["error"].is_string());
    assert!(
        first
            .preview_prefab(
                Actor::Agent,
                "Good",
                DVec3::splat(f64::NAN),
                false,
                &stamp(&first)
            )
            .is_err()
    );
    let healthy = second.prefab_report("", 0, 64, Some("Good"));
    assert!(healthy["error"].is_null());
    assert_eq!(healthy["selected"]["objects"], 1);
    assert_eq!(second.prefab_registry_serial(), revision);
    assert_eq!(
        second.prefab_thumbnail_report("Good")["graphics"]["initialized"],
        false
    );
    assert_eq!(
        first.prefab_thumbnail_report("Good")["graphics"]["initialized"],
        false
    );
}

#[test]
fn shared_metadata_keeps_fgd_reference_interpretation_and_destination_matches_local() {
    let (mut first, mut second, _directory, _brush) = attached();
    let typed = source_formats::fgd::Catalog::parse("@PointClass = sample [ targetname(string) : \"Name\" reference(target_destination) : \"Reference\" ]").unwrap();
    let plain = source_formats::fgd::Catalog::parse(
        "@PointClass = sample [ targetname(string) : \"Name\" reference(string) : \"Reference\" ]",
    )
    .unwrap();
    first.set_definitions(Actor::User, typed.clone());
    second.set_definitions(Actor::User, plain);
    let source = first
        .create_entity(
            Actor::User,
            "sample",
            Some(DVec3::ZERO),
            std::collections::BTreeMap::from([("reference".into(), "destination".into())]),
        )
        .unwrap();
    first
        .save_prefab(Actor::User, "References", &[source], &stamp(&first))
        .unwrap();
    let first_report = first.prefab_report("", 0, 64, Some("References"));
    let second_report = second.prefab_report("", 0, 64, Some("References"));
    assert_eq!(
        first_report["selected"]["external_links"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        second_report["selected"]["external_links"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    assert_eq!(first_report["entries"], second_report["entries"]);
    assert_eq!(
        first_report["registry_revision"],
        second_report["registry_revision"]
    );
    assert!(first_report["error"].is_null());
    assert!(second_report["error"].is_null());
    second.set_definitions(Actor::User, typed);
    second
        .create_entity(
            Actor::User,
            "sample",
            Some(DVec3::X * 64.0),
            std::collections::BTreeMap::from([("targetname".into(), "destination".into())]),
        )
        .unwrap();
    let destination_report = second.prefab_report("", 0, 64, Some("References"));
    assert_eq!(
        destination_report["selected"]["external_links"][0]["destination_matches"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        first.prefab_report("", 0, 64, Some("References"))["selected"]["external_links"][0]["destination_matches"],
        json!([])
    );
    assert_eq!(
        destination_report["registry_revision"],
        first_report["registry_revision"]
    );
}

#[test]
fn peer_replacement_invalidates_only_its_source_and_preserves_owned_paste_on_failed_admission() {
    let (mut first, mut second, directory, brush) = attached();
    first
        .save_prefab(Actor::User, "Source", &[brush], &stamp(&first))
        .unwrap();
    let preview = second
        .preview_prefab(
            Actor::Agent,
            "Source",
            DVec3::X * 80.0,
            false,
            &stamp(&second),
        )
        .unwrap();
    let token = preview["paste"]["preview"]["token"].as_u64().unwrap();
    let source = second.document().vmf().clone();
    let rendered = second.render_document().vmf().clone();
    let clipboard = second.clipboard_report();
    let text = std::fs::read(directory.path().join("Source.vmf")).unwrap();
    let _original = std::fs::File::open(directory.path().join("Source.vmf")).unwrap();
    first
        .delete_prefab(Actor::User, "Source", &stamp(&first))
        .unwrap();
    first
        .save_prefab(Actor::User, "Source", &[brush], &stamp(&first))
        .unwrap();
    assert_eq!(
        std::fs::read(directory.path().join("Source.vmf")).unwrap(),
        text
    );
    assert!(
        second
            .apply_paste(Actor::User, token)
            .unwrap_err()
            .contains("changed")
    );
    assert_eq!(second.document().vmf(), &source);
    assert_eq!(second.render_document().vmf(), &rendered);
    assert_eq!(second.clipboard_report()["payload"], clipboard["payload"]);
    assert_eq!(second.clipboard_report()["preview"]["token"], token);
}
