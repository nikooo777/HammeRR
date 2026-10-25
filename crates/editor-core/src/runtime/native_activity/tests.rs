use super::*;
use std::os::unix::fs::symlink;
use std::sync::mpsc;
use std::time::Duration;

#[test]
fn production_native_owner_reuses_only_confirmed_idle_capacity() {
    for mode in ["clean", "failed"] {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "runtime::native_activity::tests::process_lifetime_fixture",
                "--ignored",
                "--nocapture",
            ])
            .env("HAMMERR_NATIVE_AUTHORITY_FIXTURE", mode)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
#[ignore]
fn process_lifetime_fixture() {
    let mode = std::env::var("HAMMERR_NATIVE_AUTHORITY_FIXTURE").unwrap();
    let authority = Authority::process().unwrap();
    let (client, _) = clients(&authority);
    let mut permit = client.reserve(Phase::Capture, None, None).unwrap();
    if mode == "failed" {
        permit.started();
    }
    drop(authority);
    assert!(Authority::process().is_err());
    drop(permit);
    if mode == "failed" {
        assert!(
            Authority::process()
                .err()
                .unwrap()
                .contains("unconfirmed cleanup")
        );
        assert_eq!(client.status()["owner"]["phase"], "failed_cleanup");
    } else {
        let successor = Authority::process().unwrap();
        assert!(client.reserve(Phase::PlayStartup, None, None).is_err());
        let fresh = successor
            .client(DocumentId::allocate().unwrap())
            .reserve(Phase::PlayStartup, None, None)
            .unwrap();
        drop(fresh);
    }
}

#[test]
#[cfg(target_os = "linux")]
fn changed_executable_and_mount_fifo_reject_without_blocking_open() {
    for executable in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("binding");
        if executable {
            std::fs::write(&path, b"original executable").unwrap();
        } else {
            std::fs::create_dir(&path).unwrap();
        }
        let (send, receive) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let binding = Binding::capture_with(&path, executable, |canonical| {
                if executable {
                    std::fs::remove_file(canonical)?;
                } else {
                    std::fs::remove_dir(canonical)?;
                }
                rustix::fs::mkfifoat(
                    rustix::fs::CWD,
                    canonical,
                    rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
                )?;
                open_binding(canonical)
            });
            send.send(binding.is_err()).unwrap();
            directory
        });
        assert!(
            receive
                .recv_timeout(Duration::from_secs(3))
                .expect("native binding blocked opening a replaced FIFO")
        );
        drop(worker.join().unwrap());
    }
}

fn clients(authority: &Authority) -> (Client, Client) {
    (
        authority.client(DocumentId::allocate().unwrap()),
        authority.client(DocumentId::allocate().unwrap()),
    )
}

fn target(root: &Path) -> Target {
    std::fs::create_dir_all(root.join("cstrike")).unwrap();
    std::fs::write(root.join("cstrike_linux64"), b"fake executable").unwrap();
    Target::play(root, &root.join("cstrike"), &root.join("cstrike_linux64")).unwrap()
}

#[test]
fn one_activity_spans_different_installations_and_only_exact_clean_owner_releases() {
    let directory = tempfile::tempdir().unwrap();
    let mut a_root = directory.path().join("a");
    for _ in 0..12 {
        a_root.push("a".repeat(200));
    }
    let a_target = target(&a_root);
    let b_target = target(&directory.path().join("b"));
    let authority = Authority::isolated();
    let (a, b) = clients(&authority);
    assert_eq!(a.status()["available"], true);
    assert_eq!(b.status()["available"], true);
    let mut permit = a
        .reserve(Phase::Capture, Some(&a_target), Some(&b_target))
        .unwrap();
    let captured = b.status();
    assert_eq!(captured["owner"]["document_id"], a.document.value());
    assert_eq!(captured["owner"]["phase"], "capture");
    let busy = b
        .reserve(Phase::PlayStartup, None, Some(&b_target))
        .err()
        .unwrap();
    assert!(busy.contains("Native activity busy: reflection capture"));
    assert!(busy.contains(&format!(
        "activity {}, document {}",
        captured["owner"]["activity_id"],
        a.document.value()
    )));
    assert_eq!(busy, captured["message"].as_str().unwrap());
    assert!(busy.len() <= MAX_DIAGNOSTIC_BYTES);
    assert!(busy.ends_with('…'));
    assert_eq!(
        captured["owner"]["targets"]["capture"]["engine"]["path"],
        json!(a_root.canonicalize().unwrap())
    );
    permit.phase(Phase::PlayStartup);
    assert_eq!(
        b.status()["owner"]["activity_id"],
        captured["owner"]["activity_id"]
    );
    assert_eq!(b.status()["owner"]["phase"], "play_startup");
    permit.started();
    permit.confirmed_clean();
    drop(permit);
    let newer = b
        .reserve(Phase::PlayStartup, None, Some(&b_target))
        .unwrap();
    assert_ne!(
        a.status()["owner"]["activity_id"],
        captured["owner"]["activity_id"]
    );
    drop(newer);
    assert_eq!(a.status()["available"], true);
}

