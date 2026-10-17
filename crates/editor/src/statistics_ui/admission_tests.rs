use super::*;
use crate::runtime::{DocumentLifecycle, lock};
use crate::session::Actor;
use std::sync::{Arc, Mutex};

#[test]
fn vanished_statistics_window_abandons_refresh_while_document_stays_live() {
    i_slint_backend_testing::init_no_event_loop();
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("No assets required".into()));
    session.configure_recovery(Ok(directory.path().join("recovery")));
    session
        .create_block(
            Actor::User,
            glam::DVec3::ZERO,
            glam::DVec3::splat(32.0),
            "brick/test",
            false,
        )
        .unwrap();
    let shared = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    let window = StatisticsWindow::new().unwrap();
    bind_with_access(&window, &access);
    window.invoke_refresh();
    assert_eq!(window.get_brushes(), "1");
    let vanished = window.as_weak();
    drop(window);
    assert!(vanished.upgrade().is_none());
    let before = lock(&shared).history_snapshot();
    admitted_refresh(&vanished, &access.downgrade());
    assert_eq!(lock(&shared).history_snapshot(), before);
    assert_eq!(document.lifecycle(), DocumentLifecycle::Live);
    document.shutdown().unwrap();
}
