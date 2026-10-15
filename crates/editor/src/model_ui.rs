use std::cell::Cell;
use std::rc::Rc;

use slint::ComponentHandle;

use crate::{
    ModelWindow,
    runtime::{SharedSession, lock},
    session::{Actor, Session},
};

pub struct Controller {
    pub window: ModelWindow,
    target: Rc<Cell<Option<(u32, u64)>>>,
    guide: Rc<Cell<Option<u64>>>,
}

fn attachment_target(session: &Session, target: Option<(u32, u64)>) -> Result<u32, String> {
    let (id, generation) = target.ok_or("Select a model entity")?;
    if generation != session.document_generation() {
        return Err("Map changed; reopen model details".into());
    }
    Ok(id)
}

fn refresh_attachment(window: &ModelWindow, report: &serde_json::Value) {
    let attachments = report["attachments"].as_array().unwrap();
    window.set_attachments(slint::ModelRc::new(slint::VecModel::from(
        attachments
            .iter()
            .map(|attachment| attachment["name"].as_str().unwrap().into())
            .collect::<Vec<slint::SharedString>>(),
    )));
    let index =
        (window.get_attachment_index().max(0) as usize).min(attachments.len().saturating_sub(1));
    window.set_attachment_index(index as i32);
    window.set_attachment_info(
        attachments
            .get(index)
            .map(|attachment| {
                let point = &attachment["position"];
                format!(
                    "Position · X {:.2} · Y {:.2} · Z {:.2}\nBone {}{}",
                    point[0].as_f64().unwrap(),
                    point[1].as_f64().unwrap(),
                    point[2].as_f64().unwrap(),
                    attachment["bone"],
                    if attachment["world_aligned"] == true {
                        " · world-aligned axes"
                    } else {
                        ""
                    }
                )
            })
            .unwrap_or_default()
            .into(),
    );
}

fn show_attachment(
    window: &ModelWindow,
    session: &mut Session,
    target: Option<(u32, u64)>,
    guide: &Cell<Option<u64>>,
) {
    if !window.get_show_axes() {
        if let Some(token) = guide.take() {
            let _ = session.clear_model_attachment(token);
        }
        return;
    }
    let result = attachment_target(session, target).and_then(|id| {
        session.show_model_attachment(id, window.get_attachment_index().max(0) as usize + 1)
    });
    match result {
        Ok(report) => guide.set(report["token"].as_u64()),
        Err(error) => {
            if let Some(token) = guide.take() {
                let _ = session.clear_model_attachment(token);
            }
            window.set_show_axes(false);
            window.set_message(error.into());
        }
    }
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
            refresh_attachment(window, &report);
            let asset = &report["asset"];
            window.set_pose_info(if asset["pose"] == "reference" {
                format!("Reference pose · {} bones · body {} · LOD 0. Animation and scaling remain planned.",asset["reference_bones"],asset["body"]).into()
            } else {
                format!("Static model · body {} · LOD 0. Scaling remains planned.",asset["body"]).into()
            });
            window.set_bodygroups(slint::ModelRc::new(slint::VecModel::from(
                asset["bodygroups"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|group| group["variants"].as_array().unwrap().len() > 1)
                    .map(|group| {
                        format!(
                                    "{} · {}",
                                    group["label"].as_str().unwrap(),
                                    group["variants"][group["selected"].as_u64().unwrap() as usize]
                                        ["label"]
                                        .as_str()
                                        .unwrap()
                                )
                        .into()
                    })
                    .collect::<Vec<slint::SharedString>>(),
            )));
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
            } else if report["helper"] == true {
                "Selected bodygroups have no mesh; an editor helper is shown.".into()
            } else {
                "Model details are read-only. Reload models after changing model files.".into()
            });
        }
        Err(error) => {
            window.set_attachments(Default::default());
            window.set_attachment_info("".into());
            window.set_geometry("Model preview unavailable".into());
            window.set_skin_info("".into());
            window.set_bounds("".into());
            window.set_pose_info("".into());
            window.set_bodygroups(Default::default());
            window.set_materials(Default::default());
            window.set_message(error.into());
        }
    }
}

