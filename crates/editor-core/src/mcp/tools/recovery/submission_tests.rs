use super::*;
use crate::recovery::{Entry, Kind, Settings, Store};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const TIMEOUT: Duration = Duration::from_secs(5);

fn fixture() -> (tempfile::TempDir, SharedSession, Store, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("source.vmf");
    std::fs::write(&path, Document::empty().vmf().write(&Default::default())).unwrap();
    let store = Store {
        directory: directory.path().join("recovery"),
    };
    let mut state = Session::new(Err("no assets required".into()));
    state.open(Actor::User, &path, false).unwrap();
    state.configure_recovery(Ok(store.directory.clone()));
    edit(&mut state, 0.0);
    (directory, Arc::new(Mutex::new(state)), store, path)
}

fn edit(session: &mut Session, offset: f64) {
    session
        .create_block(
            Actor::User,
            DVec3::splat(offset),
            DVec3::splat(offset + 16.0),
            "test",
            false,
        )
        .unwrap();
}

async fn occupy_blocking_pool() -> (tokio::task::JoinHandle<()>, mpsc::Sender<()>) {
    let (started, entered) = tokio::sync::oneshot::channel();
    let (release, wait) = mpsc::channel();
    let blocker = tokio::task::spawn_blocking(move || {
        started.send(()).unwrap();
        wait.recv_timeout(TIMEOUT).unwrap();
    });
    entered.await.unwrap();
    (blocker, release)
}

async fn wait_for_busy(session: &SharedSession, busy: bool) {
    let deadline = Instant::now() + TIMEOUT;
    while lock(session).recovery_busy() != busy {
        assert!(
            Instant::now() < deadline,
            "recovery busy did not become {busy}"
        );
        tokio::task::yield_now().await;
    }
}

fn snapshot(result: ToolResult) -> Entry {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false), "{result:?}");
    let value: Value = serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap();
    serde_json::from_value(value["snapshot"].clone()).unwrap()
}

#[test]
fn mcp_queued_recovery_freezes_content_and_keep_before_blocking_worker_runs() {
    let (_directory, shared, store, path) = fixture();
    let saved = std::fs::read(&path).unwrap();
    lock(&shared)
        .set_recovery_settings(
            Actor::User,
            Settings {
                keep_per_map: 3,
                ..Settings::default()
            },
            None,
        )
        .unwrap();
    let first = crate::recovery::capture(&shared, Actor::User, Kind::Manual).unwrap();
    let second = crate::recovery::capture(&shared, Actor::User, Kind::Manual).unwrap();
    lock(&shared)
        .preview_block(DVec3::splat(128.0), DVec3::splat(144.0))
        .unwrap();
    let frozen = lock(&shared).document().vmf().clone();
    let server = EditorServer::new(shared.clone(), None);
    let retained = server.clone();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let (blocker, release) = occupy_blocking_pool().await;
        let caller = tokio::spawn(async move { server.create_recovery_snapshot().await });
        wait_for_busy(&shared, true).await;
        assert_eq!(store.list().unwrap().entries.len(), 2);
        {
            let mut state = lock(&shared);
            edit(&mut state, 32.0);
            state
                .set_recovery_settings(
                    Actor::User,
                    Settings {
                        keep_per_map: 1,
                        ..Settings::default()
                    },
                    None,
                )
                .unwrap();
            state
                .preview_block(DVec3::splat(160.0), DVec3::splat(176.0))
                .unwrap();
        }
        let source = lock(&shared).document().vmf().clone();
        let history = lock(&shared).history_snapshot();
        let camera = lock(&shared).camera();
        let preview = lock(&shared).block_preview();
        release.send(()).unwrap();
        blocker.await.unwrap();
        let entry = snapshot(caller.await.unwrap());
        let (_, text) = store.read(&entry.id).unwrap();
        assert_eq!(source_formats::vmf::Vmf::parse(&text).unwrap(), frozen);
        let entries = store.list().unwrap().entries;
        assert_eq!(entries.len(), 3);
        assert!(entries.iter().any(|entry| entry.id == first.id));
        assert!(entries.iter().any(|entry| entry.id == second.id));
        let state = lock(&shared);
        assert_eq!(state.document().vmf(), &source);
        assert_eq!(state.history_snapshot(), history);
        assert_eq!(state.camera(), camera);
        assert_eq!(state.block_preview(), preview);
        assert_eq!(state.path(), Some(path.as_path()));
        assert_eq!(state.recovery_status()["pending_changes"], true);
        assert!(!state.recovery_busy());
    });
    assert_eq!(std::fs::read(path).unwrap(), saved);
    assert!(retained.offscreen.lock().unwrap().renderer.is_none());
}

#[test]
fn mcp_cancelled_queued_recovery_skips_capture_and_preserves_prior_errors() {
    let (_directory, shared, store, path) = fixture();
    let request = lock(&shared).begin_recovery_snapshot(Kind::Manual).unwrap();
    lock(&shared)
        .finish_recovery_snapshot(Actor::User, &request, Err("prior snapshot failure".into()))
        .unwrap_err();
    lock(&shared)
        .set_recovery_settings(
            Actor::User,
            Settings {
                interval_seconds: 0,
                ..Settings::default()
            },
            None,
        )
        .unwrap_err();
    let errors = lock(&shared).recovery_status();
    let source = lock(&shared).document().vmf().clone();
    let history = lock(&shared).history_snapshot();
    let camera = lock(&shared).camera();
    let saved = std::fs::read(&path).unwrap();
    let server = EditorServer::new(shared.clone(), None);
    let retained = server.clone();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let (blocker, release) = occupy_blocking_pool().await;
        let caller = tokio::spawn(async move { server.create_recovery_snapshot().await });
        wait_for_busy(&shared, true).await;
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        assert!(lock(&shared).recovery_busy());
        assert!(store.list().unwrap().entries.is_empty());
        release.send(()).unwrap();
        blocker.await.unwrap();
        wait_for_busy(&shared, false).await;
    });
    let state = lock(&shared);
    let status = state.recovery_status();
    assert_eq!(status["snapshot_error"], errors["snapshot_error"]);
    assert_eq!(status["settings_error"], errors["settings_error"]);
    assert_eq!(status["last_snapshot"], errors["last_snapshot"]);
    assert_eq!(state.document().vmf(), &source);
    assert_eq!(state.history_snapshot(), history);
    assert_eq!(state.camera(), camera);
    assert_eq!(state.path(), Some(path.as_path()));
    assert!(store.list().unwrap().entries.is_empty());
    assert_eq!(std::fs::read(path).unwrap(), saved);
    assert!(retained.offscreen.lock().unwrap().renderer.is_none());
}
