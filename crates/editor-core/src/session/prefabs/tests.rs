use super::*;
use crate::session::{SelectMode, Session};

use super::fixtures::fixture;

#[cfg(unix)]
#[test]
fn late_source_failure_preserves_clipboard_owned_preview_and_map_history() {
    let (mut session, directory, brush) = fixture();
    session
        .save_prefab(
            Actor::User,
            "Candidate",
            &[brush],
            &session.prefab_snapshot().unwrap().1,
        )
        .unwrap();
    session
        .capture_clipboard(
            Actor::User,
            &[brush],
            &session.clipboard_stamp(),
            false,
            false,
        )
        .unwrap();
    let report = session
        .preview_paste(
            transfer::Options {
                offset: DVec3::X * 80.0,
                ..Default::default()
            },
            &session.clipboard_stamp(),
        )
        .unwrap();
    let token = report["preview"]["token"].as_u64().unwrap();
    let (snapshot, stamp) = session.prefab_snapshot().unwrap();
    let store = Store {
        directory: directory.path().into(),
    };
    let text = store.read("Candidate", &snapshot.stamp).unwrap();
    let payload = transfer::Payload::decode(&text, &session.definitions)
        .unwrap()
        .transformed(
            DVec3::Z * 90.0,
            DVec3::new(2.0, 1.0, 1.0),
            DVec3::ZERO,
            &session.definitions,
        )
        .unwrap();
    let source = snapshot
        .entries
        .into_iter()
        .find(|entry| entry.name == "Candidate")
        .unwrap();
    let prepared = session
        .prepare_paste(&payload, transfer::Options::default(), Some(source))
        .unwrap();
    let clipboard = session.clipboard_report();
    let rendered = session.render_document().vmf().clone();
    let original = session.document.vmf().clone();
    let history = session.document.undo_labels().count();
    let mut replacement = tempfile::NamedTempFile::new_in(directory.path()).unwrap();
    std::io::Write::write_all(&mut replacement, text.as_bytes()).unwrap();
    replacement
        .persist(directory.path().join("Candidate.vmf"))
        .unwrap();
    let result = session.finish_prefab_placement(Actor::Agent, payload, prepared, &stamp);
    assert!(result.unwrap_err().contains("changed"));
    assert_eq!(session.clipboard_report(), clipboard);
    assert_eq!(session.render_document().vmf(), &rendered);
    assert_eq!(session.document.vmf(), &original);
    assert_eq!(session.document.undo_labels().count(), history);
    session.apply_paste(Actor::User, token).unwrap();
    assert_eq!(session.document.vmf(), &rendered);
    assert_eq!(session.document.undo_labels().count(), history + 1);
}

