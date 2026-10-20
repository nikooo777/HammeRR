use super::*;

fn nested_opened(actor: Actor) -> (tempfile::TempDir, Session, PathBuf, PathBuf, PathBuf) {
    let (directory, session, source, middle) = opened(actor, false);
    let leaf = directory.path().join("leaf.vmf");
    std::fs::copy(&middle, &leaf).unwrap();
    let mut child = Document::empty();
    child
        .edit("grandchild reference", |tx| {
            map_core::edit::create_entity(
                tx,
                "func_instance",
                Some(DVec3::ZERO),
                &[
                    ("file".into(), "leaf.vmf".into()),
                    ("angles".into(), "0 0 0".into()),
                ],
                Vec::new(),
            )?;
            Ok(())
        })
        .unwrap();
    std::fs::write(&middle, child.vmf().write(&Default::default())).unwrap();
    (directory, session, source, middle, leaf)
}

#[test]
fn staged_nested_builds_refuse_changed_grandchildren_and_missing_intermediates_atomically() {
    for actor in [Actor::User, Actor::Agent] {
        for change in ["same_size_leaf", "replaced_leaf", "missing_middle"] {
            let (directory, mut session, source, middle, leaf) = nested_opened(actor);
            session
                .create_block(
                    actor,
                    DVec3::new(64.0, 64.0, 32.0),
                    DVec3::new(80.0, 80.0, 48.0),
                    "dev/dev_measuregeneric01",
                    false,
                )
                .unwrap();
            let memory = session.document().vmf().clone();
            let disk = std::fs::read(&source).unwrap();
            let history = session.history_snapshot();
            let selection = session.selected_ids();
            let camera = session.camera();
            let settings = session.compile_settings();
            let result = session.begin_compile(actor, None).unwrap().run().unwrap();
            assert!(result.output.bsp.is_some(), "{:?}", result.output.report);
            assert!(!result.published());
            let stats = result.output.stats.instances.as_ref().unwrap();
            assert_eq!(
                (stats.instances, stats.files, stats.max_expansion_depth),
                (2, 2, 2)
            );
            assert_eq!(
                result.output.stats.brushes,
                session.document().vmf().world.solids.len() + 2
            );
            assert!(result.output.instance_dependencies.is_some());
            match change {
                "same_size_leaf" => change_child(&leaf),
                "replaced_leaf" => {
                    let replacement = directory.path().join("identical-replacement.vmf");
                    std::fs::copy(&leaf, &replacement).unwrap();
                    std::fs::rename(replacement, &leaf).unwrap();
                }
                "missing_middle" => std::fs::remove_file(&middle).unwrap(),
                _ => unreachable!(),
            }
            let result = Ok(result);
            session.finish_compile(actor, &result);
            assert!(!result.as_ref().unwrap().published(), "{change}");
            assert!(
                result.as_ref().unwrap().publication_error().is_some(),
                "{change}"
            );
            assert_ne!(session.compile_status()["job"]["status"], "completed");
            assert!(session.launch_readiness().is_err());
            assert_eq!(
                std::fs::read(source.with_extension("bsp")).unwrap(),
                b"previous BSP"
            );
            assert_eq!(
                std::fs::read(source.with_extension("lin")).unwrap(),
                b"previous pointfile"
            );
            assert_eq!(session.document().vmf(), &memory);
            assert_eq!(std::fs::read(&source).unwrap(), disk);
            assert_eq!(session.history_snapshot(), history);
            assert_eq!(session.selected_ids(), selection);
            assert_eq!(session.camera(), camera);
            assert_eq!(session.compile_settings(), settings);
        }
    }
}

