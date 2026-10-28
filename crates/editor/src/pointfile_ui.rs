use slint::ComponentHandle;

use crate::PointfileWindow;
use crate::runtime::{DocumentAccess, DocumentContinuation};

use crate::session::{Actor, Session};

pub fn refresh(window: &PointfileWindow, session: &Session) {
    let report = session.pointfile_report(0, 1).unwrap();
    window.set_stamp(session.pointfile_stamp().into());
    window.set_token(report.token.into());
    window.set_total(report.total as i32);
    window.set_point(report.index as i32 + 1);
    window.set_current(report.current);
    window.set_has_path(report.source.is_some());
    window.set_source(
        report
            .path
            .clone()
            .unwrap_or_else(|| {
                if report.source.is_some() {
                    "Leak from the current compile".into()
                } else {
                    "No pointfile loaded".into()
                }
            })
            .into(),
    );
    if window.get_path().is_empty()
        && let Some(path) = report.path
    {
        window.set_path(path.into());
    }
    window.set_summary(if report.total == 0 {
        "Load a .lin file or compile a leaking map.".into()
    } else if !report.current {
        "Map changed. Reload the pointfile or compile again.".into()
    } else {
        format!(
            "{} points · {:.1} units along the path",
            report.total, report.length
        )
        .into()
    });
    window.set_position(
        report
            .position
            .map_or_else(String::new, |point| {
                format!(
                    "Position: {:.1}, {:.1}, {:.1}",
                    point[0], point[1], point[2]
                )
            })
            .into(),
    );
}

pub fn refresh_changed(window: &PointfileWindow, session: &Session) {
    if window.get_stamp() != session.pointfile_stamp() {
        refresh(window, session);
    }
}

fn with_session(
    window: &slint::Weak<PointfileWindow>,
    target: &DocumentContinuation,
    body: impl FnOnce(&PointfileWindow, &mut Session),
) {
    let Some(window) = window.upgrade() else {
        return;
    };
    let result = target.upgrade().and_then(|access| {
        access.with_session(|session| {
            body(&window, session);
            Ok(())
        })
    });
    if let Err(error) = result {
        window.set_message(error.into());
    }
}

fn browse_dialog(
    window: &slint::Weak<PointfileWindow>,
    target: &DocumentContinuation,
) -> Option<rfd::AsyncFileDialog> {
    let window = window.upgrade()?;
    let result = target.upgrade().and_then(|access| {
        access.with_shared(|_| {
            Ok(rfd::AsyncFileDialog::new()
                .set_title("Load pointfile")
                .add_filter("Leak pointfile", &["lin", "pts"]))
        })
    });
    match result {
        Ok(dialog) => Some(dialog),
        Err(error) => {
            window.set_message(error.into());
            None
        }
    }
}

fn accept_browse(
    window: &slint::Weak<PointfileWindow>,
    target: &DocumentContinuation,
    path: &std::path::Path,
) {
    let Some(window) = window.upgrade() else {
        return;
    };
    let result = target.upgrade().and_then(|access| {
        access.with_shared(|_| {
            window.set_path(path.display().to_string().into());
            Ok(())
        })
    });
    if let Err(error) = result {
        window.set_message(error.into());
    }
}

pub fn bind_with_access(window: &PointfileWindow, access: &DocumentAccess) {
    window.on_load({
        let (window, target) = (window.as_weak(), access.downgrade());
        move || {
            with_session(&window, &target, |window, session| {
                let result = session.load_pointfile(
                    Actor::User,
                    std::path::Path::new(window.get_path().as_str()),
                );
                match result {
                    Ok(()) => {
                        window.set_message("".into());
                        refresh(window, session);
                    }
                    Err(error) => window.set_message(error.into()),
                }
            })
        }
    });
    window.on_navigate({
        let (window, target) = (window.as_weak(), access.downgrade());
        move |index| {
            with_session(&window, &target, |window, session| {
                let result = session
                    .navigate_pointfile(index.max(0) as usize, Some(window.get_token().as_str()));
                match result {
                    Ok(()) => {
                        window.set_message("".into());
                        refresh(window, session);
                    }
                    Err(error) => window.set_message(error.into()),
                }
            })
        }
    });
    window.on_frame({
        let (window, target) = (window.as_weak(), access.downgrade());
        move || {
            with_session(&window, &target, |window, session| {
                let result =
                    session.frame_pointfile(window.get_aspect(), Some(window.get_token().as_str()));
                window.set_message(result.err().unwrap_or_default().into());
            })
        }
    });
    window.on_clear({
        let (window, target) = (window.as_weak(), access.downgrade());
        move || {
            with_session(&window, &target, |window, session| {
                let result = session.clear_pointfile(Some(window.get_token().as_str()));
                match result {
                    Ok(()) => {
                        window.set_message("".into());
                        refresh(window, session);
                    }
                    Err(error) => window.set_message(error.into()),
                }
            })
        }
    });
    window.on_browse({
        let (window, target) = (window.as_weak(), access.downgrade());
        move || {
            let Some(dialog) = browse_dialog(&window, &target) else {
                return;
            };
            let (window, target) = (window.clone(), target.clone());
            slint::spawn_local(async move {
                if let Some(file) = dialog.pick_file().await {
                    accept_browse(&window, &target, file.path());
                }
            })
            .expect("event loop is running");
        }
    });
    window.on_cancel({
        let window = window.as_weak();
        move || {
            if let Some(window) = window.upgrade() {
                let _ = window.hide();
            }
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
}
