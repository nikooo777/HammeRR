use std::{cell::RefCell, rc::Rc};

use map_core::groups::Member;
use serde_json::Value;
use slint::{ComponentHandle, VecModel};

use crate::runtime::{DocumentAccess, DocumentContinuation, SharedSession, lock};
use crate::session::{Actor, SelectMode, Session, groups::parse_member};
use crate::{ObjectGroupMemberRow, ObjectGroupRow, ObjectGroupsWindow};

const PAGE_SIZE: usize = 100;
type Stamp = (u64, u64, u64, u64);

#[derive(Default)]
struct Form {
    selected: Option<u32>,
    token: Option<u64>,
    generation: Option<u64>,
    seen: Option<Stamp>,
    offset: usize,
    member_offset: usize,
}

pub struct Controller {
    pub window: ObjectGroupsWindow,
    form: Rc<RefCell<Form>>,
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or("")
}

fn mode(window: &ObjectGroupsWindow) -> SelectMode {
    match window.get_selection_mode() {
        1 => SelectMode::Add,
        2 => SelectMode::Remove,
        3 => SelectMode::Toggle,
        _ => SelectMode::Replace,
    }
}

fn members(window: &ObjectGroupsWindow, values: &Value) {
    let rows = values
        .as_array()
        .into_iter()
        .flatten()
        .map(|member| ObjectGroupMemberRow {
            id: text(&member["id"]).into(),
            label: text(&member["label"]).into(),
            detail: format!(
                "{}{}{}",
                text(&member["id"]),
                if member["kind"] == "group" {
                    format!(" · {} objects", member["objects"])
                } else {
                    String::new()
                },
                if member["hidden"] == true {
                    " · hidden"
                } else {
                    ""
                }
            )
            .into(),
        })
        .collect::<Vec<_>>();
    window.set_members(Rc::new(VecModel::from(rows)).into());
}

fn refresh(window: &ObjectGroupsWindow, session: &Session, state: &RefCell<Form>, force: bool) {
    let mut form = state.borrow_mut();
    let stamp = (
        session.document_generation(),
        session.document().revision(),
        session.scene_serial(),
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
    window.set_bypass(session.group_selection_bypass());
    let report = match session.object_groups(None, form.offset, PAGE_SIZE) {
        Ok(report) => report,
        Err(error) => {
            window.set_available(false);
            window.set_ready(false);
            window.set_groups(Default::default());
            window.set_members(Default::default());
            window.set_message(error.into());
            return;
        }
    };
    window.set_available(true);
    if form.selected.is_some_and(|id| {
        !session
            .document()
            .vmf()
            .world
            .groups
            .iter()
            .any(|group| group.id == id)
    }) {
        form.selected = None;
        form.member_offset = 0;
    }
    let rows = report["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|group| {
            let name = text(&group["name"]);
            ObjectGroupRow {
                id: group["id"].to_string().into(),
                label: format!(
                    "{}{} · {}",
                    "  ".repeat(group["depth"].as_u64().unwrap_or(0).min(8) as usize),
                    if name.is_empty() {
                        "Unnamed group"
                    } else {
                        name
                    },
                    group["id"]
                )
                .into(),
                detail: format!(
                    "{} · {} direct members · {} objects{}",
                    if group["parent"].is_null() {
                        "Root".to_string()
                    } else {
                        format!("Parent {}", group["parent"])
                    },
                    group["direct_members"],
                    group["objects"],
                    if group["owned_brush_links"]
                        .as_array()
                        .is_some_and(|links| !links.is_empty())
                    {
                        " · owned-brush links preserved"
                    } else {
                        ""
                    }
                )
                .into(),
            }
        })
        .collect::<Vec<_>>();
    window.set_groups(Rc::new(VecModel::from(rows)).into());
    window.set_selected(
        form.selected
            .map_or_else(String::new, |id| id.to_string())
            .into(),
    );
    window.set_previous(form.offset > 0);
    window.set_next(!report["next_offset"].is_null());
    window.set_page_label(format!("{} groups", report["total"]).into());
    let draft = &report["draft"];
    if form.token.is_some() && draft["token"].as_u64() != form.token {
        form.token = None;
        window.set_message("Group draft was closed or replaced. Choose New from selection or Edit group to continue.".into());
    }
    let editing = form.token.is_some();
    window.set_editing(editing);
    window.set_ungrouping(editing && draft["ungroup"] == true);
    window.set_ready(editing && draft["valid"] == true);
    window.set_member_previous(false);
    window.set_member_next(false);
    if editing {
        window.set_group_name(text(&draft["name"]).into());
        window.set_parent_label(if draft["parent"].is_null() {
            "Root".into()
        } else {
            format!("Group {}", draft["parent"]).into()
        });
        window.set_summary(
            if draft["ungroup"] == true {
                format!("Ungroup {}", draft["group"])
            } else if draft["group"].is_null() {
                "New group draft".into()
            } else {
                format!("Editing group {}", draft["group"])
            }
            .into(),
        );
        members(window, &draft["members"]);
        if let Some(error) = draft["error"].as_str() {
            window.set_message(error.into());
        } else {
            window.set_message(format!("{} object memberships will change. Apply records one Undo step; Cancel discards this draft.", draft["plan"]["changed_objects"]).into());
        }
    } else if let Some(id) = form.selected {
        match session.object_groups(Some(id), form.member_offset, PAGE_SIZE) {
            Ok(detail) => {
                let detail = &detail["detail"];
                let name = text(&detail["group"]["name"]);
                window.set_summary(
                    format!(
                        "{} · group {}",
                        if name.is_empty() {
                            "Unnamed group"
                        } else {
                            name
                        },
                        id
                    )
                    .into(),
                );
                members(window, &detail["members"]);
                window.set_member_previous(form.member_offset > 0);
                window.set_member_next(!detail["next_offset"].is_null());
                window.set_member_page_label(format!("{} direct members", detail["total"]).into());
            }
            Err(error) => window.set_message(error.into()),
        }
    } else {
        window.set_summary(
            format!(
                "{} selected objects · choose a group to inspect",
                session.selected_ids().len()
            )
            .into(),
        );
        window.set_members(Default::default());
        window.set_member_page_label("".into());
    }
}

