use serde::Serialize;

use super::{Actor, Session};

#[derive(Debug, Clone, Serialize)]
pub struct Failure {
    pub material: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub generation: u64,
    pub reloaded: usize,
    pub models_refreshed: usize,
    pub models_changed: usize,
    pub failed: usize,
    pub failures: Vec<Failure>,
    pub summary: String,
}

impl Session {
    pub fn material_reload_report(&self) -> Option<&Report> {
        self.material_reload.as_ref()
    }

    pub fn reload_material_assets(&mut self, actor: Actor) -> Report {
        self.cancel_geometry_drags();
        self.cancel_selection_previews();
        self.advance_content_epoch();
        let models = self.models.borrow_mut().reload_materials(&self.game);
        let mut result = self
            .materials
            .reload_with_names(&self.game, models.materials);
        for (path, error) in models.failures {
            result.record_failure(path, error);
        }
        self.material_index = None;
        match self.search_materials("", usize::MAX) {
            Ok(names) => self.material_index = Some(names),
            Err(error) => result.record_failure("Material catalog".into(), error),
        }
        let previous_scene = self.scene_serial;
        self.scene_serial += 1;
        self.model_browser
            .materials_reloaded(previous_scene, self.scene_serial);
        self.model_attachments
            .materials_reloaded(previous_scene, self.scene_serial);
        self.particles
            .materials_reloaded(previous_scene, self.scene_serial);
        self.sounds
            .materials_reloaded(previous_scene, self.scene_serial);
        self.view_serial += 1;
        self.preflight_sky3d_preview();
        let summary = format!(
            "Reloaded {} cached materials · {} model material sets refreshed · {} unavailable previews",
            result.reloaded, models.refreshed, result.failed
        );
        let report = Report {
            generation: self.materials.generation(),
            reloaded: result.reloaded,
            models_refreshed: models.refreshed,
            models_changed: models.changed,
            failed: result.failed,
            failures: result
                .failures
                .into_iter()
                .map(|(material, error)| Failure { material, error })
                .collect(),
            summary,
        };
        self.log(actor, report.summary.clone());
        self.material_reload = Some(report.clone());
        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DVec3;
    use map_core::edit::Transform;
    use map_core::query::Ray;

    #[test]
    fn reload_retires_captured_selection_eligibility_for_paint_volume_and_marquee() {
        use crate::session::{Actor, Pointer, SelectMode, paint, volume};
        use glam::DVec2;
        use map_core::region::Match;
        use render::{Camera, Projection};
        for kind in ["paint", "volume", "area"] {
            let (directory, game) = crate::test_assets::game();
            let mut session = Session::new(Ok(game));
            session
                .create_block(
                    Actor::User,
                    DVec3::ZERO,
                    DVec3::splat(64.0),
                    "test/checker",
                    false,
                )
                .unwrap();
            session.select(&[], SelectMode::Replace).unwrap();
            session.set_camera(Camera::planar(
                Projection::Top,
                glam::Vec3::splat(32.0),
                256.0,
            ));
            let mut view = session.view();
            view.tool_textures = false;
            session.set_view(view);
            let ray = Ray {
                origin: DVec3::new(32.0, 32.0, 128.0),
                direction: -DVec3::Z,
            };
            assert!(session.pick(&ray).is_some());
            let before = session.document.vmf().clone();
            let history = session.history_snapshot();
            let token = match kind {
                "paint" => session
                    .begin_paint_selection(paint::Spec {
                        camera: session.camera(),
                        pointer: Pointer {
                            position: DVec2::new(16.0, 128.0),
                            size: (256, 256),
                        },
                        radius: 4.0,
                        mode: SelectMode::Replace,
                        faces: false,
                    })
                    .unwrap(),
                "volume" => session
                    .preview_volume_selection(volume::Spec {
                        mins: DVec3::splat(-1.0),
                        maxs: DVec3::splat(65.0),
                        rule: Match::Intersecting,
                        mode: SelectMode::Replace,
                        faces: false,
                    })
                    .unwrap(),
                _ => {
                    session
                        .preview_area_selection(
                            Actor::User,
                            session.camera(),
                            Pointer {
                                position: DVec2::new(96.0, 96.0),
                                size: (256, 256),
                            },
                            DVec2::new(160.0, 160.0),
                            Match::Intersecting,
                            SelectMode::Replace,
                        )
                        .unwrap();
                    0
                }
            };
            std::fs::write(
                directory.path().join("materials/test/checker.vmt"),
                "LightmappedGeneric { $basetexture test/checker %compilenodraw 1 }",
            )
            .unwrap();
            session.reload_material_assets(Actor::Agent);
            assert!(session.pick(&ray).is_none());
            match kind {
                "paint" => {
                    assert!(
                        session
                            .update_paint_selection(token, &[DVec2::splat(128.0)])
                            .is_err()
                    );
                    assert!(session.commit_paint_selection(Actor::User, token).is_err());
                }
                "volume" => assert!(session.commit_volume_selection(Actor::User, token).is_err()),
                _ => assert!(session.commit_area_selection().is_err()),
            }
            assert!(session.selected_ids().is_empty());
            assert_eq!(session.document.vmf(), &before);
            assert_eq!(session.history_snapshot(), history);
        }
    }

    #[test]
    fn reload_replaces_cached_pixels_failures_and_repairs_without_changing_source_or_draft() {
        for actor in [Actor::User, Actor::Agent] {
            let (directory, game) = crate::test_assets::game();
            let mut session = Session::new(Ok(game));
            session
                .edit(Actor::User, "remove fixture sky", |tx| {
                    tx.world_properties_mut().set("skyname", "");
                    Ok(())
                })
                .unwrap();
            let id = session
                .create_block(
                    actor,
                    DVec3::ZERO,
                    DVec3::splat(64.0),
                    "test/checker",
                    false,
                )
                .unwrap();
            session.build_scene();
            let material_id = session.materials.load(&session.game, "test/checker");
            let initial = session
                .material_preview("test/checker")
                .unwrap()
                .image
                .unwrap();
            let source = session.document.vmf().clone();
            let history = session
                .document
                .undo_labels()
                .map(str::to_owned)
                .collect::<Vec<_>>();
            let camera = session.camera();
            session
                .preview_transform(
                    actor,
                    &[id],
                    Transform {
                        translation: DVec3::X * 32.0,
                        ..Default::default()
                    },
                    None,
                )
                .unwrap();
            let draft = session.render_document().vmf().clone();
            let texture = directory.path().join("materials/test/checker.vtf");
            let mut bytes = std::fs::read(&texture).unwrap();
            for pixel in bytes[80..].as_chunks_mut::<4>().0 {
                pixel.copy_from_slice(&[220, 20, 30, 255]);
            }
            bytes[16..18].copy_from_slice(&32u16.to_le_bytes());
            bytes[18..20].copy_from_slice(&64u16.to_le_bytes());
            std::fs::write(&texture, &bytes).unwrap();
            let report = session.reload_material_assets(actor);
            assert_eq!(report.failed, 0);
            let preview = session.material_preview("test/checker").unwrap();
            assert_eq!((preview.width, preview.height), (32, 64));
            assert_eq!(
                session.materials.load(&session.game, "test/checker"),
                material_id
            );
            assert_ne!(
                session
                    .material_preview("test/checker")
                    .unwrap()
                    .image
                    .unwrap(),
                initial
            );
            std::fs::write(&texture, b"broken VTF").unwrap();
            let report = session.reload_material_assets(actor);
            assert_eq!(report.failed, 1);
            assert!(report.failures[0].error.contains("checker.vtf"));
            assert!(session.materials.get(material_id).base.is_none());
            assert!(session.material_preview("test/checker").is_err());
            std::fs::write(&texture, bytes).unwrap();
            assert_eq!(session.reload_material_assets(actor).failed, 0);
            assert!(!session.material_preview("test/checker").unwrap().missing);
            assert_eq!(session.document.vmf(), &source);
            assert_eq!(session.render_document().vmf(), &draft);
            assert!(session.transform_preview().is_some());
            assert_eq!(session.document.undo_labels().collect::<Vec<_>>(), history);
            assert_eq!(session.camera(), camera);
            assert!(
                !session
                    .materials
                    .get(render::MaterialLibrary::EDITOR)
                    .missing
            );
        }
    }

    #[test]
    fn reload_rescans_catalog_and_updates_tool_picking_and_map_check_freshness() {
        let (directory, game) = crate::test_assets::game();
        let mut session = Session::new(Ok(game));
        session
            .edit(Actor::User, "remove fixture sky", |tx| {
                tx.world_properties_mut().set("skyname", "");
                Ok(())
            })
            .unwrap();
        session
            .create_block(
                Actor::User,
                DVec3::ZERO,
                DVec3::splat(64.0),
                "test/checker",
                false,
            )
            .unwrap();
        let ray = Ray {
            origin: DVec3::new(32.0, 32.0, 128.0),
            direction: -DVec3::Z,
        };
        let mut view = session.view();
        view.tool_textures = false;
        session.set_view(view);
        assert!(session.pick(&ray).is_some());
        assert_eq!(session.material_catalog("", "all", 0, 36).unwrap().total, 1);
        session.run_map_checks(Actor::User);
        assert!(!session.map_checks_stale());
        let folder = directory.path().join("materials/test");
        std::fs::write(
            folder.join("checker.vmt"),
            "LightmappedGeneric { $basetexture test/checker %compilenodraw 1 }",
        )
        .unwrap();
        std::fs::write(
            folder.join("new.vmt"),
            "LightmappedGeneric { $basetexture test/checker }",
        )
        .unwrap();
        session.reload_material_assets(Actor::Agent);
        assert!(session.map_checks_stale());
        assert!(session.pick(&ray).is_none());
        assert_eq!(session.material_catalog("", "all", 0, 36).unwrap().total, 2);
        std::fs::remove_file(folder.join("new.vmt")).unwrap();
        session.reload_material_assets(Actor::User);
        assert_eq!(session.material_catalog("", "all", 0, 36).unwrap().total, 1);
    }

    #[cfg(unix)]
    #[test]
    fn model_material_failures_preserve_geometry_choices_and_owned_previews_until_repaired() {
        use source_formats::gamefs::{Mount, MountedPath};
        use std::{collections::BTreeMap, sync::Arc};

        for actor in [Actor::User, Actor::Agent] {
            let Ok(mut game) = crate::startup::mount_game(None) else {
                return;
            };
            let directory = tempfile::tempdir().unwrap();
            game.mounts.insert(
                0,
                MountedPath {
                    ids: vec!["game".into()],
                    mount: Mount::Directory(directory.path().into()),
                },
            );
            let mut session = Session::new(Ok(game));
            let path = "models/kleiner.mdl";
            let map_core::ObjectId::Entity(id) = session
                .create_entity(
                    actor,
                    "prop_dynamic",
                    Some(DVec3::ZERO),
                    BTreeMap::from([("model".into(), path.into())]),
                )
                .unwrap()
            else {
                panic!()
            };
            let initial = session
                .models
                .borrow_mut()
                .get(&session.game, path)
                .unwrap();
            let initial_materials = initial.materials().unwrap().to_vec();
            let before = session.inspect_entity_model(id).unwrap();
            let guide = session.show_model_attachment(id, 1).unwrap();
            let choice = session.begin_model_choice(id).unwrap()["token"]
                .as_u64()
                .unwrap();
            session.choose_model(choice, path, 0).unwrap();
            let source = session.document.vmf().clone();
            let history = session
                .document
                .undo_labels()
                .map(str::to_owned)
                .collect::<Vec<_>>();
            let camera = session.camera();
            let materials = directory.path().join("materials");
            std::fs::create_dir(&materials).unwrap();
            let component = initial.model.material_directories[0]
                .split('/')
                .next()
                .unwrap();
            assert!(!component.is_empty());
            std::os::unix::fs::symlink(component, materials.join(component)).unwrap();
            let report = session.reload_material_assets(actor);
            assert!(report.models_changed > 0);
            assert!(
                report
                    .failures
                    .iter()
                    .any(|failure| failure.material == path)
            );
            let current = session
                .models
                .borrow_mut()
                .get(&session.game, path)
                .unwrap();
            assert!(Arc::ptr_eq(&initial, &current));
            assert!(current.materials().is_err());
            let failed = session.inspect_entity_model(id).unwrap();
            assert_eq!(failed["displayed"], true);
            assert_eq!(failed["world_bounds"], before["world_bounds"]);
            assert!(failed["asset"]["material_error"].is_string());
            assert!(session.model_choice(choice).unwrap()["asset"]["material_error"].is_string());
            assert_eq!(session.model_attachment_guide(), guide);
            let (scene, _, _) = session.model_preview_scene(path, 0, 0, 1.5).unwrap();
            assert_eq!(scene.stats.model_entities, 1);
            assert!(scene.stats.missing_materials > 0);
            std::fs::remove_file(materials.join(component)).unwrap();
            session.reload_material_assets(actor);
            let repaired = session
                .models
                .borrow_mut()
                .get(&session.game, path)
                .unwrap();
            assert!(Arc::ptr_eq(&initial, &repaired));
            assert_eq!(&*repaired.materials().unwrap(), initial_materials);
            assert!(session.model_choice(choice).unwrap()["asset"]["material_error"].is_null());
            assert_eq!(session.model_attachment_guide(), guide);
            session
                .select(&[], super::super::SelectMode::Replace)
                .unwrap();
            session.reload_material_assets(actor);
            assert!(session.model_choice(choice).is_err());
            assert_eq!(session.model_attachment_guide()["valid"], false);
            session
                .preview_transform(
                    actor,
                    &[map_core::ObjectId::Entity(id)],
                    Transform {
                        translation: DVec3::X * 32.0,
                        ..Default::default()
                    },
                    None,
                )
                .unwrap();
            let draft = session.render_document().vmf().clone();
            session.reload_material_assets(actor);
            assert_eq!(session.render_document().vmf(), &draft);
            assert_eq!(session.document.vmf(), &source);
            assert_eq!(session.document.undo_labels().collect::<Vec<_>>(), history);
            assert_eq!(session.camera(), camera);
            session.commit_transform_preview(actor).unwrap();
            assert_eq!(session.document.vmf(), &draft);
        }
    }
}

#[cfg(test)]
mod sky_tests {
    #[test]
    fn reload_replaces_cached_sky_faces_and_reports_then_repairs_failed_faces() {
        let (directory, game) = crate::test_assets::game();
        let folder = directory.path().join("materials/skybox");
        std::fs::create_dir(&folder).unwrap();
        for suffix in render::sky::SUFFIXES {
            std::fs::write(
                folder.join(format!("reload{suffix}.vmt")),
                "UnlitGeneric { $basetexture test/checker }",
            )
            .unwrap();
        }
        let mut library = render::MaterialLibrary::default();
        let first = library.skybox(&game, "reload");
        assert!(first.ready());
        std::fs::write(folder.join("reloadrt.vmt"), "broken {").unwrap();
        let generation = library.generation();
        let report = library.reload(&game);
        assert_ne!(library.generation(), generation);
        assert_eq!(report.failed, 1);
        assert_eq!(report.failures[0].0, "skybox/reloadrt");
        let failed = library.skybox(&game, "reload");
        assert!(!std::sync::Arc::ptr_eq(&first, &failed));
        assert!(!failed.ready());
        std::fs::write(
            folder.join("reloadrt.vmt"),
            "UnlitGeneric { $basetexture test/checker }",
        )
        .unwrap();
        assert_eq!(library.reload(&game).failed, 0);
        assert!(library.skybox(&game, "reload").ready());
    }
}
