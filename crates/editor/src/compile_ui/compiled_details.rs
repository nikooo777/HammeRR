use std::{cell::RefCell, rc::Rc};

use serde_json::Value;
use slint::ComponentHandle;

use super::{CompileWindow, DocumentAccess, Session, SharedSession, lock};
use crate::{CompiledDetailsWindow, session::Actor};

pub(super) struct Form {
    window: CompiledDetailsWindow,
    state: Rc<RefCell<State>>,
}

#[derive(Default)]
struct State {
    token: Option<u64>,
    offset: usize,
    observation: Option<Value>,
}

#[derive(Clone, Copy)]
enum Action {
    Prepare,
    Cancel,
    Clear,
    Show,
    Hide,
    Inspect,
    Page(i32),
}

fn present(window: &CompiledDetailsWindow, state: &mut State, report: Value, observation: Value) {
    state.token = report["token"]
        .as_str()
        .and_then(|token| token.parse().ok());
    state.observation = Some(observation);
    window.set_has_token(state.token.is_some());
    window.set_worker_active(report["worker_active"].as_bool().unwrap_or(false));
    window.set_state(
        format!(
            "{} · requested {} · displayed {} · complete geometry {}",
            report["status"].as_str().unwrap_or("unavailable"),
            report["requested"],
            report["displayed"],
            report["complete"]
        )
        .into(),
    );
    let counts = if let Some(total) = report["page"]["total"].as_u64() {
        format!(
            "{total} final records · models {} · cards {} · cross {} · tri {}",
            report["counts"]["models"],
            report["counts"]["cards"],
            report["counts"]["cross"],
            report["counts"]["tri"]
        )
    } else {
        "Final records have not been prepared or are unavailable".into()
    };
    window.set_build_state(
        format!(
            "At inspection: build {} · build token {} · {counts}",
            report["build"]["status"].as_str().unwrap_or("unavailable"),
            report["build"]["token"].as_str().unwrap_or("none")
        )
        .into(),
    );
    window.set_previous(state.offset > 0);
    window.set_next(report["page"]["next_offset"].is_number());
    window.set_page(
        report["page"]["total"]
            .as_u64()
            .map_or_else(String::new, |total| {
                format!(
                    "Native records {}–{} of {total}",
                    state.offset,
                    state.offset + report["page"]["returned"].as_u64().unwrap_or(0) as usize
                )
            })
            .into(),
    );
    let rows=report["page"]["rows"].as_array().map(|rows|rows.iter().map(|row|format!(
        "#{}  {}  leaf {}  dictionary {}{}\n    origin {}  angles {}  scale {}  orientation {}  sway {}  shape angle/size {}/{}\n    native lighting {}  styles offset/count {}/{}",
        row["ordinal"],row["kind_name"].as_str().unwrap_or("unknown"),row["leaf"],row["dictionary_index"],
        row["model"].as_str().map(|name|format!("  {name}")).unwrap_or_default(),row["origin"],row["angles"],row["scale"],
        row["orientation"],row["sway"],row["shape_angle"],row["shape_size"],row["lighting"],row["light_styles"],row["light_style_count"]
    )).collect::<Vec<_>>().join("\n\n")).unwrap_or_default();
    window.set_rows(rows.into());
    let mut messages = Vec::new();
    if let Some(reason) = report["suppression_reason"].as_str() {
        messages.push(reason.to_owned());
    }
    if let Some(error) = report["error"].as_str() {
        messages.push(error.to_owned());
    }
    if let Some(diagnostics) = report["build"]["diagnostics"].as_array() {
        messages.extend(
            diagnostics
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned),
        );
    }
    if let Some(diagnostics) = report["preview_assets"]["diagnostics"].as_array() {
        messages.extend(
            diagnostics
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_owned),
        );
    }
    window.set_message(messages.join("\n").into());
}

fn perform(
    window: &CompiledDetailsWindow,
    shared: &SharedSession,
    state: &Rc<RefCell<State>>,
    action: Action,
) -> Result<(), String> {
    if matches!(action, Action::Prepare) {
        let request = lock(shared).begin_compiled_details(Actor::User)?;
        let token = request.token();
        state.borrow_mut().token = Some(token);
        state.borrow_mut().offset = 0;
        let worker_session = shared.clone();
        let spawned = std::thread::Builder::new()
            .name("compiled-details".into())
            .spawn(move || {
                let prepared = request.run();
                let _ = lock(&worker_session).finish_compiled_details(Actor::User, prepared);
            });
        if let Err(error) = spawned {
            let _ = lock(shared).fail_compiled_details(
                Actor::User,
                token,
                format!("Could not start detail preparation: {error}"),
            );
        }
    }
    let mut session = lock(shared);
    let token = state.borrow().token;
    match action {
        Action::Cancel => {
            session.cancel_compiled_details(
                Actor::User,
                token.ok_or("Prepare compiled details first")?,
            )?;
        }
        Action::Clear => {
            session.clear_compiled_details(
                Actor::User,
                token.ok_or("Prepare compiled details first")?,
            )?;
            state.borrow_mut().offset = 0;
        }
        Action::Show | Action::Hide => {
            session.set_compiled_details_visible(
                Actor::User,
                token.ok_or("Prepare compiled details first")?,
                matches!(action, Action::Show),
            )?;
        }
        Action::Page(direction) => {
            let offset = state.borrow().offset;
            state.borrow_mut().offset = if direction < 0 {
                offset.saturating_sub(64)
            } else {
                offset.saturating_add(64)
            };
        }
        Action::Prepare | Action::Inspect => {}
    }
    let offset = state.borrow().offset;
    let token =
        if matches!(action, Action::Clear | Action::Inspect | Action::Prepare) && offset == 0 {
            None
        } else {
            state.borrow().token
        };
    let report = session.inspect_compiled_details(token, offset, 64)?;
    present(
        window,
        &mut state.borrow_mut(),
        report,
        session.compiled_details_observation(),
    );
    Ok(())
}