fn begin(
    window: &ObjectGroupsWindow,
    session: &mut Session,
    state: &RefCell<Form>,
    new: bool,
    ungroup: bool,
) {
    let selected = state.borrow().selected;
    if !new && selected.is_none() {
        window.set_message("Choose a group first.".into());
        return;
    }
    match session.begin_group_edit(if new { None } else { selected }, ungroup) {
        Ok(report) => {
            state.borrow_mut().token = report["token"].as_u64();
            refresh(window, session, state, true);
        }
        Err(error) => window.set_message(error.into()),
    }
}

fn cancel(window: &ObjectGroupsWindow, session: &mut Session, state: &RefCell<Form>) {
    let token = state.borrow().token;
    if let Some(token) = token {
        if let Err(error) = session.cancel_group_edit(token) {
            window.set_message(error.into());
        }
        state.borrow_mut().token = None;
    }
    refresh(window, session, state, true);
}

#[derive(Clone, Copy)]
enum Action {
    Name,
    AddSelection,
    AddGroup,
    Parent,
    Root,
    Apply,
}

fn edit(window: &ObjectGroupsWindow, session: &mut Session, state: &RefCell<Form>, action: Action) {
    let (token, selected) = {
        let form = state.borrow();
        (form.token, form.selected)
    };
    let Some(token) = token else {
        window.set_message("Create or edit a group first.".into());
        return;
    };
    let result = match action {
        Action::Name => {
            session.update_group_edit(token, Some(window.get_group_name().to_string()), None, None)
        }
        Action::AddSelection => session.add_selection_to_group_edit(token),
        Action::AddGroup => selected
            .ok_or_else(|| "Choose a group on the left".into())
            .and_then(|id| session.add_group_edit_members(token, &[Member::Group(id)])),
        Action::Parent => selected
            .ok_or_else(|| "Choose a parent group on the left".into())
            .and_then(|id| session.update_group_edit(token, None, Some(Some(id)), None)),
        Action::Root => session.update_group_edit(token, None, Some(None), None),
        Action::Apply => session.apply_group_edit(Actor::User, token),
    };
    match result {
        Ok(report) => {
            if matches!(action, Action::Apply) {
                let mut form = state.borrow_mut();
                form.token = None;
                form.selected = if report["ungrouped"] == true {
                    None
                } else {
                    report["group"].as_u64().map(|id| id as u32)
                };
                form.member_offset = 0;
                window.set_message(
                    "Group applied. Save the VMF to keep it; Undo restores the change.".into(),
                );
            }
            refresh(window, session, state, true);
        }
        Err(error) => window.set_message(error.into()),
    }
}

