use std::cell::Cell;
use std::path::Path;
use std::rc::Rc;

use slint::ComponentHandle;

use crate::ColorCorrectionWindow;
use crate::runtime::{DocumentAccess, DocumentContinuation};
use crate::session::Session;

pub struct Controller {
    pub window: ColorCorrectionWindow,
    identity: Rc<Cell<Option<u64>>>,
}

fn refresh(window: &ColorCorrectionWindow, identity: &Cell<Option<u64>>, session: &mut Session) {
    let report = session.color_correction_state();
    identity.set(report["identity"].as_u64());
    window.set_source(report["source"].as_str().unwrap_or_default().into());
    window.set_preview_enabled(report["enabled"] == true);
    window.set_weight(report["weight"].to_string().into());
    window.set_status(report["summary"].as_str().unwrap_or_default().into());
}

fn callback(
    window: &slint::Weak<ColorCorrectionWindow>,
    target: &DocumentContinuation,
    identity: &Cell<Option<u64>>,
    body: impl FnOnce(&ColorCorrectionWindow, &mut Session) -> Result<(), String>,
) {
    let Some(window) = window.upgrade() else {
        return;
    };
    let result = target.upgrade().and_then(|access| {
        access.with_session(|session| {
            let result = body(&window, session);
            refresh(&window, identity, session);
            result
        })
    });
    window.set_message(result.err().unwrap_or_default().into());
}

impl Controller {
    pub fn new_with_access(access: &DocumentAccess) -> Result<Self, slint::PlatformError> {
        let target = access.downgrade();
        let window = ColorCorrectionWindow::new()?;
        let identity = Rc::new(Cell::new(None));
        window.on_load({
            let (window, target, identity) = (window.as_weak(), target.clone(), identity.clone());
            move || {
                callback(&window, &target, &identity, |window, session| {
                    session.load_color_correction(Path::new(window.get_source().as_str()))?;
                    Ok(())
                })
            }
        });
        window.on_apply({
            let (window, target, identity) = (window.as_weak(), target.clone(), identity.clone());
            move || {
                callback(&window, &target, &identity, |window, session| {
                    if window.get_preview_enabled() && identity.get().is_none() {
                        return Err("Load or review a LUT before enabling color correction".into());
                    }
                    let weight = window
                        .get_weight()
                        .parse::<f32>()
                        .map_err(|_| "Weight must be a number from 0 to 1")?;
                    session.set_color_correction(
                        window.get_preview_enabled(),
                        weight,
                        identity.get(),
                    )?;
                    Ok(())
                })
            }
        });
        window.on_clear({
            let (window, target, identity) = (window.as_weak(), target.clone(), identity.clone());
            move || {
                callback(&window, &target, &identity, |_, session| {
                    session.clear_color_correction();
                    Ok(())
                })
            }
        });
        window.on_dismiss({
            let window = window.as_weak();
            move || {
                if let Some(window) = window.upgrade() {
                    let _ = window.hide();
                }
            }
        });
        Ok(Self { window, identity })
    }

    pub fn show(&self, session: &mut Session) -> Result<(), slint::PlatformError> {
        refresh(&self.window, &self.identity, session);
        self.window.set_message("".into());
        self.window.show()
    }
}

#[cfg(test)]
mod tests;
