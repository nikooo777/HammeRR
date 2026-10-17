use std::{cell::RefCell, rc::Rc};

use map_core::surface::FaceId;
use serde_json::Value;
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::{
    CubemapFaceRow, CubemapFacesWindow,
    runtime::{DocumentAccess, DocumentContinuation},
    session::{
        Actor, SelectMode, Session,
        cubemap_bindings::{Operation, Query, Request},
    },
};

const PAGE_SIZE: usize = 20;

#[cfg(test)]
use crate::runtime::{SharedSession, lock};

#[derive(Default)]
struct Form {
    helper: Option<u32>,
    generation: u64,
    owned_token: Option<u64>,
    reviewed_token: Option<u64>,
    snapshot: String,
    seen: String,
    report: Option<Value>,
    report_snapshot: String,
    report_query: String,
    offset: usize,
}

pub struct Controller {
    pub window: CubemapFacesWindow,
    form: Rc<RefCell<Form>>,
}

fn face(value: &Value) -> Option<FaceId> {
    Some(FaceId {
        solid: value["solid"].as_u64()?.try_into().ok()?,
        side: value["side"].as_u64()?.try_into().ok()?,
    })
}

fn rows(values: &[Value], selected: &[FaceId]) -> ModelRc<CubemapFaceRow> {
    ModelRc::new(VecModel::from(
        values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let source = face(&value["face"]);
                CubemapFaceRow {
                    index: index as i32,
                    label: source
                        .map(|face| format!("Face {}:{}", face.solid, face.side))
                        .unwrap_or_else(|| format!("Side {}", value["side"]))
                        .into(),
                    choice: format!(
                        "{} · helper {} at {}",
                        if value["manual"] == true {
                            "Manual"
                        } else {
                            "Automatic"
                        },
                        value["helper"],
                        value["position"]
                    )
                    .into(),
                    material: format!("Material: {}", value["material"].as_str().unwrap_or(""))
                        .into(),
                    alias: format!("Alias: {}", value["alias"].as_str().unwrap_or("")).into(),
                    selected: source.is_some_and(|face| selected.contains(&face)),
                }
            })
            .collect::<Vec<_>>(),
    ))
}

fn manual_faces(form: &Form) -> Vec<FaceId> {
    form.report
        .as_ref()
        .and_then(|report| report["helpers"].as_array())
        .and_then(|helpers| {
            helpers
                .iter()
                .find(|helper| helper["helper"].as_u64() == form.helper.map(u64::from))
        })
        .and_then(|helper| helper["manual_faces"].as_array())
        .into_iter()
        .flatten()
        .filter_map(face)
        .collect()
}

fn inspection_current(session: &Session, form: &Form) -> bool {
    form.report.is_some()
        && form.generation == session.document_generation()
        && form.report_snapshot == session.cubemap_binding_report_stamp()
}