fn handler(
    window: &CompiledDetailsWindow,
    access: &DocumentAccess,
    state: &Rc<RefCell<State>>,
    action: impl Fn(i32) -> Action + 'static,
) -> impl Fn(i32) + 'static {
    let (window, target, state) = (window.as_weak(), access.downgrade(), state.clone());
    move |value| {
        let Some(window) = window.upgrade() else {
            return;
        };
        let result = target.upgrade().and_then(|target| {
            target.with_shared(|shared| perform(&window, shared, &state, action(value)))
        });
        if let Err(error) = result {
            window.set_message(error.into());
        }
    }
}

pub(super) fn bind(
    parent: &CompileWindow,
    access: &DocumentAccess,
) -> Result<Form, slint::PlatformError> {
    let window = CompiledDetailsWindow::new()?;
    let state = Rc::new(RefCell::new(State::default()));
    parent.on_inspect_compiled_details({
        let (parent, window, target, state) = (
            parent.as_weak(),
            window.as_weak(),
            access.downgrade(),
            state.clone(),
        );
        move || {
            super::admitted(&parent, &target, |_, shared| {
                let Some(window) = window.upgrade() else {
                    return;
                };
                state.borrow_mut().offset = 0;
                if let Err(error) = perform(&window, shared, &state, Action::Inspect) {
                    window.set_message(error.into());
                }
                let _ = window.show();
            })
        }
    });
    let callback = handler(&window, access, &state, |_| Action::Prepare);
    window.on_prepare_details(move || callback(0));
    let callback = handler(&window, access, &state, |_| Action::Cancel);
    window.on_cancel_preparation(move || callback(0));
    let callback = handler(&window, access, &state, |_| Action::Clear);
    window.on_clear_details(move || callback(0));
    let callback = handler(&window, access, &state, |_| Action::Show);
    window.on_show_details(move || callback(0));
    let callback = handler(&window, access, &state, |_| Action::Hide);
    window.on_hide_details(move || callback(0));
    let callback = handler(&window, access, &state, |_| Action::Inspect);
    window.on_refresh_inspection(move || callback(0));
    window.on_page_records(handler(&window, access, &state, Action::Page));
    window.on_close_dialog({
        let window = window.as_weak();
        move || {
            if let Some(window) = window.upgrade() {
                let _ = window.hide();
            }
        }
    });
    Ok(Form { window, state })
}

impl Form {
    pub(super) fn refresh(&self, session: &Session) {
        if !self.is_visible() {
            return;
        }
        let observation = session.compiled_details_observation();
        if self.state.borrow().observation.as_ref() == Some(&observation) {
            return;
        }
        self.window.set_rows("".into());
        self.window.set_previous(false);
        self.window.set_next(false);
        self.window.set_page("".into());
        self.window
            .set_worker_active(observation["worker_active"].as_bool().unwrap_or(false));
        self.state.borrow_mut().token = observation["token"]
            .as_str()
            .and_then(|token| token.parse::<u64>().ok())
            .filter(|token| *token != 0);
        self.window
            .set_has_token(self.state.borrow().token.is_some());
        self.window.set_state(
            format!(
                "{} · requested {} · displayed {} · complete geometry {}",
                observation["status"].as_str().unwrap_or("not_loaded"),
                observation["requested"],
                observation["displayed"],
                observation["complete"]
            )
            .into(),
        );
        self.window.set_build_state("State changed. Refresh inspection to check source/output freshness and show native records.".into());
        self.window
            .set_message(observation["error"].as_str().unwrap_or("").into());
        self.state.borrow_mut().offset = 0;
        self.state.borrow_mut().observation = Some(observation);
    }

    pub(super) fn is_visible(&self) -> bool {
        self.window.window().is_visible()
    }
    pub(super) fn hide_window(&self, errors: &mut Vec<String>) {
        crate::ui::hide_window("Compiled details", self.window.window(), errors);
    }
}

#[cfg(test)]
mod tests;
