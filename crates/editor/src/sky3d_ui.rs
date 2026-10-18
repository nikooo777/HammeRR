use std::cell::RefCell;
use std::rc::Rc;

use serde_json::Value;
use slint::{ComponentHandle, VecModel};

use crate::Sky3dWindow;
use crate::runtime::{DocumentAccess, DocumentContinuation};
use crate::session::{Actor, Session};

#[derive(Default)]
struct Form {
    cameras: Vec<Value>,
}

pub struct Controller {
    pub window: Sky3dWindow,
    form: Rc<RefCell<Form>>,
}

fn camera_id(form: &Form, index: i32) -> Option<u32> {
    usize::try_from(index - 1)
        .ok()
        .and_then(|index| form.cameras.get(index))
        .and_then(|camera| camera["id"].as_str())
        .and_then(|id| id.strip_prefix("entity:"))
        .and_then(|id| id.parse().ok())
}

fn selected_detail(form: &Form, index: i32) -> String {
    let Some(camera) = usize::try_from(index - 1)
        .ok()
        .and_then(|index| form.cameras.get(index))
    else {
        return "Choose an authored sky_camera. If the map has none, create one with Entity… and author its origin and scale. No camera is selected automatically.".into();
    };
    let mut detail = format!(
        "{} · {}\nOrigin: {}\nScale: {}",
        camera["id"].as_str().unwrap_or("unknown"),
        camera["targetname"]
            .as_str()
            .filter(|name| !name.is_empty())
            .unwrap_or("unnamed"),
        camera["origin"],
        camera["scale"],
    );
    if let Some(issue) = camera["issue"].as_str() {
        detail.push_str(&format!("\nUnavailable: {issue}"));
    }
    detail
}

fn present(window: &Sky3dWindow, form: &RefCell<Form>, report: Value) {
    let mut form = form.borrow_mut();
    form.cameras = report["cameras"].as_array().cloned().unwrap_or_default();
    if report["selected_camera"].is_object()
        && !form
            .cameras
            .iter()
            .any(|camera| camera["id"] == report["selected_camera"]["id"])
    {
        form.cameras.push(report["selected_camera"].clone());
    }
    window.set_camera_entity(report["camera_entity"].as_str().unwrap_or_default().into());
    let mut labels = vec!["Choose a sky_camera".into()];
    labels.extend(form.cameras.iter().map(|camera| {
        format!(
            "{} · {}{}",
            camera["id"].as_str().unwrap_or("unknown"),
            camera["targetname"]
                .as_str()
                .filter(|name| !name.is_empty())
                .unwrap_or("unnamed"),
            if camera["ready"] == true {
                ""
            } else {
                " · unavailable"
            },
        )
        .into()
    }));
    window.set_cameras(Rc::new(VecModel::from(labels)).into());
    let index = form
        .cameras
        .iter()
        .position(|camera| camera["id"] == report["camera_entity"])
        .map_or(0, |index| index as i32 + 1);
    window.set_camera_index(index);
    window.set_preview_enabled(report["enabled"].as_bool().unwrap_or(false));
    window.set_loading(report["loading"].as_bool().unwrap_or(false));
    window.set_cancellable(report["cancellable"].as_bool().unwrap_or(false));
    window.set_can_prepare(
        report["worker_active"] != true
            && usize::try_from(index - 1)
                .ok()
                .and_then(|index| form.cameras.get(index))
                .is_some_and(|camera| camera["ready"] == true),
    );
    window.set_summary(
        report["summary"]
            .as_str()
            .unwrap_or("3D sky has not been prepared")
            .into(),
    );
    let mut detail = selected_detail(&form, index);
    if let Some(geometry) = report["stats"]["geometry"].as_object() {
        detail.push_str(&format!(
            "\nPrepared: {} surfaces · {} vertices · {} materials",
            geometry["surfaces"], geometry["vertices"], geometry["materials"]
        ));
    }
    window.set_detail(detail.into());
    let mut diagnostics = Vec::new();
    if let Some(issue) = report["issue"].as_str() {
        diagnostics.push(issue.to_owned());
    }
    if let Some(issues) = report["diagnostics"].as_array() {
        for issue in issues {
            let issue = issue
                .get("message")
                .unwrap_or(issue)
                .as_str()
                .map_or_else(|| issue.to_string(), str::to_owned);
            if !diagnostics.contains(&issue) {
                diagnostics.push(issue);
            }
        }
    }
    if report["cameras_truncated"] == true {
        diagnostics.push(
            "The list shows the first 64 sky cameras plus the current choice. Enter an exact Camera entity ID and click Use camera to choose another."
                .into(),
        );
    }
    window.set_diagnostics(diagnostics.join("\n").into());
    let limits = &report["limits"];
    let render = &limits["render"];
    window.set_limits(format!(
        "Limits: {} camera choices · {} source brushes · {} source sides\nMiniature: {} vertices · {} indices · {} materials\nMemory: {} MiB preparation · {} MiB GPU assets · {} MiB target · {} pixels per target axis",
        limits["camera_rows"], limits["region"]["source_brushes"], limits["region"]["source_sides"],
        render["vertices"], render["indices"], render["materials"],
        limits["cpu_bytes"].as_u64().unwrap_or_default() / (1024 * 1024),
        render["gpu_asset_bytes"].as_u64().unwrap_or_default() / (1024 * 1024),
        render["target_bytes"].as_u64().unwrap_or_default() / (1024 * 1024), render["target_dimension"],
    ).into());
}

