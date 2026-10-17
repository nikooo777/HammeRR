use super::*;
use crate::ContextWindow;
use crate::context_ui;
use crate::runtime::{DocumentHandle, DocumentLifecycle, lock};
use glam::DVec3;
use serde_json::{Value, json};
use slint::Model;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(5);

struct Fixture {
    document: DocumentHandle,
    access: DocumentAccess,
    shared: SharedSession,
    checks: ChecksWindow,
    context: ContextWindow,
    directory: tempfile::TempDir,
}

fn fixture() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("No assets required".into()));
    session
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(64.0),
            "brick/test",
            true,
        )
        .unwrap();
    let mut vmf = session.document().vmf().clone();
    vmf.world.solids[0].sides[0].u_axis.scale = 0.0;
    vmf.world.solids[0].sides[1].lightmap_scale = 0;
    let path = directory.path().join("checks.vmf");
    std::fs::write(&path, vmf.write(&Default::default())).unwrap();
    session.open(Actor::User, &path, true).unwrap();
    session.configure_recovery(Ok(directory.path().join("recovery")));
    let shared = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    let checks = ChecksWindow::new().unwrap();
    bind_with_access(&checks, &access);
    let context = ContextWindow::new().unwrap();
    context_ui::bind_with_access(&context, &access);
    context.invoke_begin("map".into());
    context.set_summary("Courtyard lighting".into());
    context.set_body("Keep the entrance readable.".into());
    context.invoke_edit();
    assert!(context.get_can_apply());
    context.invoke_apply();
    assert_eq!(lock(&shared).context_state()["notes"], 1);
    context.invoke_begin("new_region".into());
    context.set_summary("Entrance region".into());
    context.set_body("Keep props below waist height.".into());
    context.invoke_edit();
    assert!(context.get_can_apply());
    context.invoke_apply();
    context.invoke_begin("region:1".into());
    checks.invoke_run_checks();
    checks.invoke_inspect("0".into());
    assert_eq!(checks.get_detail_title(), "Invalid texture projection");
    checks.invoke_review_repair();
    assert!(!checks.get_preview_token().is_empty());
    Fixture {
        document,
        access,
        shared,
        checks,
        context,
        directory,
    }
}

fn state(shared: &SharedSession) -> Value {
    let session = lock(shared);
    json!({
        "map": session.document().vmf().write(&Default::default()),
        "history": session.history_snapshot(),
        "selection": session.selected_ids().iter().map(ToString::to_string).collect::<Vec<_>>(),
        "faces": format!("{:?}", session.selected_faces()),
        "camera": format!("{:?}", session.camera()),
        "context": session.context_state(),
        "report": session.map_checks_json(None, "", 0, 200).unwrap(),
        "repair": session.map_fix_preview_json(),
        "serials": [session.map_check_serial(), session.context_serial(), session.scene_serial(), session.view_serial(), session.log_serial(), session.camera_serial()],
    })
}

fn forms(f: &Fixture) -> Value {
    let checks = &f.checks;
    let context = &f.context;
    json!({
        "checks": {
            "tokens": [checks.get_report_token().to_string(), checks.get_selected_id().to_string(), checks.get_preview_token().to_string()],
            "query": checks.get_query().to_string(), "severity": checks.get_severity(), "page": checks.get_page(),
            "summary": checks.get_summary().to_string(), "page_summary": checks.get_page_summary().to_string(),
            "preview": checks.get_preview_text().to_string(), "stale": checks.get_stale(), "next": checks.get_has_next(),
            "detail": [checks.get_detail_title().to_string(), checks.get_detail().to_string(), checks.get_location().to_string()],
            "select": checks.get_can_select(), "repair": checks.get_can_repair(),
            "rows": checks.get_rows().iter().map(|row| (row.id.to_string(), row.title.to_string(), row.location.to_string(), row.severity.to_string())).collect::<Vec<_>>(),
        },
        "context": {
            "token": context.get_token().to_string(), "scope": context.get_scope().to_string(),
            "summary": context.get_summary().to_string(), "body": context.get_body().to_string(),
            "bounds": [context.get_min_x().to_string(),context.get_min_y().to_string(),context.get_min_z().to_string(),context.get_max_x().to_string(),context.get_max_y().to_string(),context.get_max_z().to_string()],
            "color": [context.get_red(),context.get_green(),context.get_blue()], "regions": context.get_show_regions(),
            "query": context.get_query().to_string(), "filter": context.get_filter_mode(), "page": context.get_page(),
            "query_bounds": [context.get_query_min_x().to_string(),context.get_query_min_y().to_string(),context.get_query_min_z().to_string(),context.get_query_max_x().to_string(),context.get_query_max_y().to_string(),context.get_query_max_z().to_string()],
            "include_map": context.get_include_map(), "confirm": context.get_confirm_delete(),
            "labels": [context.get_budget().to_string(),context.get_page_summary().to_string(),context.get_scope_label().to_string(),context.get_source_status().to_string(),context.get_note_budget().to_string(),context.get_feedback().to_string()],
            "flags": [context.get_can_attach(),context.get_has_next(),context.get_region(),context.get_can_apply(),context.get_can_delete(),context.get_can_focus()],
            "rows": context.get_rows().iter().map(|row| (row.scope.to_string(),row.summary.to_string(),row.detail.to_string())).collect::<Vec<_>>(),
        },
    })
}

