use super::*;
use std::sync::Barrier;

#[test]
fn concurrent_same_observation_mutations_serialize_and_preserve_winning_payload() {
    let directory = tempfile::tempdir().unwrap();
    let authority = Authority::from_directory(Ok(directory.path().into()));
    let snapshot = authority.snapshot();
    let start = Arc::new(Barrier::new(3));
    let workers = ["Alpha", "Beta"].map(|name| {
        let authority = authority.clone();
        let snapshot = snapshot.clone();
        let start = start.clone();
        std::thread::spawn(move || {
            start.wait();
            authority.change(
                snapshot.revision,
                &snapshot.stamp,
                Edit::Save { name, text: name },
            )
        })
    });
    start.wait();
    let results = workers.map(|worker| worker.join().unwrap());
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert!(
        results
            .iter()
            .filter_map(|result| result.as_ref().err())
            .all(|error| error.contains("changed"))
    );
    let current = authority.snapshot();
    assert_eq!(current.entries.len(), 1);
    let entry = &current.entries[0];
    assert_eq!(
        std::fs::read_to_string(directory.path().join(format!("{}.vmf", entry.name))).unwrap(),
        entry.name
    );
    assert_eq!(snapshot.entries.len(), 0);
}

#[test]
fn committed_write_receipt_preserves_warning_and_unavailable_catalog() {
    let directory = tempfile::tempdir().unwrap();
    let authority = Authority::from_directory(Ok(directory.path().into()));
    let mut state = authority.lock().unwrap();
    let stamp = state.catalog.as_ref().unwrap().stamp.clone();
    let mutation = state
        .store
        .as_ref()
        .unwrap()
        .save("Committed", "published bytes", &stamp)
        .unwrap();
    state.store = Err("Injected post-publication observation failure".into());
    state.observe();
    let receipt = state.completed(Ok(mutation)).unwrap();
    assert!(receipt.entries.is_empty());
    assert_eq!(
        receipt.error.as_deref(),
        Some("Injected post-publication observation failure")
    );
    assert_eq!(
        std::fs::read_to_string(directory.path().join("Committed.vmf")).unwrap(),
        "published bytes"
    );
    let receipt = state
        .completed(Ok(super::super::Mutation {
            warning: Some("Published into the inspected directory; configured path changed".into()),
        }))
        .unwrap();
    let revision = receipt.revision;
    assert!(receipt.write_error.is_some());
    drop(state);
    let observed = authority.snapshot();
    assert_eq!(observed.write_error, receipt.write_error);
    assert_eq!(observed.error, receipt.error);
    assert_eq!(observed.revision, revision);
    authority
        .bind(Ok(directory.path().join("replacement")))
        .unwrap();
    let rebound = authority.snapshot();
    assert!(rebound.write_error.is_none());
    assert!(rebound.error.is_none());
    assert!(rebound.revision > revision);
}

#[test]
fn poisoned_registry_is_explicitly_unavailable_and_never_returns_old_rows() {
    let directory = tempfile::tempdir().unwrap();
    let authority = Authority::from_directory(Ok(directory.path().into()));
    let poisoned = authority.clone();
    assert!(
        std::thread::spawn(move || {
            let _guard = poisoned.state.lock().unwrap();
            panic!("injected registry poisoning");
        })
        .join()
        .is_err()
    );
    let snapshot = authority.snapshot();
    assert_eq!(snapshot.error.as_deref(), Some(POISONED));
    assert!(snapshot.entries.is_empty());
    assert_eq!(authority.serial(), u64::MAX);
    assert!(authority.check(1, "").unwrap_err().contains("poisoned"));
}

#[test]
fn independent_reconfiguration_changes_only_its_own_catalog() {
    let directory = tempfile::tempdir().unwrap();
    let first = Authority::from_directory(Ok(directory.path().join("first")));
    let second = Authority::from_directory(Ok(directory.path().join("second")));
    let second_before = second.snapshot();
    let inspected = first.snapshot();
    first
        .change(
            inspected.revision,
            &inspected.stamp,
            Edit::Save {
                name: "First",
                text: "first",
            },
        )
        .unwrap();
    first.bind(Ok(directory.path().join("third"))).unwrap();
    assert!(first.snapshot().entries.is_empty());
    assert!(directory.path().join("first/First.vmf").exists());
    assert_eq!(second.snapshot().stamp, second_before.stamp);
    assert_eq!(second.serial(), second_before.revision);
}

#[test]
fn owned_catalog_snapshot_retains_the_observed_directory_guard_after_rebinding() {
    let directory = tempfile::tempdir().unwrap();
    let authority = Authority::from_directory(Ok(directory.path().join("original")));
    let snapshot = authority.snapshot();
    let observation = Arc::downgrade(snapshot._observation.as_ref().unwrap());
    authority
        .bind(Ok(directory.path().join("replacement")))
        .unwrap();
    assert!(observation.upgrade().is_some());
    drop(snapshot);
    assert!(observation.upgrade().is_none());
}