fn callback(
    window: &slint::Weak<Sky3dWindow>,
    target: &DocumentContinuation,
    body: impl FnOnce(&Sky3dWindow, &DocumentAccess) -> Result<(), String>,
) {
    let Some(window) = window.upgrade() else {
        return;
    };
    let result = target.upgrade().and_then(|access| body(&window, &access));
    if let Err(error) = result {
        window.set_message(error.into());
    }
}

impl Controller {
    pub fn new_with_access(access: &DocumentAccess) -> Result<Self, slint::PlatformError> {
        let window = Sky3dWindow::new()?;
        let target = access.downgrade();
        let form = Rc::new(RefCell::new(Form::default()));
        window.on_settings_changed({
            let (window, form, target) = (window.as_weak(), form.clone(), target.clone());
            move || {
                callback(&window, &target, |window, access| {
                    access.with_session(|session| {
                        let camera = camera_id(&form.borrow(), window.get_camera_index());
                        let result =
                            session.set_sky3d_preview(window.get_preview_enabled(), camera);
                        present(window, &form, session.sky3d_preview_state());
                        result?;
                        window.set_message("".into());
                        Ok(())
                    })
                })
            }
        });
        window.on_use_camera({
            let (window, form, target) = (window.as_weak(), form.clone(), target.clone());
            move || {
                callback(&window, &target, |window, access| {
                    access.with_session(|session| {
                        let camera = match crate::identity::parse_id(&window.get_camera_entity())? {
                            map_core::ObjectId::Entity(id) => id,
                            _ => return Err("3D sky camera must be an entity:<id>".into()),
                        };
                        session.set_sky3d_preview(window.get_preview_enabled(), Some(camera))?;
                        present(window, &form, session.sky3d_preview_state());
                        window.set_message("".into());
                        Ok(())
                    })
                })
            }
        });
        window.on_prepare({
            let (window, form, target) = (window.as_weak(), form.clone(), target.clone());
            move || {
                callback(&window, &target, |window, access| {
                    let result = access.start_sky3d_preview(Actor::User);
                    access.with_session(|session| {
                        present(window, &form, session.sky3d_preview_state());
                        Ok(())
                    })?;
                    result?;
                    window.set_message("".into());
                    Ok(())
                })
            }
        });
        window.on_cancel_preparation({
            let (window, form, target) = (window.as_weak(), form.clone(), target.clone());
            move || {
                callback(&window, &target, |window, access| {
                    access.with_session(|session| {
                        session.cancel_sky3d_preview()?;
                        present(window, &form, session.sky3d_preview_state());
                        window.set_message("".into());
                        Ok(())
                    })
                })
            }
        });
        window.on_refresh({
            let (window, form, target) = (window.as_weak(), form.clone(), target.clone());
            move || {
                callback(&window, &target, |window, access| {
                    access.with_session(|session| {
                        present(window, &form, session.sky3d_preview_state());
                        window.set_message("".into());
                        Ok(())
                    })
                })
            }
        });
        window.on_close_dialog({
            let window = window.as_weak();
            move || {
                if let Some(window) = window.upgrade() {
                    let _ = window.hide();
                }
            }
        });
        window
            .window()
            .on_close_requested(|| slint::CloseRequestResponse::HideWindow);
        Ok(Self { window, form })
    }

    pub fn show(&self, session: &mut Session) -> Result<(), slint::PlatformError> {
        self.refresh(session);
        self.window.set_message("".into());
        self.window.show()
    }

    pub fn refresh(&self, session: &mut Session) {
        present(&self.window, &self.form, session.sky3d_preview_state());
    }
}

#[cfg(test)]
mod tests;
