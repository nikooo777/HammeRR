use map_core::subobject::{Partition, VertexSource};

use super::*;

mod materials;
mod picking;
mod textures;

pub(super) fn consume_saved(
    tx: &mut map_core::Transaction,
    requests: &[Crease],
) -> Result<(), EditError> {
    if let Ok(mut metadata) = map_core::partition_metadata::Metadata::parse(tx.vmf()) {
        let count = metadata.entries.len();
        for request in requests {
            metadata.entries.remove(&(request.solid, request.side));
        }
        if metadata.entries.len() != count {
            metadata.write(tx)?;
        }
    }
    Ok(())
}

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Deserialize, schemars::JsonSchema,
)]
pub struct PartitionRef {
    pub solid: u32,
    pub partition: u32,
}

#[derive(Clone)]
pub(super) struct Draft {
    pub partitions: Vec<Partition>,
    pub rise: f64,
    pub roof_texture_lock: bool,
    pub selection: BTreeSet<PartitionRef>,
    pub materials: BTreeMap<PartitionRef, String>,
    pub textures: BTreeMap<PartitionRef, subobject::PartitionTexture>,
}

impl Draft {
    fn requests(&self) -> Vec<Crease> {
        self.partitions
            .iter()
            .map(|partition| partition.source.clone())
            .collect()
    }

    pub fn report(&self, document: &map_core::Document) -> Value {
        let parameters = self
            .partitions
            .iter()
            .map(|partition| partition.cut().unwrap().edge_parameters)
            .collect::<Vec<_>>();
        let edge_parameters: [Option<f64>; 2] = std::array::from_fn(|index| {
            parameters
                .first()
                .map(|first| first[index])
                .filter(|value| {
                    parameters
                        .iter()
                        .all(|parameters| parameters[index] == *value)
                })
        });
        json!({"ridge_supported":self.partitions.iter().all(|part|part.loops.iter().all(|piece|piece.vertices.len()==4)),"edge_parameters":edge_parameters,"rise":self.rise,"roof_texture_lock":self.roof_texture_lock,"temporary":true,"selection":self.selection.iter().map(|part|json!({"solid":part.solid,"partition":part.partition})).collect::<Vec<_>>(),
            "partitions":self.partitions.iter().map(|part| {
                let positions = part.positions(self.rise);
                let owner = match part.owner {ObjectId::Solid(id)=>format!("solid:{id}"),ObjectId::Entity(id)=>format!("entity:{id}")};
                json!({"solid":part.source.solid,"source_side":part.source.side,"source_corner_count":part.source_corner_count(),"edges":part.source.edges,"owner":owner,"plane":part.plane.map(|point|point.to_array()),
                    "vertices":part.vertices.iter().map(|vertex|json!({"id":vertex.id,"position":vertex.position.to_array(),"preview_position":positions[vertex.id as usize].to_array(),"source":match &vertex.source {VertexSource::Corner(sides)=>json!({"kind":"corner","sides":sides}),VertexSource::Edge{corners,parameter}=>json!({"kind":"edge","corners":corners,"parameter":parameter})}})).collect::<Vec<_>>(),
                    "loops":part.loops.iter().map(|piece|json!({"id":piece.id,"vertices":piece.vertices,"shape":match piece.vertices.len(){3=>"triangle",4=>"quad",5=>"pentagon",6=>"hexagon",7=>"heptagon",8=>"octagon",9=>"nonagon",_=>unreachable!()},"source_side":piece.source_side,"top_material":self.materials.get(&PartitionRef{solid:part.source.solid,partition:piece.id}).map(|name|name.to_ascii_lowercase()),"effective_material":self.effective_material(document, part, piece.id).map(str::to_ascii_lowercase),"inherited":!self.materials.contains_key(&PartitionRef{solid:part.source.solid,partition:piece.id}),"texture_plan":self.textures.get(&PartitionRef{solid:part.source.solid,partition:piece.id}).copied().map(textures::texture_json),"effective_texture":self.effective_texture(document,part,piece.id).map(textures::texture_json),"texture_inherited":!self.textures.contains_key(&PartitionRef{solid:part.source.solid,partition:piece.id}),"selected":self.selection.contains(&PartitionRef{solid:part.source.solid,partition:piece.id})})).collect::<Vec<_>>()})
            }).collect::<Vec<_>>()})
    }
}

