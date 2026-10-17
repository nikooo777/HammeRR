use std::cell::RefCell;
use std::rc::Rc;

use slint::{ComponentHandle, VecModel};

use crate::runtime::{DocumentAccess, DocumentContinuation};
#[cfg(test)]
use crate::runtime::{SharedSession, lock};
use crate::session::history::{INITIAL_STATE, PAGE_SIZE, Report};
use crate::session::{Actor, Session};
use crate::{HistoryRow, HistoryWindow};

#[derive(Default)]
struct Form {
    snapshot: String,
    selected: Option<String>,
    offset: usize,
    next: Option<usize>,
    report: Option<Report>,
}

pub struct Controller {
    pub window: HistoryWindow,
    form: Rc<RefCell<Form>>,
}

fn cancel(window: &HistoryWindow, form: &RefCell<Form>) {
    form.borrow_mut().selected = None;
    window.set_selected_state("".into());
    window.set_can_navigate(false);
    window
        .set_detail("Select a transaction to inspect its changes, or choose Initial state.".into());
}

fn update_navigation(window: &HistoryWindow, session: &Session, form: &Form) {
    let Some(target) = &form.selected else {
        window.set_can_navigate(false);
        return;
    };
    match session.plan_history_navigation(&form.snapshot, target) {
        Ok(plan) => {
            window.set_can_navigate(plan.steps > 0);
            window.set_message(
                if plan.steps == 0 {
                    "Already at this state. No changes will be made.".into()
                } else {
                    format!(
                        "Go to selected state will {} {} transaction(s), from state {} to state {}. The map will remain unsaved until you save it.",
                        plan.direction, plan.steps, plan.from, plan.to
                    )
                }
                .into(),
            );
        }
        Err(error) => {
            window.set_can_navigate(false);
            window.set_message(error.into());
        }
    }
}

fn refresh(window: &HistoryWindow, session: &Session, form: &RefCell<Form>) {
    if form.borrow().snapshot == session.history_snapshot() {
        let blocked = session.history_navigation_blocked();
        {
            let mut form = form.borrow_mut();
            if let Some(report) = &mut form.report
                && report.navigation_blocked != blocked
            {
                report.navigation_blocked = blocked.clone();
                window.set_message(
                    blocked
                        .unwrap_or_else(|| "Ready to navigate history.".into())
                        .into(),
                );
            }
        }
        update_navigation(window, session, &form.borrow());
        return;
    }
    let previous = !form.borrow().snapshot.is_empty();
    let offset = form.borrow().offset;
    let report = session
        .inspect_history(offset, PAGE_SIZE)
        .or_else(|_| session.inspect_history(0, PAGE_SIZE));
    match report {
        Ok(report) => {
            cancel(window, form);
            window.set_document_title(report.title.clone().into());
            window.set_position(
                format!(
                    "{} applied · {} available to redo · Current state {} · {}",
                    report.undo_count,
                    report.redo_count,
                    report.current_state,
                    if report.unsaved_changes {
                        "Unsaved changes"
                    } else {
                        "Saved"
                    }
                )
                .into(),
            );
            window.set_rows(
                Rc::new(VecModel::from(
                    report
                        .entries
                        .iter()
                        .map(|entry| HistoryRow {
                            state: entry.state.clone().into(),
                            title: format!(
                                "{}. {}{}",
                                entry.position,
                                entry.label,
                                if entry.label_truncated { "…" } else { "" }
                            )
                            .into(),
                            status: format!(
                                "{} · {} · State {}",
                                entry.actor,
                                if entry.current {
                                    "Current state"
                                } else if entry.applied {
                                    "Applied"
                                } else {
                                    "Available to redo"
                                },
                                entry.state
                            )
                            .into(),
                            summary: change_summary(entry).into(),
                            current: entry.current,
                        })
                        .collect::<Vec<_>>(),
                ))
                .into(),
            );
            window.set_page(
                if report.total == 0 {
                    "0 transactions".into()
                } else {
                    format!(
                        "{}–{} of {}",
                        report.offset + 1,
                        report.offset + report.entries.len(),
                        report.total
                    )
                }
                .into(),
            );
            window.set_has_newer(report.offset > 0);
            window.set_has_older(report.next_offset.is_some());
            window.set_message(
                report
                    .navigation_blocked
                    .clone()
                    .unwrap_or_else(|| {
                        if previous {
                            "History changed. Select the state again.".into()
                        } else {
                            "Inspection leaves the map unchanged.".into()
                        }
                    })
                    .into(),
            );
            let mut form = form.borrow_mut();
            form.snapshot = report.snapshot.clone();
            form.offset = report.offset;
            form.next = report.next_offset;
            form.report = Some(report);
        }
        Err(error) => window.set_message(error.into()),
    }
}

fn change_summary(entry: &crate::session::history::Entry) -> String {
    let mut result = format!(
        "{} added · {} modified · {} removed",
        entry.added, entry.modified, entry.removed
    );
    if !entry.sections.is_empty() {
        result.push_str(" · ");
        result.push_str(&entry.sections.join(", "));
    }
    result
}

