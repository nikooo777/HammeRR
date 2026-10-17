use super::*;
use crate::runtime::LaunchSubmission;
use native_runtime::{
    CaptureFailure, CaptureObserver, CaptureReceipt, CaptureRequest, CleanupDisposition,
};
use source_formats::bsp::{Bsp, PakLimits};
use std::sync::atomic::AtomicUsize;
use std::time::Instant;

fn fake_capture(
    request: CaptureRequest,
    observer: &mut dyn CaptureObserver,
) -> Result<CaptureReceipt, CaptureFailure> {
    observer.checkpoint().map_err(|error| CaptureFailure {
        error,
        cleanup: CleanupDisposition::Confirmed,
    })?;
    let staged =
        native_runtime::capture_staging_bsp_for_channel(&request.bsp, request.channel).unwrap();
    let mut captured = Bsp::parse(&staged).unwrap();
    let mut archive = captured.pak_data(PakLimits::default()).unwrap();
    for sample in &request.samples {
        let entry = archive
            .entries
            .iter_mut()
            .find(|entry| entry.name == sample.texture)
            .unwrap();
        let header = u32::from_le_bytes(entry.data[12..16].try_into().unwrap()) as usize;
        if request.channel == native_runtime::CaptureChannel::Hdr {
            for pixel in entry.data[header..].as_chunks_mut::<8>().0 {
                pixel[..2].copy_from_slice(&0x3c00u16.to_le_bytes());
                pixel[6..8].copy_from_slice(&0x3c00u16.to_le_bytes());
            }
        } else {
            for pixel in entry.data[header..].as_chunks_mut::<4>().0 {
                pixel[0] = 32;
                pixel[3] = 255;
            }
        }
    }
    captured
        .set_pak_data(&archive, PakLimits::default())
        .unwrap();
    Ok(CaptureReceipt {
        channel: request.channel,
        bsp: captured.to_bytes(),
        staged_bsp: staged,
        artifacts: "fake-native-capture-evidence".into(),
        elapsed_ms: 1,
        verified_controls: true,
        preservation_passed: true,
    })
}

fn enqueue_capture(
    fixture: &Fixture,
    queue: &QueueClient,
    play: bool,
    capture: crate::session::compilation::CaptureBackend,
) -> CompileHandle {
    let (reservation, mut submission) = fixture.prepare_with_play(queue, play);
    submission.request.capture_with(capture);
    queue.enqueue(reservation, submission, || {}).unwrap()
}

