use super::*;
use crate::runtime::{DocumentHandle, lock};

fn cancellation_fixture(instances: bool) -> (tempfile::TempDir, Session, PathBuf) {
    let (directory, game, source) = if instances {
        let (directory, game, source, _) = crate::test_assets::instance_map(false);
        (directory, game, source)
    } else {
        let (directory, game) = crate::test_assets::game();
        let source = directory.path().join("lighting-cancellation.vmf");
        std::fs::write(
            &source,
            crate::test_assets::room().write(&Default::default()),
        )
        .unwrap();
        (directory, game, source)
    };
    let mut vmf =
        source_formats::vmf::Vmf::parse(&std::fs::read_to_string(&source).unwrap()).unwrap();
    let id = vmf.object_ids().chain(vmf.side_ids()).max().unwrap() + 1;
    let mut light = source_formats::vmf::Entity::new(id, "light");
    light.properties.set("origin", "0 0 128");
    light.properties.set("_light", "255 255 255 200");
    vmf.entities.push(light);
    std::fs::write(&source, vmf.write(&Default::default())).unwrap();
    std::fs::write(source.with_extension("bsp"), b"previous BSP").unwrap();
    std::fs::write(source.with_extension("lin"), b"previous pointfile").unwrap();
    let mut session = Session::new(Ok(game));
    session.open(Actor::User, &source, false).unwrap();
    (directory, session, source)
}

#[test]
fn lighting_work_jobs_freeze_exact_units_and_keep_terminal_settings() {
    for actor in [Actor::User, Actor::Agent] {
        let (directory, game) = crate::test_assets::game();
        let mut session = Session::new(Ok(game));
        session.replace_document(Document::new(crate::test_assets::room()), None);
        let limit = 1_234_567_890;
        session
            .update_build_settings(
                actor,
                BuildSettingsUpdate {
                    lighting_work_limit: Some(limit),
                    ..Default::default()
                },
            )
            .unwrap();
        let output = directory.path().join("frozen-lighting.bsp");
        let request = session.begin_compile(actor, Some(&output)).unwrap();
        assert_eq!(request.options.lighting_work_limit, limit);
        let frozen = session.compile_status();
        assert_eq!(frozen["job"]["settings"]["lighting_work_limit"], limit);
        assert_eq!(frozen["job"]["stale"], false);
        assert!(
            session
                .update_build_settings(
                    actor,
                    BuildSettingsUpdate {
                        lighting_work_limit: Some(compiler::MAX_LIGHTING_WORK_LIMIT),
                        ..Default::default()
                    }
                )
                .is_err()
        );
        assert_eq!(
            session.compile_status()["job"]["settings"],
            frozen["job"]["settings"]
        );
        session
            .cancel_compile(actor, session.compile_token().unwrap())
            .unwrap();
        let result = request.run();
        session.finish_compile(actor, &result);
        assert!(result.is_err());
        session
            .update_build_settings(
                actor,
                BuildSettingsUpdate {
                    lighting_work_limit: Some(compiler::MAX_LIGHTING_WORK_LIMIT),
                    ..Default::default()
                },
            )
            .unwrap();
        let terminal = session.compile_status();
        assert_eq!(terminal["job"]["settings"]["lighting_work_limit"], limit);
        assert_eq!(
            terminal["settings"]["lighting_work_limit"],
            compiler::MAX_LIGHTING_WORK_LIMIT
        );
        assert_eq!(terminal["job"]["stale"], true);
    }
}

