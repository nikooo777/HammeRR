use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{
    Graph, MAX_FIELD_BYTES, MAX_GRAPH_NODES, MAX_REPORT_BYTES, ReportSize, Session, Traversal, id,
    reserve,
};

pub const NODE_WIDTH: f64 = 220.0;
pub const NODE_HEIGHT: f64 = 78.0;
const COLUMN_STEP: f64 = 300.0;
const ROW_STEP: f64 = 110.0;
const MAX_COORDINATE: f64 = 1_000_000.0;

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Layout {
    #[default]
    Layered,
    Grid,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct View {
    pub pan: [f64; 2],
    pub zoom: f64,
}

impl Default for View {
    fn default() -> Self {
        Self {
            pan: [24.0, 24.0],
            zoom: 1.0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodePosition {
    pub id: String,
    pub position: [f64; 2],
}

#[derive(Debug, Serialize)]
pub struct DiagramNode {
    pub id: String,
    pub owner: String,
    pub provenance: Option<serde_json::Value>,
    pub classname: String,
    pub targetname: String,
    pub depth: usize,
    pub root: bool,
    pub position: [f64; 2],
}

#[derive(Debug, Serialize)]
pub struct DiagramEdge {
    pub source: String,
    pub target: String,
    pub connection_index: usize,
    pub output: String,
    pub input: String,
    pub target_reference: String,
    pub parameter: String,
    pub delay: f32,
    pub times: i32,
    pub resolution: &'static str,
}

#[derive(Debug, Serialize)]
pub struct Diagram {
    pub snapshot: String,
    pub generation: u64,
    pub revision: u64,
    pub presentation_serial: u64,
    pub layout: Layout,
    pub customized: bool,
    pub view: View,
    pub node_size: [f64; 2],
    pub nodes: Vec<DiagramNode>,
    pub edges: Vec<DiagramEdge>,
}

#[derive(Clone, Default)]
pub(crate) struct State {
    serial: u64,
    snapshot: String,
    layout: Layout,
    positions: BTreeMap<String, [f64; 2]>,
    view: View,
}

fn coordinates(position: [f64; 2]) -> Result<(), String> {
    if position
        .iter()
        .any(|value| !value.is_finite() || value.abs() > MAX_COORDINATE)
    {
        return Err("Graph diagram coordinates must be finite and within ±1000000".into());
    }
    Ok(())
}

impl State {
    fn current(&self, snapshot: &str) -> Self {
        if self.snapshot == snapshot {
            self.clone()
        } else {
            Self {
                snapshot: snapshot.into(),
                serial: self.serial,
                ..Self::default()
            }
        }
    }
}

impl Graph<'_> {
    fn diagram(&self, session: &Session, state: &State) -> Result<Diagram, String> {
        let mut report_size = ReportSize(0);
        let mut nodes = Vec::new();
        reserve(&mut nodes, self.selected.len())?;
        let mut ranks = BTreeMap::<usize, usize>::new();
        let columns = (self.selected.len() as f64).sqrt().ceil().max(1.0) as usize;
        for (ordinal, &index) in self.selected.iter().enumerate() {
            let entity = self.entity(index);
            let identity = self.id(index);
            let depth = self.distances[index].unwrap();
            let automatic = match state.layout {
                Layout::Layered => {
                    let rank = ranks.entry(depth).or_default();
                    let position = [depth as f64 * COLUMN_STEP, *rank as f64 * ROW_STEP];
                    *rank += 1;
                    position
                }
                Layout::Grid => [
                    (ordinal % columns) as f64 * COLUMN_STEP,
                    (ordinal / columns) as f64 * ROW_STEP,
                ],
            };
            if self.source.provenance_bound(entity.id)
                > MAX_REPORT_BYTES.saturating_sub(report_size.0)
            {
                return Err("Graph diagram provenance exceeds its report-storage limit".into());
            }
            let node = DiagramNode {
                position: state.positions.get(&identity).copied().unwrap_or(automatic),
                id: identity,
                owner: id(self.owner(index)),
                provenance: self.source.provenance(entity.id),
                classname: entity.classname().unwrap_or_default().into(),
                targetname: entity
                    .properties
                    .get("targetname")
                    .unwrap_or_default()
                    .into(),
                depth,
                root: self.roots.contains(&entity.id),
            };
            serde_json::to_writer(&mut report_size, &node).map_err(|error| error.to_string())?;
            nodes.push(node);
        }
        let mut edges = Vec::new();
        reserve(&mut edges, self.graph_links.len())?;
        for &index in &self.graph_links {
            let link = &self.links[index];
            let record = &self.records[link.record];
            let spec = record.spec.as_ref().unwrap();
            let edge = DiagramEdge {
                source: self.id(link.source),
                target: self.id(link.target),
                connection_index: record.index,
                output: spec.output.clone(),
                input: spec.input.clone(),
                target_reference: spec.target.clone(),
                parameter: spec.parameter.clone(),
                delay: spec.delay,
                times: spec.times,
                resolution: record.status,
            };
            serde_json::to_writer(&mut report_size, &edge).map_err(|error| error.to_string())?;
            edges.push(edge);
        }
        let report = Diagram {
            snapshot: state.snapshot.clone(),
            generation: session.document_generation,
            revision: session.document.revision(),
            presentation_serial: state.serial,
            layout: state.layout,
            customized: !state.positions.is_empty(),
            view: state.view,
            node_size: [NODE_WIDTH, NODE_HEIGHT],
            nodes,
            edges,
        };
        self.source.verify()?;
        serde_json::to_writer(ReportSize(0), &report).map_err(|error| error.to_string())?;
        Ok(report)
    }
}

impl Session {
    pub fn entity_graph_diagram_serial(&self) -> u64 {
        self.entity_graph.serial
    }

    pub fn inspect_entity_graph_diagram(
        &self,
        traversal: &Traversal,
        snapshot: &str,
    ) -> Result<Diagram, String> {
        let graph = self.entity_graph_action(traversal, snapshot)?;
        graph.diagram(self, &self.entity_graph.current(snapshot))
    }

    pub fn layout_entity_graph(
        &mut self,
        traversal: &Traversal,
        snapshot: &str,
        layout: Layout,
    ) -> Result<Diagram, String> {
        let graph = self.entity_graph_action(traversal, snapshot)?;
        let mut state = self.entity_graph.current(snapshot);
        state.layout = layout;
        state.positions.clear();
        state.serial = self.entity_graph.serial.wrapping_add(1);
        let report = graph.diagram(self, &state)?;
        self.entity_graph = state;
        Ok(report)
    }

    pub fn update_entity_graph_diagram(
        &mut self,
        traversal: &Traversal,
        snapshot: &str,
        positions: &[NodePosition],
        view: Option<View>,
    ) -> Result<Diagram, String> {
        if positions.len() > MAX_GRAPH_NODES {
            return Err("Too many graph diagram positions".into());
        }
        let graph = self.entity_graph_action(traversal, snapshot)?;
        let mut state = self.entity_graph.current(snapshot);
        let mut seen = BTreeSet::new();
        let admitted = graph
            .selected
            .iter()
            .map(|&index| graph.id(index))
            .collect::<BTreeSet<_>>();
        let mut retained = state
            .positions
            .keys()
            .map(|key| key.len() + 96)
            .sum::<usize>();
        for position in positions {
            if position.id.len() > MAX_FIELD_BYTES || !seen.insert(&position.id) {
                return Err("Graph diagram IDs must be bounded and unique".into());
            }
            coordinates(position.position)?;
            if !admitted.contains(&position.id) {
                return Err("Graph diagram node is outside the current traversal".into());
            }
            if !state.positions.contains_key(&position.id) {
                retained = retained
                    .checked_add(position.id.len() + 96)
                    .filter(|&bytes| bytes <= MAX_REPORT_BYTES)
                    .ok_or("Graph diagram positions exceed their retained-storage bound")?;
            }
            state
                .positions
                .insert(position.id.clone(), position.position);
        }
        if let Some(view) = view {
            coordinates(view.pan)?;
            if !view.zoom.is_finite() || !(0.1..=4.0).contains(&view.zoom) {
                return Err("Graph diagram zoom must be finite and within 0.1–4".into());
            }
            state.view = view;
        }
        state.serial = self.entity_graph.serial.wrapping_add(1);
        let report = graph.diagram(self, &state)?;
        self.entity_graph = state;
        Ok(report)
    }

    pub fn reset_entity_graph_diagram(
        &mut self,
        traversal: &Traversal,
        snapshot: &str,
    ) -> Result<Diagram, String> {
        let graph = self.entity_graph_action(traversal, snapshot)?;
        let state = State {
            snapshot: snapshot.into(),
            serial: self.entity_graph.serial.wrapping_add(1),
            ..State::default()
        };
        let report = graph.diagram(self, &state)?;
        self.entity_graph = state;
        Ok(report)
    }
}
