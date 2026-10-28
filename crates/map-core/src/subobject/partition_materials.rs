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
    super::partition_textures::crease_partitions_with_surfaces(
        tx,
        partitions,
        rise,
        texture_lock,
        materials,
        &[],
    )
}
