use std::collections::{BTreeMap, BTreeSet};

use source_formats::keyvalues::Node;
use source_formats::vmf::Vmf;

use crate::subobject::{self, Crease, Partition, VertexSource};
use crate::{Document, EditError, Transaction};

pub const KEY: &str = "hammerr_partitions";
pub const MAX_PARTITIONS: usize = 64;
pub const MAX_TEXT_BYTES: usize = 64 * 1024;
pub const MAX_NODES: usize = 4096;
pub const MAX_DEPTH: usize = 64;
const KIND: &str = "opposing_midpoints";
const EDGE_KIND: &str = "opposing_edges";
const MATERIAL_KIND: &str = "opposing_midpoints_materials";
const EDGE_MATERIAL_KIND: &str = "opposing_edges_materials";
const BOUNDARY_KIND: &str = "boundary_edges";
const BOUNDARY_MATERIAL_KIND: &str = "boundary_edges_materials";
const POLYGON_KIND: &str = "polygon_boundary_edges";
const POLYGON_MATERIAL_KIND: &str = "polygon_boundary_edges_materials";
const MEMBERSHIP_FIELDS: [&str; 2] = ["piece_0_corner", "piece_1_corner"];
const MATERIAL_FIELDS: [&str; 2] = ["top_material_0", "top_material_1"];
const PARAMETERS_FIELD: &str = "edge_parameters";
const VERSION: &str = "1";
const RECORD_FIELDS: [&str; 5] = ["kind", "solid", "side", "edges", "corner"];

#[derive(Debug, Clone, PartialEq)]
pub struct Intent {
    pub source: Crease,
    pub adjacent: bool,
    pub edge_parameters: [f64; 2],
    pub top_materials: [Option<String>; 2],
    pub corners: Vec<Vec<u32>>,
    pub piece_membership: Option<[Vec<Vec<u32>>; 2]>,
    pub extra: Vec<Node>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Metadata {
    pub entries: BTreeMap<(u32, u32), Intent>,
    pub extra: Vec<Node>,
}

pub struct Resolution {
    pub source: Crease,
    pub result: Result<Partition, String>,
}

#[derive(Default)]
struct Budget {
    text: usize,
    nodes: usize,
}

impl Budget {
    fn visit(&mut self, node: &Node, depth: usize) -> Result<(), String> {
        if depth > MAX_DEPTH {
            return Err("Partition metadata nesting exceeds 64 levels".into());
        }
        self.nodes += 1;
        if self.nodes > MAX_NODES {
            return Err("Partition metadata exceeds 4096 nodes".into());
        }
        self.add_text(node.key.len())?;
        if let Some(value) = node.as_str() {
            self.add_text(value.len())?;
        } else {
            for child in node.children().unwrap() {
                self.visit(child, depth + 1)?;
            }
        }
        Ok(())
    }

    fn add_text(&mut self, bytes: usize) -> Result<(), String> {
        self.text = self
            .text
            .checked_add(bytes)
            .ok_or("Partition metadata text size overflow")?;
        if self.text > MAX_TEXT_BYTES {
            return Err("Partition metadata exceeds the shared 64 KiB text budget".into());
        }
        Ok(())
    }
}

fn field<'a>(nodes: &'a [Node], key: &str) -> Result<&'a str, String> {
    let mut found = nodes.iter().filter(|node| node.key == key);
    let value = found
        .next()
        .and_then(Node::as_str)
        .ok_or_else(|| format!("Partition metadata is missing {key}"))?;
    if found.next().is_some() {
        return Err(format!("Partition metadata repeats {key}"));
    }
    Ok(value)
}

fn id(value: &str) -> Result<u32, String> {
    let id = value
        .parse::<u32>()
        .map_err(|_| "Partition IDs must be positive 32-bit integers")?;
    if id == 0 {
        return Err("Partition IDs must be positive 32-bit integers".into());
    }
    Ok(id)
}

fn ids(value: &str, minimum: usize, maximum: usize) -> Result<Vec<u32>, String> {
    let mut result = Vec::new();
    for value in value.split_whitespace() {
        if result.len() == maximum {
            return Err("Partition incidence contains too many side IDs".into());
        }
        result.push(id(value)?);
    }
    result.sort_unstable();
    if result.len() < minimum || result.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err("Partition incidence needs distinct source side IDs".into());
    }
    Ok(result)
}

fn encoded(ids: &[u32]) -> String {
    ids.iter().map(u32::to_string).collect::<Vec<_>>().join(" ")
}

pub(crate) fn checked_top_material(material: &str) -> Result<&str, String> {
    if material.len() > MAX_TEXT_BYTES {
        return Err("Partition metadata exceeds the shared 64 KiB text budget".into());
    }
    if material.trim().is_empty() || material.chars().any(char::is_control) {
        return Err("Planned top materials need nonempty names without control characters".into());
    }
    Ok(material.trim())
}

pub fn normalize_top_material(material: &str) -> Result<String, String> {
    checked_top_material(material).map(str::to_ascii_uppercase)
}

impl Intent {
    fn from_partition(partition: &Partition) -> Self {
        Self {
            source: partition.source.clone(),
            adjacent: partition.source_corner_count() == 4
                && partition
                    .loops
                    .iter()
                    .any(|piece| piece.vertices.len() != 4),
            edge_parameters: partition.cut().unwrap().edge_parameters,
            top_materials: [None, None],
            corners: partition.vertices[..partition.source_corner_count()]
                .iter()
                .map(|vertex| match &vertex.source {
                    VertexSource::Corner(sides) => sides.clone(),
                    _ => unreachable!(),
                })
                .collect(),
            piece_membership: (partition.source_corner_count() != 4).then(|| {
                partition
                    .loops
                    .each_ref()
                    .map(|piece| piece_corners(partition, piece).into_iter().collect())
            }),
            extra: Vec::new(),
        }
    }

