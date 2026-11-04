use std::collections::{BTreeMap, VecDeque};
use std::io::{self, Write};

use map_core::{ObjectId, connections};

mod diagram;
mod expanded;
mod resolve;
mod source;

pub(crate) use diagram::State;
pub use diagram::{
    Diagram, DiagramEdge, DiagramNode, Layout, NODE_HEIGHT, NODE_WIDTH, NodePosition, View,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use source::Source;

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
const SCOPE: &str = "Committed entities in the requested source scope; saved-hidden entities and unapplied drafts are excluded. Expanded scope uses checked instance preparation and exact source provenance, never an authored-scope fallback. Native event targets use name matches first and class fallback only when no names match, with the installed byte/wildcard rules. !self and !caller use the supplied event caller, defaulting to each output owner; !activator needs the supplied scenario identity. Player/picker and unknown dynamic identities stay unresolved. This is an authored scenario, not runtime execution. Search/pages filter displayed rows only; selection and framing use the complete bounded traversal. Virtual selections map to authored owners; framing uses prepared positions or refuses explicitly.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Outgoing,
    Incoming,
    Both,
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    #[default]
    Authored,
    Expanded,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Context {
    pub activator: Option<String>,
    pub caller: Option<String>,
    #[serde(default)]
    pub caller_unbound: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Traversal {
    pub scope: Scope,
    pub context: Context,
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
    pub owner: String,
    pub provenance: Option<serde_json::Value>,
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
    pub source_scope: Scope,
    pub context: Context,
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
    resolved: bool,
}
struct Link {
    source: usize,
    target: usize,
    record: usize,
}
struct Graph<'a> {
    source: Source<'a>,
    entities: Vec<usize>,
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
        .and_then(|bytes| bytes.checked_add(6 * MAX_REPORT_BYTES))
        .filter(|&bytes| bytes <= MAX_SCRATCH_BYTES)
        .ok_or("Entity graph exceeds its scratch-storage limit")?;
    Ok(bytes)
}

fn validate(traversal: &Traversal) -> Result<Vec<u32>, String> {
    if traversal.context.caller_unbound && traversal.context.caller.is_some() {
        return Err("Supply a caller ID or an unbound caller, not both".into());
    }
    if [&traversal.context.activator, &traversal.context.caller]
        .into_iter()
        .flatten()
        .any(|value| value.len() > MAX_FIELD_BYTES)
    {
        return Err("Graph scenario ID exceeds the field byte limit".into());
    }
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
    fn entity_graph_snapshot(&self, traversal: &Traversal, graph: &Graph<'_>) -> String {
        let mut hash = Sha256::new();
        hash.update(self.buffer_review_scope.as_bytes());
        hash.update(self.document_generation.to_le_bytes());
        hash.update(self.document.revision().to_le_bytes());
        hash.update([traversal.direction as u8, traversal.scope as u8]);
        for value in [&traversal.context.activator, &traversal.context.caller] {
            hash.update([u8::from(value.is_some())]);
            if let Some(value) = value {
                hash.update((value.len() as u64).to_le_bytes());
                hash.update(value.as_bytes());
            }
        }
        hash.update([u8::from(traversal.context.caller_unbound)]);
        graph.source.hash(&mut hash);
        hash.update((traversal.depth as u64).to_le_bytes());
        for root in &graph.roots {
            hash.update(root.to_le_bytes());
        }
        format!("{:x}", hash.finalize())
    }

    fn entity_graph(&self, traversal: &Traversal) -> Result<Graph<'_>, String> {
        let roots = validate(traversal)?;
        let prepared = Source::prepare(self, traversal.scope)?;
        let roots = prepared.seeds(&roots)?;
        if roots.is_empty() || roots.len() > MAX_GRAPH_NODES {
            return Err(
                "Expanded graph roots exceed the reached-node limit or contain no entities".into(),
            );
        }
        let source = &prepared.document().vmf().entities;
        if source.len() > MAX_MAP_NODES {
            return Err(format!(
                "Entity graph admits at most {MAX_MAP_NODES} map entities"
            ));
        }
        let scenario = resolve::Scenario::prepare(&prepared, &traversal.context)?;
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
        let live_bound = storage_bound(source.len(), count, bytes)?
            .checked_add(prepared.retained_bytes())
            .filter(|&bytes| bytes <= MAX_SCRATCH_BYTES)
            .ok_or("Expanded entity graph exceeds its retained scratch-storage limit")?;
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
                        let resolution = resolve::targets(
                            &prepared,
                            entity.id,
                            &spec.target,
                            &scenario,
                            &mut work,
                        )?;
                        let matches = resolution.matches;
                        let total = matches.len();
                        let status = resolution.status;
                        let reason = resolution.reason;
                        if resolution.resolved {
                            if links.len().saturating_add(matches.len()) > MAX_CONNECTIONS {
                                return Err(
                                    "Entity graph fan-out exceeds the map-edge limit".into()
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
                        if !resolve::is_resolved(status) {
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
                    resolved: resolve::is_resolved(status),
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
        charge(&mut work, source.len())?;
        drop(entities);
        let entities = source
            .iter()
            .enumerate()
            .filter_map(|(index, entity)| (!entity.hidden).then_some(index))
            .collect();
        Ok(Graph {
            source: prepared,
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
        let graph = self.entity_graph(&query.traversal)?;
        let snapshot = self.entity_graph_snapshot(&query.traversal, &graph);
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
        let search = query.search.to_ascii_lowercase();
        let mut work = graph.work;
        let mut nodes = Vec::new();
        reserve(&mut nodes, graph.selected.len())?;
        for &index in &graph.selected {
            let entity = graph.entity(index);
            let identity = graph.id(index);
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
            let source = graph.id(link.source);
            let target = graph.id(link.target);
            let fields = [
                source.as_str(),
                target.as_str(),
                spec.output.as_str(),
                spec.input.as_str(),
                spec.target.as_str(),
                graph
                    .entity(link.source)
                    .properties
                    .get("targetname")
                    .unwrap_or_default(),
                graph.entity(link.source).classname().unwrap_or_default(),
                graph.entity(link.target).classname().unwrap_or_default(),
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
            if record.resolved {
                continue;
            }
            let entity = graph.entity(record.source);
            let raw = &entity.connections.as_ref().unwrap()[record.index];
            let source = graph.id(record.source);
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
        nodes
            .iter()
            .skip(query.node_offset)
            .take(query.limit)
            .try_fold(0usize, |total, &index| {
                total.checked_add(graph.source.provenance_bound(graph.entity(index).id))
            })
            .filter(|&bytes| bytes <= MAX_REPORT_BYTES)
            .ok_or("Graph node provenance exceeds its report-storage limit")?;
        let report = Inspection {
            snapshot,
            generation: self.document_generation,
            revision: self.document.revision(),
            roots: validate(&query.traversal)?.into_iter().map(id).collect(),
            direction: query.traversal.direction,
            depth: query.traversal.depth,
            source_scope: query.traversal.scope,
            context: query.traversal.context.clone(),
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
                .filter(|record| !record.resolved)
                .count(),
            nodes: page(nodes, query.node_offset, query.limit, |index| {
                let entity = graph.entity(index);
                Node {
                    id: graph.id(index),
                    owner: id(graph.owner(index)),
                    provenance: graph.source.provenance(entity.id),
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
                    source: graph.id(link.source),
                    source_label: label(graph.entity(link.source)),
                    target: graph.id(link.target),
                    target_label: label(graph.entity(link.target)),
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
                let entity = graph.entity(record.source);
                let raw = &entity.connections.as_ref().unwrap()[record.index];
                Unresolved {
                    source: graph.id(record.source),
                    source_label: label(entity),
                    connection_index: record.index,
                    output: raw.output.clone(),
                    raw: raw.value.clone(),
                    status: record.status,
                    reason: record.reason.clone(),
                    candidates: record
                        .candidates
                        .iter()
                        .map(|&entity| graph.source.id(entity))
                        .collect(),
                    candidates_total: record.candidates_total,
                    candidates_truncated: record.candidates_total > record.candidates.len(),
                }
            })?,
            scope: SCOPE,
            limits: serde_json::json!({"map_nodes":MAX_MAP_NODES,"map_connections":MAX_CONNECTIONS,"graph_nodes":MAX_GRAPH_NODES,"graph_edges":MAX_GRAPH_EDGES,"depth":MAX_DEPTH,"roots":MAX_ROOTS,"page":MAX_PAGE,"source_bytes":MAX_SOURCE_BYTES,"field_bytes":MAX_FIELD_BYTES,"work":MAX_WORK,"report_bytes":MAX_REPORT_BYTES,"candidate_ids":MAX_CANDIDATES,"scratch_bytes":MAX_SCRATCH_BYTES,"conservative_live_bytes":graph.live_bound,"work_unit":"source/search byte, target candidate or adjacency entry"}),
        };
        graph.source.verify()?;
        serde_json::to_writer(ReportSize(0), &report).map_err(|error| error.to_string())?;
        Ok(report)
    }

    fn entity_graph_action(
        &self,
        traversal: &Traversal,
        snapshot: &str,
    ) -> Result<Graph<'_>, String> {
        self.ensure_not_closing()?;
        let graph = self.entity_graph(traversal)?;
        if snapshot.len() != 64 || snapshot != self.entity_graph_snapshot(traversal, &graph) {
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
        graph.source.verify()?;
        Ok(graph)
    }

    pub fn select_entity_graph(
        &mut self,
        actor: Actor,
        traversal: &Traversal,
        snapshot: &str,
        mode: SelectMode,
    ) -> Result<Selection, String> {
        self.select_entity_graph_impl(actor, traversal, snapshot, None, mode)
    }

    pub fn select_entity_graph_nodes(
        &mut self,
        actor: Actor,
        traversal: &Traversal,
        snapshot: &str,
        nodes: &[String],
        mode: SelectMode,
    ) -> Result<Selection, String> {
        self.select_entity_graph_impl(actor, traversal, snapshot, Some(nodes), mode)
    }

    fn select_entity_graph_impl(
        &mut self,
        actor: Actor,
        traversal: &Traversal,
        snapshot: &str,
        nodes: Option<&[String]>,
        mode: SelectMode,
    ) -> Result<Selection, String> {
        let graph = self.entity_graph_action(traversal, snapshot)?;
        let selected = graph.requested(nodes)?;
        let matched = selected.len();
        let mut owners = match &graph.source {
            Source::Authored(_) => selected
                .iter()
                .map(|&index| graph.entity(index).id)
                .collect::<Vec<_>>(),
            Source::Expanded(source) => source.selection(
                &selected
                    .iter()
                    .map(|&index| graph.entity(index).id)
                    .collect::<Vec<_>>(),
            )?,
        };
        owners.sort_unstable();
        owners.dedup();
        let ids = owners.into_iter().map(ObjectId::Entity).collect::<Vec<_>>();
        graph.source.verify()?;
        self.select(&ids, mode)?;
        self.log(actor, "selected entity graph nodes");
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
        self.frame_entity_graph_impl(actor, traversal, snapshot, None, aspect)
    }

    pub fn frame_entity_graph_nodes(
        &mut self,
        actor: Actor,
        traversal: &Traversal,
        snapshot: &str,
        nodes: &[String],
        aspect: f32,
    ) -> Result<Frame, String> {
        self.frame_entity_graph_impl(actor, traversal, snapshot, Some(nodes), aspect)
    }

    fn frame_entity_graph_impl(
        &mut self,
        actor: Actor,
        traversal: &Traversal,
        snapshot: &str,
        nodes: Option<&[String]>,
        aspect: f32,
    ) -> Result<Frame, String> {
        if !aspect.is_finite() || !(0.01..=100.0).contains(&aspect) {
            return Err("Graph framing aspect must be finite and between 0.01 and 100".into());
        }
        let graph = self.entity_graph_action(traversal, snapshot)?;
        let selected = graph.requested(nodes)?;
        let matched = selected.len();
        let mut bounds = match &graph.source {
            Source::Authored(_) => {
                let presentation = self.presentation_bounds_in(&self.document, None);
                let mut bounds = Vec::new();
                reserve(&mut bounds, selected.len())?;
                for &index in &selected {
                    bounds.push(
                        super::models::object_bounds(
                            &self.document,
                            ObjectId::Entity(graph.entity(index).id),
                            &presentation,
                        )
                        .ok_or_else(|| {
                            format!("Graph node {} has no framing bounds", graph.id(index))
                        })?,
                    );
                }
                bounds
            }
            Source::Expanded(source) => {
                let ids = selected
                    .iter()
                    .map(|&index| graph.entity(index).id)
                    .collect::<Vec<_>>();
                source.bounds(self, &ids)?
            }
        };
        if bounds.len() != matched {
            return Err("Graph framing requires bounds for every requested node".into());
        }
        for (mins, maxs) in &mut bounds {
            if !mins.is_finite() || !maxs.is_finite() || !mins.cmple(*maxs).all() {
                return Err("Graph node has nonfinite or unordered framing bounds".into());
            }
        }
        let (mins, maxs) =
            map_core::query::union(bounds).ok_or("Graph nodes have no framing bounds")?;
        let camera = self
            .camera
            .frame_bounds(mins.as_vec3(), maxs.as_vec3(), aspect);
        if !camera.position.is_finite() || !camera.view_projection(aspect).is_finite() {
            return Err("Graph nodes have no finite framing camera".into());
        }
        graph.source.verify()?;
        self.try_set_camera(camera)?;
        self.log(actor, "framed entity graph nodes");
        Ok(Frame {
            snapshot: snapshot.into(),
            matched,
            bounds: [mins.to_array(), maxs.to_array()],
            camera,
        })
    }
}

impl Graph<'_> {
    fn entity(&self, index: usize) -> &source_formats::vmf::Entity {
        &self.source.document().vmf().entities[self.entities[index]]
    }
    fn id(&self, index: usize) -> String {
        self.source.id(self.entity(index).id)
    }
    fn owner(&self, index: usize) -> u32 {
        self.source.owner(self.entity(index).id)
    }
    fn requested(&self, ids: Option<&[String]>) -> Result<Vec<usize>, String> {
        let Some(ids) = ids else {
            return Ok(self.selected.clone());
        };
        if ids.is_empty() || ids.len() > MAX_GRAPH_NODES {
            return Err("Graph node actions require 1–2048 node IDs".into());
        }
        let indices = self
            .selected
            .iter()
            .map(|&index| (self.id(index), index))
            .collect::<BTreeMap<_, _>>();
        let mut result = Vec::new();
        reserve(&mut result, ids.len())?;
        for id in ids {
            if id.len() > MAX_FIELD_BYTES {
                return Err("Graph node ID exceeds its byte limit".into());
            }
            let index = indices
                .get(id)
                .ok_or_else(|| format!("Graph node {id} is outside the current traversal"))?;
            result.push(*index);
        }
        result.sort_unstable();
        result.dedup();
        Ok(result)
    }
}
