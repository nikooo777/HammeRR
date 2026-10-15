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
    pub failed: usize,
    pub failures: Vec<Failure>,
    pub summary: String,
}

impl Session {
    pub fn material_reload_report(&self) -> Option<&Report> {
        self.material_reload.as_ref()
    }

    pub fn reload_material_assets(&mut self, actor: Actor) -> Report {
        let mut result = self.materials.reload(&self.game);
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
        let summary = format!(
            "Reloaded {} cached materials · {} unavailable previews",
            result.reloaded, result.failed
        );
        let report = Report {
            generation: self.materials.generation(),
            reloaded: result.reloaded,
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