#[cfg(target_os = "linux")]
#[test]
fn rename_preserves_map_clipboard_and_file_bytes_but_stales_owned_placement() {
    let (mut session, directory, brush) = fixture();
    for name in ["Alpha", "Beta"] {
        session
            .save_prefab(
                Actor::User,
                name,
                &[brush],
                &session.prefab_snapshot().unwrap().1,
            )
            .unwrap();
    }
    let report = session
        .preview_prefab(
            Actor::User,
            "Alpha",
            DVec3::splat(80.0),
            false,
            &session.prefab_snapshot().unwrap().1,
        )
        .unwrap();
    let token = report["paste"]["preview"]["token"].as_u64().unwrap();
    let original = session.document.vmf().clone();
    let selection = session.selected_ids();
    let history = session.document.undo_labels().count();
    let clipboard = session.clipboard_report();
    let bytes = std::fs::read(directory.path().join("Alpha.vmf")).unwrap();
    assert!(
        session
            .rename_prefab(
                Actor::Agent,
                "Alpha",
                "beta",
                &session.prefab_snapshot().unwrap().1
            )
            .is_err()
    );
    assert_eq!(session.clipboard_report(), clipboard);
    let renamed = session
        .rename_prefab(
            Actor::User,
            "Alpha",
            "Renamed",
            &session.prefab_snapshot().unwrap().1,
        )
        .unwrap();
    assert_eq!(renamed["selected"]["name"], "Renamed");
    assert!(!directory.path().join("Alpha.vmf").exists());
    assert_eq!(
        std::fs::read(directory.path().join("Renamed.vmf")).unwrap(),
        bytes
    );
    assert_eq!(session.document.vmf(), &original);
    assert_eq!(session.selected_ids(), selection);
    assert_eq!(session.document.undo_labels().count(), history);
    let stale = session.clipboard_report();
    assert_eq!(stale["payload"], clipboard["payload"]);
    assert_eq!(stale["preview"]["valid"], false);
    assert!(
        stale["preview"]["stale_reason"]
            .as_str()
            .unwrap()
            .contains("renamed")
    );
    assert!(
        session
            .apply_paste(Actor::Agent, token)
            .unwrap_err()
            .contains("renamed")
    );
    assert!(
        session
            .preview_paste(transfer::Options::default(), &session.clipboard_stamp())
            .unwrap_err()
            .contains("renamed")
    );
    session
        .rename_prefab(
            Actor::Agent,
            "Renamed",
            "Alpha",
            &session.prefab_snapshot().unwrap().1,
        )
        .unwrap();
    assert!(
        session
            .apply_paste(Actor::User, token)
            .unwrap_err()
            .contains("renamed")
    );
    session
        .rename_prefab(
            Actor::Agent,
            "Alpha",
            "Renamed",
            &session.prefab_snapshot().unwrap().1,
        )
        .unwrap();
    let fresh = session
        .preview_prefab(
            Actor::User,
            "Renamed",
            DVec3::splat(80.0),
            false,
            &session.prefab_snapshot().unwrap().1,
        )
        .unwrap();
    let token = fresh["paste"]["preview"]["token"].as_u64().unwrap();
    session
        .delete_prefab(
            Actor::User,
            "Renamed",
            &session.prefab_snapshot().unwrap().1,
        )
        .unwrap();
    assert_eq!(session.clipboard_report()["preview"]["valid"], true);
    session.apply_paste(Actor::User, token).unwrap();
    assert_eq!(session.document.undo_labels().count(), history + 1);
}

#[cfg(target_os = "linux")]
#[test]
fn another_library_writer_renaming_or_replacing_source_stales_apply() {
    let (mut session, directory, brush) = fixture();
    session
        .save_prefab(
            Actor::User,
            "Fixture",
            &[brush],
            &session.prefab_snapshot().unwrap().1,
        )
        .unwrap();
    let report = session
        .preview_prefab(
            Actor::User,
            "Fixture",
            DVec3::ZERO,
            false,
            &session.prefab_snapshot().unwrap().1,
        )
        .unwrap();
    let token = report["paste"]["preview"]["token"].as_u64().unwrap();
    let store = Store {
        directory: directory.path().into(),
    };
    store
        .rename("Fixture", "Moved", &store.snapshot().unwrap().stamp)
        .unwrap();
    assert!(
        session
            .apply_paste(Actor::Agent, token)
            .unwrap_err()
            .contains("renamed")
    );
    let report = session
        .preview_prefab(
            Actor::User,
            "Moved",
            DVec3::ZERO,
            false,
            &session.prefab_snapshot().unwrap().1,
        )
        .unwrap();
    let token = report["paste"]["preview"]["token"].as_u64().unwrap();
    let path = directory.path().join("Moved.vmf");
    let bytes = std::fs::read(&path).unwrap();
    let mut pending = tempfile::NamedTempFile::new_in(directory.path()).unwrap();
    std::io::Write::write_all(&mut pending, &bytes).unwrap();
    pending.persist(&path).unwrap();
    let original = session.document.vmf().clone();
    assert!(
        session
            .apply_paste(Actor::User, token)
            .unwrap_err()
            .contains("source changed")
    );
    assert_eq!(session.document.vmf(), &original);
}