impl Session {
    pub fn saved_partition_report(&self) -> Value {
        let metadata = map_core::partition_metadata::Metadata::parse(self.document.vmf());
        let (entries, error) = match metadata {
            Ok(metadata) => (metadata.resolve(&self.document).into_iter().map(|entry| {
                json!({"solid":entry.source.solid,"side":entry.source.side,"edges":entry.source.edges,
                    "piece_membership":metadata.entries.get(&(entry.source.solid, entry.source.side)).and_then(|intent|intent.piece_membership.as_ref()),
                    "source_corner_count":metadata.entries.get(&(entry.source.solid, entry.source.side)).map(|intent|intent.corners.len()),
                    "top_materials":metadata.entries.get(&(entry.source.solid, entry.source.side)).map(|intent|intent.top_materials.each_ref().map(|value|value.as_ref().map(|name|name.to_ascii_lowercase()))),
                    "top_textures":metadata.entries.get(&(entry.source.solid, entry.source.side)).map(|intent|intent.top_textures.map(|value|value.map(textures::texture_json))),
                    "effective_materials":metadata.entries.get(&(entry.source.solid, entry.source.side)).map(|intent| std::array::from_fn::<_, 2, _>(|index| intent.top_materials[index].as_deref().or_else(||subobject::solid(&self.document, entry.source.solid).ok()?.1.sides.iter().find(|side|side.id==entry.source.side).map(|side|side.material.as_str())).map(str::to_ascii_lowercase))),
                    "valid":entry.result.is_ok(),"edge_parameters":entry.result.as_ref().ok().map(|part|part.cut().unwrap().edge_parameters),"error":entry.result.err()})
            }).collect::<Vec<_>>(), None),
            Err(error) => (Vec::new(), Some(error)),
        };
        json!({"snapshot":self.component_stamp(),"entries":entries,"error":error,
            "limits":{"partitions":map_core::partition_metadata::MAX_PARTITIONS,"text_bytes":map_core::partition_metadata::MAX_TEXT_BYTES}})
    }

    pub fn store_component_partition(&mut self, actor: Actor, token: u64) -> Result<usize, String> {
        if self.checked_component_preview(token)?.extrusion.is_some() {
            return Err("Return to flat before keeping pieces, or Apply the extrusion".into());
        }
        let draft = self.checked_partition(token)?;
        if draft.rise != 0.0 {
            return Err(
                "Return to flat before keeping a partition; Apply commits raised geometry".into(),
            );
        }
        let draft = draft.clone();
        let selection = self.geometry.components.selection.clone();
        let picking = self.geometry.components.picking.clone();
        let count = self.edit(actor, "keep flat face partitions", |tx| {
            draft.store(tx)?;
            Ok(draft.partitions.len())
        })?;
        if self.partition_preview_token() == Some(token) {
            self.cancel_component_move(token)?;
        }
        self.geometry.components.selection = selection;
        self.geometry.components.picking = picking;
        self.refresh_component_origin();
        Ok(count)
    }

    pub fn resume_component_partitions(
        &mut self,
        sources: &[(u32, u32)],
        stamp: &str,
    ) -> Result<u64, String> {
        self.check_component_stamp(stamp)?;
        if sources.len() > subobject::MAX_SOLIDS {
            return Err("Resume at most 16 saved partitions".into());
        }
        if self.geometry.components.preview.is_some() {
            return Err(
                "Apply or discard the current component preview before resuming saved pieces"
                    .into(),
            );
        }
        let sources = sources.iter().copied().collect::<BTreeSet<_>>();
        let solids = sources
            .iter()
            .map(|source| source.0)
            .collect::<BTreeSet<_>>();
        if sources.is_empty()
            || sources.len() > subobject::MAX_SOLIDS
            || sources.len() != solids.len()
        {
            return Err("Resume one saved partition per brush across 1–16 brushes".into());
        }
        let metadata = map_core::partition_metadata::Metadata::parse(self.document.vmf())?;
        let mut resolved = metadata
            .resolve(&self.document)
            .into_iter()
            .map(|entry| ((entry.source.solid, entry.source.side), entry.result))
            .collect::<BTreeMap<_, _>>();
        let mut partitions = Vec::new();
        for source in &sources {
            partitions.push(
                resolved
                    .remove(source)
                    .ok_or("Saved partition does not exist")??,
            );
        }
        let materials = materials::saved_materials(&metadata, &partitions);
        let textures = textures::saved_textures(&self.document, &metadata, &partitions);
        let ids = solids.into_iter().collect::<Vec<_>>();
        self.set_component_picking(
            Some(super::super::component_picking::Kind::Face),
            &ids,
            stamp,
        )?;
        let selection = sources
            .into_iter()
            .map(|(solid, side)| Component {
                solid,
                sides: vec![side],
            })
            .collect::<Vec<_>>();
        self.select_components(&selection, SelectMode::Replace, &self.component_stamp())?;
        let selection = partitions
            .iter()
            .flat_map(|part| {
                part.loops.iter().map(|piece| PartitionRef {
                    solid: part.source.solid,
                    partition: piece.id,
                })
            })
            .collect();
        Ok(self.install_partition_preview(
            Draft {
                partitions,
                rise: 0.0,
                roof_texture_lock: false,
                selection,
                materials,
                textures,
            },
            self.document.preview_snapshot(),
            None,
        ))
    }

