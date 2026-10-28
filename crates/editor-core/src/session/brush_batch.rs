use glam::DVec3;
use map_core::{ObjectId, block_batch};
use serde::Deserialize;

use super::{Actor, SelectMode, Session};

pub const MAX_IMPORT_BYTES: usize = 512 * 1024;
pub const DEFAULT_MATERIAL: &str = "dev/dev_measuregeneric01";

#[derive(Debug, Clone, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BlockSpec {
    /// Give exactly mins+maxs or center+size. Bounds must be within ±65536 units and at least 0.125 units thick.
    pub mins: Option<[f64; 3]>,
    pub maxs: Option<[f64; 3]>,
    pub center: Option<[f64; 3]>,
    pub size: Option<[f64; 3]>,
    /// Right-handed world [X,Y,Z] degrees, applied X then Y then Z about this block's center. Texture axes follow the rotation.
    pub rotation: Option<[f64; 3]>,
    /// Material on all six faces; defaults to dev/dev_measuregeneric01. At most 1024 bytes.
    #[schemars(length(max = 1024))]
    pub material: Option<String>,
    /// Texture [U,V] scales on all six faces; signed, finite and nonzero, using the shared face-edit rules. Defaults to [0.25,0.25].
    pub texture_scale: Option<[f64; 2]>,
    /// World units per lighting pixel, 1–1024; defaults to 16.
    #[schemars(range(min = 1, max = 1024))]
    pub lightmap_scale: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Batch {
    /// 1–256 heterogeneous blocks, created in this order as one Undo step. An invalid block rejects the entire batch.
    #[schemars(length(min = 1, max = 256))]
    pub blocks: Vec<BlockSpec>,
    /// Snap each block's corners to the shared grid before rotation; defaults to false for exact coordinates.
    #[serde(default)]
    pub snap: bool,
}

impl Batch {
    pub fn from_json(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_IMPORT_BYTES {
            return Err(format!(
                "Brush batch files must be at most {MAX_IMPORT_BYTES} bytes"
            ));
        }
        let batch: Self = serde_json::from_slice(bytes)
            .map_err(|error| format!("Invalid brush batch JSON: {error}"))?;
        validate_count(batch.blocks.len())?;
        Ok(batch)
    }
}

fn validate_count(count: usize) -> Result<(), String> {
    if !(1..=block_batch::MAX_BLOCKS).contains(&count) {
        return Err(format!(
            "Create between 1 and {} blocks per batch",
            block_batch::MAX_BLOCKS
        ));
    }
    Ok(())
}

impl BlockSpec {
    fn resolve(
        &self,
        spatial: crate::spatial::Settings,
        snap: bool,
    ) -> Result<block_batch::Spec, String> {
        if self
            .material
            .as_ref()
            .is_some_and(|material| material.len() > block_batch::MAX_MATERIAL_BYTES)
        {
            return Err(format!(
                "Material paths must be at most {} bytes",
                block_batch::MAX_MATERIAL_BYTES
            ));
        }
        let (mins, maxs) = match (self.mins, self.maxs, self.center, self.size) {
            (Some(mins), Some(maxs), None, None) => {
                (DVec3::from_array(mins), DVec3::from_array(maxs))
            }
            (None, None, Some(center), Some(size)) => {
                let (center, size) = (DVec3::from_array(center), DVec3::from_array(size));
                (center - size * 0.5, center + size * 0.5)
            }
            _ => return Err("Give either mins and maxs, or center and size".into()),
        };
        crate::spatial::validate_bounds(mins, maxs)?;
        let (mins, maxs) = if snap {
            (spatial.snap_point(mins), spatial.snap_point(maxs))
        } else {
            (mins, maxs)
        };
        Ok(block_batch::Spec {
            mins,
            maxs,
            rotation: DVec3::from_array(self.rotation.unwrap_or([0.0; 3])),
            material: self
                .material
                .clone()
                .unwrap_or_else(|| DEFAULT_MATERIAL.into()),
            texture_scale: self.texture_scale,
            lightmap_scale: self.lightmap_scale,
        })
    }
}

impl Session {
    pub fn create_blocks(
        &mut self,
        actor: Actor,
        blocks: &[BlockSpec],
        snap: bool,
    ) -> Result<Vec<ObjectId>, String> {
        self.ensure_not_closing()?;
        validate_count(blocks.len())?;
        let specs = blocks
            .iter()
            .enumerate()
            .map(|(index, block)| {
                block
                    .resolve(self.spatial, snap)
                    .map_err(|error| format!("Block {}: {error}", index + 1))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let label = if specs.len() == 1 {
            "create block"
        } else {
            "create blocks"
        };
        let ids = self.edit(actor, label, |tx| block_batch::create(tx, &specs))?;
        self.select(&ids, SelectMode::Replace)?;
        Ok(ids)
    }
}
