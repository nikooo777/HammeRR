use super::*;
use crate::runtime::{DocumentHandle, DocumentLifecycle};
use slint::Model;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(5);

struct Fixture {
    directory: tempfile::TempDir,
    document: DocumentHandle,
    access: DocumentAccess,
    shared: SharedSession,
    window: MaterialWindow,
    entity: u32,
}

fn create_fixture() -> Fixture {
    let (directory, game) = crate::test_assets::game();
    let mut session = Session::new(Ok(game));
    session.configure_recovery(Ok(directory.path().join("state")));
    session.favorite_material("test/checker", true).unwrap();
    let ObjectId::Entity(entity) = session
        .create_entity(
            Actor::User,
            "info_target",
            Some(DVec3::ZERO),
            BTreeMap::new(),
        )
        .unwrap()
    else {
        panic!()
    };
    let generation = session.document_generation();
    let shared = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    let window = MaterialWindow::new().unwrap();
    bind_material_search_with_access(&window, &access, || true);
    bind_material_preview_with_access(&window, &access);
    bind_material_favorites_with_access(&window, &access, || true);
    bind_material_reload_with_access(&window, &access);
    bind_entity_material_choice_with_access(
        &window,
        &access,
        (entity, "material".into(), generation),
    );
    bind_material_cancel(&window);
    window.set_material("test/checker".into());
    window.invoke_search();
    window.invoke_preview_material();
    Fixture {
        directory,
        document,
        access,
        shared,
        window,
        entity,
    }
}

fn form(window: &MaterialWindow) -> (serde_json::Value, Vec<u8>) {
    let rows = window.get_rows();
    let names = (0..rows.row_count())
        .flat_map(|index| {
            let tiles = rows.row_data(index).unwrap().tiles;
            (0..tiles.row_count()).map(move |index| {
                let tile = tiles.row_data(index).unwrap();
                (tile.name.to_string(), tile.favorite)
            })
        })
        .collect::<Vec<_>>();
    (
        serde_json::json!({
            "material": window.get_material().to_string(),
            "filter": window.get_filter().to_string(), "group": window.get_group().to_string(),
            "offset": window.get_offset(), "total": window.get_total(), "names": names,
            "catalog": window.get_catalog_label().to_string(), "available": window.get_available(),
            "favorite": window.get_favorite(), "favorite_revision": window.get_favorites_revision().to_string(),
            "favorite_status": window.get_favorites_status().to_string(),
            "generation": window.get_asset_generation().to_string(),
            "reload": window.get_reload_status().to_string(), "channel": window.get_preview_channel()
        }),
        window
            .get_preview()
            .to_rgba8()
            .map(|image| image.as_bytes().to_vec())
            .unwrap_or_default(),
    )
}

fn actions(window: &MaterialWindow, error: &str) {
    for invoke in [
        MaterialWindow::invoke_search,
        MaterialWindow::invoke_preview_material,
        MaterialWindow::invoke_toggle_favorite,
        MaterialWindow::invoke_reload_favorites,
        MaterialWindow::invoke_reload_material_assets,
        MaterialWindow::invoke_use_material,
    ] {
        invoke(window);
        assert_eq!(window.get_detail(), error);
    }
}

