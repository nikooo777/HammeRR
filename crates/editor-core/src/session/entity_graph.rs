use std::collections::{BTreeMap, VecDeque};
use std::io::{self, Write};

use map_core::{ObjectId, connections};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{Actor, SelectMode, Session};

pub const MAX_MAP_NODES: usize = 8192;
pub const MAX_CONNECTIONS: usize = 32768;
pub const MAX_GRAPH_NODES: usize = 2048;
pub const MAX_GRAPH_EDGES: usize = 8192;
pub const MAX_DEPTH: usize = 16;
pub const MAX_ROOTS: usize = 64;
pub const MAX_PAGE: usize = 64;
pub const MAX_SOURCE_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_FIELD_BYTES: usize = 4096;
pub const MAX_WORK: usize = 1_000_000;
pub const MAX_SCRATCH_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_REPORT_BYTES: usize = 1024 * 1024;
const MAX_CANDIDATES: usize = 16;
const SCOPE: &str = "Committed authored root-map entities except saved hidden entities; temporary editor exclusions do not alter connectivity. Unapplied drafts and expanded instance contents are excluded. Exact named fan-out and !self links are traversed. Wildcard-prefix candidates, class/runtime-dependent and invalid records are not followed. Edges include all exact links between reached nodes, including boundary cycles. Unresolved pages cover the whole admitted root map. Search filters displayed rows only; selection/framing use the complete bounded traversal, including roots, within the requested depth.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Outgoing,
    Incoming,
    Both,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Traversal {
    pub roots: Vec<u32>,
    pub direction: Direction,
    pub depth: usize,
}

#[derive(Debug, Clone)]
pub struct Query {
    pub snapshot: Option<String>,
    pub traversal: Traversal,
    pub search: String,
    pub node_offset: usize,
    pub edge_offset: usize,
    pub unresolved_offset: usize,
    pub limit: usize,
}

#[derive(Debug, Serialize)]
pub struct Page<T> {
    pub offset: usize,
    pub total: usize,
    pub next_offset: Option<usize>,
    pub rows: Vec<T>,
}
#[derive(Debug, Serialize)]
pub struct Node {
    pub id: String,
    pub classname: String,
    pub targetname: String,
    pub depth: usize,
    pub root: bool,
}
#[derive(Debug, Serialize)]
pub struct Edge {
    pub source: String,
    pub source_label: String,
    pub target: String,
    pub target_label: String,
    pub target_reference: String,
    pub connection_index: usize,
    pub output: String,
    pub input: String,
    pub parameter: String,
    pub delay: f32,
    pub times: i32,
    pub resolution: &'static str,
}
#[derive(Debug, Serialize)]
pub struct Unresolved {
    pub source: String,
    pub source_label: String,
    pub connection_index: usize,
    pub output: String,
    pub raw: String,
    pub status: &'static str,
    pub reason: String,
    pub candidates: Vec<String>,
    pub candidates_total: usize,
    pub candidates_truncated: bool,
}
#[derive(Debug, Serialize)]
pub struct Inspection {
    pub snapshot: String,
    pub generation: u64,
    pub revision: u64,
    pub roots: Vec<String>,
    pub direction: Direction,
    pub depth: usize,
    pub traversal_nodes: usize,
    pub traversal_edges: usize,
    pub depth_boundary_nodes: usize,
    pub unresolved_connections: usize,
    pub nodes: Page<Node>,
    pub edges: Page<Edge>,
    pub unresolved: Page<Unresolved>,
    pub scope: &'static str,
    pub limits: serde_json::Value,
}
#[derive(Debug, Serialize)]
pub struct Selection {
    pub snapshot: String,
    pub matched: usize,
    pub selected_count: usize,
}

pub struct Frame {
    pub snapshot: String,
    pub matched: usize,
    pub bounds: [[f64; 3]; 2],
    pub camera: render::Camera,
}

struct Record {
    source: usize,
    index: usize,
    spec: Option<connections::Spec>,
    status: &'static str,
    reason: String,
    candidates: Vec<u32>,
    candidates_total: usize,
}
struct Link {
    source: usize,
    target: usize,
    record: usize,
}
struct Graph<'a> {
    entities: Vec<&'a source_formats::vmf::Entity>,
    records: Vec<Record>,
    links: Vec<Link>,
    distances: Vec<Option<usize>>,
    roots: Vec<u32>,
    selected: Vec<usize>,
    graph_links: Vec<usize>,
    work: usize,
    live_bound: usize,
}

