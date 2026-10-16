use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use glam::DVec3;
use map_core::transfer::Options;
use serde_json::Value;
use slint::ComponentHandle;

use crate::{
    PasteWindow,
    runtime::{SharedSession, lock},
    session::{Actor, Session},
};

pub struct Controller {
    pub window: PasteWindow,
    stamp: Rc<RefCell<String>>,
    token: Rc<Cell<Option<u64>>>,
    prepared: Rc<Cell<Option<Options>>>,
}

fn options(window: &PasteWindow) -> Result<Options, String> {
    let offset = if window.get_placement() == 0 {
        DVec3::ZERO
    } else {
        let values = [
            window.get_offset_x(),
            window.get_offset_y(),
            window.get_offset_z(),
        ];
        let mut result = DVec3::ZERO;
        for (axis, value) in values.iter().enumerate() {
            result[axis] = value.parse().map_err(|_| "Enter finite XYZ offsets")?;
        }
        result
    };
    Ok(Options {
        offset,
        texture_lock: window.get_texture_lock(),
        keep_external: window.get_keep_external(),
        restore_names: window.get_restore_names(),
    })
}

fn present(window: &PasteWindow, session: &Session, report: &Value) {
    window.set_destination(format!("Destination: {}", session.title()).into());
    let payload = &report["payload"];
    window.set_has_payload(!payload.is_null());
    window.set_can_restore(report["can_restore_cut_names"] == true);
    if payload.is_null() {
        window.set_payload(
            "Clipboard is empty. Copy objects, or read VMF text from the system clipboard.".into(),
        );
        window.set_details("Only whole brushes and entities transfer. Map settings and cameras stay with their maps.".into());
        return;
    }
    window.set_payload(
        format!(
            "Objects: {} · Groups: {} · Layers: {} · Faces: {} · {} KiB",
            payload["objects"].as_array().map_or(0, Vec::len),
            payload["groups"],
            payload["layers"],
            payload["sides"],
            payload["bytes"].as_u64().unwrap_or(0).div_ceil(1024)
        )
        .into(),
    );
    let mut lines = Vec::new();
    if let Some(source) = report["preview"]["prefab_source"].as_str() {
        lines.push(format!("Prefab source: {source}"));
    }
    if let Some(error) = report["preview"]["stale_reason"].as_str() {
        lines.push(error.into());
    }
    if let Some(error) = payload["reference_error"].as_str() {
        lines.push(error.into());
    }
    if let Some(links) = payload["external_links"].as_array() {
        if !links.is_empty() {
            lines.push(format!(
                "External links: {}. Keeping these links may connect to objects in this map.",
                links.len()
            ));
        }
        for link in links {
            lines.push(format!(
                "{} → {} destination matches",
                link["target"].as_str().unwrap_or_default(),
                link["destination_matches"].as_array().map_or(0, Vec::len)
            ));
        }
    }
    if let Some(result) = report["preview"]["result"].as_object() {
        lines.push(format!(
            "Preview objects: {} · Internal reference updates: {}",
            result["objects"].as_array().map_or(0, Vec::len),
            result["remapped_references"]
        ));
        for name in result["names"].as_array().into_iter().flatten() {
            lines.push(format!(
                "{} → {}",
                name["before"].as_str().unwrap_or_default(),
                name["after"].as_str().unwrap_or_default()
            ));
        }
    } else {
        lines.push("Preview allocates fresh object, face, group and layer IDs. Object notes and native group/layer ancestry transfer with the objects.".into());
    }
    window.set_details(lines.join("\n").into());
}

fn checked(session: &Session, stamp: &RefCell<String>, token: Option<u64>) -> Result<(), String> {
    let report = session.clipboard_report();
    if session.clipboard_stamp() != *stamp.borrow() || report["preview"]["token"].as_u64() != token
    {
        return Err("Map, selection or clipboard changed; reopen Paste to review it".into());
    }
    if let Some(reason) = report["preview"]["stale_reason"].as_str() {
        return Err(reason.into());
    }
    Ok(())
}

