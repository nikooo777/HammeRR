use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use map_core::ObjectId;
use serde_json::Value;
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::{
    BulkEntitiesWindow,
    runtime::{DocumentAccess, DocumentContinuation},
    session::{Actor, Session},
};

#[derive(Default)]
struct State {
    ids: Vec<ObjectId>,
    generation: u64,
    revision: u64,
    definitions: u64,
    token: Option<u64>,
    classname: String,
    move_to_world: bool,
    dirty: bool,
}

type RefreshStamp = (u64, u64, u64, u64, Option<u64>);

pub struct Controller {
    pub window: BulkEntitiesWindow,
    state: Rc<RefCell<State>>,
    seen: Cell<Option<RefreshStamp>>,
}

fn current(state: &State, session: &Session) -> Result<(), String> {
    if state.generation != session.document_generation()
        || state.revision != session.document().revision()
        || state.definitions != session.definitions_serial()
    {
        return Err("Map or entity definitions changed; reopen the action".into());
    }
    Ok(())
}

fn target_text(row: &Value, move_to_world: bool) -> slint::SharedString {
    let mut text = format!(
        "{} · {} → {}",
        row["entity"].as_str().unwrap_or_default(),
        row["before"].as_str().unwrap_or("(no class)"),
        row["after"].as_str().unwrap_or_default()
    );
    if let Some(name) = row["name"].as_str().filter(|name| !name.is_empty()) {
        text.push_str(&format!("\nName · {name}"));
    }
    if move_to_world {
        text.push_str(&format!(
            "\n{} brushes retain their source geometry and UVs.",
            row["brushes"]
        ));
        if let Some(properties) = row["removed_properties"].as_array() {
            for property in properties {
                text.push_str(&format!(
                    "\nRemove {} = {}",
                    property[0].as_str().unwrap_or_default(),
                    property[1].as_str().unwrap_or_default()
                ));
            }
        }
        if let Some(connections) = row["removed_connections"].as_array() {
            for connection in connections {
                text.push_str(&format!(
                    "\nRemove output {} → {}",
                    connection["output"].as_str().unwrap_or_default(),
                    connection["value"].as_str().unwrap_or_default()
                ));
            }
        }
        if let Some(properties) = row["source_editor_properties"].as_array() {
            for property in properties {
                text.push_str(&format!(
                    "\nOwner editor {} = {}",
                    property[0].as_str().unwrap_or_default(),
                    property[1].as_str().unwrap_or_default()
                ));
            }
        }
    } else {
        text.push_str(&format!(
            "\n{} properties and {} connections retained",
            row["retained_properties"], row["retained_connections"]
        ));
    }
    text.into()
}

