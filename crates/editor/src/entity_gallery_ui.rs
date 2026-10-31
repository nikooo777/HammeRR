use std::{cell::RefCell, collections::BTreeMap, rc::Rc};

use glam::DVec3;
use serde_json::Value;
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::{
    EntityGalleryClassRow, EntityGalleryPreviewRow, EntityGalleryWindow,
    runtime::{DocumentAccess, DocumentContinuation},
    session::{
        Actor, Session,
        entity_gallery::{Example, MAX_EXAMPLES, Request},
    },
};

#[derive(Default)]
struct State {
    page: usize,
    selected: BTreeMap<String, BTreeMap<String, String>>,
    token: Option<u64>,
    snapshot: String,
    stamp: String,
    dirty: bool,
    aspect: f32,
}

pub struct Controller {
    pub window: EntityGalleryWindow,
    state: Rc<RefCell<State>>,
}

fn selection_summary(window: &EntityGalleryWindow, state: &State) {
    let names = state
        .selected
        .keys()
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    window.set_selection_summary(
        format!(
            "{} / {MAX_EXAMPLES} selected · {names}",
            state.selected.len()
        )
        .into(),
    );
}

fn browse(
    window: &EntityGalleryWindow,
    state: &RefCell<State>,
    session: &Session,
) -> Result<(), String> {
    let mut state = state.borrow_mut();
    let report = session.browse_entity_gallery(&window.get_search(), state.page)?;
    let rows = report["classes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|class| {
            let classname = class["classname"].as_str().unwrap();
            let properties = state.selected.get(classname);
            let has_model = session
                .definitions()
                .class(classname)
                .is_some_and(|class| class.property("model").is_some());
            let model = properties
                .and_then(|properties| properties.get("model"))
                .map(String::as_str)
                .or_else(|| class["defaults"]["model"].as_str())
                .unwrap_or_default();
            let helpers = class["helpers"]
                .as_array()
                .unwrap()
                .iter()
                .map(|helper| {
                    let arguments = helper["arguments"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("{}({arguments})", helper["name"].as_str().unwrap())
                })
                .collect::<Vec<_>>()
                .join(" · ");
            EntityGalleryClassRow {
                classname: classname.into(),
                description: class["description"].as_str().unwrap_or_default().into(),
                detail: class["reason"]
                    .as_str()
                    .unwrap_or(if helpers.is_empty() {
                        "Editor origin marker"
                    } else {
                        &helpers
                    })
                    .into(),
                checked: properties.is_some(),
                available: class["placeable"] == true,
                has_model,
                model: model.into(),
            }
        })
        .collect::<Vec<_>>();
    window.set_classes(ModelRc::new(VecModel::from(rows)));
    window.set_page_summary(
        format!(
            "{} matching point classes · page {} · {} shown",
            report["total"],
            state.page + 1,
            report["classes"].as_array().unwrap().len()
        )
        .into(),
    );
    window.set_has_previous(state.page > 0);
    window.set_has_next(report["has_next"] == true);
    state.snapshot = report["snapshot"].as_str().unwrap().into();
    selection_summary(window, &state);
    if let Some(error) = report["definitions_error"].as_str() {
        window.set_message(error.into());
    }
    Ok(())
}

fn present(window: &EntityGalleryWindow, state: &RefCell<State>, session: &Session) {
    let report = session.get_entity_gallery();
    let draft = &report["draft"];
    let mut state = state.borrow_mut();
    state.token = draft["token"].as_u64();
    state.snapshot = report["snapshot"].as_str().unwrap().into();
    state.stamp = format!("{}:{}", state.snapshot, session.view_serial());
    let own = draft.is_null() || draft["owner"] == Actor::User.label();
    window.set_owned(own);
    window.set_shared_draft(!draft.is_null() && !own);
    window.set_ready(!state.dirty && draft["valid"] == true);
    let examples = draft["examples"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|example| {
            let origin = &example["origin"];
            let model = if let Some(error) = example["model_error"].as_str() {
                format!(" · Model unavailable: {error}")
            } else if example["model_state"] == "not_configured" {
                " · No model configured; set Example model to inspect one".into()
            } else {
                String::new()
            };
            EntityGalleryPreviewRow {
                title: format!(
                    "{} · row {}, column {} · ({:.1}, {:.1}, {:.1})",
                    example["classname"].as_str().unwrap(),
                    example["row"].as_u64().unwrap() + 1,
                    example["column"].as_u64().unwrap() + 1,
                    origin[0].as_f64().unwrap(),
                    origin[1].as_f64().unwrap(),
                    origin[2].as_f64().unwrap()
                )
                .into(),
                detail: format!(
                    "{}{model}",
                    example["helper"]["status"]
                        .as_str()
                        .unwrap_or("Editor helper; no FGD visual report")
                )
                .into(),
            }
        })
        .collect::<Vec<_>>();
    let count = examples.len();
    window.set_examples(ModelRc::new(VecModel::from(examples)));
    window.set_preview_summary(if count == 0 {"No preview. Examples appear in the main viewport.".into()}else{format!("{count} examples in the main viewport · {} columns · {}-unit gap · cell {}. Bounds fit models, icons and helper boxes; radius/line guides may overlap.",draft["columns"],draft["gap"],draft["cell_size"]).into()});
    window.set_message(if let Some(error) = draft["error"].as_str() {error.into()}
        else if !own {"Review shared gallery to take ownership before applying or cancelling.".into()}
        else if state.dirty {"Settings changed. Preview selected again before placing.".into()}
        else if count > 0 {"Review every example and asset warning in the viewport. Place reviewed examples adds one Undo step.".into()}
        else {"Select up to 32 classes. Missing model/icon resources remain explicit in the review.".into()});
}