#[test]
fn transformed_placement_keeps_ids_metadata_owned_review_and_history() {
    let (mut session, directory, brush) = fixture();
    session
        .document
        .edit("assembly metadata", |tx| {
            map_core::groups::apply(
                tx,
                &map_core::groups::Edit {
                    group: None,
                    name: "Assembly".into(),
                    parent: None,
                    members: vec![map_core::groups::Member::Object(brush)],
                    ungroup: false,
                },
            )?;
            map_core::layers::apply(
                tx,
                &map_core::layers::Edit {
                    id: None,
                    name: "Prefab layer".into(),
                    color: [40, 160, 220],
                    parent: None,
                    members: vec![brush],
                    delete: false,
                },
            )?;
            map_core::context::set_note(
                tx,
                map_core::context::Scope::Object(brush),
                "Assembly".into(),
                "Keep together".into(),
                "human",
                None,
            )
        })
        .unwrap();
    session
        .save_prefab(
            Actor::User,
            "Transformed",
            &[brush],
            &session.prefab_snapshot().unwrap().1,
        )
        .unwrap();
    let original = session.document.vmf().clone();
    let history = session.document.undo_labels().count();
    let file = std::fs::read(directory.path().join("Transformed.vmf")).unwrap();
    let placement = Placement {
        position: DVec3::new(128.0, 64.0, 16.0),
        rotation: DVec3::Z * 90.0,
        scale: DVec3::new(2.0, 0.5, 1.25),
        pivot: Some(DVec3::ZERO),
    };
    let first = session
        .preview_prefab_placement(
            Actor::User,
            "Transformed",
            placement,
            false,
            &session.prefab_snapshot().unwrap().1,
        )
        .unwrap();
    let first_token = first["paste"]["preview"]["token"].as_u64().unwrap();
    assert_eq!(session.document.vmf(), &original);
    assert_eq!(session.document.undo_labels().count(), history);
    let bounds = &first["paste"]["preview"]["bounds"];
    for (actual, expected) in bounds
        .as_array()
        .unwrap()
        .iter()
        .zip([[112.0, 64.0, 16.0], [128.0, 128.0, 56.0]])
    {
        let actual: [f64; 3] = serde_json::from_value(actual.clone()).unwrap();
        assert!(DVec3::from_array(actual).abs_diff_eq(DVec3::from_array(expected), 1e-8));
    }
    session.cancel_paste(first_token).unwrap();
    let first = session
        .preview_prefab_placement(
            Actor::Agent,
            "Transformed",
            placement,
            false,
            &session.prefab_snapshot().unwrap().1,
        )
        .unwrap();
    let old_token = first["paste"]["preview"]["token"].as_u64().unwrap();
    let second = session
        .preview_prefab_placement(
            Actor::Agent,
            "Transformed",
            placement,
            false,
            &session.prefab_snapshot().unwrap().1,
        )
        .unwrap();
    let token = second["paste"]["preview"]["token"].as_u64().unwrap();
    assert_ne!(old_token, token);
    assert!(session.cancel_paste(old_token).is_err());
    assert!(session.apply_paste(Actor::User, old_token).is_err());
    let expected = session.render_document().vmf().clone();
    let pasted = session.apply_paste(Actor::User, token).unwrap();
    assert_eq!(session.document.vmf(), &expected);
    assert_eq!(session.document.undo_labels().count(), history + 1);
    assert_eq!(pasted["group_ids"].as_array().unwrap().len(), 1);
    assert_eq!(pasted["visgroup_ids"].as_array().unwrap().len(), 1);
    let id = crate::identity::parse_id(pasted["objects"][0].as_str().unwrap()).unwrap();
    assert_ne!(id, brush);
    assert!(
        session
            .document
            .context()
            .unwrap()
            .notes
            .contains_key(&map_core::context::Scope::Object(id))
    );
    assert_eq!(
        std::fs::read(directory.path().join("Transformed.vmf")).unwrap(),
        file
    );
    session.undo(Actor::Agent).unwrap();
    assert_eq!(session.document.vmf(), &original);
    session.redo(Actor::User).unwrap();
    assert_eq!(session.document.vmf(), &expected);
    let reopened = map_core::Document::try_new(
        source_formats::vmf::Vmf::parse(
            &expected.write(&source_formats::keyvalues::WriteOptions::default()),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(reopened.vmf(), &expected);
}

#[test]
fn failed_transforms_retain_owned_preview_and_changed_definitions_reject_apply() {
    let (mut session, _directory, brush) = fixture();
    session
        .save_prefab(
            Actor::User,
            "Fixture",
            &[brush],
            &session.prefab_snapshot().unwrap().1,
        )
        .unwrap();
    let report = session
        .preview_prefab(
            Actor::User,
            "Fixture",
            DVec3::ZERO,
            false,
            &session.prefab_snapshot().unwrap().1,
        )
        .unwrap();
    let token = report["paste"]["preview"]["token"].as_u64().unwrap();
    let before = session.clipboard_report();
    for placement in [
        Placement {
            scale: DVec3::ZERO,
            ..Default::default()
        },
        Placement {
            scale: DVec3::splat(-1.0),
            ..Default::default()
        },
        Placement {
            scale: DVec3::splat(f64::INFINITY),
            ..Default::default()
        },
        Placement {
            scale: DVec3::splat(1e100),
            ..Default::default()
        },
        Placement {
            rotation: DVec3::splat(f64::NAN),
            ..Default::default()
        },
        Placement {
            pivot: Some(DVec3::splat(70000.0)),
            ..Default::default()
        },
        Placement {
            position: DVec3::splat(70000.0),
            ..Default::default()
        },
    ] {
        assert!(
            session
                .preview_prefab_placement(
                    Actor::Agent,
                    "Fixture",
                    placement,
                    false,
                    &session.prefab_snapshot().unwrap().1
                )
                .is_err()
        );
        assert_eq!(session.clipboard_report(), before);
    }
    session.definitions_serial += 1;
    let original = session.document.vmf().clone();
    assert!(session.apply_paste(Actor::User, token).is_err());
    assert_eq!(session.document.vmf(), &original);
}

#[test]
fn prefab_persistence_preview_cancel_and_stale_requests_preserve_source() {
    let directory = tempfile::tempdir().unwrap();
    let (mut session, id, _) = crate::test_assets::clipboard();
    session.prefabs.store = Ok(Store {
        directory: directory.path().into(),
    });
    let before = session.document.vmf().clone();
    let history = session.document.undo_labels().count();
    let stamp = session.prefab_snapshot().unwrap().1;
    session
        .save_prefab(Actor::User, "Fixture", &[id], &stamp)
        .unwrap();
    assert_eq!(session.document.vmf(), &before);
    assert_eq!(session.document.undo_labels().count(), history);
    assert!(
        session
            .delete_prefab(Actor::Agent, "Fixture", &stamp)
            .is_err()
    );
    let stamp = session.prefab_snapshot().unwrap().1;
    let report = session
        .preview_prefab(Actor::User, "Fixture", DVec3::splat(80.0), false, &stamp)
        .unwrap();
    let token = report["paste"]["preview"]["token"].as_u64().unwrap();
    assert_eq!(session.document.vmf(), &before);
    session.cancel_paste(token).unwrap();
    let stamp = session.prefab_snapshot().unwrap().1;
    let report = session
        .preview_prefab(Actor::Agent, "Fixture", DVec3::splat(80.0), false, &stamp)
        .unwrap();
    let token = report["paste"]["preview"]["token"].as_u64().unwrap();
    session.apply_paste(Actor::User, token).unwrap();
    assert_eq!(session.document.vmf().world.solids.len(), 2);
    session.undo(Actor::Agent).unwrap();
    assert_eq!(session.document.vmf(), &before);
    session.select(&[id], SelectMode::Replace).unwrap();
    let stamp = session.prefab_snapshot().unwrap().1;
    session.select(&[], SelectMode::Replace).unwrap();
    assert!(
        session
            .save_prefab(Actor::Agent, "Stale", &[id], &stamp)
            .is_err()
    );
    let mut fresh = Session::new(Err("no assets".into()));
    fresh.prefabs.store = Ok(Store {
        directory: directory.path().into(),
    });
    assert_eq!(
        fresh.prefab_report("Fix", 0, 64, Some("Fixture"))["selected"]["objects"],
        1
    );
    let stamp = fresh.prefab_snapshot().unwrap().1;
    fresh.delete_prefab(Actor::User, "Fixture", &stamp).unwrap();
    assert_eq!(fresh.prefab_report("", 0, 64, None)["total"], 0);
}

#[test]
fn invalid_files_or_placement_retain_clipboard_and_owned_paste() {
    let directory = tempfile::tempdir().unwrap();
    let (mut session, id, _) = crate::test_assets::clipboard();
    session.prefabs.store = Ok(Store {
        directory: directory.path().into(),
    });
    let stamp = session.prefab_snapshot().unwrap().1;
    session
        .save_prefab(Actor::Agent, "Fixture", &[id], &stamp)
        .unwrap();
    let stamp = session.prefab_snapshot().unwrap().1;
    session
        .preview_prefab(Actor::Agent, "Fixture", DVec3::ZERO, false, &stamp)
        .unwrap();
    let before = session.clipboard_report();
    let stamp = session.prefab_snapshot().unwrap().1;
    assert!(
        session
            .preview_prefab(
                Actor::User,
                "Fixture",
                DVec3::splat(f64::NAN),
                false,
                &stamp
            )
            .is_err()
    );
    assert_eq!(session.clipboard_report(), before);
    std::fs::write(directory.path().join("Broken.vmf"), "not a VMF").unwrap();
    let stamp = session.prefab_snapshot().unwrap().1;
    assert!(
        session
            .preview_prefab(Actor::User, "Broken", DVec3::ZERO, false, &stamp)
            .is_err()
    );
    assert_eq!(session.clipboard_report(), before);
}

#[test]
fn pending_partitions_refuse_placement_before_replacing_clipboard() {
    let (mut session, directory, brush) = fixture();
    let ObjectId::Solid(solid) = brush else {
        unreachable!()
    };
    session
        .capture_clipboard(
            Actor::Agent,
            &[brush],
            &session.clipboard_stamp(),
            false,
            false,
        )
        .unwrap();
    let point = session
        .create_entity(
            Actor::Agent,
            "info_target",
            Some(DVec3::splat(80.0)),
            std::collections::BTreeMap::new(),
        )
        .unwrap();
    let stamp = session.prefab_snapshot().unwrap().1;
    session
        .save_prefab(Actor::Agent, "Different", &[point], &stamp)
        .unwrap();
    let source = session.document.solid(solid).unwrap();
    let side = source
        .sides
        .iter()
        .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
        .unwrap()
        .id;
    let edges = map_core::subobject::crease_pairs(source, side).unwrap()[0].edges;
    session
        .set_component_picking(
            Some(crate::session::component_picking::Kind::Face),
            &[solid],
            &session.component_stamp(),
        )
        .unwrap();
    session
        .select_components(
            &[map_core::subobject::Component {
                solid,
                sides: vec![side],
            }],
            SelectMode::Replace,
            &session.component_stamp(),
        )
        .unwrap();
    session
        .preview_component_partition(
            &[crate::session::components::CreaseEdges { solid, edges }],
            &session.component_stamp(),
        )
        .unwrap();
    let before = session.clipboard_report();
    let stamp = session.prefab_snapshot().unwrap().1;
    assert!(
        session
            .preview_prefab(Actor::User, "Different", DVec3::ZERO, false, &stamp)
            .unwrap_err()
            .contains("partition")
    );
    assert_eq!(session.clipboard_report(), before);
    assert!(directory.path().join("Different.vmf").exists());
    assert!(session.partition_preview_token().is_some());
}
