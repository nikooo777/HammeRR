use super::*;
use map_core::ObjectId;
use slint::Model;
use source_formats::vmf::Entity;
use std::sync::{Arc, Mutex};

fn add_marker(session: &mut Session, actor: Actor, label: &str) -> ObjectId {
    session
        .edit(actor, label, |tx| {
            let id = tx.new_object_id()?;
            tx.add_entity(Entity::new(id, "info_target"))?;
            Ok(ObjectId::Entity(id))
        })
        .unwrap()
}

#[test]
fn history_window_inspects_cancels_navigates_guards_and_reopens_with_software_rendering() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let mut session = Session::new(Err("no assets".into()));
    add_marker(&mut session, Actor::User, "Create courtyard marker");
    let first = session.inspect_history(0, PAGE_SIZE).unwrap().current_state;
    let first_source = session.document().vmf().clone();
    let removed_by_undo = add_marker(&mut session, Actor::Agent, "Add agent route marker");
    let initial = session.inspect_history(0, PAGE_SIZE).unwrap();
    let source = session.document().vmf().clone();
    let camera = session.camera();
    let shared = Arc::new(Mutex::new(session));
    let controller = Controller::new(&shared).unwrap();
    let window = &controller.window;
    controller.show(&lock(&shared)).unwrap();
    assert_eq!(window.get_rows().row_count(), 2);
    assert!(window.get_position().contains("2 applied"));
    assert!(
        window
            .get_rows()
            .row_data(0)
            .unwrap()
            .status
            .contains("agent")
    );
    assert!(
        window
            .get_rows()
            .row_data(1)
            .unwrap()
            .status
            .contains("you")
    );
    assert!(!window.get_can_navigate());
    window.window().take_snapshot().unwrap();
    i_slint_backend_testing::ElementHandle::find_by_accessible_label(
        window,
        "Inspect 1. Create courtyard marker",
    )
    .next()
    .unwrap()
    .invoke_accessible_default_action();
    assert!(window.get_can_navigate());
    assert!(window.get_message().contains("undo 1 transaction"));
    assert!(window.get_detail().contains("added entity:"));
    crate::ui::tests::save_ui_snapshot(window.window(), "history");
    crate::ui::tests::assert_dialog_button_visible(window, "Go to selected state");
    crate::ui::tests::assert_dialog_button_visible(window, "Cancel selection");
    crate::ui::tests::assert_dialog_button_visible(window, "Close");
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).history_snapshot(), initial.snapshot);
    window.invoke_cancel();
    assert!(!window.get_can_navigate());
    assert_eq!(window.get_selected_state(), "");
    assert_eq!(lock(&shared).document().vmf(), &source);
    window.invoke_choose(first.clone().into());
    window.invoke_close_dialog();
    assert!(!window.window().is_visible());
    assert_eq!(lock(&shared).history_snapshot(), initial.snapshot);
    controller.show(&lock(&shared)).unwrap();
    assert_eq!(window.get_selected_state(), "");
    lock(&shared)
        .select(&[removed_by_undo], crate::session::SelectMode::Replace)
        .unwrap();
    window.invoke_choose(first.clone().into());
    window.invoke_navigate();
    assert_eq!(lock(&shared).document().vmf(), &first_source);
    assert!(lock(&shared).selection().is_empty());
    assert_eq!(lock(&shared).camera(), camera);
    assert!(window.get_message().contains("Navigation complete: undo 1"));
    assert!(window.get_position().contains("1 available to redo"));
    window.invoke_choose(initial.current_state.clone().into());
    assert!(window.get_message().contains("redo 1 transaction"));
    window.invoke_navigate();
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert!(window.get_message().contains("Navigation complete: redo 1"));

    window.invoke_choose(first.into());
    add_marker(&mut lock(&shared), Actor::Agent, "Intervening agent edit");
    let intervening = lock(&shared).document().vmf().clone();
    window.invoke_navigate();
    assert_eq!(lock(&shared).document().vmf(), &intervening);
    assert!(window.get_message().contains("Refresh history"));
    assert_eq!(window.get_selected_state(), "");
    window.invoke_choose(INITIAL_STATE.into());
    assert!(window.get_can_navigate());
    lock(&shared)
        .create_primitive(Actor::Agent, map_core::primitive::Spec::default(), true)
        .unwrap();
    controller.refresh(&lock(&shared));
    assert!(!window.get_can_navigate());
    window.invoke_navigate();
    assert_eq!(lock(&shared).document().vmf(), &intervening);
    assert!(window.get_message().contains("active preview"));
    let token = lock(&shared).primitive_preview().unwrap().token;
    lock(&shared).cancel_primitive_preview(token).unwrap();
    controller.refresh(&lock(&shared));
    assert!(window.get_can_navigate());
    window.invoke_cancel();
    for index in 0..PAGE_SIZE {
        add_marker(&mut lock(&shared), Actor::User, &format!("Marker {index}"));
    }
    controller.refresh(&lock(&shared));
    assert_eq!(window.get_rows().row_count(), PAGE_SIZE);
    assert!(window.get_has_older());
    window.invoke_older();
    assert_eq!(window.get_rows().row_count(), 3);
    assert!(window.get_has_newer());
    assert!(!window.get_has_older());
    window.invoke_newer();
    assert_eq!(window.get_rows().row_count(), PAGE_SIZE);
    let before = lock(&shared).history_snapshot();
    window.invoke_refresh_history();
    assert_eq!(lock(&shared).history_snapshot(), before);
    let target = window.get_rows().row_data(1).unwrap().state;
    window.invoke_choose(target);
    lock(&shared).new_map(Actor::Agent, true).unwrap();
    window.invoke_navigate();
    assert!(lock(&shared).document().objects().next().is_none());
    assert_eq!(window.get_rows().row_count(), 0);
    assert!(!window.get_can_navigate());
    assert!(window.get_message().contains("Refresh history"));
    window.invoke_close_dialog();
    controller.show(&lock(&shared)).unwrap();
    assert!(window.window().is_visible());
    assert_eq!(window.get_rows().row_count(), 0);
    assert_eq!(window.get_selected_state(), "");
    window.invoke_close_dialog();
}

