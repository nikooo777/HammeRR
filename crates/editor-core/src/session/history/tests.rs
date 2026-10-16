use super::*;
use source_formats::vmf::Entity;

fn sky(session: &mut Session, actor: Actor, name: &str) {
    session
        .edit(actor, name, |tx| {
            tx.world_properties_mut().set("skyname", name);
            Ok(())
        })
        .unwrap();
}

fn report(session: &Session) -> Report {
    session.inspect_history(0, PAGE_SIZE).unwrap()
}

#[test]
fn history_inspection_is_bounded_truthful_and_preserves_original_authors() {
    let mut session = Session::new(Err("no assets".into()));
    let initial = report(&session);
    assert!(initial.entries.is_empty());
    assert_eq!(initial.current_state, INITIAL_STATE);
    assert_eq!(initial.navigation_blocked, None);
    session
        .edit(Actor::User, "Create targets", |tx| {
            for _ in 0..MAX_OBJECT_SAMPLES + 4 {
                let id = tx.new_object_id()?;
                tx.add_entity(Entity::new(id, "info_target"))?;
            }
            Ok(())
        })
        .unwrap();
    let creation = report(&session).entries.remove(0);
    assert_eq!(creation.actor, "you");
    assert_eq!(creation.added, MAX_OBJECT_SAMPLES + 4);
    assert_eq!(creation.modified, 0);
    assert_eq!(creation.objects.len(), MAX_OBJECT_SAMPLES);
    assert_eq!(creation.objects_total, MAX_OBJECT_SAMPLES + 4);
    assert!(creation.objects.iter().all(|object| object.kind == "added"));
    let id = session.document.vmf().entities[0].id;
    session
        .edit(Actor::Agent, "Modify and remove", |tx| {
            tx.entity_mut(id)?.properties.set("targetname", "entry");
            tx.remove(ObjectId::Entity(id + 1))?;
            tx.world_properties_mut().set("skyname", "changed");
            Ok(())
        })
        .unwrap();
    for _ in 0..LOG_LENGTH + 10 {
        session.log(Actor::System, "unrelated activity");
    }
    let before = session.document.vmf().clone();
    let revision = session.document.revision();
    let log = session.log_serial;
    let selected = session.selected_ids();
    let camera = session.camera();
    let page = session.inspect_history(0, 1).unwrap();
    assert_eq!(page.total, 2);
    assert_eq!(page.next_offset, Some(1));
    let change = &page.entries[0];
    assert_eq!((change.added, change.modified, change.removed), (0, 1, 1));
    assert_eq!(change.sections, ["world properties"]);
    assert_eq!(change.actor, "agent");
    assert!(change.current && change.applied);
    let older = session.inspect_history(1, 1).unwrap();
    assert_eq!(older.entries[0].actor, "you");
    assert_eq!(older.entries[0].state, creation.state);
    assert_eq!(older.next_offset, None);
    assert_eq!(session.inspect_history(2, 1).unwrap().entries.len(), 0);
    assert!(session.inspect_history(3, 1).is_err());
    assert!(session.inspect_history(0, 0).is_err());
    assert!(session.inspect_history(0, MAX_PAGE_SIZE + 1).is_err());
    assert_eq!(session.document.vmf(), &before);
    assert_eq!(session.document.revision(), revision);
    assert_eq!(session.log_serial, log);
    assert_eq!(session.selected_ids(), selected);
    assert_eq!(session.camera(), camera);
    session.undo(Actor::User).unwrap();
    let undone = report(&session);
    assert_eq!(undone.entries[0].actor, "agent");
    assert!(!undone.entries[0].applied && !undone.entries[0].current);
    assert_eq!(undone.current_state, creation.state);
    assert_eq!(undone.undo_count, 1);
    assert_eq!(undone.redo_count, 1);
    session.redo(Actor::System).unwrap();
    assert_eq!(report(&session).entries[0].state, change.state);
    assert_eq!(report(&session).entries[0].actor, "agent");
}

