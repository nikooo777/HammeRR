use std::cell::Cell;
use std::rc::Rc;

use slint::ComponentHandle;

use crate::{
    ModelWindow,
    mcp::{SharedSession, lock},
    session::{Actor, Session},
};

pub struct Controller {
    pub window: ModelWindow,
    target: Rc<Cell<Option<(u32, u64)>>>,
}

fn refresh(window: &ModelWindow, session: &Session, target: Option<(u32, u64)>) {
    let result = target
        .ok_or("Select a model entity".to_owned())
        .and_then(|(id, generation)| {
            if generation != session.document_generation() {
                return Err("Map changed; reopen model details".into());
            }
            session.inspect_entity_model(id)
        });
    match result {
        Ok(report) => {
            let asset = &report["asset"];
            window.set_model_path(asset["path"].as_str().unwrap_or_default().into());
            window.set_geometry(
                format!(
                    "Geometry · {} vertices · {} triangles · meshes: {}",
                    asset["vertices"], asset["triangles"], asset["meshes"]
                )
                .into(),
            );
            window.set_skin_info(
                format!(
                    "Skin {} · {} available · body parts: {}",
                    asset["skin"].as_u64().unwrap_or(0),
                    asset["skins"],
                    asset["body_parts"]
                )
                .into(),
            );
            let bounds = report["world_bounds"]
                .as_array()
                .map(|bounds| {
                    let point = |index: usize, axis: usize| {
                        bounds[index][axis].as_f64().unwrap_or_default()
                    };
                    format!(
                        "World bounds · X {:.2} to {:.2} · Y {:.2} to {:.2} · Z {:.2} to {:.2}",
                        point(0, 0),
                        point(1, 0),
                        point(0, 1),
                        point(1, 1),
                        point(0, 2),
                        point(1, 2)
                    )
                })
                .unwrap_or_default();
            window.set_bounds(bounds.into());
            window.set_materials(
                Rc::new(slint::VecModel::from(
                    asset["materials"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|value| value.as_str().map(Into::into))
                        .collect::<Vec<slint::SharedString>>(),
                ))
                .into(),
            );
            window.set_message(if let Some(error) = report["error"].as_str() {
                error.into()
            } else if report["hidden"].as_bool() == Some(true) {
                "This entity is hidden.".into()
            } else {
                "Model details are read-only. Reload models after changing model files.".into()
            });
        }
        Err(error) => {
            window.set_geometry("Model preview unavailable".into());
            window.set_skin_info("".into());
            window.set_bounds("".into());
            window.set_materials(Default::default());
            window.set_message(error.into());
        }
    }
}

impl Controller {
    pub fn new(shared: &SharedSession) -> Result<Self, slint::PlatformError> {
        let window = ModelWindow::new()?;
        let target = Rc::new(Cell::new(None));
        window.on_reload({
            let (window, shared, target) = (window.as_weak(), shared.clone(), target.clone());
            move || {
                let mut session = lock(&shared);
                session.refresh_model_assets(Actor::User);
                refresh(&window.unwrap(), &session, target.get());
            }
        });
        window.on_cancel({
            let window = window.as_weak();
            move || {
                let _ = window.unwrap().hide();
            }
        });
        Ok(Self { window, target })
    }

    pub fn initialize(&self, session: &Session, id: u32) {
        self.target.set(Some((id, session.document_generation())));
        self.window.set_model_path(
            session
                .render_document()
                .entity(id)
                .and_then(|entity| entity.properties.get("model"))
                .unwrap_or("Model details")
                .into(),
        );
        self.refresh(session);
    }

    pub fn refresh(&self, session: &Session) {
        refresh(&self.window, session, self.target.get());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DVec3;
    use map_core::ObjectId;
    use slint::Model;
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    #[test]
    fn model_details_inspection_reload_and_cancel_preserve_the_source() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let Ok(game) = crate::mount_game(None) else {
            eprintln!("CS:S not installed; skipping");
            return;
        };
        let mut session = Session::new(Ok(game));
        let ObjectId::Entity(id) = session
            .create_entity(
                Actor::User,
                "prop_static",
                Some(DVec3::ZERO),
                BTreeMap::from([(
                    "model".into(),
                    "models/props_junk/wood_crate001a.mdl".into(),
                )]),
            )
            .unwrap()
        else {
            unreachable!()
        };
        let before = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let shared = Arc::new(Mutex::new(session));
        let controller = Controller::new(&shared).unwrap();
        controller.initialize(&lock(&shared), id);
        assert!(controller.window.get_geometry().contains("vertices"));
        assert!(
            !controller.window.get_geometry().contains("unavailable"),
            "{}",
            controller.window.get_message()
        );
        assert!(controller.window.get_materials().row_count() > 0);
        controller.window.show().unwrap();
        crate::tests::save_ui_snapshot(controller.window.window(), "model-details");
        controller.window.invoke_reload();
        assert!(!controller.window.get_geometry().contains("unavailable"));
        controller.window.invoke_cancel();
        assert!(!controller.window.window().is_visible());
        assert_eq!(lock(&shared).document().vmf(), &before);
        assert_eq!(lock(&shared).document().undo_labels().count(), history);
    }
}