fn files(directory: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut result = BTreeMap::new();
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            result.extend(files(&path));
        } else {
            result.insert(path.clone(), std::fs::read(path).unwrap());
        }
    }
    result
}

fn reject_actions(f: &Fixture, error: &str) {
    let expected_state = state(&f.shared);
    let expected_form = forms(f);
    macro_rules! rejected {
        ($action:expr, $message:expr) => {{
            $action;
            assert_eq!($message, error);
            assert_eq!(forms(f), expected_form);
            assert_eq!(state(&f.shared), expected_state);
        }};
    }
    rejected!(f.checks.invoke_run_checks(), f.checks.get_message());
    rejected!(f.checks.invoke_filter(), f.checks.get_message());
    rejected!(f.checks.invoke_inspect("1".into()), f.checks.get_message());
    rejected!(f.checks.invoke_navigate(false), f.checks.get_message());
    rejected!(f.checks.invoke_navigate(true), f.checks.get_message());
    rejected!(f.checks.invoke_review_repair(), f.checks.get_message());
    rejected!(f.checks.invoke_apply_repair(), f.checks.get_message());
    rejected!(f.checks.invoke_cancel_repair(), f.checks.get_message());
    rejected!(f.context.invoke_filter(), f.context.get_message());
    for scope in ["map", "selection", "new_region", "region:1"] {
        rejected!(
            f.context.invoke_begin(scope.into()),
            f.context.get_message()
        );
    }
    rejected!(f.context.invoke_edit(), f.context.get_message());
    rejected!(f.context.invoke_apply(), f.context.get_message());
    rejected!(f.context.invoke_cancel_edit(), f.context.get_message());
    rejected!(f.context.invoke_delete_note(), f.context.get_message());
    rejected!(f.context.invoke_focus_note(), f.context.get_message());
    rejected!(f.context.invoke_toggle_regions(), f.context.get_message());
    macro_rules! dismiss {
        ($window:expr) => {{
            let window = $window;
            window.show().unwrap();
            window.invoke_cancel();
            assert!(!window.window().is_visible());
            window.show().unwrap();
            window
                .window()
                .dispatch_event(slint::platform::WindowEvent::CloseRequested);
            assert!(!window.window().is_visible());
            assert_eq!(window.get_message(), error);
            assert_eq!(forms(f), expected_form);
            assert_eq!(state(&f.shared), expected_state);
        }};
    }
    dismiss!(&f.checks);
    dismiss!(&f.context);
}

