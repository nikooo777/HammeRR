use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use glam::DVec3;
use map_core::{ObjectId, repeat::Options};
use slint::ComponentHandle;

use crate::{
    RepeatWindow,
    runtime::{SharedSession, lock},
    session::{Actor, Session},
};

#[derive(Clone)]
struct Context {
    ids: Vec<ObjectId>,
    generation: u64,
    revision: u64,
    definitions: u64,
}

pub struct Controller {
    pub window: RepeatWindow,
    context: Rc<RefCell<Option<Context>>>,
    token: Rc<Cell<Option<u64>>>,
    prepared: Rc<Cell<Option<Options>>>,
}

fn options(window: &RepeatWindow) -> Result<Options, String> {
    let vector = |values: [slint::SharedString; 3]| {
        let mut result = DVec3::ZERO;
        for (index, value) in values.iter().enumerate() {
            result[index] = value
                .parse::<f64>()
                .map_err(|_| "Enter finite XYZ numbers")?;
        }
        Ok::<DVec3, String>(result)
    };
    Ok(Options {
        count: u32::try_from(window.get_count()).map_err(|_| "Choose a positive copy count")?,
        translation: vector([
            window.get_move_x(),
            window.get_move_y(),
            window.get_move_z(),
        ])?,
        rotation: vector([
            window.get_rotate_x(),
            window.get_rotate_y(),
            window.get_rotate_z(),
        ])?,
        pivot: vector([
            window.get_pivot_x(),
            window.get_pivot_y(),
            window.get_pivot_z(),
        ])?,
        texture_lock: window.get_texture_lock(),
    })
}

fn validate(
    session: &Session,
    context: &RefCell<Option<Context>>,
    token: Option<u64>,
) -> Result<Vec<ObjectId>, String> {
    let context = context.borrow();
    let context = context.as_ref().ok_or("Reopen Repeat array")?;
    if context.generation != session.document_generation()
        || context.revision != session.document().revision()
        || context.definitions != session.definitions_serial()
        || context.ids != session.selected_ids()
    {
        return Err("Map, selection or entity definitions changed; reopen Repeat array".into());
    }
    if session.repeat_array()["preview"]["token"].as_u64() != token {
        return Err("Repeat array was replaced; reopen to review it".into());
    }
    Ok(context.ids.clone())
}

fn present(window: &RepeatWindow, report: &serde_json::Value) {
    let preview = &report["preview"];
    window.set_ready(preview["valid"] == true);
    let mut lines = vec![format!(
        "{} additional copies · {} new objects",
        preview["count"], preview["object_count"]
    )];
    if let Some(copies) = preview["copies"].as_array() {
        for copy in copies {
            lines.push(format!(
                "Copy {} · {} internal reference updates",
                copy["index"], copy["remapped_references"]
            ));
            if let Some(names) = copy["names"].as_array() {
                for name in names {
                    lines.push(format!(
                        "  {} → {}",
                        name["before"].as_str().unwrap_or_default(),
                        name["after"].as_str().unwrap_or_default()
                    ));
                }
            }
        }
    }
    window.set_summary(lines.join("\n").into());
    window.set_message(
        "Review the copies and names. Apply array creates them in one Undo step.".into(),
    );
}

