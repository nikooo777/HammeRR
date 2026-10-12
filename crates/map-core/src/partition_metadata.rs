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
const VERSION: &str = "1";
const RECORD_FIELDS: [&str; 5] = ["kind", "solid", "side", "edges", "corner"];

#[derive(Debug, Clone, PartialEq)]
pub struct Intent {
    pub source: Crease,
    pub corners: [Vec<u32>; 4],
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

impl Intent {
    fn from_partition(partition: &Partition) -> Self {
        Self {
            source: partition.source.clone(),
            corners: std::array::from_fn(|index| match &partition.vertices[index].source {
                VertexSource::Corner(sides) => sides.clone(),
                _ => unreachable!(),
            }),
            extra: Vec::new(),
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.source.solid == 0
            || self.source.side == 0
            || self.source.edges[0] == 0
            || self.source.edges[0] >= self.source.edges[1]
            || self.source.edges.contains(&self.source.side)
        {
            return Err("Partition source IDs must be distinct, positive and canonical".into());
        }
        for (index, corner) in self.corners.iter().enumerate() {
            if !(3..=subobject::MAX_SIDES).contains(&corner.len())
                || corner.first() == Some(&0)
                || corner.windows(2).any(|pair| pair[0] >= pair[1])
                || !corner.contains(&self.source.side)
                || !corner.contains(&self.source.edges[index / 2])
                || corner.contains(&self.source.edges[1 - index / 2])
            {
                return Err("Partition corner incidence is invalid".into());
            }
        }
        if self.corners.iter().collect::<BTreeSet<_>>().len() != 4 {
            return Err("Partition needs four distinct source corners".into());
        }
        if self
            .extra
            .iter()
            .any(|node| RECORD_FIELDS.contains(&node.key.as_str()))
        {
            return Err("Partition extra metadata repeats a reserved field".into());
        }
        Ok(())
    }

    fn node(&self, include_extra: bool) -> Node {
        let mut fields = vec![
            Node::string("kind", KIND),
            Node::string("solid", self.source.solid.to_string()),
            Node::string("side", self.source.side.to_string()),
            Node::string("edges", encoded(&self.source.edges)),
        ];
        fields.extend(
            self.corners
                .iter()
                .map(|corner| Node::string("corner", encoded(corner))),
        );
        if include_extra {
            fields.extend(self.extra.clone());
        }
        Node::block("partition", fields)
    }

    fn resolve(&self, document: &Document) -> Result<Partition, String> {
        let mut partition =
            subobject::plan_partitions(document, std::slice::from_ref(&self.source))
                .map_err(|error| error.to_string())?
                .remove(0);
        let current = Self::from_partition(&partition);
        if current.corners.iter().collect::<BTreeSet<_>>()
            != self.corners.iter().collect::<BTreeSet<_>>()
        {
            return Err(format!(
                "Brush {}, face {}: saved partition corner incidence changed",
                self.source.solid, self.source.side
            ));
        }
        let membership = |corners: &[Vec<u32>; 4], piece: usize| {
            if piece == 0 {
                [&corners[0], &corners[3]]
            } else {
                [&corners[1], &corners[2]]
            }
            .into_iter()
            .cloned()
            .collect::<BTreeSet<_>>()
        };
        let expected = [membership(&self.corners, 0), membership(&self.corners, 1)];
        let actual = [
            membership(&current.corners, 0),
            membership(&current.corners, 1),
        ];
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
            if field(fields, "kind")? != KIND {
                return Err(
                    "Unsupported partition intent kind; raw metadata has been preserved".into(),
                );
            }
            let edges = ids(field(fields, "edges")?, 2, 2)?.try_into().unwrap();
            let mut corners = Vec::new();
            for corner in fields.iter().filter(|field| field.key == "corner") {
                if corners.len() == 4 {
                    return Err("Partition needs exactly four corner incidences".into());
                }
                corners.push(ids(
                    corner
                        .as_str()
                        .ok_or("Partition corner must be a side-ID list")?,
                    3,
                    subobject::MAX_SIDES,
                )?);
            }
            let corners = corners
                .try_into()
                .map_err(|_| "Partition needs exactly four corner incidences")?;
            let source = Crease {
                solid: id(field(fields, "solid")?)?,
                side: id(field(fields, "side")?)?,
                edges,
            };
            let intent = Intent {
                source,
                corners,
                extra: fields
                    .iter()
                    .filter(|field| !RECORD_FIELDS.contains(&field.key.as_str()))
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
    let mut metadata = Metadata::parse(tx.vmf()).map_err(EditError::Invalid)?;
    let partition =
        subobject::plan_partitions(tx.document(), std::slice::from_ref(request))?.remove(0);
    let mut intent = Intent::from_partition(&partition);
    let key = (request.solid, request.side);
    if let Some(previous) = metadata.entries.get(&key) {
        intent.extra = previous.extra.clone();
        if previous.source == intent.source && previous.resolve(tx.document()).is_ok() {
            intent.corners = previous.corners.clone();
        }
    }
    metadata.entries.insert(key, intent);
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