#[test]
fn checks_and_context_live_actions_reject_real_drain_and_finished_without_touching_peer() {
    i_slint_backend_testing::init_no_event_loop();
    let f = fixture();
    let original = lock(&f.shared).document().vmf().clone();
    f.checks.invoke_navigate(false);
    f.checks.invoke_navigate(true);
    assert_eq!(lock(&f.shared).selected_faces().len(), 1);
    f.checks.invoke_cancel_repair();
    assert!(lock(&f.shared).map_fix_draft().is_none());
    f.checks.invoke_review_repair();
    f.checks.invoke_apply_repair();
    assert_ne!(lock(&f.shared).document().vmf(), &original);
    lock(&f.shared).undo(Actor::User).unwrap();
    assert_eq!(lock(&f.shared).document().vmf(), &original);
    f.context.invoke_begin("selection".into());
    assert!(f.context.get_scope().starts_with("solid:"));
    f.context.set_summary("Attached brush context".into());
    f.context.invoke_edit();
    f.context.invoke_apply();
    f.context.invoke_begin("region:1".into());
    f.context.invoke_focus_note();
    assert_eq!(lock(&f.shared).context_state()["focused_region"], 1);
    f.context.set_filter_mode(5);
    f.context.set_query_min_x("0".into());
    f.context.invoke_filter();
    assert!(f.context.get_rows().row_count() > 0);
    f.context.set_show_regions(false);
    f.context.invoke_toggle_regions();
    assert_eq!(lock(&f.shared).context_state()["show_regions"], false);
    let saved = lock(&f.shared).document().vmf().clone();
    f.context.invoke_delete_note();
    assert!(f.context.get_message().contains("Confirm"));
    f.context.set_confirm_delete(true);
    f.context.invoke_delete_note();
    assert_ne!(lock(&f.shared).document().vmf(), &saved);
    lock(&f.shared).undo(Actor::User).unwrap();
    f.context.invoke_begin("region:1".into());
    f.context.set_min_x("unfinished".into());
    f.context.invoke_edit();
    assert!(!f.context.get_can_apply());
    f.context.invoke_cancel_edit();
    assert!(lock(&f.shared).context_draft().is_none());
    f.context.invoke_begin("region:1".into());
    f.context.set_body("Retained pending context".into());
    f.context.invoke_edit();
    f.context.set_body("Unsubmitted local text".into());
    f.context.set_confirm_delete(true);
    f.checks.invoke_run_checks();
    f.checks.invoke_inspect("0".into());
    f.checks.invoke_review_repair();
    f.checks.set_query("pending filter".into());
    f.checks.set_page(7);
    let peer = fixture();
    let peer_before = state(&peer.shared);
    let peer_form = forms(&peer);
    let gate = f.document.admit().unwrap();
    let draining = f.document.clone();
    let (send, receive) = mpsc::channel();
    let thread = std::thread::spawn(move || send.send(draining.shutdown()).unwrap());
    let deadline = Instant::now() + TIMEOUT;
    while f.document.lifecycle() == DocumentLifecycle::Live
        || lock(&f.shared).compile_settings()["editable"] != false
    {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    let before = state(&f.shared);
    let before_files = files(f.directory.path());
    reject_actions(&f, "the editor is closing");
    assert_eq!(state(&f.shared), before);
    assert_eq!(files(f.directory.path()), before_files);
    assert!(receive.try_recv().is_err());
    assert_eq!(state(&peer.shared), peer_before);
    assert_eq!(forms(&peer), peer_form);
    drop(gate);
    receive.recv_timeout(TIMEOUT).unwrap().unwrap();
    thread.join().unwrap();
    assert_eq!(f.document.lifecycle(), DocumentLifecycle::Finished);
    let final_files = files(f.directory.path());
    reject_actions(&f, "the document is closed");
    assert_eq!(files(f.directory.path()), final_files);
    let entries = lock(&f.shared).recovery_entries().unwrap().entries;
    assert_eq!(entries.len(), 1);
    let store = crate::recovery::Store {
        directory: f.directory.path().join("recovery"),
    };
    let recovered = store.read(&entries[0].id).unwrap().1;
    assert!(recovered.contains("Attached brush context"));
    assert!(!recovered.contains("Retained pending context"));
    assert_eq!(state(&peer.shared), peer_before);
    assert_eq!(forms(&peer), peer_form);
    peer.checks.invoke_apply_repair();
    assert!(peer.checks.get_message().starts_with("Repaired 1"));
    peer.context.invoke_begin("map".into());
    peer.context.set_body("Peer remains editable".into());
    peer.context.invoke_edit();
    peer.context.invoke_apply();
    assert_eq!(
        lock(&peer.shared)
            .get_context_note(map_core::context::Scope::Map)
            .unwrap()["note"]["body"],
        "Peer remains editable"
    );
    assert_eq!(peer.document.lifecycle(), DocumentLifecycle::Live);
    peer.document.shutdown().unwrap();
}

#[test]
fn checks_and_context_preserve_exact_replaced_and_stale_review_guards() {
    i_slint_backend_testing::init_no_event_loop();
    let f = fixture();
    let old_repair = f.checks.get_preview_token();
    let report = lock(&f.shared).map_check_report().unwrap().token;
    lock(&f.shared).preview_map_fixes(report, &[1]).unwrap();
    let foreign = state(&f.shared);
    for invoke in [
        ChecksWindow::invoke_apply_repair,
        ChecksWindow::invoke_cancel_repair,
    ] {
        f.checks.set_preview_token(old_repair.clone());
        invoke(&f.checks);
        assert!(f.checks.get_message().contains("replaced"));
        assert_eq!(state(&f.shared), foreign);
    }
    f.checks.set_preview_token(old_repair.clone());
    f.checks.invoke_cancel();
    assert_eq!(state(&f.shared), foreign);
    f.checks.set_preview_token(old_repair);
    f.checks.show().unwrap();
    f.checks
        .window()
        .dispatch_event(slint::platform::WindowEvent::CloseRequested);
    assert!(!f.checks.window().is_visible());
    assert_eq!(state(&f.shared), foreign);
    let old_context = f.context.get_token();
    lock(&f.shared)
        .begin_context_edit(Some(map_core::context::Scope::Map))
        .unwrap();
    let foreign = state(&f.shared);
    for invoke in [
        ContextWindow::invoke_edit,
        ContextWindow::invoke_apply,
        ContextWindow::invoke_cancel_edit,
        ContextWindow::invoke_delete_note,
    ] {
        f.context.set_token(old_context.clone());
        invoke(&f.context);
        assert!(f.context.get_message().contains("replaced"));
        assert_eq!(state(&f.shared), foreign);
    }
    f.context.set_token(old_context.clone());
    f.context.show().unwrap();
    f.context.invoke_cancel();
    assert!(f.context.window().is_visible());
    assert_eq!(state(&f.shared), foreign);
    f.context.set_token(old_context);
    f.context
        .window()
        .dispatch_event(slint::platform::WindowEvent::CloseRequested);
    assert!(f.context.window().is_visible());
    assert_eq!(state(&f.shared), foreign);
    f.context.hide().unwrap();
    let old_report = f.checks.get_report_token();
    lock(&f.shared).run_map_checks(Actor::Agent);
    let replaced_report = state(&f.shared);
    f.checks.set_report_token(old_report.clone());
    f.checks.invoke_navigate(true);
    assert!(f.checks.get_message().contains("replaced"));
    assert_eq!(state(&f.shared), replaced_report);
    f.checks.set_report_token(old_report);
    f.checks.set_selected_id("0".into());
    f.checks.invoke_review_repair();
    assert!(f.checks.get_message().contains("replaced"));
    assert_eq!(state(&f.shared), replaced_report);
    f.context.invoke_begin("region:1".into());
    f.checks.invoke_run_checks();
    f.checks.invoke_inspect("0".into());
    f.checks.invoke_review_repair();
    lock(&f.shared)
        .create_block(
            Actor::User,
            DVec3::splat(128.0),
            DVec3::splat(160.0),
            "brick/test",
            false,
        )
        .unwrap();
    let stale = state(&f.shared);
    f.checks.invoke_apply_repair();
    assert!(f.checks.get_message().contains("stale"));
    f.context.invoke_apply();
    assert!(f.context.get_message().contains("changed"));
    f.context.set_confirm_delete(true);
    f.context.invoke_delete_note();
    assert!(f.context.get_message().contains("changed"));
    assert_eq!(state(&f.shared), stale);
    f.document.shutdown().unwrap();
}

#[test]
fn checks_and_context_children_keep_no_idle_session_or_runtime_owner() {
    i_slint_backend_testing::init_no_event_loop();
    let f = fixture();
    let checks = f.checks.clone_strong();
    let context = f.context.clone_strong();
    checks.show().unwrap();
    context.show().unwrap();
    checks.hide().unwrap();
    context.hide().unwrap();
    let weak_session = Arc::downgrade(&f.shared);
    let continuation = f.access.downgrade();
    drop(f);
    assert!(weak_session.upgrade().is_none());
    assert!(continuation.upgrade().is_err());
    checks.invoke_run_checks();
    checks.invoke_apply_repair();
    context.invoke_begin("new_region".into());
    context.invoke_apply();
    assert_eq!(checks.get_message(), "the document is closed");
    assert_eq!(context.get_message(), "the document is closed");
    checks.show().unwrap();
    context.show().unwrap();
    checks.invoke_cancel();
    context.invoke_cancel();
    assert!(!checks.window().is_visible());
    assert!(!context.window().is_visible());
    let f = fixture();
    f.checks.show().unwrap();
    f.checks.hide().unwrap();
    let weak = f.checks.as_weak();
    let before = state(&f.shared);
    let Fixture {
        checks,
        context,
        document,
        access,
        shared,
        directory: _directory,
    } = f;
    drop(checks);
    assert!(weak.upgrade().is_none());
    assert!(
        with_session(&weak, &access.downgrade(), |_, _| panic!(
            "vanished child accessed Session"
        ))
        .is_none()
    );
    assert_eq!(state(&shared), before);
    assert_eq!(document.lifecycle(), DocumentLifecycle::Live);
    drop(context);
    document.shutdown().unwrap();
}

#[test]
fn failed_checks_and_context_drain_preserves_forms_and_memoized_failure() {
    i_slint_backend_testing::init_no_event_loop();
    let f = fixture();
    f.context.set_body("Pending local text".into());
    let recovery = f.directory.path().join("recovery");
    std::fs::write(&recovery, b"blocked").unwrap();
    let error = f.document.shutdown().unwrap_err();
    assert_eq!(
        f.document.lifecycle(),
        DocumentLifecycle::Failed(error.clone())
    );
    let before = state(&f.shared);
    let before_files = files(f.directory.path());
    reject_actions(&f, &format!("document shutdown failed: {error}"));
    assert_eq!(state(&f.shared), before);
    assert_eq!(files(f.directory.path()), before_files);
    std::fs::remove_file(&recovery).unwrap();
    std::fs::create_dir(&recovery).unwrap();
    assert_eq!(f.document.shutdown().unwrap_err(), error);
    assert_eq!(std::fs::read_dir(recovery).unwrap().count(), 0);
}
