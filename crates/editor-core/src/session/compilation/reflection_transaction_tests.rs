use super::*;
use source_formats::bsp::Bsp;
use source_formats::gamefs::{Mount, MountedPath};

fn prepared(leak: bool) -> (tempfile::TempDir, Session, CompileResult) {
    prepared_source(leak, true)
}

fn prepared_source(leak: bool, named: bool) -> (tempfile::TempDir, Session, CompileResult) {
    let (directory, mut game, _) = crate::test_assets::game_process(false);
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().to_owned()),
    });
    let source_path = directory.path().join("room.vmf");
    let destination = directory.path().join("room.bsp");
    if named {
        std::fs::write(&source_path, b"source VMF identity").unwrap();
    }
    std::fs::write(&destination, b"previous BSP").unwrap();
    std::fs::write(destination.with_extension("lin"), b"previous pointfile").unwrap();
    let mut session = Session::new(Ok(game));
    session.replace_document(
        Document::new(crate::test_assets::room()),
        named.then(|| source_path.clone()),
    );
    let request = session
        .begin_compile(Actor::User, Some(&destination))
        .unwrap();
    session.compilation.options.lighting = compiler::LightingMode::DirectLdr;
    session.compilation.options.generate_reflections = true;
    session.compilation.job.as_mut().unwrap().options = session.compilation.options;
    let control = request.control;
    let destination_stamp = compiler::FileStamp::capture(&destination).unwrap();
    let lease = compiler::OutputLease::acquire(&destination, &control).unwrap();
    let pointfile_lease = leak.then(|| {
        compiler::OutputLease::acquire(&destination.with_extension("lin"), &control).unwrap()
    });
    let source = named
        .then(|| compiler::FileIdentity::capture(&source_path, &control))
        .transpose()
        .unwrap();
    let output = compiler::Output {
        bsp: (!leak).then(Bsp::new),
        report: Default::default(),
        leak: leak.then(|| compiler::Leak {
            entity: 1,
            points: vec![DVec3::ZERO, DVec3::ONE],
        }),
        stats: Default::default(),
        cubemap_assets: None,
    };
    let pending = output
        .prepare_write_controlled(&destination, &control)
        .unwrap();
    session.compiled = Some(CompiledMap {
        path: destination.clone(),
        snapshot: Snapshot {
            generation: session.document_generation,
            revision: session.document.revision(),
        },
        options: session.compilation.options,
        content_epoch: session.content_epoch,
    });
    let result = CompileResult {
        token: request.token,
        output,
        path: destination,
        generation: session.document_generation,
        revision: session.document.revision(),
        options: session.compilation.options,
        content_epoch: session.content_epoch,
        source,
        destination_stamp: Some(destination_stamp),
        control,
        publication: Mutex::new(Publication {
            pending,
            lease: Some(lease),
            pointfile_lease,
            sources: None,
            published: false,
            committed: false,
            error: None,
        }),
    };
    (directory, session, result)
}

#[test]
fn save_as_during_reflection_capture_preserves_previous_outputs() {
    for actor in [Actor::User, Actor::Agent] {
        for named in [false, true] {
            for leak in [false, true] {
                let (directory, mut session, result) = prepared_source(leak, named);
                let source = directory.path().join("renamed.vmf");
                let generation = session.document_generation;
                let revision = session.document.revision();
                session.save(actor, Some(&source)).unwrap();
                let saved = std::fs::read(&source).unwrap();
                assert_eq!(session.path.as_ref(), Some(&source));
                assert_eq!(session.document_generation, generation);
                assert_eq!(session.document.revision(), revision);
                if let Some(identity) = &result.source {
                    identity.verify_metadata().unwrap();
                }
                let outcome = Ok(result);
                session.finish_compile(actor, &outcome);
                let result = outcome.as_ref().unwrap();
                assert!(!result.published());
                assert!(
                    result
                        .publication_error()
                        .unwrap()
                        .contains("source path changed")
                );
                assert_eq!(std::fs::read(&result.path).unwrap(), b"previous BSP");
                assert_eq!(
                    std::fs::read(result.path.with_extension("lin")).unwrap(),
                    b"previous pointfile"
                );
                assert_eq!(std::fs::read(&source).unwrap(), saved);
                assert_eq!(session.compiled.as_ref().unwrap().path, result.path);
                assert!(!session.compiling());
                let status = session.compile_status();
                assert_eq!(status["job"]["status"], "failed");
                assert_eq!(status["job"]["result"]["published"], false);
                assert_eq!(status["job"]["result"]["success"], false);
                assert_eq!(status["job"]["result"]["bsp"], Value::Null);
            }
        }
    }
}