impl Controller {
    pub fn new(shared: &SharedSession) -> Result<Self, slint::PlatformError> {
        let window = RepeatWindow::new()?;
        window
            .window()
            .set_size(slint::LogicalSize::new(660.0, 650.0));
        let context = Rc::new(RefCell::new(None));
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
            let (window, shared, context, token, prepared) = (
                window.as_weak(),
                shared.clone(),
                context.clone(),
                token.clone(),
                prepared.clone(),
            );
            move || {
                let window = window.unwrap();
                let result = (|| {
                    let mut session = lock(&shared);
                    let ids = validate(&session, &context, token.get())?;
                    let requested = options(&window)?;
                    let snapshot = session.repeat_stamp();
                    let report = session.preview_repeat_array(&ids, requested, &snapshot)?;
                    Ok::<_, String>((requested, report))
                })();
                match result {
                    Ok((requested, report)) => {
                        token.set(report["preview"]["token"].as_u64());
                        prepared.set(Some(requested));
                        present(&window, &report);
                    }
                    Err(error) => {
                        window.set_ready(false);
                        window.set_message(error.into());
                    }
                }
            }
        });
        window.on_apply({
            let (window, shared, context, token, prepared) = (
                window.as_weak(),
                shared.clone(),
                context.clone(),
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
                    validate(&session, &context, token.get())?;
                    session.apply_repeat_array(
                        Actor::User,
                        token.get().ok_or("Preview the array first")?,
                    )
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
                    let _ = lock(&shared).cancel_repeat_array(token);
                }
                prepared.set(None);
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
        Ok(Self {
            window,
            context,
            token,
            prepared,
        })
    }

    pub fn initialize(&self, session: &Session) -> Result<(), String> {
        let ids = session.selected_ids();
        if ids.is_empty() || ids.len() > map_core::repeat::MAX_SOURCES {
            return Err("Select 1–16 objects to repeat".into());
        }
        let report = session.repeat_array();
        let expected = ids
            .iter()
            .copied()
            .map(crate::identity::id_string)
            .collect::<std::collections::BTreeSet<_>>();
        let preview = &report["preview"];
        let current = preview["sources"].as_array().map(|sources| {
            sources
                .iter()
                .filter_map(|source| source.as_str().map(str::to_owned))
                .collect::<std::collections::BTreeSet<_>>()
        });
        let resume = preview["valid"] == true && current.as_ref() == Some(&expected);
        *self.context.borrow_mut() = Some(Context {
            ids: ids.clone(),
            generation: session.document_generation(),
            revision: session.document().revision(),
            definitions: session.definitions_serial(),
        });
        self.token.set(if resume {
            preview["token"].as_u64()
        } else {
            None
        });
        self.prepared.set(None);
        self.window.set_selection(
            format!(
                "{} source object{}",
                ids.len(),
                if ids.len() == 1 { "" } else { "s" }
            )
            .into(),
        );
        let pivot = session
            .transform_tool()
            .pivot
            .unwrap_or(session.selection_pivot(&ids)?);
        let read_vector = |key: &str, fallback: DVec3| {
            preview[key].as_array().map_or(fallback, |values| {
                DVec3::new(
                    values[0].as_f64().unwrap(),
                    values[1].as_f64().unwrap(),
                    values[2].as_f64().unwrap(),
                )
            })
        };
        let (translation, rotation, pivot) = if resume {
            (
                read_vector("translation", DVec3::ZERO),
                read_vector("rotation", DVec3::ZERO),
                read_vector("pivot", pivot),
            )
        } else {
            (DVec3::X * 64.0, DVec3::ZERO, pivot)
        };
        self.window.set_count(if resume {
            preview["count"].as_i64().unwrap() as i32
        } else {
            3
        });
        self.window.set_move_x(translation.x.to_string().into());
        self.window.set_move_y(translation.y.to_string().into());
        self.window.set_move_z(translation.z.to_string().into());
        self.window.set_rotate_x(rotation.x.to_string().into());
        self.window.set_rotate_y(rotation.y.to_string().into());
        self.window.set_rotate_z(rotation.z.to_string().into());
        self.window.set_pivot_x(pivot.x.to_string().into());
        self.window.set_pivot_y(pivot.y.to_string().into());
        self.window.set_pivot_z(pivot.z.to_string().into());
        self.window.set_texture_lock(if resume {
            preview["texture_lock"] == true
        } else {
            session.transform_tool().texture_lock
        });
        self.window.set_summary("".into());
        self.window.set_ready(false);
        self.window
            .set_message("Set copy count and steps, then Preview array.".into());
        if resume {
            self.prepared.set(Some(options(&self.window)?));
            present(&self.window, &report);
        }
        Ok(())
    }

    pub fn refresh(&self, session: &Session) {
        if let Err(error) = validate(session, &self.context, self.token.get()) {
            self.window.set_ready(false);
            self.window.set_message(error.into());
        } else if self.token.get().is_some() && self.prepared.get().is_some() {
            present(&self.window, &session.repeat_array());
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[test]
    fn repeat_controls_preview_apply_cancel_and_preserve_foreign_drafts() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let (session, source) = crate::session::repeat::tests::fixture();
        let before = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let shared = Arc::new(Mutex::new(session));
        let controller = Controller::new(&shared).unwrap();
        controller.initialize(&lock(&shared)).unwrap();
        controller.window.set_rotate_z("15".into());
        controller.window.invoke_preview();
        assert!(controller.window.get_ready());
        assert!(controller.window.get_summary().contains("array_part_copy3"));
        assert_eq!(lock(&shared).document().vmf(), &before);
        assert_eq!(lock(&shared).document().undo_labels().count(), history);
        controller.window.show().unwrap();
        crate::ui::tests::save_ui_snapshot(controller.window.window(), "repeat-array");
        controller.window.set_move_y("16".into());
        controller.window.invoke_invalidate();
        controller.window.invoke_apply();
        assert_eq!(lock(&shared).document().vmf(), &before);
        controller.window.invoke_preview();
        controller.window.invoke_apply();
        assert!(!controller.window.window().is_visible());
        assert_eq!(lock(&shared).document().undo_labels().count(), history + 1);
        lock(&shared).undo(Actor::User).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &before);
        lock(&shared)
            .select(&[source], crate::session::SelectMode::Replace)
            .unwrap();
        controller.initialize(&lock(&shared)).unwrap();
        controller.window.invoke_preview();
        lock(&shared)
            .preview_transform(
                Actor::Agent,
                &[source],
                map_core::edit::Transform {
                    translation: DVec3::Z * 20.0,
                    ..map_core::edit::Transform::default()
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
        controller.initialize(&lock(&shared)).unwrap();
        controller.window.invoke_preview();
        controller.window.invoke_cancel();
        assert_eq!(lock(&shared).document().vmf(), &before);
        assert!(lock(&shared).repeat_array()["preview"].is_null());
    }
}
