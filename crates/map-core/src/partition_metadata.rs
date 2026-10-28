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
const TEXTURE_FIELDS: [&str; 2] = ["top_texture_0", "top_texture_1"];
const TEXTURE_KINDS: [&str; 4] = [
    "opposing_midpoints_textures",
    "opposing_edges_textures",
    "boundary_edges_textures",
    "polygon_boundary_edges_textures",
];
const PARAMETERS_FIELD: &str = "edge_parameters";
const VERSION: &str = "1";
const RECORD_FIELDS: [&str; 5] = ["kind", "solid", "side", "edges", "corner"];

#[derive(Debug, Clone, PartialEq)]
pub struct Intent {
    pub source: Crease,
    pub adjacent: bool,
    pub edge_parameters: [f64; 2],
    pub top_materials: [Option<String>; 2],
    pub top_textures: [Option<subobject::PartitionTexture>; 2],
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
            top_textures: [None, None],
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
        for texture in self.top_textures.iter().flatten() {
            texture.validate().map_err(|error| error.to_string())?;
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
                || ((self.top_materials.iter().any(Option::is_some)
                    || self.top_textures.iter().any(Option::is_some))
                    && MATERIAL_FIELDS.contains(&node.key.as_str()))
                || (self.top_textures.iter().any(Option::is_some)
                    && TEXTURE_FIELDS.contains(&node.key.as_str()))
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
                if self.top_textures.iter().any(Option::is_some) {
                    TEXTURE_KINDS[if self.piece_membership.is_some() {
                        3
                    } else if self.adjacent {
                        2
                    } else if self.edge_parameters != [0.5; 2] {
                        1
                    } else {
                        0
                    }]
                } else if self.piece_membership.is_some() {
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
        for (key, texture) in TEXTURE_FIELDS.iter().zip(&self.top_textures) {
            if let Some(texture) = texture {
                fields.push(Node::string(
                    *key,
                    format!(
                        "{} {} {} {} {}",
                        texture.shift[0],
                        texture.shift[1],
                        texture.scale[0],
                        texture.scale[1],
                        texture.rotation
                    ),
                ));
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

    pub fn resolve(&self, document: &Document) -> Result<Partition, String> {
        self.validate()?;
        if self.top_textures.iter().any(Option::is_some) {
            let source = crate::surface::side(
                document,
                crate::surface::FaceId {
                    solid: self.source.solid,
                    side: self.source.side,
                },
            )
            .ok_or("Saved partition source face does not exist")?;
            for texture in self.top_textures.iter().flatten() {
                texture.project(source).map_err(|error| error.to_string())?;
            }
        }
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
                && !TEXTURE_KINDS.contains(&kind)
            {
                return Err(
                    "Unsupported partition intent kind; raw metadata has been preserved".into(),
                );
            }
            let textured = TEXTURE_KINDS.contains(&kind);
            let adjacent =
                kind == BOUNDARY_KIND || kind == BOUNDARY_MATERIAL_KIND || kind == TEXTURE_KINDS[2];
            let polygon =
                kind == POLYGON_KIND || kind == POLYGON_MATERIAL_KIND || kind == TEXTURE_KINDS[3];
            let adjusted = polygon
                || adjacent
                || kind == EDGE_KIND
                || kind == EDGE_MATERIAL_KIND
                || kind == TEXTURE_KINDS[1];
            let styled = kind == MATERIAL_KIND
                || kind == EDGE_MATERIAL_KIND
                || kind == BOUNDARY_MATERIAL_KIND
                || kind == POLYGON_MATERIAL_KIND
                || textured;
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
                if top_materials.iter().all(Option::is_none) && !textured {
                    return Err(
                        "Material partition intent needs at least one planned top material".into(),
                    );
                }
            }
            let mut top_textures = [None, None];
            if textured {
                for (index, key) in TEXTURE_FIELDS.iter().enumerate() {
                    if fields.iter().any(|node| node.key == *key) {
                        let values = field(fields, key)?
                            .split_whitespace()
                            .take(6)
                            .map(str::parse::<f64>)
                            .collect::<Result<Vec<_>, _>>()
                            .map_err(
                                |_| "Planned textures need numeric shift, scale and rotation",
                            )?;
                        let [u, v, su, sv, rotation]: [f64; 5] = values
                            .try_into()
                            .map_err(|_: Vec<_>| "Planned textures need exactly five values")?;
                        let texture = subobject::PartitionTexture {
                            shift: [u, v],
                            scale: [su, sv],
                            rotation,
                        };
                        texture.validate().map_err(|error| error.to_string())?;
                        top_textures[index] = Some(texture);
                    }
                }
                if top_textures.iter().all(Option::is_none) {
                    return Err(
                        "Texture partition intent needs at least one planned texture".into(),
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
                top_textures,
                corners,
                piece_membership,
                extra: fields
                    .iter()
                    .filter(|field| {
                        !RECORD_FIELDS.contains(&field.key.as_str())
                            && !(adjusted && field.key == PARAMETERS_FIELD)
                            && !(styled && MATERIAL_FIELDS.contains(&field.key.as_str()))
                            && !(textured && TEXTURE_FIELDS.contains(&field.key.as_str()))
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
    store_partition_plan(tx, partition, None, None)
}

pub fn store_partition_materials(
    tx: &mut Transaction,
    partition: &Partition,
    materials: &[Option<String>; 2],
) -> Result<(), EditError> {
    store_partition_plan(tx, partition, Some(materials), None)
}

pub fn store_partition_surfaces(
    tx: &mut Transaction,
    partition: &Partition,
    materials: &[Option<String>; 2],
    textures: &[Option<subobject::PartitionTexture>; 2],
) -> Result<(), EditError> {
    store_partition_plan(tx, partition, Some(materials), Some(textures))
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
    textures: Option<&[Option<subobject::PartitionTexture>; 2]>,
) -> Result<(), EditError> {
    if let Some(materials) = materials {
        for material in materials.iter().flatten() {
            checked_top_material(material).map_err(EditError::Invalid)?;
        }
    }
    if let Some(textures) = textures {
        for texture in textures.iter().flatten() {
            texture.validate()?;
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
            intent.top_textures = previous.top_textures;
        } else if previous.top_materials.iter().any(Option::is_some)
            || previous.top_textures.iter().any(Option::is_some)
        {
            return Err(EditError::Invalid(
                "Saved piece surface plans require unchanged source incidence and cut direction"
                    .into(),
            ));
        }
    }
    if materials.is_some() || textures.is_some() {
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
            if let Some(materials) = materials {
                intent.top_materials[target.id as usize] = materials[piece.id as usize].clone();
            }
            if let Some(textures) = textures {
                intent.top_textures[target.id as usize] = textures[piece.id as usize];
            }
        }
    }
    intent.resolve(tx.document()).map_err(EditError::Invalid)?;
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
