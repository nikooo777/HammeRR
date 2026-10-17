use super::*;
use crate::runtime::{DocumentHandle, DocumentLifecycle};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(5);

struct Fixture {
    directory: tempfile::TempDir,
    document: DocumentHandle,
    shared: SharedSession,
    access: DocumentAccess,
    controller: Controller,
    token: u64,
}

#[derive(Debug, PartialEq)]
struct Snapshot {
    map: source_formats::vmf::Vmf,
    history: String,
    selected: Vec<map_core::ObjectId>,
    serials: [u64; 3],
    choice: Result<serde_json::Value, String>,
    playback: serde_json::Value,
    files: BTreeMap<PathBuf, Vec<u8>>,
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

fn create_fixture() -> Fixture {
    let (directory, mut session, id) = crate::test_assets::sounds();
    session.configure_recovery(Ok(directory.path().join("recovery")));
    session
        .save(Actor::User, Some(&directory.path().join("source.vmf")))
        .unwrap();
    session
        .create_block(
            Actor::User,
            glam::DVec3::splat(128.0),
            glam::DVec3::splat(144.0),
            "test",
            false,
        )
        .unwrap();
    session
        .select(
            &[map_core::ObjectId::Entity(id)],
            crate::session::SelectMode::Replace,
        )
        .unwrap();
    let shared = Arc::new(Mutex::new(session));
    let document = crate::test_assets::document(shared.clone());
    let access = DocumentAccess::for_document(document.clone());
    let controller = Controller::new_with_access(&access).unwrap();
    controller
        .initialize(&mut lock(&shared), id, "message")
        .unwrap();
    controller.window.invoke_choose("Test.Rain".into());
    let token = controller.token.get().unwrap();
    Fixture {
        directory,
        document,
        shared,
        access,
        controller,
        token,
    }
}

fn snapshot(fixture: &Fixture) -> Snapshot {
    let mut session = lock(&fixture.shared);
    Snapshot {
        map: session.document().vmf().clone(),
        history: session.history_snapshot(),
        selected: session.selected_ids(),
        serials: [
            session.scene_serial(),
            session.view_serial(),
            session.log_serial(),
        ],
        choice: session.sound_choice(fixture.token),
        playback: session.sound_preview_status(),
        files: files(fixture.directory.path()),
    }
}

fn retained_actions(controller: &Controller) {
    controller.window.set_query("rain".into());
    controller.window.invoke_search();
    controller.window.invoke_page(1);
    controller.window.invoke_choose("ambient/two.wav".into());
    controller.window.set_variant(1);
    controller.window.invoke_variant_changed();
    controller.window.set_volume(80.0);
    controller.window.invoke_volume_changed();
    controller.window.invoke_apply();
    controller.window.invoke_reload();
    controller.window.invoke_play();
    controller.displayed_playback.set(Some(u64::MAX));
    controller.window.invoke_stop();
    controller.owned_playback.set(Some(u64::MAX));
    controller.window.invoke_cancel();
    assert!(controller.token.get().is_none());
    assert!(controller.owned_playback.get().is_none());
    assert!(controller.displayed_playback.get().is_none());
    assert!(!controller.window.window().is_visible());
    assert_eq!(controller.catalog.borrow().offset, 0);
}

#[test]
fn retained_sound_callbacks_reject_drain_and_closed_document_while_peer_stays_live() {
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            renderer_name: Some("software".into()),
            mock_time: true,
            threading: false,
        },
    )))
    .unwrap();
    let fixture = create_fixture();
    let peer = create_fixture();
    fixture.controller.window.set_volume(25.0);
    fixture.controller.window.invoke_volume_changed();
    assert_eq!(lock(&fixture.shared).sound_preview_volume(), 0.25);
    fixture.controller.window.show().unwrap();
    let gate = fixture.document.admit().unwrap();
    let draining = fixture.document.clone();
    let (send, receive) = mpsc::channel();
    let thread = std::thread::spawn(move || send.send(draining.shutdown()).unwrap());
    let deadline = Instant::now() + TIMEOUT;
    while fixture.document.lifecycle() == DocumentLifecycle::Live
        || lock(&fixture.shared).compile_settings()["editable"] != false
    {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(receive.try_recv().is_err());
    let before = snapshot(&fixture);
    let peer_before = snapshot(&peer);
    retained_actions(&fixture.controller);
    assert_eq!(
        fixture.controller.window.get_message(),
        "the editor is closing"
    );
    assert_eq!(snapshot(&fixture), before);
    assert_eq!(snapshot(&peer), peer_before);
    let peer_history = lock(&peer.shared).history_snapshot();
    peer.controller
        .window
        .invoke_choose("ambient/two.wav".into());
    peer.controller.window.invoke_apply();
    assert_ne!(lock(&peer.shared).history_snapshot(), peer_history);
    assert_eq!(peer.document.lifecycle(), DocumentLifecycle::Live);
    let peer_after = snapshot(&peer);
    drop(gate);
    receive.recv_timeout(TIMEOUT).unwrap().unwrap();
    thread.join().unwrap();
    assert_eq!(fixture.document.lifecycle(), DocumentLifecycle::Finished);
    let before = snapshot(&fixture);
    retained_actions(&fixture.controller);
    assert_eq!(
        fixture.controller.window.get_message(),
        "the document is closed"
    );
    assert_eq!(snapshot(&fixture), before);
    assert_eq!(snapshot(&peer), peer_after);
    assert_eq!(
        lock(&fixture.shared)
            .recovery_entries()
            .unwrap()
            .entries
            .len(),
        1
    );
    let continuation = fixture.access.downgrade();
    drop(fixture.access);
    drop(fixture.document);
    assert!(continuation.upgrade().is_err());
    let volume = lock(&fixture.shared).sound_preview_volume();
    fixture.controller.window.invoke_volume_changed();
    assert_eq!(lock(&fixture.shared).sound_preview_volume(), volume);
    assert_eq!(snapshot(&peer), peer_after);
    peer.document.shutdown().unwrap();
}

#[test]
fn vanished_sound_window_abandons_session_work_before_admission() {
    i_slint_backend_testing::init_no_event_loop();
    let fixture = create_fixture();
    let window = fixture.controller.window.as_weak();
    let continuation = fixture.access.downgrade();
    drop(fixture.controller);
    admitted(&window, &continuation, |_, _| {
        panic!("vanished sound window accessed Session")
    });
    assert_eq!(fixture.document.lifecycle(), DocumentLifecycle::Live);
    fixture.document.shutdown().unwrap();
}
