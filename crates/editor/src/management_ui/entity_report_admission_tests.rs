use super::*;
use crate::runtime::{DocumentHandle, DocumentLifecycle};
use crate::{ConnectionWindow, RenameWindow, connection_ui, rename_ui};
use glam::DVec3;
use map_core::ObjectId;
use serde_json::{Value, json};
use slint::Model;
use source_formats::fgd::Catalog;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(5);

struct Fixture {
    document: DocumentHandle,
    access: DocumentAccess,
    shared: SharedSession,
    report: ReportWindow,
    connection: ConnectionWindow,
    rename: RenameWindow,
    source: u32,
    target: u32,
    member: u32,
    directory: tempfile::TempDir,
}

fn entity(session: &mut Session, name: &str, position: DVec3) -> u32 {
    let ObjectId::Entity(id) = session
        .create_entity(
            Actor::User,
            "logic_relay",
            Some(position),
            BTreeMap::from([("targetname".into(), name.into())]),
        )
        .unwrap()
    else {
        panic!()
    };
    id
}

fn fixture() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session.configure_recovery(Ok(directory.path().join("recovery")));
    session.set_definitions(
        Actor::User,
        Catalog::parse(
            r#"
@PointClass = logic_relay [
    targetname(target_source) : "Name"
    target(target_destination) : "Target"
    input Trigger(void) : "Trigger the relay"
    output OnTrigger(void) : "Fires when triggered"
]
"#,
        )
        .unwrap(),
    );
    let target = entity(&mut session, "relay", DVec3::ZERO);
    let member = entity(&mut session, "relay", DVec3::X * 64.0);
    let source = entity(&mut session, "source", DVec3::Y * 128.0);
    session
        .set_entity_properties(
            Actor::User,
            source,
            &[
                ("target".into(), Some("relay".into())),
                ("notes".into(), Some("relay".into())),
            ],
        )
        .unwrap();
    session
        .set_entity_connection(
            Actor::User,
            source,
            None,
            &map_core::connections::Spec {
                output: "OnTrigger".into(),
                target: "relay*".into(),
                input: "Trigger".into(),
                parameter: String::new(),
                delay: 0.25,
                times: -1,
            },
        )
        .unwrap();
    session.begin_entity_rename(target).unwrap();
    let shared = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    let report = ReportWindow::new().unwrap();
    bind_report_with_access(&report, &access);
    report.set_aspect(1.5);
    refresh_report(&report, &lock(&shared));
    let connection = ConnectionWindow::new().unwrap();
    connection_ui::bind_with_access(&connection, &access);
    connection.set_source_id(ObjectId::Entity(source).to_string().into());
    connection_ui::refresh(&connection, &lock(&shared));
    let rename = RenameWindow::new().unwrap();
    rename_ui::bind_with_access(&rename, &access);
    rename_ui::refresh(&rename, &lock(&shared));
    Fixture {
        document,
        access,
        shared,
        report,
        connection,
        rename,
        source,
        target,
        member,
        directory,
    }
}

fn state(shared: &SharedSession) -> Value {
    let session = lock(shared);
    json!({
        "map": session.document().vmf().write(&Default::default()),
        "generation": session.document_generation(),
        "revision": session.document().revision(),
        "history": session.history_snapshot(),
        "selection": session.selected_ids().iter().map(ToString::to_string).collect::<Vec<_>>(),
        "camera": format!("{:?}", session.camera()),
        "view": session.view_serial(),
        "scene": session.scene_serial(),
        "log": session.log_serial(),
        "management": session.management_serial(),
        "connection_serial": session.connection_serial(),
        "connection": session.connection_draft(),
        "rename_serial": session.rename_serial(),
        "rename": session.rename_draft(),
    })
}