fn invalidate(window: &EntityGalleryWindow, state: &RefCell<State>) {
    state.borrow_mut().dirty = true;
    window.set_ready(false);
    window.set_message("Settings changed. Preview selected again before placing.".into());
}

fn callback<R>(
    window: &slint::Weak<EntityGalleryWindow>,
    access: &DocumentContinuation,
    body: impl FnOnce(&EntityGalleryWindow, &mut Session) -> Result<R, String>,
) -> Option<R> {
    let window = window.upgrade()?;
    let result = access
        .upgrade()
        .and_then(|access| access.with_session(|session| body(&window, session)));
    match result {
        Ok(value) => Some(value),
        Err(error) => {
            window.set_message(error.into());
            None
        }
    }
}

impl Controller {
    pub fn new_with_access(access: &DocumentAccess) -> Result<Self, slint::PlatformError> {
        let window = EntityGalleryWindow::new()?;
        let state = Rc::new(RefCell::new(State::default()));
        let access = access.downgrade();
        window.on_browse({
            let (window, state, access) = (window.as_weak(), state.clone(), access.clone());
            move || {
                callback(&window, &access, |window, session| {
                    state.borrow_mut().page = 0;
                    browse(window, &state, session)
                });
            }
        });
        window.on_page({
            let (window, state, access) = (window.as_weak(), state.clone(), access.clone());
            move |delta| {
                callback(&window, &access, |window, session| {
                    let page = state.borrow().page.saturating_add_signed(delta as isize);
                    state.borrow_mut().page = page;
                    browse(window, &state, session)
                });
            }
        });
        window.on_toggle_class({
            let (window, state, access) = (window.as_weak(), state.clone(), access.clone());
            move |classname, checked| {
                callback(&window, &access, |window, session| {
                    if checked {
                        if state.borrow().selected.len() >= MAX_EXAMPLES {
                            return Err("Choose at most 32 classes per batch".into());
                        }
                        state
                            .borrow_mut()
                            .selected
                            .entry(classname.to_string())
                            .or_default();
                    } else {
                        state.borrow_mut().selected.remove(classname.as_str());
                    }
                    invalidate(window, &state);
                    browse(window, &state, session)
                });
            }
        });
        window.on_set_model({
            let (window, state, access) = (window.as_weak(), state.clone(), access.clone());
            move |classname, model| {
                callback(&window, &access, |window, _| {
                    if !state.borrow().selected.contains_key(classname.as_str())
                        && state.borrow().selected.len() >= MAX_EXAMPLES
                    {
                        return Err("Choose at most 32 classes per batch".into());
                    }
                    state
                        .borrow_mut()
                        .selected
                        .entry(classname.to_string())
                        .or_default()
                        .insert("model".into(), model.to_string());
                    invalidate(window, &state);
                    selection_summary(window, &state.borrow());
                    Ok(())
                });
            }
        });
        window.on_select_page({
            let (window,state,access)=(window.as_weak(),state.clone(),access.clone());
            move || {callback(&window,&access,|window,session| {
                let report=session.browse_entity_gallery(&window.get_search(),state.borrow().page)?;
                let names=report["classes"].as_array().unwrap().iter().filter(|class|class["placeable"]==true).map(|class|class["classname"].as_str().unwrap().to_string()).collect::<Vec<_>>();
                let new=names.iter().filter(|name|!state.borrow().selected.contains_key(*name)).count();
                if state.borrow().selected.len()+new > MAX_EXAMPLES {return Err("Selecting this page exceeds 32 classes; clear selection or choose individual classes".into());}
                for name in names {state.borrow_mut().selected.entry(name).or_default();}
                invalidate(window,&state);
                browse(window,&state,session)
            });}
        });
        window.on_clear_selection({
            let (window, state, access) = (window.as_weak(), state.clone(), access.clone());
            move || {
                callback(&window, &access, |window, session| {
                    state.borrow_mut().selected.clear();
                    invalidate(window, &state);
                    browse(window, &state, session)
                });
            }
        });
        window.on_invalidate({
            let (window, state) = (window.as_weak(), state.clone());
            move || {
                if let Some(window) = window.upgrade() {
                    invalidate(&window, &state);
                }
            }
        });
        window.on_preview({
            let (window, state, access) = (window.as_weak(), state.clone(), access.clone());
            move || {
                callback(&window, &access, |window, session| {
                    let request = Request {
                        examples: state
                            .borrow()
                            .selected
                            .iter()
                            .map(|(classname, properties)| Example {
                                classname: classname.clone(),
                                properties: properties.clone(),
                            })
                            .collect(),
                        origin: DVec3::new(
                            window
                                .get_origin_x()
                                .parse()
                                .map_err(|_| "Enter a finite X coordinate")?,
                            window
                                .get_origin_y()
                                .parse()
                                .map_err(|_| "Enter a finite Y coordinate")?,
                            window
                                .get_origin_z()
                                .parse()
                                .map_err(|_| "Enter a finite Z coordinate")?,
                        ),
                        columns: window
                            .get_columns()
                            .parse()
                            .map_err(|_| "Columns must be an integer from 1 to 32")?,
                        gap: window
                            .get_gap()
                            .parse()
                            .map_err(|_| "Gap must be from 16 to 4096 units")?,
                    };
                    let report = session.preview_entity_gallery(
                        Actor::User,
                        request,
                        &state.borrow().snapshot,
                    )?;
                    state.borrow_mut().dirty = false;
                    let token = report["draft"]["token"].as_u64().unwrap();
                    session.frame_entity_gallery(token, None, state.borrow().aspect)?;
                    present(window, &state, session);
                    Ok(())
                });
            }
        });
        window.on_review({
            let (window, state, access) = (window.as_weak(), state.clone(), access.clone());
            move || {
                callback(&window, &access, |window, session| {
                    session.review_entity_gallery(
                        Actor::User,
                        state.borrow().token.ok_or("No shared gallery to review")?,
                    )?;
                    state.borrow_mut().dirty = false;
                    present(window, &state, session);
                    Ok(())
                });
            }
        });
        window.on_frame({
            let (window, state, access) = (window.as_weak(), state.clone(), access.clone());
            move |index| {
                callback(&window, &access, |_, session| {
                    session.frame_entity_gallery(
                        state.borrow().token.ok_or("Preview the gallery first")?,
                        usize::try_from(index).ok(),
                        state.borrow().aspect,
                    )
                });
            }
        });
        window.on_apply({
            let (window, state, access) = (window.as_weak(), state.clone(), access.clone());
            move || {
                callback(&window, &access, |window, session| {
                    if state.borrow().dirty {
                        return Err("Preview the changed settings before placing".into());
                    }
                    session.apply_entity_gallery(
                        Actor::User,
                        state.borrow().token.ok_or("Preview the gallery first")?,
                    )?;
                    present(window, &state, session);
                    browse(window, &state, session)
                });
            }
        });
        window.on_cancel_preview({
            let (window, state, access) = (window.as_weak(), state.clone(), access.clone());
            move || {
                callback(&window, &access, |window, session| {
                    if let Some(token) = state.borrow().token {
                        session.cancel_entity_gallery(Actor::User, token)?;
                    }
                    state.borrow_mut().dirty = false;
                    present(window, &state, session);
                    browse(window, &state, session)
                });
            }
        });
        window.on_close_dialog({
            let (window, state, access) = (window.as_weak(), state.clone(), access.clone());
            move || {
                callback(&window, &access, |window, session| {
                    let report = session.get_entity_gallery();
                    if let Some(token) = state.borrow().token
                        && report["draft"]["token"].as_u64() == Some(token)
                        && report["draft"]["owner"] == Actor::User.label()
                    {
                        session.cancel_entity_gallery(Actor::User, token)?;
                    }
                    window.hide().map_err(|error| error.to_string())
                });
            }
        });
        window.window().on_close_requested({
            let window = window.as_weak();
            move || {
                if let Some(window) = window.upgrade() {
                    window.invoke_close_dialog();
                }
                slint::CloseRequestResponse::KeepWindowShown
            }
        });
        Ok(Self { window, state })
    }

    pub fn show(&self, session: &Session, aspect: f32) -> Result<(), slint::PlatformError> {
        self.state.borrow_mut().aspect = aspect;
        if self.state.borrow().snapshot.is_empty() {
            let origin = session.placement_origin();
            self.window.set_origin_x(origin.x.to_string().into());
            self.window.set_origin_y(origin.y.to_string().into());
            self.window.set_origin_z(origin.z.to_string().into());
        }
        present(&self.window, &self.state, session);
        if let Err(error) = browse(&self.window, &self.state, session) {
            self.window.set_message(error.into());
        }
        self.window.show()
    }

    pub fn refresh(&self, session: &Session, aspect: f32) {
        if !self.window.window().is_visible() {
            return;
        }
        self.state.borrow_mut().aspect = aspect;
        let stamp = format!(
            "{}:{}",
            session.entity_gallery_snapshot(),
            session.view_serial()
        );
        if self.state.borrow().stamp != stamp {
            present(&self.window, &self.state, session);
            if let Err(error) = browse(&self.window, &self.state, session) {
                self.window.set_message(error.into());
            }
        }
    }
}
