use super::*;
use map_core::subobject::{PartitionTexture, PieceTexture};
use std::collections::btree_map::Entry;

pub(super) fn saved_textures(
    document: &map_core::Document,
    metadata: &map_core::partition_metadata::Metadata,
    partitions: &[Partition],
) -> BTreeMap<PartitionRef, PartitionTexture> {
    partitions
        .iter()
        .flat_map(|part| {
            metadata
                .entries
                .get(&(part.source.solid, part.source.side))
                .filter(|intent| intent.source == part.source && intent.resolve(document).is_ok())
                .into_iter()
                .flat_map(move |intent| {
                    intent
                        .top_textures
                        .iter()
                        .enumerate()
                        .filter_map(move |(index, texture)| {
                            texture.map(|texture| {
                                (
                                    PartitionRef {
                                        solid: part.source.solid,
                                        partition: index as u32,
                                    },
                                    texture,
                                )
                            })
                        })
                })
        })
        .collect()
}

pub(super) fn texture_json(texture: PartitionTexture) -> Value {
    json!({"shift":texture.shift,"scale":texture.scale,"rotation":texture.rotation,"axes":texture.axes})
}

impl Draft {
    pub(super) fn texture_targets(
        &self,
        pieces: &[PartitionRef],
    ) -> Result<BTreeSet<PartitionRef>, String> {
        let targets = pieces.iter().cloned().collect::<BTreeSet<_>>();
        if pieces.is_empty()
            || pieces.len() > subobject::MAX_COMPONENTS
            || targets.len() != pieces.len()
            || targets.iter().any(|piece| !self.contains_piece(piece))
        {
            return Err("Choose distinct existing pieces from the current draft".into());
        }
        Ok(targets)
    }

    pub(in crate::session::components) fn texture_plan(&self) -> Vec<PieceTexture> {
        self.textures
            .iter()
            .map(|(piece, texture)| PieceTexture {
                solid: piece.solid,
                partition: piece.partition,
                texture: *texture,
            })
            .collect()
    }

    pub(super) fn effective_piece_texture(
        &self,
        document: &map_core::Document,
        solid: u32,
        side: u32,
        piece: u32,
    ) -> Option<PartitionTexture> {
        self.textures
            .get(&PartitionRef {
                solid,
                partition: piece,
            })
            .copied()
            .or_else(|| {
                map_core::surface::side(document, map_core::surface::FaceId { solid, side })
                    .map(PartitionTexture::from_side)
            })
    }

    pub(super) fn effective_texture_json(
        &self,
        document: &map_core::Document,
        part: &Partition,
        piece: u32,
    ) -> Option<Value> {
        self.effective_piece_texture_json(document, part.source.solid, part.source.side, piece)
    }

    pub(super) fn effective_piece_texture_json(
        &self,
        document: &map_core::Document,
        solid: u32,
        side: u32,
        piece: u32,
    ) -> Option<Value> {
        let texture = self.effective_piece_texture(document, solid, side, piece)?;
        let source = map_core::surface::side(document, map_core::surface::FaceId { solid, side })?;
        let projection = texture.project(source).ok()?;
        Some(texture_json(PartitionTexture::from_projection(&projection)))
    }
}

impl Session {
    pub fn place_component_partition_texture(
        &mut self,
        token: u64,
        pieces: &[PartitionRef],
        fit: bool,
        justify: [Option<map_core::surface::Justify>; 2],
    ) -> Result<u64, String> {
        let mut draft = self.checked_flat_texture_partition(token)?;
        let targets = draft.texture_targets(pieces)?;
        if !fit && justify.iter().all(Option::is_none) {
            return Err("Choose Fit or at least one texture justification".into());
        }
        let mut textures = Vec::new();
        let mut material_dimensions = BTreeMap::new();
        for target in targets {
            let piece = draft
                .pieces()
                .into_iter()
                .find(|piece| piece.solid == target.solid && piece.id == target.partition)
                .unwrap();
            let name = draft
                .effective_piece_material(&self.document, piece.solid, piece.side, piece.id)
                .ok_or("Partition source material does not exist")?;
            let dimensions = match material_dimensions.entry(name.trim().to_ascii_lowercase()) {
                Entry::Occupied(entry) => *entry.get(),
                Entry::Vacant(entry) => {
                    let material = self.material_preview(name)?;
                    if material.missing
                        || material.image.is_none()
                        || material.width == 0
                        || material.height == 0
                    {
                        return Err(format!(
                            "Cannot place {name}: texture dimensions unavailable"
                        ));
                    }
                    *entry.insert([material.width, material.height])
                }
            };
            let source = map_core::surface::side(
                &self.document,
                map_core::surface::FaceId {
                    solid: piece.solid,
                    side: piece.side,
                },
            )
            .ok_or("Partition source face does not exist")?;
            let texture = draft
                .effective_piece_texture(&self.document, piece.solid, piece.side, piece.id)
                .ok_or("Partition source texture does not exist")?
                .place(source, &piece.points, dimensions, fit, justify)
                .map_err(|error| error.to_string())?;
            textures.push((target, texture));
        }
        draft.textures.extend(textures);
        self.install_checked_texture_partition(draft)
    }

    fn checked_flat_texture_partition(&self, token: u64) -> Result<Draft, String> {
        let draft = self.checked_partition(token)?;
        if draft.rise != 0.0 || self.checked_component_preview(token)?.extrusion.is_some() {
            return Err("Return to flat before changing planned top textures".into());
        }
        Ok(draft.clone())
    }

    fn install_checked_texture_partition(&mut self, draft: Draft) -> Result<u64, String> {
        self.document
            .preview_snapshot()
            .edit("check planned partition textures", |tx| draft.store(tx))
            .map_err(|error| error.to_string())?;
        self.install_partition_preview(draft, self.document.preview_snapshot(), None)
    }

    pub fn update_component_partition_texture(
        &mut self,
        token: u64,
        pieces: &[PartitionRef],
        texture: Option<PartitionTexture>,
    ) -> Result<u64, String> {
        let mut draft = self.checked_flat_texture_partition(token)?;
        let targets = draft.texture_targets(pieces)?;
        if let Some(texture) = texture {
            texture.validate().map_err(|error| error.to_string())?;
            for solid in targets
                .iter()
                .map(|target| target.solid)
                .collect::<BTreeSet<_>>()
            {
                let side = draft.source_side(solid).unwrap();
                let source = map_core::surface::side(
                    &self.document,
                    map_core::surface::FaceId { solid, side },
                )
                .ok_or("Partition source face does not exist")?;
                texture.project(source).map_err(|error| error.to_string())?;
            }
        }
        for piece in targets {
            if let Some(texture) = texture {
                draft.textures.insert(piece, texture);
            } else {
                draft.textures.remove(&piece);
            }
        }
        self.install_checked_texture_partition(draft)
    }
}
