use std::collections::{BTreeMap, BTreeSet};

use glam::DVec3;
use source_formats::{bsp, vmf::Vmf};

use crate::{BrushGeometry, EditError, Transaction, overlay, overlay_projection::Frame};

const MAX_BINDINGS: usize = bsp::MAX_OVERLAYS * bsp::MAX_OVERLAY_FACES;

struct Candidate {
    entity: u32,
    definition: overlay::Definition,
    branches: BTreeSet<u32>,
    unchanged: BTreeSet<u32>,
}

pub(crate) fn remap(
    tx: &mut Transaction<'_>,
    sides: &BTreeMap<u32, Vec<u32>>,
) -> Result<(), EditError> {
    let writes = plan(tx.vmf(), sides).map_err(EditError::Invalid)?;
    for (entity, sides) in writes {
        tx.entity_mut(entity)?.properties.set("sides", sides);
    }
    Ok(())
}

fn plan(vmf: &Vmf, sides: &BTreeMap<u32, Vec<u32>>) -> Result<Vec<(u32, String)>, String> {
    if sides.is_empty() {
        return Ok(Vec::new());
    }
    let mut candidates = Vec::new();
    let mut required = BTreeSet::<u32>::new();
    let mut work = 0usize;
    for entity in &vmf.entities {
        if !entity
            .classname()
            .is_some_and(|class| class.eq_ignore_ascii_case("info_overlay"))
            || !entity.properties.get_all("sides").any(|value| {
                value
                    .split_whitespace()
                    .filter_map(|token| token.parse::<u32>().ok())
                    .any(|id| sides.contains_key(&id))
            })
        {
            continue;
        }
        let entity_id = entity.id;
        let error = |message: &str| format!("entity:{entity_id}: {message}");
        if entity.properties.get_all("sides").count() != 1 {
            return Err(error("overlay sides property is ambiguous"));
        }
        if candidates.len() >= bsp::MAX_OVERLAYS {
            return Err(error("overlay rebinding exceeds the native overlay limit"));
        }
        let definition = overlay::parse(entity).map_err(|message| error(&message))?;
        let mut branches = BTreeSet::new();
        let mut unchanged = BTreeSet::new();
        for side in &definition.sides {
            if let Some(replacements) = sides.get(side) {
                work = work.saturating_add(replacements.len());
                if work > MAX_BINDINGS {
                    return Err(error("overlay rebinding exceeds its face reference budget"));
                }
                branches.extend(replacements);
            } else {
                unchanged.insert(*side);
            }
        }
        required.extend(branches.iter().chain(unchanged.iter()));
        candidates.push(Candidate {
            entity: entity_id,
            definition,
            branches,
            unchanged,
        });
    }
    if candidates.is_empty() {
        return Ok(Vec::new());
    }

    let mut surfaces = BTreeMap::new();
    let owners = vmf.world.solids.iter().chain(
        vmf.entities
            .iter()
            .filter(|entity| {
                entity
                    .classname()
                    .is_some_and(|name| name.eq_ignore_ascii_case("func_detail"))
            })
            .flat_map(|entity| &entity.solids),
    );
    for solid in owners.filter(|solid| solid.sides.iter().any(|side| required.contains(&side.id))) {
        let geometry = BrushGeometry::from_solid(solid).map_err(|error| {
            format!(
                "Brush {}: cannot retain overlay bindings: {error}",
                solid.id
            )
        })?;
        let terrain = solid.sides.iter().any(|side| side.displacement.is_some());
        for face in geometry.faces {
            let side = &solid.sides[face.side];
            if required.contains(&side.id)
                && (!terrain || side.displacement.is_some())
                && surfaces
                    .insert(
                        side.id,
                        face.vertices
                            .into_iter()
                            .map(crate::displacement::native::stored_vertex)
                            .collect(),
                    )
                    .is_some()
            {
                return Err(format!("Duplicate overlay receiver side {}", side.id));
            }
        }
    }
    candidates
        .into_iter()
        .filter_map(|candidate| candidate.write(&surfaces).transpose())
        .collect()
}

impl Candidate {
    fn write(self, surfaces: &BTreeMap<u32, Vec<DVec3>>) -> Result<Option<(u32, String)>, String> {
        let frame = Frame::new(&self.definition)?;
        let intersects = |id: &u32| surfaces.get(id).is_some_and(|face| frame.covers(face));
        let descendants = self.branches.into_iter().filter(intersects);
        let mut output = self.unchanged;
        if let Some(id) = output.iter().find(|id| !surfaces.contains_key(id)) {
            return Err(format!(
                "entity:{}: unchanged overlay side {id} is not a world or detail receiver",
                self.entity
            ));
        }
        output.extend(descendants);
        if !output.iter().any(intersects) {
            return Err(format!(
                "entity:{}: this cut removes the overlay's entire projection; rebind or remove the overlay first",
                self.entity
            ));
        }
        if output.len() > bsp::MAX_OVERLAY_FACES {
            return Err(format!(
                "entity:{}: this cut exceeds the overlay's {}-side limit",
                self.entity,
                bsp::MAX_OVERLAY_FACES
            ));
        }
        Ok(
            (!output.iter().copied().eq(self.definition.sides)).then(|| {
                (
                    self.entity,
                    output
                        .into_iter()
                        .map(|id| id.to_string())
                        .collect::<Vec<_>>()
                        .join(" "),
                )
            }),
        )
    }
}