fn forms(f: &Fixture) -> Value {
    let report = &f.report;
    let connection = &f.connection;
    let rename = &f.rename;
    json!({
        "report": {
            "query": report.get_query().to_string(),
            "class": report.get_classname().to_string(),
            "target": report.get_targetname().to_string(),
            "material": report.get_material().to_string(),
            "kind": report.get_kind(),
            "visibility": report.get_visibility(),
            "page": report.get_page(),
            "selection_mode": report.get_selection_mode(),
            "bounds": report.get_limit_bounds(),
            "min": [report.get_min_x().to_string(), report.get_min_y().to_string(), report.get_min_z().to_string()],
            "max": [report.get_max_x().to_string(), report.get_max_y().to_string(), report.get_max_z().to_string()],
            "generation": report.get_generation().to_string(),
            "revision": report.get_revision().to_string(),
            "summary": report.get_summary().to_string(),
            "next": report.get_has_next(),
            "rows": report.get_rows().iter().map(|row| (row.id.to_string(), row.label.to_string(), row.detail.to_string())).collect::<Vec<_>>(),
        },
        "connection": {
            "source": connection.get_source_id().to_string(),
            "label": connection.get_source_label().to_string(),
            "generation": connection.get_generation().to_string(),
            "revision": connection.get_revision().to_string(),
            "token": connection.get_draft_token().to_string(),
            "output": connection.get_output().to_string(),
            "target": connection.get_target().to_string(),
            "input": connection.get_input().to_string(),
            "parameter": connection.get_parameter().to_string(),
            "delay": connection.get_delay().to_string(),
            "times": connection.get_times().to_string(),
            "filter": connection.get_target_filter().to_string(),
            "editing": connection.get_editing(),
            "apply": connection.get_can_apply(),
            "warnings": connection.get_warnings().to_string(),
            "outputs": connection.get_outputs().iter().map(|value| value.to_string()).collect::<Vec<_>>(),
            "inputs": connection.get_inputs().iter().map(|value| value.to_string()).collect::<Vec<_>>(),
            "targets": connection.get_targets().iter().map(|value| value.to_string()).collect::<Vec<_>>(),
            "outgoing": connection.get_outgoing().iter().map(|row| (row.source.to_string(), row.index, row.label.to_string(), row.detail.to_string(), row.warning.to_string())).collect::<Vec<_>>(),
            "incoming": connection.get_incoming().iter().map(|row| (row.source.to_string(), row.index, row.label.to_string(), row.detail.to_string(), row.warning.to_string())).collect::<Vec<_>>(),
        },
        "rename": {
            "token": rename.get_draft_token().to_string(),
            "entity": rename.get_entity_label().to_string(),
            "old": rename.get_old_name().to_string(),
            "new": rename.get_new_name().to_string(),
            "group": rename.get_rename_group(),
            "accept": rename.get_accept_review(),
            "details": rename.get_show_details(),
            "review": rename.get_requires_review(),
            "apply": rename.get_can_apply(),
            "proposal": rename.get_has_proposal(),
            "shared": rename.get_shared_count(),
            "members": rename.get_members().to_string(),
            "summary": rename.get_summary().to_string(),
            "rows": rename.get_references().iter().map(|row| (row.section.to_string(), row.label.to_string(), row.change.to_string(), row.reason.to_string(), row.review, row.changed, row.details.to_string())).collect::<Vec<_>>(),
        },
    })
}

fn pending(f: &Fixture) {
    f.connection.invoke_add();
    f.connection.set_target("!self".into());
    f.connection.set_input("Trigger".into());
    f.connection.set_delay("0.5".into());
    f.connection.set_times("1".into());
    f.connection.invoke_fields_changed();
    f.rename.set_new_name("pending_relay".into());
    f.rename.set_rename_group(true);
    f.rename.set_show_details(true);
    f.rename.invoke_fields_changed();
    f.rename.set_accept_review(true);
    f.rename.invoke_fields_changed();
    f.report.set_query("unsearched pending text".into());
    f.report.set_limit_bounds(true);
    f.report.set_min_x("unparsed".into());
}

