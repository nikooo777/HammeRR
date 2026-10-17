use std::{cell::RefCell, rc::Rc};

use slint::ComponentHandle;

use crate::{
    AppWindow, NavigationDefaultsWindow,
    runtime::{SharedSession, lock},
    session::{Actor, Session},
};

pub struct Controller {
    pub window: NavigationDefaultsWindow,
    snapshot: Rc<RefCell<String>>,
}

pub fn refresh_speed(app: &AppWindow, session: &Session) {
    app.set_fly_speed(session.fly_speed().to_string().into());
    app.set_navigation_defaults_error(session.navigation_preferences_error().is_some());
}

pub fn bind_speed(app: &AppWindow, shared: &SharedSession) {
    refresh_speed(app, &lock(shared));
    app.on_fly_speed_changed({
        let (app, shared) = (app.as_weak(), shared.clone());
        move || {
            let app = app.unwrap();
            let mut session = lock(&shared);
            let result = app
                .get_fly_speed()
                .parse::<f32>()
                .map_err(|_| "Enter a valid fly speed".to_string())
                .and_then(|speed| session.set_fly_speed(speed));
            if let Err(error) = result {
                app.set_message(error.into());
            }
            refresh_speed(&app, &session);
        }
    });
}

pub fn wheel_speed(
    session: &mut Session,
    projection: render::Projection,
    looking: bool,
    delta: f32,
) -> Result<bool, String> {
    if projection != render::Projection::Perspective || !looking {
        return Ok(false);
    }
    session.adjust_fly_speed(delta)?;
    Ok(true)
}

fn refresh(window: &NavigationDefaultsWindow, session: &Session, snapshot: &RefCell<String>) {
    let state = session.navigation_preferences_state();
    *snapshot.borrow_mut() = state["snapshot"].as_str().unwrap().into();
    window.set_current(format!("{} units/s", session.fly_speed()).into());
    window.set_saved(
        state["defaults"]["fly_speed"]
            .as_f64()
            .map(|speed| format!("{speed} units/s"))
            .unwrap_or_else(|| "Unavailable".into())
            .into(),
    );
    window.set_defaults(
        if state["ready"] != true {
            "Saved defaults could not load."
        } else if state["stored"] == true {
            "Saved startup defaults"
        } else {
            "Factory defaults · no saved file"
        }
        .into(),
    );
    window.set_storage(
        format!(
            "Storage: {}",
            state["path"].as_str().unwrap_or("unavailable")
        )
        .into(),
    );
    window.set_status(state["error"].as_str().or(state["storage_error"].as_str()).unwrap_or("Startup uses these defaults. Apply saved defaults changes only fly speed; active map previews and picks remain available.").into());
    window.set_ready(state["ready"] == true);
}

fn act(
    window: &NavigationDefaultsWindow,
    session: &mut Session,
    snapshot: &RefCell<String>,
    action: &str,
) {
    let result = session.change_navigation_preferences(Actor::User, action, &snapshot.borrow());
    match result {
        Ok(state) if state["write_committed"] == true && state["ready"] != true => window.set_message("Navigation defaults were written but could not reload. Review the storage error before continuing.".into()),
        Ok(_) => window.set_message(
            match action {
                "save" => "Current navigation settings saved for future startups.",
                "apply" => "Saved navigation settings applied to this session.",
                "reset" => "Factory defaults saved. Apply saved defaults to use them now.",
                _ => "Navigation defaults reloaded. Current navigation settings are unchanged.",
            }
            .into(),
        ),
        Err(error) => window.set_message(error.into()),
    }
    refresh(window, session, snapshot);
}

impl Controller {
    pub fn new(shared: &SharedSession) -> Result<Self, slint::PlatformError> {
        let window = NavigationDefaultsWindow::new()?;
        let snapshot = Rc::new(RefCell::new(String::new()));
        let callback = |action: &'static str| {
            let (window, shared, snapshot) = (window.as_weak(), shared.clone(), snapshot.clone());
            move || act(&window.unwrap(), &mut lock(&shared), &snapshot, action)
        };
        window.on_save(callback("save"));
        window.on_apply(callback("apply"));
        window.on_reload(callback("reload"));
        window.on_reset(callback("reset"));
        window.on_close_dialog({
            let window = window.as_weak();
            move || {
                let _ = window.unwrap().hide();
            }
        });
        Ok(Self { window, snapshot })
    }

    pub fn refresh(&self, session: &Session) {
        refresh(&self.window, session, &self.snapshot);
    }

    pub fn show(&self, session: &Session) -> Result<(), slint::PlatformError> {
        self.window.set_message("".into());
        self.refresh(session);
        self.window.show()
    }
}

#[cfg(test)]
mod tests;
