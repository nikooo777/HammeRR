use super::root_callback;
use crate::AppWindow;
use crate::runtime::{DocumentAccess, DocumentLifecycle, SharedSession, lock};
use crate::session::{Actor, Session};
use glam::DVec3;
use render::Projection;
use slint::ComponentHandle;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

struct Owner {
    session: SharedSession,
}

#[test]
fn root_undo_and_projection_callbacks_reject_terminal_and_missing_owners() {
    i_slint_backend_testing::init_no_event_loop();
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no assets required".into()));
    session.configure_recovery(Ok(directory.path().join("recovery")));
    session
        .create_block(Actor::User, DVec3::ZERO, DVec3::splat(16.0), "test", false)
        .unwrap();
    let shared = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    let owner = Rc::new(Owner {
        session: shared.clone(),
    });
    let app = AppWindow::new().unwrap();
    root_callback!(@target app, owner, &access, on_undo, |_app| {
        drop(lock(&owner.session).undo(Actor::User));
    });
    root_callback!(@target app, owner, &access, on_projection_changed, |_app; index| {
        let projection = if index == 0 { Projection::Perspective } else { Projection::Top };
        lock(&owner.session).set_projection(Actor::User, projection);
    });
    assert_eq!(Rc::strong_count(&owner), 1);
    app.invoke_undo();
    assert!(lock(&shared).document().vmf().world.solids.is_empty());
    app.invoke_projection_changed(1);
    assert_eq!(lock(&shared).camera().projection, Projection::Top);
    lock(&shared).redo(Actor::User).unwrap();
    document.shutdown().unwrap();
    let before = lock(&shared).history_snapshot();
    let camera = lock(&shared).camera();
    app.invoke_undo();
    app.invoke_projection_changed(0);
    assert_eq!(app.get_message(), "the document is closed");
    assert_eq!(lock(&shared).history_snapshot(), before);
    assert_eq!(lock(&shared).camera(), camera);
    let weak = Rc::downgrade(&owner);
    drop(owner);
    assert!(weak.upgrade().is_none());
    app.set_message("Owner disappeared".into());
    app.invoke_undo();
    assert_eq!(app.get_message(), "Owner disappeared");
    assert_eq!(document.lifecycle(), DocumentLifecycle::Finished);
}
