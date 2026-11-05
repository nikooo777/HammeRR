use map_core::subobject::{Partition, VertexSource};

use super::*;

mod free;
mod materials;
mod picking;
mod textures;

pub use free::FaceCut;
pub(super) use free::Preview as FreePreview;

pub(super) fn consume_saved(
    tx: &mut map_core::Transaction,
    requests: &[Crease],
) -> Result<(), EditError> {
    let mut metadata =
        map_core::partition_metadata::Metadata::parse(tx.vmf()).map_err(EditError::Invalid)?;
    let count = metadata.entries.len();
    for request in requests {
        metadata.entries.remove(&(request.solid, request.side));
    }
    if metadata.entries.len() != count {
        metadata.write(tx)?;
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
    pub free_partitions: Vec<subobject::FreePartition>,
    pub rise: f64,
    pub roof_texture_lock: bool,
    pub neighbor_texture_lock: bool,
    pub selection: BTreeSet<PartitionRef>,
    pub materials: BTreeMap<PartitionRef, String>,
    pub textures: BTreeMap<PartitionRef, subobject::PartitionTexture>,
}

pub(super) struct Piece {
    pub solid: u32,
    pub side: u32,
    pub id: u32,
    pub points: Vec<DVec3>,
    pub normal: DVec3,
}

impl Draft {
    pub(super) fn is_free(&self) -> bool {
        !self.free_partitions.is_empty()
    }

    pub(super) fn source_count(&self) -> usize {
        self.partitions.len() + self.free_partitions.len()
    }

    pub(super) fn pieces(&self) -> Vec<Piece> {
        let mut pieces = self
            .partitions
            .iter()
            .flat_map(|part| {
                let positions = part.positions(self.rise);
                part.loops.iter().map(move |piece| Piece {
                    solid: part.source.solid,
                    side: part.source.side,
                    id: piece.id,
                    normal: part.normal(),
                    points: piece
                        .vertices
                        .iter()
                        .map(|id| positions[*id as usize])
                        .collect(),
                })
            })
            .collect::<Vec<_>>();
        pieces.extend(self.free_partitions.iter().flat_map(|part| {
            part.pieces.iter().map(|piece| Piece {
                solid: part.solid,
                side: part.side,
                id: piece.id,
                normal: part.normal(),
                points: piece
                    .vertices
                    .iter()
                    .map(|id| part.vertices[*id as usize].position)
                    .collect(),
            })
        }));
        pieces
    }

    pub(super) fn contains_piece(&self, piece: &PartitionRef) -> bool {
        self.partitions.iter().any(|part| {
            part.source.solid == piece.solid
                && part.loops.iter().any(|item| item.id == piece.partition)
        }) || self.free_partitions.iter().any(|part| {
            part.solid == piece.solid && part.pieces.iter().any(|item| item.id == piece.partition)
        })
    }

    pub(super) fn source_side(&self, solid: u32) -> Option<u32> {
        self.partitions
            .iter()
            .find(|part| part.source.solid == solid)
            .map(|part| part.source.side)
            .or_else(|| {
                self.free_partitions
                    .iter()
                    .find(|part| part.solid == solid)
                    .map(|part| part.side)
            })
    }

    fn requests(&self) -> Vec<Crease> {
        self.partitions
            .iter()
            .map(|partition| partition.source.clone())
            .collect()
    }

    pub fn report(
        &self,
        document: &map_core::Document,
        crease: Option<&CreaseDraft>,
        free: Option<&FreePreview>,
    ) -> Value {
        if self.is_free() {
            return self.free_report(document, free);
        }
        let fragments = crease.map_or(0, |crease| crease.plan.fragments.len());
        let output_brushes = if fragments == 0 {
            self.partitions.len()
        } else {
            fragments
        };
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
        json!({"kind":"boundary_edges","ridge_supported":self.partitions.iter().all(|part|part.loops.iter().all(|piece|piece.vertices.len()==4)),"edge_parameters":edge_parameters,"rise":self.rise,"roof_texture_lock":self.roof_texture_lock,"neighbor_texture_lock":self.neighbor_texture_lock,"output_brushes":output_brushes,"added_brushes":output_brushes.saturating_sub(self.partitions.len()),"fragment_count":fragments,"can_pick_pieces":fragments==0,"picking_error":(fragments!=0).then_some("Return to flat before picking partition pieces after neighbor attachment"),"temporary":true,"selection":self.selection.iter().map(|part|json!({"solid":part.solid,"partition":part.partition})).collect::<Vec<_>>(),
            "partitions":self.partitions.iter().map(|part| {
                let positions = part.positions(self.rise);
                let owner = match part.owner {ObjectId::Solid(id)=>format!("solid:{id}"),ObjectId::Entity(id)=>format!("entity:{id}")};
                json!({"solid":part.source.solid,"source_side":part.source.side,"source_corner_count":part.source_corner_count(),"edges":part.source.edges,"owner":owner,"plane":part.plane.map(|point|point.to_array()),
                    "vertices":part.vertices.iter().map(|vertex|json!({"id":vertex.id,"position":vertex.position.to_array(),"preview_position":positions[vertex.id as usize].to_array(),"source":match &vertex.source {VertexSource::Corner(sides)=>json!({"kind":"corner","sides":sides}),VertexSource::Edge{corners,parameter}=>json!({"kind":"edge","corners":corners,"parameter":parameter})}})).collect::<Vec<_>>(),
                    "loops":part.loops.iter().map(|piece|json!({"id":piece.id,"vertices":piece.vertices,"shape":match piece.vertices.len(){3=>"triangle",4=>"quad",5=>"pentagon",6=>"hexagon",7=>"heptagon",8=>"octagon",9=>"nonagon",_=>unreachable!()},"source_side":piece.source_side,"top_material":self.materials.get(&PartitionRef{solid:part.source.solid,partition:piece.id}).map(|name|name.to_ascii_lowercase()),"effective_material":self.effective_material(document, part, piece.id).map(str::to_ascii_lowercase),"inherited":!self.materials.contains_key(&PartitionRef{solid:part.source.solid,partition:piece.id}),"texture_plan":self.textures.get(&PartitionRef{solid:part.source.solid,partition:piece.id}).copied().map(textures::texture_json),"effective_texture":self.effective_texture_json(document,part,piece.id),"texture_inherited":!self.textures.contains_key(&PartitionRef{solid:part.source.solid,partition:piece.id}),"selected":self.selection.contains(&PartitionRef{solid:part.source.solid,partition:piece.id})})).collect::<Vec<_>>()})
            }).collect::<Vec<_>>()})
    }
}

impl Session {
    pub fn saved_partition_report(&self) -> Value {
        let metadata = map_core::partition_metadata::Metadata::parse(self.document.vmf());
        let (mut entries, mut error) = match metadata {
            Ok(metadata) => (metadata.resolve(&self.document).into_iter().map(|entry| {
                json!({"kind":"boundary_edges","solid":entry.source.solid,"side":entry.source.side,"edges":entry.source.edges,
                    "piece_membership":metadata.entries.get(&(entry.source.solid, entry.source.side)).and_then(|intent|intent.piece_membership.as_ref()),
                    "source_corner_count":metadata.entries.get(&(entry.source.solid, entry.source.side)).map(|intent|intent.corners.len()),
                    "top_materials":metadata.entries.get(&(entry.source.solid, entry.source.side)).map(|intent|intent.top_materials.each_ref().map(|value|value.as_ref().map(|name|name.to_ascii_lowercase()))),
                    "top_textures":metadata.entries.get(&(entry.source.solid, entry.source.side)).map(|intent|intent.top_textures.map(|value|value.map(textures::texture_json))),
                    "effective_materials":metadata.entries.get(&(entry.source.solid, entry.source.side)).map(|intent| std::array::from_fn::<_, 2, _>(|index| intent.top_materials[index].as_deref().or_else(||subobject::solid(&self.document, entry.source.solid).ok()?.1.sides.iter().find(|side|side.id==entry.source.side).map(|side|side.material.as_str())).map(str::to_ascii_lowercase))),
                    "valid":entry.result.is_ok(),"edge_parameters":entry.result.as_ref().ok().map(|part|part.cut().unwrap().edge_parameters),"error":entry.result.err()})
            }).collect::<Vec<_>>(), None),
            Err(error) => (Vec::new(), Some(error)),
        };
        match self.free_saved_entries() {
            Ok(free) => entries.extend(free),
            Err(free_error) => {
                error =
                    Some(error.map_or(free_error.clone(), |error| format!("{error}; {free_error}")))
            }
        }
        entries.sort_by_key(|entry| {
            (
                entry["solid"].as_u64(),
                entry["side"].as_u64(),
                entry["kind"].as_str().map(str::to_owned),
            )
        });
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
            Ok(draft.source_count())
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
        let free_metadata =
            map_core::free_partition_metadata::Metadata::parse(self.document.vmf())?;
        if sources
            .iter()
            .any(|source| free_metadata.entries.contains_key(source))
        {
            return self.resume_free_component_partitions(&sources, &free_metadata, stamp);
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
        self.install_partition_preview(
            Draft {
                partitions,
                free_partitions: Vec::new(),
                rise: 0.0,
                roof_texture_lock: false,
                neighbor_texture_lock: false,
                selection,
                materials,
                textures,
            },
            self.document.preview_snapshot(),
            None,
        )
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
        let free_metadata =
            map_core::free_partition_metadata::Metadata::parse(self.document.vmf())?;
        if sources.iter().any(|source| {
            !metadata.entries.contains_key(source) && !free_metadata.entries.contains_key(source)
        }) {
            return Err("Saved partition does not exist".into());
        }
        self.edit(actor, "remove saved face partitions", |tx| {
            for (solid, side) in &sources {
                if metadata.entries.contains_key(&(*solid, *side)) {
                    map_core::partition_metadata::remove(tx, *solid, *side)?;
                }
            }
            let free_sources = sources
                .iter()
                .filter(|source| free_metadata.entries.contains_key(source))
                .copied()
                .collect::<Vec<_>>();
            if !free_sources.is_empty() {
                map_core::free_partition_metadata::remove(tx, &free_sources)?;
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
        mut document: Document,
        plan: Option<subobject::CreasePlan>,
    ) -> Result<u64, String> {
        if draft.is_free() {
            return self.install_free_partition_preview(draft);
        }
        let requests = draft.requests();
        let material_sources = plan
            .as_ref()
            .map(|plan| {
                self.capture_ridge_fragment_materials(&requests, plan, draft.neighbor_texture_lock)
            })
            .transpose()?
            .flatten();
        let cubemap_bindings = plan
            .as_ref()
            .map(|plan| self.prepare_ridge_cubemap_bindings(&mut document, plan))
            .transpose()?
            .unwrap_or_default();
        let selection = plan.as_ref().map_or_else(
            || self.geometry.components.selection.clone(),
            ridge::component_selection,
        );
        let (replacements, crease) = match plan {
            Some(plan) => (
                plan.replacements.clone(),
                Some(CreaseDraft {
                    rise: draft.rise,
                    roof_texture_lock: draft.roof_texture_lock,
                    neighbor_texture_lock: draft.neighbor_texture_lock,
                    definitions: self.definitions_serial,
                    material_generation: self.materials.generation(),
                    material_sources,
                    cubemap_bindings,
                    requests,
                    plan,
                }),
            ),
            None => (Vec::new(), None),
        };
        Ok(self.install_component_preview(Preview {
            replacements,
            crease,
            partition: Some(draft),
            ..Preview::new(document, selection)
        }))
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
        let metadata = map_core::partition_metadata::Metadata::parse(self.document.vmf())?;
        let mut materials = BTreeMap::new();
        for current in &mut partitions {
            if let Some(intent) = metadata
                .entries
                .get(&(current.source.solid, current.source.side))
                && intent.source == current.source
            {
                let saved = intent.resolve(&self.document)?;
                materials.extend(materials::saved_materials(
                    &metadata,
                    std::slice::from_ref(&saved),
                ));
                current.loops = saved.loops;
            }
        }
        let textures = textures::saved_textures(&self.document, &metadata, &partitions);
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
            free_partitions: Vec::new(),
            rise: 0.0,
            roof_texture_lock: false,
            neighbor_texture_lock: false,
            selection,
            materials,
            textures,
        };
        self.install_partition_preview(draft, self.document.preview_snapshot(), None)
    }

    pub fn update_component_partition_cut(
        &mut self,
        token: u64,
        edge_parameters: [f64; 2],
    ) -> Result<u64, String> {
        let mut draft = self.checked_partition(token)?.clone();
        if draft.is_free() {
            return Err("Append a face cut to edit free edges".into());
        }
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
        self.install_partition_preview(draft, self.document.preview_snapshot(), None)
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
        self.raise_component_partition_with_textures(token, rise, roof_texture_lock, None)
    }

    pub fn raise_component_partition_with_textures(
        &mut self,
        token: u64,
        rise: f64,
        roof_texture_lock: Option<bool>,
        neighbor_texture_lock: Option<bool>,
    ) -> Result<u64, String> {
        let mut draft = self.checked_partition(token)?.clone();
        if draft.is_free() {
            return Err("Apply face cuts before raising real components".into());
        }
        draft.roof_texture_lock = roof_texture_lock.unwrap_or(draft.roof_texture_lock);
        draft.neighbor_texture_lock = neighbor_texture_lock.unwrap_or(draft.neighbor_texture_lock);
        if !rise.is_finite()
            || (rise != 0.0
                && !(subobject::MIN_SPLIT_RISE..=subobject::MAX_SPLIT_RISE).contains(&rise))
        {
            return Err("Partition ridge height must be zero (flat) or 0.125–4096 units".into());
        }
        let requests = draft.requests();
        if rise != 0.0 && draft.neighbor_texture_lock {
            self.check_ridge_fragment_references(&requests, true)?;
        }
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
                        let plan = subobject::crease_partitions_with_surface_options(
                            tx,
                            &draft.partitions,
                            rise,
                            draft.roof_texture_lock,
                            draft.neighbor_texture_lock,
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
        self.install_partition_preview(draft, document, plan)
    }

    pub fn select_component_partitions(
        &mut self,
        token: u64,
        parts: &[PartitionRef],
        mode: SelectMode,
    ) -> Result<(), String> {
        let draft = self.checked_partition(token)?;
        if parts.len() > subobject::MAX_COMPONENTS
            || parts.iter().any(|part| !draft.contains_piece(part))
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
        for piece in draft.pieces() {
            if !draft.selection.contains(&PartitionRef {
                solid: piece.solid,
                partition: piece.id,
            }) {
                continue;
            }
            for point in piece.points {
                bounds = Some(bounds.map_or((point, point), |(mins, maxs)| {
                    (mins.min(point), maxs.max(point))
                }));
                if let Some(extrusion) = &self.checked_component_preview(token)?.extrusion
                    && extrusion.selection.contains(&PartitionRef {
                        solid: piece.solid,
                        partition: piece.id,
                    })
                    && let Some(part) = draft
                        .partitions
                        .iter()
                        .find(|part| part.source.solid == piece.solid)
                {
                    let top = point + part.normal() * extrusion.spec.height;
                    bounds = bounds.map(|(mins, maxs)| (mins.min(top), maxs.max(top)));
                }
            }
        }
        let (mins, maxs) = bounds.ok_or("Select a partition loop to frame")?;
        self.try_set_camera(self.camera.frame_bounds(
            mins.as_vec3(),
            maxs.as_vec3(),
            aspect.max(0.01),
        ))?;
        self.log(actor, "framed face partitions");
        Ok(())
    }

    pub(super) fn append_partition_guides(&mut self, mesh: &mut render::SceneMesh) {
        let Some(draft) = self.partition_surface_draft() else {
            return;
        };
        let visible = self.partition_piece_visibility(&draft);
        for part in &draft.free_partitions {
            for edge in &part.edges {
                let adjacent = part
                    .pieces
                    .iter()
                    .filter(|piece| {
                        edge.vertices
                            .iter()
                            .all(|vertex| piece.vertices.contains(vertex))
                            && visible.get(&PartitionRef {
                                solid: part.solid,
                                partition: piece.id,
                            }) == Some(&true)
                    })
                    .collect::<Vec<_>>();
                if adjacent.is_empty() {
                    continue;
                }
                let selected = adjacent.iter().any(|piece| {
                    draft.selection.contains(&PartitionRef {
                        solid: part.solid,
                        partition: piece.id,
                    })
                });
                let color = if selected {
                    [255, 190, 65, 255]
                } else {
                    [95, 215, 255, 255]
                };
                let [a, b] = edge
                    .vertices
                    .map(|vertex| part.vertices[vertex as usize].position + part.normal() * 0.02);
                let divisions = if edge.authored || edge.boundary { 1 } else { 6 };
                for index in (0..divisions).step_by(2) {
                    for parameter in [
                        f64::from(index) / f64::from(divisions),
                        f64::from(index + 1) / f64::from(divisions),
                    ] {
                        mesh.lines.push(render::scene::LineVertex {
                            position: a.lerp(b, parameter).as_vec3().to_array(),
                            color,
                        });
                    }
                }
            }
        }
        let mut pieces = draft.pieces();
        pieces.retain(|piece| {
            !draft
                .free_partitions
                .iter()
                .any(|part| part.solid == piece.solid && part.side == piece.side)
        });
        pieces.sort_by_key(|piece| {
            draft.selection.contains(&PartitionRef {
                solid: piece.solid,
                partition: piece.id,
            })
        });
        for piece in pieces {
            let target = PartitionRef {
                solid: piece.solid,
                partition: piece.id,
            };
            if visible.get(&target) != Some(&true) {
                continue;
            }
            let normal = piece.normal;
            let color = if draft.selection.contains(&target) {
                [255, 190, 65, 255]
            } else {
                [95, 215, 255, 255]
            };
            for index in 0..piece.points.len() {
                let base = piece.points[index];
                let next = piece.points[(index + 1) % piece.points.len()];
                for point in [base, next] {
                    mesh.lines.push(render::scene::LineVertex {
                        position: (point + normal * 0.02).as_vec3().to_array(),
                        color,
                    });
                }
                if let Some(extrusion) = self
                    .geometry
                    .components
                    .preview
                    .as_ref()
                    .and_then(|preview| preview.extrusion.as_ref())
                    && extrusion.selection.contains(&target)
                {
                    let delta = normal * extrusion.spec.height;
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