    pub fn remove_saved_component_partitions(
        &mut self,
        actor: Actor,
        sources: &[(u32, u32)],
        stamp: &str,
    ) -> Result<usize, String> {
        self.check_component_stamp(stamp)?;
        if sources.len() > map_core::partition_metadata::MAX_PARTITIONS {
            return Err("Remove at most 64 saved partitions".into());
        }
        if self.geometry.components.preview.is_some() {
            return Err(
                "Apply or discard the current component preview before removing saved pieces"
                    .into(),
            );
        }
        let sources = sources.iter().copied().collect::<BTreeSet<_>>();
        if sources.is_empty() || sources.len() > map_core::partition_metadata::MAX_PARTITIONS {
            return Err("Choose 1–64 saved partitions to remove".into());
        }
        let metadata = map_core::partition_metadata::Metadata::parse(self.document.vmf())?;
        if sources
            .iter()
            .any(|source| !metadata.entries.contains_key(source))
        {
            return Err("Saved partition does not exist".into());
        }
        self.edit(actor, "remove saved face partitions", |tx| {
            for (solid, side) in &sources {
                map_core::partition_metadata::remove(tx, *solid, *side)?;
            }
            Ok(sources.len())
        })
    }

    pub fn partition_preview_token(&self) -> Option<u64> {
        self.geometry
            .components
            .preview
            .as_ref()
            .filter(|preview| preview.partition.is_some())
            .map(|preview| preview.token)
    }

    pub fn ensure_partitions_resolved(&self) -> Result<(), String> {
        if self.partition_preview_token().is_some() {
            return Err("A temporary face partition is pending. Keep flat pieces, raise and Apply, or Cancel before saving, compiling or playing.".into());
        }
        Ok(())
    }

    pub(super) fn checked_partition(&self, token: u64) -> Result<&Draft, String> {
        self.checked_component_preview(token)?
            .partition
            .as_ref()
            .ok_or("No flat partition draft to edit".into())
    }

    fn install_partition_preview(
        &mut self,
        draft: Draft,
        document: Document,
        plan: Option<subobject::CreasePlan>,
    ) -> u64 {
        let requests = draft.requests();
        let selection = plan.as_ref().map_or_else(
            || self.geometry.components.selection.clone(),
            |plan| {
                plan.changes
                    .iter()
                    .flat_map(|change| change.selection.iter().cloned())
                    .collect()
            },
        );
        let (replacements, crease) = match plan {
            Some(plan) => (
                plan.replacements,
                Some(CreaseDraft {
                    rise: draft.rise,
                    roof_texture_lock: draft.roof_texture_lock,
                    requests,
                    changes: plan.changes,
                }),
            ),
            None => (Vec::new(), None),
        };
        self.install_component_preview(Preview {
            replacements,
            crease,
            partition: Some(draft),
            ..Preview::new(document, selection)
        })
    }

