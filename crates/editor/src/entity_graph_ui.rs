use std::cell::RefCell;
use std::rc::Rc;

use editor_core::session::entity_graph::{Direction, Inspection, MAX_PAGE, Query, Traversal};
use map_core::ObjectId;
use slint::ComponentHandle;

use crate::runtime::{DocumentAccess, DocumentContinuation};
use crate::session::{Actor, SelectMode, Session};
use crate::{EntityGraphRow, EntityGraphWindow};

#[derive(Default)]
struct State {
    query: Option<Query>,
    report: Option<Inspection>,
    retired: bool,
}
pub struct Form {
    window: EntityGraphWindow,
    state: Rc<RefCell<State>>,
}

fn clear(window: &EntityGraphWindow, state: &RefCell<State>) {
    let retired = state.borrow().retired;
    *state.borrow_mut() = State {
        retired,
        ..State::default()
    };
    window.set_rows(slint::ModelRc::default());
    window.set_can_act(false);
    window.set_has_next(false);
    window.set_has_previous(false);
    window.set_summary("Inspect graph to capture the current map.".into());
    window.set_page_summary("No retained graph page".into());
}
fn traversal(window: &EntityGraphWindow) -> Result<Traversal, String> {
    let roots = window
        .get_roots()
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|value| !value.is_empty())
        .take(editor_core::session::entity_graph::MAX_ROOTS + 1)
        .map(|value| match crate::identity::parse_id(value)? {
            ObjectId::Entity(id) => Ok(id),
            _ => Err("Graph roots must be entity IDs".into()),
        })
        .collect::<Result<Vec<_>, String>>()?;
    if roots.len() > editor_core::session::entity_graph::MAX_ROOTS {
        return Err("Graph supports at most 64 captured roots; reduce the entity selection".into());
    }
    let direction = match window.get_direction().as_str() {
        "Incoming" => Direction::Incoming,
        "Both" => Direction::Both,
        _ => Direction::Outgoing,
    };
    Ok(Traversal {
        roots,
        direction,
        depth: window
            .get_depth()
            .parse()
            .map_err(|_| "Graph depth must be an integer from 1 to 16")?,
    })
}
fn root_summary(window: &EntityGraphWindow, session: &Session, roots: &[u32]) {
    let mut labels = roots
        .iter()
        .take(3)
        .filter_map(|&id| {
            session.document().entity(id).map(|entity| {
                let name = entity
                    .properties
                    .get("targetname")
                    .filter(|name| !name.is_empty())
                    .unwrap_or("Unnamed")
                    .chars()
                    .take(40)
                    .collect::<String>();
                let class = entity
                    .classname()
                    .unwrap_or("Unknown class")
                    .chars()
                    .take(40)
                    .collect::<String>();
                format!("{name} · {class} (entity:{id})")
            })
        })
        .collect::<Vec<_>>();
    if roots.len() > 3 {
        labels.push(format!("{} more captured roots", roots.len() - 3));
    }
    window.set_root_summary(if labels.is_empty() {
        "Select entities in the map, then choose Use selected entities.".into()
    } else {
        labels.join("; ").into()
    });
}
fn use_selection(window: &EntityGraphWindow, session: &Session) {
    let roots = session
        .selected_ids()
        .into_iter()
        .filter_map(|id| match id {
            ObjectId::Entity(id) => Some(id),
            _ => None,
        })
        .collect::<Vec<_>>();
    window.set_roots(
        roots
            .iter()
            .map(|&id| crate::identity::id_string(ObjectId::Entity(id)))
            .collect::<Vec<_>>()
            .join(", ")
            .into(),
    );
    root_summary(window, session, &roots);
}

