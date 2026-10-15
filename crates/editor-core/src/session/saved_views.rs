use map_core::saved_views::{View, Views};
use serde_json::{Value, json};

use super::*;

pub type Guard = (u64, u64);

fn camera(view: &View) -> Camera {
    Camera {
        position: view.position.as_vec3(),
        projection: Projection::parse(&view.projection).unwrap(),
        yaw_degrees: view.yaw as f32,
        pitch_degrees: view.pitch as f32,
        fov_y_degrees: view.fov as f32,
        ortho_height: view.ortho_height as f32,
    }
}

impl Session {
    pub fn camera_views(&self) -> Result<Views, String> {
        Views::parse(self.document.vmf())
    }

    pub fn camera_view_guard(&self) -> Guard {
        (self.document_generation, self.document.revision())
    }

    fn check_camera_view_guard(&self, guard: Option<Guard>) -> Result<(), String> {
        if guard.is_some_and(|guard| guard != self.camera_view_guard()) {
            return Err("Map changed; refresh the saved views before applying this action".into());
        }
        Ok(())
    }

    pub fn camera_views_json(&self) -> Result<Value, String> {
        let views = self.camera_views()?;
        Ok(
            json!({"generation":self.document_generation,"revision":self.document.revision(),
                "max_views":map_core::saved_views::MAX_VIEWS,"max_name_characters":map_core::saved_views::MAX_NAME_CHARS,
                "views":views.views.iter().map(|view|json!({"id":view.id,"name":view.name,
                    "camera":{"position":view.position.to_array(),"projection":view.projection,"yaw":view.yaw,"pitch":view.pitch,"fov":view.fov,"ortho_height":view.ortho_height}})).collect::<Vec<_>>()
            }),
        )
    }

    pub fn save_camera_view(
        &mut self,
        actor: Actor,
        name: &str,
        replace: Option<u32>,
        guard: Option<Guard>,
    ) -> Result<u32, String> {
        self.check_camera_view_guard(guard)?;
        let mut views = self.camera_views()?;
        let id = replace.map(Ok).unwrap_or_else(|| views.next_id())?;
        let camera = self.camera;
        let mut view = View {
            id,
            name: name.trim().into(),
            projection: camera.projection.name().into(),
            position: camera.position.as_dvec3(),
            yaw: camera.yaw_degrees as f64,
            pitch: camera.pitch_degrees as f64,
            fov: camera.fov_y_degrees as f64,
            ortho_height: camera.ortho_height as f64,
            extra: Vec::new(),
        };
        if replace.is_some() {
            let existing = views
                .views
                .iter_mut()
                .find(|view| view.id == id)
                .ok_or("Saved view no longer exists")?;
            view.extra = existing.extra.clone();
            *existing = view;
        } else {
            views.views.push(view);
        }
        views.validate()?;
        self.edit(actor, "save camera view", |tx| views.write(tx))?;
        Ok(id)
    }

    pub fn rename_camera_view(
        &mut self,
        actor: Actor,
        id: u32,
        name: &str,
        guard: Option<Guard>,
    ) -> Result<(), String> {
        self.check_camera_view_guard(guard)?;
        let mut views = self.camera_views()?;
        views
            .views
            .iter_mut()
            .find(|view| view.id == id)
            .ok_or("Saved view no longer exists")?
            .name = name.trim().into();
        views.validate()?;
        self.edit(actor, "rename camera view", |tx| views.write(tx))?;
        Ok(())
    }

    pub fn delete_camera_view(
        &mut self,
        actor: Actor,
        id: u32,
        guard: Option<Guard>,
    ) -> Result<(), String> {
        self.check_camera_view_guard(guard)?;
        let mut views = self.camera_views()?;
        if !views.views.iter().any(|view| view.id == id) {
            return Err("Saved view no longer exists".into());
        }
        views.views.retain(|view| view.id != id);
        self.edit(actor, "delete camera view", |tx| views.write(tx))?;
        Ok(())
    }