#[test]
fn managed_browser_keeps_choice_review_and_rejects_terminal_reads_storage_and_form_changes() {
    i_slint_backend_testing::init_no_event_loop();
    let fixture = create_fixture();
    let peer = create_fixture();
    let peer_source = lock(&peer.shared).document().vmf().clone();
    let peer_history = lock(&peer.shared).history_snapshot();
    let peer_generation = lock(&peer.shared).materials().generation();
    let peer_form = form(&peer.window);
    let peer_favorites_path = peer
        .directory
        .path()
        .join("state")
        .join(material_favorites::FILE_NAME);
    let peer_favorites = std::fs::read(&peer_favorites_path).unwrap();
    let window = &fixture.window;
    assert!(window.get_total() > 0);
    assert!(window.get_available());
    let original = lock(&fixture.shared).document().vmf().clone();
    window.set_asset_generation("stale".into());
    window.invoke_use_material();
    assert!(window.get_detail().contains("Material assets changed"));
    assert_eq!(lock(&fixture.shared).document().vmf(), &original);
    window.invoke_use_material();
    assert_eq!(
        lock(&fixture.shared)
            .document()
            .entity(fixture.entity)
            .unwrap()
            .properties
            .get("material"),
        Some("test/checker")
    );
    window.show().unwrap();
    window.invoke_toggle_favorite();
    assert!(!lock(&fixture.shared).is_material_favorite("test/checker"));
    window.invoke_toggle_favorite();
    window.invoke_reload_favorites();
    window.invoke_reload_material_assets();
    window.set_filter("no-longer-searchable".into());
    window.set_material("test/late-choice".into());
    std::fs::write(
        fixture
            .directory
            .path()
            .join("materials/test/late-choice.vmt"),
        "UnlitGeneric { $basetexture test/checker }",
    )
    .unwrap();
    let expected_form = form(window);
    let source = lock(&fixture.shared).document().vmf().clone();
    let history = lock(&fixture.shared).history_snapshot();
    let camera = lock(&fixture.shared).camera();
    let favorites_path = fixture
        .directory
        .path()
        .join("state")
        .join(material_favorites::FILE_NAME);
    let favorites = std::fs::read(&favorites_path).unwrap();
    let generation = lock(&fixture.shared).materials().generation();
    let gate = fixture.document.admit().unwrap();
    let draining = fixture.document.clone();
    let (done, completed) = mpsc::sync_channel(1);
    let drain = std::thread::spawn(move || done.send(draining.shutdown()).unwrap());
    let deadline = Instant::now() + TIMEOUT;
    while fixture.document.lifecycle() == DocumentLifecycle::Live
        || lock(&fixture.shared).compile_settings()["editable"] != false
    {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    actions(window, "the editor is closing");
    assert_eq!(form(window), expected_form);
    assert!(completed.try_recv().is_err());
    drop(gate);
    completed.recv_timeout(TIMEOUT).unwrap().unwrap();
    drain.join().unwrap();
    let store = crate::recovery::Store {
        directory: fixture.directory.path().join("state"),
    };
    let entries = store.list().unwrap().entries;
    assert_eq!(entries.len(), 1);
    let recovery = store.read(&entries[0].id).unwrap().1;
    actions(window, "the document is closed");
    assert_eq!(form(window), expected_form);
    assert_eq!(lock(&fixture.shared).document().vmf(), &source);
    assert_eq!(lock(&fixture.shared).history_snapshot(), history);
    assert_eq!(lock(&fixture.shared).camera(), camera);
    assert_eq!(lock(&fixture.shared).materials().generation(), generation);
    assert_eq!(std::fs::read(&favorites_path).unwrap(), favorites);
    assert_eq!(store.list().unwrap().entries.len(), 1);
    assert_eq!(store.read(&entries[0].id).unwrap().1, recovery);
    assert!(window.window().is_visible());
    assert_eq!(lock(&peer.shared).document().vmf(), &peer_source);
    assert_eq!(lock(&peer.shared).history_snapshot(), peer_history);
    assert_eq!(lock(&peer.shared).materials().generation(), peer_generation);
    assert_eq!(std::fs::read(&peer_favorites_path).unwrap(), peer_favorites);
    assert_eq!(form(&peer.window), peer_form);
    peer.window.invoke_use_material();
    peer.window.invoke_toggle_favorite();
    assert_eq!(
        lock(&peer.shared)
            .document()
            .entity(peer.entity)
            .unwrap()
            .properties
            .get("material"),
        Some("test/checker")
    );
    assert!(!lock(&peer.shared).is_material_favorite("test/checker"));
    assert_eq!(peer.document.lifecycle(), DocumentLifecycle::Live);
    window.invoke_cancel();
    assert!(!window.window().is_visible());
    let weak = Arc::downgrade(&fixture.shared);
    let Fixture {
        directory: _directory,
        document,
        access,
        shared,
        window,
        entity: _,
    } = fixture;
    drop(access);
    drop(shared);
    drop(document);
    assert!(weak.upgrade().is_none());
    actions(&window, "the document is closed");
}

#[test]
fn actual_search_callback_holds_admission_through_catalog_refresh_before_final_recovery() {
    i_slint_backend_testing::init_no_event_loop();
    let fixture = create_fixture();
    fixture.window.set_filter("no-such-material".into());
    assert!(fixture.window.get_total() > 0);
    let draining = fixture.document.clone();
    let observed = fixture.document.clone();
    let shared = fixture.shared.clone();
    let (done, completed) = mpsc::sync_channel(1);
    let completed = Rc::new(RefCell::new(completed));
    let pending = completed.clone();
    let joining = Rc::new(RefCell::new(None));
    let joined = joining.clone();
    BEFORE_MATERIAL_ACTION.with(|slot| {
        *slot.borrow_mut() = Some(Box::new(move || {
            *joined.borrow_mut() = Some(std::thread::spawn(move || {
                done.send(draining.shutdown()).unwrap()
            }));
            let deadline = Instant::now() + TIMEOUT;
            while observed.lifecycle() == DocumentLifecycle::Live
                || lock(&shared).compile_settings()["editable"] != false
            {
                assert!(Instant::now() < deadline);
                std::thread::yield_now();
            }
            assert!(pending.borrow().try_recv().is_err());
        }));
    });
    fixture.window.invoke_search();
    assert_eq!(fixture.window.get_total(), 0);
    assert_eq!(fixture.window.get_catalog_label(), "No matching materials");
    completed.borrow().recv_timeout(TIMEOUT).unwrap().unwrap();
    joining.borrow_mut().take().unwrap().join().unwrap();
    assert_eq!(fixture.document.lifecycle(), DocumentLifecycle::Finished);
    fixture.window.invoke_search();
    assert_eq!(fixture.window.get_detail(), "the document is closed");
    assert_eq!(fixture.window.get_total(), 0);
}

#[test]
fn failed_browser_actions_keep_exact_error_and_vanished_window_abandons_before_target() {
    i_slint_backend_testing::init_no_event_loop();
    let fixture = create_fixture();
    let expected_form = form(&fixture.window);
    let state = fixture.directory.path().join("state");
    let parked = fixture.directory.path().join("parked");
    std::fs::rename(&state, &parked).unwrap();
    std::fs::write(&state, b"blocked").unwrap();
    let error = fixture.document.shutdown().unwrap_err();
    let expected_error = format!("document shutdown failed: {error}");
    actions(&fixture.window, &expected_error);
    assert_eq!(form(&fixture.window), expected_form);
    std::fs::remove_file(&state).unwrap();
    std::fs::create_dir(&state).unwrap();
    assert_eq!(fixture.document.shutdown().unwrap_err(), error);
    assert_eq!(std::fs::read_dir(&state).unwrap().count(), 0);
    let live = create_fixture();
    let owner = Rc::new(());
    let weak_owner = Rc::downgrade(&owner);
    bind_material_search_with_access(&live.window, &live.access, move || {
        weak_owner.upgrade().is_some()
    });
    live.window.set_filter("no-owner-search".into());
    let expected_form = form(&live.window);
    drop(owner);
    live.window.invoke_search();
    assert_eq!(form(&live.window), expected_form);
    let target = live.access.downgrade();
    let weak_window = live.window.as_weak();
    drop(live.window);
    assert!(
        with_material_window(&weak_window, &target, |_, _| panic!(
            "vanished browser performed work"
        ))
        .is_none()
    );
    assert_eq!(live.document.lifecycle(), DocumentLifecycle::Live);
}