#[test]
fn navigation_replays_exact_retained_states_without_losing_redo_or_forging_history() {
    for actor in [Actor::User, Actor::Agent] {
        let mut session = Session::new(Err("no assets".into()));
        let empty = session.document.vmf().clone();
        let id = session
            .edit(Actor::User, "Create marker", |tx| {
                let id = tx.new_object_id()?;
                tx.add_entity(Entity::new(id, "info_target"))?;
                Ok(ObjectId::Entity(id))
            })
            .unwrap();
        let first = report(&session).current_state;
        let first_source = session.document.vmf().clone();
        sky(&mut session, Actor::Agent, "second");
        let second = report(&session).current_state;
        sky(&mut session, Actor::System, "third");
        let latest = report(&session);
        let latest_source = session.document.vmf().clone();
        session.select(&[id], SelectMode::Replace).unwrap();
        let camera = session.camera();
        let plan = session
            .plan_history_navigation(&latest.snapshot, &first)
            .unwrap();
        assert_eq!((plan.direction, plan.steps), ("undo", 2));
        assert_eq!(session.document.vmf(), &latest_source);
        let result = session
            .navigate_history(actor, &latest.snapshot, &first)
            .unwrap();
        assert_eq!(result.current_state, first);
        assert_eq!(session.document.vmf(), &first_source);
        assert!(result.unsaved_changes);
        assert_eq!(report(&session).redo_count, 2);
        assert_eq!(report(&session).entries[0].state, latest.current_state);
        assert_eq!(report(&session).entries[1].state, second);
        assert_eq!(report(&session).entries[0].actor, "system");
        assert_eq!(session.camera(), camera);
        assert!(session.selection().contains(&id));
        let result = session
            .navigate_history(actor, &result.snapshot, &latest.current_state)
            .unwrap();
        assert_eq!((result.plan.direction, result.plan.steps), ("redo", 2));
        assert_eq!(session.document.vmf(), &latest_source);
        let result = session
            .navigate_history(actor, &result.snapshot, INITIAL_STATE)
            .unwrap();
        assert_eq!(result.plan.steps, 3);
        assert_eq!(session.document.vmf(), &empty);
        assert!(session.selection().is_empty());
        assert_eq!(report(&session).redo_count, 3);
        let result = session
            .navigate_history(actor, &result.snapshot, &latest.current_state)
            .unwrap();
        assert_eq!(session.document.vmf(), &latest_source);
        assert_eq!(report(&session).total, 3);
        let revision = session.document.revision();
        let serial = session.scene_serial;
        let log = session.log_serial;
        let no_op = session
            .navigate_history(actor, &result.snapshot, &latest.current_state)
            .unwrap();
        assert_eq!((no_op.plan.direction, no_op.plan.steps), ("none", 0));
        assert_eq!(session.document.revision(), revision);
        assert_eq!(session.scene_serial, serial);
        assert_eq!(session.log_serial, log);
    }
}

