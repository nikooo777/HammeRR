use std::collections::{BTreeMap, BTreeSet};

use glam::DMat3;
use source_formats::keyvalues::Node;
use source_formats::vmf::Vmf;

use crate::subobject::{self, FreePartition, FreePartitionCut, FreePieceSurface, PartitionTexture};
use crate::{Document, EditError, Transaction};

pub mod transfer;

pub const KEY: &str = "hammerr_free_partitions";
pub const MAX_PARTITIONS: usize = crate::partition_metadata::MAX_PARTITIONS;
pub const MAX_TEXT_BYTES: usize = crate::partition_metadata::MAX_TEXT_BYTES;
const VERSION: &str = "1";
const RECORD_FIELDS: [&str; 7] = [
    "solid",
    "side",
    "corner",
    "vertex",
    "endpoint_corner",
    "edge",
    "surface",
];

#[derive(Debug, Clone, PartialEq)]
pub struct Intent {
    pub solid: u32,
    pub side: u32,
    pub corners: Vec<Vec<u32>>,
    pub vertices: Vec<[f64; 3]>,
    pub endpoint_corners: BTreeMap<u32, u32>,
    pub edges: Vec<[u32; 2]>,
    pub surfaces: Vec<FreePieceSurface>,
    pub extra: Vec<Node>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Metadata {
    pub entries: BTreeMap<(u32, u32), Intent>,
    pub extra: Vec<Node>,
}

pub struct Resolution {
    pub solid: u32,
    pub side: u32,
    pub result: Result<FreePartition, String>,
}

fn field<'a>(nodes: &'a [Node], key: &str) -> Result<&'a str, String> {
    let mut matches = nodes.iter().filter(|node| node.key == key);
    let value = matches
        .next()
        .and_then(Node::as_str)
        .ok_or_else(|| format!("Free-cut metadata is missing {key}"))?;
    if matches.next().is_some() {
        return Err(format!("Free-cut metadata repeats {key}"));
    }
    Ok(value)
}

fn numbers<const N: usize>(value: &str) -> Result<[f64; N], String> {
    let values = value
        .split_whitespace()
        .take(N + 1)
        .map(str::parse::<f64>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "Free-cut coordinates must be numeric")?;
    let values: [f64; N] = values
        .try_into()
        .map_err(|_| "Free-cut coordinate count is invalid")?;
    if values.iter().any(|value| !value.is_finite()) {
        return Err("Free-cut coordinates must be finite".into());
    }
    Ok(values)
}

fn indices<const N: usize>(value: &str) -> Result<[u32; N], String> {
    value
        .split_whitespace()
        .take(N + 1)
        .map(str::parse::<u32>)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "Free-cut vertex indices must be unsigned integers")?
        .try_into()
        .map_err(|_| "Free-cut edge index count is invalid".into())
}

fn positive_id(value: &str) -> Result<u32, String> {
    value
        .parse::<u32>()
        .ok()
        .filter(|value| *value != 0)
        .ok_or_else(|| "Free-cut identities must be positive 32-bit integers".into())
}