#[test]
fn deferred_outputs_publish_only_for_the_current_job() {
    for change in [
        "revision",
        "generation",
        "content",
        "settings",
        "closing",
        "cancel",
        "source",
        "destination",
        "token",
    ] {
        let (directory, mut session, result) = prepared(false);
        assert_eq!(std::fs::read(&result.path).unwrap(), b"previous BSP");
        match change {
            "revision" => {
                session
                    .edit(Actor::Agent, "change map", |tx| {
                        tx.world_properties_mut().set("message", "changed");
                        Ok(())
                    })
                    .unwrap();
            }
            "generation" => session.document_generation += 1,
            "content" => session.content_epoch += 1,
            "settings" => session.compilation.options.bounces = 1,
            "closing" => session.closing = true,
            "cancel" => result.control.cancel().unwrap(),
            "source" => {
                std::fs::write(directory.path().join("room.vmf"), b"external source change")
                    .unwrap()
            }
            "destination" => std::fs::write(&result.path, b"external BSP").unwrap(),
            "token" => session.compilation.job.as_mut().unwrap().token += 1,
            _ => unreachable!(),
        }
        let expected = std::fs::read(&result.path).unwrap();
        let outcome = Ok(result);
        session.finish_compile(Actor::User, &outcome);
        let result = outcome.as_ref().unwrap();
        assert!(!result.published(), "{change}");
        assert_eq!(std::fs::read(&result.path).unwrap(), expected, "{change}");
        assert_eq!(session.compiled.as_ref().unwrap().path, result.path);
        if change == "token" {
            assert!(session.compiling());
            assert_eq!(session.compile_status()["job"]["status"], "running");
        } else {
            assert!(result.publication_error().is_some(), "{change}");
            assert!(!session.compiling());
        }
    }
}

#[test]
fn accepted_completion_commits_once_and_keeps_no_pending_artifact() {
    let (_directory, mut session, result) = prepared(false);
    let outcome = Ok(result);
    session.finish_compile(Actor::User, &outcome);
    let result = outcome.as_ref().unwrap();
    assert!(result.published());
    assert!(Bsp::parse(&std::fs::read(&result.path).unwrap()).is_ok());
    assert!(result.publication.lock().unwrap().pending.is_none());
    assert!(result.publication.lock().unwrap().lease.is_none());
    session.finish_compile(Actor::User, &outcome);
    assert!(result.published());
}

#[test]
fn pointfiles_use_the_same_deferred_gate_and_a_separate_lease() {
    for change in ["none", "content", "cancel", "pointfile"] {
        let (_directory, mut session, result) = prepared(true);
        let pointfile = result.path.with_extension("lin");
        assert_eq!(std::fs::read(&pointfile).unwrap(), b"previous pointfile");
        match change {
            "content" => session.content_epoch += 1,
            "cancel" => result.control.cancel().unwrap(),
            "pointfile" => std::fs::write(&pointfile, b"external pointfile").unwrap(),
            _ => {}
        }
        let previous = std::fs::read(&pointfile).unwrap();
        let outcome = Ok(result);
        session.finish_compile(Actor::User, &outcome);
        let result = outcome.as_ref().unwrap();
        assert!(!result.published());
        assert_eq!(std::fs::read(&result.path).unwrap(), b"previous BSP");
        if change == "none" {
            assert_eq!(
                std::fs::read_to_string(&pointfile).unwrap(),
                "0 0 0\n1 1 1\n"
            );
            assert!(result.publication_error().is_none());
            assert!(result.publication.lock().unwrap().committed);
        } else {
            assert_eq!(std::fs::read(&pointfile).unwrap(), previous);
            assert!(result.publication_error().is_some());
        }
    }
}

#[test]
fn backing_asset_replacement_after_preparation_rejects_publication() {
    let (directory, mut session, result) = prepared(false);
    let asset = directory.path().join("source.vmt");
    std::fs::write(&asset, b"original asset").unwrap();
    let manifest = compiler::CubemapAssetManifest {
        version: 1,
        phase: compiler::CubemapAssetPhase::Verified,
        logical_namespace: "room".into(),
        namespace: "room".into(),
        channel: "ldr".into(),
        samples: vec![],
        hdr_samples: vec![],
        assignments: vec![],
        aliases: vec![],
        missing_sources: vec![],
        default_texture: None,
        hdr_default_texture: None,
        texinfo_bindings: vec![],
        face_bindings: vec![],
        sources: vec![compiler::cubemap_assets::CubemapSourceAsset {
            path: "source.vmt".into(),
            sha256: native_runtime::fingerprint(&asset).unwrap(),
        }],
    };
    let guard = compiler::cubemap_assets::capture_reflection_source_guard(
        &manifest,
        &session.game,
        &result.control,
    )
    .unwrap();
    guard.verify_metadata().unwrap();
    result.publication.lock().unwrap().sources = Some(guard);
    std::fs::write(&asset, b"changed asset").unwrap();
    let outcome = Ok(result);
    session.finish_compile(Actor::User, &outcome);
    let result = outcome.as_ref().unwrap();
    assert!(!result.published());
    assert!(result.publication_error().is_some());
    assert_eq!(std::fs::read(&result.path).unwrap(), b"previous BSP");
}