    fn validate(&self) -> Result<(), String> {
        for material in self.top_materials.iter().flatten() {
            checked_top_material(material)?;
        }
        if self
            .edge_parameters
            .iter()
            .any(|parameter| !parameter.is_finite() || *parameter <= 0.0 || *parameter >= 1.0)
        {
            return Err(
                "Partition edge positions must be finite and strictly between 0 and 1".into(),
            );
        }
        if self.source.solid == 0
            || self.source.side == 0
            || self.source.edges[0] == 0
            || self.source.edges[0] >= self.source.edges[1]
            || self.source.edges.contains(&self.source.side)
        {
            return Err("Partition source IDs must be distinct, positive and canonical".into());
        }
        let count = self.corners.len();
        let polygon = self.piece_membership.is_some();
        if (polygon
            && (!(3..=subobject::MAX_BOUNDARY_SOURCE_CORNERS).contains(&count)
                || count == 4
                || self.adjacent))
            || (!polygon && count != 4)
        {
            return Err("Partition kind disagrees with its bounded source corner count".into());
        }
        if self
            .corners
            .iter()
            .any(|corner| !(3..=subobject::MAX_SIDES).contains(&corner.len()))
        {
            return Err("Partition corner incidence exceeds its bounded source".into());
        }
        let second = if polygon {
            (1..count)
                .find(|index| {
                    self.corners[*index].contains(&self.source.edges[1])
                        && self.corners[(*index + 1) % count].contains(&self.source.edges[1])
                })
                .ok_or("Polygon partition second edge incidence is invalid")?
        } else if self.adjacent {
            if self.corners[0].contains(&self.source.edges[1]) {
                3
            } else {
                1
            }
        } else {
            2
        };
        for (index, corner) in self.corners.iter().enumerate() {
            if !(3..=subobject::MAX_SIDES).contains(&corner.len())
                || corner.first() == Some(&0)
                || corner.windows(2).any(|pair| pair[0] >= pair[1])
                || !corner.contains(&self.source.side)
                || corner.contains(&self.source.edges[0]) != (index < 2)
                || corner.contains(&self.source.edges[1])
                    != (index == second || index == (second + 1) % count)
            {
                return Err("Partition corner incidence is invalid".into());
            }
        }
        if self.corners.iter().collect::<BTreeSet<_>>().len() != count {
            return Err("Partition needs distinct source corners".into());
        }
        if let Some(membership) = &self.piece_membership {
            if membership.iter().any(|piece| {
                piece.is_empty()
                    || piece.len() >= count
                    || piece
                        .iter()
                        .any(|corner| !(3..=subobject::MAX_SIDES).contains(&corner.len()))
            }) || membership.iter().map(Vec::len).sum::<usize>() != count
            {
                return Err("Polygon piece corner membership exceeds its bounded source".into());
            }
            let actual = membership
                .each_ref()
                .map(|piece| piece.iter().collect::<BTreeSet<_>>());
            if membership
                .iter()
                .zip(&actual)
                .any(|(piece, unique)| piece.len() != unique.len())
            {
                return Err("Polygon piece corner membership repeats a source corner".into());
            }
            let first = self.corners[1..=second].iter().collect::<BTreeSet<_>>();
            let remaining = self
                .corners
                .iter()
                .filter(|corner| !first.contains(corner))
                .collect::<BTreeSet<_>>();
            let expected = [first, remaining];
            if actual != expected && actual != [expected[1].clone(), expected[0].clone()] {
                return Err(
                    "Polygon piece corner membership does not cover the boundary cut".into(),
                );
            }
        }
        if self.extra.iter().any(|node| {
            RECORD_FIELDS.contains(&node.key.as_str())
                || ((polygon || self.adjacent || self.edge_parameters != [0.5; 2])
                    && node.key == PARAMETERS_FIELD)
                || (self.top_materials.iter().any(Option::is_some)
                    && MATERIAL_FIELDS.contains(&node.key.as_str()))
                || (polygon && MEMBERSHIP_FIELDS.contains(&node.key.as_str()))
        }) {
            return Err("Partition extra metadata repeats a reserved field".into());
        }
        Ok(())
    }

    fn node(&self, include_extra: bool) -> Node {
        let mut fields = vec![
            Node::string(
                "kind",
                if self.piece_membership.is_some() {
                    if self.top_materials.iter().any(Option::is_some) {
                        POLYGON_MATERIAL_KIND
                    } else {
                        POLYGON_KIND
                    }
                } else {
                    match (
                        self.adjacent,
                        self.edge_parameters == [0.5; 2],
                        self.top_materials.iter().any(Option::is_some),
                    ) {
                        (true, _, false) => BOUNDARY_KIND,
                        (true, _, true) => BOUNDARY_MATERIAL_KIND,
                        (false, true, false) => KIND,
                        (false, false, false) => EDGE_KIND,
                        (false, true, true) => MATERIAL_KIND,
                        (false, false, true) => EDGE_MATERIAL_KIND,
                    }
                },
            ),
            Node::string("solid", self.source.solid.to_string()),
            Node::string("side", self.source.side.to_string()),
            Node::string("edges", encoded(&self.source.edges)),
        ];
        if self.piece_membership.is_some() || self.adjacent || self.edge_parameters != [0.5; 2] {
            fields.push(Node::string(
                PARAMETERS_FIELD,
                format!("{} {}", self.edge_parameters[0], self.edge_parameters[1]),
            ));
        }
        for (key, material) in MATERIAL_FIELDS.iter().zip(&self.top_materials) {
            if let Some(material) = material {
                fields.push(Node::string(*key, material));
            }
        }
        fields.extend(
            self.corners
                .iter()
                .map(|corner| Node::string("corner", encoded(corner))),
        );
        if let Some(membership) = &self.piece_membership {
            for (key, piece) in MEMBERSHIP_FIELDS.iter().zip(membership) {
                fields.extend(
                    piece
                        .iter()
                        .map(|corner| Node::string(*key, encoded(corner))),
                );
            }
        }
        if include_extra {
            fields.extend(self.extra.clone());
        }
        Node::block("partition", fields)
    }

    fn resolve(&self, document: &Document) -> Result<Partition, String> {
        self.validate()?;
        let mut partition = subobject::plan_partitions_at(
            document,
            &[subobject::PartitionCut {
                source: self.source.clone(),
                edge_parameters: self.edge_parameters,
            }],
        )
        .map_err(|error| error.to_string())?
        .remove(0);
        let current = Self::from_partition(&partition);
        if current.adjacent != self.adjacent
            || current.piece_membership.is_some() != self.piece_membership.is_some()
            || current.corners.iter().collect::<BTreeSet<_>>()
                != self.corners.iter().collect::<BTreeSet<_>>()
        {
            return Err(format!(
                "Brush {}, face {}: saved partition corner incidence changed",
                self.source.solid, self.source.side
            ));
        }
        let expected = if let Some(membership) = &self.piece_membership {
            membership
                .each_ref()
                .map(|piece| piece.iter().cloned().collect::<BTreeSet<_>>())
        } else {
            let first = if self.adjacent {
                self.corners
                    .iter()
                    .filter(|corner| self.source.edges.iter().all(|edge| corner.contains(edge)))
                    .cloned()
                    .collect::<BTreeSet<_>>()
            } else {
                [self.corners[0].clone(), self.corners[3].clone()]
                    .into_iter()
                    .collect()
            };
            let second = self
                .corners
                .iter()
                .filter(|corner| !first.contains(*corner))
                .cloned()
                .collect();
            [first, second]
        };
        let actual = partition
            .loops
            .each_ref()
            .map(|piece| piece_corners(&partition, piece));
        if actual == [expected[1].clone(), expected[0].clone()] {
            partition.loops.swap(0, 1);
            partition.loops[0].id = 0;
            partition.loops[1].id = 1;
        } else if actual != expected {
            return Err(format!(
                "Brush {}, face {}: saved partition piece incidence changed",
                self.source.solid, self.source.side
            ));
        }
        Ok(partition)
    }
}

