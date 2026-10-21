use std::sync::atomic::{AtomicUsize, Ordering};

use native_runtime::{
    CaptureFailure, CaptureObserver, CaptureReceipt, CaptureRequest, CleanupDisposition,
};
use source_formats::bsp::{Bsp, PakLimits};
use source_formats::gamefs::{Mount, MountedPath};

use super::*;

fn fixture(actor: Actor) -> (tempfile::TempDir, Session, PathBuf, PathBuf) {
    let (directory, mut game, source, child) = crate::test_assets::instance_map(false);
    std::fs::create_dir_all(directory.path().join("materials/dev")).unwrap();
    std::fs::create_dir_all(directory.path().join("materials/skybox")).unwrap();
    std::fs::write(
        directory
            .path()
            .join("materials/dev/dev_measuregeneric01.vmt"),
        "LightmappedGeneric { $envmap env_cubemap }",
    )
    .unwrap();
    for face in ["rt", "lf", "bk", "ft", "up", "dn"] {
        std::fs::write(
            directory
                .path()
                .join(format!("materials/skybox/sky_day01_01{face}.vmt")),
            "UnlitGeneric { $unknown capture_protocol_fixture }",
        )
        .unwrap();
    }
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().to_owned()),
    });
    let steam = directory.path().join("steam");
    std::fs::create_dir(&steam).unwrap();
    let mut session = Session::new(Ok(game));
    session.open(actor, &source, true).unwrap();
    let instance = session
        .document
        .vmf()
        .entities
        .iter()
        .find(|entity| entity.classname() == Some("func_instance"))
        .unwrap()
        .id;
    session
        .edit(actor, "root reflection helpers", |tx| {
            let properties = &mut tx.entity_mut(instance)?.properties;
            properties.set("origin", "32 -16 0");
            properties.set("angles", "0 90 0");
            map_core::edit::create_entity(
                tx,
                "env_cubemap",
                Some(DVec3::new(0.0, 0.0, 96.0)),
                &[("cubemapsize".into(), "2".into())],
                Vec::new(),
            )?;
            map_core::edit::create_entity(
                tx,
                "light",
                Some(DVec3::new(0.0, 0.0, 180.0)),
                &[("_light".into(), "255 255 255 200".into())],
                Vec::new(),
            )?;
            Ok(())
        })
        .unwrap();
    session.save(actor, None).unwrap();
    session
        .update_build_settings(
            actor,
            BuildSettingsUpdate {
                lighting: Some("direct_ldr"),
                generate_reflections: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
    session.use_test_capture_environment(native_runtime::CaptureEnvironment {
        engine_root: directory.path().to_owned(),
        steam_root: steam,
        mounts: vec![native_runtime::ContentMount {
            ids: vec!["game".into()],
            path: directory.path().to_owned(),
        }],
    });
    (directory, session, source, child)
}

fn protocol_capture(
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
        for pixel in entry.data[header..].as_chunks_mut::<4>().0 {
            pixel[0] = 32;
            pixel[3] = 255;
        }
    }
    captured
        .set_pak_data(&archive, PakLimits::default())
        .unwrap();
    Ok(CaptureReceipt {
        channel: request.channel,
        bsp: captured.to_bytes(),
        staged_bsp: staged,
        artifacts: "synthetic-session-reflection-protocol".into(),
        elapsed_ms: 1,
        verified_controls: true,
        preservation_passed: true,
    })
}