fn callback(
    window: &slint::Weak<ObjectGroupsWindow>,
    target: &DocumentContinuation,
    body: impl FnOnce(ObjectGroupsWindow, &SharedSession),
) {
    let Some(window) = window.upgrade() else {
        return;
    };
    let result = target.upgrade().and_then(|access| {
        access.with_shared(|shared| {
            body(window.clone_strong(), shared);
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
        let target = access.downgrade();
        let window = ObjectGroupsWindow::new()?;
        let form = Rc::new(RefCell::new(Form::default()));
        window.on_choose({
            let (window, target, form) = (window.as_weak(), target.clone(), form.clone());
            move |id| {
                callback(&window, &target, |window, shared| {
                    let mut state = form.borrow_mut();
                    state.selected = id.parse().ok();
                    state.member_offset = 0;
                    drop(state);
                    refresh(&window, &lock(shared), &form, true);
                })
            }
        });
        let begin_callback = |new, ungroup| {
            let (window, target, form) = (window.as_weak(), target.clone(), form.clone());
            move || {
                callback(&window, &target, |window, shared| {
                    begin(&window, &mut lock(shared), &form, new, ungroup)
                })
            }
        };
        window.on_begin_new(begin_callback(true, false));
        window.on_begin_edit(begin_callback(false, false));
        window.on_begin_ungroup(begin_callback(false, true));
        let edit_callback = |action| {
            let (window, target, form) = (window.as_weak(), target.clone(), form.clone());
            move || {
                callback(&window, &target, |window, shared| {
                    edit(&window, &mut lock(shared), &form, action)
                })
            }
        };
        window.on_name_changed(edit_callback(Action::Name));
        window.on_add_selection(edit_callback(Action::AddSelection));
        window.on_add_chosen_group(edit_callback(Action::AddGroup));
        window.on_chosen_parent(edit_callback(Action::Parent));
        window.on_root_parent(edit_callback(Action::Root));
        window.on_apply(edit_callback(Action::Apply));
        window.on_cancel({
            let (window, target, form) = (window.as_weak(), target.clone(), form.clone());
            move || {
                callback(&window, &target, |window, shared| {
                    cancel(&window, &mut lock(shared), &form)
                })
            }
        });
        window.on_close_dialog({
            let (window, target, form) = (window.as_weak(), target.clone(), form.clone());
            move || {
                callback(&window, &target, |window, shared| {
                    cancel(&window, &mut lock(shared), &form);
                });
                if let Some(window) = window.upgrade() {
                    let _ = window.hide();
                }
            }
        });
        window.window().on_close_requested({
            let window = window.as_weak();
            move || {
                window.unwrap().invoke_close_dialog();
                slint::CloseRequestResponse::HideWindow
            }
        });
        window.on_remove_member({
            let (window, target, form) = (window.as_weak(), target.clone(), form.clone());
            move |id| {
                callback(&window, &target, |window, shared| {
                    let result = (|| {
                        let token = form.borrow().token.ok_or("Create or edit a group first")?;
                        lock(shared).remove_group_edit_member(token, parse_member(&id)?)
                    })();
                    match result {
                        Ok(_) => refresh(&window, &lock(shared), &form, true),
                        Err(error) => window.set_message(error.into()),
                    }
                })
            }
        });
        window.on_select_member({
            let (window, target, form) = (window.as_weak(), target.clone(), form.clone());
            move |id| {
                callback(&window, &target, |window, shared| {
                    let result = parse_member(&id).and_then(|member| {
                        lock(shared).select_group_member(
                            member,
                            mode(&window),
                            window.get_include_hidden(),
                        )
                    });
                    if let Err(error) = result {
                        window.set_message(error.into());
                    }
                    refresh(&window, &lock(shared), &form, true);
                })
            }
        });
        window.on_select_group({
            let (window, target, form) = (window.as_weak(), target.clone(), form.clone());
            move || {
                callback(&window, &target, |window, shared| {
                    let result = form
                        .borrow()
                        .selected
                        .ok_or_else(|| "Choose a group first".to_string())
                        .and_then(|id| {
                            lock(shared).select_object_group(
                                id,
                                mode(&window),
                                window.get_include_hidden(),
                            )
                        });
                    if let Err(error) = result {
                        window.set_message(error.into());
                    }
                    refresh(&window, &lock(shared), &form, true);
                })
            }
        });
        window.on_bypass_changed({
            let (window, target) = (window.as_weak(), target.clone());
            move || {
                callback(&window, &target, |window, shared| {
                    lock(shared).set_group_selection_bypass(Actor::User, window.get_bypass())
                })
            }
        });
        let page_callback = |member_page| {
            let (window, target, form) = (window.as_weak(), target.clone(), form.clone());
            move |direction: i32| {
                callback(&window, &target, |window, shared| {
                    let mut state = form.borrow_mut();
                    let offset = if member_page {
                        &mut state.member_offset
                    } else {
                        &mut state.offset
                    };
                    *offset = if direction < 0 {
                        offset.saturating_sub(PAGE_SIZE)
                    } else {
                        offset.saturating_add(PAGE_SIZE)
                    };
                    drop(state);
                    refresh(&window, &lock(shared), &form, true);
                })
            }
        };
        window.on_page(page_callback(false));
        window.on_member_page(page_callback(true));
        Ok(Self { window, form })
    }

    pub fn refresh(&self, session: &Session) {
        refresh(&self.window, session, &self.form, false);
    }

    pub fn show(&self, session: &Session) -> Result<(), slint::PlatformError> {
        self.window.set_message(
            "Choose a group to inspect, or select objects and choose New from selection.".into(),
        );
        refresh(&self.window, session, &self.form, true);
        self.window.show()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::DVec3;
    use map_core::ObjectId;
    use slint::Model;
    use std::{
        collections::BTreeMap,
        sync::{Arc, Mutex},
    };

    fn software() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
    }

    #[test]
    fn managed_group_apply_rejects_after_real_drain_and_keeps_peer_live() {
        software();
        let directory = tempfile::tempdir().unwrap();
        let (mut session, _, _) = crate::test_assets::clipboard();
        session.configure_recovery(Ok(directory.path().join("recovery")));
        let shared = Arc::new(Mutex::new(session));
        let document = crate::test_assets::document(shared.clone());
        let access = DocumentAccess::for_document(document.clone());
        let controller = Controller::new_with_access(&access).unwrap();
        controller.show(&lock(&shared)).unwrap();
        let initial_history = lock(&shared).document().undo_labels().count();
        controller.window.invoke_begin_new();
        controller.window.set_group_name("Committed group".into());
        controller.window.invoke_name_changed();
        controller.window.invoke_apply();
        assert_eq!(lock(&shared).document().vmf().world.groups.len(), 1);
        assert_eq!(
            lock(&shared).document().undo_labels().count(),
            initial_history + 1
        );
        controller.window.invoke_begin_edit();
        controller.window.set_group_name("Late group edit".into());
        controller.window.invoke_name_changed();
        assert!(controller.window.get_ready());

        let (mut peer_state, _, _) = crate::test_assets::clipboard();
        peer_state.configure_recovery(Ok(directory.path().join("peer-recovery")));
        let peer = Arc::new(Mutex::new(peer_state));
        let peer_document = crate::test_assets::document(peer.clone());
        let peer_access = DocumentAccess::for_document(peer_document.clone());
        let peer_controller = Controller::new_with_access(&peer_access).unwrap();
        peer_controller.show(&lock(&peer)).unwrap();
        let peer_before = lock(&peer).document().vmf().clone();
        let peer_history = lock(&peer).history_snapshot();
        document.shutdown().unwrap();
        let source = lock(&shared).document().vmf().clone();
        let history = lock(&shared).history_snapshot();
        let draft = lock(&shared).group_edit_report();
        controller.window.invoke_apply();
        assert_eq!(controller.window.get_message(), "the document is closed");
        controller.window.invoke_close_dialog();
        assert!(!controller.window.window().is_visible());
        assert_eq!(lock(&shared).document().vmf(), &source);
        assert_eq!(lock(&shared).history_snapshot(), history);
        assert_eq!(lock(&shared).group_edit_report(), draft);
        assert_eq!(lock(&peer).document().vmf(), &peer_before);
        assert_eq!(lock(&peer).history_snapshot(), peer_history);
        peer_controller.window.invoke_begin_new();
        peer_controller
            .window
            .set_group_name("Independent peer".into());
        peer_controller.window.invoke_name_changed();
        peer_controller.window.invoke_apply();
        assert_eq!(lock(&peer).document().vmf().world.groups.len(), 1);
        peer_document.shutdown().unwrap();
    }

    #[test]
    fn organizing_dialog_registrations_retain_neither_document_nor_session() {
        software();
        let directory = tempfile::tempdir().unwrap();
        let (mut session, _, _) = crate::test_assets::clipboard();
        session.configure_recovery(Ok(directory.path().join("recovery")));
        let shared = Arc::new(Mutex::new(session));
        let weak_session = Arc::downgrade(&shared);
        let document = crate::test_assets::document(shared.clone());
        let access = DocumentAccess::for_document(document.clone());
        let continuation = access.downgrade();
        let groups = Controller::new_with_access(&access).unwrap();
        let layers = crate::layers_ui::Controller::new_with_access(&access).unwrap();
        let clipboard = crate::clipboard_ui::Controller::new_with_access(&access).unwrap();
        drop(shared);
        drop(access);
        drop(document);
        assert!(weak_session.upgrade().is_none());
        assert!(continuation.upgrade().is_err());
        groups.window.invoke_begin_new();
        layers.window.invoke_begin(0);
        clipboard.window.invoke_preview();
        assert_eq!(groups.window.get_message(), "the document is closed");
        assert_eq!(layers.window.get_message(), "the document is closed");
        assert_eq!(clipboard.window.get_message(), "the document is closed");
    }

    #[test]
    fn group_window_create_nested_membership_cancel_and_ungroup_share_session() {
        software();
        let mut session = Session::new(Err("no assets".into()));
        for x in [0.0, 64.0, 128.0] {
            session
                .create_entity(
                    Actor::User,
                    "info_target",
                    Some(DVec3::new(x, 0.0, 0.0)),
                    BTreeMap::new(),
                )
                .unwrap();
        }
        session
            .select(
                &[ObjectId::Entity(2), ObjectId::Entity(3)],
                SelectMode::Replace,
            )
            .unwrap();
        let shared = Arc::new(Mutex::new(session));
        let controller = Controller::new(&shared).unwrap();
        controller.show(&lock(&shared)).unwrap();
        let source = lock(&shared).document().vmf().clone();
        let history = lock(&shared).document().undo_labels().count();
        controller.window.invoke_begin_new();
        controller.window.set_group_name("Courtyard props".into());
        controller.window.invoke_name_changed();
        assert!(controller.window.get_ready());
        assert_eq!(controller.window.get_members().row_count(), 2);
        controller.window.invoke_cancel();
        assert_eq!(lock(&shared).document().vmf(), &source);
        assert_eq!(lock(&shared).document().undo_labels().count(), history);
        controller.window.invoke_begin_new();
        controller.window.set_group_name("Courtyard props".into());
        controller.window.invoke_name_changed();
        controller.window.invoke_apply();
        let inner = controller.form.borrow().selected.unwrap();
        assert_eq!(controller.window.get_groups().row_count(), 1);
        controller.window.invoke_select_group();
        assert_eq!(lock(&shared).selected_ids().len(), 2);
        controller.window.invoke_begin_new();
        assert_eq!(controller.window.get_members().row_count(), 1);
        controller
            .window
            .set_group_name("Courtyard assembly".into());
        controller.window.invoke_name_changed();
        controller.window.invoke_apply();
        let outer = controller.form.borrow().selected.unwrap();
        controller.window.invoke_choose(inner.to_string().into());
        controller.window.invoke_begin_edit();
        controller.window.invoke_remove_member("entity:3".into());
        controller.window.invoke_apply();
        controller.window.invoke_select_group();
        assert_eq!(lock(&shared).selected_ids(), vec![ObjectId::Entity(2)]);
        controller.window.invoke_choose(outer.to_string().into());
        controller.window.invoke_select_group();
        assert_eq!(lock(&shared).selected_ids().len(), 2);
        controller.window.invoke_choose(inner.to_string().into());
        controller.window.invoke_begin_ungroup();
        assert!(controller.window.get_ready());
        crate::ui::tests::assert_ui_renders(controller.window.window(), "object-groups");
        controller.window.invoke_apply();
        assert_eq!(lock(&shared).document().vmf().world.groups.len(), 1);
        lock(&shared).undo(Actor::Agent).unwrap();
        assert_eq!(lock(&shared).document().vmf().world.groups.len(), 2);
        controller.refresh(&lock(&shared));
        controller.window.invoke_choose(inner.to_string().into());
        controller.window.invoke_begin_edit();
        let foreign = lock(&shared).begin_group_edit(Some(outer), false).unwrap()["token"]
            .as_u64()
            .unwrap();
        controller.window.invoke_close_dialog();
        assert_eq!(lock(&shared).group_edit_report()["token"], foreign);
    }
}