#[test]
fn navigation_rejects_stale_branch_document_job_and_preview_targets_atomically() {
    let mut session = Session::new(Err("no assets".into()));
    sky(&mut session, Actor::User, "first");
    let first = report(&session);
    sky(&mut session, Actor::Agent, "discarded");
    let discarded = report(&session).current_state;
    let before = session.document.vmf().clone();
    assert!(
        session
            .navigate_history(Actor::Agent, &first.snapshot, INITIAL_STATE)
            .is_err()
    );
    assert_eq!(session.document.vmf(), &before);
    session.undo(Actor::User).unwrap();
    sky(&mut session, Actor::User, "replacement");
    let replacement = report(&session);
    assert_ne!(replacement.current_state, discarded);
    assert!(
        !session
            .history_actors
            .contains_key(&discarded.parse::<u64>().unwrap())
    );
    assert!(
        session
            .navigate_history(Actor::Agent, &replacement.snapshot, &discarded)
            .is_err()
    );
    assert!(
        session
            .navigate_history(Actor::Agent, &replacement.snapshot, "unknown")
            .is_err()
    );
    let before = session.document.vmf().clone();
    let revision = session.document.revision();
    for busy in 0..3 {
        session.compiling = busy == 0;
        session.launching = busy == 1;
        session.closing = busy == 2;
        assert!(report(&session).navigation_blocked.is_some());
        assert!(
            session
                .navigate_history(Actor::User, &replacement.snapshot, INITIAL_STATE)
                .is_err()
        );
        assert_eq!(session.document.vmf(), &before);
        assert_eq!(session.document.revision(), revision);
    }
    session.compiling = false;
    session.launching = false;
    session.closing = false;
    session.block_preview = Some((DVec3::ZERO, DVec3::splat(64.0)));
    assert!(report(&session).navigation_blocked.is_some());
    assert!(
        session
            .navigate_history(Actor::User, &replacement.snapshot, INITIAL_STATE)
            .is_err()
    );
    assert!(session.block_preview.is_some());
    assert_eq!(session.document.vmf(), &before);
    session.cancel_block_preview();
    let snapshot_before_save = session.history_snapshot();
    session.document.mark_saved();
    assert!(
        session
            .navigate_history(Actor::User, &snapshot_before_save, INITIAL_STATE)
            .is_err()
    );
    let saved = report(&session);
    let no_op = session
        .navigate_history(Actor::User, &saved.snapshot, &saved.current_state)
        .unwrap();
    assert!(!no_op.unsaved_changes);
    let result = session
        .navigate_history(Actor::User, &saved.snapshot, INITIAL_STATE)
        .unwrap();
    assert!(result.unsaved_changes);
    session.new_map(Actor::User, true).unwrap();
    sky(&mut session, Actor::User, "new document");
    assert!(
        session
            .navigate_history(Actor::User, &saved.snapshot, &first.current_state)
            .is_err()
    );
}

#[test]
fn history_bounds_navigation_and_labels_without_recording_noop_or_failed_edits() {
    let mut session = Session::new(Err("no assets".into()));
    for index in 0..MAX_NAVIGATION_STEPS + 1 {
        sky(&mut session, Actor::User, &format!("sky_{index}"));
    }
    let initial = report(&session);
    let source = session.document.vmf().clone();
    assert!(
        session
            .navigate_history(Actor::User, &initial.snapshot, INITIAL_STATE)
            .is_err()
    );
    assert_eq!(session.document.vmf(), &source);
    let label = "雪".repeat(MAX_LABEL_CHARS + 1);
    sky(&mut session, Actor::Agent, &label);
    let page = session.inspect_history(0, MAX_PAGE_SIZE).unwrap();
    assert_eq!(page.entries.len(), MAX_PAGE_SIZE);
    assert_eq!(page.entries[0].label.chars().count(), MAX_LABEL_CHARS);
    assert!(page.entries[0].label_truncated);
    session.edit(Actor::User, "noop", |_| Ok(())).unwrap();
    let failed: Result<(), String> = session.edit(Actor::Agent, "failed", |tx| {
        tx.world_properties_mut().set("skyname", "rollback");
        Err(EditError::Invalid("reject".into()))
    });
    assert!(failed.is_err());
    assert_eq!(session.history_snapshot(), page.snapshot);
    assert_eq!(report(&session).total, MAX_NAVIGATION_STEPS + 2);
    let old = session.inspect_history(MAX_NAVIGATION_STEPS, 2).unwrap();
    let result = session
        .navigate_history(Actor::User, &page.snapshot, &old.entries[0].state)
        .unwrap();
    assert_eq!(result.plan.steps, MAX_NAVIGATION_STEPS);
    let current = report(&session);
    session
        .navigate_history(Actor::User, &current.snapshot, INITIAL_STATE)
        .unwrap();
    assert_eq!(report(&session).undo_count, 0);
}

