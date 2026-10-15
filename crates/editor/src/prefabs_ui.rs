use crate::{
    PrefabRow, PrefabsWindow, clipboard_ui,
    runtime::{SharedSession, lock},
    session::{Actor, Session},
};
use slint::{ComponentHandle, VecModel};
use std::{cell::RefCell, rc::Rc};
const PAGE: usize = 64;
#[derive(Default)]
struct Form {
    selected: Option<String>,
    offset: usize,
    stamp: String,
    action: Option<(bool, String)>,
    seen: Option<(u64, u64, u64)>,
}
pub struct Controller {
    pub window: PrefabsWindow,
    form: Rc<RefCell<Form>>,
    paste: Rc<clipboard_ui::Controller>,
}
fn bounds_label(bounds: &serde_json::Value) -> String {
    let low = (0..3)
        .map(|axis| bounds[0][axis].as_f64().unwrap_or_default())
        .collect::<Vec<_>>();
    let high = (0..3)
        .map(|axis| bounds[1][axis].as_f64().unwrap_or_default())
        .collect::<Vec<_>>();
    format!(
        "Size: {} × {} × {} units\nSource center: {}, {}, {}",
        high[0] - low[0],
        high[1] - low[1],
        high[2] - low[2],
        (high[0] + low[0]) * 0.5,
        (high[1] + low[1]) * 0.5,
        (high[2] + low[2]) * 0.5
    )
}
fn refresh(window: &PrefabsWindow, session: &Session, state: &RefCell<Form>, force: bool) {
    let mut form = state.borrow_mut();
    let seen = (
        session.document_generation(),
        session.document().revision(),
        session.view_serial(),
    );
    if !force && form.seen == Some(seen) {
        return;
    }
    form.seen = Some(seen);
    let report = session.prefab_report(
        &window.get_query(),
        form.offset,
        PAGE,
        form.selected.as_deref(),
    );
    window.set_can_save(!session.selected_ids().is_empty());
    window.set_can_preview(!report["selected"].is_null() && form.action.is_none());
    window.set_saving(form.action.as_ref().is_some_and(|(delete, _)| !*delete));
    window.set_deleting(form.action.as_ref().is_some_and(|(delete, _)| *delete));
    window.set_directory(
        report["directory"]
            .as_str()
            .unwrap_or("Storage unavailable")
            .into(),
    );
    form.stamp = report["snapshot"].as_str().unwrap_or_default().into();
    if let Some(error) = report["error"].as_str() {
        window.set_message(error.into());
        window.set_entries(Default::default());
        return;
    }
    let rows = report["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| PrefabRow {
            name: entry["name"].as_str().unwrap().into(),
            detail: format!(
                "{} KiB · VMF assembly",
                entry["bytes"].as_u64().unwrap().div_ceil(1024)
            )
            .into(),
        })
        .collect::<Vec<_>>();
    window.set_entries(Rc::new(VecModel::from(rows)).into());
    window.set_selected(form.selected.as_deref().unwrap_or_default().into());
    window.set_page_label(format!("Prefabs: {}", report["total"]).into());
    window.set_previous(form.offset > 0);
    window.set_next(!report["next_offset"].is_null());
    let selected = &report["selected"];
    window.set_detail(if selected.is_null() {
        report["selection_error"]
            .as_str()
            .unwrap_or(
                "Save an assembly from the current selection, or choose one from the library.",
            )
            .into()
    } else {
        format!(
            "Objects: {} · Groups: {} · Layers: {} · Faces: {}\nExternal links: {}\n{}",
            selected["objects"],
            selected["groups"],
            selected["layers"],
            selected["faces"],
            selected["external_links"].as_array().map_or(0, Vec::len),
            bounds_label(&selected["bounds"])
        )
        .into()
    });
}
impl Controller {
    pub fn new(shared: &SharedSession) -> Result<Self, slint::PlatformError> {
        let window = PrefabsWindow::new()?;
        let form = Rc::new(RefCell::new(Form::default()));
        let paste = Rc::new(clipboard_ui::Controller::new(shared)?);
        window.on_choose({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move |name| {
                form.borrow_mut().selected = Some(name.to_string());
                refresh(&window.unwrap(), &lock(&shared), &form, true);
            }
        });
        let refresh_callback = || {
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || refresh(&window.unwrap(), &lock(&shared), &form, true)
        };
        window.on_refresh(refresh_callback());
        window.on_search({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || {
                form.borrow_mut().offset = 0;
                refresh(&window.unwrap(), &lock(&shared), &form, true);
            }
        });
        window.on_page({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move |direction: i32| {
                let mut state = form.borrow_mut();
                state.offset = if direction < 0 {
                    state.offset.saturating_sub(PAGE)
                } else {
                    state.offset.saturating_add(PAGE)
                };
                drop(state);
                refresh(&window.unwrap(), &lock(&shared), &form, true);
            }
        });
        let begin_callback = |delete| {
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || {
                let mut state = form.borrow_mut();
                let stamp = state.stamp.clone();
                state.action = Some((delete, stamp));
                drop(state);
                window.unwrap().set_prefab_name("".into());
                refresh(&window.unwrap(), &lock(&shared), &form, true);
            }
        };
        window.on_begin_save(begin_callback(false));
        window.on_begin_delete(begin_callback(true));
        let cancel_callback = |close| {
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || {
                form.borrow_mut().action = None;
                let window = window.unwrap();
                refresh(&window, &lock(&shared), &form, true);
                if close {
                    let _ = window.hide();
                }
            }
        };
        window.on_cancel(cancel_callback(false));
        window.on_close_dialog(cancel_callback(true));
        window.window().on_close_requested({
            let window = window.as_weak();
            move || {
                window.unwrap().invoke_close_dialog();
                slint::CloseRequestResponse::HideWindow
            }
        });
        let write_callback = |delete| {
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || {
                let window = window.unwrap();
                let mut session = lock(&shared);
                let result = (|| {
                    let state = form.borrow();
                    let (deleting, stamp) = state
                        .action
                        .as_ref()
                        .ok_or("Start a save or deletion first")?;
                    if *deleting != delete {
                        return Err("Library action changed".into());
                    }
                    if delete {
                        session.delete_prefab(
                            Actor::User,
                            state.selected.as_deref().ok_or("Choose a prefab first")?,
                            stamp,
                        )
                    } else {
                        {
                            let ids = session.selected_ids();
                            session.save_prefab(Actor::User, &window.get_prefab_name(), &ids, stamp)
                        }
                    }
                })();
                match result {
                    Ok(_) => {
                        let mut state = form.borrow_mut();
                        if delete {
                            state.selected = None;
                        } else {
                            state.selected = Some(window.get_prefab_name().to_string());
                        }
                        state.action = None;
                        drop(state);
                        window.set_message(if delete {
                            "Prefab deleted; placed map copies remain.".into()
                        } else {
                            "Prefab saved without changing the map.".into()
                        });
                        refresh(&window, &session, &form, true);
                    }
                    Err(error) => window.set_message(error.into()),
                }
            }
        };
        window.on_save(write_callback(false));
        window.on_delete(write_callback(true));
        window.on_preview({
            let (window, shared, form, paste) = (
                window.as_weak(),
                shared.clone(),
                form.clone(),
                paste.clone(),
            );
            move || {
                let window = window.unwrap();
                let mut session = lock(&shared);
                let result = (|| {
                    let state = form.borrow();
                    let position = [
                        window.get_center_x(),
                        window.get_center_y(),
                        window.get_center_z(),
                    ]
                    .map(|value| {
                        value
                            .parse::<f64>()
                            .map_err(|_| "Placement coordinates must be finite numbers".to_string())
                    })
                    .into_iter()
                    .collect::<Result<Vec<_>, _>>()?;
                    session.preview_prefab(
                        Actor::User,
                        state.selected.as_deref().ok_or("Choose a prefab first")?,
                        glam::DVec3::new(position[0], position[1], position[2]),
                        window.get_keep_external(),
                        &state.stamp,
                    )
                })();
                match result {
                    Ok(_) => {
                        paste.initialize(&mut session, false);
                        if let Err(error) = paste.window.show() {
                            window.set_message(error.to_string().into());
                        }
                    }
                    Err(error) => window.set_message(error.into()),
                }
                refresh(&window, &session, &form, true);
            }
        });
        Ok(Self {
            window,
            form,
            paste,
        })
    }
    pub fn show(&self, session: &Session) -> Result<(), slint::PlatformError> {
        refresh(&self.window, session, &self.form, true);
        self.window.show()
    }
    pub fn refresh(&self, session: &Session) {
        if self.window.window().is_visible() {
            refresh(&self.window, session, &self.form, false);
        }
        if self.paste.window.window().is_visible() {
            self.paste.refresh(session);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    #[test]
    fn prefab_controls_share_capture_review_apply_and_cancel_delete() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let (mut session, directory, brush) = crate::session::prefabs::tests::fixture();
        let mut ids = vec![brush];
        for x in [0.0, 32.0] {
            ids.push(
                session
                    .create_entity(
                        Actor::User,
                        "light",
                        Some(glam::DVec3::new(x, 0.0, 48.0)),
                        std::collections::BTreeMap::new(),
                    )
                    .unwrap(),
            );
        }
        session
            .select(&ids, crate::session::SelectMode::Replace)
            .unwrap();
        let original = session.document().vmf().clone();
        let shared = Arc::new(Mutex::new(session));
        let controller = Controller::new(&shared).unwrap();
        controller.show(&lock(&shared)).unwrap();
        controller.window.invoke_begin_save();
        controller
            .window
            .set_prefab_name("Corridor light cluster".into());
        controller.window.invoke_cancel();
        assert!(!directory.path().join("Corridor light cluster.vmf").exists());
        controller.window.invoke_begin_save();
        controller
            .window
            .set_prefab_name("Corridor light cluster".into());
        controller.window.invoke_save();
        assert!(directory.path().join("Corridor light cluster.vmf").exists());
        assert_eq!(lock(&shared).document().vmf(), &original);
        controller
            .window
            .invoke_choose("Corridor light cluster".into());
        controller.window.set_center_x("80".into());
        controller.window.set_center_z("48".into());
        controller.window.invoke_preview();
        assert!(lock(&shared).clipboard_report()["preview"]["valid"] == true);
        controller.paste.window.invoke_cancel();
        assert_eq!(lock(&shared).document().vmf(), &original);
        controller.refresh(&lock(&shared));
        controller.window.invoke_preview();
        controller.paste.window.invoke_apply();
        assert_eq!(lock(&shared).document().vmf().world.solids.len(), 2);
        assert_eq!(lock(&shared).document().vmf().entities.len(), 4);
        controller.refresh(&lock(&shared));
        crate::ui::tests::save_ui_snapshot(controller.window.window(), "prefab-library");
        controller.window.invoke_begin_delete();
        controller.window.invoke_cancel();
        assert!(directory.path().join("Corridor light cluster.vmf").exists());
        controller.window.invoke_begin_delete();
        controller.window.invoke_delete();
        assert!(!directory.path().join("Corridor light cluster.vmf").exists());
        assert_eq!(lock(&shared).document().vmf().world.solids.len(), 2);
    }
}
