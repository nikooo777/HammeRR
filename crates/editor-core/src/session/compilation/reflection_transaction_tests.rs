use super::*;
use source_formats::bsp::Bsp;
use source_formats::gamefs::{Mount, MountedPath};

const ASSET_MODES: [(bool, bool); 3] = [(true, false), (false, true), (true, true)];

fn prepared(leak: bool) -> (tempfile::TempDir, Session, CompileResult) {
    prepared_source(leak, true)
}

fn prepared_source(leak: bool, named: bool) -> (tempfile::TempDir, Session, CompileResult) {
    prepared_assets(leak, named, true, false)
}

fn prepared_assets(
    leak: bool,
    named: bool,
    reflections: bool,
    packing: bool,
) -> (tempfile::TempDir, Session, CompileResult) {
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
    let mut vmf = crate::test_assets::room();
    if packing {
        write_packed_sky(directory.path());
        vmf.world.properties.set("skyname", "portable_test");
    }
    let mut session = Session::new(Ok(game));
    session.replace_document(Document::new(vmf), named.then(|| source_path.clone()));
    let request = session
        .begin_compile(Actor::User, Some(&destination))
        .unwrap();
    session.compilation.options.lighting = compiler::LightingMode::DirectLdr;
    session.compilation.options.generate_reflections = reflections;
    session.compilation.options.pack_sky_assets = packing;
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
    let reservation = compiler::WriteReservation::new(&destination, &control).unwrap();
    let output = if packing && !leak {
        let output = compiler::compile_with_options(
            session.document.vmf(),
            &session.game,
            compiler::Options {
                lighting: session.compilation.options.lighting,
                pack_sky_assets: true,
                ..Default::default()
            },
        );
        assert!(output.bsp.is_some(), "{:?}", output.report);
        assert!(output.sky_assets.is_some());
        output
    } else {
        compiler::Output {
            bsp: (!leak).then(Bsp::new),
            report: Default::default(),
            leak: leak.then(|| compiler::Leak {
                entity: 1,
                points: vec![DVec3::ZERO, DVec3::ONE],
            }),
            stats: Default::default(),
            cubemap_assets: None,
            sky_assets: None,
        }
    };
    let pending = output
        .prepare_write_reserved(reservation, &control)
        .unwrap();
    let sources = compiler::asset_sources::capture_output_asset_source_guard(
        &output,
        &session.game,
        &control,
    )
    .unwrap();
    session.compiled = Some(CompiledMap {
        path: destination.clone(),
        snapshot: Snapshot {
            generation: session.document_generation,
            revision: session.document.revision(),
        },
        options: session.compilation.options,
        content_epoch: session.content_epoch,
        lightmap_identity: baked_lightmaps::BuildIdentity::capture(0, &destination, Ok(None), None),
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
            sources: Some(sources),
            published: false,
            committed: false,
            error: None,
        }),
    };
    (directory, session, result)
}