#[test]
fn failed_cleanup_is_sticky_after_permit_and_authority_observers_are_dropped() {
    let authority = Authority::isolated();
    let (a, b) = clients(&authority);
    let mut permit = a.reserve(Phase::Capture, None, None).unwrap();
    permit.started();
    permit.failed_cleanup("owned child did not exit");
    permit.confirmed_clean();
    drop(permit);
    assert_eq!(b.status()["owner"]["phase"], "failed_cleanup");
    assert_eq!(
        b.status()["owner"]["cleanup_error"],
        "owned child did not exit"
    );
    assert!(authority.cleanup_result().is_err());
    assert!(a.cleanup_result().is_err());
    assert!(b.cleanup_result().is_ok());
    assert!(b.reserve(Phase::PlayStartup, None, None).is_err());
    drop(authority);
    assert_eq!(b.status()["available"], false);
    assert!(b.reserve(Phase::PlayStartup, None, None).is_err());
}

#[test]
fn dropped_started_owner_retains_failure_and_closed_clients_cannot_revive() {
    let authority = Authority::isolated();
    let (a, b) = clients(&authority);
    let mut permit = a.reserve(Phase::Capture, None, None).unwrap();
    permit.started();
    drop(permit);
    assert_eq!(b.status()["owner"]["phase"], "failed_cleanup");
    drop(authority);
    a.core.owner_alive.store(true, Ordering::Release);
    assert!(a.reserve(Phase::PlayStartup, None, None).is_err());
}

#[test]
fn physical_aliases_bind_and_retargeted_or_replaced_entries_reject() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("root");
    let original = target(&root);
    let alias = directory.path().join("alias");
    symlink(&root, &alias).unwrap();
    let aliased = Target::play(
        &alias,
        &alias.join("cstrike"),
        &alias.join("cstrike_linux64"),
    )
    .unwrap();
    assert_eq!(original.describe(), aliased.describe());
    let replacement = directory.path().join("replacement");
    target(&replacement);
    std::fs::remove_file(&alias).unwrap();
    symlink(&replacement, &alias).unwrap();
    assert!(aliased.verify().is_err());
    original.verify().unwrap();
    std::fs::rename(root.join("cstrike_linux64"), root.join("old-client")).unwrap();
    std::fs::write(root.join("cstrike_linux64"), b"fake executable").unwrap();
    assert!(original.verify().is_err());
    let refreshed =
        Target::play(&root, &root.join("cstrike"), &root.join("cstrike_linux64")).unwrap();
    std::fs::rename(root.join("cstrike"), root.join("old-game")).unwrap();
    std::fs::create_dir(root.join("cstrike")).unwrap();
    assert!(refreshed.verify().is_err());
}

#[test]
fn capture_freezes_steam_and_directory_or_archive_content_bindings() {
    for archive in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("engine");
        target(&root);
        let steam = directory.path().join("steam");
        std::fs::create_dir(&steam).unwrap();
        let content = directory
            .path()
            .join(if archive { "content.vpk" } else { "content" });
        if archive {
            std::fs::write(&content, b"archive").unwrap();
        } else {
            std::fs::create_dir(&content).unwrap();
        }
        let environment = native_runtime::CaptureEnvironment {
            engine_root: root,
            steam_root: steam,
            mounts: vec![native_runtime::ContentMount {
                ids: vec!["game".into()],
                path: content.clone(),
            }],
        };
        let frozen = Target::capture(&environment).unwrap();
        frozen.verify().unwrap();
        std::fs::rename(&content, directory.path().join("old-content")).unwrap();
        if archive {
            std::fs::write(&content, b"archive").unwrap();
        } else {
            std::fs::create_dir(&content).unwrap();
        }
        assert!(frozen.verify().is_err());
    }
}

#[test]
fn raw_physical_installation_paths_fail_before_native_reservation() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    let directory = tempfile::tempdir().unwrap();
    let physical = directory
        .path()
        .join(OsString::from_vec(b"install-\xff".to_vec()));
    std::fs::create_dir_all(physical.join("cstrike")).unwrap();
    std::fs::write(physical.join("cstrike_linux64"), b"fixture").unwrap();
    let alias = directory.path().join("installation");
    symlink(&physical, &alias).unwrap();
    let error = Target::play(
        &alias,
        &alias.join("cstrike"),
        &alias.join("cstrike_linux64"),
    )
    .err()
    .unwrap();
    assert!(
        error.contains("cannot be represented as UTF-8 JSON"),
        "{error}"
    );
    let authority = Authority::isolated();
    let (client, _) = clients(&authority);
    assert_eq!(client.status()["available"], true);
    let valid = target(&directory.path().join("valid"));
    let permit = client
        .reserve(Phase::PlayStartup, None, Some(&valid))
        .unwrap();
    assert_eq!(
        client.status()["owner"]["targets"]["play"]["engine"]["path"],
        valid.engine.canonical.to_str().unwrap()
    );
    drop(permit);
    assert_eq!(client.status()["available"], true);
}