fn encode<T: ToString>(values: &[T]) -> String {
    values
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_surface(node: &Node, solid: u32) -> Result<FreePieceSurface, String> {
    let fields = node.children().ok_or("Free-cut surface must be a block")?;
    if fields
        .iter()
        .any(|node| !["piece", "material", "texture", "axes"].contains(&node.key.as_str()))
    {
        return Err("Free-cut surface has unsupported fields".into());
    }
    let piece = field(fields, "piece")?
        .parse::<u32>()
        .map_err(|_| "Invalid free-cut piece index")?;
    let material = if fields.iter().any(|node| node.key == "material") {
        Some(crate::partition_metadata::normalize_top_material(field(
            fields, "material",
        )?)?)
    } else {
        None
    };
    let texture = if fields.iter().any(|node| node.key == "texture") {
        let [u, v, su, sv, rotation] = numbers(field(fields, "texture")?)?;
        let axes = if fields.iter().any(|node| node.key == "axes") {
            let [ux, uy, uz, vx, vy, vz] = numbers(field(fields, "axes")?)?;
            Some([[ux, uy, uz], [vx, vy, vz]])
        } else {
            None
        };
        Some(PartitionTexture {
            shift: [u, v],
            scale: [su, sv],
            rotation,
            axes,
        })
    } else {
        if fields.iter().any(|node| node.key == "axes") {
            return Err("Free-cut axes require a texture plan".into());
        }
        None
    };
    Ok(FreePieceSurface {
        solid,
        piece,
        material,
        texture,
    })
}

impl Intent {
    pub fn from_partition(
        part: &FreePartition,
        surfaces: &[FreePieceSurface],
    ) -> Result<Self, String> {
        let corners = &part.vertices[..part.corners.len()];
        let origin = corners[0].position;
        let a = corners[1].position - origin;
        let b = corners[2].position - origin;
        let inverse = DMat3::from_cols(a, b, part.normal()).inverse();
        let vertices = part
            .cut
            .vertices
            .iter()
            .map(|point| {
                let coordinates = inverse * (*point - origin);
                [
                    1.0 - coordinates.x - coordinates.y,
                    coordinates.x,
                    coordinates.y,
                ]
            })
            .collect();
        let result = Self {
            solid: part.solid,
            side: part.side,
            corners: part.corners.clone(),
            vertices,
            endpoint_corners: part
                .endpoint_vertices
                .iter()
                .enumerate()
                .filter_map(|(index, vertex)| {
                    ((*vertex as usize) < part.corners.len()).then_some((index as u32, *vertex))
                })
                .collect(),
            edges: part.cut.edges.clone(),
            surfaces: surfaces
                .iter()
                .filter(|surface| surface.solid == part.solid)
                .cloned()
                .collect(),
            extra: Vec::new(),
        };
        result.validate()?;
        Ok(result)
    }

    fn validate(&self) -> Result<(), String> {
        if self.solid == 0
            || self.side == 0
            || !(3..=subobject::MAX_BOUNDARY_SOURCE_CORNERS).contains(&self.corners.len())
            || self.corners.iter().collect::<BTreeSet<_>>().len() != self.corners.len()
            || self.corners.iter().any(|corner| {
                !(3..=subobject::MAX_SIDES).contains(&corner.len())
                    || corner.contains(&0)
                    || !corner.contains(&self.side)
                    || corner.windows(2).any(|pair| pair[0] >= pair[1])
            })
            || self.vertices.len() > subobject::MAX_FREE_PARTITION_VERTICES - self.corners.len()
            || self.vertices.iter().any(|coordinates| {
                coordinates.iter().any(|value| !value.is_finite())
                    || (coordinates.iter().sum::<f64>() - 1.0).abs() > 1e-8
            })
            || self.edges.is_empty()
            || self.endpoint_corners.iter().any(|(endpoint, corner)| {
                *endpoint as usize >= self.vertices.len() || *corner as usize >= self.corners.len()
            })
            || self.edges.len() > subobject::MAX_FREE_PARTITION_EDGES
            || self.edges.iter().any(|edge| {
                edge[0] == edge[1]
                    || edge
                        .iter()
                        .any(|vertex| *vertex as usize >= self.vertices.len())
            })
        {
            return Err("Free-cut source, corner, endpoint or edge bounds are invalid".into());
        }
        let mut pieces = BTreeSet::new();
        for surface in &self.surfaces {
            if surface.solid != self.solid
                || surface.piece as usize >= subobject::MAX_FREE_PARTITION_PIECES
                || !pieces.insert(surface.piece)
            {
                return Err("Free-cut surface plans repeat or exceed their piece bounds".into());
            }
            if let Some(material) = &surface.material {
                crate::partition_metadata::checked_top_material(material)?;
            }
            if let Some(texture) = &surface.texture {
                texture.validate().map_err(|error| error.to_string())?;
            }
        }
        Ok(())
    }

    pub(crate) fn resolve_topology(&self, document: &Document) -> Result<FreePartition, String> {
        self.validate()?;
        let (_, source) =
            subobject::solid(document, self.solid).map_err(|error| error.to_string())?;
        let shape = subobject::topology(source).map_err(|error| error.to_string())?;
        let anchors = self
            .corners
            .iter()
            .map(|corner| {
                shape
                    .vertices
                    .iter()
                    .find(|vertex| &vertex.sides == corner)
                    .map(|vertex| vertex.position)
                    .ok_or("Free-cut source corner incidence changed")
            })
            .collect::<Result<Vec<_>, _>>()?;
        let cut = FreePartitionCut {
            solid: self.solid,
            side: self.side,
            corners: self.corners.clone(),
            vertices: self
                .vertices
                .iter()
                .enumerate()
                .map(|(index, coordinates)| {
                    if let Some(corner) = self.endpoint_corners.get(&(index as u32)) {
                        return anchors[*corner as usize];
                    }
                    anchors[0] * coordinates[0]
                        + anchors[1] * coordinates[1]
                        + anchors[2] * coordinates[2]
                })
                .collect(),
            edges: self.edges.clone(),
        };
        let part = subobject::plan_free_partitions(document, &[cut])
            .map_err(|error| error.to_string())?
            .remove(0);
        if self
            .surfaces
            .iter()
            .any(|surface| !part.pieces.iter().any(|piece| piece.id == surface.piece))
        {
            return Err("Saved free-cut surface piece is unavailable".into());
        }
        Ok(part)
    }

    pub fn resolve(&self, document: &Document) -> Result<FreePartition, String> {
        let part = self.resolve_topology(document)?;
        let (_, source) =
            subobject::solid(document, self.solid).map_err(|error| error.to_string())?;
        let side = source
            .sides
            .iter()
            .find(|side| side.id == self.side)
            .ok_or("Saved free-cut source face does not exist")?;
        for texture in self.surfaces.iter().filter_map(|surface| surface.texture) {
            texture.project(side).map_err(|error| error.to_string())?;
        }
        Ok(part)
    }

    fn node(&self) -> Node {
        let mut fields = vec![
            Node::string("solid", self.solid.to_string()),
            Node::string("side", self.side.to_string()),
        ];
        fields.extend(
            self.corners
                .iter()
                .map(|corner| Node::string("corner", encode(corner))),
        );
        fields.extend(
            self.vertices
                .iter()
                .map(|vertex| Node::string("vertex", encode(vertex))),
        );
        fields.extend(self.endpoint_corners.iter().map(|(endpoint, corner)| {
            Node::string("endpoint_corner", encode(&[*endpoint, *corner]))
        }));
        fields.extend(
            self.edges
                .iter()
                .map(|edge| Node::string("edge", encode(edge))),
        );
        fields.extend(self.surfaces.iter().map(|surface| {
            let mut fields = vec![Node::string("piece", surface.piece.to_string())];
            if let Some(material) = &surface.material {
                fields.push(Node::string("material", material));
            }
            if let Some(texture) = &surface.texture {
                fields.push(Node::string(
                    "texture",
                    encode(&[
                        texture.shift[0],
                        texture.shift[1],
                        texture.scale[0],
                        texture.scale[1],
                        texture.rotation,
                    ]),
                ));
                if let Some(axes) = texture.axes {
                    fields.push(Node::string(
                        "axes",
                        encode(&[
                            axes[0][0], axes[0][1], axes[0][2], axes[1][0], axes[1][1], axes[1][2],
                        ]),
                    ));
                }
            }
            Node::block("surface", fields)
        }));
        fields.extend(self.extra.clone());
        Node::block("partition", fields)
    }
}

impl Metadata {
    pub fn parse(vmf: &Vmf) -> Result<Self, String> {
        let mut blocks = vmf
            .extra
            .iter()
            .filter(|node| node.key.eq_ignore_ascii_case(KEY));
        let Some(block) = blocks.next() else {
            return Ok(Self::default());
        };
        if block.key != KEY || blocks.next().is_some() {
            return Err("Map contains noncanonical or duplicate free-cut metadata".into());
        }
        crate::partition_metadata::Budget::default().visit(block, 1)?;
        let children = block
            .children()
            .ok_or("Free-cut metadata must be a block")?;
        if field(children, "version")? != VERSION {
            return Err(
                "Unsupported free-cut metadata version; raw data has been preserved".into(),
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
                return Err("Map supports at most 64 saved free-cut faces".into());
            }
            let fields = node
                .children()
                .ok_or("Free-cut partition must be a block")?;
            let solid = positive_id(field(fields, "solid")?)?;
            let side = positive_id(field(fields, "side")?)?;
            let mut intent = Intent {
                solid,
                side,
                corners: Vec::new(),
                vertices: Vec::new(),
                endpoint_corners: BTreeMap::new(),
                edges: Vec::new(),
                surfaces: Vec::new(),
                extra: Vec::new(),
            };
            for field in fields {
                match field.key.as_str() {
                    "solid" | "side" => (),
                    "corner" => {
                        if intent.corners.len() == subobject::MAX_BOUNDARY_SOURCE_CORNERS {
                            return Err("Free-cut source exceeds eight corners".into());
                        }
                        let value = field
                            .as_str()
                            .ok_or("Free-cut corner incidence must be a string")?;
                        let corner = value
                            .split_whitespace()
                            .take(subobject::MAX_SIDES + 1)
                            .map(positive_id)
                            .collect::<Result<Vec<_>, _>>()?;
                        intent.corners.push(corner);
                    }
                    "vertex" => {
                        if intent.vertices.len() == subobject::MAX_FREE_PARTITION_VERTICES {
                            return Err("Free-cut metadata exceeds its endpoint budget".into());
                        }
                        intent.vertices.push(numbers(
                            field.as_str().ok_or("Free-cut vertex must be a string")?,
                        )?);
                    }
                    "edge" => {
                        if intent.edges.len() == subobject::MAX_FREE_PARTITION_EDGES {
                            return Err("Free-cut metadata exceeds its edge budget".into());
                        }
                        intent.edges.push(indices(
                            field.as_str().ok_or("Free-cut edge must be a string")?,
                        )?);
                    }
                    "endpoint_corner" => {
                        let [endpoint, corner] = indices(
                            field
                                .as_str()
                                .ok_or("Free-cut endpoint corner must be a string")?,
                        )?;
                        if intent.endpoint_corners.insert(endpoint, corner).is_some() {
                            return Err(
                                "Free-cut endpoint repeats its source corner reference".into()
                            );
                        }
                    }
                    "surface" => intent.surfaces.push(parse_surface(field, solid)?),
                    _ => intent.extra.push(field.clone()),
                }
            }
            intent.validate()?;
            if result.entries.insert((solid, side), intent).is_some() {
                return Err("Free-cut metadata repeats a source face".into());
            }
        }
        result.validate()?;
        Ok(result)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.entries.len() > MAX_PARTITIONS {
            return Err("Map supports at most 64 saved free-cut faces".into());
        }
        if self
            .extra
            .iter()
            .any(|node| ["version", "partition"].contains(&node.key.as_str()))
        {
            return Err("Free-cut extensions conflict with reserved root fields".into());
        }
        for (key, intent) in &self.entries {
            intent.validate()?;
            if *key != (intent.solid, intent.side)
                || intent
                    .extra
                    .iter()
                    .any(|node| RECORD_FIELDS.contains(&node.key.as_str()))
            {
                return Err(
                    "Free-cut extensions or entry key conflict with reserved fields".into(),
                );
            }
        }
        crate::partition_metadata::Budget::default().visit(&self.node(), 1)
    }

    fn node(&self) -> Node {
        let mut fields = vec![Node::string("version", VERSION)];
        fields.extend(self.entries.values().map(Intent::node));
        fields.extend(self.extra.clone());
        Node::block(KEY, fields)
    }

    pub fn write(&self, tx: &mut Transaction) -> Result<(), EditError> {
        Self::parse(tx.vmf()).map_err(EditError::Invalid)?;
        self.validate().map_err(EditError::Invalid)?;
        crate::partition_metadata::Metadata::parse(tx.vmf()).map_err(EditError::Invalid)?;
        let node = (!self.entries.is_empty() || !self.extra.is_empty()).then(|| self.node());
        crate::partition_metadata::validate_combined(
            tx.vmf()
                .extra
                .iter()
                .find(|node| node.key == crate::partition_metadata::KEY),
            node.as_ref(),
        )
        .map_err(EditError::Invalid)?;
        tx.set_editor_metadata(KEY, node);
        Ok(())
    }

    pub fn resolve(&self, document: &Document) -> Vec<Resolution> {
        self.entries
            .values()
            .map(|intent| Resolution {
                solid: intent.solid,
                side: intent.side,
                result: intent.resolve(document),
            })
            .collect()
    }
}

pub fn store(
    tx: &mut Transaction,
    partitions: &[FreePartition],
    surfaces: &[FreePieceSurface],
) -> Result<(), EditError> {
    let mut metadata = Metadata::parse(tx.vmf()).map_err(EditError::Invalid)?;
    let mut targets = BTreeSet::new();
    for part in partitions {
        if !targets.insert(part.solid) {
            return Err(EditError::Invalid(
                "Keep free cuts on one source face per brush".into(),
            ));
        }
        let checked = subobject::plan_free_partitions(tx.document(), &[part.cut()])?.remove(0);
        if checked != *part {
            return Err(EditError::Invalid(
                "Free-cut source topology or ownership changed".into(),
            ));
        }
        let mut intent = Intent::from_partition(part, surfaces).map_err(EditError::Invalid)?;
        if let Some(previous) = metadata.entries.get(&(part.solid, part.side)) {
            intent.extra = previous.extra.clone();
        }
        intent.resolve(tx.document()).map_err(EditError::Invalid)?;
        metadata.entries.insert((part.solid, part.side), intent);
    }
    if surfaces
        .iter()
        .any(|surface| !targets.contains(&surface.solid))
    {
        return Err(EditError::Invalid(
            "Free-cut surface plan has no staged source face".into(),
        ));
    }
    metadata.write(tx)
}

pub fn remove(tx: &mut Transaction, sources: &[(u32, u32)]) -> Result<(), EditError> {
    let mut metadata = Metadata::parse(tx.vmf()).map_err(EditError::Invalid)?;
    for source in sources {
        if metadata.entries.remove(source).is_none() {
            return Err(EditError::Invalid(
                "Choose existing saved free-cut faces to remove".into(),
            ));
        }
    }
    metadata.write(tx)
}