fn reserve<T>(values: &mut Vec<T>, count: usize) -> Result<(), String> {
    values
        .try_reserve_exact(count)
        .map_err(|_| "Entity graph storage cannot be allocated".into())
}
fn charge(work: &mut usize, amount: usize) -> Result<(), String> {
    *work = work
        .checked_add(amount)
        .filter(|&work| work <= MAX_WORK)
        .ok_or("Entity graph exceeds its bounded work budget; reduce map connections")?;
    Ok(())
}
fn storage_bound(nodes: usize, connections: usize, source_bytes: usize) -> Result<usize, String> {
    let bytes = nodes
        .checked_mul(
            size_of::<&source_formats::vmf::Entity>()
                + 128
                + 4 * size_of::<Vec<usize>>()
                + size_of::<Option<usize>>(),
        )
        .and_then(|bytes| {
            connections
                .checked_mul(size_of::<Record>() + 256)
                .and_then(|records| bytes.checked_add(records))
        })
        .and_then(|bytes| {
            bytes.checked_add(MAX_CONNECTIONS * (size_of::<Link>() + 4 * size_of::<usize>()))
        })
        .and_then(|bytes| bytes.checked_add(2 * source_bytes))
        .and_then(|bytes| bytes.checked_add(4 * MAX_REPORT_BYTES))
        .filter(|&bytes| bytes <= MAX_SCRATCH_BYTES)
        .ok_or("Entity graph exceeds its scratch-storage limit")?;
    Ok(bytes)
}

fn validate(traversal: &Traversal) -> Result<Vec<u32>, String> {
    if traversal.roots.is_empty() || traversal.roots.len() > MAX_ROOTS {
        return Err(format!(
            "Graph requires 1–{MAX_ROOTS} explicit root entity IDs"
        ));
    }
    if !(1..=MAX_DEPTH).contains(&traversal.depth) {
        return Err(format!("Graph depth must be 1–{MAX_DEPTH}"));
    }
    let mut roots = traversal.roots.clone();
    roots.sort_unstable();
    roots.dedup();
    Ok(roots)
}
fn page<T>(
    rows: Vec<usize>,
    offset: usize,
    limit: usize,
    build: impl Fn(usize) -> T,
) -> Result<Page<T>, String> {
    if offset > rows.len() {
        return Err("Entity graph page offset exceeds the filtered result count".into());
    }
    let total = rows.len();
    let end = offset.saturating_add(limit).min(total);
    let mut result = Vec::new();
    reserve(&mut result, end - offset)?;
    result.extend(rows.into_iter().skip(offset).take(limit).map(build));
    Ok(Page {
        offset,
        total,
        next_offset: (end < total).then_some(end),
        rows: result,
    })
}
fn id(entity: u32) -> String {
    crate::identity::id_string(ObjectId::Entity(entity))
}

fn label(entity: &source_formats::vmf::Entity) -> String {
    let class = entity.classname().unwrap_or("Unknown class");
    let name = entity.properties.get("targetname").unwrap_or_default();
    if name.is_empty() {
        class.into()
    } else {
        format!("{name} · {class}")
    }
}