#[test]
fn assembled_reflection_capture_keeps_dependency_and_publication_lifecycles_for_both_actors() {
    for actor in [Actor::User, Actor::Agent] {
        for change in [
            "none",
            "cancel_capture",
            "child_during_capture",
            "child_before_publish",
        ] {
            let (_directory, mut session, source, child) = fixture(actor);
            session.begin_property_edit(PropertyTarget::World).unwrap();
            let memory = session.document.vmf().clone();
            let disk = std::fs::read(&source).unwrap();
            let child_bytes = std::fs::read(&child).unwrap();
            let history = session.history_snapshot();
            let selection = session.selected_ids();
            let camera = session.camera();
            let draft = serde_json::to_value(session.property_draft()).unwrap();
            let source_faces = memory.side_ids().collect::<std::collections::BTreeSet<_>>();
            let mut request = session.begin_compile(actor, None).unwrap();
            let control = request.control();
            let calls = Arc::new(AtomicUsize::new(0));
            let observed = calls.clone();
            let changed_child = child.clone();
            request.capture_with(Box::new(move |request, observer| {
                observed.fetch_add(1, Ordering::AcqRel);
                assert_eq!(request.channel, native_runtime::CaptureChannel::Ldr);
                assert_eq!(request.samples.len(), 1);
                if change == "cancel_capture" {
                    control.cancel().unwrap();
                }
                let receipt = protocol_capture(request, observer)?;
                if change == "child_during_capture" {
                    let mut bytes = std::fs::read(&changed_child).unwrap();
                    bytes.push(b'\n');
                    std::fs::write(&changed_child, bytes).unwrap();
                }
                Ok(receipt)
            }));
            let (result, native) = request.run_admitted();
            assert!(native.is_none());
            assert_eq!(calls.load(Ordering::Acquire), 1, "{change}");
            if matches!(change, "cancel_capture" | "child_during_capture") {
                assert!(result.is_err(), "{change}");
            } else {
                let output = &result.as_ref().unwrap().output;
                assert!(output.bsp.is_some(), "{:?}", output.report);
                assert_eq!(output.stats.instances.as_ref().unwrap().instances, 1);
                assert!(output.instance_dependencies.is_some());
                let manifest = output.cubemap_assets.as_ref().unwrap();
                assert_eq!(manifest.phase, compiler::CubemapAssetPhase::Verified);
                assert!(
                    manifest
                        .assignments
                        .iter()
                        .any(|assignment| { !source_faces.contains(&assignment.face_id) })
                );
                assert!(!result.as_ref().unwrap().published());
            }
            if change == "child_before_publish" {
                let mut bytes = child_bytes.clone();
                bytes.push(b'\n');
                std::fs::write(&child, bytes).unwrap();
            }
            session.finish_compile(actor, &result);
            assert!(!session.compiling());
            if change == "none" {
                assert!(result.as_ref().unwrap().published());
                assert_eq!(session.compile_status()["job"]["stale"], false);
                assert!(session.launch_readiness().is_ok());
                let published = std::fs::read(source.with_extension("bsp")).unwrap();
                let mut bytes = child_bytes.clone();
                bytes.push(b'\n');
                std::fs::write(&child, bytes).unwrap();
                assert_eq!(session.compile_status()["job"]["stale"], true);
                assert!(session.launch_readiness().is_err());
                assert_eq!(
                    std::fs::read(source.with_extension("bsp")).unwrap(),
                    published
                );
            } else {
                assert!(!result.as_ref().is_ok_and(|result| result.published()));
                assert_eq!(
                    std::fs::read(source.with_extension("bsp")).unwrap(),
                    b"previous BSP"
                );
                if change == "child_before_publish" {
                    assert!(result.as_ref().unwrap().publication_error().is_some());
                }
            }
            assert_eq!(
                std::fs::read(source.with_extension("lin")).unwrap(),
                b"previous pointfile"
            );
            assert_eq!(session.document.vmf(), &memory);
            assert_eq!(std::fs::read(&source).unwrap(), disk);
            assert_eq!(session.history_snapshot(), history);
            assert_eq!(session.selected_ids(), selection);
            assert_eq!(session.camera(), camera);
            assert_eq!(
                serde_json::to_value(session.property_draft()).unwrap(),
                draft
            );
            if change == "cancel_capture" {
                assert_eq!(std::fs::read(&child).unwrap(), child_bytes);
            }
        }
    }
}
