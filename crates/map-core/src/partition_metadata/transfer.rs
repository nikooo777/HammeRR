use std::collections::{BTreeMap, BTreeSet};

use super::{Intent, Metadata, piece_corners};
use crate::subobject::{self, Crease, Partition, PartitionCut, VertexSource};
use crate::{Document, EditError, Transaction, edit, surface};

fn invalid(message: impl Into<String>) -> EditError {
    EditError::Invalid(message.into())
}

fn checked_intent(source: &Document, intent: &Intent) -> Result<Partition, EditError> {
    if !intent.extra.is_empty() {
        return Err(invalid(format!(
            "Brush {}, face {}: saved partition has opaque extension fields that cannot be remapped",
            intent.source.solid, intent.source.side
        )));
    }
    intent.resolve(source).map_err(invalid)
}

pub fn capture(source: &Document, solids: &BTreeSet<u32>) -> Result<Metadata, EditError> {
    let metadata = Metadata::parse(source.vmf()).map_err(invalid)?;
    let mut captured = Metadata::default();
    for (key, intent) in metadata.entries {
        if solids.contains(&key.0) {
            checked_intent(source, &intent)?;
            captured.entries.insert(key, intent);
        }
    }
    captured.validate().map_err(invalid)?;
    Ok(captured)
}

fn mapped_id(ids: &BTreeMap<u32, u32>, id: u32) -> Result<u32, EditError> {
    ids.get(&id)
        .copied()
        .filter(|mapped| *mapped != 0)
        .ok_or_else(|| {
            invalid(format!(
                "Saved partition has no valid identity mapping for {id}"
            ))
        })
}

fn mapped_corner(ids: &BTreeMap<u32, u32>, corner: &[u32]) -> Result<Vec<u32>, EditError> {
    let mut mapped = corner
        .iter()
        .map(|id| mapped_id(ids, *id))
        .collect::<Result<Vec<_>, _>>()?;
    mapped.sort_unstable();
    Ok(mapped)
}

fn mapped_membership(
    part: &Partition,
    piece: &subobject::PartitionLoop,
    sides: &BTreeMap<u32, u32>,
) -> Result<BTreeSet<Vec<u32>>, EditError> {
    piece_corners(part, piece)
        .iter()
        .map(|corner| mapped_corner(sides, corner))
        .collect()
}

fn remapped_cut(
    source: &Partition,
    solids: &BTreeMap<u32, u32>,
    sides: &BTreeMap<u32, u32>,
) -> Result<PartitionCut, EditError> {
    let mut cuts = [(0, 0.0); 2];
    for (index, cut) in cuts.iter_mut().enumerate() {
        let VertexSource::Edge { corners, parameter } =
            &source.vertices[source.source_corner_count() + index].source
        else {
            return Err(invalid("Saved partition cut endpoint has no edge identity"));
        };
        let first = mapped_corner(sides, &corners[0])?;
        let second = mapped_corner(sides, &corners[1])?;
        *cut = (
            mapped_id(sides, source.source.edges[index])?,
            if first < second {
                *parameter
            } else {
                1.0 - *parameter
            },
        );
    }
    cuts.sort_by_key(|cut| cut.0);
    Ok(PartitionCut {
        source: Crease {
            solid: mapped_id(solids, source.source.solid)?,
            side: mapped_id(sides, source.source.side)?,
            edges: cuts.map(|cut| cut.0),
        },
        edge_parameters: cuts.map(|cut| cut.1),
    })
}

fn check_pose(
    source: &Partition,
    target: &Partition,
    sides: &BTreeMap<u32, u32>,
    pose: edit::Transform,
) -> Result<(), EditError> {
    for vertex in &source.vertices[..source.source_corner_count()] {
        let VertexSource::Corner(corner) = &vertex.source else {
            return Err(invalid("Saved partition source corner has no incidence"));
        };
        let corner = mapped_corner(sides, corner)?;
        let target = target.vertices.iter().find(
            |vertex| matches!(&vertex.source, VertexSource::Corner(sides) if *sides == corner),
        );
        let target = target.ok_or_else(|| invalid("Saved partition corner identity changed"))?;
        let expected = crate::transform::transform_point(vertex.position, pose)?;
        if !expected.abs_diff_eq(target.position, crate::plane::ON_EPSILON) {
            return Err(invalid(
                "Saved partition target geometry disagrees with its placement",
            ));
        }
    }
    Ok(())
}