fn refresh(window: &CubemapFacesWindow, session: &Session, form: &RefCell<Form>) {
    let state = session.cubemap_binding_state();
    let stamp = state["snapshot"].as_str().unwrap_or("");
    let mut form = form.borrow_mut();
    if form.seen == stamp {
        return;
    }
    form.seen = stamp.into();
    form.snapshot = stamp.into();
    let same_map = form.generation == session.document_generation();
    let helper = form.helper.and_then(|id| session.document().entity(id));
    window.set_helper_label(
        match helper {
            Some(entity) if same_map => format!(
                "Helper {} · position {} · {}",
                entity.id,
                entity.properties.get("origin").unwrap_or("invalid"),
                if entity.hidden { "hidden" } else { "visible" }
            ),
            _ => "Select one visible env_cubemap and reopen Cubemap faces…".into(),
        }
        .into(),
    );
    let selected = session.selected_faces();
    window.set_selection_label(format!("{} selected faces", selected.len()).into());
    let current = inspection_current(session, &form);
    let manual = manual_faces(&form);
    let valid =
        same_map && helper.is_some_and(|helper| !helper.hidden) && state["context_error"].is_null();
    let clean = form
        .report
        .as_ref()
        .is_some_and(|report| report["diagnostics"].as_array().is_some_and(Vec::is_empty));
    window.set_can_bind(
        valid
            && !selected.is_empty()
            && selected.len() <= crate::session::cubemap_bindings::MAX_FACES,
    );
    window.set_can_remove(
        valid
            && current
            && clean
            && !selected.is_empty()
            && selected.iter().all(|face| manual.contains(face)),
    );
    window.set_can_clear(valid && current && clean && !manual.is_empty());
    window.set_can_select_bound(valid && current && clean && !manual.is_empty());
    let entries = form
        .report
        .as_ref()
        .and_then(|report| report["assignments"].as_array())
        .cloned()
        .unwrap_or_default();
    window.set_assignments(rows(&entries, &selected));
    let total = form
        .report
        .as_ref()
        .and_then(|report| report["total"].as_u64())
        .unwrap_or(0) as usize;
    window.set_previous_page(current && form.offset > 0);
    window.set_next_page(current && form.offset + entries.len() < total);
    window.set_page_label(
        if total == 0 {
            "No assignments".into()
        } else {
            format!(
                "{}–{} of {total}",
                form.offset + 1,
                form.offset + entries.len()
            )
        }
        .into(),
    );
    window.set_inspection_label(
        match &form.report {
            Some(report) => format!(
                "{} · namespace {}{}",
                if current {
                    "Current assignments"
                } else {
                    "Map, content or output name changed · Refresh assignments"
                },
                report["namespace"].as_str().unwrap_or("unavailable"),
                if report["provisional_namespace"] == true {
                    " (provisional)"
                } else {
                    ""
                }
            ),
            None => "Refresh to inspect compiler sample choices and material aliases.".into(),
        }
        .into(),
    );
    window.set_helper_status(
        form.report
            .as_ref()
            .map(|report| {
                let helpers = report["helpers"].as_array().cloned().unwrap_or_default();
                let hidden = helpers
                    .iter()
                    .filter(|helper| helper["hidden"] == true)
                    .count();
                let excluded = helpers
                    .iter()
                    .filter(|helper| helper["excluded"] == true)
                    .count();
                let unknown = helpers
                    .iter()
                    .filter(|helper| helper["excluded"].is_null())
                    .count();
                let inactive = helpers
                    .iter()
                    .filter(|helper| helper["hidden"] == true || helper["excluded"] == true)
                    .take(8)
                    .map(|helper| {
                        format!(
                            "{} ({})",
                            helper["entity"].as_str().unwrap_or("unknown"),
                            if helper["hidden"] == true {
                                "hidden"
                            } else {
                                "excluded"
                            }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                let diagnostics = report["diagnostics"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("; ");
                format!(
                    "{} helpers · {hidden} hidden · {excluded} excluded{}{}{}",
                    helpers.len(),
                    if unknown > 0 {
                        format!(" · {unknown} inclusion unknown")
                    } else {
                        String::new()
                    },
                    if inactive.is_empty() {
                        String::new()
                    } else {
                        format!(
                            "\n{inactive}{}",
                            if hidden + excluded > 8 { " …" } else { "" }
                        )
                    },
                    if diagnostics.is_empty() {
                        String::new()
                    } else {
                        format!("\n{diagnostics}")
                    }
                )
            })
            .unwrap_or_default()
            .into(),
    );
    let draft = &state["draft"];
    let token = draft["token"].as_u64();
    let ready = token.is_some()
        && token == form.owned_token
        && token == form.reviewed_token
        && draft["owner"] == Actor::User.label()
        && draft["valid"] == true
        && draft["helper"].as_u64() == form.helper.map(u64::from)
        && same_map;
    window.set_ready(ready);
    window.set_can_review(token.is_some() && draft["valid"] == true && !ready);
    window.set_draft_label(
        if draft.is_null() {
            "No pending binding change".into()
        } else {
            format!(
                "Draft by {} · helper {} · {} faces · {}{}",
                draft["owner"].as_str().unwrap_or("unknown"),
                draft["helper"],
                draft["faces"].as_array().map_or(0, Vec::len),
                draft["operation"].as_str().unwrap_or(""),
                if draft["valid"] == true {
                    ""
                } else {
                    " · stale: preview again"
                }
            )
        }
        .into(),
    );
    window.set_preview_assignments(rows(
        draft["report"]["assignments"]
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[]),
        &[],
    ));
}

fn inspect(window: &CubemapFacesWindow, session: &Session, form: &RefCell<Form>) {
    let (helper, offset, generation) = {
        let form = form.borrow();
        (form.helper, form.offset, form.generation)
    };
    if generation != session.document_generation() {
        window.set_message(
            "Map changed. Select one visible env_cubemap and reopen Cubemap faces….".into(),
        );
        return;
    }
    match session.get_cubemap_bindings(Query {
        helper,
        offset,
        limit: PAGE_SIZE,
        query: window.get_search().to_string(),
    }) {
        Ok(report) => {
            let mut form = form.borrow_mut();
            form.report_snapshot = report["report_snapshot"].as_str().unwrap_or("").into();
            form.report_query = window.get_search().to_string();
            form.report = Some(report);
            form.seen.clear();
            window.set_message(
                "Assignments refreshed. Choose faces in this list or in the map.".into(),
            );
        }
        Err(error) => window.set_message(error.into()),
    }
    refresh(window, session, form);
}

fn adopt(form: &RefCell<Form>, state: &Value) {
    let mut form = form.borrow_mut();
    let token = state["draft"]["token"].as_u64();
    form.owned_token = token;
    form.reviewed_token = token;
    form.helper = state["draft"]["helper"]
        .as_u64()
        .and_then(|id| id.try_into().ok());
    form.seen.clear();
}

fn preview(
    window: &CubemapFacesWindow,
    session: &mut Session,
    form: &RefCell<Form>,
    operation: Operation,
    all: bool,
) {
    let result = (|| {
        let form = form.borrow();
        if form.generation != session.document_generation() {
            return Err("Map changed; reopen Cubemap faces…".into());
        }
        let helper = form
            .helper
            .ok_or("Select one visible env_cubemap and reopen Cubemap faces…")?;
        let faces = if all {
            if !inspection_current(session, &form) {
                return Err("Refresh before clearing the helper's manual bindings".into());
            }
            manual_faces(&form)
        } else {
            session.selected_faces()
        };
        session.preview_cubemap_bindings(
            Actor::User,
            Request {
                helper,
                faces,
                operation,
            },
            &form.snapshot,
        )
    })();
    match result {
        Ok(state) => {
            adopt(form, &state);
            window.set_message("Review the exact sample choices below, then Apply. The map is unchanged until Apply.".into());
        }
        Err(error) => {
            form.borrow_mut().reviewed_token = None;
            form.borrow_mut().seen.clear();
            window.set_message(error.into());
        }
    }
    refresh(window, session, form);
}

fn cancel_owned(session: &mut Session, form: &RefCell<Form>) {
    let token = form.borrow().owned_token;
    if let Some(token) = token {
        let _ = session.cancel_cubemap_bindings(Actor::User, token);
    }
    form.borrow_mut().owned_token = None;
    form.borrow_mut().reviewed_token = None;
    form.borrow_mut().seen.clear();
}

fn callback(
    window: &slint::Weak<CubemapFacesWindow>,
    document: &DocumentContinuation,
    body: impl FnOnce(CubemapFacesWindow, &mut Session),
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
    pub fn new(shared: &SharedSession) -> Result<Self, slint::PlatformError> {
        Self::new_with_access(&DocumentAccess::unmanaged(shared.clone()))
    }

    pub fn new_with_access(access: &DocumentAccess) -> Result<Self, slint::PlatformError> {
        let window = CubemapFacesWindow::new()?;
        let form = Rc::new(RefCell::new(Form::default()));
        window.on_refresh({
            let (window, document, form) = (window.as_weak(), access.downgrade(), form.clone());
            move || {
                callback(&window, &document, |window, session| {
                    form.borrow_mut().offset = 0;
                    inspect(&window, session, &form);
                });
            }
        });
        window.on_page_change({
            let (window, document, form) = (window.as_weak(), access.downgrade(), form.clone());
            move |direction| {
                callback(&window, &document, |window, session| {
                    if !inspection_current(session, &form.borrow())
                        || form.borrow().report_query != window.get_search().as_str()
                    {
                        window.set_message("Refresh assignments before changing pages.".into());
                        return;
                    }
                    let mut state = form.borrow_mut();
                    state.offset = if direction < 0 {
                        state.offset.saturating_sub(PAGE_SIZE)
                    } else {
                        state.offset.saturating_add(PAGE_SIZE)
                    };
                    drop(state);
                    inspect(&window, session, &form);
                });
            }
        });
        window.on_select_face({
            let (window, document, form) = (window.as_weak(), access.downgrade(), form.clone());
            move |index| {
                callback(&window, &document, |window, session| {
                    let source = {
                        let form = form.borrow();
                        if !inspection_current(session, &form) {
                            window.set_message(
                                "Refresh assignments before selecting a listed face.".into(),
                            );
                            return;
                        }
                        usize::try_from(index)
                            .ok()
                            .and_then(|index| form.report.as_ref()?.get("assignments")?.get(index))
                            .and_then(|row| face(&row["face"]))
                    };
                    if let Some(source) = source
                        && let Err(error) = session.select_faces(&[source], SelectMode::Toggle)
                    {
                        window.set_message(error.into());
                    }
                    refresh(&window, session, &form);
                });
            }
        });
        window.on_select_bound({
            let (window, document, form) = (window.as_weak(), access.downgrade(), form.clone());
            move || {
                callback(&window, &document, |window, session| {
                    if !inspection_current(session, &form.borrow()) {
                        window.set_message("Refresh before selecting bound faces.".into());
                        return;
                    }
                    let faces = manual_faces(&form.borrow());
                    if let Err(error) = session.select_faces(&faces, SelectMode::Replace) {
                        window.set_message(error.into());
                    }
                    refresh(&window, session, &form);
                });
            }
        });
        window.on_bind({
            let (window, document, form) = (window.as_weak(), access.downgrade(), form.clone());
            move || {
                callback(&window, &document, |window, session| {
                    preview(&window, session, &form, Operation::Bind, false)
                });
            }
        });
        window.on_remove({
            let (window, document, form) = (window.as_weak(), access.downgrade(), form.clone());
            move || {
                callback(&window, &document, |window, session| {
                    preview(&window, session, &form, Operation::Remove, false)
                });
            }
        });
        window.on_clear_bindings({
            let (window, document, form) = (window.as_weak(), access.downgrade(), form.clone());
            move || {
                callback(&window, &document, |window, session| {
                    preview(&window, session, &form, Operation::Remove, true)
                });
            }
        });
        window.on_review({
            let (window, document, form) = (window.as_weak(), access.downgrade(), form.clone());
            move || {
                callback(&window, &document, |window, session| {
                    let token = session.cubemap_binding_state()["draft"]["token"].as_u64();
                    if let Some(token) = token {
                        match session.review_cubemap_bindings(Actor::User, token) {
                            Ok(state) => {
                                let mut current = form.borrow_mut();
                                if current.helper.map(u64::from) != state["draft"]["helper"].as_u64() {
                                    current.report = None;
                                }
                                current.generation = session.document_generation();
                                drop(current);
                                adopt(&form, &state);
                                let message = "Current draft reviewed. Check its helper, faces and exact sample choices before Apply.";
                                window.set_message(message.into());
                            }
                            Err(error) => window.set_message(error.into()),
                        }
                    }
                    refresh(&window, session, &form);
                });
            }
        });
        window.on_apply({
            let (window, document, form) = (window.as_weak(), access.downgrade(), form.clone());
            move || {
                callback(&window, &document, |window, session| {
                    refresh(&window, session, &form);
                    if !window.get_ready() {
                        window.set_message("Preview or Review the current binding draft before Apply.".into());
                        return;
                    }
                    let token = form.borrow().reviewed_token.unwrap();
                    match session.apply_cubemap_bindings(Actor::User, token) {
                        Ok(_) => {
                            let mut form = form.borrow_mut();
                            form.owned_token = None;
                            form.reviewed_token = None;
                            form.seen.clear();
                            let message = "Cubemap bindings applied. Undo restores the previous sides property. Refresh to update the assignment list.";
                            window.set_message(message.into());
                        }
                        Err(error) => {
                            let mut form = form.borrow_mut();
                            form.reviewed_token = None;
                            form.seen.clear();
                            window.set_message(error.into());
                        }
                    }
                    refresh(&window, session, &form);
                });
            }
        });
        window.on_cancel({
            let (window, document, form) = (window.as_weak(), access.downgrade(), form.clone());
            move || {
                callback(&window, &document, |_window, session| {
                    cancel_owned(session, &form);
                });
                if let Some(window) = window.upgrade() {
                    let _ = window.hide();
                }
            }
        });
        window.window().on_close_requested({
            let (window, document, form) = (window.as_weak(), access.downgrade(), form.clone());
            move || {
                callback(&window, &document, |_window, session| {
                    cancel_owned(session, &form);
                });
                slint::CloseRequestResponse::HideWindow
            }
        });
        Ok(Self { window, form })
    }

    pub fn show(&self, session: &Session) -> Result<(), slint::PlatformError> {
        let mut form = self.form.borrow_mut();
        form.helper = session.selected_cubemap_helper();
        form.generation = session.document_generation();
        form.reviewed_token = None;
        form.report = None;
        form.offset = 0;
        form.seen.clear();
        drop(form);
        self.window.set_search("".into());
        if self.form.borrow().helper.is_some() {
            inspect(&self.window, session, &self.form);
        } else {
            refresh(&self.window, session, &self.form);
        }
        self.window.show()
    }

    pub fn refresh(&self, session: &Session) {
        refresh(&self.window, session, &self.form);
    }
}

#[cfg(test)]
mod admission_tests;

#[cfg(test)]
mod tests;