    pub fn preview_component_partition(
        &mut self,
        pairs: &[CreaseEdges],
        stamp: &str,
    ) -> Result<u64, String> {
        self.check_component_stamp(stamp)?;
        let requests = self.crease_requests(pairs)?;
        let mut partitions = subobject::plan_partitions(&self.document, &requests)
            .map_err(|error| error.to_string())?;
        let materials = if let Ok(metadata) =
            map_core::partition_metadata::Metadata::parse(self.document.vmf())
        {
            let mut materials = BTreeMap::new();
            for resolved in metadata.resolve(&self.document) {
                if let Ok(saved) = resolved.result
                    && let Some(current) = partitions
                        .iter_mut()
                        .find(|part| part.source == saved.source)
                {
                    materials.extend(materials::saved_materials(
                        &metadata,
                        std::slice::from_ref(&saved),
                    ));
                    current.loops = saved.loops;
                }
            }
            materials
        } else {
            BTreeMap::new()
        };
        let textures = map_core::partition_metadata::Metadata::parse(self.document.vmf())
            .map(|metadata| textures::saved_textures(&self.document, &metadata, &partitions))
            .unwrap_or_default();
        let selection = partitions
            .iter()
            .flat_map(|part| {
                part.loops.iter().map(|piece| PartitionRef {
                    solid: part.source.solid,
                    partition: piece.id,
                })
            })
            .collect();
        let draft = Draft {
            partitions,
            rise: 0.0,
            roof_texture_lock: false,
            selection,
            materials,
            textures,
        };
        Ok(self.install_partition_preview(draft, self.document.preview_snapshot(), None))
    }

    pub fn update_component_partition_cut(
        &mut self,
        token: u64,
        edge_parameters: [f64; 2],
    ) -> Result<u64, String> {
        let mut draft = self.checked_partition(token)?.clone();
        if draft.rise != 0.0 || self.checked_component_preview(token)?.extrusion.is_some() {
            return Err("Return to flat before adjusting the dividing edge".into());
        }
        let requests = draft
            .partitions
            .iter()
            .map(|part| subobject::PartitionCut {
                source: part.source.clone(),
                edge_parameters,
            })
            .collect::<Vec<_>>();
        let mut partitions = subobject::plan_partitions_at(&self.document, &requests)
            .map_err(|error| error.to_string())?;
        for (part, previous) in partitions.iter_mut().zip(&draft.partitions) {
            part.loops = previous.loops.clone();
        }
        draft.partitions = partitions;
        Ok(self.install_partition_preview(draft, self.document.preview_snapshot(), None))
    }

    pub fn raise_component_partition(&mut self, token: u64, rise: f64) -> Result<u64, String> {
        self.raise_component_partition_with_texture(token, rise, None)
    }

    pub fn raise_component_partition_with_texture(
        &mut self,
        token: u64,
        rise: f64,
        roof_texture_lock: Option<bool>,
    ) -> Result<u64, String> {
        let mut draft = self.checked_partition(token)?.clone();
        draft.roof_texture_lock = roof_texture_lock.unwrap_or(draft.roof_texture_lock);
        if !rise.is_finite()
            || (rise != 0.0
                && !(subobject::MIN_SPLIT_RISE..=subobject::MAX_SPLIT_RISE).contains(&rise))
        {
            return Err("Partition ridge height must be zero (flat) or 0.125–4096 units".into());
        }
        let requests = draft.requests();
        let cuts = draft
            .partitions
            .iter()
            .map(|part| part.cut())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        let current = subobject::plan_partitions_at(&self.document, &cuts)
            .map_err(|error| error.to_string())?;
        if current
            .iter()
            .zip(&draft.partitions)
            .any(|(current, saved)| {
                let mut canonical = saved.clone();
                canonical.loops = current.loops.clone();
                current != &canonical
            })
        {
            return Err(
                "Partition source geometry or ownership changed; stage the face again".into(),
            );
        }
        let mut document = self.document.preview_snapshot();
        let plan = if rise == 0.0 {
            None
        } else {
            Some(
                document
                    .edit("preview raised partition", |tx| {
                        let plan = subobject::crease_partitions_with_surfaces(
                            tx,
                            &draft.partitions,
                            rise,
                            draft.roof_texture_lock,
                            &draft.material_plan(),
                            &draft.texture_plan(),
                        )?;
                        consume_saved(tx, &requests)?;
                        Ok(plan)
                    })
                    .map_err(|error| error.to_string())?,
            )
        };
        draft.rise = rise;
        Ok(self.install_partition_preview(draft, document, plan))
    }

