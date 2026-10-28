use std::cell::Cell;
use std::rc::Rc;

use slint::ComponentHandle;

use crate::{
    ModelWindow,
    runtime::{DocumentAccess, DocumentContinuation},
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
                format!(
                    "Reference pose · {} bones · body {} · LOD 0. Animation remains planned.",
                    asset["reference_bones"], asset["body"]
                )
                .into()
            } else {
                format!("Static model · body {} · LOD 0.", asset["body"]).into()
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
            let mut skin_info = format!(
                "Skin {} · {} available · body parts: {}",
                asset["skin"].as_u64().unwrap_or(0),
                asset["skins"],
                asset["body_parts"]
            );
            if let Some(scale) = report["model_scale"]["effective"].as_f64() {
                skin_info.push_str(&format!("\nModel scale {scale} · about entity origin"));
            }
            if report["appearance"]["state"] == "texture" {
                skin_info.push_str(&format!(
                    "\nTexture · Opacity {}/255 · source texture alpha",
                    report["appearance"]["render_amount"]
                ));
            }
            window.set_skin_info(skin_info.into());
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

fn callback(
    window: &slint::Weak<ModelWindow>,
    continuation: &DocumentContinuation,
    body: impl FnOnce(ModelWindow, &mut Session),
) {
    let Some(window) = window.upgrade() else {
        return;
    };
    let result = continuation.upgrade().and_then(|access| {
        access.with_session(|session| {
            body(window.clone_strong(), session);
            Ok(())
        })
    });
    if let Err(error) = result {
        window.set_message(error.into());
    }
}

impl Controller {
    pub fn new_with_access(access: &DocumentAccess) -> Result<Self, slint::PlatformError> {
        let continuation = access.downgrade();
        let window = ModelWindow::new()?;
        let target = Rc::new(Cell::new(None));
        let guide = Rc::new(Cell::new(None::<u64>));
        window.on_attachment_changed({
            let (window, continuation, target, guide) = (
                window.as_weak(),
                continuation.clone(),
                target.clone(),
                guide.clone(),
            );
            move || {
                callback(&window, &continuation, |window, session| {
                    refresh(&window, session, target.get());
                    show_attachment(&window, session, target.get(), &guide);
                })
            }
        });
        window.on_show_attachment({
            let (window, continuation, target, guide) = (
                window.as_weak(),
                continuation.clone(),
                target.clone(),
                guide.clone(),
            );
            move || {
                callback(&window, &continuation, |window, session| {
                    show_attachment(&window, session, target.get(), &guide)
                })
            }
        });
        window.on_frame_attachment({
            let (window, continuation, target) =
                (window.as_weak(), continuation.clone(), target.clone());
            move || {
                callback(&window, &continuation, |window, session| {
                    let result = attachment_target(session, target.get()).and_then(|id| {
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
                })
            }
        });
        window.on_reload({
            let (window, continuation, target) =
                (window.as_weak(), continuation.clone(), target.clone());
            move || {
                callback(&window, &continuation, |window, session| {
                    let snap_preview_cancelled = session.refresh_model_assets(Actor::User);
                    refresh(&window, session, target.get());
                    if snap_preview_cancelled {
                        window.set_message(
                            "Model assets reloaded; preview snap to grid again".into(),
                        );
                    }
                })
            }
        });
        window.on_cancel({
            let (window, continuation, guide) =
                (window.as_weak(), continuation.clone(), guide.clone());
            move || {
                let Some(local_window) = window.upgrade() else {
                    return;
                };
                let token = guide.take();
                callback(&window, &continuation, |_, session| {
                    if let Some(token) = token {
                        let _ = session.clear_model_attachment(token);
                    }
                });
                local_window.set_show_axes(false);
                let _ = local_window.hide();
            }
        });
        window.window().on_close_requested({
            let window = window.as_weak();
            move || {
                if let Some(window) = window.upgrade() {
                    window.invoke_cancel();
                }
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
