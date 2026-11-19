use std::collections::{BTreeMap, BTreeSet};

use glam::DVec3;
use source_formats::{bsp, vmf::Vmf};

use crate::{
    BrushGeometry, EditError, Transaction,
    overlay::{self, water},
    overlay_projection::Frame,
};

const MAX_BINDINGS: usize = bsp::MAX_OVERLAYS * bsp::MAX_OVERLAY_FACES;

struct Candidate {
    entity: u32,
    location: Option<water::Location>,
    definition: overlay::Definition,
    branches: BTreeSet<u32>,
    unchanged: BTreeSet<u32>,
}

struct Write {
    entity: u32,
    location: Option<water::Location>,
    sides: String,
}

pub(crate) fn remap(
    tx: &mut Transaction<'_>,
    sides: &BTreeMap<u32, Vec<u32>>,
) -> Result<(), EditError> {
    let writes = plan(tx.vmf(), sides).map_err(EditError::Invalid)?;
    for write in writes {
        let entity = tx.entity_mut(write.entity)?;
        if let Some(location) = write.location {
            water::update_at(entity, location, [("sides".to_owned(), write.sides)])
                .map_err(EditError::Invalid)?;
        } else {
            entity.properties.set("sides", write.sides);
        }
    }
    Ok(())
}

fn plan(vmf: &Vmf, sides: &BTreeMap<u32, Vec<u32>>) -> Result<Vec<Write>, String> {
    if sides.is_empty() {
        return Ok(Vec::new());
    }
    let mut candidates = Vec::new();
    let mut required = BTreeSet::<u32>::new();
    let mut work = 0usize;
    let mapped = |value: &str| {
        value
            .split_whitespace()
            .filter_map(|token| token.parse::<u32>().ok())
            .any(|id| sides.contains_key(&id))
    };
    for entity in &vmf.entities {
        let entity_id = entity.id;
        let error = |message: &str| format!("entity:{entity_id}: {message}");
        let definitions = if water::is_transition(entity) {
            if ["sides", "sides2"]
                .iter()
                .any(|key| entity.properties.get_all(key).any(mapped))
            {
                return Err(error(
                    "this cut touches legacy shoreline source references; edit the transition first",
                ));
            }
            if !water::side_lists(entity).any(mapped) {
                continue;
            }
            water::projections(entity)
                .map_err(|message| error(&message))?
                .into_iter()
                .map(|(location, definition)| (Some(location), definition))
                .collect::<Vec<_>>()
        } else if entity
            .classname()
            .is_some_and(|class| class.eq_ignore_ascii_case("info_overlay"))
            && entity.properties.get_all("sides").any(mapped)
        {
            if entity.properties.get_all("sides").count() != 1 {
                return Err(error("overlay sides property is ambiguous"));
            }
            vec![(
                None,
                overlay::parse(entity).map_err(|message| error(&message))?,
            )]
        } else {
            continue;
        };
        for (location, definition) in definitions {
            if !definition.sides.iter().any(|side| sides.contains_key(side)) {
                continue;
            }
            if location.is_some() && !water::topology_fields_known(entity) {
                return Err(error(
                    "this cut touches a water transition with unknown nested fields",
                ));
            }
            if candidates.len() >= bsp::MAX_OVERLAYS + bsp::MAX_WATER_OVERLAYS {
                return Err(error("overlay rebinding exceeds the native overlay limits"));
            }
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
                location,
                definition,
                branches,
                unchanged,
            });
        }
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
    fn write(self, surfaces: &BTreeMap<u32, Vec<DVec3>>) -> Result<Option<Write>, String> {
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
            (!output.iter().copied().eq(self.definition.sides)).then(|| Write {
                entity: self.entity,
                location: self.location,
                sides: output
                    .into_iter()
                    .map(|id| id.to_string())
                    .collect::<Vec<_>>()
                    .join(" "),
            }),
        )
    }
}
