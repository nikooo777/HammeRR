use super::*;

fn fixture(directory: &Path) -> Session {
    let mut session = Session::new(Err("no assets needed".into()));
    session
        .create_block(Actor::User, DVec3::ZERO, DVec3::splat(16.0), "test", false)
        .unwrap();
    session
        .save(Actor::User, Some(&directory.join("source.vmf")))
        .unwrap();
    session
        .create_block(
            Actor::Agent,
            DVec3::splat(32.0),
            DVec3::splat(48.0),
            "test",
            false,
        )
        .unwrap();
    let path = directory.join("current.lin");
    std::fs::write(&path, "0 0 32\n64 0 32\n64 64 64\n").unwrap();
    session.load_pointfile(Actor::User, &path).unwrap();
    session.navigate_pointfile(1, None).unwrap();
    session
}

fn rejected_load_keeps_state(session: &mut Session, path: &Path) -> String {
    let document = session.document.vmf().clone();
    let history = session.history_snapshot();
    let report = serde_json::to_value(session.pointfile_report(0, MAX_PAGE).unwrap()).unwrap();
    let lines = session.build_scene().leak_lines;
    let source_path = session.path.clone().unwrap();
    let source = std::fs::read(&source_path).unwrap();
    let camera = session.camera;
    let view = session.view;
    let serials = (
        session.scene_serial,
        session.view_serial,
        session.log_serial,
    );
    let error = session.load_pointfile(Actor::Agent, path).unwrap_err();
    assert_eq!(session.document.vmf(), &document);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(session.path.as_ref(), Some(&source_path));
    assert_eq!(std::fs::read(&source_path).unwrap(), source);
    assert_eq!(session.camera, camera);
    assert_eq!(session.view, view);
    assert_eq!(
        (
            session.scene_serial,
            session.view_serial,
            session.log_serial
        ),
        serials
    );
    assert_eq!(
        serde_json::to_value(session.pointfile_report(0, MAX_PAGE).unwrap()).unwrap(),
        report
    );
    assert_eq!(session.build_scene().leak_lines, lines);
    error
}

#[test]
fn failed_loads_preserve_the_current_path_overlay_and_map_history() {
    let directory = tempfile::tempdir().unwrap();
    let mut session = fixture(directory.path());
    rejected_load_keeps_state(&mut session, directory.path());
    rejected_load_keeps_state(&mut session, &directory.path().join("missing.lin"));
    let path = directory.path().join("invalid.lin");
    for bytes in [
        b"\xff\xfe".as_slice(),
        b"0 0 0\nNaN 0 0\n",
        b"0 0 0\n1 2 3 4\n",
        b"0 0 0\n1000001 0 0\n",
    ] {
        std::fs::write(&path, bytes).unwrap();
        rejected_load_keeps_state(&mut session, &path);
    }
    std::fs::write(&path, "0 0 0\n".repeat(MAX_POINTS + 1)).unwrap();
    assert!(rejected_load_keeps_state(&mut session, &path).contains("points"));
    std::fs::File::create(&path)
        .unwrap()
        .set_len(MAX_BYTES + 1)
        .unwrap();
    assert!(rejected_load_keeps_state(&mut session, &path).contains("regular file"));
}

#[test]
fn pointfile_byte_and_point_limits_accept_the_boundary() {
    let directory = tempfile::tempdir().unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    let path = directory.path().join("boundary.lin");
    let mut text = "0 0 0\n".repeat(MAX_POINTS);
    text.extend(std::iter::repeat_n(' ', MAX_BYTES as usize - text.len()));
    std::fs::write(&path, &text).unwrap();
    session.load_pointfile(Actor::User, &path).unwrap();
    assert_eq!(session.pointfile_report(0, 1).unwrap().total, MAX_POINTS);
}

#[cfg(unix)]
#[test]
fn regular_file_and_directory_aliases_preserve_the_requested_path() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().unwrap();
    let real = directory.path().join("real");
    std::fs::create_dir(&real).unwrap();
    let path = real.join("path.lin");
    std::fs::write(&path, "0 0 32\n64 0 32\n").unwrap();
    let hardlink = directory.path().join("hardlink.lin");
    std::fs::hard_link(&path, &hardlink).unwrap();
    let alias = directory.path().join("alias.lin");
    symlink(&path, &alias).unwrap();
    let alias_dir = directory.path().join("alias-dir");
    symlink(&real, &alias_dir).unwrap();
    let mut session = Session::new(Err("no assets needed".into()));
    for requested in [path, hardlink, alias, alias_dir.join("path.lin")] {
        session.load_pointfile(Actor::User, &requested).unwrap();
        let report = session.pointfile_report(0, 2).unwrap();
        assert_eq!(report.path, Some(requested.display().to_string()));
        assert_eq!(report.points, [[0.0, 0.0, 32.0], [64.0, 0.0, 32.0]]);
        assert!(report.current);
    }
}

#[cfg(target_os = "linux")]
#[test]
fn fifo_load_rejects_without_a_writer_and_preserves_the_overlay() {
    let directory = tempfile::tempdir().unwrap();
    let mut session = fixture(directory.path());
    let path = directory.path().join("pipe.lin");
    rustix::fs::mkfifoat(
        rustix::fs::CWD,
        &path,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
    )
    .unwrap();
    let (send, receive) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        send.send(rejected_load_keeps_state(&mut session, &path))
            .unwrap();
    });
    let error = receive
        .recv_timeout(std::time::Duration::from_secs(3))
        .expect("Pointfile load blocked on a FIFO");
    assert!(error.contains("regular file"), "{error}");
    thread.join().unwrap();
}

#[test]
fn drained_document_rejects_retained_pointfile_loads_before_filesystem_work() {
    let directory = tempfile::tempdir().unwrap();
    let mut state = fixture(directory.path());
    state.configure_recovery(Ok(directory.path().join("recovery")));
    let shared = std::sync::Arc::new(std::sync::Mutex::new(state));
    let document = crate::runtime::DocumentHandle::start(shared.clone()).unwrap();
    document.shutdown().unwrap();
    let mut state = crate::runtime::lock(&shared);
    let recovery = serde_json::to_value(state.recovery_entries().unwrap()).unwrap();
    for path in [
        directory.path().join("current.lin"),
        directory.path().join("missing.lin"),
    ] {
        let error = rejected_load_keeps_state(&mut state, &path);
        assert!(error.contains("closing"), "{error}");
    }
    assert_eq!(
        serde_json::to_value(state.recovery_entries().unwrap()).unwrap(),
        recovery
    );
    let mut peer = Session::new(Err("no assets needed".into()));
    peer.load_pointfile(Actor::Agent, &directory.path().join("current.lin"))
        .unwrap();
    assert_eq!(peer.pointfile_report(0, 1).unwrap().total, 3);
}