#[test]
fn active_geometry_drafts_survive_rejected_navigation_and_remain_usable() {
    let mut session = Session::new(Err("no assets".into()));
    sky(&mut session, Actor::User, "first");
    let initial = report(&session);
    let source = session.document.vmf().clone();
    session
        .create_primitive(Actor::Agent, map_core::primitive::Spec::default(), true)
        .unwrap();
    let token = session.primitive_preview().unwrap().token;
    let draft = session.render_document().vmf().clone();
    assert!(
        session
            .navigate_history(Actor::User, &initial.snapshot, INITIAL_STATE)
            .is_err()
    );
    assert_eq!(session.render_document().vmf(), &draft);
    assert_eq!(session.document.vmf(), &source);
    assert_eq!(session.primitive_preview().unwrap().token, token);
    session
        .commit_primitive_preview(Actor::Agent, token)
        .unwrap();
    let committed = report(&session);
    assert_eq!(committed.total, 2);
    assert_eq!(committed.entries[0].actor, "agent");
    session
        .navigate_history(Actor::User, &committed.snapshot, &initial.current_state)
        .unwrap();
    assert_eq!(session.document.vmf(), &source);
    assert_eq!(report(&session).redo_count, 1);
}

#[test]
fn saves_invalidate_history_targets_without_resetting_transactions_or_saved_file_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("history.vmf");
    let mut session = Session::new(Err("no assets".into()));
    sky(&mut session, Actor::User, "first");
    sky(&mut session, Actor::Agent, "second");
    let edited = report(&session);
    session.save(Actor::User, Some(&path)).unwrap();
    let saved = report(&session);
    assert!(!saved.unsaved_changes);
    assert_eq!(saved.total, edited.total);
    assert_ne!(saved.snapshot, edited.snapshot);
    assert!(
        session
            .navigate_history(Actor::Agent, &edited.snapshot, INITIAL_STATE)
            .is_err()
    );
    session.save(Actor::User, None).unwrap();
    let saved_again = report(&session);
    assert!(!saved_again.unsaved_changes);
    assert_ne!(saved_again.snapshot, saved.snapshot);
    assert_eq!(saved_again.current_state, saved.current_state);
    assert!(
        session
            .navigate_history(Actor::Agent, &saved.snapshot, INITIAL_STATE)
            .is_err()
    );
    let bytes = std::fs::read(&path).unwrap();
    let version = session
        .document
        .vmf()
        .world
        .properties
        .get("mapversion")
        .unwrap()
        .to_owned();
    let first = saved_again.entries[1].state.clone();
    let undone = session
        .navigate_history(Actor::Agent, &saved_again.snapshot, &first)
        .unwrap();
    assert!(undone.unsaved_changes);
    assert_eq!(
        session.document.vmf().world.properties.get("skyname"),
        Some("first")
    );
    assert_eq!(
        session.document.vmf().world.properties.get("mapversion"),
        Some(version.as_str())
    );
    session
        .navigate_history(Actor::User, &undone.snapshot, &saved_again.current_state)
        .unwrap();
    assert!(session.document.is_dirty());
    assert_eq!(
        session.document.vmf().world.properties.get("skyname"),
        Some("second")
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert_eq!(report(&session).entries[0].actor, "agent");
}

fn assert_history_draft_guard(session: &mut Session, target: &str) {
    let history = report(session);
    let source = session.document.vmf().clone();
    let entity_placement = session.get_entity_placement();
    let measurement = session.measurement_state();
    let scene = session.scene_serial;
    let log = session.log_serial;
    let selection = session.selected_ids();
    let camera = session.camera();
    assert!(history.navigation_blocked.is_some());
    for actor in [Actor::User, Actor::Agent] {
        assert!(
            session
                .plan_history_navigation(&history.snapshot, target)
                .is_err()
        );
        assert!(
            session
                .navigate_history(actor, &history.snapshot, target)
                .is_err()
        );
        let current = session
            .navigate_history(actor, &history.snapshot, &history.current_state)
            .unwrap();
        assert_eq!((current.plan.direction, current.plan.steps), ("none", 0));
    }
    assert_eq!(session.document.vmf(), &source);
    assert_eq!(session.history_snapshot(), history.snapshot);
    assert_eq!(session.scene_serial, scene);
    assert_eq!(session.log_serial, log);
    assert_eq!(session.selected_ids(), selection);
    assert_eq!(session.camera(), camera);
    assert_eq!(session.get_entity_placement(), entity_placement);
    assert_eq!(session.measurement_state(), measurement);
}

