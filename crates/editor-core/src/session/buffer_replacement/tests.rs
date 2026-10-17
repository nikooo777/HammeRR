use super::*;

#[derive(Clone, Copy)]
enum Replacement {
    New,
    Open,
    Recent,
    Recovery,
}

const REPLACEMENTS: [Replacement; 4] = [
    Replacement::New,
    Replacement::Open,
    Replacement::Recent,
    Replacement::Recovery,
];

fn fixture() -> (tempfile::TempDir, Session, PathBuf, String) {
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session.configure_recent_maps(Ok(directory.path().join("preferences")));
    session.configure_recovery(Ok(directory.path().join("recovery")));
    let path = directory.path().join("source.vmf");
    std::fs::write(&path, crate::test_assets::room().write(&Default::default())).unwrap();
    session.open(Actor::User, &path, false).unwrap();
    let request = session
        .begin_recovery_snapshot(crate::recovery::Kind::Manual)
        .unwrap();
    let entry = session
        .finish_recovery_snapshot(Actor::User, &request, request.run())
        .unwrap();
    (directory, session, path, entry.id)
}

fn replace(
    session: &mut Session,
    actor: Actor,
    replacement: Replacement,
    path: &Path,
    recovery_id: &str,
    discard: bool,
    review: &BufferReplacementReview,
) -> Result<(), String> {
    match replacement {
        Replacement::New => session.new_map_reviewed(actor, discard, review),
        Replacement::Open => session
            .open_reviewed(actor, path, discard, review)
            .map(|_| ()),
        Replacement::Recent => session
            .open_recent_map_reviewed(actor, path, discard, review)
            .map(|_| ()),
        Replacement::Recovery => {
            session.restore_recovery_snapshot_reviewed(actor, recovery_id, discard, None, review)
        }
    }
}

fn recovery_state(session: &Session) -> serde_json::Value {
    let mut state = session.recovery_status();
    state
        .as_object_mut()
        .unwrap()
        .remove("next_autosave_seconds");
    state
}

#[test]
fn reviewed_replacements_reject_peer_edits_and_saves_without_mutation() {
    for actor in [Actor::User, Actor::Agent] {
        for replacement in REPLACEMENTS {
            for save in [false, true] {
                let (directory, mut session, path, recovery_id) = fixture();
                let review = session.buffer_replacement_review();
                let source = std::fs::read(&path).unwrap();
                if save {
                    let revision = session.document().revision();
                    session
                        .save(Actor::Agent, Some(&directory.path().join("copy.vmf")))
                        .unwrap();
                    assert_eq!(session.document().revision(), revision);
                } else {
                    session
                        .create_block(
                            Actor::Agent,
                            DVec3::ZERO,
                            DVec3::splat(16.0),
                            "brick/test",
                            false,
                        )
                        .unwrap();
                }
                let document = session.document().vmf().clone();
                let history = session.history_snapshot();
                let path_before = session.path().map(Path::to_owned);
                let camera = session.camera();
                let recent = session.recent_maps();
                let recovery = recovery_state(&session);
                let serials = (
                    session.scene_serial(),
                    session.view_serial(),
                    session.log_serial(),
                );
                let error = replace(
                    &mut session,
                    actor,
                    replacement,
                    &path,
                    &recovery_id,
                    true,
                    &review,
                )
                .unwrap_err();
                assert!(error.contains("Map changed"));
                assert_eq!(session.document().vmf(), &document);
                assert_eq!(session.history_snapshot(), history);
                assert_eq!(session.path(), path_before.as_deref());
                assert_eq!(session.camera(), camera);
                assert_eq!(session.recent_maps(), recent);
                assert_eq!(recovery_state(&session), recovery);
                assert_eq!(
                    (
                        session.scene_serial(),
                        session.view_serial(),
                        session.log_serial()
                    ),
                    serials
                );
                assert_eq!(std::fs::read(&path).unwrap(), source);
            }
        }
    }
}

#[test]
fn matching_review_keeps_discard_opt_in_and_is_consumed_by_replacement() {
    for replacement in REPLACEMENTS {
        let (_directory, mut session, path, recovery_id) = fixture();
        session
            .create_block(
                Actor::Agent,
                DVec3::ZERO,
                DVec3::splat(16.0),
                "brick/test",
                false,
            )
            .unwrap();
        let review = session.buffer_replacement_review();
        assert!(
            replace(
                &mut session,
                Actor::User,
                replacement,
                &path,
                &recovery_id,
                false,
                &review
            )
            .is_err()
        );
        session.ensure_buffer_replacement_review(&review).unwrap();
        replace(
            &mut session,
            Actor::User,
            replacement,
            &path,
            &recovery_id,
            true,
            &review,
        )
        .unwrap();
        let current = session.buffer_replacement_review();
        assert!(
            replace(
                &mut session,
                Actor::User,
                replacement,
                &path,
                &recovery_id,
                true,
                &review
            )
            .unwrap_err()
            .contains("Map changed")
        );
        assert_eq!(session.buffer_replacement_review(), current);
    }
}