#[test]
fn published_nested_builds_stale_and_refuse_a_prepared_launch_after_grandchild_changes() {
    for actor in [Actor::User, Actor::Agent] {
        let (_directory, mut session, source, _middle, leaf) = nested_opened(actor);
        let memory = session.document().vmf().clone();
        let disk = std::fs::read(&source).unwrap();
        let history = session.history_snapshot();
        let selection = session.selected_ids();
        let camera = session.camera();
        let result = session.begin_compile(actor, None).unwrap().run();
        session.finish_compile(actor, &result);
        assert!(result.as_ref().unwrap().published());
        assert_eq!(session.compile_status()["job"]["status"], "completed");
        assert_eq!(session.compile_status()["job"]["stale"], false);
        assert_eq!(session.baked_lightmap_build_state()["status"], "current");
        assert!(session.launch_readiness().is_ok());
        let output = std::fs::read(source.with_extension("bsp")).unwrap();
        let request = session.begin_launch(actor).unwrap();
        let origin = request.origin();
        request.verify_instance_dependencies().unwrap();
        change_child(&leaf);
        assert!(request.verify_instance_dependencies().is_err());
        let launch = request.run();
        assert!(launch.is_err());
        assert!(
            session
                .finish_launch(actor, &origin, launch)
                .err()
                .unwrap()
                .contains("Instance source/dependencies changed")
        );
        let status = session.compile_status();
        assert_eq!(status["job"]["status"], "completed");
        assert_eq!(status["job"]["stale"], true);
        assert_eq!(status["job"]["instance_dependencies"]["current"], false);
        assert_eq!(status["launch"]["ready"], false);
        assert_eq!(session.baked_lightmap_build_state()["status"], "stale");
        let inspection = session
            .inspect_baked_lightmaps(&baked_lightmaps::Query::default())
            .unwrap();
        assert_eq!(inspection.report["build"]["status"], "stale");
        assert_eq!(std::fs::read(source.with_extension("bsp")).unwrap(), output);
        assert_eq!(
            std::fs::read(source.with_extension("lin")).unwrap(),
            b"previous pointfile"
        );
        assert_eq!(session.document().vmf(), &memory);
        assert_eq!(std::fs::read(&source).unwrap(), disk);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(session.selected_ids(), selection);
        assert_eq!(session.camera(), camera);
    }
}

fn opened(actor: Actor, leak: bool) -> (tempfile::TempDir, Session, PathBuf, PathBuf) {
    let (directory, game, source, child) = crate::test_assets::instance_map(leak);
    let mut session = Session::new(Ok(game));
    session.open(actor, &source, true).unwrap();
    (directory, session, source, child)
}

