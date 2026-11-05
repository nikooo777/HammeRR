use super::*;
use std::collections::HashSet;

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
        self.effective_piece_material(document, part.source.solid, part.source.side, piece)
    }

    pub(super) fn effective_piece_material<'a>(
        &'a self,
        document: &'a map_core::Document,
        solid: u32,
        side: u32,
        piece: u32,
    ) -> Option<&'a str> {
        self.materials
            .get(&PartitionRef {
                solid,
                partition: piece,
            })
            .map(String::as_str)
            .or_else(|| {
                subobject::solid(document, solid)
                    .ok()?
                    .1
                    .sides
                    .iter()
                    .find(|source| source.id == side)
                    .map(|side| side.material.as_str())
            })
    }

    pub(super) fn store(&self, tx: &mut map_core::Transaction) -> Result<(), EditError> {
        if self.is_free() {
            return map_core::free_partition_metadata::store(
                tx,
                &self.free_partitions,
                &self.free_surfaces(),
            );
        }
        for part in &self.partitions {
            let materials = std::array::from_fn(|partition| {
                self.materials
                    .get(&PartitionRef {
                        solid: part.source.solid,
                        partition: partition as u32,
                    })
                    .cloned()
            });
            let textures = std::array::from_fn(|partition| {
                self.textures
                    .get(&PartitionRef {
                        solid: part.source.solid,
                        partition: partition as u32,
                    })
                    .copied()
            });
            map_core::partition_metadata::store_partition_surfaces(
                tx, part, &materials, &textures,
            )?;
        }
        Ok(())
    }
}

impl Session {
    pub(super) fn partition_surface_draft(&self) -> Option<Draft> {
        if let Some(draft) = self
            .geometry
            .components
            .preview
            .as_ref()
            .and_then(|preview| preview.partition.as_ref())
        {
            return Some(draft.clone());
        }
        let ids = if self.geometry.components.picking.is_some() {
            self.component_solids().into_iter().collect::<BTreeSet<_>>()
        } else {
            self.geometry
                .components
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
        let free_metadata =
            map_core::free_partition_metadata::Metadata::parse(self.render_document().vmf())
                .ok()?;
        let free_partitions = free_metadata
            .resolve(self.render_document())
            .into_iter()
            .filter(|entry| ids.contains(&entry.solid))
            .filter_map(|entry| entry.result.ok())
            .collect::<Vec<_>>();
        let (mut materials, mut textures) =
            super::free::saved_surfaces(&free_metadata, &free_partitions);
        materials.extend(saved_materials(&metadata, &partitions));
        textures.extend(super::textures::saved_textures(
            self.render_document(),
            &metadata,
            &partitions,
        ));
        Some(Draft {
            materials,
            textures,
            partitions,
            free_partitions,
            rise: 0.0,
            roof_texture_lock: false,
            neighbor_texture_lock: false,
            selection: BTreeSet::new(),
        })
    }

    pub(in crate::session) fn partition_face_overlays(&self) -> Vec<render::scene::FaceOverlay> {
        if self
            .geometry
            .components
            .preview
            .as_ref()
            .is_some_and(|preview| {
                preview
                    .extrusion
                    .as_ref()
                    .is_some_and(|draft| draft.spec.height < 0.0)
            })
        {
            return Vec::new();
        }
        let Some(draft) = self
            .partition_surface_draft()
            .filter(|draft| draft.rise == 0.0)
        else {
            return Vec::new();
        };
        draft
            .pieces()
            .into_iter()
            .filter_map(|piece| {
                let target = PartitionRef {
                    solid: piece.solid,
                    partition: piece.id,
                };
                if !draft
                    .materials
                    .keys()
                    .chain(draft.textures.keys())
                    .any(|target| target.solid == piece.solid)
                {
                    return None;
                }
                let source = map_core::surface::side(
                    &self.document,
                    map_core::surface::FaceId {
                        solid: piece.solid,
                        side: piece.side,
                    },
                )?;
                Some(render::scene::FaceOverlay {
                    source: map_core::surface::FaceId {
                        solid: piece.solid,
                        side: piece.side,
                    },
                    points: piece.points,
                    material: draft
                        .effective_piece_material(
                            &self.document,
                            piece.solid,
                            piece.side,
                            piece.id,
                        )?
                        .to_string(),
                    projection: draft
                        .textures
                        .get(&target)
                        .and_then(|texture| texture.project(source).ok()),
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
        let inward = self
            .geometry
            .components
            .preview
            .as_ref()
            .and_then(|preview| preview.extrusion.as_ref())
            .is_some_and(|extrusion| extrusion.spec.height < 0.0);
        let mut visible = BTreeMap::new();
        for piece in draft.pieces() {
            let reference = PartitionRef {
                solid: piece.solid,
                partition: piece.id,
            };
            let outputs = include_outputs
                .then_some(())
                .and_then(|()| {
                    self.geometry
                        .components
                        .preview
                        .as_ref()
                        .and_then(|preview| preview.extrusion.as_ref())
                })
                .map(|extrusion| {
                    if inward {
                        extrusion
                            .plan
                            .fragments
                            .iter()
                            .filter(|output| {
                                output.source == reference.solid
                                    && output
                                        .faces
                                        .iter()
                                        .any(|face| face.partition == Some(reference.partition))
                            })
                            .map(|output| output.solid.id)
                            .collect::<Vec<_>>()
                    } else {
                        extrusion
                            .plan
                            .pieces
                            .iter()
                            .filter(|output| {
                                output.source.solid == reference.solid
                                    && output.partition == reference.partition
                            })
                            .map(|output| output.solid.id)
                            .collect()
                    }
                })
                .unwrap_or_default();
            let shown = if !outputs.is_empty() {
                outputs
                    .iter()
                    .any(|solid| !self.component_hidden(*solid, &hidden))
            } else if self.component_hidden(piece.solid, &no_tools) {
                false
            } else if self.view.tool_textures {
                true
            } else {
                let material = if inward {
                    map_core::surface::side(
                        &self.document,
                        map_core::surface::FaceId {
                            solid: piece.solid,
                            side: piece.side,
                        },
                    )
                    .map(|side| side.material.as_str())
                } else {
                    draft.effective_piece_material(
                        &self.document,
                        piece.solid,
                        piece.side,
                        piece.id,
                    )
                };
                material.is_some_and(|material| {
                    let id = self.materials.load(&self.game, material);
                    !self.materials.get(id).tool
                })
            };
            visible.insert(reference, shown);
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
        let targets = draft.texture_targets(pieces)?;
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
        self.install_partition_preview(draft, self.document.preview_snapshot(), None)
    }
}
