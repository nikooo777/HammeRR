use std::cell::RefCell;
use std::rc::Rc;

use serde_json::Value;
use slint::{ComponentHandle, VecModel};

use crate::FogWindow;
use crate::runtime::{DocumentAccess, DocumentContinuation};
use crate::session::Session;

#[derive(Default)]
struct Form {
    snapshot: String,
    choices: Vec<Value>,
}

pub struct Controller {
    pub window: FogWindow,
    form: Rc<RefCell<Form>>,
}

fn selected(form: &Form, index: i32) -> Option<&Value> {
    usize::try_from(index - 1)
        .ok()
        .and_then(|index| form.choices.get(index))
}

fn update_choice(window: &FogWindow, form: &Form) {
    let choice = selected(form, window.get_controller_index());
    window.set_can_apply(
        !window.get_preview_enabled() || choice.is_some_and(|choice| choice["ready"] == true),
    );
    let detail = match choice {
        None => "Choose an authored env_fog_controller. If the map has none, create one with Entity… and author its fog settings.".into(),
        Some(choice) => if let Some(issue) = choice["issue"].as_str() {
            format!("{}\nUnsupported settings: {}", choice["id"].as_str().unwrap(), issue)
        } else {
            let settings = &choice["settings"];
            let color = &settings["color"];
            format!("{} · {}\nColor: RGB {} {} {}\nStart: {} units · End: {} units\nMaximum density: {} · Authored fog: {}",
                choice["id"].as_str().unwrap(),choice["targetname"].as_str().filter(|name|!name.is_empty()).unwrap_or("unnamed"),
                color[0],color[1],color[2],
                settings["start"].as_f64().unwrap() as f32,
                settings["end"].as_f64().unwrap() as f32,
                settings["max_density"].as_f64().unwrap() as f32,
                if settings["enabled"] == true { "enabled" } else { "disabled (fogenable=0); preview draws no fog" })
        },
    };
    window.set_detail(detail.into());
}

fn refresh(window: &FogWindow, form: &RefCell<Form>, session: &Session) {
    let report = session.fog_preview_state();
    let mut form = form.borrow_mut();
    form.snapshot = report["snapshot"].as_str().unwrap().into();
    form.choices = report["controllers"].as_array().unwrap().clone();
    let mut labels: Vec<slint::SharedString> = vec!["Choose a controller".into()];
    labels.extend(form.choices.iter().map(|choice| {
        format!(
            "{} · {}{}",
            choice["id"].as_str().unwrap(),
            choice["targetname"]
                .as_str()
                .filter(|name| !name.is_empty())
                .unwrap_or("unnamed"),
            if choice["ready"] == true {
                ""
            } else {
                " · unsupported"
            }
        )
        .into()
    }));
    window.set_controllers(Rc::new(VecModel::from(labels)).into());
    window.set_controller_index(
        form.choices
            .iter()
            .position(|choice| choice["id"] == report["controller"])
            .map_or(0, |index| index as i32 + 1),
    );
    window.set_preview_enabled(report["enabled"].as_bool().unwrap());
    window.set_current(format!("Current: {}", report["summary"].as_str().unwrap()).into());
    window.set_message("".into());
    update_choice(window, &form);
}

fn callback(
    window: &slint::Weak<FogWindow>,
    target: &DocumentContinuation,
    body: impl FnOnce(FogWindow, &mut Session),
) {
    let Some(window) = window.upgrade() else {
        return;
    };
    let result = target.upgrade().and_then(|access| {
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
        let target = access.downgrade();
        let window = FogWindow::new()?;
        let form = Rc::new(RefCell::new(Form::default()));
        window.on_choice_changed({
            let (window, form, target) = (window.as_weak(), form.clone(), target.clone());
            move || {
                callback(&window, &target, |window, _| {
                    update_choice(&window, &form.borrow())
                })
            }
        });
        window.on_refresh({
            let (window, form, target) = (window.as_weak(), form.clone(), target.clone());
            move || {
                callback(&window, &target, |window, session| {
                    refresh(&window, &form, session)
                })
            }
        });
        window.on_apply({
            let (window, form, target) = (window.as_weak(), form.clone(), target.clone());
            move || {
                callback(&window, &target, |window, session| {
                    let form = form.borrow();
                    let controller = selected(&form, window.get_controller_index())
                        .and_then(|choice| choice["id"].as_str())
                        .and_then(|id| id.strip_prefix("entity:"))
                        .and_then(|id| id.parse().ok());
                    let result = session.set_fog_preview(
                        window.get_preview_enabled(),
                        controller,
                        &form.snapshot,
                    );
                    match result {
                        Ok(_) => {
                            let _ = window.hide();
                        }
                        Err(error) => window.set_message(error.into()),
                    }
                })
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
        Ok(Self { window, form })
    }

    pub fn show(&self, session: &Session) -> Result<(), slint::PlatformError> {
        refresh(&self.window, &self.form, session);
        self.window.show()
    }
}