#[test]
fn history_preserves_armed_entity_placement_and_measurement_drafts_until_explicit_cancellation() {
    for owner in [Actor::User, Actor::Agent] {
        let other = if owner == Actor::User {
            Actor::Agent
        } else {
            Actor::User
        };
        let mut session = Session::new(Err("no assets".into()));
        session
            .create_block(
                Actor::User,
                DVec3::new(-64.0, -64.0, -16.0),
                DVec3::new(64.0, 64.0, 0.0),
                "test",
                false,
            )
            .unwrap();
        sky(&mut session, Actor::User, "first");
        let target = report(&session).current_state;
        sky(&mut session, Actor::Agent, "second");
        let history = report(&session);
        let request = entity_placement::Request {
            classname: "info_target".into(),
            properties: BTreeMap::new(),
            offset: 0.0,
            align: false,
            snap: false,
        };
        let placement = session
            .begin_entity_placement(owner, request.clone(), &session.entity_placement_stamp())
            .unwrap();
        let placement_token = placement["draft"]["token"].as_u64().unwrap();
        assert_eq!(placement["draft"]["picking"], true);
        assert!(std::ptr::eq(session.render_document(), session.document()));
        assert_history_draft_guard(&mut session, &target);
        let picked = session
            .pick_entity_placement(
                owner,
                placement_token,
                query::Ray {
                    origin: DVec3::new(0.0, 0.0, 64.0),
                    direction: DVec3::NEG_Z,
                },
            )
            .unwrap();
        assert!(!picked["draft"]["plan"].is_null());
        assert_history_draft_guard(&mut session, &target);
        session
            .cancel_entity_placement(owner, placement_token)
            .unwrap();
        assert!(session.history_navigation_blocked().is_none());

        let measurement = session
            .preview_measurement(
                owner,
                measurements::Request {
                    id: None,
                    name: "Uncommitted clearance".into(),
                    start: DVec3::ZERO,
                    end: DVec3::new(32.0, 64.0, 72.0),
                },
                &session.measurement_stamp(),
            )
            .unwrap();
        let measurement_token = measurement["draft"]["token"].as_u64().unwrap();
        assert_eq!(measurement["draft"]["valid"], true);
        assert!(std::ptr::eq(session.render_document(), session.document()));
        assert_history_draft_guard(&mut session, &target);
        let placement = session
            .begin_entity_placement(other, request, &session.entity_placement_stamp())
            .unwrap();
        let placement_token = placement["draft"]["token"].as_u64().unwrap();
        assert_eq!(placement["draft"]["picking"], true);
        assert_eq!(session.measurement_state()["draft"]["valid"], true);
        assert_history_draft_guard(&mut session, &target);
        session
            .cancel_entity_placement(other, placement_token)
            .unwrap();
        assert_history_draft_guard(&mut session, &target);
        assert_eq!(session.measurement_state()["draft"]["valid"], true);
        session
            .cancel_measurement(owner, measurement_token)
            .unwrap();
        assert!(session.history_navigation_blocked().is_none());
        let result = session
            .navigate_history(other, &history.snapshot, &target)
            .unwrap();
        assert_eq!((result.plan.direction, result.plan.steps), ("undo", 1));
        assert_eq!(
            session.document.vmf().world.properties.get("skyname"),
            Some("first")
        );
        assert_eq!(report(&session).redo_count, 1);
        session
            .navigate_history(owner, &result.snapshot, &history.current_state)
            .unwrap();
        assert_eq!(
            session.document.vmf().world.properties.get("skyname"),
            Some("second")
        );
        assert_eq!(report(&session).total, history.total);
        assert!(session.get_entity_placement()["draft"].is_null());
        assert!(session.measurement_state()["draft"].is_null());
    }
}