impl Metadata {
    pub fn parse(vmf: &Vmf) -> Result<Self, String> {
        let mut blocks = vmf.extra.iter().filter(|node| node.key == KEY);
        let Some(block) = blocks.next() else {
            return Ok(Self::default());
        };
        if blocks.next().is_some() {
            return Err("Map contains more than one partition metadata block".into());
        }
        Budget::default().visit(block, 1)?;
        let children = block
            .children()
            .ok_or("Partition metadata must be a block")?;
        if field(children, "version")? != VERSION {
            return Err(
                "Unsupported partition metadata version; raw metadata has been preserved".into(),
            );
        }
        let mut result = Self::default();
        for node in children {
            if node.key == "version" {
                continue;
            }
            if node.key != "partition" {
                result.extra.push(node.clone());
                continue;
            }
            if result.entries.len() == MAX_PARTITIONS {
                return Err("Map supports at most 64 saved flat partitions".into());
            }
            let fields = node.children().ok_or("Partition entry must be a block")?;
            let kind = field(fields, "kind")?;
            if ![
                KIND,
                EDGE_KIND,
                MATERIAL_KIND,
                EDGE_MATERIAL_KIND,
                BOUNDARY_KIND,
                BOUNDARY_MATERIAL_KIND,
                POLYGON_KIND,
                POLYGON_MATERIAL_KIND,
            ]
            .contains(&kind)
            {
                return Err(
                    "Unsupported partition intent kind; raw metadata has been preserved".into(),
                );
            }
            let adjacent = kind == BOUNDARY_KIND || kind == BOUNDARY_MATERIAL_KIND;
            let polygon = kind == POLYGON_KIND || kind == POLYGON_MATERIAL_KIND;
            let adjusted = polygon || adjacent || kind == EDGE_KIND || kind == EDGE_MATERIAL_KIND;
            let styled = kind == MATERIAL_KIND
                || kind == EDGE_MATERIAL_KIND
                || kind == BOUNDARY_MATERIAL_KIND
                || kind == POLYGON_MATERIAL_KIND;
            let edge_parameters = if adjusted {
                let values = field(fields, PARAMETERS_FIELD)?
                    .split_whitespace()
                    .take(3)
                    .map(str::parse::<f64>)
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|_| "Partition edge positions must be numeric")?;
                values
                    .try_into()
                    .map_err(|_| "Partition needs exactly two edge positions")?
            } else {
                [0.5; 2]
            };
            let mut top_materials = [None, None];
            if styled {
                for (index, key) in MATERIAL_FIELDS.iter().enumerate() {
                    if fields.iter().any(|node| node.key == *key) {
                        top_materials[index] = Some(field(fields, key)?.to_owned());
                    }
                }
                if top_materials.iter().all(Option::is_none) {
                    return Err(
                        "Material partition intent needs at least one planned top material".into(),
                    );
                }
            }
            let edges = ids(field(fields, "edges")?, 2, 2)?.try_into().unwrap();
            let mut corners = Vec::new();
            for corner in fields.iter().filter(|field| field.key == "corner") {
                if corners.len()
                    == if polygon {
                        subobject::MAX_BOUNDARY_SOURCE_CORNERS
                    } else {
                        4
                    }
                {
                    return Err("Partition source corner incidence exceeds its kind's limit".into());
                }
                corners.push(ids(
                    corner
                        .as_str()
                        .ok_or("Partition corner must be a side-ID list")?,
                    3,
                    subobject::MAX_SIDES,
                )?);
            }
            let piece_membership = if polygon {
                let mut membership = [Vec::new(), Vec::new()];
                let mut total = 0;
                for (key, piece) in MEMBERSHIP_FIELDS.iter().zip(&mut membership) {
                    for corner in fields.iter().filter(|field| field.key == *key) {
                        if piece.len() == subobject::MAX_BOUNDARY_SOURCE_CORNERS - 1
                            || total == subobject::MAX_BOUNDARY_SOURCE_CORNERS
                        {
                            return Err(
                                "Polygon piece corner membership exceeds its bounded source".into(),
                            );
                        }
                        piece.push(ids(
                            corner
                                .as_str()
                                .ok_or("Polygon piece corner must be a side-ID list")?,
                            3,
                            subobject::MAX_SIDES,
                        )?);
                        total += 1;
                    }
                }
                Some(membership)
            } else {
                None
            };
            let source = Crease {
                solid: id(field(fields, "solid")?)?,
                side: id(field(fields, "side")?)?,
                edges,
            };
            let intent = Intent {
                source,
                adjacent,
                edge_parameters,
                top_materials,
                corners,
                piece_membership,
                extra: fields
                    .iter()
                    .filter(|field| {
                        !RECORD_FIELDS.contains(&field.key.as_str())
                            && !(adjusted && field.key == PARAMETERS_FIELD)
                            && !(styled && MATERIAL_FIELDS.contains(&field.key.as_str()))
                            && !(polygon && MEMBERSHIP_FIELDS.contains(&field.key.as_str()))
                    })
                    .cloned()
                    .collect(),
            };
            intent.validate()?;
            let key = (intent.source.solid, intent.source.side);
            if result.entries.insert(key, intent).is_some() {
                return Err("More than one partition is attached to a source face".into());
            }
        }
        result.validate()?;
        Ok(result)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.entries.len() > MAX_PARTITIONS {
            return Err("Map supports at most 64 saved flat partitions".into());
        }
        if self
            .extra
            .iter()
            .any(|node| matches!(node.key.as_str(), "version" | "partition"))
        {
            return Err("Partition extra metadata repeats a reserved field".into());
        }
        let mut budget = Budget::default();
        for node in &self.extra {
            budget.visit(node, 2)?;
        }
        for (key, intent) in &self.entries {
            intent.validate()?;
            if *key != (intent.source.solid, intent.source.side) {
                return Err("Partition entry key disagrees with its source IDs".into());
            }
            for node in &intent.extra {
                budget.visit(node, 3)?;
            }
        }
        budget.visit(&self.node(false), 1)
    }

    fn node(&self, include_extra: bool) -> Node {
        let mut fields = vec![Node::string("version", VERSION)];
        fields.extend(
            self.entries
                .values()
                .map(|intent| intent.node(include_extra)),
        );
        if include_extra {
            fields.extend(self.extra.clone());
        }
        Node::block(KEY, fields)
    }

    pub fn write(&self, tx: &mut Transaction) -> Result<(), EditError> {
        Self::parse(tx.vmf()).map_err(EditError::Invalid)?;
        self.validate().map_err(EditError::Invalid)?;
        let node = if self.entries.is_empty() && self.extra.is_empty() {
            None
        } else {
            Some(self.node(true))
        };
        tx.set_editor_metadata(KEY, node);
        Ok(())
    }

    pub fn resolve(&self, document: &Document) -> Vec<Resolution> {
        self.entries
            .values()
            .map(|intent| Resolution {
                source: intent.source.clone(),
                result: intent.resolve(document),
            })
            .collect()
    }
}

