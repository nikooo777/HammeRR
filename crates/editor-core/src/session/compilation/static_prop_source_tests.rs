use source_formats::gamefs::{Mount, MountedPath};
use source_formats::vmf::Entity;

use super::*;

const MODEL: &str = "models/props/source.mdl";
const PHYSICS: &str = "models/props/source.phy";

struct Fixture {
    directory: tempfile::TempDir,
    session: Session,
    source: PathBuf,
    output: PathBuf,
    higher: PathBuf,
    assets: PathBuf,
}

impl Fixture {
    fn new(actor: Actor, solid: &str) -> Self {
        let (directory, mut game, _) = crate::test_assets::game_process(false);
        let higher = directory.path().join("higher");
        let assets = game.game_dir.clone();
        for path in [
            assets.join("models/props"),
            higher.join("models/props"),
            directory.path().join("outputs"),
        ] {
            std::fs::create_dir_all(path).unwrap();
        }
        let mut model = vec![0; 240];
        model[..4].copy_from_slice(b"IDST");
        for (offset, value) in [(4, 48_i32), (76, 240), (152, 16), (224, 1)] {
            model[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        for (offset, values) in [
            (104, [-16.0_f32, -8.0, 0.0]),
            (116, [16.0, 8.0, 32.0]),
            (128, [-16.0, -8.0, 0.0]),
            (140, [16.0, 8.0, 32.0]),
        ] {
            for (axis, value) in values.into_iter().enumerate() {
                model[offset + axis * 4..offset + axis * 4 + 4]
                    .copy_from_slice(&value.to_le_bytes());
            }
        }
        std::fs::write(assets.join(MODEL), model).unwrap();
        let mut physics = [16_i32, 0, 1, 0].map(i32::to_le_bytes).concat();
        physics.extend_from_slice(&32_i32.to_le_bytes());
        physics.extend_from_slice(b"VPHY");
        physics.extend_from_slice(&0x100_u16.to_le_bytes());
        physics.extend_from_slice(&0_u16.to_le_bytes());
        physics.extend_from_slice(&4_u32.to_le_bytes());
        physics.extend_from_slice(&[0; 20]);
        physics.extend_from_slice(b"solid {}\0");
        std::fs::write(assets.join(PHYSICS), physics).unwrap();
        game.mounts = vec![
            MountedPath {
                ids: vec!["game".into()],
                mount: Mount::Directory(higher.clone()),
            },
            MountedPath {
                ids: vec!["game".into()],
                mount: Mount::Directory(assets.clone()),
            },
        ];
        let mut vmf = crate::test_assets::room();
        let mut prop = Entity::new(vmf.object_ids().max().unwrap() + 1, "prop_static");
        for (key, value) in [("model", MODEL), ("origin", "32 32 8"), ("solid", solid)] {
            prop.properties.set(key, value);
        }
        vmf.entities.push(prop);
        let source = directory.path().join("source.vmf");
        let output = directory.path().join("outputs/map.bsp");
        std::fs::write(&source, vmf.write(&Default::default())).unwrap();
        std::fs::write(&output, b"previous BSP").unwrap();
        std::fs::write(output.with_extension("lin"), b"previous LIN").unwrap();
        let mut session = Session::new(Ok(game));
        session.open(actor, &source, true).unwrap();
        Self {
            directory,
            session,
            source,
            output,
            higher,
            assets,
        }
    }

    fn mount_model_vpk(&mut self) {
        let mut tree = Vec::new();
        let mut payloads = Vec::new();
        for (extension, path) in [("mdl", MODEL), ("phy", PHYSICS)] {
            let payload = std::fs::read(self.assets.join(path)).unwrap();
            tree.extend_from_slice(extension.as_bytes());
            tree.extend_from_slice(b"\0models/props\0source\0");
            tree.extend_from_slice(&0_u32.to_le_bytes());
            tree.extend_from_slice(&0_u16.to_le_bytes());
            tree.extend_from_slice(&0x7fff_u16.to_le_bytes());
            tree.extend_from_slice(&u32::try_from(payloads.len()).unwrap().to_le_bytes());
            tree.extend_from_slice(&u32::try_from(payload.len()).unwrap().to_le_bytes());
            tree.extend_from_slice(&0xffff_u16.to_le_bytes());
            tree.extend_from_slice(b"\0\0");
            payloads.extend_from_slice(&payload);
            std::fs::remove_file(self.assets.join(path)).unwrap();
        }
        tree.push(0);
        let mut index = 0x55aa_1234_u32.to_le_bytes().to_vec();
        index.extend_from_slice(&1_u32.to_le_bytes());
        index.extend_from_slice(&u32::try_from(tree.len()).unwrap().to_le_bytes());
        index.extend_from_slice(&tree);
        index.extend_from_slice(&payloads);
        let path = self.directory.path().join("props_dir.vpk");
        std::fs::write(&path, index).unwrap();
        self.session.game = Arc::new(GameFs {
            game_dir: self.assets.clone(),
            base_dir: self.session.game.base_dir.clone(),
            mounts: vec![
                MountedPath {
                    ids: vec!["game".into()],
                    mount: Mount::Directory(self.higher.clone()),
                },
                MountedPath {
                    ids: vec!["game".into()],
                    mount: Mount::Directory(self.assets.clone()),
                },
                MountedPath {
                    ids: vec!["game".into()],
                    mount: Mount::Pak(source_formats::vpk::Vpk::open(path).unwrap()),
                },
            ],
        });
    }

    fn request(&mut self, actor: Actor) -> CompileRequest {
        self.session
            .begin_compile(actor, Some(&self.output))
            .unwrap()
    }

    fn compile(&mut self, actor: Actor) {
        let result = self.request(actor).run();
        assert!(
            result.as_ref().unwrap().output.bsp.is_some(),
            "{:?}",
            result.as_ref().unwrap().output.report
        );
        self.session.finish_compile(actor, &result);
        assert!(
            result.as_ref().unwrap().published(),
            "{}",
            self.session.compile_status()
        );
    }

    fn mutate(&self, change: &str) {
        let model = self.assets.join(MODEL);
        let physics = self.assets.join(PHYSICS);
        match change {
            "model" => mutate_file(&model),
            "physics" => mutate_file(&physics),
            "replacement" => {
                let replacement = self.directory.path().join("replacement.mdl");
                std::fs::copy(&model, &replacement).unwrap();
                std::fs::rename(replacement, model).unwrap();
            }
            "missing" => std::fs::remove_file(model).unwrap(),
            "winner" => {
                std::fs::copy(model, self.higher.join(MODEL)).unwrap();
            }
            _ => unreachable!(),
        }
    }
}

fn mutate_file(path: &Path) {
    let mut bytes = std::fs::read(path).unwrap();
    let offset = if path.extension().unwrap() == "mdl" {
        20
    } else {
        bytes.len() - 2
    };
    bytes[offset] ^= 1;
    std::fs::write(path, bytes).unwrap();
}

fn owned_console_exists(game: &Path) -> bool {
    std::fs::read_dir(game).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("hammerr_console_")
    })
}

