use super::*;

#[test]
fn accepted_reads_reserve_only_the_captured_file_size() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("source");
    let expected = vec![b' '; READ_BYTES + 1];
    fs::write(&path, &expected).unwrap();
    let bytes = Source::capture(&path, (READ_BYTES * 2) as u64)
        .unwrap()
        .read()
        .unwrap();
    assert_eq!(bytes, expected);
    assert_eq!(bytes.capacity(), expected.len());
}

#[test]
fn bounded_reads_reject_oversize_growth_shrink_and_replacement() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("source");
    fs::File::create(&path).unwrap().set_len(17).unwrap();
    assert!(Source::capture(&path, 16).is_err());
    assert!(Source::capture(directory.path(), 16).is_err());
    for replacement in [b"longer".as_slice(), b"s", b"other"] {
        fs::write(&path, b"first").unwrap();
        let captured = Source::capture(&path, 16).unwrap();
        fs::remove_file(&path).unwrap();
        fs::write(&path, replacement).unwrap();
        assert!(captured.read().is_err());
    }
}

#[test]
fn controlled_reads_reject_growth_and_cancellation_between_chunks() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("source");
    let bytes = vec![b' '; READ_BYTES * 2];
    fs::write(&path, &bytes).unwrap();
    let mut polls = 0;
    let error = Source::capture(&path, bytes.len() as u64)
        .unwrap()
        .read_controlled(|| {
            polls += 1;
            if polls == 3 {
                Err(io::Error::other("cancelled"))
            } else {
                Ok(())
            }
        })
        .unwrap_err();
    assert_eq!(error.to_string(), "cancelled");
    assert_eq!(polls, 3);
    let mut polls = 0;
    assert!(
        Source::capture(&path, bytes.len() as u64)
            .unwrap()
            .read_controlled(|| {
                polls += 1;
                if polls == 3 {
                    fs::File::options()
                        .append(true)
                        .open(&path)?
                        .set_len(bytes.len() as u64 + 1)?;
                }
                Ok(())
            })
            .is_err()
    );
    assert_eq!(fs::metadata(&path).unwrap().len(), bytes.len() as u64 + 1);
}

#[cfg(unix)]
#[test]
fn regular_aliases_load_but_retargeted_aliases_reject() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().unwrap();
    let real = directory.path().join("real");
    fs::create_dir(&real).unwrap();
    let original = real.join("source");
    let replacement = real.join("replacement");
    fs::write(&original, b"first").unwrap();
    fs::write(&replacement, b"other").unwrap();
    let hardlink = directory.path().join("hardlink");
    fs::hard_link(&original, &hardlink).unwrap();
    let alias = directory.path().join("alias");
    symlink(&original, &alias).unwrap();
    let alias_dir = directory.path().join("alias-dir");
    symlink(&real, &alias_dir).unwrap();
    for path in [&original, &hardlink, &alias, &alias_dir.join("source")] {
        assert_eq!(Source::capture(path, 16).unwrap().read().unwrap(), b"first");
    }
    let captured = Source::capture(&alias, 16).unwrap();
    fs::remove_file(&alias).unwrap();
    symlink(&replacement, &alias).unwrap();
    assert!(captured.read().is_err());
    let captured = Source::capture(&original, 16).unwrap();
    fs::remove_file(&original).unwrap();
    symlink(&replacement, &original).unwrap();
    assert!(captured.read().is_err());
}

#[cfg(target_os = "linux")]
#[test]
fn captured_regular_target_replaced_with_fifo_rejects_without_a_writer() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("source");
    fs::write(&path, b"first").unwrap();
    let captured = Source::capture(&path, 16).unwrap();
    fs::remove_file(&path).unwrap();
    rustix::fs::mkfifoat(
        rustix::fs::CWD,
        &path,
        rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
    )
    .unwrap();
    assert!(Source::capture(&path, 16).is_err());
    let (send, receive) = std::sync::mpsc::channel();
    let thread = std::thread::spawn(move || {
        send.send(captured.read()).unwrap();
    });
    assert!(
        receive
            .recv_timeout(std::time::Duration::from_secs(3))
            .expect("Opened pointfile target blocked on a FIFO")
            .is_err()
    );
    thread.join().unwrap();
}
