use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use editor_core::session::entity_graph::{
    Context, Diagram, Direction, Inspection, Layout, MAX_PAGE, NodePosition, Query, Scope,
    Traversal, View,
};
use map_core::ObjectId;
use slint::ComponentHandle;

use crate::runtime::{DocumentAccess, DocumentContinuation};
use crate::session::{Actor, SelectMode, Session};
use crate::{EntityGraphEdge, EntityGraphNode, EntityGraphRow, EntityGraphWindow};

const MAX_CANVAS_NODES: usize = 512;
const MAX_CANVAS_EDGES: usize = 2048;

#[derive(Default)]
struct State {
    query: Option<Query>,
    report: Option<Inspection>,
    diagram: Option<Diagram>,
    diagram_serial: u64,
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
    unavailable_canvas(window, "Inspect graph to show the complete bounded canvas.");
    window.set_can_act(false);
    window.set_has_next(false);
    window.set_has_previous(false);
    window.set_summary("Inspect graph to capture the current map.".into());
    window.set_page_summary("No retained graph page".into());
}

fn unavailable_canvas(window: &EntityGraphWindow, reason: &str) {
    window.set_canvas_ready(false);
    window.set_graph_nodes(slint::ModelRc::default());
    window.set_graph_edges(slint::ModelRc::default());
    window.set_canvas_message(reason.into());
    window.set_chosen_node("".into());
    window.set_chosen_edge(-1);
    window.set_selection_details("".into());
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
        scope: if window.get_scope() == "Expanded instances" {
            Scope::Expanded
        } else {
            Scope::Authored
        },
        context: Context {
            activator: (!window.get_activator().trim().is_empty())
                .then(|| window.get_activator().trim().to_string()),
            caller: (!window.get_caller_unbound() && !window.get_caller().trim().is_empty())
                .then(|| window.get_caller().trim().to_string()),
            caller_unbound: window.get_caller_unbound(),
        },
    })
}

fn selection_mode(window: &EntityGraphWindow) -> SelectMode {
    match window.get_mode().as_str() {
        "Add" => SelectMode::Add,
        "Remove" => SelectMode::Remove,
        "Toggle" => SelectMode::Toggle,
        _ => SelectMode::Replace,
    }
}