#[test]
fn save_as_during_reflection_capture_preserves_previous_outputs() {
    for (reflections, packing) in ASSET_MODES {
        for actor in [Actor::User, Actor::Agent] {
            for named in [false, true] {
                for leak in [false, true] {
                    let (directory, mut session, result) =
                        prepared_assets(leak, named, reflections, packing);
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
}

#[test]
fn deferred_outputs_publish_only_for_the_current_job() {
    for (reflections, packing) in ASSET_MODES {
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
            let (directory, mut session, result) =
                prepared_assets(false, true, reflections, packing);
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
}

#[test]
fn accepted_completion_commits_once_and_keeps_no_pending_artifact() {
    for (reflections, packing) in ASSET_MODES {
        let (_directory, mut session, result) = prepared_assets(false, true, reflections, packing);
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
}

#[test]
fn pointfiles_use_the_same_deferred_gate_and_a_separate_lease() {
    for (reflections, packing) in ASSET_MODES {
        for change in ["none", "content", "cancel", "pointfile"] {
            let (_directory, mut session, result) =
                prepared_assets(true, true, reflections, packing);
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
}

#[test]
fn backing_asset_replacement_after_preparation_rejects_publication() {
    let (directory, mut session, mut result) = prepared(false);
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
    result.output.cubemap_assets = Some(manifest);
    let guard = compiler::asset_sources::capture_output_asset_source_guard(
        &result.output,
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

fn write_packed_sky(root: &Path) {
    let skybox = root.join("materials/skybox");
    std::fs::create_dir_all(&skybox).unwrap();
    for suffix in ["rt", "bk", "lf", "ft", "up", "dn"] {
        std::fs::write(
            skybox.join(format!("portable_test{suffix}.vmt")),
            "UnlitGeneric { $basetexture skybox/portable_shared }",
        )
        .unwrap();
    }
    let mut texture = vec![0; 80];
    texture[..4].copy_from_slice(b"VTF\0");
    texture[4..8].copy_from_slice(&7u32.to_le_bytes());
    texture[8..12].copy_from_slice(&2u32.to_le_bytes());
    texture[12..16].copy_from_slice(&80u32.to_le_bytes());
    texture[16..18].copy_from_slice(&1u16.to_le_bytes());
    texture[18..20].copy_from_slice(&1u16.to_le_bytes());
    texture[24..26].copy_from_slice(&1u16.to_le_bytes());
    texture[56] = 1;
    texture[57..61].copy_from_slice(&(-1i32).to_le_bytes());
    texture[63..65].copy_from_slice(&1u16.to_le_bytes());
    texture.extend_from_slice(&[20, 60, 180, 255]);
    std::fs::write(skybox.join("portable_shared.vtf"), texture).unwrap();
}

fn packed_fixture(leak: bool, named: bool) -> (tempfile::TempDir, Session, PathBuf) {
    let (directory, mut game, _) = crate::test_assets::game_process(false);
    write_packed_sky(directory.path());
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().to_owned()),
    });
    let source = directory.path().join("room.vmf");
    if named {
        std::fs::write(&source, b"source VMF identity").unwrap();
    }
    let destination = directory.path().join("room (portable sky).bsp");
    std::fs::write(&destination, b"previous BSP").unwrap();
    std::fs::write(destination.with_extension("lin"), b"previous pointfile").unwrap();
    let mut vmf = crate::test_assets::room();
    vmf.world.properties.set("skyname", "portable_test");
    if leak {
        vmf.world.solids.pop();
    }
    let mut session = Session::new(Ok(game));
    session.replace_document(Document::new(vmf), named.then_some(source));
    session.compilation.options.pack_sky_assets = true;
    (directory, session, destination)
}

#[test]
fn packed_output_in_a_content_mount_uses_no_native_runtime_and_commits_once() {
    for actor in [Actor::User, Actor::Agent] {
        for named in [false, true] {
            let (_directory, mut session, destination) = packed_fixture(false, named);
            let pointfile = destination.with_extension("lin");
            std::fs::remove_file(&pointfile).unwrap();
            std::fs::create_dir(&pointfile).unwrap();
            session
                .edit(actor, "unsaved map edit", |tx| {
                    tx.world_properties_mut().set("message", "unsaved");
                    Ok(())
                })
                .unwrap();
            let vmf = session.document.vmf().clone();
            let revision = session.document.revision();
            let history: Vec<_> = session.document.undo_labels().map(str::to_owned).collect();
            let request = session.begin_compile(actor, Some(&destination)).unwrap();
            assert!(request.context.is_none() && request.environment.is_none());
            assert_eq!(request.source_stamp.is_some(), named);
            assert!(request.destination_stamp.is_some());
            let result = request.run().unwrap();
            assert!(result.output.bsp.is_some(), "{:?}", result.output.report);
            assert!(result.output.sky_assets.is_some());
            assert!(result.output.cubemap_assets.is_none());
            assert!(!result.published());
            assert_eq!(std::fs::read(&destination).unwrap(), b"previous BSP");
            result
                .publication
                .lock()
                .unwrap()
                .sources
                .as_ref()
                .unwrap()
                .verify_metadata()
                .unwrap();
            let outcome = Ok(result);
            session.finish_compile(actor, &outcome);
            let result = outcome.as_ref().unwrap();
            assert!(result.published(), "{:?}", result.publication_error());
            assert!(pointfile.is_dir());
            assert_eq!(session.document.vmf(), &vmf);
            assert_eq!(session.document.revision(), revision);
            assert_eq!(
                session
                    .document
                    .undo_labels()
                    .map(str::to_owned)
                    .collect::<Vec<_>>(),
                history
            );
            let bytes = std::fs::read(&destination).unwrap();
            assert!(
                compiler::sky_assets::read_packed_sky(
                    &Bsp::parse(&bytes).unwrap(),
                    &compiler::Control::default()
                )
                .unwrap()
                .is_some()
            );
            session.finish_compile(actor, &outcome);
            assert_eq!(std::fs::read(&destination).unwrap(), bytes);
            assert!(result.publication.lock().unwrap().pending.is_none());
            assert!(result.publication.lock().unwrap().lease.is_none());
        }
    }
}

#[test]
fn packed_worker_preserves_leak_outputs_until_the_session_accepts_them() {
    for cancel in [false, true] {
        let (_directory, mut session, destination) = packed_fixture(true, true);
        let result = session
            .begin_compile(Actor::Agent, Some(&destination))
            .unwrap()
            .run()
            .unwrap();
        assert!(result.output.bsp.is_none() && result.output.leak.is_some());
        assert!(result.output.sky_assets.is_none());
        assert!(result.publication.lock().unwrap().pointfile_lease.is_some());
        let pointfile = destination.with_extension("lin");
        assert_eq!(std::fs::read(&pointfile).unwrap(), b"previous pointfile");
        if cancel {
            result.control.cancel().unwrap();
        }
        let outcome = Ok(result);
        session.finish_compile(Actor::Agent, &outcome);
        let result = outcome.as_ref().unwrap();
        assert!(!result.published());
        assert_eq!(std::fs::read(&destination).unwrap(), b"previous BSP");
        if cancel {
            assert!(result.publication_error().is_some());
            assert_eq!(std::fs::read(&pointfile).unwrap(), b"previous pointfile");
        } else {
            assert!(result.publication_error().is_none());
            assert!(result.publication.lock().unwrap().committed);
            assert_ne!(std::fs::read(&pointfile).unwrap(), b"previous pointfile");
        }
    }
}

#[test]
fn packed_source_replacement_after_worker_preparation_rejects_publication() {
    let (directory, mut session, destination) = packed_fixture(false, true);
    let result = session
        .begin_compile(Actor::Agent, Some(&destination))
        .unwrap()
        .run()
        .unwrap();
    assert!(result.output.sky_assets.is_some());
    std::fs::write(
        directory
            .path()
            .join("materials/skybox/portable_testrt.vmt"),
        "UnlitGeneric { $basetexture skybox/replaced }",
    )
    .unwrap();
    let outcome = Ok(result);
    session.finish_compile(Actor::Agent, &outcome);
    let result = outcome.as_ref().unwrap();
    assert!(!result.published());
    assert!(result.publication_error().is_some());
    assert_eq!(std::fs::read(&destination).unwrap(), b"previous BSP");
}

#[test]
fn guarded_commit_requires_retained_sky_plan_and_aggregate_source_guard() {
    for missing in ["plan", "guard"] {
        let (_directory, mut session, mut result) = prepared_assets(false, true, false, true);
        if missing == "plan" {
            result.output.sky_assets = None;
        } else {
            result.publication.lock().unwrap().sources = None;
        }
        let outcome = Ok(result);
        session.finish_compile(Actor::User, &outcome);
        let result = outcome.as_ref().unwrap();
        assert!(!result.published(), "{missing}");
        assert!(result.publication_error().is_some(), "{missing}");
        assert_eq!(std::fs::read(&result.path).unwrap(), b"previous BSP");
    }
}

#[test]
fn packed_run_survives_custom_mount_removal_but_rejects_incomplete_pak() {
    let (directory, mut session, destination) = packed_fixture(false, true);
    let result = session
        .begin_compile(Actor::Agent, Some(&destination))
        .unwrap()
        .run()
        .unwrap();
    let outcome = Ok(result);
    session.finish_compile(Actor::Agent, &outcome);
    assert!(outcome.as_ref().unwrap().published());
    let bytes = std::fs::read(&destination).unwrap();
    let mut incomplete = Bsp::parse(&bytes).unwrap();
    let mut pak = incomplete
        .pak_data(source_formats::bsp::PakLimits::default())
        .unwrap();
    pak.entries
        .retain(|entry| entry.name != "materials/skybox/portable_shared.vtf");
    incomplete
        .set_pak_data(&pak, source_formats::bsp::PakLimits::default())
        .unwrap();
    assert!(game::stage_map_bytes(&incomplete.to_bytes(), "hammerr_portable").is_err());
    std::fs::remove_dir_all(directory.path().join("materials/skybox")).unwrap();
    session.game = Arc::new(GameFs {
        game_dir: session.game.game_dir.clone(),
        base_dir: session.game.base_dir.clone(),
        mounts: Vec::new(),
    });
    session.reload_material_assets(Actor::User);
    assert_ne!(
        session.compiled.as_ref().unwrap().content_epoch,
        session.content_epoch
    );
    assert_eq!(session.launch_readiness().unwrap(), destination);
    assert_eq!(
        game::stage_map_bytes(&bytes, "hammerr_portable").unwrap(),
        bytes
    );
}
