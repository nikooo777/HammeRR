use crate::{
    LayerRow, LayersWindow,
    mcp::{SharedSession, lock},
    session::{Actor, SelectMode, Session},
};
use map_core::ObjectId;
use serde_json::Value;
use slint::{ComponentHandle, VecModel};
use std::{cell::RefCell, rc::Rc};
const PAGE: usize = 64;
const MEMBERS: usize = 128;
#[derive(Clone, PartialEq)]
struct Fields {
    name: String,
    color: [u8; 3],
    parent: Option<u32>,
    members: Vec<ObjectId>,
}
#[derive(Default)]
struct Form {
    selected: Option<u32>,
    token: Option<(u64, u64)>,
    generation: Option<u64>,
    seen: Option<(u64, u64, u64)>,
    offset: usize,
    member_offset: usize,
    parents: Vec<Option<u32>>,
    members: Vec<ObjectId>,
    reviewed: Option<Fields>,
}
pub struct Controller {
    pub window: LayersWindow,
    form: Rc<RefCell<Form>>,
}
fn text(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}
fn fields(window: &LayersWindow, form: &Form) -> Fields {
    Fields {
        name: window.get_layer_name().to_string(),
        color: [
            window.get_red() as u8,
            window.get_green() as u8,
            window.get_blue() as u8,
        ],
        parent: form
            .parents
            .get(window.get_parent_index().max(0) as usize)
            .copied()
            .flatten(),
        members: form.members.clone(),
    }
}
fn refresh(window: &LayersWindow, session: &Session, state: &RefCell<Form>, force: bool) {
    let mut form = state.borrow_mut();
    let stamp = (
        session.document_generation(),
        session.document().revision(),
        session.view_serial(),
    );
    if !force && form.seen == Some(stamp) {
        return;
    }
    if form.generation != Some(stamp.0) {
        form.selected = None;
        form.token = None;
        form.offset = 0;
        form.member_offset = 0;
        form.generation = Some(stamp.0);
    }
    form.seen = Some(stamp);
    let report = session.layer_report(
        &window.get_query(),
        form.offset,
        PAGE,
        form.selected,
        form.member_offset,
    );
    if let Some(error) = report["error"].as_str() {
        window.set_message(error.into());
        window.set_ready(false);
        window.set_layers(Default::default());
        return;
    }
    let rows = report["layers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|record| LayerRow {
            id: record["id"].to_string().into(),
            label: format!(
                "{}{}",
                "  ".repeat(record["depth"].as_u64().unwrap_or(0).min(8) as usize),
                text(&record["name"])
            )
            .into(),
            detail: format!(
                "ID {} · Members: {} · Objects with children: {}",
                record["id"], record["direct_members"], record["objects_with_descendants"]
            )
            .into(),
            color: slint::Color::from_rgb_u8(
                record["color"][0].as_u64().unwrap() as u8,
                record["color"][1].as_u64().unwrap() as u8,
                record["color"][2].as_u64().unwrap() as u8,
            ),
        })
        .collect::<Vec<_>>();
    window.set_layers(Rc::new(VecModel::from(rows)).into());
    window.set_previous(form.offset > 0);
    window.set_next(!report["next_offset"].is_null());
    window.set_page_label(format!("{} layers", report["total"]).into());
    window.set_selected(
        form.selected
            .map(|id| id.to_string())
            .unwrap_or_default()
            .into(),
    );
    let draft = &report["draft"];
    let current = draft["token"].as_u64().zip(draft["version"].as_u64());
    if form.token.is_some() && form.token != current {
        form.token = None;
        form.reviewed = None;
        window.set_message("Layer draft changed elsewhere. Reopen it to continue.".into());
    }
    let editing = form.token.is_some();
    window.set_editing(editing);
    window.set_deleting(editing && draft["delete"] == true);
    if editing {
        window.set_summary(if draft["id"].is_null() {
            "New layer".into()
        } else {
            format!("{} · layer {}", text(&draft["name"]), draft["id"]).into()
        });
        let members = form
            .members
            .iter()
            .skip(form.member_offset)
            .take(MEMBERS)
            .map(|id| crate::mcp::id_string(*id).into())
            .collect::<Vec<_>>();
        window.set_members(Rc::new(VecModel::from(members)).into());
        window.set_member_label(format!("Direct members: {}", form.members.len()).into());
        window.set_member_previous(form.member_offset > 0);
        window.set_member_next(form.member_offset.saturating_add(MEMBERS) < form.members.len());
        window.set_ready(
            draft["valid"] == true && form.reviewed.as_ref() == Some(&fields(window, &form)),
        );
        if let Some(error) = draft["error"].as_str() {
            window.set_message(error.into());
        }
    } else {
        let selected = &report["selected"];
        if selected.is_null() {
            form.selected = None;
            window.set_selected("".into());
            window.set_summary("Choose a layer, or create one from the current selection.".into());
            window.set_members(Default::default());
            window.set_member_label("Direct members".into());
        } else {
            window.set_summary(
                format!("{} · layer {}", text(&selected["name"]), selected["id"]).into(),
            );
            window.set_members(
                Rc::new(VecModel::from(
                    selected["members"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|member| text(&member["id"]).into())
                        .collect::<Vec<_>>(),
                ))
                .into(),
            );
            window.set_member_label(format!("Direct members: {}", selected["member_total"]).into());
        }
        window.set_member_previous(form.member_offset > 0);
        window.set_member_next(!selected["next_member_offset"].is_null());
        window.set_ready(false);
    }
}
fn begin(window: &LayersWindow, session: &mut Session, state: &RefCell<Form>, action: i32) {
    let id = if action == 0 {
        None
    } else {
        state.borrow().selected
    };
    if action != 0 && id.is_none() {
        return;
    }
    match session.begin_layer_edit(id, action == 2, &session.layer_stamp()) {
        Err(error) => window.set_message(error.into()),
        Ok(report) => {
            let draft = &report["draft"];
            let mut form = state.borrow_mut();
            form.token = draft["token"].as_u64().zip(draft["version"].as_u64());
            form.member_offset = 0;
            form.members = draft["members"]
                .as_array()
                .unwrap()
                .iter()
                .map(|id| crate::mcp::parse_id(text(id)).unwrap())
                .collect();
            let mut parents = vec!["Root (no parent)".into()];
            form.parents = vec![None];
            for parent in report["parent_choices"].as_array().unwrap() {
                form.parents
                    .push(Some(parent["id"].as_u64().unwrap() as u32));
                parents.push(format!("{} · {}", text(&parent["name"]), parent["id"]).into());
            }
            window.set_parents(Rc::new(VecModel::from(parents)).into());
            let parent = draft["parent"].as_u64().map(|id| id as u32);
            window.set_parent_index(
                form.parents
                    .iter()
                    .position(|id| *id == parent)
                    .unwrap_or(0) as i32,
            );
            window.set_layer_name(text(&draft["name"]).into());
            window.set_red(draft["color"][0].as_i64().unwrap() as i32);
            window.set_green(draft["color"][1].as_i64().unwrap() as i32);
            window.set_blue(draft["color"][2].as_i64().unwrap() as i32);
            form.reviewed = Some(fields(window, &form));
            drop(form);
            window.set_message(
                "Changes stay in this draft until Apply. Cancel discards them.".into(),
            );
            refresh(window, session, state, true);
        }
    }
}
fn review(window: &LayersWindow, session: &mut Session, state: &RefCell<Form>) {
    let (token, values) = {
        let form = state.borrow();
        (form.token, fields(window, &form))
    };
    let Some((token, version)) = token else {
        return;
    };
    match session.update_layer_edit(
        token,
        version,
        values.name.clone(),
        values.color,
        values.parent,
        values.members.clone(),
    ) {
        Ok(report) => {
            let mut form = state.borrow_mut();
            form.token = report["draft"]["token"]
                .as_u64()
                .zip(report["draft"]["version"].as_u64());
            form.reviewed = Some(values);
            window.set_message(if report["draft"]["valid"] == true {
                "Ready to apply as one Undo step.".into()
            } else {
                text(&report["draft"]["error"]).into()
            });
            drop(form);
            refresh(window, session, state, true);
        }
        Err(error) => {
            window.set_ready(false);
            window.set_message(error.into());
        }
    }
}
fn cancel(window: &LayersWindow, session: &mut Session, state: &RefCell<Form>) {
    if let Some((token, version)) = state.borrow_mut().token.take()
        && let Err(error) = session.cancel_layer_edit(token, version)
    {
        window.set_message(error.into());
    }
    state.borrow_mut().reviewed = None;
    refresh(window, session, state, true);
}
impl Controller {
    pub fn new(shared: &SharedSession) -> Result<Self, slint::PlatformError> {
        let window = LayersWindow::new()?;
        let form = Rc::new(RefCell::new(Form::default()));
        window.on_choose({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move |id| {
                let mut state = form.borrow_mut();
                if state.token.is_some() {
                    return;
                }
                state.selected = id.parse().ok();
                state.member_offset = 0;
                drop(state);
                refresh(&window.unwrap(), &lock(&shared), &form, true);
            }
        });
        window.on_search({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || {
                form.borrow_mut().offset = 0;
                refresh(&window.unwrap(), &lock(&shared), &form, true);
            }
        });
        window.on_begin({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move |action| begin(&window.unwrap(), &mut lock(&shared), &form, action)
        });
        window.on_review({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || review(&window.unwrap(), &mut lock(&shared), &form)
        });
        window.on_apply({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || {
                let window = window.unwrap();
                let identity = {
                    let state = form.borrow();
                    if state.reviewed.as_ref() != Some(&fields(&window, &state)) {
                        window.set_ready(false);
                        window.set_message("Review the changed fields before applying.".into());
                        return;
                    }
                    state.token
                };
                if let Some((token, version)) = identity {
                    let mut session = lock(&shared);
                    match session.apply_layer_edit(Actor::User, token, version) {
                        Ok(report) => {
                            let mut state = form.borrow_mut();
                            state.token = None;
                            state.reviewed = None;
                            state.selected = if report["deleted"] == true {
                                None
                            } else {
                                report["layer"].as_u64().map(|id| id as u32)
                            };
                            state.member_offset = 0;
                            drop(state);
                            window.set_message("Layer changes applied. Undo restores them.".into());
                        }
                        Err(error) => window.set_message(error.into()),
                    }
                    refresh(&window, &session, &form, true);
                }
            }
        });
        window.on_cancel({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || cancel(&window.unwrap(), &mut lock(&shared), &form)
        });
        window.on_close_dialog({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || {
                let window = window.unwrap();
                cancel(&window, &mut lock(&shared), &form);
                let _ = window.hide();
            }
        });
        window.window().on_close_requested({
            let window = window.as_weak();
            move || {
                window.unwrap().invoke_close_dialog();
                slint::CloseRequestResponse::HideWindow
            }
        });
        window.on_selection({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move |action| {
                let selection = lock(&shared).layer_selection();
                let mut state = form.borrow_mut();
                if state.token.is_none() {
                    return;
                }
                let mut members = state
                    .members
                    .iter()
                    .copied()
                    .collect::<std::collections::BTreeSet<_>>();
                match action {
                    0 => members = selection.into_iter().collect(),
                    1 => members.extend(selection),
                    _ => members.retain(|id| !selection.contains(id)),
                };
                state.members = members.into_iter().collect();
                state.member_offset = 0;
                state.reviewed = None;
                drop(state);
                window.unwrap().set_ready(false);
                refresh(&window.unwrap(), &lock(&shared), &form, true);
            }
        });
        window.on_remove({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move |id| {
                if let Ok(id) = crate::mcp::parse_id(&id) {
                    let mut state = form.borrow_mut();
                    if state.token.is_none() {
                        return;
                    }
                    state.members.retain(|member| *member != id);
                    state.reviewed = None;
                    drop(state);
                    refresh(&window.unwrap(), &lock(&shared), &form, true);
                }
            }
        });
        let page_callback = |member| {
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move |direction: i32| {
                let mut state = form.borrow_mut();
                let (offset, step) = if member {
                    (&mut state.member_offset, MEMBERS)
                } else {
                    (&mut state.offset, PAGE)
                };
                *offset = if direction < 0 {
                    offset.saturating_sub(step)
                } else {
                    offset.saturating_add(step)
                };
                drop(state);
                refresh(&window.unwrap(), &lock(&shared), &form, true);
            }
        };
        window.on_page(page_callback(false));
        window.on_member_page(page_callback(true));
        window.on_select_members({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move || {
                let window = window.unwrap();
                let Some(id) = form.borrow().selected else {
                    return;
                };
                let mode = match window.get_selection_mode() {
                    1 => SelectMode::Add,
                    2 => SelectMode::Remove,
                    3 => SelectMode::Toggle,
                    _ => SelectMode::Replace,
                };
                let mut session = lock(&shared);
                let stamp = session.layer_stamp();
                if let Err(error) = session.select_layer_members(
                    id,
                    window.get_recursive(),
                    window.get_include_hidden(),
                    mode,
                    &stamp,
                ) {
                    window.set_message(error.into());
                }
                refresh(&window, &session, &form, true);
            }
        });
        window.on_visibility({
            let (window, shared, form) = (window.as_weak(), shared.clone(), form.clone());
            move |action| {
                let window = window.unwrap();
                let Some(id) = form.borrow().selected else {
                    return;
                };
                let mut session = lock(&shared);
                let stamp = session.layer_stamp();
                if let Err(error) = session.set_layer_visibility(
                    Actor::User,
                    id,
                    window.get_recursive(),
                    &action,
                    &stamp,
                ) {
                    window.set_message(error.into());
                }
                refresh(&window, &session, &form, true);
            }
        });
        Ok(Self { window, form })
    }
    pub fn refresh(&self, session: &Session) {
        refresh(&self.window, session, &self.form, false);
    }
    pub fn show(&self, session: &Session) -> Result<(), slint::PlatformError> {
        refresh(&self.window, session, &self.form, true);
        self.window.show()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    #[test]
    fn layer_controls_review_fields_cancel_and_preserve_newer_agent_drafts() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let (session, id, _) = crate::session::clipboard::tests::fixture();
        let shared = Arc::new(Mutex::new(session));
        let controller = Controller::new(&shared).unwrap();
        controller.show(&lock(&shared)).unwrap();
        controller.window.invoke_begin(0);
        controller.window.set_layer_name("Architecture".into());
        controller.window.invoke_apply();
        assert!(
            lock(&shared)
                .document()
                .vmf()
                .visgroups
                .as_ref()
                .unwrap()
                .is_empty()
        );
        controller.window.invoke_review();
        assert!(controller.window.get_ready());
        controller.window.invoke_apply();
        let root = controller.form.borrow().selected.unwrap();
        assert_eq!(
            map_core::layers::Graph::read(lock(&shared).document())
                .unwrap()
                .members(root, false, true)
                .unwrap(),
            vec![id]
        );
        controller.window.invoke_begin(0);
        controller.window.set_layer_name("Lighting".into());
        controller.window.set_parent_index(1);
        controller.window.set_red(255);
        controller.window.set_green(190);
        controller.window.set_blue(70);
        controller.window.invoke_review();
        controller.window.invoke_apply();
        let child = controller.form.borrow().selected.unwrap();
        assert_eq!(
            map_core::layers::Graph::read(lock(&shared).document())
                .unwrap()
                .records[&child]
                .parent,
            Some(root)
        );
        controller.window.invoke_begin(1);
        controller
            .window
            .set_layer_name("Lights and helpers".into());
        controller.window.invoke_review();
        crate::tests::save_ui_snapshot(controller.window.window(), "layers");
        let identity = controller.form.borrow().token.unwrap();
        lock(&shared)
            .update_layer_edit(
                identity.0,
                identity.1,
                "Agent's newer edit".into(),
                [40, 160, 220],
                Some(root),
                vec![id],
            )
            .unwrap();
        controller.window.invoke_close_dialog();
        assert_eq!(
            lock(&shared).layer_report("", 0, 64, Some(child), 0)["draft"]["name"],
            "Agent's newer edit"
        );
        lock(&shared)
            .cancel_layer_edit(identity.0, identity.1 + 1)
            .unwrap();
        controller.show(&lock(&shared)).unwrap();
        controller.window.invoke_begin(2);
        controller.window.invoke_cancel();
        assert_eq!(
            map_core::layers::Graph::read(lock(&shared).document())
                .unwrap()
                .records
                .len(),
            2
        );
        controller.window.invoke_begin(2);
        controller.window.invoke_apply();
        assert_eq!(
            map_core::layers::Graph::read(lock(&shared).document())
                .unwrap()
                .records
                .len(),
            1
        );
        lock(&shared).undo(Actor::Agent).unwrap();
        assert_eq!(
            map_core::layers::Graph::read(lock(&shared).document())
                .unwrap()
                .records
                .len(),
            2
        );
    }
}
