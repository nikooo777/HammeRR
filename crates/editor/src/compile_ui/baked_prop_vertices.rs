use std::cell::RefCell;
use std::rc::Rc;

use editor_core::session::baked_prop_vertices::{Channel, Query};
use serde_json::Value;
use slint::ComponentHandle;

use super::{CompileWindow, DocumentAccess, Session, lock};
use crate::BakedPropVerticesWindow;

pub(super) struct Form {
    window: BakedPropVerticesWindow,
    state: Rc<RefCell<State>>,
}

#[derive(Default)]
struct State {
    observation: Option<Value>,
    report: Option<Value>,
    query: Query,
}

#[derive(Clone, Copy)]
enum Action {
    Refresh,
    Channel,
    Prop(i32),
    Section(i32),
    Page(&'static str, i32),
}

fn rows(page: &Value) -> &[Value] {
    page["rows"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn strings(values: impl Iterator<Item = String>) -> slint::ModelRc<slint::SharedString> {
    Rc::new(slint::VecModel::from(
        values.map(Into::into).collect::<Vec<_>>(),
    ))
    .into()
}

fn build_text(build: &Value) -> String {
    let pending = build["render_context"]["pending_preview"] == true;
    format!(
        "Build {} · token {} · {} · {} bounces{}",
        build["status"].as_str().unwrap_or("unavailable"),
        build["token"].as_str().unwrap_or("none"),
        build["lighting"].as_str().unwrap_or("unknown lighting"),
        build["bounces"]
            .as_u64()
            .map_or_else(|| "?".into(), |n| n.to_string()),
        if pending {
            "\nThe displayed draft was not baked."
        } else {
            ""
        }
    )
}

fn page_text(page: &Value) -> String {
    format!(
        "{} rows from {} · {} total",
        page["returned"], page["offset"], page["total"]
    )
}

fn clear(window: &BakedPropVerticesWindow, state: &RefCell<State>) {
    *state.borrow_mut() = State::default();
    window.set_prop_rows(slint::ModelRc::default());
    window.set_section_rows(slint::ModelRc::default());
    window.set_vertex_rows(slint::ModelRc::default());
    window.set_prop_page("No retained prop page".into());
    window.set_section_page("No retained section page".into());
    window.set_vertex_page("No retained vertex page".into());
    window.set_selected("No stored color selected".into());
    window.set_statistics("".into());
    window.set_channels("".into());
    window.set_previous_props(false);
    window.set_next_props(false);
    window.set_previous_sections(false);
    window.set_next_sections(false);
    window.set_previous_vertices(false);
    window.set_next_vertices(false);
}

fn source_text(association: &Value, historical: bool, compact: bool) -> String {
    if association["status"] == "available" {
        let label = association[if compact { "label" } else { "description" }]
            .as_str()
            .unwrap_or("Unavailable");
        if compact {
            label.into()
        } else {
            format!(
                "{} source: {label}",
                if historical { "Historical" } else { "Captured" }
            )
        }
    } else if compact {
        "Source unavailable".into()
    } else {
        format!(
            "Source association unavailable: {}",
            association["reason"]
                .as_str()
                .unwrap_or("No captured metadata")
        )
    }
}

fn present(window: &BakedPropVerticesWindow, report: &Value) {
    window.set_build_state(build_text(&report["build"]).into());
    window.set_channels(
        report["channels"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|channel| {
                format!(
                    "{}: {} · {} bytes",
                    channel["channel"].as_str().unwrap_or("?"),
                    if channel["available"] == true {
                        "available"
                    } else {
                        "absent"
                    },
                    channel["bytes"]
                )
            })
            .collect::<Vec<_>>()
            .join(" · ")
            .into(),
    );
    window.set_prop_rows(strings(rows(&report["props"]).iter().map(|prop| {
        format!(
            "Prop {} · {} · {} · origin {} · skin {} · flags {} · {} sections · {} vertices · source {}",
            prop["ordinal"],
            prop["model"].as_str().unwrap_or("unknown model"),
            prop["status"].as_str().unwrap_or("unavailable"),
            prop["origin"],
            prop["skin"],
            prop["flags"],
            prop["section_count"],
            prop["vertex_count"],source_text(&prop["source_association"],report["historical"]==true,true)
        )
    })));
    window.set_section_rows(strings(rows(&report["sections"]).iter().map(|section| {
        format!(
            "Section {} · LOD {} · {} vertices · hardware start {}",
            section["index"], section["lod"], section["vertex_count"], section["hardware_start"]
        )
    })));
    window.set_vertex_rows(strings(rows(&report["vertices"]).iter().map(|vertex| {
        let color = &vertex["rgba8"];
        format!(
            "{}    {}    {}    {}    {}    {}",
            vertex["index"], vertex["hardware_index"], color[0], color[1], color[2], color[3]
        )
    })));
    window.set_prop_page(page_text(&report["props"]).into());
    window.set_section_page(page_text(&report["sections"]).into());
    window.set_vertex_page(page_text(&report["vertices"]).into());
    window.set_previous_props(report["props"]["offset"].as_u64().is_some_and(|n| n > 0));
    window.set_next_props(report["props"]["next_offset"].is_u64());
    window.set_previous_sections(report["sections"]["offset"].as_u64().is_some_and(|n| n > 0));
    window.set_next_sections(report["sections"]["next_offset"].is_u64());
    window.set_previous_vertices(report["vertices"]["offset"].as_u64().is_some_and(|n| n > 0));
    window.set_next_vertices(report["vertices"]["next_offset"].is_u64());
    let selected = &report["selected"];
    let source = source_text(
        &report["selected_prop"]["source_association"],
        report["historical"] == true,
        false,
    );
    window.set_selected(if selected.is_object() {
        format!("{source}\nProp {} · {} · checksum {} · {} · section {} · LOD {}\n{} · {} bytes · SHA256 {}\nSource closure: {}",
            selected["ordinal"], selected["model"].as_str().unwrap_or("?"), selected["checksum"], report["channel"].as_str().unwrap_or("?"), report["vertices"]["section"],
            report["vertices"]["lod"], selected["path"].as_str().unwrap_or("?"), selected["bytes"],
            selected["sha256"].as_str().unwrap_or("?"), report["bake"]["source_closure"].as_str().unwrap_or("unverifiable"))
    } else if report["selected_prop"].is_object(){format!("{source}\nNo stored color selected")}else { "No stored color selected".into() }.into());
    let statistics = &report["statistics"];
    window.set_statistics(
        if statistics.is_object() {
            format!(
                "Returned vertex page only · {} samples · encoded RGB min {} · mean {} · max {}",
                statistics["count"],
                statistics["encoded_rgb8"]["minimum"],
                statistics["encoded_rgb8"]["mean"],
                statistics["encoded_rgb8"]["maximum"]
            )
        } else {
            String::new()
        }
        .into(),
    );
    window.set_diagnostics(
        [
            &report["diagnostics"],
            &report["build"]["diagnostics"],
            &report["bake"]["diagnostics"],
            &report["source_associations"]["diagnostics"],
            &report["build"]["render_context"]["diagnostics"],
        ]
        .into_iter()
        .flat_map(|value| value.as_array().into_iter().flatten())
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("\n")
        .into(),
    );
}

fn current(window: &BakedPropVerticesWindow, session: &Session, state: &RefCell<State>) -> bool {
    let observation = session.baked_prop_vertex_build_state();
    if state.borrow().report.is_some() && state.borrow().observation.as_ref() == Some(&observation)
    {
        return true;
    }
    clear(window, state);
    window.set_build_state(build_text(&observation).into());
    state.borrow_mut().observation = Some(observation);
    window.set_diagnostics("Build/source/settings changed. Refresh inspection before selecting a retained row or page.".into());
    false
}

fn inspect(
    window: &BakedPropVerticesWindow,
    session: &Session,
    state: &RefCell<State>,
    action: Action,
) {
    let mut query = if matches!(action, Action::Refresh) {
        Query::default()
    } else {
        if !current(window, session, state) {
            return;
        }
        state.borrow().query.clone()
    };
    query.prop_limit = usize::try_from(window.get_prop_limit()).unwrap_or(0);
    query.channel = if window.get_channel() == "HDR" {
        Channel::Hdr
    } else {
        Channel::Ldr
    };
    match action {
        Action::Refresh => {}
        Action::Channel => {
            query.vertex_offset = 0;
        }
        Action::Prop(index) | Action::Section(index) => {
            let key = if matches!(action, Action::Prop(_)) {
                "props"
            } else {
                "sections"
            };
            let index = usize::try_from(index).ok();
            let value = state
                .borrow()
                .report
                .as_ref()
                .and_then(|report| index.and_then(|i| rows(&report[key]).get(i)))
                .and_then(|row| row[if key == "props" { "ordinal" } else { "index" }].as_u64());
            let Some(value) = value else {
                window.set_diagnostics("Choose a current inspection row.".into());
                return;
            };
            if key == "props" {
                query.prop = u32::try_from(value).ok();
                query.section = None;
                query.section_offset = 0;
            } else {
                query.section = usize::try_from(value).ok();
            }
            query.vertex_offset = 0;
        }
        Action::Page(key, direction) => {
            let next = state
                .borrow()
                .report
                .as_ref()
                .and_then(|report| {
                    let page = &report[key];
                    if direction > 0 {
                        page["next_offset"].as_u64()
                    } else {
                        page["offset"]
                            .as_u64()
                            .filter(|n| *n > 0)
                            .map(|n| n.saturating_sub(page["limit"].as_u64().unwrap_or(0)))
                    }
                })
                .and_then(|n| usize::try_from(n).ok());
            let Some(offset) = next else {
                return;
            };
            match key {
                "props" => query.prop_offset = offset,
                "sections" => query.section_offset = offset,
                "vertices" => query.vertex_offset = offset,
                _ => return,
            }
        }
    }
    match session.inspect_baked_prop_vertices(&query) {
        Ok(inspection) => {
            query.build_token = inspection.report["build"]["token"]
                .as_str()
                .map(str::to_owned);
            if query.prop.is_none() {
                query.prop = inspection.report["selected"]["ordinal"]
                    .as_u64()
                    .and_then(|n| u32::try_from(n).ok());
            }
            if query.section.is_none() {
                query.section = inspection.report["vertices"]["section"]
                    .as_u64()
                    .and_then(|n| usize::try_from(n).ok());
            }
            present(window, &inspection.report);
            *state.borrow_mut() = State {
                observation: Some(session.baked_prop_vertex_build_state()),
                report: Some(inspection.report),
                query,
            };
        }
        Err(error) => {
            clear(window, state);
            let observation = session.baked_prop_vertex_build_state();
            window.set_build_state(build_text(&observation).into());
            state.borrow_mut().observation = Some(observation);
            window.set_diagnostics(error.into());
        }
    }
}

fn handler(
    window: &BakedPropVerticesWindow,
    access: &DocumentAccess,
    state: &Rc<RefCell<State>>,
    action: impl Fn(i32) -> Action + 'static,
) -> impl Fn(i32) + 'static {
    let (window, target, state) = (window.as_weak(), access.downgrade(), state.clone());
    move |index| {
        let Some(window) = window.upgrade() else {
            return;
        };
        let result = target.upgrade().and_then(|target| {
            target.with_shared(|shared| {
                inspect(&window, &lock(shared), &state, action(index));
                Ok(())
            })
        });
        if let Err(error) = result {
            clear(&window, &state);
            window.set_build_state("Build unavailable".into());
            window.set_diagnostics(error.into());
        }
    }
}

pub(super) fn bind(
    parent: &CompileWindow,
    access: &DocumentAccess,
) -> Result<Form, slint::PlatformError> {
    let window = BakedPropVerticesWindow::new()?;
    let state = Rc::new(RefCell::new(State::default()));
    parent.on_inspect_baked_prop_vertices({
        let (window, target, state, parent) = (
            window.as_weak(),
            access.downgrade(),
            state.clone(),
            parent.as_weak(),
        );
        move || {
            super::admitted(&parent, &target, |_, shared| {
                let Some(window) = window.upgrade() else {
                    return;
                };
                inspect(&window, &lock(shared), &state, Action::Refresh);
                let _ = window.show();
            });
        }
    });
    let refresh = handler(&window, access, &state, |_| Action::Refresh);
    window.on_refresh_inspection(move || refresh(0));
    let channel = handler(&window, access, &state, |_| Action::Channel);
    window.on_change_channel(move || channel(0));
    window.on_inspect_prop(handler(&window, access, &state, Action::Prop));
    window.on_inspect_section(handler(&window, access, &state, Action::Section));
    window.on_page_props(handler(&window, access, &state, |n| {
        Action::Page("props", n)
    }));
    window.on_page_sections(handler(&window, access, &state, |n| {
        Action::Page("sections", n)
    }));
    window.on_page_vertices(handler(&window, access, &state, |n| {
        Action::Page("vertices", n)
    }));
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
    pub(super) fn hide_window(&self, errors: &mut Vec<String>) {
        crate::ui::hide_window("Baked prop vertices", self.window.window(), errors);
    }

    pub(super) fn is_visible(&self) -> bool {
        self.window.window().is_visible()
    }

    pub(super) fn refresh(&self, session: &Session) {
        if !self.is_visible() {
            return;
        }
        let changed = self.state.borrow().observation.as_ref()
            != Some(&session.baked_prop_vertex_build_state());
        if changed {
            current(&self.window, session, &self.state);
        }
    }

    #[cfg(test)]
    pub(super) fn window(&self) -> &BakedPropVerticesWindow {
        &self.window
    }
}

#[cfg(test)]
mod tests;