fn choose(window: &HistoryWindow, session: &Session, form: &RefCell<Form>, target: String) {
    if form.borrow().snapshot != session.history_snapshot() {
        refresh(window, session, form);
        return;
    }
    let detail = if target == INITIAL_STATE {
        Some("Initial state: the document before its first retained transaction. Opening a map starts a new history.".to_owned())
    } else {
        form.borrow().report.as_ref().and_then(|report| {
            report
                .entries
                .iter()
                .find(|entry| entry.state == target)
                .map(|entry| {
                    let samples = entry
                        .objects
                        .iter()
                        .map(|object| format!("{} {}", object.kind, object.id))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!(
                        "{}{} — {}\n{}{}",
                        entry.label,
                        if entry.label_truncated { "…" } else { "" },
                        change_summary(entry),
                        samples,
                        if entry.objects_total > entry.objects.len() {
                            format!(
                                " ({} of {} objects shown)",
                                entry.objects.len(),
                                entry.objects_total
                            )
                        } else {
                            String::new()
                        }
                    )
                })
        })
    };
    match detail {
        Some(detail) => {
            form.borrow_mut().selected = Some(target.clone());
            window.set_selected_state(target.into());
            window.set_detail(detail.into());
            update_navigation(window, session, &form.borrow());
        }
        None => {
            cancel(window, form);
            window.set_message(
                "This state is not on the displayed page. Refresh history and select it again."
                    .into(),
            );
        }
    }
}

fn navigate(window: &HistoryWindow, session: &mut Session, form: &RefCell<Form>) {
    let (snapshot, target) = {
        let form = form.borrow();
        (form.snapshot.clone(), form.selected.clone())
    };
    let Some(target) = target else {
        window.set_message("Select a history state first.".into());
        return;
    };
    match session.navigate_history(Actor::User, &snapshot, &target) {
        Ok(result) => {
            refresh(window, session, form);
            window.set_message(
                format!(
                    "Navigation complete: {} {} transaction(s); now at state {}. {}",
                    result.plan.direction,
                    result.plan.steps,
                    result.current_state,
                    if result.unsaved_changes {
                        "Unsaved changes."
                    } else {
                        "Saved."
                    }
                )
                .into(),
            );
        }
        Err(error) => {
            refresh(window, session, form);
            window.set_message(error.into());
        }
    }
}

fn page(window: &HistoryWindow, session: &Session, form: &RefCell<Form>, older: bool) {
    let offset = if older {
        form.borrow().next
    } else {
        Some(form.borrow().offset.saturating_sub(PAGE_SIZE))
    };
    if let Some(offset) = offset {
        form.borrow_mut().offset = offset;
        form.borrow_mut().snapshot.clear();
        refresh(window, session, form);
    }
}

fn with_session(
    window: &slint::Weak<HistoryWindow>,
    target: &DocumentContinuation,
    body: impl FnOnce(&HistoryWindow, &mut Session),
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

impl Controller {
    #[cfg(test)]
    pub fn new(shared: &SharedSession) -> Result<Self, slint::PlatformError> {
        Self::new_with_access(&DocumentAccess::unmanaged(shared.clone()))
    }

    pub fn new_with_access(access: &DocumentAccess) -> Result<Self, slint::PlatformError> {
        let window = HistoryWindow::new()?;
        let form = Rc::new(RefCell::new(Form::default()));
        window.on_choose({
            let (window, target, form) = (window.as_weak(), access.downgrade(), form.clone());
            move |state| {
                with_session(&window, &target, |window, session| {
                    choose(window, session, &form, state.to_string());
                })
            }
        });
        window.on_navigate({
            let (window, target, form) = (window.as_weak(), access.downgrade(), form.clone());
            move || {
                with_session(&window, &target, |window, session| {
                    navigate(window, session, &form)
                })
            }
        });
        window.on_newer({
            let (window, target, form) = (window.as_weak(), access.downgrade(), form.clone());
            move || {
                with_session(&window, &target, |window, session| {
                    page(window, session, &form, false)
                })
            }
        });
        window.on_older({
            let (window, target, form) = (window.as_weak(), access.downgrade(), form.clone());
            move || {
                with_session(&window, &target, |window, session| {
                    page(window, session, &form, true)
                })
            }
        });
        window.on_refresh_history({
            let (window, target, form) = (window.as_weak(), access.downgrade(), form.clone());
            move || {
                with_session(&window, &target, |window, session| {
                    form.borrow_mut().snapshot.clear();
                    refresh(window, session, &form);
                })
            }
        });
        window.on_cancel({
            let (window, form) = (window.as_weak(), form.clone());
            move || {
                if let Some(window) = window.upgrade() {
                    cancel(&window, &form);
                    window.set_message("Selection cancelled. Map and history unchanged.".into());
                }
            }
        });
        window.on_close_dialog({
            let (window, form) = (window.as_weak(), form.clone());
            move || {
                if let Some(window) = window.upgrade() {
                    cancel(&window, &form);
                    let _ = window.hide();
                }
            }
        });
        window.window().on_close_requested({
            let (window, form) = (window.as_weak(), form.clone());
            move || {
                if let Some(window) = window.upgrade() {
                    cancel(&window, &form);
                }
                slint::CloseRequestResponse::HideWindow
            }
        });
        Ok(Self { window, form })
    }

    pub fn refresh(&self, session: &Session) {
        refresh(&self.window, session, &self.form);
    }

    pub fn show(&self, session: &Session) -> Result<(), slint::PlatformError> {
        *self.form.borrow_mut() = Form::default();
        self.refresh(session);
        self.window.show()
    }
}

#[cfg(test)]
mod tests;