    pub fn restore_camera_view(
        &mut self,
        actor: Actor,
        id: u32,
        guard: Option<Guard>,
    ) -> Result<(), String> {
        self.check_camera_view_guard(guard)?;
        let views = self.camera_views()?;
        let view = views
            .views
            .iter()
            .find(|view| view.id == id)
            .ok_or("Saved view no longer exists")?;
        self.set_camera(camera(view));
        self.log(actor, format!("restored view {}", view.name));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_actors_save_restore_rename_replace_delete_and_reload_camera_views() {
        for actor in [Actor::User, Actor::Agent] {
            let mut session = Session::new(Err("no assets".into()));
            let mut snapshots = Vec::new();
            for projection in [
                Projection::Perspective,
                Projection::Top,
                Projection::Front,
                Projection::Side,
            ] {
                session.set_projection(actor, projection);
                let mut camera = session.camera();
                camera.ortho_height = 1733.125;
                camera.fov_y_degrees = 71.5;
                session.set_camera(camera);
                let guard = session.camera_view_guard();
                let id = session
                    .save_camera_view(actor, projection.name(), None, Some(guard))
                    .unwrap();
                snapshots.push((id, camera));
            }
            let history = session.document.undo_labels().count();
            for &(id, camera) in &snapshots {
                session.restore_camera_view(actor, id, None).unwrap();
                assert_eq!(session.camera(), camera);
            }
            assert_eq!(session.document.undo_labels().count(), history);
            let guard = session.camera_view_guard();
            session
                .rename_camera_view(actor, 1, "Entry", Some(guard))
                .unwrap();
            assert!(session.delete_camera_view(actor, 1, Some(guard)).is_err());
            assert!(session.restore_camera_view(actor, 1, Some(guard)).is_err());
            let saved = session.camera_views().unwrap();
            session.document.undo().unwrap();
            assert_eq!(session.camera_views().unwrap().views[0].name, "perspective");
            session.document.redo().unwrap();
            assert_eq!(session.camera_views().unwrap(), saved);
            let current = session.camera();
            session
                .save_camera_view(actor, "Entry updated", Some(1), None)
                .unwrap();
            session.restore_camera_view(actor, 1, None).unwrap();
            assert_eq!(session.camera(), current);
            session.delete_camera_view(actor, 1, None).unwrap();
            assert!(session.restore_camera_view(actor, 1, None).is_err());
            session.document.undo().unwrap();
            let text = session.document.vmf().write(&Default::default());
            let expected = session.camera_views().unwrap();
            session.replace_document(Document::new(Vmf::parse(&text).unwrap()), None);
            assert_eq!(session.camera_views().unwrap(), expected);
            assert!(session.restore_camera_view(actor, 2, Some(guard)).is_err());
            session.restore_camera_view(actor, 2, None).unwrap();
            assert_eq!(session.camera(), snapshots[1].1);
        }
    }

    #[test]
    fn invalid_saved_view_edits_leave_camera_selection_history_and_metadata_unchanged() {
        let mut session = Session::new(Err("no assets".into()));
        session
            .save_camera_view(Actor::User, "Entry", None, None)
            .unwrap();
        let before = session.document.vmf().clone();
        let camera = session.camera();
        let history = session.document.undo_labels().count();
        for (name, replace) in [
            ("entry", None),
            ("", None),
            ("bad\nname", Some(1)),
            ("Missing", Some(9)),
        ] {
            assert!(
                session
                    .save_camera_view(Actor::Agent, name, replace, None)
                    .is_err()
            );
        }
        assert!(
            session
                .rename_camera_view(Actor::Agent, 1, "", None)
                .is_err()
        );
        assert!(session.delete_camera_view(Actor::Agent, 9, None).is_err());
        assert_eq!(session.document.vmf(), &before);
        assert_eq!(session.camera(), camera);
        assert_eq!(session.document.undo_labels().count(), history);
    }
}