fn reject_actions(f: &Fixture, expected_forms: &Value, error: &str) {
    let expected = state(&f.shared);
    macro_rules! rejected {
        ($action:expr, $message:expr) => {{
            $action;
            assert_eq!($message, error);
            assert_eq!(state(&f.shared), expected);
            assert_eq!(forms(f), *expected_forms);
        }};
    }
    rejected!(f.report.invoke_search(), f.report.get_message());
    rejected!(
        f.report
            .invoke_select_result(ObjectId::Entity(f.target).to_string().into(), true),
        f.report.get_message()
    );
    rejected!(f.report.invoke_select_all(true), f.report.get_message());
    rejected!(
        f.report.invoke_visibility_action("hide".into()),
        f.report.get_message()
    );
    rejected!(f.connection.invoke_add(), f.connection.get_message());
    rejected!(
        f.connection
            .invoke_choose(ObjectId::Entity(f.source).to_string().into(), 0),
        f.connection.get_message()
    );
    rejected!(
        f.connection.invoke_delete_connection(0),
        f.connection.get_message()
    );
    rejected!(
        f.connection.invoke_fields_changed(),
        f.connection.get_message()
    );
    rejected!(
        f.connection.invoke_filter_changed(),
        f.connection.get_message()
    );
    rejected!(f.connection.invoke_apply(), f.connection.get_message());
    rejected!(f.rename.invoke_fields_changed(), f.rename.get_message());
    rejected!(f.rename.invoke_apply(), f.rename.get_message());
    f.connection.show().unwrap();
    rejected!(f.connection.invoke_cancel(), f.connection.get_message());
    assert!(!f.connection.window().is_visible());
    f.connection.show().unwrap();
    rejected!(
        f.connection
            .window()
            .dispatch_event(slint::platform::WindowEvent::CloseRequested),
        f.connection.get_message()
    );
    assert!(!f.connection.window().is_visible());
    f.rename.show().unwrap();
    rejected!(f.rename.invoke_cancel(), f.rename.get_message());
    assert!(!f.rename.window().is_visible());
    f.rename.show().unwrap();
    rejected!(
        f.rename
            .window()
            .dispatch_event(slint::platform::WindowEvent::CloseRequested),
        f.rename.get_message()
    );
    assert!(!f.rename.window().is_visible());
    f.report.show().unwrap();
    f.report.invoke_cancel();
    assert!(!f.report.window().is_visible());
    assert_eq!(state(&f.shared), expected);
    assert_eq!(forms(f), *expected_forms);
}

