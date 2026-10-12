use super::*;
use std::collections::HashSet;

#[cfg(test)]
mod tests;

pub(super) fn saved_materials(
    metadata: &map_core::partition_metadata::Metadata,
    partitions: &[Partition],
) -> BTreeMap<PartitionRef, String> {
    partitions
        .iter()
        .flat_map(|part| {
            metadata
                .entries
                .get(&(part.source.solid, part.source.side))
                .filter(|intent| intent.source == part.source)
                .into_iter()
                .flat_map(move |intent| {
                    intent
                        .top_materials
                        .iter()
                        .enumerate()
                        .filter_map(move |(index, material)| {
                            material.as_ref().map(|material| {
                                (
                                    PartitionRef {
                                        solid: part.source.solid,
                                        partition: index as u32,
                                    },
                                    material.clone(),
                                )
                            })
                        })
                })
        })
        .collect()
}

impl Draft {
    pub(in crate::session::components) fn material_plan(
        &self,
    ) -> Vec<subobject::PartitionMaterial> {
        self.materials
            .iter()
            .map(|(piece, material)| subobject::PartitionMaterial {
                solid: piece.solid,
                partition: piece.partition,
                material: material.clone(),
            })
            .collect()
    }

    pub(super) fn effective_material<'a>(
        &'a self,
        document: &'a map_core::Document,
        part: &Partition,
        piece: u32,
    ) -> Option<&'a str> {
        self.materials
            .get(&PartitionRef {
                solid: part.source.solid,
                partition: piece,
            })
            .map(String::as_str)
            .or_else(|| {
                subobject::solid(document, part.source.solid)
                    .ok()?
                    .1
                    .sides
                    .iter()
                    .find(|side| side.id == part.source.side)
                    .map(|side| side.material.as_str())
            })
    }

    pub(super) fn store(&self, tx: &mut map_core::Transaction) -> Result<(), EditError> {
        for part in &self.partitions {
            let materials = std::array::from_fn(|partition| {
                self.materials
                    .get(&PartitionRef {
                        solid: part.source.solid,
                        partition: partition as u32,
                    })
                    .cloned()
            });
            map_core::partition_metadata::store_partition_materials(tx, part, &materials)?;
        }
        Ok(())
    }
}

impl Session {
    pub(super) fn partition_surface_draft(&self) -> Option<Draft> {
        if let Some(draft) = self
            .components
            .preview
            .as_ref()
            .and_then(|preview| preview.partition.as_ref())
        {
            return Some(draft.clone());
        }
        let ids = if self.components.picking.is_some() {
            self.component_solids().into_iter().collect::<BTreeSet<_>>()
        } else {
            self.components
                .selection
                .iter()
                .map(|component| component.solid)
                .collect()
        };
        if ids.is_empty() {
            return None;
        }
        let metadata =
            map_core::partition_metadata::Metadata::parse(self.render_document().vmf()).ok()?;
        let partitions = metadata
            .resolve(self.render_document())
            .into_iter()
            .filter(|entry| ids.contains(&entry.source.solid))
            .filter_map(|entry| entry.result.ok())
            .collect::<Vec<_>>();
        Some(Draft {
            materials: saved_materials(&metadata, &partitions),
            partitions,
            rise: 0.0,
            roof_texture_lock: false,
            selection: BTreeSet::new(),
        })
    }

    pub(in crate::session) fn partition_face_overlays(&self) -> Vec<render::scene::FaceOverlay> {
        let Some(draft) = self
            .partition_surface_draft()
            .filter(|draft| draft.rise == 0.0)
        else {
            return Vec::new();
        };
        draft
            .partitions
            .iter()
            .filter(|part| {
                draft
                    .materials
                    .keys()
                    .any(|piece| piece.solid == part.source.solid)
            })
            .flat_map(|part| {
                part.loops.iter().filter_map(|piece| {
                    Some(render::scene::FaceOverlay {
                        source: map_core::surface::FaceId {
                            solid: part.source.solid,
                            side: part.source.side,
                        },
                        points: piece
                            .vertices
                            .map(|index| part.vertices[index as usize].position),
                        material: draft
                            .effective_material(&self.document, part, piece.id)?
                            .to_string(),
                    })
                })
            })
            .collect()
    }

    pub(super) fn partition_piece_visibility(
        &mut self,
        draft: &Draft,
    ) -> BTreeMap<PartitionRef, bool> {
        self.partition_visibility(draft, true)
    }

    pub(super) fn partition_top_visibility(
        &mut self,
        draft: &Draft,
    ) -> BTreeMap<PartitionRef, bool> {
        self.partition_visibility(draft, false)
    }

    fn partition_visibility(
        &mut self,
        draft: &Draft,
        include_outputs: bool,
    ) -> BTreeMap<PartitionRef, bool> {
        let hidden = self.hidden_tool_brushes();
        let no_tools = HashSet::new();
        let mut visible = BTreeMap::new();
        for part in &draft.partitions {
            for piece in &part.loops {
                let reference = PartitionRef {
                    solid: part.source.solid,
                    partition: piece.id,
                };
                let output = include_outputs.then_some(()).and_then(|()| {
                    self.components
                        .preview
                        .as_ref()
                        .and_then(|preview| preview.extrusion.as_ref())
                        .and_then(|extrusion| {
                            extrusion.plan.pieces.iter().find(|output| {
                                output.source.solid == reference.solid
                                    && output.partition == reference.partition
                            })
                        })
                        .map(|output| output.solid.id)
                });
                let shown = if let Some(solid) = output {
                    !self.component_hidden(solid, &hidden)
                } else if self.component_hidden(part.source.solid, &no_tools) {
                    false
                } else if self.view.tool_textures {
                    true
                } else {
                    draft
                        .effective_material(&self.document, part, piece.id)
                        .is_some_and(|material| {
                            let id = self.materials.load(&self.game, material);
                            !self.materials.get(id).tool
                        })
                };
                visible.insert(reference, shown);
            }
        }
        visible
    }

    pub fn update_component_partition_material(
        &mut self,
        token: u64,
        pieces: &[PartitionRef],
        material: Option<&str>,
    ) -> Result<u64, String> {
        let mut draft = self.checked_partition(token)?.clone();
        if draft.rise != 0.0 || self.checked_component_preview(token)?.extrusion.is_some() {
            return Err("Return to flat before changing planned top materials".into());
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
        let material = material
            .map(map_core::partition_metadata::normalize_top_material)
            .transpose()?;
        for piece in targets {
            if let Some(material) = &material {
                draft.materials.insert(piece, material.clone());
            } else {
                draft.materials.remove(&piece);
            }
        }
        self.document
            .preview_snapshot()
            .edit("check planned partition materials", |tx| draft.store(tx))
            .map_err(|error| error.to_string())?;
        Ok(self.install_partition_preview(draft, self.document.preview_snapshot(), None))
    }
}
