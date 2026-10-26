use super::*;

fn directory() -> tempfile::TempDir {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/color-correction-tests");
    std::fs::create_dir_all(&root).unwrap();
    tempfile::Builder::new()
        .prefix("source-")
        .tempdir_in(root)
        .unwrap()
}

#[test]
fn changed_missing_or_bad_lut_never_reuses_previous_data_or_claims_render_readiness() {
    let directory = directory();
    let path = directory.path().join("grade.raw");
    std::fs::write(&path, vec![17; color_correction::RAW_BYTES]).unwrap();
    let mut session = Session::new(Err("fixture content unavailable".into()));
    assert_eq!(session.color_correction_state()["enabled"], false);
    assert!(session.set_color_correction(true, 1.0, None).is_err());
    let source = session.document().vmf().clone();
    let history = session.history_snapshot();
    let camera = session.camera();
    let loaded = session.load_color_correction(&path).unwrap();
    assert_eq!(loaded["enabled"], false);
    session.set_color_correction(true, 0.5, None).unwrap();
    let view_serial = session.view_serial();
    assert!(session.set_color_correction(false, f32::NAN, None).is_err());
    assert_eq!(session.view_serial(), view_serial);
    assert_eq!(session.color_correction_state()["enabled"], true);
    std::fs::write(&path, vec![29; color_correction::RAW_BYTES]).unwrap();
    assert!(session.color_correction_for_render().is_err());
    let changed = session.color_correction_state();
    assert_eq!(changed["enabled"], true);
    assert_eq!(changed["ready"], false);
    assert!(changed["identity"].is_null());
    assert!(changed["error"].as_str().unwrap().contains("unavailable"));
    assert!(session.color_correction_for_render().is_err());
    assert!(session.set_color_correction(true, 0.5, None).is_err());
    session.set_color_correction(false, 0.5, None).unwrap();
    assert!(session.color_correction_for_render().unwrap().is_none());
    let reloaded = session.load_color_correction(&path).unwrap();
    assert_ne!(reloaded["identity"], loaded["identity"]);
    assert_ne!(reloaded["sha256"], loaded["sha256"]);
    assert!(
        session
            .set_color_correction(true, 1.0, loaded["identity"].as_u64())
            .is_err()
    );
    assert_eq!(session.color_correction_state()["enabled"], false);
    session.set_color_correction(true, 1.0, None).unwrap();
    let frame = session.color_correction_for_render().unwrap();
    session.set_color_correction(true, 0.75, None).unwrap();
    assert!(
        session
            .verify_color_correction_render(frame.as_ref())
            .is_err()
    );
    session.set_color_correction(true, 1.0, None).unwrap();
    std::fs::remove_file(&path).unwrap();
    assert!(session.color_correction_for_render().is_err());
    std::fs::write(&path, vec![29; color_correction::RAW_BYTES]).unwrap();
    assert!(session.color_correction_for_render().is_err());
    session.load_color_correction(&path).unwrap();
    for size in [
        color_correction::RAW_BYTES - 1,
        color_correction::RAW_BYTES + 1,
    ] {
        std::fs::write(&path, vec![0; size]).unwrap();
        assert!(session.load_color_correction(&path).is_err());
        let report = session.color_correction_state();
        assert_eq!(report["ready"], false);
        assert_eq!(report["enabled"], false);
        assert!(report["identity"].is_null());
    }
    assert!(session.load_color_correction(directory.path()).is_err());
    let cleared = session.clear_color_correction();
    assert!(cleared["error"].is_null());
    assert!(cleared["source"].is_null());
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(session.camera(), camera);
}

#[cfg(unix)]
#[test]
fn replaced_alias_and_fifo_are_explicitly_unavailable_without_blocking() {
    use std::os::unix::fs::symlink;
    let directory = directory();
    let first = directory.path().join("first.raw");
    let second = directory.path().join("second.raw");
    let alias = directory.path().join("alias.raw");
    std::fs::write(&first, vec![17; color_correction::RAW_BYTES]).unwrap();
    std::fs::write(&second, vec![29; color_correction::RAW_BYTES]).unwrap();
    symlink(&first, &alias).unwrap();
    let mut session = Session::new(Err("fixture content unavailable".into()));
    session.load_color_correction(&alias).unwrap();
    session.set_color_correction(true, 1.0, None).unwrap();
    std::fs::remove_file(&alias).unwrap();
    symlink(&second, &alias).unwrap();
    assert!(session.color_correction_for_render().is_err());
    let fifo = directory.path().join("pipe.raw");
    rustix::fs::mkfifoat(rustix::fs::CWD, &fifo, rustix::fs::Mode::RUSR).unwrap();
    assert!(session.load_color_correction(&fifo).is_err());
    assert_eq!(session.color_correction_state()["ready"], false);
}