#[test]
fn entity_reports_connections_and_rename_share_live_operations_and_exact_reviews() {
    i_slint_backend_testing::init_no_event_loop();
    let f = fixture();
    let original = lock(&f.shared).document().vmf().clone();
    f.report.set_targetname("source".into());
    f.report.invoke_search();
    assert_eq!(f.report.get_rows().row_count(), 1);
    f.report
        .invoke_select_result(ObjectId::Entity(f.source).to_string().into(), true);
    assert_eq!(
        lock(&f.shared).selected_ids(),
        vec![ObjectId::Entity(f.source)]
    );
    f.report.invoke_visibility_action("hide".into());
    assert!(lock(&f.shared).document().entity(f.source).unwrap().hidden);
    f.report.invoke_visibility_action("show_all".into());
    assert!(!lock(&f.shared).document().entity(f.source).unwrap().hidden);
    f.report.set_targetname("relay".into());
    f.report.invoke_select_all(false);
    assert_eq!(lock(&f.shared).selected_ids().len(), 2);
    f.report.set_selection_mode(1);
    f.report.set_targetname("source".into());
    f.report.invoke_select_all(true);
    assert_eq!(lock(&f.shared).selected_ids().len(), 3);
    f.report.set_selection_mode(2);
    f.report.invoke_select_all(false);
    assert_eq!(lock(&f.shared).selected_ids().len(), 2);
    f.report.set_selection_mode(3);
    f.report.invoke_select_all(false);
    assert_eq!(lock(&f.shared).selected_ids().len(), 3);

    connection_ui::refresh(&f.connection, &lock(&f.shared));
    f.connection.invoke_add();
    f.connection.set_target("!self".into());
    f.connection.set_input("Trigger".into());
    f.connection.set_delay("0.5".into());
    f.connection.set_times("1".into());
    f.connection.invoke_fields_changed();
    assert!(f.connection.get_can_apply());
    f.connection.set_target_filter("source".into());
    f.connection.invoke_filter_changed();
    assert_eq!(f.connection.get_targets().row_count(), 2);
    f.connection.invoke_apply();
    assert_eq!(
        lock(&f.shared).connection_records(f.source).unwrap().len(),
        2
    );
    f.connection
        .invoke_choose(ObjectId::Entity(f.source).to_string().into(), 1);
    assert_eq!(lock(&f.shared).connection_draft().unwrap().index, Some(1));
    f.connection.set_delay("0.75".into());
    f.connection.invoke_fields_changed();
    f.connection.invoke_apply();
    assert_eq!(
        lock(&f.shared).connection_records(f.source).unwrap()[1]
            .spec
            .as_ref()
            .unwrap()
            .delay,
        0.75
    );
    f.connection.invoke_delete_connection(1);
    assert_eq!(
        lock(&f.shared).connection_records(f.source).unwrap().len(),
        1
    );
    f.connection.invoke_add();
    f.connection.invoke_cancel();
    assert!(lock(&f.shared).connection_draft().is_none());
    for _ in 0..3 {
        lock(&f.shared).undo(Actor::User).unwrap();
    }
    assert_eq!(lock(&f.shared).document().vmf(), &original);

    lock(&f.shared).begin_entity_rename(f.target).unwrap();
    rename_ui::refresh(&f.rename, &lock(&f.shared));
    f.rename.set_new_name("renamed_relay".into());
    f.rename.set_rename_group(true);
    f.rename.set_show_details(true);
    f.rename.invoke_fields_changed();
    assert!(f.rename.get_requires_review());
    assert!(!f.rename.get_can_apply());
    assert!(lock(&f.shared).rename_draft().unwrap().show_details);
    f.rename.invoke_apply();
    assert_eq!(lock(&f.shared).document().vmf(), &original);
    f.rename.set_accept_review(true);
    f.rename.invoke_fields_changed();
    assert!(f.rename.get_can_apply());
    f.rename.invoke_apply();
    assert!(lock(&f.shared).rename_draft().is_none());
    for id in [f.target, f.member] {
        assert_eq!(
            lock(&f.shared)
                .document()
                .entity(id)
                .unwrap()
                .properties
                .get("targetname"),
            Some("renamed_relay")
        );
    }
    assert_eq!(
        lock(&f.shared)
            .document()
            .entity(f.source)
            .unwrap()
            .properties
            .get("target"),
        Some("renamed_relay")
    );
    assert_eq!(
        lock(&f.shared)
            .document()
            .entity(f.source)
            .unwrap()
            .properties
            .get("notes"),
        Some("relay")
    );
    lock(&f.shared).undo(Actor::User).unwrap();
    assert_eq!(lock(&f.shared).document().vmf(), &original);
    lock(&f.shared).begin_entity_rename(f.target).unwrap();
    rename_ui::refresh(&f.rename, &lock(&f.shared));
    f.rename.invoke_cancel();
    assert!(lock(&f.shared).rename_draft().is_none());
}