#[test]
fn replacement_reviews_reject_foreign_sessions_and_partition_only_changes() {
    use crate::session::components::CreaseEdges;
    use map_core::subobject::{self, Component};

    let (directory, mut session, path, recovery_id) = fixture();
    let mut other = Session::new(Err("no assets needed".into()));
    other.open(Actor::User, &path, false).unwrap();
    assert_eq!(other.document_generation(), session.document_generation());
    assert_eq!(other.document().revision(), session.document().revision());
    assert_eq!(other.history_save_serial, session.history_save_serial);
    let foreign = other.buffer_replacement_review();
    assert!(session.ensure_buffer_replacement_review(&foreign).is_err());
    assert!(
        session
            .buffer_replacement_review_from_token(&foreign.token())
            .is_err()
    );
    let ObjectId::Solid(id) = session
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "brick/test",
            false,
        )
        .unwrap()
    else {
        unreachable!()
    };
    let source = session.document().solid(id).unwrap();
    let side = source
        .sides
        .iter()
        .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
        .unwrap()
        .id;
    let edges = subobject::crease_pairs(source, side).unwrap()[0].edges;
    session
        .save(Actor::User, Some(&directory.path().join("saved.vmf")))
        .unwrap();
    session
        .set_component_picking(
            Some(component_picking::Kind::Face),
            &[id],
            &session.component_stamp(),
        )
        .unwrap();
    session
        .select_components(
            &[Component {
                solid: id,
                sides: vec![side],
            }],
            SelectMode::Replace,
            &session.component_stamp(),
        )
        .unwrap();
    let revision = session.document().revision();
    let review = session.buffer_replacement_review();
    let token = session
        .preview_component_partition(
            &[CreaseEdges { solid: id, edges }],
            &session.component_stamp(),
        )
        .unwrap();
    for replacement in REPLACEMENTS {
        assert!(
            replace(
                &mut session,
                Actor::User,
                replacement,
                &path,
                &recovery_id,
                true,
                &review
            )
            .is_err()
        );
    }
    assert_eq!(session.document().revision(), revision);
    assert_eq!(session.partition_preview_token(), Some(token));
    let review = session.buffer_replacement_review();
    let updated = session
        .update_component_partition_cut(token, [0.25, 0.75])
        .unwrap();
    assert_ne!(token, updated);
    assert!(
        session
            .new_map_reviewed(Actor::User, true, &review)
            .is_err()
    );
    assert_eq!(session.document().revision(), revision);
    assert_eq!(session.partition_preview_token(), Some(updated));
    let review = session.buffer_replacement_review();
    assert!(
        session
            .new_map_reviewed(Actor::User, false, &review)
            .is_err()
    );
    session
        .new_map_reviewed(Actor::User, true, &review)
        .unwrap();
}

#[test]
fn generation_guards_reject_old_choosers_before_save_compile_or_worker_admission() {
    let (directory, game) = crate::test_assets::game();
    let mut session = Session::new(Ok(game));
    let generation = session.document_generation();
    session.new_map(Actor::Agent, true).unwrap();
    let saved = directory.path().join("stale.vmf");
    let compiled = directory.path().join("stale.bsp");
    std::fs::write(&saved, b"keep vmf").unwrap();
    std::fs::write(&compiled, b"keep bsp").unwrap();
    let document = session.document().vmf().clone();
    let history = session.history_snapshot();
    assert!(
        session
            .save_for_generation(Actor::User, Some(&saved), Some(generation))
            .unwrap_err()
            .contains("replaced")
    );
    assert!(
        session
            .begin_compile_for_generation(Actor::User, Some(&compiled), Some(generation))
            .is_err()
    );
    let shared = std::sync::Mutex::new(session).into();
    assert!(
        crate::runtime::start_compile_for_generation(
            &shared,
            Actor::User,
            Some(&compiled),
            false,
            Some(generation)
        )
        .unwrap_err()
        .contains("replaced")
    );
    let session = crate::runtime::lock(&shared);
    assert_eq!(session.document().vmf(), &document);
    assert_eq!(session.history_snapshot(), history);
    assert!(!session.compiling());
    assert!(session.compile_token().is_none());
    assert_eq!(std::fs::read(saved).unwrap(), b"keep vmf");
    assert_eq!(std::fs::read(compiled).unwrap(), b"keep bsp");
}