impl Controller {
    pub fn new(shared: &SharedSession) -> Result<Self, slint::PlatformError> {
        let window = PasteWindow::new()?;
        window
            .window()
            .set_size(slint::LogicalSize::new(760.0, 690.0));
        let stamp = Rc::new(RefCell::new(String::new()));
        let token = Rc::new(Cell::new(None));
        let prepared = Rc::new(Cell::new(None));
        window.on_invalidate({
            let (window, prepared) = (window.as_weak(), prepared.clone());
            move || {
                prepared.set(None);
                let window = window.unwrap();
                window.set_ready(false);
                window.set_message("Preview changed settings before applying them.".into());
            }
        });
        window.on_preview({
            let (window, shared, stamp, token, prepared) = (
                window.as_weak(),
                shared.clone(),
                stamp.clone(),
                token.clone(),
                prepared.clone(),
            );
            move || {
                let window = window.unwrap();
                let result = (|| {
                    let mut session = lock(&shared);
                    checked(&session, &stamp, token.get())?;
                    let requested = options(&window)?;
                    let snapshot = session.clipboard_stamp();
                    let report = session.preview_paste(requested, &snapshot)?;
                    *stamp.borrow_mut() = session.clipboard_stamp();
                    token.set(report["preview"]["token"].as_u64());
                    prepared.set(Some(requested));
                    present(&window, &session, &report);
                    Ok::<_, String>(())
                })();
                match result {
                    Ok(()) => {
                        window.set_ready(true);
                        window.set_message(
                            "Review the map preview, names and links. Apply creates one Undo step."
                                .into(),
                        );
                    }
                    Err(error) => {
                        window.set_ready(false);
                        window.set_message(error.into());
                    }
                }
            }
        });
        window.on_apply({
            let (window, shared, stamp, token, prepared) = (
                window.as_weak(),
                shared.clone(),
                stamp.clone(),
                token.clone(),
                prepared.clone(),
            );
            move || {
                let window = window.unwrap();
                let result = (|| {
                    if prepared.get() != Some(options(&window)?) {
                        return Err("Preview changed settings before applying them".into());
                    }
                    let mut session = lock(&shared);
                    checked(&session, &stamp, token.get())?;
                    session.apply_paste(Actor::User, token.get().ok_or("Preview the paste first")?)
                })();
                match result {
                    Ok(_) => {
                        token.set(None);
                        prepared.set(None);
                        window.set_ready(false);
                        let _ = window.hide();
                    }
                    Err(error) => window.set_message(error.into()),
                }
            }
        });
        window.on_cancel({
            let (window, shared, token, prepared) = (
                window.as_weak(),
                shared.clone(),
                token.clone(),
                prepared.clone(),
            );
            move || {
                if let Some(token) = token.take() {
                    let _ = lock(&shared).cancel_paste(token);
                }
                prepared.set(None);
                let _ = window.unwrap().hide();
            }
        });
        window.on_read_system({
            let (window, shared, stamp, token, prepared) = (
                window.as_weak(),
                shared.clone(),
                stamp.clone(),
                token.clone(),
                prepared.clone(),
            );
            move || {
                let window = window.unwrap();
                let mut session = lock(&shared);
                let snapshot = session.clipboard_stamp();
                match session.import_clipboard(Actor::User, None, &snapshot) {
                    Ok(report) => {
                        *stamp.borrow_mut() = session.clipboard_stamp();
                        token.set(report["preview"]["token"].as_u64());
                        prepared.set(None);
                        window.set_ready(false);
                        window.set_restore_names(session.can_restore_cut_names());
                        present(&window, &session, &report);
                        window.set_message(
                            "System clipboard imported. Preview before applying.".into(),
                        );
                    }
                    Err(error) => window.set_message(error.into()),
                }
            }
        });
        window.on_publish_system({
            let (window, shared, stamp, token) = (
                window.as_weak(),
                shared.clone(),
                stamp.clone(),
                token.clone(),
            );
            move || {
                let result = (|| {
                    let mut session = lock(&shared);
                    checked(&session, &stamp, token.get())?;
                    let content = session.clipboard_report()["payload"]["content_revision"]
                        .as_u64()
                        .ok_or("Clipboard is empty")?;
                    session.export_clipboard(content, true)
                })();
                window.unwrap().set_message(match result {
                    Ok(_) => "VMF text published to system clipboard.".into(),
                    Err(error) => error.into(),
                });
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
            stamp,
            token,
            prepared,
        })
    }

    pub fn initialize(&self, session: &mut Session, read_system: bool) {
        let active = session.clipboard_report()["preview"]["valid"] == true;
        let error = if read_system && !active {
            let snapshot = session.clipboard_stamp();
            session.import_clipboard(Actor::User, None, &snapshot).err()
        } else {
            None
        };
        let report = session.clipboard_report();
        let preview = &report["preview"];
        let error = error.or_else(|| preview["stale_reason"].as_str().map(str::to_owned));
        *self.stamp.borrow_mut() = session.clipboard_stamp();
        self.token.set(preview["token"].as_u64());
        self.prepared.set(None);
        let offset = preview["offset"].as_array();
        self.window.set_placement(i32::from(
            offset.is_some_and(|values| values.iter().any(|value| value.as_f64() != Some(0.0))),
        ));
        for (axis, set) in [
            PasteWindow::set_offset_x,
            PasteWindow::set_offset_y,
            PasteWindow::set_offset_z,
        ]
        .into_iter()
        .enumerate()
        {
            set(
                &self.window,
                offset
                    .and_then(|values| values[axis].as_f64())
                    .unwrap_or(0.0)
                    .to_string()
                    .into(),
            );
        }
        self.window
            .set_texture_lock(preview["texture_lock"].as_bool().unwrap_or(true));
        self.window
            .set_keep_external(preview["keep_external_links"] == true);
        self.window.set_restore_names(
            preview["restore_cut_names"]
                .as_bool()
                .unwrap_or_else(|| session.can_restore_cut_names()),
        );
        self.window.set_ready(false);
        present(&self.window, session, &report);
        self.window.set_message(error.map_or_else(
            || "Choose placement and link handling, then Preview paste.".into(),
            |error| format!("{error}. The retained session payload is shown.").into(),
        ));
        if active {
            self.prepared.set(options(&self.window).ok());
            self.window.set_ready(true);
        }
    }

    pub fn refresh(&self, session: &Session) {
        if let Err(error) = checked(session, &self.stamp, self.token.get()) {
            self.window.set_ready(false);
            self.window.set_message(error.into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn paste_controls_share_native_import_owned_preview_review_apply_and_cancel() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let (mut session, id, _) = crate::test_assets::clipboard();
        session
            .capture_clipboard(Actor::User, &[id], &session.clipboard_stamp(), false, true)
            .unwrap();
        let before = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let shared = Arc::new(Mutex::new(session));
        let controller = Controller::new(&shared).unwrap();
        controller.initialize(&mut lock(&shared), true);
        controller.window.set_placement(1);
        controller.window.set_offset_x("80".into());
        controller.window.invoke_preview();
        assert!(controller.window.get_ready());
        assert_eq!(lock(&shared).document().vmf(), &before);
        assert_eq!(lock(&shared).document().undo_labels().count(), history);
        controller.window.show().unwrap();
        crate::ui::tests::save_ui_snapshot(controller.window.window(), "clipboard-paste");
        controller.window.set_offset_x("96".into());
        controller.window.invoke_invalidate();
        controller.window.invoke_apply();
        assert_eq!(lock(&shared).document().vmf(), &before);
        controller.window.invoke_preview();
        controller.window.invoke_apply();
        assert!(!controller.window.window().is_visible());
        assert_eq!(lock(&shared).document().undo_labels().count(), history + 1);
        lock(&shared).undo(Actor::User).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &before);
        controller.initialize(&mut lock(&shared), false);
        controller.window.invoke_preview();
        lock(&shared)
            .preview_transform(
                Actor::Agent,
                &[id],
                map_core::edit::Transform {
                    translation: DVec3::Z * 20.0,
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        let foreign = lock(&shared).render_document().vmf().clone();
        controller.refresh(&lock(&shared));
        assert!(!controller.window.get_ready());
        controller.window.invoke_cancel();
        assert_eq!(lock(&shared).render_document().vmf(), &foreign);
        lock(&shared).cancel_transform_preview();
        controller.initialize(&mut lock(&shared), false);
        controller.window.invoke_preview();
        controller
            .window
            .window()
            .dispatch_event(slint::platform::WindowEvent::KeyPressed {
                text: slint::platform::Key::Escape.into(),
            });
        assert!(!controller.window.window().is_visible());
        assert_eq!(lock(&shared).document().vmf(), &before);
    }
}