fn change_child(path: &Path) {
    let mut bytes = std::fs::read(path).unwrap();
    let newline = bytes.iter().position(|byte| *byte == b'\n').unwrap();
    bytes[newline] = b' ';
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn instance_builds_use_unsaved_snapshots_and_guard_ordinary_publication_and_freshness() {
    for actor in [Actor::User, Actor::Agent] {
        let (_directory, mut session, source, child) = opened(actor, false);
        session
            .create_block(
                actor,
                DVec3::new(64.0, 64.0, 32.0),
                DVec3::new(80.0, 80.0, 48.0),
                "dev/dev_measuregeneric01",
                false,
            )
            .unwrap();
        let memory = session.document().vmf().clone();
        let disk = std::fs::read(&source).unwrap();
        let history = session.history_snapshot();
        let selection = session.selected_ids();
        let camera = session.camera();
        session.run_map_checks(actor);
        assert_eq!(
            session
                .map_checks_json(Some("info"), "dependencies", 0, 50)
                .unwrap()["matching"],
            1
        );
        let request = session.begin_compile(actor, None).unwrap();
        assert!(request.instance_source.is_some() && request.source_stamp.is_none());
        let result = request.run().unwrap();
        assert!(result.output.bsp.is_some(), "{:?}", result.output.report);
        assert!(result.output.instance_dependencies.is_some());
        assert_eq!(
            result.output.stats.brushes,
            session.document().vmf().world.solids.len() + 2
        );
        assert!(!result.published());
        assert_eq!(
            std::fs::read(source.with_extension("bsp")).unwrap(),
            b"previous BSP"
        );
        let stats = result.output.stats.instances.as_ref().unwrap();
        assert_eq!(
            (
                stats.instances,
                stats.files,
                stats.world_brushes,
                stats.detail_brushes
            ),
            (1, 1, 1, 1)
        );
        let result = Ok(result);
        session.finish_compile(actor, &result);
        assert!(result.as_ref().unwrap().published());
        assert!(session.launch_readiness().is_ok());
        assert_eq!(session.compile_status()["job"]["stale"], false);
        assert_eq!(session.baked_lightmap_build_state()["status"], "current");
        let request = session.begin_launch(actor).unwrap();
        let origin = request.origin();
        request.verify_instance_dependencies().unwrap();
        change_child(&child);
        assert!(request.verify_instance_dependencies().is_err());
        let launch = request.run();
        assert!(launch.is_err());
        assert!(
            session
                .finish_launch(actor, &origin, launch)
                .err()
                .unwrap()
                .contains("Instance source/dependencies changed")
        );
        let status = session.compile_status();
        assert_eq!(status["job"]["status"], "completed");
        assert_eq!(status["job"]["stale"], true);
        assert_eq!(status["job"]["instance_dependencies"]["current"], false);
        assert_eq!(status["launch"]["ready"], false);
        assert!(session.compile_label().contains("dependencies"));
        assert_eq!(session.baked_lightmap_build_state()["status"], "stale");
        let inspection = session
            .inspect_baked_lightmaps(&baked_lightmaps::Query::default())
            .unwrap();
        assert_eq!(inspection.report["build"]["status"], "stale");
        assert_eq!(session.document().vmf(), &memory);
        assert_eq!(std::fs::read(&source).unwrap(), disk);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(session.selected_ids(), selection);
        assert_eq!(session.camera(), camera);
        assert_eq!(
            std::fs::read(source.with_extension("lin")).unwrap(),
            b"previous pointfile"
        );
    }
}

#[test]
fn instance_dependency_or_session_changes_after_staging_preserve_bsp_and_lin_for_both_actors() {
    for actor in [Actor::User, Actor::Agent] {
        for change in [
            "same_size_child",
            "replaced_child",
            "missing_child",
            "source_path",
            "revision",
            "content",
            "destination",
            "cancel",
        ] {
            let (directory, mut session, source, child) = opened(actor, false);
            let memory = session.document().vmf().clone();
            let history = session.history_snapshot();
            let camera = session.camera();
            let result = session.begin_compile(actor, None).unwrap().run().unwrap();
            assert!(result.output.bsp.is_some(), "{:?}", result.output.report);
            let mut expected = b"previous BSP".to_vec();
            match change {
                "same_size_child" => change_child(&child),
                "replaced_child" => {
                    let replacement = directory.path().join("replacement.vmf");
                    std::fs::copy(&child, &replacement).unwrap();
                    std::fs::rename(replacement, &child).unwrap();
                }
                "missing_child" => std::fs::remove_file(&child).unwrap(),
                "source_path" => {
                    session
                        .save(actor, Some(&directory.path().join("saved-as.vmf")))
                        .unwrap();
                }
                "revision" => {
                    session
                        .create_block(
                            actor,
                            DVec3::new(96.0, 96.0, 32.0),
                            DVec3::new(112.0, 112.0, 48.0),
                            "dev/dev_measuregeneric01",
                            false,
                        )
                        .unwrap();
                }
                "content" => session.content_epoch += 1,
                "destination" => {
                    expected = b"external output change".to_vec();
                    std::fs::write(source.with_extension("bsp"), &expected).unwrap();
                }
                "cancel" => result.control.cancel().unwrap(),
                _ => unreachable!(),
            }
            let before_finish = session.document().vmf().clone();
            let history_before_finish = session.history_snapshot();
            let selection_before_finish = session.selected_ids();
            let camera_before_finish = session.camera();
            let source_before_finish = std::fs::read(&source).unwrap();
            let result = Ok(result);
            session.finish_compile(actor, &result);
            assert!(!result.as_ref().unwrap().published(), "{change}");
            assert!(
                result.as_ref().unwrap().publication_error().is_some(),
                "{change}"
            );
            assert_eq!(
                std::fs::read(source.with_extension("bsp")).unwrap(),
                expected,
                "{change}"
            );
            assert_eq!(
                std::fs::read(source.with_extension("lin")).unwrap(),
                b"previous pointfile",
                "{change}"
            );
            assert_ne!(session.compile_status()["job"]["status"], "completed");
            assert_eq!(session.document().vmf(), &before_finish);
            assert_eq!(session.history_snapshot(), history_before_finish);
            assert_eq!(session.selected_ids(), selection_before_finish);
            assert_eq!(session.camera(), camera_before_finish);
            assert_eq!(std::fs::read(&source).unwrap(), source_before_finish);
            if !matches!(change, "revision" | "source_path") {
                assert_eq!(session.document().vmf(), &memory);
                assert_eq!(session.history_snapshot(), history);
                assert_eq!(session.camera(), camera);
            }
        }
    }
}

#[test]
fn instance_leak_publication_revalidates_dependencies_and_preserves_previous_pointfile_on_failure()
{
    for actor in [Actor::User, Actor::Agent] {
        let (_directory, mut session, source, child) = opened(actor, true);
        let result = session.begin_compile(actor, None).unwrap().run().unwrap();
        assert!(
            result.output.bsp.is_none() && result.output.leak.is_some(),
            "{:?}",
            result.output.report
        );
        assert!(result.output.instance_dependencies.is_some());
        change_child(&child);
        let result = Ok(result);
        session.finish_compile(actor, &result);
        assert!(result.as_ref().unwrap().publication_error().is_some());
        assert_eq!(
            std::fs::read(source.with_extension("bsp")).unwrap(),
            b"previous BSP"
        );
        assert_eq!(
            std::fs::read(source.with_extension("lin")).unwrap(),
            b"previous pointfile"
        );
    }
}

#[test]
fn instance_source_context_rejects_untitled_maps_and_source_changes_before_worker_admission() {
    let (directory, mut session, source, _child) = opened(Actor::Agent, false);
    let request = session.begin_compile(Actor::Agent, None).unwrap();
    std::fs::write(&source, b"replaced root bytes").unwrap();
    let result = request.run();
    assert!(result.is_err());
    session.finish_compile(Actor::Agent, &result);
    assert_eq!(
        std::fs::read(source.with_extension("bsp")).unwrap(),
        b"previous BSP"
    );
    session.replace_document(Document::new(session.document.vmf().clone()), None);
    assert!(
        session
            .begin_compile(Actor::Agent, Some(&directory.path().join("untitled.bsp")))
            .err()
            .unwrap()
            .contains("Save the parent VMF")
    );
    session.run_map_checks(Actor::Agent);
    assert_eq!(
        session
            .map_checks_json(Some("error"), "Save the parent VMF", 0, 50)
            .unwrap()["matching"],
        1
    );
}

#[test]
fn instance_helper_checks_track_save_and_save_as_without_loading_children() {
    for actor in [Actor::User, Actor::Agent] {
        let (directory, mut session, _source, child) = opened(actor, false);
        std::fs::remove_file(child).unwrap();
        session.path = None;
        session.run_map_checks(actor);
        assert!(!session.map_checks_stale());
        assert_eq!(
            session
                .map_checks_json(Some("error"), "Save the parent VMF", 0, 50)
                .unwrap()["matching"],
            1
        );
        session.path = Some(directory.path().join("saved.vmf"));
        assert!(session.map_checks_stale());
        session.path = None;
        assert!(!session.map_checks_stale());
        session
            .save(actor, Some(&directory.path().join("saved.vmf")))
            .unwrap();
        assert!(session.map_checks_stale());
        session.run_map_checks(actor);
        assert!(!session.map_checks_stale());
        assert_eq!(
            session
                .map_checks_json(Some("info"), "dependencies", 0, 50)
                .unwrap()["matching"],
            1
        );
        session
            .save(actor, Some(&directory.path().join("saved-as.vmf")))
            .unwrap();
        assert!(session.map_checks_stale());
    }
}

#[test]
fn no_instance_builds_keep_direct_publication_and_existing_status_shape() {
    let (directory, game, source, child) = crate::test_assets::instance_map(false);
    std::fs::remove_file(child).unwrap();
    let vmf = crate::test_assets::room();
    std::fs::write(&source, vmf.write(&Default::default())).unwrap();
    let mut session = Session::new(Ok(game));
    session.open(Actor::User, &source, true).unwrap();
    let request = session.begin_compile(Actor::User, None).unwrap();
    assert!(request.instance_source.is_none());
    assert!(request.source_stamp.is_none());
    assert!(request.destination_stamp.is_none());
    assert!(request.pointfile_stamp.is_none());
    std::fs::write(&source, b"source changed after ordinary snapshot capture").unwrap();
    let result = request.run().unwrap();
    assert!(result.published(), "{:?}", result.output.report);
    assert!(result.output.instance_dependencies.is_none());
    assert!(result.output.stats.instances.is_none());
    assert!(result.publication.lock().unwrap().pending.is_none());
    let result = Ok(result);
    session.finish_compile(Actor::User, &result);
    let status = session.compile_status();
    assert_eq!(status["job"]["status"], "completed");
    assert_eq!(status["job"]["stale"], false);
    assert!(status["job"].get("instance_dependencies").is_none());
    assert!(status["job"]["result"]["stats"].get("instances").is_none());
    assert!(directory.path().join("instances.bsp").is_file());
}

#[cfg(unix)]
#[test]
fn instance_source_symlinks_use_the_referring_directory_and_stale_retargeted_roots() {
    use std::os::unix::fs::symlink;
    let (directory, game, source, child) = crate::test_assets::instance_map(false);
    let referring = directory.path().join("referring");
    std::fs::create_dir(&referring).unwrap();
    std::fs::copy(&child, referring.join("child.vmf")).unwrap();
    let alias = referring.join("linked.vmf");
    symlink(&source, &alias).unwrap();
    let mut session = Session::new(Ok(game));
    session.open(Actor::User, &alias, true).unwrap();
    let result = session.begin_compile(Actor::User, None).unwrap().run();
    assert!(
        result.as_ref().unwrap().output.bsp.is_some(),
        "{:?}",
        result.as_ref().unwrap().output.report
    );
    session.finish_compile(Actor::User, &result);
    assert!(result.as_ref().unwrap().published());
    assert_eq!(session.baked_lightmap_build_state()["status"], "current");
    let replacement = directory.path().join("replacement-root.vmf");
    std::fs::copy(&source, &replacement).unwrap();
    std::fs::remove_file(&alias).unwrap();
    symlink(&replacement, &alias).unwrap();
    assert_eq!(session.compile_status()["job"]["stale"], true);
    assert!(session.launch_readiness().is_err());
    assert_eq!(session.baked_lightmap_build_state()["status"], "stale");
}

#[test]
fn instance_pointfile_conflicts_before_worker_and_reflection_requests_preserve_outputs() {
    let (_directory, mut session, source, _child) = opened(Actor::Agent, true);
    let request = session.begin_compile(Actor::Agent, None).unwrap();
    std::fs::write(source.with_extension("lin"), b"external pointfile").unwrap();
    let result = request.run();
    assert!(result.is_err());
    session.finish_compile(Actor::Agent, &result);
    assert_eq!(
        std::fs::read(source.with_extension("bsp")).unwrap(),
        b"previous BSP"
    );
    assert_eq!(
        std::fs::read(source.with_extension("lin")).unwrap(),
        b"external pointfile"
    );
    session
        .update_build_settings(
            Actor::Agent,
            BuildSettingsUpdate {
                lighting: Some("direct_ldr"),
                generate_reflections: Some(true),
                ..Default::default()
            },
        )
        .unwrap();
    assert!(
        session
            .begin_compile(Actor::Agent, None)
            .err()
            .unwrap()
            .contains("Generate reflections with instances")
    );
    assert!(!session.compiling());
}