struct ReportSize(usize);
impl Write for ReportSize {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self
            .0
            .checked_add(bytes.len())
            .filter(|&bytes| bytes <= MAX_REPORT_BYTES)
            .ok_or_else(|| io::Error::other("Entity graph report exceeds its storage limit"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Session {
    fn entity_graph_snapshot(&self, traversal: &Traversal, roots: &[u32]) -> String {
        let mut hash = Sha256::new();
        hash.update(self.buffer_review_scope.as_bytes());
        hash.update(self.document_generation.to_le_bytes());
        hash.update(self.document.revision().to_le_bytes());
        hash.update([traversal.direction as u8]);
        hash.update((traversal.depth as u64).to_le_bytes());
        for root in roots {
            hash.update(root.to_le_bytes());
        }
        format!("{:x}", hash.finalize())
    }

    fn entity_graph(&self, traversal: &Traversal) -> Result<Graph<'_>, String> {
        let roots = validate(traversal)?;
        let source = &self.document.vmf().entities;
        if source.len() > MAX_MAP_NODES {
            return Err(format!(
                "Entity graph admits at most {MAX_MAP_NODES} map entities"
            ));
        }
        let mut bytes = 0usize;
        let mut count = 0usize;
        let mut work = 0usize;
        for entity in source {
            for field in [
                entity.classname().unwrap_or_default(),
                entity.properties.get("targetname").unwrap_or_default(),
            ] {
                if field.len() > MAX_FIELD_BYTES {
                    return Err("Entity graph labels exceed the field byte limit".into());
                }
                bytes = bytes
                    .checked_add(field.len())
                    .ok_or("Entity graph byte count overflow")?;
            }
            for raw in entity.connections.iter().flatten() {
                count += 1;
                if count > MAX_CONNECTIONS {
                    return Err("Entity graph exceeds its connection-count limit".into());
                }
                if raw.output.len() > MAX_FIELD_BYTES || raw.value.len() > MAX_FIELD_BYTES {
                    return Err("Entity graph connection exceeds the field byte limit".into());
                }
                bytes = bytes
                    .checked_add(raw.output.len() + raw.value.len())
                    .ok_or("Entity graph byte count overflow")?;
            }
        }
        if bytes > MAX_SOURCE_BYTES || count > MAX_CONNECTIONS {
            return Err("Entity graph exceeds its source-byte or connection-count limit".into());
        }
        let live_bound = storage_bound(source.len(), count, bytes)?;
        charge(&mut work, bytes + source.len())?;
        let mut entities = Vec::new();
        reserve(&mut entities, source.len())?;
        entities.extend(source.iter().filter(|entity| !entity.hidden));
        let indices: BTreeMap<_, _> = entities
            .iter()
            .enumerate()
            .map(|(index, entity)| (entity.id, index))
            .collect();
        if indices.len() != entities.len() {
            return Err("Entity graph cannot resolve duplicate authored entity IDs".into());
        }
        for root in &roots {
            if !indices.contains_key(root) {
                return Err(format!("Graph root entity:{root} is missing or hidden"));
            }
        }
        let mut records = Vec::new();
        reserve(&mut records, count)?;
        let mut links = Vec::new();
        reserve(&mut links, MAX_CONNECTIONS)?;
        for (source_index, entity) in entities.iter().enumerate() {
            for (index, raw) in entity.connections.iter().flatten().enumerate() {
                charge(&mut work, source.len() + 1)?;
                let parsed = connections::parse(raw).and_then(|spec| {
                    spec.validate(connections::separator(raw))?;
                    Ok(spec)
                });
                let (spec, status, reason, candidates, candidates_total) = match parsed {
                    Err(error) => (None, "invalid", error, Vec::new(), 0),
                    Ok(spec) => {
                        let matches = connections::targets(&self.document, entity.id, &spec.target);
                        let total = matches.len();
                        let (status, reason) = if spec.target == "!self" {
                            ("self", String::new())
                        } else if spec.target.starts_with('!') {
                            (
                                "runtime_dependent",
                                "Special target is resolved at runtime".into(),
                            )
                        } else if spec.target.contains('*') {
                            ("wildcard_candidates", "Existing editor prefix matching supplies candidates, not engine-complete wildcard resolution".into())
                        } else if matches.is_empty() {
                            charge(&mut work, entities.len())?;
                            if entities.iter().any(|entity| {
                                entity
                                    .classname()
                                    .is_some_and(|class| class.eq_ignore_ascii_case(&spec.target))
                            }) {
                                ("class_or_runtime", "No visible named target matches; class/runtime lookup is not resolved by this authored graph".into())
                            } else {
                                (
                                    "missing",
                                    "No visible named target matches; it may be created at runtime"
                                        .into(),
                                )
                            }
                        } else {
                            ("named", String::new())
                        };
                        if matches!(status, "named" | "self") {
                            if links.len().saturating_add(matches.len()) > MAX_CONNECTIONS {
                                return Err(
                                    "Entity graph exact fan-out exceeds the map-edge limit".into(),
                                );
                            }
                            for target in &matches {
                                links.push(Link {
                                    source: source_index,
                                    target: indices[target],
                                    record: records.len(),
                                });
                            }
                        }
                        let mut candidates = Vec::new();
                        if !matches!(status, "named" | "self") {
                            reserve(&mut candidates, matches.len().min(MAX_CANDIDATES))?;
                            candidates.extend(matches.iter().take(MAX_CANDIDATES).copied());
                        }
                        (Some(spec), status, reason, candidates, total)
                    }
                };
                records.push(Record {
                    source: source_index,
                    index,
                    spec,
                    status,
                    reason,
                    candidates,
                    candidates_total,
                });
            }
        }
        let mut outgoing = Vec::new();
        reserve(&mut outgoing, entities.len())?;
        outgoing.resize_with(entities.len(), Vec::new);
        let mut incoming = Vec::new();
        reserve(&mut incoming, entities.len())?;
        incoming.resize_with(entities.len(), Vec::new);
        for (index, link) in links.iter().enumerate() {
            outgoing[link.source]
                .try_reserve(1)
                .map_err(|_| "Entity graph adjacency cannot be allocated")?;
            incoming[link.target]
                .try_reserve(1)
                .map_err(|_| "Entity graph adjacency cannot be allocated")?;
            outgoing[link.source].push(index);
            incoming[link.target].push(index);
        }
        let mut distances = Vec::new();
        reserve(&mut distances, entities.len())?;
        distances.resize(entities.len(), None);
        let mut queue = VecDeque::new();
        queue
            .try_reserve(MAX_GRAPH_NODES)
            .map_err(|_| "Entity graph queue cannot be allocated")?;
        let mut selected = Vec::new();
        reserve(&mut selected, MAX_GRAPH_NODES)?;
        for root in &roots {
            let index = indices[root];
            distances[index] = Some(0);
            queue.push_back(index);
            selected.push(index);
        }
        while let Some(index) = queue.pop_front() {
            let depth = distances[index].unwrap();
            if depth == traversal.depth {
                continue;
            }
            let candidates = outgoing[index]
                .iter()
                .filter(|_| traversal.direction != Direction::Incoming)
                .chain(
                    incoming[index]
                        .iter()
                        .filter(|_| traversal.direction != Direction::Outgoing),
                );
            for &link_index in candidates {
                charge(&mut work, 1)?;
                let link = &links[link_index];
                let other = if link.source == index {
                    link.target
                } else {
                    link.source
                };
                if distances[other].is_none() {
                    if selected.len() == MAX_GRAPH_NODES {
                        return Err(
                            "Entity graph traversal exceeds its node limit; reduce depth or roots"
                                .into(),
                        );
                    }
                    distances[other] = Some(depth + 1);
                    selected.push(other);
                    queue.push_back(other);
                }
            }
        }
        let mut graph_links = Vec::new();
        reserve(&mut graph_links, MAX_GRAPH_EDGES)?;
        for (index, link) in links.iter().enumerate() {
            charge(&mut work, 1)?;
            if distances[link.source].is_some() && distances[link.target].is_some() {
                if graph_links.len() == MAX_GRAPH_EDGES {
                    return Err(
                        "Entity graph traversal exceeds its edge limit; reduce depth or roots"
                            .into(),
                    );
                }
                graph_links.push(index);
            }
        }
        selected.sort_unstable_by_key(|&index| (distances[index], entities[index].id));
        Ok(Graph {
            entities,
            records,
            links,
            distances,
            roots,
            selected,
            graph_links,
            work,
            live_bound,
        })
    }

    pub fn inspect_entity_graph(&self, query: &Query) -> Result<Inspection, String> {
        if query.limit == 0 || query.limit > MAX_PAGE || query.search.len() > 128 {
            return Err("Graph page limit must be 1–64 and search at most 128 bytes".into());
        }
        let roots = validate(&query.traversal)?;
        let snapshot = self.entity_graph_snapshot(&query.traversal, &roots);
        if query
            .snapshot
            .as_ref()
            .is_some_and(|token| token != &snapshot)
            || (query.snapshot.is_none()
                && [
                    query.node_offset,
                    query.edge_offset,
                    query.unresolved_offset,
                ]
                .iter()
                .any(|&offset| offset != 0))
        {
            return Err("Retained graph pages require the current matching snapshot".into());
        }
        let graph = self.entity_graph(&query.traversal)?;
        let search = query.search.to_ascii_lowercase();
        let mut work = graph.work;
        let mut nodes = Vec::new();
        reserve(&mut nodes, graph.selected.len())?;
        for &index in &graph.selected {
            let entity = graph.entities[index];
            let identity = id(entity.id);
            let fields = [
                identity.as_str(),
                entity.classname().unwrap_or_default(),
                entity.properties.get("targetname").unwrap_or_default(),
            ];
            charge(&mut work, fields.iter().map(|field| field.len()).sum())?;
            if search.is_empty()
                || fields
                    .iter()
                    .any(|value| value.to_ascii_lowercase().contains(&search))
            {
                nodes.push(index);
            }
        }
        let mut edges = Vec::new();
        reserve(&mut edges, graph.graph_links.len())?;
        for &index in &graph.graph_links {
            let link = &graph.links[index];
            let spec = graph.records[link.record].spec.as_ref().unwrap();
            let source = id(graph.entities[link.source].id);
            let target = id(graph.entities[link.target].id);
            let fields = [
                source.as_str(),
                target.as_str(),
                spec.output.as_str(),
                spec.input.as_str(),
                spec.target.as_str(),
                graph.entities[link.source]
                    .properties
                    .get("targetname")
                    .unwrap_or_default(),
                graph.entities[link.source].classname().unwrap_or_default(),
                graph.entities[link.target].classname().unwrap_or_default(),
            ];
            charge(&mut work, fields.iter().map(|field| field.len()).sum())?;
            if search.is_empty()
                || fields
                    .iter()
                    .any(|value| value.to_ascii_lowercase().contains(&search))
            {
                edges.push(index);
            }
        }
        let mut unresolved = Vec::new();
        reserve(&mut unresolved, graph.records.len())?;
        for (index, record) in graph.records.iter().enumerate() {
            if matches!(record.status, "named" | "self") {
                continue;
            }
            let entity = graph.entities[record.source];
            let raw = &entity.connections.as_ref().unwrap()[record.index];
            let source = id(entity.id);
            let fields = [
                source.as_str(),
                raw.output.as_str(),
                raw.value.as_str(),
                record.reason.as_str(),
                record.status,
                entity.properties.get("targetname").unwrap_or_default(),
                entity.classname().unwrap_or_default(),
            ];
            charge(&mut work, fields.iter().map(|field| field.len()).sum())?;
            if search.is_empty()
                || fields
                    .iter()
                    .any(|value| value.to_ascii_lowercase().contains(&search))
            {
                unresolved.push(index);
            }
        }
        let report = Inspection {
            snapshot,
            generation: self.document_generation,
            revision: self.document.revision(),
            roots: graph.roots.iter().map(|&entity| id(entity)).collect(),
            direction: query.traversal.direction,
            depth: query.traversal.depth,
            traversal_nodes: graph.selected.len(),
            traversal_edges: graph.graph_links.len(),
            depth_boundary_nodes: graph
                .selected
                .iter()
                .filter(|&&index| graph.distances[index] == Some(query.traversal.depth))
                .count(),
            unresolved_connections: graph
                .records
                .iter()
                .filter(|record| !matches!(record.status, "named" | "self"))
                .count(),
            nodes: page(nodes, query.node_offset, query.limit, |index| {
                let entity = graph.entities[index];
                Node {
                    id: id(entity.id),
                    classname: entity.classname().unwrap_or_default().into(),
                    targetname: entity
                        .properties
                        .get("targetname")
                        .unwrap_or_default()
                        .into(),
                    depth: graph.distances[index].unwrap(),
                    root: graph.roots.contains(&entity.id),
                }
            })?,
            edges: page(edges, query.edge_offset, query.limit, |index| {
                let link = &graph.links[index];
                let record = &graph.records[link.record];
                let spec = record.spec.as_ref().unwrap();
                Edge {
                    source: id(graph.entities[link.source].id),
                    source_label: label(graph.entities[link.source]),
                    target: id(graph.entities[link.target].id),
                    target_label: label(graph.entities[link.target]),
                    target_reference: spec.target.clone(),
                    connection_index: record.index,
                    output: spec.output.clone(),
                    input: spec.input.clone(),
                    parameter: spec.parameter.clone(),
                    delay: spec.delay,
                    times: spec.times,
                    resolution: record.status,
                }
            })?,
            unresolved: page(unresolved, query.unresolved_offset, query.limit, |index| {
                let record = &graph.records[index];
                let entity = graph.entities[record.source];
                let raw = &entity.connections.as_ref().unwrap()[record.index];
                Unresolved {
                    source: id(entity.id),
                    source_label: label(entity),
                    connection_index: record.index,
                    output: raw.output.clone(),
                    raw: raw.value.clone(),
                    status: record.status,
                    reason: record.reason.clone(),
                    candidates: record.candidates.iter().map(|&entity| id(entity)).collect(),
                    candidates_total: record.candidates_total,
                    candidates_truncated: record.candidates_total > record.candidates.len(),
                }
            })?,
            scope: SCOPE,
            limits: serde_json::json!({"map_nodes":MAX_MAP_NODES,"map_connections":MAX_CONNECTIONS,"graph_nodes":MAX_GRAPH_NODES,"graph_edges":MAX_GRAPH_EDGES,"depth":MAX_DEPTH,"roots":MAX_ROOTS,"page":MAX_PAGE,"source_bytes":MAX_SOURCE_BYTES,"field_bytes":MAX_FIELD_BYTES,"work":MAX_WORK,"report_bytes":MAX_REPORT_BYTES,"candidate_ids":MAX_CANDIDATES,"scratch_bytes":MAX_SCRATCH_BYTES,"conservative_live_bytes":graph.live_bound,"work_unit":"source/search byte, target candidate or adjacency entry"}),
        };
        serde_json::to_writer(ReportSize(0), &report).map_err(|error| error.to_string())?;
        Ok(report)
    }

    fn entity_graph_action(
        &self,
        traversal: &Traversal,
        snapshot: &str,
    ) -> Result<Graph<'_>, String> {
        self.ensure_not_closing()?;
        let roots = validate(traversal)?;
        if snapshot.len() != 64 || snapshot != self.entity_graph_snapshot(traversal, &roots) {
            return Err("Entity graph snapshot changed; refresh the graph before acting".into());
        }
        if self.viewport_pick_conflict()
            || self.measurement_picking()
            || self.linked_path_picking()
            || self.block_preview.is_some()
            || !std::ptr::eq(self.render_document(), &self.document)
        {
            return Err("Finish or cancel active geometry drafts and viewport gestures before selecting or framing graph nodes".into());
        }
        self.entity_graph(traversal)
    }

    pub fn select_entity_graph(
        &mut self,
        actor: Actor,
        traversal: &Traversal,
        snapshot: &str,
        mode: SelectMode,
    ) -> Result<Selection, String> {
        let graph = self.entity_graph_action(traversal, snapshot)?;
        let ids = graph
            .selected
            .iter()
            .map(|&index| ObjectId::Entity(graph.entities[index].id))
            .collect::<Vec<_>>();
        let matched = ids.len();
        self.select(&ids, mode)?;
        self.log(actor, "selected connected entity graph");
        Ok(Selection {
            snapshot: snapshot.into(),
            matched,
            selected_count: self.selection.len(),
        })
    }

    pub fn frame_entity_graph(
        &mut self,
        actor: Actor,
        traversal: &Traversal,
        snapshot: &str,
        aspect: f32,
    ) -> Result<Frame, String> {
        if !aspect.is_finite() || !(0.01..=100.0).contains(&aspect) {
            return Err("Graph framing aspect must be finite and between 0.01 and 100".into());
        }
        let graph = self.entity_graph_action(traversal, snapshot)?;
        let ids = graph
            .selected
            .iter()
            .map(|&index| ObjectId::Entity(graph.entities[index].id))
            .collect::<Vec<_>>();
        let matched = ids.len();
        let presentation = self.presentation_bounds_in(&self.document, None);
        let mut bounds = Vec::new();
        reserve(&mut bounds, ids.len())?;
        for id in &ids {
            let (mins, maxs) = super::models::object_bounds(&self.document, *id, &presentation)
                .ok_or_else(|| format!("Graph node {id} has no framing bounds"))?;
            if !mins.is_finite() || !maxs.is_finite() || !mins.cmple(maxs).all() {
                return Err(format!(
                    "Graph node {id} has nonfinite or unordered framing bounds"
                ));
            }
            bounds.push((mins, maxs));
        }
        let (mins, maxs) =
            map_core::query::union(bounds).ok_or("Graph nodes have no framing bounds")?;
        let camera = self
            .camera
            .frame_bounds(mins.as_vec3(), maxs.as_vec3(), aspect);
        if !camera.position.is_finite() || !camera.view_projection(aspect).is_finite() {
            return Err("Graph nodes have no finite framing camera".into());
        }
        self.set_camera(camera);
        self.log(actor, "framed connected entity graph");
        Ok(Frame {
            snapshot: snapshot.into(),
            matched,
            bounds: [mins.to_array(), maxs.to_array()],
            camera,
        })
    }
}

#[cfg(test)]
mod tests;
