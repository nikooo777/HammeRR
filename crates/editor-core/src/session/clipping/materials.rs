use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use compiler::asset_sources::AssetSourceGuard;
use glam::DVec3;
use map_core::clip::TerrainTexture;
use map_core::surface::{self, FaceId};
use serde_json::{Value, json};
use source_formats::gamefs::GameFs;

use super::Session;

pub(in crate::session) struct Prepared {
    pub(in crate::session) textures: BTreeMap<FaceId, TerrainTexture>,
    dimensions: BTreeMap<String, [u32; 2]>,
    detail_scales: BTreeMap<String, f32>,
    secondary_surfaces: BTreeSet<String>,
    sources: AssetSourceGuard,
    content: u64,
    materials: u64,
}

impl Prepared {
    pub(in crate::session) fn prepare(
        session: &Session,
        faces: &[FaceId],
    ) -> Result<Option<Arc<Self>>, String> {
        if faces.is_empty() {
            return Ok(None);
        }
        let sides = faces
            .iter()
            .map(|face| {
                surface::side(&session.document, *face)
                    .ok_or("Retiled terrain source face is missing")
            })
            .collect::<Result<Vec<_>, _>>()?;
        let captured = compiler::materials::capture_flat_terrain_materials(
            &session.game,
            sides.iter().map(|side| side.material.as_str()),
            &compiler::Control::default(),
        )?;
        let mut textures = BTreeMap::new();
        for (face, side) in faces.iter().zip(sides) {
            let name = side.material.trim().replace('\\', "/").to_ascii_lowercase();
            let mapping_size = *captured
                .dimensions
                .get(&name)
                .ok_or("Retiled terrain material has no captured runtime mapping dimensions")?;
            let [texture_vecs, _] = compiler::map::native_side_projections(side, DVec3::ZERO)?;
            textures.insert(
                *face,
                TerrainTexture {
                    texture_vecs,
                    mapping_size,
                    detail_scale: captured.detail_scales.get(&name).copied(),
                    has_secondary_surface: captured.secondary_surfaces.contains(&name),
                },
            );
        }
        Ok(Some(Arc::new(Self {
            textures,
            dimensions: captured.dimensions,
            detail_scales: captured.detail_scales,
            secondary_surfaces: captured.secondary_surfaces,
            sources: captured.sources,
            content: session.build_content_epoch(),
            materials: session.materials.generation(),
        })))
    }

    pub(in crate::session) fn current(&self, session: &Session) -> Result<(), String> {
        if self.content != session.build_content_epoch()
            || self.materials != session.materials.generation()
        {
            return Err(
                "Material or content settings changed; preview the retiled terrain cut again"
                    .into(),
            );
        }
        self.sources
            .verify_metadata()
            .map_err(|error| format!("Retiled terrain material sources changed: {error}"))
    }

    pub(in crate::session) fn verify(&self, game: &GameFs) -> Result<(), String> {
        self.sources
            .verify_sources(game, &compiler::Control::default())
            .map_err(|error| format!("Retiled terrain material sources changed: {error}"))
    }

    pub(in crate::session) fn report(&self) -> Value {
        json!({"faces":self.textures.len(), "materials":self.dimensions.iter().map(|(name, size)| {
            json!({"name":name,"mapping_size":size,"detail_scale":self.detail_scales.get(name),"has_secondary_surface":self.secondary_surfaces.contains(name)})
        }).collect::<Vec<_>>()})
    }
}