fn wait_until(check: impl Fn() -> bool) {
    let deadline = Instant::now() + TIMEOUT;
    while !check() {
        assert!(Instant::now() < deadline, "native owner did not finish");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn managed_run_blocks_peer_launch_and_reflections_but_ordinary_compilation_continues() {
    let mut queue = CompileQueue::isolated().unwrap();
    let client = queue.client();
    let claims = crate::path_claims::Authority::default();
    let a = Fixture::new(&queue, &claims);
    let b = Fixture::new(&queue, &claims);
    let c = Fixture::new_with_reflections(&queue, &claims, Some(false));
    let executable = a._directory.path().join("cstrike_linux64");
    let script = std::fs::read_to_string(&executable)
        .unwrap()
        .replace("sleep 0.1\nexit 23", "exec sleep 60");
    std::fs::write(executable, script).unwrap();
    wait(a.submit(&client, || {})).unwrap();
    LaunchSubmission::begin(a.document.session(), Actor::User)
        .unwrap()
        .run()
        .unwrap();
    let ordinary = wait(b.submit(&client, || {})).unwrap();
    assert!(ordinary.0.published());
    assert!(
        LaunchSubmission::begin(b.document.session(), Actor::Agent)
            .err()
            .unwrap()
            .contains("Native activity busy")
    );
    assert!(!lock(b.document.session()).launching());
    assert!(lock(b.document.session()).launch_readiness().is_ok());
    let captures = Arc::new(AtomicUsize::new(0));
    let observed = captures.clone();
    std::fs::write(&c.output, b"previous BSP").unwrap();
    let handle = enqueue_capture(
        &c,
        &client,
        false,
        Box::new(move |request, observer| {
            observed.fetch_add(1, Ordering::AcqRel);
            fake_capture(request, observer)
        }),
    );
    assert!(wait(handle).err().unwrap().contains("Native activity busy"));
    assert_eq!(captures.load(Ordering::Acquire), 0);
    assert_eq!(std::fs::read(&c.output).unwrap(), b"previous BSP");
    lock(a.document.session()).stop_game(Actor::User).unwrap();
    assert_eq!(
        lock(b.document.session()).native_activity_status()["available"],
        true
    );
    drop(ordinary);
    a.document.shutdown().unwrap();
    b.document.shutdown().unwrap();
    c.document.shutdown().unwrap();
    queue.shutdown().unwrap();
}

#[test]
fn waiting_reflective_submission_does_not_reserve_native_activity() {
    let mut queue = CompileQueue::isolated().unwrap();
    let client = queue.client();
    let claims = crate::path_claims::Authority::default();
    let a = Fixture::new(&queue, &claims);
    let b = Fixture::new_with_reflections(&queue, &claims, Some(false));
    let c = Fixture::new(&queue, &claims);
    wait(a.submit(&client, || {})).unwrap();
    let (started, ready) = mpsc::channel();
    let (release, wait_for_release) = mpsc::channel();
    let active = c.submit(&client, move || {
        started.send(()).unwrap();
        wait_for_release.recv_timeout(TIMEOUT).unwrap();
    });
    ready.recv_timeout(TIMEOUT).unwrap();
    let waiting = enqueue_capture(&b, &client, false, Box::new(fake_capture));
    assert_eq!(
        lock(b.document.session()).compile_status()["job"]["status"],
        "queued"
    );
    let pending = LaunchSubmission::begin(a.document.session(), Actor::User).unwrap();
    let activity = lock(b.document.session()).native_activity_status();
    assert_eq!(activity["owner"]["document_id"], a.document.id().value());
    drop(pending);
    release.send(()).unwrap();
    wait(active).unwrap();
    let result = wait(waiting).unwrap();
    assert!(result.0.published());
    assert_eq!(
        lock(b.document.session()).native_activity_status()["available"],
        true
    );
    a.document.shutdown().unwrap();
    b.document.shutdown().unwrap();
    c.document.shutdown().unwrap();
    queue.shutdown().unwrap();
}

#[test]
fn paired_capture_transfers_one_owner_to_play_after_releasing_compiler_reservation() {
    let mut queue = CompileQueue::isolated().unwrap();
    let client = queue.client();
    let claims = crate::path_claims::Authority::default();
    let a = Fixture::new_with_reflections(&queue, &claims, Some(true));
    let b = Fixture::new(&queue, &claims);
    let (capture_directory, _, _) = crate::test_assets::game_process(false);
    let steam = capture_directory.path().join("steam");
    std::fs::create_dir(&steam).unwrap();
    lock(a.document.session()).use_test_capture_environment(native_runtime::CaptureEnvironment {
        engine_root: capture_directory.path().to_owned(),
        steam_root: steam,
        mounts: vec![native_runtime::ContentMount {
            ids: vec!["game".into()],
            path: a._directory.path().to_owned(),
        }],
    });
    wait(b.submit(&client, || {})).unwrap();
    let (started, ready) = mpsc::channel();
    let (release, wait_for_release) = mpsc::channel();
    *queue.core.before_play.lock().unwrap() = Some(Box::new(move || {
        started.send(()).unwrap();
        wait_for_release.recv_timeout(TIMEOUT).unwrap();
    }));
    let captures = Arc::new(Mutex::new(Vec::new()));
    let captured = captures.clone();
    let a_session = a.document.session().clone();
    let b_session = b.document.session().clone();
    let capture_root = capture_directory.path().to_owned();
    let play_root = a._directory.path().to_owned();
    let handle = enqueue_capture(
        &a,
        &client,
        true,
        Box::new(move |request, observer| {
            let status = lock(&a_session).native_activity_status();
            assert_eq!(status["owner"]["phase"], "capture");
            assert_eq!(request.environment.engine_root, capture_root);
            assert_eq!(
                status["owner"]["targets"]["capture"]["engine"]["path"],
                capture_root.to_str().unwrap()
            );
            assert_eq!(
                status["owner"]["targets"]["play"]["engine"]["path"],
                play_root.to_str().unwrap()
            );
            captured
                .lock()
                .unwrap()
                .push((request.channel, status["owner"]["activity_id"].clone()));
            assert!(
                LaunchSubmission::begin(&b_session, Actor::Agent)
                    .err()
                    .unwrap()
                    .contains("Native activity busy")
            );
            fake_capture(request, observer)
        }),
    );
    ready.recv_timeout(TIMEOUT).unwrap();
    let channels = captures.lock().unwrap();
    assert_eq!(channels.len(), 2);
    assert_eq!(channels[0].0, native_runtime::CaptureChannel::Ldr);
    assert_eq!(channels[1].0, native_runtime::CaptureChannel::Hdr);
    assert_eq!(channels[0].1, channels[1].1);
    let activity = lock(a.document.session()).native_activity_status();
    assert_eq!(activity["owner"]["activity_id"], channels[0].1);
    assert_eq!(activity["owner"]["phase"], "play_startup");
    drop(channels);
    assert_eq!(a.document.active_operations(), 1);
    assert!(queue.core.state.lock().unwrap().records.is_empty());
    let peer = wait(b.submit(&client, || {})).unwrap();
    assert!(peer.0.published());
    assert_eq!(a.document.active_operations(), 1);
    release.send(()).unwrap();
    let retained = wait(handle).unwrap();
    assert!(retained.0.published() && retained.1.is_some());
    let log = retained.1.as_ref().unwrap();
    let message = log.native_activity.as_ref().unwrap()["message"]
        .as_str()
        .unwrap();
    assert!(log.text().contains(message));
    assert!(!log.text().contains("\"device\""));
    wait_until(|| !lock(a.document.session()).game_running());
    assert_eq!(
        lock(b.document.session()).native_activity_status()["available"],
        true
    );
    assert!(
        lock(a.document.session())
            .game_log(20)
            .unwrap()
            .console
            .contains("loaded ")
    );
    assert_eq!(a.document.active_operations(), 0);
    let pending = LaunchSubmission::begin(b.document.session(), Actor::User).unwrap();
    drop(retained);
    assert_eq!(
        lock(a.document.session()).native_activity_status()["owner"]["document_id"],
        b.document.id().value()
    );
    drop(pending);
    a.document.shutdown().unwrap();
    b.document.shutdown().unwrap();
    queue.shutdown().unwrap();
}

#[test]
fn capture_failure_uses_typed_cleanup_and_keeps_failed_native_drain_visible() {
    for disposition in [
        Some(CleanupDisposition::Confirmed),
        Some(CleanupDisposition::Unconfirmed),
        None,
    ] {
        let mut queue = CompileQueue::isolated().unwrap();
        let client = queue.client();
        let claims = crate::path_claims::Authority::default();
        let a = Fixture::new_with_reflections(&queue, &claims, Some(false));
        std::fs::write(&a.output, b"previous BSP").unwrap();
        let handle = enqueue_capture(
            &a,
            &client,
            false,
            Box::new(move |_, _| {
                let cleanup = disposition.expect("injected capture panic");
                Err(CaptureFailure {
                    error: std::io::Error::other("injected owned capture failure"),
                    cleanup,
                })
            }),
        );
        assert!(wait(handle).is_err());
        let clean = disposition == Some(CleanupDisposition::Confirmed);
        assert_eq!(
            lock(a.document.session()).native_activity_status()["available"],
            clean
        );
        assert_eq!(std::fs::read(&a.output).unwrap(), b"previous BSP");
        assert_eq!(a.document.shutdown().is_ok(), clean);
        queue.shutdown().unwrap();
    }
}

#[test]
fn failed_play_scheduling_retires_its_origin_and_preserves_the_successful_build() {
    let mut queue = CompileQueue::isolated().unwrap();
    let client = queue.client();
    let claims = crate::path_claims::Authority::default();
    let a = Fixture::new_with_reflections(&queue, &claims, Some(false));
    queue.core.fail_play_spawn.store(true, Ordering::Release);
    let handle = enqueue_capture(&a, &client, true, Box::new(fake_capture));
    assert!(
        wait(handle)
            .err()
            .unwrap()
            .contains("schedule optional Play")
    );
    let session = lock(a.document.session());
    assert_eq!(session.compile_status()["job"]["status"], "completed");
    assert_eq!(session.native_activity_status()["available"], true);
    assert!(!session.launching() && !session.game_running());
    assert!(session.launch_readiness().is_ok());
    drop(session);
    assert_eq!(a.document.active_operations(), 0);
    a.document.shutdown().unwrap();
    queue.shutdown().unwrap();
}

#[test]
fn cancellation_between_channels_cleans_capture_without_starting_hdr() {
    let mut queue = CompileQueue::isolated().unwrap();
    let client = queue.client();
    let claims = crate::path_claims::Authority::default();
    let a = Fixture::new_with_reflections(&queue, &claims, Some(true));
    std::fs::write(&a.output, b"previous BSP").unwrap();
    let session = a.document.session().clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let handle = enqueue_capture(
        &a,
        &client,
        false,
        Box::new(move |request, observer| {
            observed.fetch_add(1, Ordering::AcqRel);
            assert_eq!(request.channel, native_runtime::CaptureChannel::Ldr);
            let receipt = fake_capture(request, observer)?;
            let mut state = lock(&session);
            let token = state.compile_token().unwrap();
            state.cancel_compile(Actor::Agent, token).unwrap();
            Ok(receipt)
        }),
    );
    assert!(wait(handle).is_err());
    assert_eq!(calls.load(Ordering::Acquire), 1);
    assert_eq!(
        lock(a.document.session()).compile_status()["job"]["status"],
        "cancelled"
    );
    assert_eq!(
        lock(a.document.session()).native_activity_status()["available"],
        true
    );
    assert_eq!(std::fs::read(&a.output).unwrap(), b"previous BSP");
    a.document.shutdown().unwrap();
    queue.shutdown().unwrap();
}

#[test]
fn optional_play_busy_preserves_published_compile_and_frozen_preflight_failure() {
    use std::os::unix::fs::PermissionsExt;
    let mut queue = CompileQueue::isolated().unwrap();
    let client = queue.client();
    let claims = crate::path_claims::Authority::default();
    let a = Fixture::new(&queue, &claims);
    let b = Fixture::new(&queue, &claims);
    wait(a.submit(&client, || {})).unwrap();
    let pending = LaunchSubmission::begin(a.document.session(), Actor::User).unwrap();
    let (reservation, submission) = b.prepare_with_play(&client, true);
    let handle = client.enqueue(reservation, submission, || {}).unwrap();
    assert!(wait(handle).err().unwrap().contains("Native activity busy"));
    assert_eq!(
        lock(b.document.session()).compile_status()["job"]["status"],
        "completed"
    );
    assert!(lock(b.document.session()).launch_readiness().is_ok());
    drop(pending);
    let executable = b._directory.path().join("cstrike_linux64");
    let bytes = std::fs::read(&executable).unwrap();
    std::fs::remove_file(&executable).unwrap();
    let (reservation, submission) = b.prepare_with_play(&client, true);
    std::fs::write(&executable, bytes).unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
    let handle = client.enqueue(reservation, submission, || {}).unwrap();
    assert!(wait(handle).err().unwrap().contains("client not found"));
    assert_eq!(
        lock(b.document.session()).compile_status()["job"]["status"],
        "completed"
    );
    assert!(lock(b.document.session()).launch_readiness().is_ok());
    assert_eq!(
        lock(b.document.session()).native_activity_status()["available"],
        true
    );
    assert!(!lock(b.document.session()).launching());
    a.document.shutdown().unwrap();
    b.document.shutdown().unwrap();
    queue.shutdown().unwrap();
}

#[test]
fn dropped_play_waiter_and_document_drain_keep_owned_startup_until_cancelled_cleanup() {
    let mut queue = CompileQueue::isolated().unwrap();
    let client = queue.client();
    let claims = crate::path_claims::Authority::default();
    let a = Fixture::new_with_reflections(&queue, &claims, Some(false));
    let (started, ready) = mpsc::channel();
    let (release, wait_for_release) = mpsc::channel();
    *queue.core.before_play.lock().unwrap() = Some(Box::new(move || {
        started.send(()).unwrap();
        wait_for_release.recv_timeout(TIMEOUT).unwrap();
    }));
    let handle = enqueue_capture(&a, &client, true, Box::new(fake_capture));
    ready.recv_timeout(TIMEOUT).unwrap();
    let activity =
        lock(a.document.session()).native_activity_status()["owner"]["activity_id"].clone();
    drop(handle);
    assert_eq!(
        lock(a.document.session()).native_activity_status()["owner"]["activity_id"],
        activity
    );
    assert_eq!(a.document.active_operations(), 1);
    let draining = a.document.clone();
    let (done, completed) = mpsc::channel();
    let worker = std::thread::spawn(move || done.send(draining.shutdown()).unwrap());
    wait_until(|| {
        a.document.lifecycle() == crate::runtime::DocumentLifecycle::Draining
            && lock(a.document.session()).compile_settings()["editable"] == false
    });
    assert!(completed.try_recv().is_err());
    assert_eq!(
        lock(a.document.session()).native_activity_status()["owner"]["activity_id"],
        activity
    );
    release.send(()).unwrap();
    completed.recv_timeout(TIMEOUT).unwrap().unwrap();
    worker.join().unwrap();
    assert_eq!(a.document.active_operations(), 0);
    assert_eq!(
        lock(a.document.session()).native_activity_status()["available"],
        true
    );
    assert!(!lock(a.document.session()).game_running());
    assert!(!a._directory.path().join("cstrike/downloadlists").exists());
    queue.shutdown().unwrap();
}

#[test]
fn raw_managed_reflective_request_cannot_bypass_shared_submission() {
    let mut queue = CompileQueue::isolated().unwrap();
    let claims = crate::path_claims::Authority::default();
    let a = Fixture::new_with_reflections(&queue, &claims, Some(false));
    let request = lock(a.document.session())
        .begin_compile(Actor::Agent, Some(&a.output))
        .unwrap();
    let result = request.run();
    assert!(
        result
            .err()
            .unwrap()
            .contains("requires the shared compile submission")
    );
    assert_eq!(
        lock(a.document.session()).native_activity_status()["available"],
        true
    );
    assert!(!a.output.exists());
    a.document.shutdown().unwrap();
    queue.shutdown().unwrap();
}

#[test]
fn completion_panic_keeps_native_permit_until_pending_publication_cleanup_retires() {
    let mut queue = CompileQueue::isolated().unwrap();
    let client = queue.client();
    let claims = crate::path_claims::Authority::default();
    let a = Fixture::new_with_reflections(&queue, &claims, Some(false));
    let b = Fixture::new(&queue, &claims);
    wait(b.submit(&client, || {})).unwrap();
    std::fs::write(&a.output, b"previous BSP").unwrap();
    let (started, ready) = mpsc::channel();
    let (release, cleanup_release) = mpsc::channel();
    *queue.core.before_completion.lock().unwrap() = Some(Box::new(move |result| {
        result.as_ref().unwrap().before_cleanup(move || {
            let _ = started.send(());
            let _ = cleanup_release.recv_timeout(TIMEOUT);
        });
        panic!("injected exact completion panic");
    }));
    let handle = enqueue_capture(&a, &client, false, Box::new(fake_capture));
    ready.recv_timeout(TIMEOUT).unwrap();
    assert_eq!(
        lock(a.document.session()).native_activity_status()["owner"]["phase"],
        "capture"
    );
    assert!(lock(a.document.session()).compiling());
    assert_eq!(a.document.active_operations(), 1);
    assert_eq!(queue.core.state.lock().unwrap().records.len(), 1);
    assert!(
        LaunchSubmission::begin(b.document.session(), Actor::Agent)
            .err()
            .unwrap()
            .contains("Native activity busy")
    );
    assert!(
        lock(b.document.session())
            .save(Actor::Agent, Some(&a.output))
            .is_err()
    );
    assert_eq!(std::fs::read(&a.output).unwrap(), b"previous BSP");
    release.send(()).unwrap();
    assert!(wait(handle).err().unwrap().contains("submission panicked"));
    assert_eq!(a.document.active_operations(), 0);
    assert!(!lock(a.document.session()).compiling());
    assert_eq!(
        lock(a.document.session()).native_activity_status()["available"],
        true
    );
    assert_eq!(std::fs::read(&a.output).unwrap(), b"previous BSP");
    let pending = LaunchSubmission::begin(b.document.session(), Actor::User).unwrap();
    drop(pending);
    wait(b.submit(&client, || {})).unwrap();
    a.document.shutdown().unwrap();
    b.document.shutdown().unwrap();
    queue.shutdown().unwrap();
}
