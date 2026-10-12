use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartitionMaterial {
    pub solid: u32,
    pub partition: u32,
    pub material: String,
}

pub(super) fn checked_materials(
    document: &Document,
    partitions: &[Partition],
    materials: &[PartitionMaterial],
) -> Result<BTreeMap<(u32, u32), String>, EditError> {
    if partitions.is_empty() || partitions.len() > MAX_SOLIDS || materials.len() > MAX_SOLIDS * 2 {
        return Err(invalid(
            "Plan materials for at most 32 pieces in 1–16 brushes",
        ));
    }
    let mut sources = BTreeMap::new();
    for partition in partitions {
        checked_partition(document, partition)?;
        if sources.insert(partition.source.solid, partition).is_some() {
            return Err(invalid("Select one partition per source brush"));
        }
    }
    let mut result = BTreeMap::new();
    let mut bytes = 0usize;
    for material in materials {
        let name = crate::partition_metadata::checked_top_material(&material.material)
            .map_err(EditError::Invalid)?;
        bytes += material.material.len();
        if bytes > crate::partition_metadata::MAX_TEXT_BYTES {
            return Err(invalid(
                "Planned piece materials exceed the shared 64 KiB text budget",
            ));
        }
        let source = sources
            .get(&material.solid)
            .ok_or_else(|| invalid("Choose material targets from the current partitions"))?;
        if !source
            .loops
            .iter()
            .any(|piece| piece.id == material.partition)
            || result
                .insert(
                    (material.solid, material.partition),
                    name.to_ascii_uppercase(),
                )
                .is_some()
        {
            return Err(invalid("Choose distinct existing partition piece IDs"));
        }
    }
    Ok(result)
}

pub fn crease_partitions_with_materials(
    tx: &mut Transaction,
    partitions: &[Partition],
    rise: f64,
    texture_lock: bool,
    materials: &[PartitionMaterial],
) -> Result<CreasePlan, EditError> {
    let materials = checked_materials(tx.document(), partitions, materials)?;
    let mut plan = super::crease::crease_partition_roofs(tx, partitions, rise, texture_lock)?;
    for part in partitions {
        let change = plan
            .changes
            .iter()
            .find(|change| change.source == part.source)
            .unwrap();
        let replacement = plan
            .replacements
            .iter_mut()
            .find(|replacement| replacement.solid.id == part.source.solid)
            .unwrap();
        let positions = part.positions(rise);
        for piece in &part.loops {
            let Some(material) = materials.get(&(part.source.solid, piece.id)) else {
                continue;
            };
            let points = piece
                .vertices
                .iter()
                .map(|vertex| positions[*vertex as usize])
                .collect::<Vec<_>>();
            let sides = replacement
                .solid
                .sides
                .iter_mut()
                .filter(|side| side.id == part.source.side || side.id == change.new_side)
                .filter(|side| {
                    let plane = Plane::from_points(side.plane.0).unwrap();
                    points
                        .iter()
                        .all(|point| plane.distance_to(*point).abs() < EPSILON)
                })
                .collect::<Vec<_>>();
            let [side]: [&mut source_formats::vmf::Side; 1] = sides
                .try_into()
                .map_err(|_: Vec<_>| invalid("Planned piece must match exactly one roof face"))?;
            side.material.clone_from(material);
        }
    }
    apply(tx, &plan.replacements)?;
    Ok(plan)
}

#[cfg(test)]
mod ridges;
#[cfg(test)]
mod tests;