fn present_diagram(window: &EntityGraphWindow, diagram: &Diagram) -> Result<(), String> {
    if diagram.nodes.len() > MAX_CANVAS_NODES || diagram.edges.len() > MAX_CANVAS_EDGES {
        return Err(format!(
            "Canvas unavailable: {} nodes and {} relationships exceed the desktop limit of {MAX_CANVAS_NODES} nodes / {MAX_CANVAS_EDGES} relationships. Use List or inspect with fewer roots or a smaller depth. No partial canvas is drawn.",
            diagram.nodes.len(),
            diagram.edges.len()
        ));
    }
    let positions = diagram
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node.position))
        .collect::<BTreeMap<_, _>>();
    let nodes = diagram
        .nodes
        .iter()
        .map(|node| EntityGraphNode {
            id: node.id.clone().into(),
            owner: node.owner.clone().into(),
            title: if node.targetname.is_empty() {
                "Unnamed".into()
            } else {
                node.targetname.clone().into()
            },
            subtitle: node.classname.clone().into(),
            x: node.position[0] as f32,
            y: node.position[1] as f32,
            is_root: node.root,
            virtual_node: node.id != node.owner,
        })
        .collect::<Vec<_>>();
    let edges = diagram
        .edges
        .iter()
        .enumerate()
        .map(|(index, edge)| {
            let source = positions
                .get(edge.source.as_str())
                .ok_or("Graph relationship has no source node")?;
            let target = positions
                .get(edge.target.as_str())
                .ok_or("Graph relationship has no target node")?;
            Ok(EntityGraphEdge {
                source: edge.source.clone().into(),
                target: edge.target.clone().into(),
                source_x: source[0] as f32,
                source_y: source[1] as f32,
                target_x: target[0] as f32,
                target_y: target[1] as f32,
                label: format!("{} → {}", edge.output, edge.input).into(),
                slot: index as i32,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    window.set_graph_nodes(slint::ModelRc::new(slint::VecModel::from(nodes)));
    window.set_graph_edges(slint::ModelRc::new(slint::VecModel::from(edges)));
    window.set_layout(
        match diagram.layout {
            Layout::Layered => "Layered",
            Layout::Grid => "Grid",
        }
        .into(),
    );
    window.set_pan_x(diagram.view.pan[0] as f32);
    window.set_pan_y(diagram.view.pan[1] as f32);
    window.set_zoom(diagram.view.zoom as f32);
    window.set_canvas_message("".into());
    window.set_canvas_ready(true);
    Ok(())
}

fn change_diagram(
    window: &EntityGraphWindow,
    session: &mut Session,
    state: &RefCell<State>,
    operation: impl FnOnce(&mut Session, &Traversal, &str) -> Result<Diagram, String>,
) {
    let result = (|| {
        let state = state.borrow();
        let query = state.query.as_ref().ok_or("Inspect the graph first")?;
        let snapshot = &state
            .report
            .as_ref()
            .ok_or("Inspect the graph first")?
            .snapshot;
        operation(session, &query.traversal, snapshot)
    })();
    match result {
        Ok(diagram) => retain_diagram(window, state, diagram),
        Err(error) => {
            clear(window, state);
            window.set_message(error.into());
        }
    }
}

fn retain_diagram(window: &EntityGraphWindow, state: &RefCell<State>, diagram: Diagram) {
    state.borrow_mut().diagram_serial = diagram.presentation_serial;
    match present_diagram(window, &diagram) {
        Ok(()) => state.borrow_mut().diagram = Some(diagram),
        Err(error) => {
            state.borrow_mut().diagram = None;
            unavailable_canvas(window, &error);
        }
    }
}

fn choose_node(window: &EntityGraphWindow, state: &RefCell<State>, id: &str) {
    let state = state.borrow();
    let Some(diagram) = state.diagram.as_ref() else {
        return;
    };
    let Some(node) = diagram.nodes.iter().find(|node| node.id == id) else {
        return;
    };
    window.set_chosen_node(id.into());
    window.set_chosen_edge(-1);
    let mut details = format!(
        "{} · {}\nSource {} · authored owner {} · depth {}{}{}",
        if node.targetname.is_empty() {
            "Unnamed"
        } else {
            &node.targetname
        },
        node.classname,
        node.id,
        node.owner,
        node.depth,
        if node.root { " · captured root" } else { "" },
        if node.id != node.owner {
            " · read-only instance source"
        } else {
            ""
        },
    );
    if let Some(provenance) = &node.provenance {
        details.push_str(&format!("\nInstance source: {provenance}"));
    }
    window.set_selection_details(details.into());
}

fn choose_edge(window: &EntityGraphWindow, state: &RefCell<State>, index: i32) {
    let state = state.borrow();
    let Some(edge) = state.diagram.as_ref().and_then(|diagram| {
        usize::try_from(index)
            .ok()
            .and_then(|index| diagram.edges.get(index))
    }) else {
        return;
    };
    window.set_chosen_node("".into());
    window.set_chosen_edge(index);
    window.set_selection_details(format!(
        "{} → {} · {} → {}\nConnection {} · {} · target {:?} · parameter {:?} · delay {} s · firing count {}",
        edge.source, edge.target, edge.output, edge.input, edge.connection_index,
        edge.resolution, edge.target_reference, edge.parameter, edge.delay, edge.times,
    ).into());
}

fn act_on_node(
    window: &EntityGraphWindow,
    session: &mut Session,
    state: &RefCell<State>,
    frame: bool,
) {
    let result = (|| {
        let state = state.borrow();
        let query = state.query.as_ref().ok_or("Inspect the graph first")?;
        let report = state.report.as_ref().ok_or("Inspect the graph first")?;
        let id = window.get_chosen_node().to_string();
        if id.is_empty() {
            return Err("Choose a graph node first".into());
        }
        if frame {
            session
                .frame_entity_graph_nodes(
                    Actor::User,
                    &query.traversal,
                    &report.snapshot,
                    &[id],
                    window.get_aspect(),
                )
                .map(|result| result.matched)
        } else {
            session
                .select_entity_graph_nodes(
                    Actor::User,
                    &query.traversal,
                    &report.snapshot,
                    &[id],
                    selection_mode(window),
                )
                .map(|result| result.matched)
        }
    })();
    match result {
        Ok(matched) => window.set_message(
            format!(
                "{} {matched} graph node{}",
                if frame {
                    "Framed"
                } else {
                    "Selected the authored owner of"
                },
                if matched == 1 { "" } else { "s" },
            )
            .into(),
        ),
        Err(error) => {
            clear(window, state);
            window.set_message(error.into());
        }
    }
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
                        "{} · authored owner {} · depth {}{}{}",
                        row.id, row.owner, row.depth,
                        if row.root { " · root" } else { "" },
                        row.provenance.as_ref().map(|value| format!("\nInstance source: {value}")).unwrap_or_default(),
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
    unavailable_canvas(window, "Preparing the complete graph canvas…");
    let diagram = match session.inspect_entity_graph_diagram(&query.traversal, &report.snapshot) {
        Ok(diagram) => match present_diagram(window, &diagram) {
            Ok(()) => Some(diagram),
            Err(error) => {
                unavailable_canvas(window, &error);
                None
            }
        },
        Err(error) => {
            unavailable_canvas(
                window,
                &format!("Canvas unavailable: {error}. Use List or reduce roots/depth."),
            );
            None
        }
    };
    *state.borrow_mut() = State {
        query: Some(query),
        report: Some(report),
        diagram,
        diagram_serial: session.entity_graph_diagram_serial(),
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
        clear(&window, state);
        window.set_message(error.into());
        window.set_summary("Graph owner unavailable".into());
    }
}
impl Form {
    pub fn new(access: &DocumentAccess) -> Result<Self, slint::PlatformError> {
        let window = EntityGraphWindow::new()?;
        let state = Rc::new(RefCell::new(State::default()));
        window.on_choose_node({
            let (window, access, state) = (window.as_weak(), access.downgrade(), state.clone());
            move |id| {
                admitted(&window, &access, &state, |window, _| {
                    choose_node(window, &state, &id);
                })
            }
        });
        window.on_choose_edge({
            let (window, access, state) = (window.as_weak(), access.downgrade(), state.clone());
            move |index| {
                admitted(&window, &access, &state, |window, _| {
                    choose_edge(window, &state, index);
                })
            }
        });
        window.on_arrange({
            let (window, access, state) = (window.as_weak(), access.downgrade(), state.clone());
            move || {
                admitted(&window, &access, &state, |window, session| {
                    let layout = if window.get_layout() == "Grid" {
                        Layout::Grid
                    } else {
                        Layout::Layered
                    };
                    change_diagram(window, session, &state, |session, traversal, snapshot| {
                        session.layout_entity_graph(traversal, snapshot, layout)
                    });
                })
            }
        });
        window.on_reset_diagram({
            let (window, access, state) = (window.as_weak(), access.downgrade(), state.clone());
            move || {
                admitted(&window, &access, &state, |window, session| {
                    change_diagram(window, session, &state, |session, traversal, snapshot| {
                        session.reset_entity_graph_diagram(traversal, snapshot)
                    });
                })
            }
        });
        window.on_move_node({
            let (window, access, state) = (window.as_weak(), access.downgrade(), state.clone());
            move |id, x, y| {
                admitted(&window, &access, &state, |window, session| {
                    change_diagram(window, session, &state, |session, traversal, snapshot| {
                        session.update_entity_graph_diagram(
                            traversal,
                            snapshot,
                            &[NodePosition {
                                id: id.to_string(),
                                position: [f64::from(x), f64::from(y)],
                            }],
                            None,
                        )
                    });
                })
            }
        });
        window.on_change_view({
            let (window, access, state) = (window.as_weak(), access.downgrade(), state.clone());
            move |x, y, zoom| {
                admitted(&window, &access, &state, |window, session| {
                    change_diagram(window, session, &state, |session, traversal, snapshot| {
                        session.update_entity_graph_diagram(
                            traversal,
                            snapshot,
                            &[],
                            Some(View {
                                pan: [f64::from(x), f64::from(y)],
                                zoom: f64::from(zoom),
                            }),
                        )
                    });
                })
            }
        });
        window.on_select_node({
            let (window, access, state) = (window.as_weak(), access.downgrade(), state.clone());
            move || {
                admitted(&window, &access, &state, |window, session| {
                    act_on_node(window, session, &state, false);
                })
            }
        });
        window.on_frame_node({
            let (window, access, state) = (window.as_weak(), access.downgrade(), state.clone());
            move || {
                admitted(&window, &access, &state, |window, session| {
                    act_on_node(window, session, &state, true);
                })
            }
        });
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
                        session.select_entity_graph(
                            Actor::User,
                            &traversal(window)?,
                            &report.snapshot,
                            selection_mode(window),
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
            return;
        }
        let request = {
            let state = self.state.borrow();
            if state.diagram_serial == session.entity_graph_diagram_serial() {
                None
            } else {
                state
                    .query
                    .as_ref()
                    .zip(state.report.as_ref())
                    .map(|(query, report)| (query.traversal.clone(), report.snapshot.clone()))
            }
        };
        if let Some((traversal, snapshot)) = request {
            match session.inspect_entity_graph_diagram(&traversal, &snapshot) {
                Ok(diagram) => retain_diagram(&self.window, &self.state, diagram),
                Err(error) => {
                    clear(&self.window, &self.state);
                    self.window.set_message(error.into());
                }
            }
        }
    }
}
