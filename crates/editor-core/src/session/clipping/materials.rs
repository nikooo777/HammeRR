use std::collections::BTreeMap;
use std::sync::Arc;

use compiler::asset_sources::AssetSourceGuard;
use glam::DVec3;
use map_core::clip::TerrainTexture;
use map_core::surface::{self, FaceId};
use serde_json::{Value, json};
use source_formats::gamefs::GameFs;

use super::Session;

pub(super) struct Prepared {
    pub(super) textures: BTreeMap<FaceId, TerrainTexture>,
    dimensions: BTreeMap<String, [u32; 2]>,
    sources: AssetSourceGuard,
    content: u64,
    materials: u64,
}

impl Prepared {
    pub(super) fn prepare(
        session: &Session,
        faces: &[FaceId],
    ) -> Result<Option<Arc<Self>>, String> {
        if faces.is_empty() {
            return Ok(None);
        }
        let sides = faces
            .iter()
            .map(|face| {
                surface::side(&session.document, *face).ok_or("Flat terrain source face is missing")
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
                .ok_or("Flat terrain material has no captured runtime mapping dimensions")?;
            let [texture_vecs, _] = compiler::map::native_side_projections(side, DVec3::ZERO)?;
            textures.insert(
                *face,
                TerrainTexture {
                    texture_vecs,
                    mapping_size,
                },
            );
        }
        Ok(Some(Arc::new(Self {
            textures,
            dimensions: captured.dimensions,
            sources: captured.sources,
            content: session.build_content_epoch(),
            materials: session.materials.generation(),
        })))
    }

    pub(super) fn current(&self, session: &Session) -> Result<(), String> {
        if self.content != session.build_content_epoch()
            || self.materials != session.materials.generation()
        {
            return Err(
                "Material or content settings changed; preview the flat terrain cut again".into(),
            );
        }
        self.sources
            .verify_metadata()
            .map_err(|error| format!("Flat terrain material sources changed: {error}"))
    }

    pub(super) fn verify(&self, game: &GameFs) -> Result<(), String> {
        self.sources
            .verify_sources(game, &compiler::Control::default())
            .map_err(|error| format!("Flat terrain material sources changed: {error}"))
    }

    pub(super) fn report(&self) -> Value {
        json!({"faces":self.textures.len(), "materials":self.dimensions.iter().map(|(name, size)| {
            json!({"name":name,"mapping_size":size})
        }).collect::<Vec<_>>()})
    }
}