fn wait_for_fake_game(run: &game::Run) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while !run.log(1).unwrap().console.contains("loaded ") {
        assert!(
            std::time::Instant::now() < deadline,
            "fake game did not start"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn ordinary_fullbright_props_wait_for_session_publication_and_keep_original_guards() {
    for actor in [Actor::User, Actor::Agent] {
        let mut fixture = Fixture::new(actor, "6");
        let request = fixture.request(actor);
        assert!(!request.options.requires_guarded_publication());
        assert!(
            request
                .options
                .requires_guarded_publication_for(&request.vmf)
        );
        assert!(request.source_stamp.is_some() && request.destination_stamp.is_some());
        let result = if actor == Actor::Agent {
            let (result, permit) = request.run_admitted();
            assert!(permit.is_none());
            result.unwrap()
        } else {
            request.run().unwrap()
        };
        assert!(result.output.static_prop_sources.is_some());
        assert!(result.static_prop_sources.is_some());
        assert!(!result.published());
        assert!(result.publication.lock().unwrap().pending.is_some());
        assert_eq!(std::fs::read(&fixture.output).unwrap(), b"previous BSP");
        let expected = result.output.bsp.as_ref().unwrap().to_bytes();
        let result = Ok(result);
        fixture.session.finish_compile(actor, &result);
        assert_eq!(std::fs::read(&fixture.output).unwrap(), expected);
        assert!(
            fixture
                .session
                .compilation
                .job
                .as_ref()
                .unwrap()
                .static_prop_sources
                .is_some()
        );
        assert!(
            fixture
                .session
                .compiled
                .as_ref()
                .unwrap()
                .static_prop_sources
                .is_some()
        );
        let status = fixture.session.compile_status();
        assert_eq!(status["job"]["status"], "completed");
        assert_eq!(status["job"]["stale"], false);
        assert!(status["job"]["static_prop_source_issue"].is_null());
        assert_eq!(status["launch"]["ready"], true);
    }
}

#[test]
fn first_read_drift_before_worker_preparation_never_publishes_a_new_baseline() {
    for asset in [MODEL, PHYSICS] {
        let mut fixture = Fixture::new(Actor::Agent, "6");
        let source = std::fs::read(&fixture.source).unwrap();
        let memory = fixture.session.document.vmf().clone();
        let path = fixture.assets.join(asset);
        let mut request = fixture.request(Actor::Agent);
        request.before_preparation = Some(Box::new(move || mutate_file(&path)));
        let result = request.run();
        assert!(result.is_err());
        assert!(
            result
                .as_ref()
                .err()
                .unwrap()
                .to_ascii_lowercase()
                .contains("changed")
        );
        fixture.session.finish_compile(Actor::Agent, &result);
        assert_ne!(
            fixture.session.compile_status()["job"]["status"],
            "completed"
        );
        assert!(fixture.session.compiled.is_none());
        assert_eq!(std::fs::read(&fixture.output).unwrap(), b"previous BSP");
        assert_eq!(
            std::fs::read(fixture.output.with_extension("lin")).unwrap(),
            b"previous LIN"
        );
        assert_eq!(fixture.session.document.vmf(), &memory);
        assert_eq!(std::fs::read(&fixture.source).unwrap(), source);
    }
}

#[test]
fn staged_model_physics_and_lookup_drift_preserve_previous_outputs_and_owned_state() {
    for actor in [Actor::User, Actor::Agent] {
        for change in ["model", "physics", "replacement", "missing", "winner"] {
            let mut fixture = Fixture::new(actor, "6");
            let memory = fixture.session.document.vmf().clone();
            let disk = std::fs::read(&fixture.source).unwrap();
            let history = fixture.session.history_snapshot();
            let selection = fixture.session.selected_ids();
            let camera = fixture.session.camera();
            let settings = fixture.session.compile_settings();
            let result = fixture.request(actor).run().unwrap();
            assert!(result.static_prop_sources.is_some());
            fixture.mutate(change);
            let result = Ok(result);
            fixture.session.finish_compile(actor, &result);
            assert!(!result.as_ref().unwrap().published(), "{change}");
            assert!(
                result.as_ref().unwrap().publication_error().is_some(),
                "{change}"
            );
            assert!(
                fixture.session.compile_status()["job"]["static_prop_source_issue"].is_string(),
                "{change}"
            );
            assert!(fixture.session.launch_readiness().is_err());
            assert_eq!(std::fs::read(&fixture.output).unwrap(), b"previous BSP");
            assert_eq!(
                std::fs::read(fixture.output.with_extension("lin")).unwrap(),
                b"previous LIN"
            );
            assert_eq!(fixture.session.document.vmf(), &memory);
            assert_eq!(std::fs::read(&fixture.source).unwrap(), disk);
            assert_eq!(fixture.session.history_snapshot(), history);
            assert_eq!(fixture.session.selected_ids(), selection);
            assert_eq!(fixture.session.camera(), camera);
            assert_eq!(fixture.session.compile_settings(), settings);
        }
    }
}

#[test]
fn completed_prop_builds_stale_inspections_and_refuse_already_admitted_launches() {
    for actor in [Actor::User, Actor::Agent] {
        for change in ["model", "physics", "replacement", "missing", "winner"] {
            let mut fixture = Fixture::new(actor, "6");
            fixture.compile(actor);
            let output = std::fs::read(&fixture.output).unwrap();
            let request = fixture.session.begin_launch(actor).unwrap();
            let origin = request.origin();
            request.verify_compiled_sources().unwrap();
            fixture.mutate(change);
            assert!(request.verify_compiled_sources().is_err(), "{change}");
            let launch = request.run();
            assert!(launch.is_err());
            assert!(
                fixture
                    .session
                    .finish_launch(actor, &origin, launch)
                    .err()
                    .unwrap()
                    .contains("Static-prop")
            );
            let status = fixture.session.compile_status();
            assert_eq!(status["job"]["status"], "completed");
            assert_eq!(status["job"]["stale"], true, "{change}");
            assert!(status["job"]["static_prop_source_issue"].is_string());
            assert_eq!(status["launch"]["ready"], false);
            assert!(
                fixture
                    .session
                    .compile_label()
                    .contains("model/physics sources")
            );
            assert_eq!(
                fixture.session.baked_lightmap_build_state()["status"],
                "stale"
            );
            assert_eq!(
                fixture.session.baked_prop_vertex_build_state()["status"],
                "stale"
            );
            let lightmaps = fixture
                .session
                .inspect_baked_lightmaps(&baked_lightmaps::Query::default())
                .unwrap();
            assert_eq!(lightmaps.report["build"]["status"], "stale");
            let vertices = fixture
                .session
                .inspect_baked_prop_vertices(&baked_prop_vertices::Query::default())
                .unwrap();
            assert_eq!(vertices.report["build"]["status"], "stale");
            assert_eq!(std::fs::read(&fixture.output).unwrap(), output);
            assert_eq!(
                std::fs::read(fixture.output.with_extension("lin")).unwrap(),
                b"previous LIN"
            );
        }
    }
}

#[test]
fn original_prop_sources_survive_owned_launch_files_in_a_consulted_game_directory() {
    for vpk in [false, true] {
        let mut fixture = Fixture::new(Actor::Agent, "6");
        if vpk {
            fixture.mount_model_vpk();
        }
        fixture.compile(Actor::Agent);
        let output = std::fs::read(&fixture.output).unwrap();
        let original = fixture
            .session
            .compiled
            .as_ref()
            .unwrap()
            .static_prop_sources
            .as_ref()
            .unwrap()
            .clone();
        original
            .guard
            .verify_sources(&original.game, &compiler::Control::default())
            .unwrap();
        let mut request = fixture.session.begin_launch(Actor::Agent).unwrap();
        let origin = request.origin();
        let checked = original.clone();
        let game = fixture.assets.clone();
        let invoked = Arc::new(AtomicBool::new(false));
        let seen = invoked.clone();
        request.before_checkpoint = Some(Box::new(move || {
            if owned_console_exists(&game) {
                assert!(
                    checked
                        .guard
                        .verify_sources(&checked.game, &compiler::Control::default())
                        .is_err()
                );
                checked.verify(&compiler::Control::default()).unwrap();
                seen.store(true, Ordering::Release);
            }
        }));
        let run = request.run().unwrap();
        assert!(invoked.load(Ordering::Acquire), "vpk={vpk}");
        wait_for_fake_game(&run);
        original.verify(&compiler::Control::default()).unwrap();
        fixture
            .session
            .finish_launch(Actor::Agent, &origin, Ok(run))
            .unwrap();
        assert!(owned_console_exists(&fixture.assets));
        assert_eq!(fixture.session.compile_status()["job"]["stale"], false);
        assert!(fixture.session.compile_status()["job"]["static_prop_source_issue"].is_null());
        let lightmaps = fixture
            .session
            .inspect_baked_lightmaps(&baked_lightmaps::Query::default())
            .unwrap();
        assert_eq!(lightmaps.report["build"]["status"], "current");
        let vertices = fixture
            .session
            .inspect_baked_prop_vertices(&baked_prop_vertices::Query::default())
            .unwrap();
        assert_eq!(vertices.report["build"]["status"], "current");
        fixture.session.stop_game(Actor::Agent).unwrap();
        drop(fixture.session.game_run.take());
        assert!(!owned_console_exists(&fixture.assets));
        original.verify(&compiler::Control::default()).unwrap();
        assert_eq!(fixture.session.compile_status()["job"]["stale"], false);
        assert_eq!(
            fixture.session.baked_lightmap_build_state()["status"],
            "current"
        );
        assert_eq!(
            fixture.session.baked_prop_vertex_build_state()["status"],
            "current"
        );
        assert!(fixture.session.launch_readiness().is_ok());
        assert_eq!(
            std::fs::read_dir(fixture.assets.join("maps"))
                .unwrap()
                .count(),
            1
        );
        assert_eq!(std::fs::read(&fixture.output).unwrap(), output);
    }
}

#[test]
fn source_drift_at_the_final_launch_checkpoint_refuses_before_any_process_spawns() {
    for change in ["model", "physics", "winner"] {
        let mut fixture = Fixture::new(Actor::Agent, "6");
        fixture.compile(Actor::Agent);
        let mut request = fixture.session.begin_launch(Actor::Agent).unwrap();
        let origin = request.origin();
        let game = fixture.session.game.game_dir.clone();
        let model = fixture.assets.join(MODEL);
        let physics = fixture.assets.join(PHYSICS);
        let winner = fixture.higher.join(MODEL);
        let invoked = Arc::new(AtomicBool::new(false));
        let seen = invoked.clone();
        let checked_game = game.clone();
        request.before_checkpoint = Some(Box::new(move || {
            if owned_console_exists(&checked_game) && !seen.swap(true, Ordering::AcqRel) {
                match change {
                    "model" => mutate_file(&model),
                    "physics" => mutate_file(&physics),
                    "winner" => {
                        std::fs::copy(&model, &winner).unwrap();
                    }
                    _ => unreachable!(),
                }
            }
        }));
        let result = request.run();
        assert!(invoked.load(Ordering::Acquire));
        assert!(result.as_ref().err().unwrap().contains("Static-prop"));
        assert!(
            fixture
                .session
                .finish_launch(Actor::Agent, &origin, result)
                .is_err()
        );
        assert_eq!(std::fs::read_dir(game.join("maps")).unwrap().count(), 1);
        assert!(!game.join("downloadlists").exists());
        assert!(!fixture.session.game_running());
    }
}

#[test]
fn content_reload_retires_only_prop_launches_and_requires_a_fresh_build() {
    for actor in [Actor::User, Actor::Agent] {
        for models in [false, true] {
            let mut fixture = Fixture::new(actor, "6");
            fixture.compile(actor);
            let output = std::fs::read(&fixture.output).unwrap();
            let request = fixture.session.begin_launch(actor).unwrap();
            let origin = request.origin();
            if models {
                fixture.session.refresh_model_assets(actor);
            } else {
                fixture.session.reload_material_assets(actor);
            }
            fixture
                .session
                .compiled
                .as_ref()
                .unwrap()
                .static_prop_sources
                .as_ref()
                .unwrap()
                .guard
                .verify_resolved_sources(&fixture.session.game, &compiler::Control::default())
                .unwrap();
            assert!(
                request
                    .verify_compiled_sources()
                    .unwrap_err()
                    .contains("Content changed")
            );
            let result = request.run();
            assert!(
                fixture
                    .session
                    .finish_launch(actor, &origin, result)
                    .err()
                    .unwrap()
                    .contains("Content changed")
            );
            let status = fixture.session.compile_status();
            assert_eq!(status["job"]["stale"], true);
            assert!(
                status["job"]["static_prop_source_issue"]
                    .as_str()
                    .unwrap()
                    .contains("Content changed")
            );
            assert_eq!(std::fs::read(&fixture.output).unwrap(), output);
            fixture.compile(actor);
            assert_eq!(fixture.session.compile_status()["job"]["stale"], false);
            assert!(fixture.session.launch_readiness().is_ok());
        }
    }
    let (directory, game, _) = crate::test_assets::game_process(false);
    let mut session = Session::new(Ok(game));
    session.replace_document(Document::new(crate::test_assets::room()), None);
    let result = session
        .begin_compile(Actor::Agent, Some(&directory.path().join("plain.bsp")))
        .unwrap()
        .run();
    session.finish_compile(Actor::Agent, &result);
    let request = session.begin_launch(Actor::Agent).unwrap();
    session.refresh_model_assets(Actor::Agent);
    request.verify_compiled_sources().unwrap();
    let origin = request.origin();
    session
        .finish_launch(
            Actor::Agent,
            &origin,
            Err("test completed without launching".into()),
        )
        .err()
        .unwrap();
}

#[test]
fn repair_does_not_rebaseline_a_completed_guard_and_recompile_restores_readiness() {
    let mut fixture = Fixture::new(Actor::Agent, "6");
    fixture.compile(Actor::Agent);
    let path = fixture.assets.join(MODEL);
    let original = std::fs::read(&path).unwrap();
    let output = std::fs::read(&fixture.output).unwrap();
    fixture.mutate("model");
    assert_eq!(fixture.session.compile_status()["job"]["stale"], true);
    std::fs::write(&path, original).unwrap();
    assert_eq!(fixture.session.compile_status()["job"]["stale"], true);
    assert!(fixture.session.launch_readiness().is_err());
    assert_eq!(std::fs::read(&fixture.output).unwrap(), output);
    fixture.compile(Actor::Agent);
    assert_eq!(fixture.session.compile_status()["job"]["stale"], false);
    assert!(fixture.session.launch_readiness().is_ok());
}

#[test]
fn current_lookup_context_is_checked_without_replacing_the_original_game_context() {
    for staged in [false, true] {
        let mut fixture = Fixture::new(Actor::Agent, "6");
        let other = fixture.directory.path().join("other");
        std::fs::create_dir_all(other.join("models/props")).unwrap();
        std::fs::copy(fixture.assets.join(MODEL), other.join(MODEL)).unwrap();
        let result = Ok(fixture.request(Actor::Agent).run().unwrap());
        if !staged {
            fixture.session.finish_compile(Actor::Agent, &result);
        }
        let game = GameFs {
            game_dir: fixture.session.game.game_dir.clone(),
            base_dir: fixture.session.game.base_dir.clone(),
            mounts: vec![
                MountedPath {
                    ids: vec!["game".into()],
                    mount: Mount::Directory(other),
                },
                MountedPath {
                    ids: vec!["game".into()],
                    mount: Mount::Directory(fixture.higher.clone()),
                },
                MountedPath {
                    ids: vec!["game".into()],
                    mount: Mount::Directory(fixture.assets.clone()),
                },
            ],
        };
        fixture.session.game = Arc::new(game);
        result
            .as_ref()
            .unwrap()
            .static_prop_sources
            .as_ref()
            .unwrap()
            .verify(&compiler::Control::default())
            .unwrap();
        if staged {
            fixture.session.finish_compile(Actor::Agent, &result);
            assert!(!result.as_ref().unwrap().published());
            assert_eq!(std::fs::read(&fixture.output).unwrap(), b"previous BSP");
        } else {
            assert_eq!(fixture.session.compile_status()["job"]["stale"], true);
            assert!(
                fixture.session.compile_status()["job"]["static_prop_source_issue"].is_string()
            );
            assert!(fixture.session.launch_readiness().is_err());
            assert_eq!(
                fixture.session.baked_lightmap_build_state()["status"],
                "stale"
            );
        }
    }
}

#[test]
fn unused_phy_hidden_props_and_mixed_case_classes_follow_actual_consumption() {
    for solid in ["0", "2"] {
        let mut fixture = Fixture::new(Actor::Agent, solid);
        fixture.compile(Actor::Agent);
        fixture.mutate("physics");
        assert_eq!(fixture.session.compile_status()["job"]["stale"], false);
        assert!(fixture.session.launch_readiness().is_ok());
    }
    let mut fixture = Fixture::new(Actor::Agent, "6");
    let prop = fixture.session.document.vmf().entities.last().unwrap().id;
    fixture
        .session
        .document
        .edit("mixed-case prop", |tx| {
            tx.entity_mut(prop)?
                .properties
                .set("classname", "PrOp_StAtIc");
            Ok(())
        })
        .unwrap();
    let request = fixture.request(Actor::Agent);
    assert!(request.destination_stamp.is_some());
    let result = request.run().unwrap();
    assert!(!result.published());
    assert!(result.static_prop_sources.is_some());
    fixture.session.finish_compile(Actor::Agent, &Ok(result));
    fixture.mutate("model");
    assert_eq!(fixture.session.compile_status()["job"]["stale"], true);

    let mut fixture = Fixture::new(Actor::Agent, "6");
    let prop = fixture.session.document.vmf().entities.last().unwrap().id;
    fixture
        .session
        .document
        .edit("hidden prop", |tx| {
            tx.entity_mut(prop)?.hidden = true;
            Ok(())
        })
        .unwrap();
    let request = fixture.request(Actor::Agent);
    assert!(request.destination_stamp.is_some());
    let result = request.run().unwrap();
    assert!(!result.published());
    assert!(result.output.static_prop_sources.is_none());
    fixture.session.finish_compile(Actor::Agent, &Ok(result));
    fixture.mutate("model");
    assert_eq!(fixture.session.compile_status()["job"]["stale"], false);
}