pub fn store(tx: &mut Transaction, request: &Crease) -> Result<(), EditError> {
    let partition =
        subobject::plan_partitions(tx.document(), std::slice::from_ref(request))?.remove(0);
    store_partition(tx, &partition)
}

pub fn store_partition(tx: &mut Transaction, partition: &Partition) -> Result<(), EditError> {
    store_partition_plan(tx, partition, None)
}

pub fn store_partition_materials(
    tx: &mut Transaction,
    partition: &Partition,
    materials: &[Option<String>; 2],
) -> Result<(), EditError> {
    store_partition_plan(tx, partition, Some(materials))
}

fn piece_corners(partition: &Partition, piece: &subobject::PartitionLoop) -> BTreeSet<Vec<u32>> {
    piece
        .vertices
        .iter()
        .filter_map(
            |vertex| match &partition.vertices[*vertex as usize].source {
                VertexSource::Corner(sides) => Some(sides.clone()),
                _ => None,
            },
        )
        .collect()
}

fn store_partition_plan(
    tx: &mut Transaction,
    partition: &Partition,
    materials: Option<&[Option<String>; 2]>,
) -> Result<(), EditError> {
    if let Some(materials) = materials {
        for material in materials.iter().flatten() {
            checked_top_material(material).map_err(EditError::Invalid)?;
        }
    }
    let mut metadata = Metadata::parse(tx.vmf()).map_err(EditError::Invalid)?;
    subobject::checked_partition(tx.document(), partition)?;
    let mut intent = Intent::from_partition(partition);
    let key = (partition.source.solid, partition.source.side);
    if let Some(previous) = metadata.entries.get(&key) {
        intent.extra = previous.extra.clone();
        if previous.source == intent.source && previous.resolve(tx.document()).is_ok() {
            intent.corners = previous.corners.clone();
            intent.piece_membership = previous.piece_membership.clone();
            intent.top_materials = previous.top_materials.clone();
        } else if previous.top_materials.iter().any(Option::is_some) {
            return Err(EditError::Invalid(
                "Saved piece materials require unchanged source incidence and cut direction".into(),
            ));
        }
    }
    if let Some(materials) = materials {
        let resolved = intent.resolve(tx.document()).map_err(EditError::Invalid)?;
        for piece in &partition.loops {
            let corners = piece_corners(partition, piece);
            let target = resolved
                .loops
                .iter()
                .find(|target| piece_corners(&resolved, target) == corners)
                .ok_or_else(|| {
                    EditError::Invalid("Planned material piece incidence changed".into())
                })?;
            intent.top_materials[target.id as usize] = materials[piece.id as usize].clone();
        }
    }
    metadata.entries.insert(key, intent);
    metadata.write(tx)
}

pub fn set_piece_top_materials(
    tx: &mut Transaction,
    source: &Crease,
    pieces: &[u32],
    material: Option<&str>,
) -> Result<(), EditError> {
    if pieces.is_empty()
        || pieces.len() > 2
        || pieces.iter().any(|piece| *piece > 1)
        || pieces.iter().collect::<BTreeSet<_>>().len() != pieces.len()
    {
        return Err(EditError::Invalid(
            "Choose distinct saved piece IDs 0 or 1".into(),
        ));
    }
    let material = material
        .map(checked_top_material)
        .transpose()
        .map_err(EditError::Invalid)?
        .map(str::to_ascii_uppercase);
    let mut metadata = Metadata::parse(tx.vmf()).map_err(EditError::Invalid)?;
    let intent = metadata
        .entries
        .get_mut(&(source.solid, source.side))
        .ok_or_else(|| EditError::Invalid("Saved partition does not exist".into()))?;
    if &intent.source != source {
        return Err(EditError::Invalid(
            "Saved partition cut direction changed".into(),
        ));
    }
    intent.resolve(tx.document()).map_err(EditError::Invalid)?;
    for piece in pieces {
        intent.top_materials[*piece as usize] = material.clone();
    }
    metadata.write(tx)
}

