use super::*;
use crate::runtime::{DocumentHandle, DocumentLifecycle};
use crate::session::Session;
use std::sync::{Arc, Mutex};

fn playback(token: u64, child: Child) -> Playback {
    Playback {
        token,
        report: json!({}),
        _file: NamedTempFile::new().unwrap(),
        child,
        started: Instant::now(),
        finished: None,
    }
}

#[test]
fn close_observation_does_not_poll_playback_and_accepted_close_stops_owned_audio() {
    let directory = tempfile::tempdir().unwrap();
    let mut completed = Command::new("/bin/sh")
        .args(["-c", "exit 0"])
        .spawn()
        .unwrap();
    completed.wait().unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    session.configure_recovery(Ok(directory.path().join("recovery")));
    session.sounds.playback_serial = 1;
    session.sounds.playback = Some(playback(1, completed));
    let review = session.document_close_review();
    assert_eq!(
        review.inspection()["work"]["audio"]["playback"]["finished"],
        Value::Null
    );
    assert_eq!(session.sounds.playback.as_ref().unwrap().finished, None);
    let child = Command::new("/bin/sleep").arg("60").spawn().unwrap();
    let pid = child.id();
    session.sounds.playback_serial = 2;
    session.sounds.playback = Some(playback(2, child));
    let shared = Arc::new(Mutex::new(session));
    let document = DocumentHandle::start(shared.clone()).unwrap();
    let review = document.document_close_review().unwrap();
    assert!(!review.requires_discard());
    document.try_commit_reviewed_close(&review, false).unwrap();
    assert_eq!(document.lifecycle(), DocumentLifecycle::Draining);
    assert!(shared.lock().unwrap().sounds.playback.is_some());
    document.shutdown().unwrap();
    assert!(shared.lock().unwrap().sounds.playback.is_none());
    assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists());
}