#[test]
fn entity_report_actions_reject_real_drain_and_finish_without_touching_pending_forms_or_peer() {
    i_slint_backend_testing::init_no_event_loop();
    let f = fixture();
    pending(&f);
    let peer = fixture();
    pending(&peer);
    let peer_state = state(&peer.shared);
    let peer_forms = forms(&peer);
    let expected = forms(&f);
    let gate = f.document.admit().unwrap();
    let draining = f.document.clone();
    let (send, receive) = mpsc::sync_channel(1);
    let thread = std::thread::spawn(move || send.send(draining.shutdown()).unwrap());
    let deadline = Instant::now() + TIMEOUT;
    while f.document.lifecycle() == DocumentLifecycle::Live
        || lock(&f.shared).compile_settings()["editable"] != false
    {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(receive.try_recv().is_err());
    reject_actions(&f, &expected, "the editor is closing");
    assert_eq!(state(&peer.shared), peer_state);
    assert_eq!(forms(&peer), peer_forms);
    let final_map = lock(&f.shared).document().vmf().clone();
    drop(gate);
    receive.recv_timeout(TIMEOUT).unwrap().unwrap();
    thread.join().unwrap();
    assert_eq!(f.document.lifecycle(), DocumentLifecycle::Finished);
    reject_actions(&f, &expected, "the document is closed");
    let store = crate::recovery::Store {
        directory: f.directory.path().join("recovery"),
    };
    let entries = store.list().unwrap().entries;
    assert_eq!(entries.len(), 1);
    assert_eq!(
        source_formats::vmf::Vmf::parse(&store.read(&entries[0].id).unwrap().1).unwrap(),
        final_map
    );
    assert_eq!(state(&peer.shared), peer_state);
    assert_eq!(forms(&peer), peer_forms);
    peer.connection.invoke_apply();
    assert_eq!(
        lock(&peer.shared)
            .connection_records(peer.source)
            .unwrap()
            .len(),
        2
    );
    assert_eq!(peer.document.lifecycle(), DocumentLifecycle::Live);
}

#[test]
fn admitted_report_action_finishes_borrowed_refresh_before_final_recovery() {
    i_slint_backend_testing::init_no_event_loop();
    let f = fixture();
    let (send, receive) = mpsc::sync_channel(1);
    let thread = with_report(
        &f.report.as_weak(),
        &f.access.downgrade(),
        |window, session| {
            let draining = f.document.clone();
            let thread = std::thread::spawn(move || send.send(draining.shutdown()).unwrap());
            let deadline = Instant::now() + TIMEOUT;
            while f.document.lifecycle() == DocumentLifecycle::Live {
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
            session
                .set_visibility(Actor::User, "hide", &[ObjectId::Entity(f.source)])
                .unwrap();
            refresh_report(window, session);
            connection_ui::refresh(&f.connection, session);
            rename_ui::refresh(&f.rename, session);
            assert!(
                window
                    .get_rows()
                    .iter()
                    .any(|row| row.label.contains("Hidden"))
            );
            assert!(receive.try_recv().is_err());
            thread
        },
    )
    .unwrap();
    receive.recv_timeout(TIMEOUT).unwrap().unwrap();
    thread.join().unwrap();
    assert_eq!(f.document.lifecycle(), DocumentLifecycle::Finished);
    let store = crate::recovery::Store {
        directory: f.directory.path().join("recovery"),
    };
    let entries = store.list().unwrap().entries;
    assert_eq!(entries.len(), 1);
    let recovered =
        source_formats::vmf::Vmf::parse(&store.read(&entries[0].id).unwrap().1).unwrap();
    assert!(
        recovered
            .entities
            .iter()
            .find(|entity| entity.id == f.source)
            .unwrap()
            .hidden
    );
    assert_eq!(recovered, *lock(&f.shared).document().vmf());
}

#[test]
fn stale_report_rows_snapshots_and_draft_tokens_preserve_exact_targets_and_foreign_cleanup() {
    i_slint_backend_testing::init_no_event_loop();
    let f = fixture();
    pending(&f);
    let old_connection = f.connection.get_draft_token();
    let old_rename = f.rename.get_draft_token();
    {
        let mut session = lock(&f.shared);
        session.begin_connection_edit(f.target, None).unwrap();
        session.begin_entity_rename(f.source).unwrap();
    }
    let foreign = state(&f.shared);
    f.connection.invoke_cancel();
    f.rename.invoke_cancel();
    assert_eq!(state(&f.shared), foreign);
    for window in [f.connection.window(), f.rename.window()] {
        window.dispatch_event(slint::platform::WindowEvent::CloseRequested);
    }
    assert_eq!(state(&f.shared), foreign);
    f.connection.invoke_fields_changed();
    assert!(f.connection.get_message().contains("draft changed"));
    assert_eq!(state(&f.shared), foreign);
    f.connection.set_draft_token(old_connection);
    f.connection.invoke_apply();
    assert!(f.connection.get_message().contains("draft changed"));
    assert_eq!(state(&f.shared), foreign);
    f.rename.invoke_fields_changed();
    assert!(f.rename.get_message().contains("draft changed"));
    assert_eq!(state(&f.shared), foreign);
    f.rename.set_draft_token(old_rename);
    f.rename.invoke_apply();
    assert!(f.rename.get_message().contains("draft changed"));
    assert_eq!(state(&f.shared), foreign);

    let generation = f.connection.get_generation();
    let revision = f.connection.get_revision();
    lock(&f.shared)
        .set_entity_properties(
            Actor::Agent,
            f.source,
            &[("custom".into(), Some("changed".into()))],
        )
        .unwrap();
    lock(&f.shared).undo(Actor::Agent).unwrap();
    let unchanged = state(&f.shared);
    f.report
        .invoke_select_result(ObjectId::Entity(f.target).to_string().into(), true);
    assert!(f.report.get_message().contains("Map changed"));
    assert_eq!(state(&f.shared), unchanged);
    f.connection.invoke_add();
    assert!(f.connection.get_message().contains("Map changed"));
    assert_eq!(state(&f.shared), unchanged);
    f.connection.set_generation(generation.clone());
    f.connection.set_revision(revision.clone());
    f.connection
        .invoke_choose(ObjectId::Entity(f.source).to_string().into(), 0);
    assert!(f.connection.get_message().contains("Map changed"));
    assert_eq!(state(&f.shared), unchanged);
    f.connection.set_generation(generation);
    f.connection.set_revision(revision);
    f.connection.invoke_delete_connection(0);
    assert!(f.connection.get_message().contains("Map changed"));
    assert_eq!(state(&f.shared), unchanged);

    lock(&f.shared).new_map(Actor::Agent, true).unwrap();
    let replacement = entity(&mut lock(&f.shared), "replacement", DVec3::ZERO);
    let unchanged = state(&f.shared);
    f.report
        .invoke_select_result(ObjectId::Entity(replacement).to_string().into(), true);
    assert!(f.report.get_message().contains("Map changed"));
    assert_eq!(state(&f.shared), unchanged);
}

#[test]
fn retained_entity_report_windows_release_document_and_vanished_origin_abandons_access() {
    i_slint_backend_testing::init_no_event_loop();
    let f = fixture();
    pending(&f);
    f.report.show().unwrap();
    f.connection.show().unwrap();
    f.rename.show().unwrap();
    let weak_session = Arc::downgrade(&f.shared);
    let continuation = f.access.downgrade();
    let Fixture {
        document,
        access,
        shared,
        report,
        connection,
        rename,
        directory,
        ..
    } = f;
    drop(access);
    drop(document);
    drop(shared);
    assert!(weak_session.upgrade().is_none());
    assert!(continuation.upgrade().is_err());
    report.invoke_search();
    connection.invoke_filter_changed();
    rename.invoke_fields_changed();
    assert_eq!(report.get_message(), "the document is closed");
    assert_eq!(connection.get_message(), "the document is closed");
    assert_eq!(rename.get_message(), "the document is closed");
    report.invoke_cancel();
    connection.invoke_cancel();
    rename.invoke_cancel();
    assert!(!report.window().is_visible());
    assert!(!connection.window().is_visible());
    assert!(!rename.window().is_visible());
    let weak_report = report.as_weak();
    let weak_connection = connection.as_weak();
    let weak_rename = rename.as_weak();
    drop(report);
    drop(connection);
    drop(rename);
    assert!(weak_report.upgrade().is_none());
    assert!(weak_connection.upgrade().is_none());
    assert!(weak_rename.upgrade().is_none());
    drop(directory);

    let f = fixture();
    let weak = f.report.as_weak();
    let access = f.access.downgrade();
    f.report.show().unwrap();
    f.report.hide().unwrap();
    drop(f.report);
    assert!(weak.upgrade().is_none());
    assert!(
        with_report(&weak, &access, |_, _| panic!(
            "vanished report accessed Session"
        ))
        .is_none()
    );
    assert_eq!(f.document.lifecycle(), DocumentLifecycle::Live);
}
