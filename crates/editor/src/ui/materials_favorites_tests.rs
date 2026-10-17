use super::*;
use std::sync::{Arc, Mutex};

#[test]
fn software_favorite_reload_and_shared_poll_preserve_selection_preview_and_weak_owner() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (directory, game) = crate::test_assets::game();
    let store = directory.path().join("state");
    let mut session = Session::new(Ok(game));
    session.configure_recovery(Ok(store.clone()));
    session.favorite_material("test/checker", true).unwrap();
    let shared = Arc::new(Mutex::new(session));
    let owner = Rc::new(());
    let weak = Rc::downgrade(&owner);
    let window = MaterialWindow::new().unwrap();
    bind_material_favorites(&window, shared.clone(), move || weak.upgrade().is_some());
    bind_material_preview(&window, shared.clone());
    window.set_material("test/checker".into());
    window.set_group("favorites".into());
    window.set_offset(36);
    window.invoke_preview_material();
    let detail = window.get_detail();
    let image = window.get_preview().to_rgba8().unwrap();
    window.show().unwrap();
    refresh_material_catalog(&window, &mut lock(&shared));
    assert_eq!(window.get_offset(), 0);
    assert_eq!(window.get_total(), 1);
    assert!(window.get_favorites_reload_available());
    let seen = lock(&shared).material_favorites_serial();
    let mut other = Session::new(Err("no graphics".into()));
    other.configure_recovery(Ok(store.clone()));
    other.favorite_material("external", true).unwrap();
    window.window().take_snapshot().unwrap();
    i_slint_backend_testing::ElementHandle::find_by_accessible_label(&window, "Reload favorites")
        .next()
        .unwrap()
        .invoke_accessible_default_action();
    assert!(lock(&shared).is_material_favorite("external"));
    assert_eq!(window.get_total(), 2);
    assert_eq!(window.get_material(), "test/checker");
    assert_eq!(window.get_group(), "favorites");
    let path = store.join(material_favorites::FILE_NAME);
    let valid = std::fs::read(&path).unwrap();
    std::fs::write(&path, b"broken").unwrap();
    window.invoke_reload_favorites();
    assert!(window.get_favorites_status().contains("could not reload"));
    assert_eq!(window.get_detail(), detail);
    std::fs::write(&path, valid).unwrap();
    window.invoke_reload_favorites();
    assert_eq!(
        window.get_favorites_status(),
        "Favorites saved across sessions"
    );
    assert_eq!(window.get_detail(), detail);
    assert_eq!(
        window.get_preview().to_rgba8().unwrap().as_bytes(),
        image.as_bytes()
    );
    other.reload_material_favorites().unwrap();
    other.favorite_material("external", false).unwrap();
    lock(&shared).reload_material_favorites().unwrap();
    let next = refresh_shared_favorites(Some(&window), &mut lock(&shared), seen);
    assert!(next > seen);
    assert_eq!(window.get_total(), 1);
    assert!(window.get_favorite());
    window
        .window()
        .set_size(slint::PhysicalSize::new(1180, 760));
    crate::ui::tests::save_ui_snapshot(window.window(), "material-favorites-reload");
    crate::ui::tests::assert_dialog_button_visible(&window, "Reload favorites");
    let before = lock(&shared).material_favorites_serial();
    drop(owner);
    window.invoke_toggle_favorite();
    window.invoke_reload_favorites();
    assert_eq!(lock(&shared).material_favorites_serial(), before);
    assert!(lock(&shared).is_material_favorite("test/checker"));
}

#[cfg(target_os = "linux")]
#[test]
fn software_favorite_reload_remains_available_when_material_catalog_fails() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (directory, game) = crate::test_assets::game();
    std::os::unix::fs::symlink("loop", directory.path().join("materials/loop")).unwrap();
    let store = directory.path().join("state");
    let mut session = Session::new(Ok(game));
    session.configure_recovery(Ok(store.clone()));
    session.favorite_material("test/checker", true).unwrap();
    let path = store.join(material_favorites::FILE_NAME);
    let valid = std::fs::read(&path).unwrap();
    let shared = Arc::new(Mutex::new(session));
    let window = MaterialWindow::new().unwrap();
    bind_material_favorites(&window, shared.clone(), || true);
    std::fs::write(&path, b"broken").unwrap();
    window.invoke_reload_favorites();
    assert_eq!(window.get_catalog_label(), "Material catalog unavailable");
    assert!(window.get_favorites_reload_available());
    assert!(window.get_favorites_status().contains("could not reload"));
    std::fs::write(&path, valid).unwrap();
    window.invoke_reload_favorites();
    assert_eq!(window.get_catalog_label(), "Material catalog unavailable");
    assert_eq!(
        window.get_favorites_status(),
        "Favorites saved across sessions"
    );
    assert!(lock(&shared).is_material_favorite("test/checker"));
}

#[test]
fn software_browser_observes_attached_session_changes_and_errors_through_shared_revision() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let (directory, game) = crate::test_assets::game();
    let store = directory.path().join("state");
    let authority = material_favorites::Authority::configured(Ok(store.clone()));
    let first = Session::with_material_favorites(Ok(game), authority.clone());
    let mut second = Session::with_material_favorites(Err("no graphics".into()), authority);
    let shared = Arc::new(Mutex::new(first));
    let window = MaterialWindow::new().unwrap();
    bind_material_favorites(&window, shared.clone(), || true);
    window.set_material("test/checker".into());
    window.set_group("favorites".into());
    window.show().unwrap();
    refresh_material_catalog(&window, &mut lock(&shared));
    let mut seen = lock(&shared).material_favorites_serial();
    let old = lock(&shared).material_favorites_snapshot();
    second.favorite_material("test/checker", true).unwrap();
    refresh_material_window_with_favorites(&window, &mut lock(&shared), &old);
    assert_eq!(window.get_total(), 0);
    assert!(!window.get_favorite());
    assert_eq!(old.status.count, 0);
    seen = refresh_shared_favorites(Some(&window), &mut lock(&shared), seen);
    assert_eq!(seen, second.material_favorites_serial());
    assert!(window.get_favorite());
    assert_eq!(window.get_total(), 1);
    assert!(second.favorite_material("../invalid", true).is_err());
    seen = refresh_shared_favorites(Some(&window), &mut lock(&shared), seen);
    assert_eq!(
        window.get_favorites_status(),
        second.material_favorite_status().error.unwrap()
    );
    window.invoke_toggle_favorite();
    assert!(!second.is_material_favorite("test/checker"));
    assert!(!window.get_favorite());
    assert_eq!(window.get_total(), 0);
    assert_eq!(
        window.get_favorites_status(),
        "Favorites saved across sessions"
    );
    let path = store.join(material_favorites::FILE_NAME);
    std::fs::write(&path, b"broken").unwrap();
    second.reload_material_favorites().unwrap_err();
    seen = refresh_shared_favorites(Some(&window), &mut lock(&shared), seen);
    assert!(window.get_favorites_status().contains("could not reload"));
    std::fs::write(&path, b"{\"version\":1,\"materials\":[\"test/checker\"]}").unwrap();
    second.reload_material_favorites().unwrap();
    let next = refresh_shared_favorites(Some(&window), &mut lock(&shared), seen);
    assert!(next > seen);
    assert_eq!(window.get_total(), 1);
    assert!(window.get_favorite());
    assert_eq!(
        window.get_favorites_status(),
        "Favorites saved across sessions"
    );
}