    pub fn select_component_partitions(
        &mut self,
        token: u64,
        parts: &[PartitionRef],
        mode: SelectMode,
    ) -> Result<(), String> {
        let draft = self.checked_partition(token)?;
        if parts.len() > subobject::MAX_SOLIDS * 2
            || parts.iter().any(|part| {
                !draft.partitions.iter().any(|source| {
                    source.source.solid == part.solid
                        && source.loops.iter().any(|piece| piece.id == part.partition)
                })
            })
        {
            return Err("Choose loops from the current partition draft".into());
        }
        let mut selection = if mode == SelectMode::Replace {
            BTreeSet::new()
        } else {
            draft.selection.clone()
        };
        for part in parts.iter().collect::<BTreeSet<_>>() {
            match mode {
                SelectMode::Replace | SelectMode::Add => {
                    selection.insert(part.clone());
                }
                SelectMode::Remove => {
                    selection.remove(part);
                }
                SelectMode::Toggle => {
                    if !selection.remove(part) {
                        selection.insert(part.clone());
                    }
                }
            }
        }
        self.geometry
            .components
            .preview
            .as_mut()
            .unwrap()
            .partition
            .as_mut()
            .unwrap()
            .selection = selection;
        self.scene_serial += 1;
        self.view_serial += 1;
        Ok(())
    }

    pub fn frame_component_partitions(
        &mut self,
        actor: Actor,
        token: u64,
        aspect: f32,
    ) -> Result<(), String> {
        if !aspect.is_finite() || aspect <= 0.0 {
            return Err("Viewport aspect must be positive and finite".into());
        }
        let draft = self.checked_partition(token)?;
        let mut bounds: Option<(DVec3, DVec3)> = None;
        for part in &draft.partitions {
            let positions = part.positions(draft.rise);
            for piece in &part.loops {
                if draft.selection.contains(&PartitionRef {
                    solid: part.source.solid,
                    partition: piece.id,
                }) {
                    for &index in &piece.vertices {
                        let point = positions[index as usize];
                        bounds = Some(bounds.map_or((point, point), |(mins, maxs)| {
                            (mins.min(point), maxs.max(point))
                        }));
                        if let Some(extrusion) = &self.checked_component_preview(token)?.extrusion
                            && extrusion.selection.contains(&PartitionRef {
                                solid: part.source.solid,
                                partition: piece.id,
                            })
                        {
                            let top = point + part.normal() * extrusion.spec.height;
                            bounds = bounds.map(|(mins, maxs)| (mins.min(top), maxs.max(top)));
                        }
                    }
                }
            }
        }
        let (mins, maxs) = bounds.ok_or("Select a partition loop to frame")?;
        self.set_camera(
            self.camera
                .frame_bounds(mins.as_vec3(), maxs.as_vec3(), aspect.max(0.01)),
        );
        self.log(actor, "framed face partitions");
        Ok(())
    }

    pub(super) fn append_partition_guides(&mut self, mesh: &mut render::SceneMesh) {
        let Some(draft) = self.partition_surface_draft() else {
            return;
        };
        let visible = self.partition_piece_visibility(&draft);
        for part in &draft.partitions {
            let positions = part.positions(draft.rise);
            let mut pieces = part.loops.iter().collect::<Vec<_>>();
            pieces.sort_by_key(|piece| {
                draft.selection.contains(&PartitionRef {
                    solid: part.source.solid,
                    partition: piece.id,
                })
            });
            for piece in pieces {
                if !visible
                    .get(&PartitionRef {
                        solid: part.source.solid,
                        partition: piece.id,
                    })
                    .copied()
                    .unwrap_or(false)
                {
                    continue;
                }
                let selected = draft.selection.contains(&PartitionRef {
                    solid: part.source.solid,
                    partition: piece.id,
                });
                let color = if selected {
                    [255, 190, 65, 255]
                } else {
                    [95, 215, 255, 255]
                };
                for index in 0..piece.vertices.len() {
                    for vertex in [
                        piece.vertices[index],
                        piece.vertices[(index + 1) % piece.vertices.len()],
                    ] {
                        mesh.lines.push(render::scene::LineVertex {
                            position: (positions[vertex as usize] + part.normal() * 0.02)
                                .as_vec3()
                                .to_array(),
                            color,
                        });
                    }
                    if let Some(extrusion) = self
                        .geometry
                        .components
                        .preview
                        .as_ref()
                        .and_then(|preview| preview.extrusion.as_ref())
                        && extrusion.selection.contains(&PartitionRef {
                            solid: part.source.solid,
                            partition: piece.id,
                        })
                    {
                        let base = positions[piece.vertices[index] as usize];
                        let next =
                            positions[piece.vertices[(index + 1) % piece.vertices.len()] as usize];
                        let delta = part.normal() * extrusion.spec.height;
                        for point in [base + delta, next + delta, base, base + delta] {
                            mesh.lines.push(render::scene::LineVertex {
                                position: point.as_vec3().to_array(),
                                color,
                            });
                        }
                    }
                }
            }
        }
    }
}