pub fn remove(tx: &mut Transaction, solid: u32, side: u32) -> Result<(), EditError> {
    let mut metadata = Metadata::parse(tx.vmf()).map_err(EditError::Invalid)?;
    if metadata.entries.remove(&(solid, side)).is_none() {
        return Err(EditError::Invalid("Saved partition does not exist".into()));
    }
    metadata.write(tx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ObjectId, edit, transform::Transform};
    use glam::DVec3;

    fn fixture() -> (Document, Crease) {
        let mut document = Document::empty();
        let ObjectId::Solid(solid) = document
            .edit("block", |tx| {
                edit::create_block(
                    tx,
                    DVec3::new(0.125, -7.25, 16.5),
                    DVec3::new(64.875, 48.125, 80.25),
                    "brick/test",
                )
            })
            .unwrap()
        else {
            unreachable!()
        };
        let source = document.solid(solid).unwrap();
        let side = source
            .sides
            .iter()
            .find(|side| crate::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
            .unwrap()
            .id;
        let edges = subobject::crease_pairs(source, side).unwrap()[0].edges;
        (document, Crease { solid, side, edges })
    }

    fn membership(partition: &Partition, piece: usize) -> BTreeSet<Vec<u32>> {
        partition.loops[piece]
            .vertices
            .iter()
            .filter_map(
                |vertex| match &partition.vertices[*vertex as usize].source {
                    VertexSource::Corner(sides) => Some(sides.clone()),
                    _ => None,
                },
            )
            .collect()
    }

    #[test]
    fn metadata_store_remove_preserves_planes_allocation_unknown_nodes_and_history() {
        let (document, request) = fixture();
        let mut vmf = document.vmf().clone();
        vmf.extra
            .push(Node::block("foreign", vec![Node::string("key", "keep")]));
        let mut document = Document::new(vmf);
        let source = document.vmf().clone();
        let control = document.clone();
        document.edit("store", |tx| store(tx, &request)).unwrap();
        let mut metadata = Metadata::parse(document.vmf()).unwrap();
        metadata.extra.push(Node::block(
            "future",
            vec![Node::string("a", "1"), Node::string("a", "2")],
        ));
        metadata
            .entries
            .get_mut(&(request.solid, request.side))
            .unwrap()
            .extra
            .push(Node::block("vendor", vec![Node::string("keep", "yes")]));
        document.edit("extra", |tx| metadata.write(tx)).unwrap();
        let metadata = Metadata::parse(document.vmf()).unwrap();
        assert_eq!(metadata.entries.len(), 1);
        assert_eq!(document.vmf().world, source.world);
        assert_eq!(document.vmf().entities, source.entities);
        assert_eq!(document.vmf().extra[0], source.extra[0]);
        let resolved = metadata.resolve(&document).remove(0).result.unwrap();
        assert_eq!(resolved.source, request);
        assert_eq!(resolved.vertices.len(), 6);
        let written = document.vmf().write(&Default::default());
        let reopened = Vmf::parse(&written).unwrap();
        assert_eq!(Metadata::parse(&reopened).unwrap(), metadata);
        reopened.validate_ids().unwrap();
        let applied = document.vmf().clone();
        document.undo().unwrap();
        document.redo().unwrap();
        assert_eq!(document.vmf(), &applied);
        let mut original_ids = control;
        let mut after_ids = document.clone();
        assert_eq!(
            original_ids
                .edit("object ID", |tx| tx.new_object_id())
                .unwrap(),
            after_ids
                .edit("object ID", |tx| tx.new_object_id())
                .unwrap()
        );
        assert_eq!(
            original_ids.edit("side ID", |tx| tx.new_side_id()).unwrap(),
            after_ids.edit("side ID", |tx| tx.new_side_id()).unwrap()
        );
        let mut alternate = request.clone();
        alternate.edges = subobject::crease_pairs(
            subobject::solid(&document, request.solid).unwrap().1,
            request.side,
        )
        .unwrap()[1]
            .edges;
        document
            .edit("replace", |tx| store(tx, &alternate))
            .unwrap();
        assert_eq!(
            Metadata::parse(document.vmf())
                .unwrap()
                .entries
                .values()
                .next()
                .unwrap()
                .extra,
            metadata.entries.values().next().unwrap().extra
        );
        document
            .edit("remove", |tx| remove(tx, request.solid, request.side))
            .unwrap();
        let empty = Metadata::parse(document.vmf()).unwrap();
        assert!(empty.entries.is_empty());
        assert_eq!(empty.extra, metadata.extra);
        let before = document.vmf().clone();
        assert!(
            document
                .edit("remove missing", |tx| remove(
                    tx,
                    request.solid,
                    request.side
                ))
                .is_err()
        );
        assert_eq!(document.vmf(), &before);
        let (mut clean, request) = fixture();
        clean.edit("store", |tx| store(tx, &request)).unwrap();
        clean
            .edit("remove", |tx| remove(tx, request.solid, request.side))
            .unwrap();
        assert!(!clean.vmf().extra.iter().any(|node| node.key == KEY));
    }

    #[test]
    fn piece_materials_roundtrip_keep_reflect_and_reset_without_geometry_changes() {
        for direction in 0..2 {
            let (mut document, mut source) = fixture();
            source.edges =
                subobject::crease_pairs(document.solid(source.solid).unwrap(), source.side)
                    .unwrap()[direction]
                    .edges;
            let partition = subobject::plan_partitions_at(
                &document,
                &[subobject::PartitionCut {
                    source: source.clone(),
                    edge_parameters: [0.2, 0.8],
                }],
            )
            .unwrap()
            .remove(0);
            document
                .edit("keep", |tx| store_partition(tx, &partition))
                .unwrap();
            let mut metadata = Metadata::parse(document.vmf()).unwrap();
            metadata
                .entries
                .get_mut(&(source.solid, source.side))
                .unwrap()
                .extra
                .push(Node::block("vendor", vec![Node::string("keep", "yes")]));
            document.edit("unknown", |tx| metadata.write(tx)).unwrap();
            let before = document.vmf().clone();
            let mut allocation = document.clone();
            document
                .edit("materials", |tx| {
                    set_piece_top_materials(tx, &source, &[0], Some(" brick/red "))?;
                    set_piece_top_materials(tx, &source, &[1], Some("wood/blue"))
                })
                .unwrap();
            let applied = document.vmf().clone();
            assert_eq!(applied.world, before.world);
            assert_eq!(applied.entities, before.entities);
            let mut after_allocation = document.clone();
            assert_eq!(
                allocation
                    .edit("IDs", |tx| Ok((tx.new_object_id()?, tx.new_side_id()?)))
                    .unwrap(),
                after_allocation
                    .edit("IDs", |tx| Ok((tx.new_object_id()?, tx.new_side_id()?)))
                    .unwrap()
            );
            document.undo().unwrap();
            assert_eq!(document.vmf(), &before);
            document.redo().unwrap();
            assert_eq!(document.vmf(), &applied);
            let original = Metadata::parse(document.vmf()).unwrap();
            assert_eq!(
                original.entries[&(source.solid, source.side)].top_materials,
                [Some("BRICK/RED".into()), Some("WOOD/BLUE".into())]
            );
            let reopened = Vmf::parse(&applied.write(&Default::default())).unwrap();
            assert_eq!(Metadata::parse(&reopened).unwrap(), original);
            let mut changed = Document::new(reopened);
            changed
                .edit("reflect", |tx| {
                    edit::transform(
                        tx,
                        &[ObjectId::Solid(source.solid)],
                        Transform {
                            reflection: [true, false, false],
                            rotation: DVec3::new(17.0, -23.0, 31.0),
                            ..Default::default()
                        },
                    )
                })
                .unwrap();
            let resolved = original.resolve(&changed).remove(0).result.unwrap();
            for piece in 0..2 {
                assert_eq!(membership(&partition, piece), membership(&resolved, piece));
            }
            changed
                .edit("keep reflected", |tx| store_partition(tx, &resolved))
                .unwrap();
            assert_eq!(Metadata::parse(changed.vmf()).unwrap(), original);
            let next = subobject::plan_partitions_at(
                &changed,
                &[subobject::PartitionCut {
                    source: source.clone(),
                    edge_parameters: [0.3, 0.7],
                }],
            )
            .unwrap()
            .remove(0);
            changed
                .edit("adjust", |tx| store_partition(tx, &next))
                .unwrap();
            let adjusted = Metadata::parse(changed.vmf()).unwrap();
            assert_eq!(
                adjusted.entries[&(source.solid, source.side)].top_materials,
                original.entries[&(source.solid, source.side)].top_materials
            );
            assert_eq!(
                adjusted.entries[&(source.solid, source.side)].corners,
                original.entries[&(source.solid, source.side)].corners
            );
            changed
                .edit("inherit first", |tx| {
                    set_piece_top_materials(tx, &source, &[0], None)
                })
                .unwrap();
            assert_eq!(
                Metadata::parse(changed.vmf()).unwrap().entries[&(source.solid, source.side)]
                    .top_materials,
                [None, Some("WOOD/BLUE".into())]
            );
            changed
                .edit("inherit both", |tx| {
                    set_piece_top_materials(tx, &source, &[0, 1], None)
                })
                .unwrap();
            let cleared = Metadata::parse(changed.vmf()).unwrap();
            assert_eq!(
                cleared.entries[&(source.solid, source.side)].extra,
                original.entries[&(source.solid, source.side)].extra
            );
            assert_eq!(
                cleared.entries[&(source.solid, source.side)].top_materials,
                [None, None]
            );
        }
    }

    #[test]
    fn piece_materials_reject_invalid_updates_and_reserved_conflicts_atomically() {
        let (mut document, source) = fixture();
        document.edit("keep", |tx| store(tx, &source)).unwrap();
        let before = document.vmf().clone();
        for (pieces, material) in [
            (vec![], Some("brick/test")),
            (vec![0, 0], Some("brick/test")),
            (vec![0, 1, 1], Some("brick/test")),
            (vec![2], None),
            (vec![0], Some(" ")),
            (vec![0], Some("brick/\nred")),
        ] {
            assert!(
                document
                    .edit("invalid", |tx| set_piece_top_materials(
                        tx, &source, &pieces, material
                    ))
                    .is_err()
            );
            assert_eq!(document.vmf(), &before);
        }
        for length in [MAX_TEXT_BYTES, MAX_TEXT_BYTES + 1] {
            let huge = "x".repeat(length);
            assert!(
                document
                    .edit("budget", |tx| set_piece_top_materials(
                        tx,
                        &source,
                        &[0, 1],
                        Some(&huge)
                    ))
                    .is_err()
            );
            assert_eq!(document.vmf(), &before);
        }
        let mut metadata = Metadata::parse(document.vmf()).unwrap();
        metadata
            .entries
            .get_mut(&(source.solid, source.side))
            .unwrap()
            .extra
            .push(Node::string(MATERIAL_FIELDS[0], "vendor data"));
        document.edit("unknown", |tx| metadata.write(tx)).unwrap();
        let before = document.vmf().clone();
        assert!(
            document
                .edit("conflict", |tx| set_piece_top_materials(
                    tx,
                    &source,
                    &[1],
                    Some("wood/test")
                ))
                .unwrap_err()
                .to_string()
                .contains("reserved field")
        );
        assert_eq!(document.vmf(), &before);
        assert_eq!(
            Metadata::parse(document.vmf()).unwrap().entries[&(source.solid, source.side)].extra,
            vec![Node::string(MATERIAL_FIELDS[0], "vendor data")]
        );
    }

    #[test]
    fn material_intents_reject_malformed_records_and_changed_sources() {
        let (mut document, source) = fixture();
        document.edit("keep", |tx| store(tx, &source)).unwrap();
        document
            .edit("material", |tx| {
                set_piece_top_materials(tx, &source, &[0], Some("brick/test"))
            })
            .unwrap();
        let metadata = Metadata::parse(document.vmf()).unwrap();
        let intent = metadata.entries[&(source.solid, source.side)].clone();
        for replacement in [
            None,
            Some(Node::string(MATERIAL_FIELDS[0], "")),
            Some(Node::string(MATERIAL_FIELDS[0], "a\nb")),
            Some(Node::block(MATERIAL_FIELDS[0], vec![])),
        ] {
            let mut node = intent.node(true);
            let source_formats::keyvalues::Value::Block(fields) = &mut node.value else {
                unreachable!()
            };
            fields.retain(|field| field.key != MATERIAL_FIELDS[0]);
            if let Some(replacement) = replacement {
                fields.push(replacement);
            }
            let mut vmf = document.vmf().clone();
            vmf.extra.retain(|node| node.key != KEY);
            vmf.extra.push(Node::block(
                KEY,
                vec![Node::string("version", VERSION), node],
            ));
            assert!(Metadata::parse(&vmf).is_err());
        }
        let mut duplicate = intent.node(true);
        let source_formats::keyvalues::Value::Block(fields) = &mut duplicate.value else {
            unreachable!()
        };
        fields.push(Node::string(MATERIAL_FIELDS[0], "wood/test"));
        let mut vmf = document.vmf().clone();
        vmf.extra.retain(|node| node.key != KEY);
        vmf.extra.push(Node::block(
            KEY,
            vec![Node::string("version", VERSION), duplicate],
        ));
        let mut malformed = Document::new(vmf);
        let before = malformed.vmf().clone();
        assert!(
            malformed
                .edit("invalid write", |tx| metadata.write(tx))
                .is_err()
        );
        assert_eq!(malformed.vmf(), &before);
        let mut alternate = source.clone();
        alternate.edges =
            subobject::crease_pairs(document.solid(source.solid).unwrap(), source.side).unwrap()[1]
                .edges;
        let before = document.vmf().clone();
        assert!(
            document
                .edit("direction", |tx| store(tx, &alternate))
                .is_err()
        );
        assert!(
            document
                .edit("stale direction", |tx| set_piece_top_materials(
                    tx,
                    &alternate,
                    &[0],
                    Some("wood/test")
                ))
                .is_err()
        );
        assert_eq!(document.vmf(), &before);
        document
            .edit("delete", |tx| {
                edit::delete(tx, &[ObjectId::Solid(source.solid)])
            })
            .unwrap();
        let before = document.vmf().clone();
        assert!(
            document
                .edit("missing source", |tx| set_piece_top_materials(
                    tx,
                    &source,
                    &[0],
                    None
                ))
                .is_err()
        );
        assert_eq!(document.vmf(), &before);
    }

    #[test]
    fn explicit_material_storage_maps_reflected_draft_loops_to_original_piece_identity() {
        let (mut document, source) = fixture();
        document.edit("keep", |tx| store(tx, &source)).unwrap();
        let original = Metadata::parse(document.vmf()).unwrap();
        document
            .edit("reflect", |tx| {
                edit::transform(
                    tx,
                    &[ObjectId::Solid(source.solid)],
                    Transform {
                        reflection: [true, false, false],
                        ..Default::default()
                    },
                )
            })
            .unwrap();
        let current = subobject::plan_partitions(&document, std::slice::from_ref(&source))
            .unwrap()
            .remove(0);
        let materials = [Some("BRICK/RED".into()), Some("WOOD/BLUE".into())];
        document
            .edit("planned materials", |tx| {
                store_partition_materials(tx, &current, &materials)
            })
            .unwrap();
        let metadata = Metadata::parse(document.vmf()).unwrap();
        assert_eq!(
            metadata.entries[&(source.solid, source.side)].corners,
            original.entries[&(source.solid, source.side)].corners
        );
        let resolved = metadata.resolve(&document).remove(0).result.unwrap();
        for (piece, material) in materials.iter().enumerate() {
            let target = (0..2)
                .find(|target| membership(&resolved, *target) == membership(&current, piece))
                .unwrap();
            assert_eq!(
                metadata.entries[&(source.solid, source.side)].top_materials[target],
                *material
            );
        }
        let before = document.vmf().clone();
        let restored = metadata.entries[&(source.solid, source.side)]
            .top_materials
            .clone();
        document
            .edit("keep again", |tx| {
                store_partition_materials(tx, &resolved, &restored)
            })
            .unwrap();
        assert_eq!(document.vmf(), &before);
        document
            .edit("inherit", |tx| {
                store_partition_materials(tx, &resolved, &[None, None])
            })
            .unwrap();
        assert_eq!(
            Metadata::parse(document.vmf()).unwrap().entries[&(source.solid, source.side)]
                .top_materials,
            [None, None]
        );
        document.undo().unwrap();
        assert_eq!(document.vmf(), &before);
        for invalid in [" ".into(), "a\nb".into(), "x".repeat(MAX_TEXT_BYTES + 1)] {
            assert!(
                document
                    .edit("invalid plan", |tx| store_partition_materials(
                        tx,
                        &resolved,
                        &[Some(invalid), None]
                    ))
                    .is_err()
            );
            assert_eq!(document.vmf(), &before);
        }
    }

    #[test]
    fn adjusted_metadata_rejects_malformed_parameters_and_preserves_conflicting_extras() {
        let (mut document, source) = fixture();
        let cut = subobject::PartitionCut {
            source: source.clone(),
            edge_parameters: [0.2, 0.8],
        };
        let partition = subobject::plan_partitions_at(&document, &[cut])
            .unwrap()
            .remove(0);
        document
            .edit("keep", |tx| store_partition(tx, &partition))
            .unwrap();
        for value in ["NaN 0.8", "0 0.8", "1 0.8", "0.2", "0.2 0.8 0.5", "bad 0.8"] {
            let mut vmf = document.vmf().clone();
            let root = vmf.extra.iter_mut().find(|node| node.key == KEY).unwrap();
            let source_formats::keyvalues::Value::Block(children) = &mut root.value else {
                unreachable!()
            };
            let entry = children
                .iter_mut()
                .find(|node| node.key == "partition")
                .unwrap();
            let source_formats::keyvalues::Value::Block(fields) = &mut entry.value else {
                unreachable!()
            };
            fields
                .iter_mut()
                .find(|node| node.key == PARAMETERS_FIELD)
                .unwrap()
                .value = source_formats::keyvalues::Value::String(value.into());
            assert!(Metadata::parse(&vmf).is_err());
        }
        document.undo().unwrap();
        document.edit("midpoint", |tx| store(tx, &source)).unwrap();
        let mut metadata = Metadata::parse(document.vmf()).unwrap();
        metadata
            .entries
            .get_mut(&(source.solid, source.side))
            .unwrap()
            .extra
            .push(Node::string(PARAMETERS_FIELD, "vendor data"));
        document.edit("unknown", |tx| metadata.write(tx)).unwrap();
        let before = document.vmf().clone();
        assert!(
            document
                .edit("adjusted", |tx| store_partition(tx, &partition))
                .unwrap_err()
                .to_string()
                .contains("reserved field")
        );
        assert_eq!(document.vmf(), &before);
        assert_eq!(
            Metadata::parse(document.vmf()).unwrap().entries[&(source.solid, source.side)].extra[0],
            Node::string(PARAMETERS_FIELD, "vendor data")
        );
    }

    #[test]
    fn adjusted_intents_roundtrip_reflect_extrude_and_preserve_piece_identity() {
        for direction in 0..2 {
            let (mut document, mut source) = fixture();
            source.edges =
                subobject::crease_pairs(document.solid(source.solid).unwrap(), source.side)
                    .unwrap()[direction]
                    .edges;
            let cut = subobject::PartitionCut {
                source: source.clone(),
                edge_parameters: [0.2, 0.8],
            };
            let partition = subobject::plan_partitions_at(&document, &[cut])
                .unwrap()
                .remove(0);
            let before = document.vmf().clone();
            document
                .edit("keep adjusted", |tx| store_partition(tx, &partition))
                .unwrap();
            let stored = document.vmf().clone();
            assert_eq!(stored.world, before.world);
            let parsed = Vmf::parse(&stored.write(&Default::default())).unwrap();
            let mut changed = Document::new(parsed);
            changed
                .edit("mirror", |tx| {
                    edit::transform(
                        tx,
                        &[ObjectId::Solid(source.solid)],
                        Transform {
                            reflection: [true, false, false],
                            rotation: DVec3::new(17.0, -23.0, 31.0),
                            ..Default::default()
                        },
                    )
                })
                .unwrap();
            changed
                .edit("owner", |tx| {
                    edit::tie_to_entity(tx, &[source.solid], "func_detail", &[])
                })
                .unwrap();
            let resolved = Metadata::parse(changed.vmf())
                .unwrap()
                .resolve(&changed)
                .remove(0)
                .result
                .unwrap();
            assert_eq!(resolved.cut().unwrap().edge_parameters, [0.2, 0.8]);
            let membership = |part: &Partition, piece: usize| {
                part.loops[piece]
                    .vertices
                    .iter()
                    .filter_map(|index| match &part.vertices[*index as usize].source {
                        VertexSource::Corner(sides) => Some(sides.clone()),
                        _ => None,
                    })
                    .collect::<BTreeSet<_>>()
            };
            assert_eq!(membership(&partition, 0), membership(&resolved, 0));
            assert_eq!(membership(&partition, 1), membership(&resolved, 1));
            let original = changed.vmf().clone();
            changed
                .edit("keep again", |tx| store_partition(tx, &resolved))
                .unwrap();
            assert_eq!(changed.vmf(), &original);
            changed
                .edit("extrude", |tx| {
                    subobject::extrude_partitions(
                        tx,
                        std::slice::from_ref(&resolved),
                        &[(source.solid, 0), (source.solid, 1)],
                        &subobject::ExtrusionSpec {
                            height: 32.125,
                            wall_material: None,
                            texture_lock: true,
                        },
                    )
                })
                .unwrap();
            assert_eq!(changed.vmf().extra, original.extra);
            changed.undo().unwrap();
            assert_eq!(changed.vmf(), &original);
            document.undo().unwrap();
            assert_eq!(document.vmf(), &before);
            document.redo().unwrap();
            assert_eq!(document.vmf(), &stored);
        }
    }

    #[test]
    fn source_incidence_and_piece_identity_survive_affine_edits_reflection_and_ownership() {
        for direction in 0..2 {
            let (mut document, mut request) = fixture();
            request.edges =
                subobject::crease_pairs(document.solid(request.solid).unwrap(), request.side)
                    .unwrap()[direction]
                    .edges;
            document.edit("store", |tx| store(tx, &request)).unwrap();
            let before = Metadata::parse(document.vmf())
                .unwrap()
                .resolve(&document)
                .remove(0)
                .result
                .unwrap();
            for reflection in [
                [false; 3],
                [true, false, false],
                [false, true, false],
                [false, false, true],
            ] {
                let mut changed = document.clone();
                changed
                    .edit("transform", |tx| {
                        edit::transform(
                            tx,
                            &[ObjectId::Solid(request.solid)],
                            Transform {
                                translation: DVec3::new(17.25, -3.125, 9.5),
                                rotation: DVec3::new(17.0, -23.0, 31.0),
                                scale: DVec3::new(1.125, 0.75, 1.5),
                                reflection,
                                pivot: DVec3::ZERO,
                                texture_lock: false,
                                ..Default::default()
                            },
                        )
                    })
                    .unwrap();
                let owner = changed
                    .edit("owner", |tx| {
                        edit::tie_to_entity(tx, &[request.solid], "func_detail", &[])
                    })
                    .unwrap();
                let vmf = Vmf::parse(&changed.vmf().write(&Default::default())).unwrap();
                let mut changed = Document::new(vmf);
                changed
                    .edit("keep again", |tx| store(tx, &request))
                    .unwrap();
                let after = Metadata::parse(changed.vmf())
                    .unwrap()
                    .resolve(&changed)
                    .remove(0)
                    .result
                    .unwrap();
                assert_eq!(after.owner, owner);
                for piece in 0..2 {
                    assert_eq!(membership(&before, piece), membership(&after, piece));
                }
                assert_eq!(after.loops[0].id, 0);
                assert_eq!(after.loops[1].id, 1);
            }
        }
    }

    #[test]
    fn stale_entries_remain_diagnosable_while_other_sources_resolve() {
        let (mut document, first) = fixture();
        let ObjectId::Solid(second) = document
            .edit("second", |tx| {
                edit::create_block(tx, DVec3::splat(128.0), DVec3::splat(192.0), "brick/test")
            })
            .unwrap()
        else {
            unreachable!()
        };
        let source = document.solid(second).unwrap();
        let side = source
            .sides
            .iter()
            .find(|side| crate::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
            .unwrap()
            .id;
        let other = Crease {
            solid: second,
            side,
            edges: subobject::crease_pairs(source, side).unwrap()[0].edges,
        };
        document
            .edit("store", |tx| {
                store(tx, &first)?;
                store(tx, &other)
            })
            .unwrap();
        let metadata = Metadata::parse(document.vmf()).unwrap();
        document
            .edit("delete", |tx| {
                edit::delete(tx, &[ObjectId::Solid(first.solid)])
            })
            .unwrap();
        let results = metadata.resolve(&document);
        assert!(results[0].result.is_err());
        assert!(results[1].result.is_ok());
        assert_eq!(Metadata::parse(document.vmf()).unwrap(), metadata);
        document.undo().unwrap();
        assert!(
            metadata
                .resolve(&document)
                .iter()
                .all(|result| result.result.is_ok())
        );
        document
            .edit("split", |tx| {
                subobject::split(
                    tx,
                    &[subobject::Component {
                        solid: first.solid,
                        sides: vec![first.side],
                    }],
                    16.125,
                )
            })
            .unwrap();
        assert!(metadata.resolve(&document)[0].result.is_err());
        assert!(metadata.resolve(&document)[1].result.is_ok());
        assert_eq!(Metadata::parse(document.vmf()).unwrap(), metadata);
    }

    #[test]
    fn malformed_future_duplicate_and_out_of_budget_metadata_cannot_be_overwritten() {
        let (mut document, request) = fixture();
        document.edit("store", |tx| store(tx, &request)).unwrap();
        let valid = Metadata::parse(document.vmf()).unwrap().node(true);
        for mode in 0..10 {
            let mut node = valid.clone();
            let source = document.vmf().clone();
            let mut vmf = source;
            vmf.extra.clear();
            match mode {
                0 => node = Node::string(KEY, "bad"),
                1 => node = Node::block(KEY, vec![Node::string("version", "2")]),
                2 => {
                    node = Node::block(
                        KEY,
                        vec![Node::string("version", "1"), Node::string("version", "1")],
                    )
                }
                3 => {
                    let mut children = node.children().unwrap().to_vec();
                    children.push(children[1].clone());
                    node = Node::block(KEY, children);
                }
                4 => {
                    let mut fields = node.children().unwrap()[1].children().unwrap().to_vec();
                    fields.push(Node::string("solid", "2"));
                    node = Node::block(
                        KEY,
                        vec![
                            Node::string("version", "1"),
                            Node::block("partition", fields),
                        ],
                    );
                }
                5 => {
                    let mut fields = node.children().unwrap()[1].children().unwrap().to_vec();
                    fields[3] = Node::string("edges", "1 1");
                    node = Node::block(
                        KEY,
                        vec![
                            Node::string("version", "1"),
                            Node::block("partition", fields),
                        ],
                    );
                }
                6 => {
                    let mut fields = node.children().unwrap()[1].children().unwrap().to_vec();
                    fields[4] = Node::string("corner", "0 1 2");
                    node = Node::block(
                        KEY,
                        vec![
                            Node::string("version", "1"),
                            Node::block("partition", fields),
                        ],
                    );
                }
                7 => {
                    let mut children = node.children().unwrap().to_vec();
                    children.push(Node::string("future", "x".repeat(MAX_TEXT_BYTES)));
                    node = Node::block(KEY, children);
                }
                8 => {
                    let mut children = node.children().unwrap().to_vec();
                    children.extend((0..MAX_NODES).map(|_| Node::string("x", "")));
                    node = Node::block(KEY, children);
                }
                _ => {
                    let mut nested = Node::string("x", "");
                    for _ in 0..MAX_DEPTH {
                        nested = Node::block("x", vec![nested]);
                    }
                    let mut children = node.children().unwrap().to_vec();
                    children.push(nested);
                    node = Node::block(KEY, children);
                }
            }
            vmf.extra.push(node);
            let mut invalid = Document::new(vmf);
            let before = invalid.vmf().clone();
            assert!(Metadata::parse(invalid.vmf()).is_err(), "mode={mode}");
            assert!(
                invalid.edit("overwrite", |tx| store(tx, &request)).is_err(),
                "mode={mode}"
            );
            assert_eq!(invalid.vmf(), &before);
            assert_eq!(invalid.undo_labels().count(), 0);
            assert_eq!(
                Vmf::parse(&invalid.vmf().write(&Default::default()))
                    .unwrap()
                    .extra,
                before.extra
            );
        }
        let mut metadata = Metadata::parse(document.vmf()).unwrap();
        let intent = metadata.entries.values().next().unwrap().clone();
        metadata.entries.clear();
        for index in 1..=MAX_PARTITIONS + 1 {
            let mut intent = intent.clone();
            intent.source.solid = index as u32;
            metadata
                .entries
                .insert((intent.source.solid, intent.source.side), intent);
        }
        let before = document.vmf().clone();
        assert!(document.edit("overflow", |tx| metadata.write(tx)).is_err());
        assert_eq!(document.vmf(), &before);
    }
}

#[cfg(test)]
mod boundary_tests;

#[cfg(test)]
mod polygon_tests;