fn present(window: &EntityGraphWindow, report: &Inspection) {
    let (offset, total, next, rows) = match window.get_section().as_str() {
        "Relationships" => (
            report.edges.offset,
            report.edges.total,
            report.edges.next_offset,
            report
                .edges
                .rows
                .iter()
                .map(|row| EntityGraphRow {
                    title: format!(
                        "{} → {} · {} → {}",
                        row.source_label, row.target_label, row.output, row.input
                    )
                    .into(),
                    details: format!(
                        "{} → {} · connection {} · {} · target {:?} · parameter {:?} · delay {} s · firing count {}",
                        row.source, row.target, row.connection_index, row.resolution, row.target_reference, row.parameter, row.delay, row.times
                    )
                    .into(),
                })
                .collect::<Vec<_>>(),
        ),
        "Unresolved (whole map)" => (
            report.unresolved.offset,
            report.unresolved.total,
            report.unresolved.next_offset,
            report
                .unresolved
                .rows
                .iter()
                .map(|row| EntityGraphRow {
                    title: format!(
                        "{} · connection {} · {} · {}",
                        row.source_label, row.connection_index, row.output, row.status
                    )
                    .into(),
                    details: format!(
                        "{} · {}\nStored: {}\nCandidates: {} ({} total)",
                        row.source,
                        row.reason,
                        row.raw,
                        row.candidates.join(", "),
                        row.candidates_total
                    )
                    .into(),
                })
                .collect(),
        ),
        _ => (
            report.nodes.offset,
            report.nodes.total,
            report.nodes.next_offset,
            report
                .nodes
                .rows
                .iter()
                .map(|row| EntityGraphRow {
                    title: format!("{} · {}", if row.targetname.is_empty() { "Unnamed" } else { &row.targetname }, row.classname).into(),
                    details: format!(
                        "{} · depth {}{}",
                        row.id, row.depth,
                        if row.root { " · root" } else { "" }
                    )
                    .into(),
                })
                .collect(),
        ),
    };
    window.set_summary(format!("{} reached nodes · {} relationships · {} unresolved map connections · generation {} / revision {}", report.traversal_nodes, report.traversal_edges, report.unresolved_connections, report.generation, report.revision).into());
    window.set_page_summary(
        format!(
            "{} displayed rows from {} · {} filtered total",
            rows.len(),
            offset,
            total
        )
        .into(),
    );
    window.set_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
    window.set_has_previous(offset > 0);
    window.set_has_next(next.is_some());
    window.set_can_act(true);
}
fn inspect(
    window: &EntityGraphWindow,
    session: &Session,
    state: &RefCell<State>,
    direction: i32,
) -> Result<(), String> {
    let mut query = if direction == 0 {
        Query {
            snapshot: None,
            traversal: traversal(window)?,
            search: window.get_search().to_string(),
            node_offset: 0,
            edge_offset: 0,
            unresolved_offset: 0,
            limit: MAX_PAGE,
        }
    } else {
        state
            .borrow()
            .query
            .clone()
            .ok_or("Inspect the graph before paging")?
    };
    if direction != 0 {
        let offset = match window.get_section().as_str() {
            "Relationships" => &mut query.edge_offset,
            "Unresolved (whole map)" => &mut query.unresolved_offset,
            _ => &mut query.node_offset,
        };
        *offset = if direction > 0 {
            offset.saturating_add(MAX_PAGE)
        } else {
            offset.saturating_sub(MAX_PAGE)
        };
    }
    let report = session.inspect_entity_graph(&query)?;
    query.snapshot = Some(report.snapshot.clone());
    root_summary(window, session, &query.traversal.roots);
    present(window, &report);
    *state.borrow_mut() = State {
        query: Some(query),
        report: Some(report),
        retired: false,
    };
    window.set_message("".into());
    Ok(())
}
fn admitted(
    window: &slint::Weak<EntityGraphWindow>,
    access: &DocumentContinuation,
    state: &RefCell<State>,
    body: impl FnOnce(&EntityGraphWindow, &mut Session),
) {
    let Some(window) = window.upgrade() else {
        return;
    };
    if state.borrow().retired {
        window.set_message("Entity graph window is retired".into());
        window.set_can_act(false);
        return;
    }
    let result = access.upgrade().and_then(|access| {
        access.with_session(|session| {
            body(&window, session);
            Ok(())
        })
    });
    if let Err(error) = result {
        window.set_message(error.into());
        window.set_rows(slint::ModelRc::default());
        window.set_summary("Graph owner unavailable".into());
        window.set_page_summary("No retained graph page".into());
        window.set_has_previous(false);
        window.set_has_next(false);
        window.set_can_act(false);
    }
}
impl Form {
    pub fn new(access: &DocumentAccess) -> Result<Self, slint::PlatformError> {
        let window = EntityGraphWindow::new()?;
        let state = Rc::new(RefCell::new(State::default()));
        window.on_fields_changed({
            let (window, state) = (window.as_weak(), state.clone());
            move || {
                if let Some(window) = window.upgrade() {
                    clear(&window, &state);
                }
            }
        });
        window.on_change_section({
            let (window, access, state) = (window.as_weak(), access.downgrade(), state.clone());
            move || {
                admitted(&window, &access, &state, |window, session| {
                    let changed = state.borrow().report.as_ref().is_some_and(|report| {
                        report.generation != session.document_generation()
                            || report.revision != session.document().revision()
                    });
                    if changed {
                        clear(window, &state);
                        window.set_message("Map changed; inspect the graph again.".into());
                    } else if let Some(report) = state.borrow().report.as_ref() {
                        present(window, report);
                    }
                })
            }
        });
        window.on_roots_from_selection({
            let (window, access, state) = (window.as_weak(), access.downgrade(), state.clone());
            move || {
                admitted(&window, &access, &state, |window, session| {
                    use_selection(window, session);
                    clear(window, &state);
                })
            }
        });
        window.on_inspect({
            let (window, access, state) = (window.as_weak(), access.downgrade(), state.clone());
            move || {
                admitted(&window, &access, &state, |window, session| {
                    if let Err(error) = inspect(window, session, &state, 0) {
                        clear(window, &state);
                        window.set_message(error.into());
                    }
                })
            }
        });
        window.on_page({
            let (window, access, state) = (window.as_weak(), access.downgrade(), state.clone());
            move |direction| {
                admitted(&window, &access, &state, |window, session| {
                    if let Err(error) = inspect(window, session, &state, direction) {
                        clear(window, &state);
                        window.set_message(error.into());
                    }
                })
            }
        });
        window.on_select_connected({
            let (window, access, state) = (window.as_weak(), access.downgrade(), state.clone());
            move || {
                admitted(&window, &access, &state, |window, session| {
                    let result = (|| {
                        let state = state.borrow();
                        let report = state
                            .report
                            .as_ref()
                            .ok_or("Inspect the graph before selecting")?;
                        let mode = match window.get_mode().as_str() {
                            "Add" => SelectMode::Add,
                            "Remove" => SelectMode::Remove,
                            "Toggle" => SelectMode::Toggle,
                            _ => SelectMode::Replace,
                        };
                        session.select_entity_graph(
                            Actor::User,
                            &traversal(window)?,
                            &report.snapshot,
                            mode,
                        )
                    })();
                    match result {
                        Ok(result) => window.set_message(
                            format!("Selected from {} complete traversal nodes", result.matched)
                                .into(),
                        ),
                        Err(error) => {
                            clear(window, &state);
                            window.set_message(error.into());
                        }
                    }
                })
            }
        });
        window.on_frame_connected({
            let (window, access, state) = (window.as_weak(), access.downgrade(), state.clone());
            move || {
                admitted(&window, &access, &state, |window, session| {
                    let result = (|| {
                        let state = state.borrow();
                        let report = state
                            .report
                            .as_ref()
                            .ok_or("Inspect the graph before framing")?;
                        session.frame_entity_graph(
                            Actor::User,
                            &traversal(window)?,
                            &report.snapshot,
                            window.get_aspect(),
                        )
                    })();
                    match result {
                        Ok(result) => window.set_message(
                            format!("Framed {} complete traversal nodes", result.matched).into(),
                        ),
                        Err(error) => {
                            clear(window, &state);
                            window.set_message(error.into());
                        }
                    }
                })
            }
        });
        window.on_close_dialog({
            let window = window.as_weak();
            move || {
                if let Some(window) = window.upgrade() {
                    let _ = window.hide();
                }
            }
        });
        window.window().on_close_requested({
            let window = window.as_weak();
            move || {
                if let Some(window) = window.upgrade() {
                    let _ = window.hide();
                }
                slint::CloseRequestResponse::HideWindow
            }
        });
        clear(&window, &state);
        Ok(Self { window, state })
    }
    pub fn show(&self, session: &Session, aspect: f32) -> Result<(), slint::PlatformError> {
        use_selection(&self.window, session);
        self.window.set_aspect(aspect);
        clear(&self.window, &self.state);
        self.window.show()
    }
    pub(crate) fn hide_windows(&self, errors: &mut Vec<String>) {
        self.state.borrow_mut().retired = true;
        clear(&self.window, &self.state);
        self.window
            .set_message("Entity graph window is retired".into());
        crate::ui::hide_window("Entity graph", self.window.window(), errors);
    }

    pub fn refresh_visible(&self, session: &Session, aspect: f32) {
        if !self.window.window().is_visible() {
            return;
        }
        self.window.set_aspect(aspect);
        let changed = self.state.borrow().report.as_ref().is_some_and(|report| {
            report.generation != session.document_generation()
                || report.revision != session.document().revision()
        });
        if changed {
            clear(&self.window, &self.state);
            self.window
                .set_message("Map changed; inspect the graph again.".into());
        }
    }
}
