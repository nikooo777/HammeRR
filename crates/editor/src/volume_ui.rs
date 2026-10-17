use std::cell::Cell;
use std::rc::Rc;

use glam::DVec3;
use map_core::region::Match;
use slint::ComponentHandle;
use source_formats::vmf::format_number;

use crate::VolumeWindow;
#[cfg(test)]
use crate::runtime::SharedSession;
use crate::runtime::{DocumentAccess, DocumentContinuation};
use crate::session::volume::Spec;
use crate::session::{Actor, SelectMode, Session};

pub struct Controller {
    pub window: VolumeWindow,
    token: Rc<Cell<Option<u64>>>,
}

fn cancel(session: &mut Session, token: &Cell<Option<u64>>) {
    if let Some(token) = token.take() {
        let _ = session.cancel_volume_selection(token);
    }
}

fn load_bounds(window: &VolumeWindow, session: &Session) {
    let [mins, maxs] = session.volume_selection_bounds();
    window.set_min_x(format_number(mins.x).into());
    window.set_min_y(format_number(mins.y).into());
    window.set_min_z(format_number(mins.z).into());
    window.set_max_x(format_number(maxs.x).into());
    window.set_max_y(format_number(maxs.y).into());
    window.set_max_z(format_number(maxs.z).into());
}

fn spec(window: &VolumeWindow) -> Result<Spec, String> {
    let number = |value: slint::SharedString| {
        value
            .trim()
            .parse()
            .map_err(|_| "Enter numeric box corners".to_string())
    };
    Ok(Spec {
        mins: DVec3::new(
            number(window.get_min_x())?,
            number(window.get_min_y())?,
            number(window.get_min_z())?,
        ),
        maxs: DVec3::new(
            number(window.get_max_x())?,
            number(window.get_max_y())?,
            number(window.get_max_z())?,
        ),
        faces: window.get_target() == 1,
        rule: if window.get_rule() == 1 {
            Match::Contained
        } else {
            Match::Intersecting
        },
        mode: match window.get_mode() {
            1 => SelectMode::Add,
            2 => SelectMode::Remove,
            3 => SelectMode::Toggle,
            _ => SelectMode::Replace,
        },
    })
}

fn refresh(window: &VolumeWindow, session: &Session, token: Option<u64>) {
    window.set_bounds_label(
        if session.selection_bounds().is_some() {
            "Use selection bounds"
        } else {
            "Use map bounds"
        }
        .into(),
    );
    let preview = session
        .volume_preview()
        .filter(|preview| Some(preview.token) == token);
    window.set_ready(preview.is_some());
    if let Some(preview) = preview {
        let count = if preview.spec.faces {
            preview.matches.faces.len()
        } else {
            preview.matches.objects.len()
        };
        window.set_summary(
            format!(
                "{} matching {}{}",
                count,
                match (preview.spec.faces, count) {
                    (true, 1) => "face",
                    (true, _) => "faces",
                    (false, 1) => "object",
                    (false, _) => "objects",
                },
                if count == 0 && preview.spec.mode == SelectMode::Replace {
                    " · applying clears the selection"
                } else {
                    ""
                }
            )
            .into(),
        );
    } else {
        window.set_summary("Preview the region to see what matches.".into());
        if token.is_some() {
            window
                .set_message("Preview was closed or replaced. Preview these corners again.".into());
        }
    }
}

fn callback(
    window: &slint::Weak<VolumeWindow>,
    document: &DocumentContinuation,
    body: impl FnOnce(VolumeWindow, &mut Session),
) {
    let Some(window) = window.upgrade() else {
        return;
    };
    let result = document.upgrade().and_then(|access| {
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
    #[cfg(test)]
    pub fn new(
        shared: &SharedSession,
        aspect: impl Fn() -> f32 + 'static,
    ) -> Result<Self, slint::PlatformError> {
        Self::new_with_access(&DocumentAccess::unmanaged(shared.clone()), move || {
            Some(aspect())
        })
    }

    pub fn new_with_access(
        access: &DocumentAccess,
        aspect: impl Fn() -> Option<f32> + 'static,
    ) -> Result<Self, slint::PlatformError> {
        let window = VolumeWindow::new()?;
        let token = Rc::new(Cell::new(None));
        window.on_edited({
            let (window, document, token) = (window.as_weak(), access.downgrade(), token.clone());
            move || {
                callback(&window, &document, |window, session| {
                    cancel(session, &token);
                    window.set_message(
                        "Region changed. Preview the new matches before applying.".into(),
                    );
                    refresh(&window, session, token.get());
                });
            }
        });
        window.on_use_bounds({
            let (window, document, token) = (window.as_weak(), access.downgrade(), token.clone());
            move || {
                callback(&window, &document, |window, session| {
                    cancel(session, &token);
                    load_bounds(&window, session);
                    window.set_message("Preview the updated bounds.".into());
                    refresh(&window, session, token.get());
                });
            }
        });
        window.on_preview_edit({
            let (window, document, token) = (window.as_weak(), access.downgrade(), token.clone());
            move || {
                callback(&window, &document, |window, session| {
                    match spec(&window).and_then(|spec| session.preview_volume_selection(spec)) {
                        Ok(value) => {
                            token.set(Some(value));
                            window.set_message(
                                "Review the highlights, then select the matches.".into(),
                            );
                        }
                        Err(error) => window.set_message(error.into()),
                    }
                    refresh(&window, session, token.get());
                });
            }
        });
        window.on_frame({
            let (window, document, token) = (window.as_weak(), access.downgrade(), token.clone());
            move || {
                callback(&window, &document, |window, session| {
                    if let Some(token) = token.get()
                        && let Some(aspect) = aspect()
                        && let Err(error) = session.frame_volume_selection(token, aspect)
                    {
                        window.set_message(error.into());
                    }
                });
            }
        });
        window.on_apply({
            let (window, document, token) = (window.as_weak(), access.downgrade(), token.clone());
            move || {
                callback(&window, &document, |window, session| {
                    let result = token
                        .get()
                        .ok_or_else(|| "Preview the region first".to_string())
                        .and_then(|token| session.commit_volume_selection(Actor::User, token));
                    match result {
                        Ok(_) => {
                            token.set(None);
                            let _ = window.hide();
                        }
                        Err(error) => window.set_message(error.into()),
                    }
                    refresh(&window, session, token.get());
                });
            }
        });
        window.on_cancel({
            let (window, document, token) = (window.as_weak(), access.downgrade(), token.clone());
            move || {
                callback(&window, &document, |_window, session| {
                    cancel(session, &token);
                });
                if let Some(window) = window.upgrade() {
                    let _ = window.hide();
                }
            }
        });
        window.window().on_close_requested({
            let (window, document, token) = (window.as_weak(), access.downgrade(), token.clone());
            move || {
                callback(&window, &document, |_window, session| {
                    cancel(session, &token);
                });
                slint::CloseRequestResponse::HideWindow
            }
        });
        Ok(Self { window, token })
    }

    pub fn initialize(&self, session: &mut Session) {
        cancel(session, &self.token);
        load_bounds(&self.window, session);
        self.window.set_target(i32::from(session.face_mode()));
        self.window.set_rule(0);
        self.window.set_mode(0);
        self.window
            .set_message("Preview highlights matches without changing your selection.".into());
        self.refresh(session);
    }

    pub fn refresh(&self, session: &Session) {
        refresh(&self.window, session, self.token.get());
    }
}