fn admitted_fixture() -> (
    tempfile::TempDir,
    crate::runtime::DocumentHandle,
    SharedSession,
) {
    i_slint_backend_testing::init_no_event_loop();
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no assets required".into()));
    session.configure_recovery(Ok(directory.path().join("recovery")));
    add_marker(&mut session, Actor::User, "First marker");
    let shared = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    (directory, document, shared)
}

#[test]
fn admitted_history_callbacks_work_then_reject_retained_terminal_actions_without_refreshing_forms()
{
    let (_directory, document, shared) = admitted_fixture();
    let access = DocumentAccess::for_document(document.clone());
    let first = lock(&shared)
        .inspect_history(0, PAGE_SIZE)
        .unwrap()
        .current_state;
    let first_source = lock(&shared).document().vmf().clone();
    add_marker(&mut lock(&shared), Actor::Agent, "Second marker");
    let last = lock(&shared)
        .inspect_history(0, PAGE_SIZE)
        .unwrap()
        .current_state;
    let controller = Controller::new_with_access(&access).unwrap();
    controller.show(&lock(&shared)).unwrap();
    controller.window.invoke_choose(first.clone().into());
    controller.window.invoke_navigate();
    assert_eq!(lock(&shared).document().vmf(), &first_source);
    controller.window.invoke_choose(last.into());
    controller.window.invoke_navigate();
    controller.window.invoke_choose(first.clone().into());
    assert!(controller.window.get_can_navigate());
    let window = controller.window.clone_strong();
    drop(controller);
    let mut peer = Session::new(Err("peer assets unavailable".into()));
    add_marker(&mut peer, Actor::User, "First marker");
    add_marker(&mut peer, Actor::Agent, "Second marker");
    let peer_source = peer.document().vmf().clone();
    let peer_history = peer.history_snapshot();
    document.shutdown().unwrap();
    let source = lock(&shared).document().vmf().clone();
    let history = lock(&shared).history_snapshot();
    let camera = lock(&shared).camera();
    let selected = window.get_selected_state();
    let position = window.get_position();
    window.invoke_navigate();
    assert_eq!(window.get_message(), "the document is closed");
    window.invoke_choose(INITIAL_STATE.into());
    assert_eq!(window.get_selected_state(), selected);
    window.invoke_older();
    assert_eq!(window.get_message(), "the document is closed");
    window.invoke_newer();
    assert_eq!(window.get_message(), "the document is closed");
    window.invoke_refresh_history();
    assert_eq!(window.get_message(), "the document is closed");
    assert_eq!(window.get_position(), position);
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).history_snapshot(), history);
    assert_eq!(lock(&shared).camera(), camera);
    assert_eq!(peer.document().vmf(), &peer_source);
    assert_eq!(peer.history_snapshot(), peer_history);
    window.invoke_cancel();
    assert_eq!(window.get_selected_state(), "");
    window.invoke_close_dialog();
    assert!(!window.window().is_visible());
}

#[test]
fn retained_history_window_does_not_keep_session_or_runtime_alive() {
    let (_directory, document, shared) = admitted_fixture();
    let weak = Arc::downgrade(&shared);
    let access = DocumentAccess::for_document(document.clone());
    let controller = Controller::new_with_access(&access).unwrap();
    controller.show(&lock(&shared)).unwrap();
    controller.window.invoke_choose(INITIAL_STATE.into());
    let window = controller.window.clone_strong();
    drop(controller);
    drop(access);
    drop(shared);
    drop(document);
    assert!(weak.upgrade().is_none());
    window.invoke_navigate();
    assert_eq!(window.get_message(), "the document is closed");
    window.invoke_refresh_history();
    assert_eq!(window.get_message(), "the document is closed");
    window.invoke_close_dialog();
}

#[test]
fn history_failed_terminal_admission_preserves_exact_error_without_retry() {
    let (directory, document, shared) = admitted_fixture();
    let access = DocumentAccess::for_document(document.clone());
    let controller = Controller::new_with_access(&access).unwrap();
    controller.show(&lock(&shared)).unwrap();
    controller.window.invoke_choose(INITIAL_STATE.into());
    let selected = controller.window.get_selected_state();
    let recovery = directory.path().join("recovery");
    std::fs::write(&recovery, b"not a directory").unwrap();
    let error = document.shutdown().unwrap_err();
    controller.window.invoke_navigate();
    assert_eq!(
        controller.window.get_message(),
        format!("document shutdown failed: {error}")
    );
    assert_eq!(controller.window.get_selected_state(), selected);
    std::fs::remove_file(&recovery).unwrap();
    std::fs::create_dir(&recovery).unwrap();
    assert_eq!(document.shutdown().unwrap_err(), error);
    assert_eq!(std::fs::read_dir(&recovery).unwrap().count(), 0);
}
