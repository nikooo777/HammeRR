use slint::ComponentHandle;

use crate::PointfileWindow;
use crate::runtime::{SharedSession, lock};
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

pub fn bind(window: &PointfileWindow, shared: &SharedSession) {
    window.on_load({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            let result = lock(&shared).load_pointfile(
                Actor::User,
                std::path::Path::new(window.get_path().as_str()),
            );
            match result {
                Ok(()) => {
                    window.set_message("".into());
                    refresh(&window, &lock(&shared));
                }
                Err(error) => window.set_message(error.into()),
            }
        }
    });
    window.on_navigate({
        let (window, shared) = (window.as_weak(), shared.clone());
        move |index| {
            let window = window.unwrap();
            let result = lock(&shared)
                .navigate_pointfile(index.max(0) as usize, Some(window.get_token().as_str()));
            match result {
                Ok(()) => {
                    window.set_message("".into());
                    refresh(&window, &lock(&shared));
                }
                Err(error) => window.set_message(error.into()),
            }
        }
    });
    window.on_frame({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            let result = lock(&shared)
                .frame_pointfile(window.get_aspect(), Some(window.get_token().as_str()));
            window.set_message(result.err().unwrap_or_default().into());
        }
    });
    window.on_clear({
        let (window, shared) = (window.as_weak(), shared.clone());
        move || {
            let window = window.unwrap();
            let result = lock(&shared).clear_pointfile(Some(window.get_token().as_str()));
            match result {
                Ok(()) => {
                    window.set_message("".into());
                    refresh(&window, &lock(&shared));
                }
                Err(error) => window.set_message(error.into()),
            }
        }
    });
    window.on_browse({
        let window = window.as_weak();
        move || {
            let window = window.clone();
            slint::spawn_local(async move {
                let file = rfd::AsyncFileDialog::new()
                    .set_title("Load pointfile")
                    .add_filter("Leak pointfile", &["lin", "pts"])
                    .pick_file()
                    .await;
                if let Some(file) = file
                    && let Some(window) = window.upgrade()
                {
                    window.set_path(file.path().display().to_string().into());
                }
            })
            .expect("event loop is running");
        }
    });
    window.on_cancel({
        let window = window.as_weak();
        move || {
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
}
