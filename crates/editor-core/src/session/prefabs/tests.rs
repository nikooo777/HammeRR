use super::*;
use crate::session::{SelectMode, Session};

use super::fixtures::fixture;

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
