use super::*;
use map_core::subobject::{PartitionTexture, PieceTexture};

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
    json!({"shift":texture.shift,"scale":texture.scale,"rotation":texture.rotation})
}

impl Draft {
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

    pub(super) fn effective_texture(
        &self,
        document: &map_core::Document,
        part: &Partition,
        piece: u32,
    ) -> Option<PartitionTexture> {
        self.textures
            .get(&PartitionRef {
                solid: part.source.solid,
                partition: piece,
            })
            .copied()
            .or_else(|| {
                map_core::surface::side(
                    document,
                    map_core::surface::FaceId {
                        solid: part.source.solid,
                        side: part.source.side,
                    },
                )
                .map(PartitionTexture::from_side)
            })
    }
}

impl Session {
    pub fn update_component_partition_texture(
        &mut self,
        token: u64,
        pieces: &[PartitionRef],
        texture: Option<PartitionTexture>,
    ) -> Result<u64, String> {
        let mut draft = self.checked_partition(token)?.clone();
        if draft.rise != 0.0 || self.checked_component_preview(token)?.extrusion.is_some() {
            return Err("Return to flat before changing planned top textures".into());
        }
        let targets = pieces.iter().cloned().collect::<BTreeSet<_>>();
        if pieces.is_empty()
            || pieces.len() > subobject::MAX_SOLIDS * 2
            || targets.len() != pieces.len()
            || targets.iter().any(|piece| {
                !draft.partitions.iter().any(|part| {
                    part.source.solid == piece.solid
                        && part.loops.iter().any(|item| item.id == piece.partition)
                })
            })
        {
            return Err("Choose 1–32 distinct pieces from the current draft".into());
        }
        if let Some(texture) = texture {
            texture.validate().map_err(|error| error.to_string())?;
        }
        for piece in targets {
            if let Some(texture) = texture {
                draft.textures.insert(piece, texture);
            } else {
                draft.textures.remove(&piece);
            }
        }
        self.document
            .preview_snapshot()
            .edit("check planned partition textures", |tx| draft.store(tx))
            .map_err(|error| error.to_string())?;
        Ok(self.install_partition_preview(draft, self.document.preview_snapshot(), None))
    }
}

#[cfg(test)]
mod tests;
