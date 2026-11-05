use super::*;
use crate::{ObjectId, edit, surface};
use source_formats::vmf::Side;

fn invalid(message: impl Into<String>) -> EditError {
    EditError::Invalid(message.into())
}

fn checked_intent(source: &Document, intent: &Intent) -> Result<FreePartition, EditError> {
    if !intent.extra.is_empty() {
        return Err(invalid(format!(
            "Brush {}, face {}: opaque saved free-cut fields cannot be remapped",
            intent.solid, intent.side
        )));
    }
    intent.resolve(source).map_err(invalid)
}

fn source_side(document: &Document, solid: u32, side: u32) -> Result<&Side, EditError> {
    surface::side(document, surface::FaceId { solid, side })
        .ok_or_else(|| invalid("Saved free-cut source face does not exist"))
}

fn incidence(part: &FreePartition) -> Vec<[u32; 3]> {
    part.pieces
        .iter()
        .map(|piece| {
            let mut vertices = piece.vertices;
            vertices.sort_unstable();
            vertices
        })
        .collect()
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
        .filter(|id| *id != 0)
        .ok_or_else(|| invalid(format!("Saved free cuts have no identity mapping for {id}")))
}

fn move_textures(
    intent: &mut Intent,
    source: &Side,
    target: &Side,
    part: &FreePartition,
    pose: edit::Transform,
) -> Result<(), EditError> {
    let points = part
        .vertices
        .iter()
        .map(|vertex| vertex.position)
        .collect::<Vec<_>>();
    for surface in &mut intent.surfaces {
        if let Some(texture) = &mut surface.texture {
            let projection =
                crate::transform::transform_projection(&texture.project(source)?, pose)?;
            *texture = texture.reproject(target, &projection, &points)?;
        }
    }
    Ok(())
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
        let values = ids.values().copied().collect::<BTreeSet<_>>();
        if values.contains(&0) || values.len() != ids.len() {
            return Err(invalid(
                "Saved free-cut identity mappings must be unique and positive",
            ));
        }
    }
    let mut remapped = Metadata::default();
    for intent in captured.entries.values() {
        let before = checked_intent(source, intent)?;
        let mut copied = intent.clone();
        copied.solid = mapped_id(solids, intent.solid)?;
        copied.side = mapped_id(sides, intent.side)?;
        copied.corners = intent
            .corners
            .iter()
            .map(|corner| {
                let mut mapped = corner
                    .iter()
                    .map(|id| mapped_id(sides, *id))
                    .collect::<Result<Vec<_>, _>>()?;
                mapped.sort_unstable();
                Ok(mapped)
            })
            .collect::<Result<Vec<_>, EditError>>()?;
        for surface in &mut copied.surfaces {
            surface.solid = copied.solid;
        }
        let after = copied.resolve_topology(target).map_err(invalid)?;
        if incidence(&before) != incidence(&after) {
            return Err(invalid(
                "Saved free-cut triangular piece identity changed during transfer",
            ));
        }
        for (old, new) in before.vertices[..before.corners.len()]
            .iter()
            .zip(&after.vertices[..after.corners.len()])
        {
            let expected = crate::transform::transform_point(old.position, pose)?;
            if !expected.abs_diff_eq(new.position, crate::plane::ON_EPSILON) {
                return Err(invalid(
                    "Saved free-cut target corners disagree with their placement",
                ));
            }
        }
        move_textures(
            &mut copied,
            source_side(source, intent.solid, intent.side)?,
            source_side(target, after.solid, after.side)?,
            &after,
            pose,
        )?;
        copied.resolve(target).map_err(invalid)?;
        let key = (copied.solid, copied.side);
        if remapped.entries.insert(key, copied).is_some() {
            return Err(invalid("Transferred free cuts repeat a target source face"));
        }
    }
    remapped.validate().map_err(invalid)?;
    Ok(remapped)
}

pub fn merge(tx: &mut Transaction, incoming: &Metadata) -> Result<usize, EditError> {
    incoming.validate().map_err(invalid)?;
    if !incoming.extra.is_empty() {
        return Err(invalid(
            "Unscoped free-cut root extensions cannot be imported",
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
                "Transferred free cuts would replace an existing source face",
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

pub struct SavedAxes {
    entries: Vec<SavedEntry>,
}

struct SavedEntry {
    intent: Intent,
    source: Side,
    incidence: Vec<[u32; 3]>,
}

pub fn capture_axes(source: &Document, object: ObjectId) -> Result<Option<SavedAxes>, EditError> {
    let solids = match object {
        ObjectId::Solid(id) => BTreeSet::from([id]),
        ObjectId::Entity(id) => source
            .entity(id)
            .ok_or(EditError::NotFound(object))?
            .solids
            .iter()
            .map(|solid| solid.id)
            .collect(),
    };
    let metadata = Metadata::parse(source.vmf()).map_err(invalid)?;
    let mut entries = Vec::new();
    for intent in metadata
        .entries
        .values()
        .filter(|intent| solids.contains(&intent.solid))
    {
        let part = checked_intent(source, intent)?;
        entries.push(SavedEntry {
            source: source_side(source, intent.solid, intent.side)?.clone(),
            incidence: incidence(&part),
            intent: intent.clone(),
        });
    }
    if entries.is_empty() {
        return Ok(None);
    }
    if !metadata.extra.is_empty() {
        return Err(invalid(
            "Saved free-cut root has opaque extensions that cannot follow transforms",
        ));
    }
    Ok(Some(SavedAxes { entries }))
}

pub fn transform_saved(
    tx: &mut Transaction,
    saved: Option<SavedAxes>,
    pose: edit::Transform,
) -> Result<(), EditError> {
    let Some(saved) = saved else {
        return Ok(());
    };
    let mut metadata = Metadata::parse(tx.vmf()).map_err(invalid)?;
    for entry in saved.entries {
        let key = (entry.intent.solid, entry.intent.side);
        if metadata.entries.get(&key) != Some(&entry.intent) {
            return Err(invalid(
                "Saved free-cut intent changed during its source transform",
            ));
        }
        let mut intent = entry.intent;
        let part = intent.resolve_topology(tx.document()).map_err(invalid)?;
        if incidence(&part) != entry.incidence {
            return Err(invalid(
                "Source transform changes the saved free-cut piece topology",
            ));
        }
        move_textures(
            &mut intent,
            &entry.source,
            source_side(tx.document(), part.solid, part.side)?,
            &part,
            pose,
        )?;
        intent.resolve(tx.document()).map_err(invalid)?;
        metadata.entries.insert(key, intent);
    }
    metadata.write(tx)
}