impl Controller {
    pub fn new(shared: &SharedSession) -> Result<Self, slint::PlatformError> {
        let window = ModelWindow::new()?;
        let target = Rc::new(Cell::new(None));
        let guide = Rc::new(Cell::new(None::<u64>));
        window.on_attachment_changed({
            let (window, shared, target, guide) = (
                window.as_weak(),
                shared.clone(),
                target.clone(),
                guide.clone(),
            );
            move || {
                let window = window.unwrap();
                let mut session = lock(&shared);
                refresh(&window, &session, target.get());
                show_attachment(&window, &mut session, target.get(), &guide);
            }
        });
        window.on_show_attachment({
            let (window, shared, target, guide) = (
                window.as_weak(),
                shared.clone(),
                target.clone(),
                guide.clone(),
            );
            move || show_attachment(&window.unwrap(), &mut lock(&shared), target.get(), &guide)
        });
        window.on_frame_attachment({
            let (window, shared, target) = (window.as_weak(), shared.clone(), target.clone());
            move || {
                let window = window.unwrap();
                let mut session = lock(&shared);
                let result = attachment_target(&session, target.get()).and_then(|id| {
                    session.frame_model_attachment(
                        Actor::User,
                        id,
                        window.get_attachment_index().max(0) as usize + 1,
                        window.get_aspect(),
                    )
                });
                if let Err(error) = result {
                    window.set_message(error.into());
                }
            }
        });
        window.on_reload({
            let (window, shared, target) = (window.as_weak(), shared.clone(), target.clone());
            move || {
                let mut session = lock(&shared);
                session.refresh_model_assets(Actor::User);
                refresh(&window.unwrap(), &session, target.get());
            }
        });
        window.on_cancel({
            let (window, shared, guide) = (window.as_weak(), shared.clone(), guide.clone());
            move || {
                if let Some(token) = guide.take() {
                    let _ = lock(&shared).clear_model_attachment(token);
                }
                window.unwrap().set_show_axes(false);
                let _ = window.unwrap().hide();
            }
        });
        window.window().on_close_requested({
            let window = window.as_weak();
            move || {
                window.unwrap().invoke_cancel();
                slint::CloseRequestResponse::HideWindow
            }
        });
        Ok(Self {
            window,
            target,
            guide,
        })
    }

    pub fn initialize(&self, session: &mut Session, id: u32) {
        if let Some(token) = self.guide.take() {
            let _ = session.clear_model_attachment(token);
        }
        self.window.set_show_axes(false);
        self.window.set_attachment_index(0);
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
        if let Some(token) = self.guide.get() {
            let report = session.model_attachment_guide();
            if report["token"] != token {
                self.guide.set(None);
                self.window.set_show_axes(false);
            } else if report["valid"] != true {
                self.window.set_show_axes(false);
            }
        }
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
                "prop_dynamic",
                Some(DVec3::ZERO),
                BTreeMap::from([("model".into(), "models/kleiner.mdl".into())]),
            )
            .unwrap()
        else {
            unreachable!()
        };
        let before = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let shared = Arc::new(Mutex::new(session));
        let controller = Controller::new(&shared).unwrap();
        controller.initialize(&mut lock(&shared), id);
        assert!(controller.window.get_geometry().contains("vertices"));
        assert!(
            !controller.window.get_geometry().contains("unavailable"),
            "{}",
            controller.window.get_message()
        );
        assert!(controller.window.get_materials().row_count() > 0);
        assert!(controller.window.get_pose_info().contains("Reference pose"));
        assert_eq!(controller.window.get_attachments().row_count(), 15);
        controller.window.set_attachment_index(4);
        controller.window.invoke_attachment_changed();
        assert!(controller.window.get_attachment_info().contains("Position"));
        controller.window.set_show_axes(true);
        controller.window.invoke_show_attachment();
        assert_eq!(
            lock(&shared).model_attachment_guide()["attachment"]["name"],
            "mouth"
        );
        let camera = lock(&shared).camera();
        controller.window.invoke_frame_attachment();
        assert_ne!(lock(&shared).camera(), camera);
        controller.window.show().unwrap();
        crate::ui::tests::save_ui_snapshot(controller.window.window(), "model-details");
        crate::ui::tests::save_ui_snapshot(controller.window.window(), "model-attachments");
        controller.window.invoke_reload();
        controller.refresh(&lock(&shared));
        assert!(!controller.window.get_show_axes());
        assert!(!controller.window.get_geometry().contains("unavailable"));
        controller.window.invoke_cancel();
        assert!(!controller.window.window().is_visible());
        assert!(lock(&shared).model_attachment_guide()["token"].is_null());
        controller.initialize(&mut lock(&shared), id);
        controller.window.set_show_axes(true);
        controller.window.invoke_show_attachment();
        let replacement = lock(&shared).show_model_attachment(id, 2).unwrap()["token"]
            .as_u64()
            .unwrap();
        controller.window.invoke_cancel();
        assert_eq!(lock(&shared).model_attachment_guide()["token"], replacement);
        lock(&shared).clear_model_attachment(replacement).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &before);
        assert_eq!(lock(&shared).document().undo_labels().count(), history);
    }
}
