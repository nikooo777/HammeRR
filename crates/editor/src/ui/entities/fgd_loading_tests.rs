use super::*;
use std::sync::{Arc, Mutex};

#[test]
fn fgd_worker_finishes_session_without_waiting_for_ui_notification() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("definitions.fgd");
    std::fs::write(&path, "@PointClass = loaded []").unwrap();
    let shared = Arc::new(Mutex::new(Session::new(Err("no assets needed".into()))));
    let app = AppWindow::new().unwrap();
    let window = EntityWindow::new().unwrap();
    spawn_fgd_load(&app.as_weak(), Some(window.as_weak()), &shared, &path);
    drop(window);
    drop(app);
    let deadline = Instant::now() + Duration::from_secs(5);
    while lock(&shared).definitions_serial() == 0 {
        assert!(
            Instant::now() < deadline,
            "FGD completion waited for the event loop"
        );
        std::thread::yield_now();
    }
    assert!(lock(&shared).definitions().class("loaded").is_some());
    let request = lock(&shared)
        .begin_definitions_load(&path)
        .expect("the worker ticket was not retired");
    drop(request);
}
