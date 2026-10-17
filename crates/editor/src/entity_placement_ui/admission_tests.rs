use super::*;
use crate::runtime::DocumentLifecycle;
use glam::DVec3;
use serde_json::json;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(5);

fn form(controller: &Controller) -> Value {
    let form = controller.form.borrow();
    json!({"token": form.token, "snapshot": form.snapshot, "properties": form.properties, "dirty": form.dirty})
}

#[test]
fn admitted_placement_body_finishes_borrowed_picking_and_apply_before_final_recovery() {
    i_slint_backend_testing::init_no_event_loop();
    let directory = tempfile::tempdir().unwrap();
    let (mut session, _) = crate::test_assets::placement();
    session.configure_recovery(Ok(directory.path().join("recovery")));
    let shared = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    let vanished = EntityPlacementWindow::new().unwrap().as_weak();
    callback(&vanished, &access.downgrade(), |_, _| {
        panic!("vanished placement accessed the document")
    });
    let controller = Controller::new_with_access(&access).unwrap();
    controller.initialize(&lock(&shared), "info_target");
    controller.window.invoke_pick();
    let (entered, begin) = mpsc::channel();
    let (finished, done) = mpsc::channel();
    let draining = document.clone();
    let worker = std::thread::spawn(move || {
        begin.recv_timeout(TIMEOUT).unwrap();
        finished.send(draining.shutdown()).unwrap();
    });
    callback(
        &controller.window.as_weak(),
        &access.downgrade(),
        |_, session| {
            entered.send(()).unwrap();
            let deadline = Instant::now() + TIMEOUT;
            while document.lifecycle() == DocumentLifecycle::Live {
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
            assert!(done.try_recv().is_err());
            controller.initialize(session, "info_target");
            controller
                .pick_at_ray(
                    session,
                    Ray {
                        origin: DVec3::new(32.0, 0.0, 64.0),
                        direction: DVec3::Y,
                    },
                )
                .unwrap();
            controller.refresh(session);
            let token = controller.form.borrow().token.unwrap();
            session.apply_entity_placement(Actor::User, token).unwrap();
        },
    );
    done.recv_timeout(TIMEOUT).unwrap().unwrap();
    worker.join().unwrap();
    assert_eq!(document.lifecycle(), DocumentLifecycle::Finished);
    let entries = lock(&shared).recovery_entries().unwrap().entries;
    assert_eq!(entries.len(), 1);
    let store = crate::recovery::Store {
        directory: directory.path().join("recovery"),
    };
    let (_, text) = store.read(&entries[0].id).unwrap();
    assert_eq!(
        source_formats::vmf::Vmf::parse(&text).unwrap(),
        *lock(&shared).document().vmf()
    );
    let before = form(&controller);
    controller.window.invoke_invalidate();
    controller.window.invoke_pick();
    controller.window.invoke_review();
    controller.window.invoke_apply();
    controller.window.invoke_cancel();
    controller
        .window
        .window()
        .dispatch_event(slint::platform::WindowEvent::CloseRequested);
    assert_eq!(form(&controller), before);
    assert_eq!(controller.window.get_message(), "the document is closed");
}