#[test]
fn lighting_work_post_compile_cancellation_retains_shared_ledger_without_publication() {
    for instances in [false, true] {
        for limit in [
            compiler::MIN_LIGHTING_WORK_LIMIT,
            compiler::DEFAULT_LIGHTING_WORK_LIMIT,
        ] {
            let (_directory, mut session, source) = cancellation_fixture(instances);
            let authority = crate::path_claims::Authority::default();
            session.file_claims.source = None;
            session.use_application_path_claims(authority.clone());
            let previous = session.begin_compile(Actor::User, None).unwrap().run();
            session.finish_compile(Actor::User, &previous);
            assert!(previous.as_ref().unwrap().published());
            drop(previous);
            session
                .update_build_settings(
                    Actor::Agent,
                    BuildSettingsUpdate {
                        lighting: Some("direct_ldr"),
                        lighting_work_limit: Some(limit),
                        ..Default::default()
                    },
                )
                .unwrap();
            let source_bytes = std::fs::read(&source).unwrap();
            let bsp = source.with_extension("bsp");
            let pointfile = source.with_extension("lin");
            let bsp_bytes = std::fs::read(&bsp).unwrap();
            let pointfile_bytes = std::fs::read(&pointfile).unwrap();
            let vmf = session.document().vmf().clone();
            let history = session.history_snapshot();
            let shared = Arc::new(Mutex::new(session));
            let document = DocumentHandle::start(shared.clone()).unwrap();
            let operation = document.admit().unwrap();
            assert_eq!(document.active_operations(), 1);
            let mut request = lock(&shared).begin_compile(Actor::Agent, None).unwrap();
            let origin = request.origin();
            let control = request.control();
            request.after_compilation = Some(Box::new(move |output| {
                let progress = output.stats.lighting_progress.as_ref().unwrap();
                assert_eq!(
                    progress.completed,
                    limit != compiler::MIN_LIGHTING_WORK_LIMIT
                );
                assert_eq!(output.bsp.is_some(), progress.completed);
                assert_eq!(output.instance_dependencies.is_some(), instances);
                assert!(progress.work > 0);
                control.cancel().unwrap();
            }));
            let result = request.run();
            let retained = result.as_ref().unwrap();
            assert!(retained.output.bsp.is_none());
            assert!(retained.output.leak.is_none());
            assert_eq!(retained.output.instance_dependencies.is_some(), instances);
            assert!(origin.busy());
            assert!(!retained.published());
            assert_eq!(
                retained.publication_error().as_deref(),
                Some("Compilation cancelled")
            );
            {
                let publication = retained.publication.lock().unwrap();
                assert!(!publication.published && !publication.committed);
                assert!(publication.pending.is_none());
                assert!(publication.claims.is_none());
                assert!(publication.artifact.is_none());
                assert!(publication.lease.is_none());
                assert!(publication.pointfile_lease.is_none());
                assert!(publication.sources.is_none());
                assert!(publication.ticket.is_some());
            }
            let reported = result_json(retained);
            assert_eq!(reported["success"], false);
            assert_eq!(reported["published"], false);
            assert!(reported["bsp"].is_null());
            assert!(reported["leak"].is_null());
            assert_eq!(reported["error"], "Compilation cancelled");
            assert_eq!(
                reported["stats"]["lighting_progress"]["completed"],
                limit != compiler::MIN_LIGHTING_WORK_LIMIT
            );
            assert_eq!(reported["stats"]["lighting_progress"]["limit"], limit);
            assert!(
                reported["stats"]["lighting_progress"]["work"]
                    .as_u64()
                    .unwrap()
                    > 0
            );
            {
                let mut session = lock(&shared);
                assert!(session.finish_compile_for_origin(Actor::Agent, &origin, &result));
                let status = session.compile_status();
                assert_eq!(status["job"]["status"], "cancelled");
                assert_eq!(status["job"]["result"], reported);
                assert_eq!(status["launch"]["ready"], false);
                assert!(!session.compiling());
                assert!(!session.launching());
                assert!(!origin.busy());
                assert!(retained.publication.lock().unwrap().ticket.is_none());
                assert_eq!(session.document().vmf(), &vmf);
                assert_eq!(session.history_snapshot(), history);
                let previous = session.compiled.as_ref().unwrap();
                assert_eq!(
                    previous.options.lighting,
                    compiler::LightingMode::Fullbright
                );
                assert!(previous.claim.is_some());
                session
                    .file_claims
                    .source
                    .as_ref()
                    .unwrap()
                    .destination()
                    .verify()
                    .unwrap();
            }
            let lease =
                compiler::OutputLease::acquire(&bsp, &compiler::Control::default()).unwrap();
            drop(lease);
            let mut peer = crate::path_claims::Binding::default();
            peer.attach(authority);
            let destinations = (
                source_formats::file_publication::BoundDestination::capture(
                    &bsp,
                    source_formats::file_publication::Policy::ReplaceEntry,
                )
                .unwrap(),
                source_formats::file_publication::BoundDestination::capture(
                    &pointfile,
                    source_formats::file_publication::Policy::ReplaceEntry,
                )
                .unwrap(),
            );
            assert!(
                peer.outputs(destinations.0.clone(), destinations.1.clone())
                    .is_err()
            );
            lock(&shared).compiled = None;
            let claims = peer.outputs(destinations.0, destinations.1).unwrap();
            drop(claims);
            assert_eq!(std::fs::read(&source).unwrap(), source_bytes);
            assert_eq!(std::fs::read(&bsp).unwrap(), bsp_bytes);
            assert_eq!(std::fs::read(&pointfile).unwrap(), pointfile_bytes);
            drop(operation);
            assert_eq!(document.active_operations(), 0);
            document.shutdown().unwrap();
        }
    }
}

#[test]
fn lighting_work_post_compile_cancellation_without_a_ledger_preserves_error_contract() {
    let (_directory, mut session, source) = cancellation_fixture(false);
    let source_bytes = std::fs::read(&source).unwrap();
    let bsp_bytes = std::fs::read(source.with_extension("bsp")).unwrap();
    let pointfile_bytes = std::fs::read(source.with_extension("lin")).unwrap();
    let history = session.history_snapshot();
    let mut request = session.begin_compile(Actor::User, None).unwrap();
    let origin = request.origin();
    let control = request.control();
    request.after_compilation = Some(Box::new(move |output| {
        assert!(output.bsp.is_some());
        assert!(output.stats.lighting_progress.is_none());
        control.cancel().unwrap();
    }));
    let result = request.run();
    assert_eq!(
        result.as_ref().err().map(String::as_str),
        Some("Compilation cancelled")
    );
    assert!(!origin.busy());
    session.finish_compile(Actor::User, &result);
    assert_eq!(session.compile_status()["job"]["status"], "cancelled");
    assert!(session.compile_status()["job"]["result"]["stats"].is_null());
    assert!(!session.compiling());
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(
        std::fs::read(source.with_extension("bsp")).unwrap(),
        bsp_bytes
    );
    assert_eq!(
        std::fs::read(source.with_extension("lin")).unwrap(),
        pointfile_bytes
    );
    assert_eq!(std::fs::read(&source).unwrap(), source_bytes);
}