#[test]
fn generation_guards_allow_same_buffer_edits_for_save_and_compile() {
    let (directory, game) = crate::test_assets::game();
    let mut session = Session::new(Ok(game));
    let generation = session.document_generation();
    session
        .create_block(
            Actor::Agent,
            DVec3::ZERO,
            DVec3::splat(16.0),
            "test/checker",
            false,
        )
        .unwrap();
    let saved = directory.path().join("edited.vmf");
    session
        .save_for_generation(Actor::User, Some(&saved), Some(generation))
        .unwrap();
    let vmf = Vmf::parse(&std::fs::read_to_string(saved).unwrap()).unwrap();
    assert_eq!(vmf.world.solids.len(), 1);
    session
        .create_block(
            Actor::Agent,
            DVec3::splat(32.0),
            DVec3::splat(48.0),
            "test/checker",
            false,
        )
        .unwrap();
    let request = session
        .begin_compile_for_generation(Actor::User, None, Some(generation))
        .unwrap();
    assert!(session.compiling());
    drop(request);
    session.finish_compile(Actor::User, &Err("fixture stopped before compiling".into()));
}

fn stored_files(directory: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut directories = vec![directory.to_owned()];
    while let Some(current) = directories.pop() {
        for entry in std::fs::read_dir(current).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                directories.push(path);
            } else {
                files.insert(path.clone(), std::fs::read(path).unwrap());
            }
        }
    }
    files
}

#[test]
fn drained_document_rejects_retained_file_operations_without_side_effects() {
    let (directory, mut state, path, recovery_id) = fixture();
    state
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(16.0),
            "brick/test",
            false,
        )
        .unwrap();
    let review = state.buffer_replacement_review();
    let generation = state.document_generation();
    let destination = directory.path().join("destination.vmf");
    std::fs::write(&destination, b"preserve destination").unwrap();
    let shared = std::sync::Arc::new(std::sync::Mutex::new(state));
    let document = crate::runtime::DocumentHandle::start(shared.clone()).unwrap();
    document.shutdown().unwrap();
    let files = stored_files(directory.path());
    let mut state = crate::runtime::lock(&shared);
    assert_eq!(state.buffer_replacement_review(), review);
    assert_eq!(state.document_generation(), generation);
    assert_eq!(state.recovery_entries().unwrap().entries.len(), 2);
    let source = state.document().vmf().clone();
    let history = state.history_snapshot();
    let recovery = state.recovery_status();
    let recent = state.recent_maps();
    let selection = state.selected_ids();
    let camera = state.camera();
    let serials = (
        state.scene_serial(),
        state.view_serial(),
        state.log_serial(),
    );
    for actor in [Actor::User, Actor::Agent] {
        for replacement in REPLACEMENTS {
            let error = replace(
                &mut state,
                actor,
                replacement,
                &path,
                &recovery_id,
                true,
                &review,
            )
            .unwrap_err();
            assert!(error.contains("closing"), "{error}");
        }
        let errors = [
            state.new_map(actor, true).unwrap_err(),
            state.open(actor, &path, true).unwrap_err(),
            state
                .open(actor, &directory.path().join("missing.vmf"), true)
                .unwrap_err(),
            state.open_recent_map(actor, &path, true).unwrap_err(),
            state
                .restore_recovery_snapshot(actor, &recovery_id, true, None)
                .unwrap_err(),
            state.save(actor, None).unwrap_err(),
            state.save(actor, Some(&destination)).unwrap_err(),
            state
                .save_for_generation(actor, Some(&destination), Some(generation))
                .unwrap_err(),
        ];
        for error in errors {
            assert!(error.contains("closing"), "{error}");
        }
    }
    assert_eq!(state.document().vmf(), &source);
    assert_eq!(state.history_snapshot(), history);
    assert_eq!(state.path(), Some(path.as_path()));
    assert_eq!(state.recovery_status(), recovery);
    assert_eq!(state.recent_maps(), recent);
    assert_eq!(state.selected_ids(), selection);
    assert_eq!(state.camera(), camera);
    assert_eq!(
        (
            state.scene_serial(),
            state.view_serial(),
            state.log_serial()
        ),
        serials
    );
    assert_eq!(stored_files(directory.path()), files);
    drop(state);
    document.shutdown().unwrap();
    assert_eq!(stored_files(directory.path()), files);

    let mut peer = Session::new(Err("no assets needed".into()));
    peer.open(Actor::Agent, &path, false).unwrap();
    peer.create_block(
        Actor::User,
        DVec3::ZERO,
        DVec3::splat(32.0),
        "brick/test",
        false,
    )
    .unwrap();
    let peer_path = directory.path().join("peer.vmf");
    peer.save(Actor::Agent, Some(&peer_path)).unwrap();
    let review = peer.buffer_replacement_review();
    peer.new_map_reviewed(Actor::User, false, &review).unwrap();
    assert!(peer.document().objects().next().is_none());
}