fn refresh(window: &BulkEntitiesWindow, state: &State, session: &Session) {
    window.set_can_apply(false);
    if let Err(error) = current(state, session) {
        window.set_message(error.into());
        return;
    }
    let Some(token) = state.token else {
        return;
    };
    let report = session.bulk_entities_report();
    if report["token"].as_u64() != Some(token) {
        window.set_message("Preview was closed or replaced elsewhere; reopen to review it".into());
        return;
    }
    window.set_targets(ModelRc::new(VecModel::from(
        report["targets"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|row| target_text(row, state.move_to_world))
            .collect::<Vec<_>>(),
    )));
    window.set_warning(
        report["warnings"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("\n")
            .into(),
    );
    window.set_can_apply(report["can_apply"].as_bool().unwrap_or(false) && !state.dirty);
    if let Some(error) = report["error"].as_str() {
        window.set_message(error.into());
    }
}

fn with_session<R>(
    window: &slint::Weak<BulkEntitiesWindow>,
    access: &DocumentContinuation,
    body: impl FnOnce(&BulkEntitiesWindow, &mut Session) -> R,
) -> Option<R> {
    let window = window.upgrade()?;
    match access
        .upgrade()
        .and_then(|access| access.with_session(|session| Ok(body(&window, session))))
    {
        Ok(result) => Some(result),
        Err(error) => {
            window.set_can_apply(false);
            window.set_message(error.into());
            None
        }
    }
}

impl Controller {
    pub fn new(access: &DocumentAccess) -> Result<Self, slint::PlatformError> {
        let window = BulkEntitiesWindow::new()?;
        let state = Rc::new(RefCell::new(State::default()));
        let access = access.downgrade();
        window.on_fields_changed({
            let (window, state) = (window.as_weak(), state.clone());
            move || {
                state.borrow_mut().dirty = true;
                if let Some(window) = window.upgrade() {
                    window.set_can_apply(false);
                    window.set_message(
                        "Preview class changes to validate every captured entity".into(),
                    );
                }
            }
        });
        window.on_preview({
            let (window, access, state) = (window.as_weak(), access.clone(), state.clone());
            move || {
                with_session(&window, &access, |window, session| {
                    let mut state = state.borrow_mut();
                    let classname = window.get_classname().to_string();
                    let result = current(&state, session).and_then(|()| {
                        if state.move_to_world {
                            session.preview_move_entities_to_world(&state.ids)
                        } else {
                            session.preview_entity_class_change(&state.ids, &classname)
                        }
                    });
                    match result {
                        Ok(report) => {
                            state.token = report["token"].as_u64();
                            state.classname =
                                report["classname"].as_str().unwrap_or_default().into();
                            state.dirty = false;
                            window.set_classname(state.classname.clone().into());
                            window.set_message("Review every target, then Apply once".into());
                            refresh(window, &state, session);
                        }
                        Err(error) => {
                            window.set_can_apply(false);
                            window.set_message(error.into());
                        }
                    }
                });
            }
        });
        window.on_apply({
            let (window, access, state) = (window.as_weak(), access.clone(), state.clone());
            move || {
                with_session(&window, &access, |window, session| {
                    let mut state = state.borrow_mut();
                    let result = current(&state, session).and_then(|()| {
                        if state.dirty
                            || !state.move_to_world
                                && window.get_classname().as_str() != state.classname
                        {
                            return Err("Preview the edited class name before applying".into());
                        }
                        session.apply_bulk_entities(
                            Actor::User,
                            state.token.ok_or("Preview the action first")?,
                        )
                    });
                    match result {
                        Ok(_) => {
                            state.token = None;
                            window.set_can_apply(false);
                            if let Err(error) = window.hide() {
                                window.set_message(
                                    format!("Could not close the window: {error}").into(),
                                );
                            }
                        }
                        Err(error) => {
                            window.set_can_apply(false);
                            window.set_message(error.into());
                        }
                    }
                });
            }
        });
        window.on_cancel({
            let (window, access, state) = (window.as_weak(), access.clone(), state.clone());
            move || {
                with_session(&window, &access, |window, session| {
                    let mut state = state.borrow_mut();
                    if let Some(token) = state.token
                        && session.bulk_entities_report()["token"].as_u64() == Some(token)
                        && let Err(error) = session.cancel_bulk_entities(token)
                    {
                        window.set_message(error.into());
                        window.set_can_apply(false);
                        return;
                    }
                    state.token = None;
                    if let Err(error) = window.hide() {
                        window.set_message(format!("Could not close the window: {error}").into());
                    }
                });
            }
        });
        window.window().on_close_requested({
            let window = window.as_weak();
            move || {
                if let Some(window) = window.upgrade() {
                    window.invoke_cancel();
                }
                slint::CloseRequestResponse::KeepWindowShown
            }
        });
        Ok(Self {
            window,
            state,
            seen: Cell::new(None),
        })
    }

    pub fn show(
        &self,
        session: &mut Session,
        ids: &[ObjectId],
        move_to_world: bool,
    ) -> Result<(), String> {
        self.seen.set(None);
        let entities = session.whole_entity_targets(ids)?;
        let ids = entities
            .iter()
            .map(|id| ObjectId::Entity(*id))
            .collect::<Vec<_>>();
        let report = session.bulk_entities_report();
        let operation = if move_to_world {
            "move_to_world"
        } else {
            "class_change"
        };
        let same = report["operation"] == operation
            && report["can_apply"] == true
            && report["entities"].as_array().is_some_and(|captured| {
                captured.iter().filter_map(Value::as_str).eq(ids
                    .iter()
                    .map(|id| id.to_string())
                    .collect::<Vec<_>>()
                    .iter()
                    .map(String::as_str))
            });
        let report = if same {
            report
        } else if move_to_world {
            session.preview_move_entities_to_world(&ids)?
        } else {
            Value::Null
        };
        let current_classes = entities
            .iter()
            .filter_map(|id| {
                session
                    .document()
                    .entity(*id)
                    .and_then(|entity| entity.classname())
            })
            .collect::<std::collections::BTreeSet<_>>();
        let classname = report["classname"]
            .as_str()
            .map(String::from)
            .or_else(|| {
                (current_classes.len() == 1).then(|| current_classes.first().unwrap().to_string())
            })
            .unwrap_or_default();
        *self.state.borrow_mut() = State {
            ids,
            generation: session.document_generation(),
            revision: session.document().revision(),
            definitions: session.definitions_serial(),
            token: report["token"].as_u64(),
            classname: classname.clone(),
            move_to_world,
            dirty: false,
        };
        self.window.set_move_to_world(move_to_world);
        self.window.set_classname(classname.into());
        self.window.set_summary(
            format!(
                "{} captured whole entities · selection changes do not retarget this action",
                entities.len()
            )
            .into(),
        );
        self.window.set_warning("".into());
        self.window.set_message(if move_to_world {"Review the removed entity behavior, then Apply once"}
            else {"Enter a loaded compatible FGD class name. Retained properties are validated before preview."}.into());
        self.window.set_can_apply(false);
        self.window.set_targets(ModelRc::new(VecModel::from(
            entities
                .iter()
                .map(|id| {
                    let classname = session
                        .document()
                        .entity(*id)
                        .and_then(|entity| entity.classname())
                        .unwrap_or("(no class)");
                    format!("entity:{id} · {classname}").into()
                })
                .collect::<Vec<slint::SharedString>>(),
        )));
        self.refresh(session);
        self.window.show().map_err(|error| error.to_string())
    }

    pub fn refresh(&self, session: &Session) {
        let state = self.state.borrow();
        let stamp = (
            session.document_generation(),
            session.document().revision(),
            session.definitions_serial(),
            session.view_serial(),
            state.token,
        );
        if self.seen.replace(Some(stamp)) != Some(stamp) {
            refresh(&self.window, &state, session);
        }
    }
}