fn remapped_intent(
    source_document: &Document,
    target_document: &Document,
    intent: &Intent,
    solids: &BTreeMap<u32, u32>,
    sides: &BTreeMap<u32, u32>,
    pose: edit::Transform,
) -> Result<Intent, EditError> {
    let source = checked_intent(source_document, intent)?;
    let cut = remapped_cut(&source, solids, sides)?;
    let target = subobject::plan_partitions_at(target_document, &[cut])?.remove(0);
    check_pose(&source, &target, sides, pose)?;
    let membership = [
        mapped_membership(&source, &source.loops[0], sides)?,
        mapped_membership(&source, &source.loops[1], sides)?,
    ];
    let mut copied = Intent::from_partition(&target);
    if let Some(pieces) = &mut copied.piece_membership {
        *pieces = membership
            .each_ref()
            .map(|piece| piece.iter().cloned().collect());
    } else if !copied.adjacent && piece_corners(&target, &target.loops[0]) != membership[0] {
        copied.corners.swap(0, 1);
        copied.corners.swap(2, 3);
    }
    let target = copied.resolve(target_document).map_err(invalid)?;
    if target
        .loops
        .iter()
        .zip(&membership)
        .any(|(piece, expected)| piece_corners(&target, piece) != *expected)
    {
        return Err(invalid(
            "Saved partition piece identity changed during transfer",
        ));
    }
    copied.top_materials.clone_from(&intent.top_materials);
    if intent.top_textures.iter().any(Option::is_some) {
        let source_side = surface::side(
            source_document,
            surface::FaceId {
                solid: source.source.solid,
                side: source.source.side,
            },
        )
        .ok_or_else(|| invalid("Saved partition source face does not exist"))?;
        let target_side = surface::side(
            target_document,
            surface::FaceId {
                solid: target.source.solid,
                side: target.source.side,
            },
        )
        .ok_or_else(|| invalid("Saved partition target face does not exist"))?;
        let points = target
            .vertices
            .iter()
            .map(|vertex| vertex.position)
            .collect::<Vec<_>>();
        for (index, texture) in intent.top_textures.iter().enumerate() {
            if let Some(texture) = texture {
                let projection =
                    crate::transform::transform_projection(&texture.project(source_side)?, pose)?;
                copied.top_textures[index] =
                    Some(texture.reproject(target_side, &projection, &points)?);
            }
        }
    }
    copied.resolve(target_document).map_err(invalid)?;
    Ok(copied)
}

pub fn remap(
    source: &Document,
    target: &Document,
    solids: &BTreeMap<u32, u32>,
    sides: &BTreeMap<u32, u32>,
    pose: edit::Transform,
) -> Result<Metadata, EditError> {
    let captured = capture(source, &solids.keys().copied().collect())?;
    if captured.entries.is_empty() {
        return Ok(captured);
    }
    for ids in [solids, sides] {
        let unique = ids.values().copied().collect::<BTreeSet<_>>();
        if unique.contains(&0) || unique.len() != ids.len() {
            return Err(invalid(
                "Saved partition identity mappings must be unique and positive",
            ));
        }
    }
    let mut remapped = Metadata::default();
    for intent in captured.entries.values() {
        let copied = remapped_intent(source, target, intent, solids, sides, pose)?;
        let key = (copied.source.solid, copied.source.side);
        if remapped.entries.insert(key, copied).is_some() {
            return Err(invalid(
                "Transferred partitions repeat a target source face",
            ));
        }
    }
    remapped.validate().map_err(invalid)?;
    Ok(remapped)
}

pub fn merge(tx: &mut Transaction, incoming: &Metadata) -> Result<usize, EditError> {
    incoming.validate().map_err(invalid)?;
    if !incoming.extra.is_empty() {
        return Err(invalid(
            "Unscoped partition root extensions cannot be imported",
        ));
    }
    if incoming.entries.is_empty() {
        return Ok(0);
    }
    let mut metadata = Metadata::parse(tx.vmf()).map_err(invalid)?;
    for (key, intent) in &incoming.entries {
        checked_intent(tx.document(), intent)?;
        if metadata.entries.contains_key(key) {
            return Err(invalid(
                "Transferred partition would replace an existing source face",
            ));
        }
    }
    metadata.entries.extend(incoming.entries.clone());
    metadata.write(tx)?;
    Ok(incoming.entries.len())
}

pub fn remove_for_solids(tx: &mut Transaction, solids: &BTreeSet<u32>) -> Result<usize, EditError> {
    let mut metadata = Metadata::parse(tx.vmf()).map_err(invalid)?;
    let before = metadata.entries.len();
    metadata.entries.retain(|key, _| !solids.contains(&key.0));
    let removed = before - metadata.entries.len();
    if removed != 0 {
        metadata.write(tx)?;
    }
    Ok(removed)
}
