use super::*;
use map_core::subobject::{FreePartitionCut, FreePartitionPlan, FreePieceSurface};

#[derive(Debug, Clone, serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FaceCut {
    pub solid: u32,
    pub side: u32,
    pub vertices: Vec<[f64; 3]>,
    pub edges: Vec<[u32; 2]>,
}

impl FaceCut {
    fn request(&self) -> FreePartitionCut {
        FreePartitionCut {
            solid: self.solid,
            side: self.side,
            corners: Vec::new(),
            vertices: self
                .vertices
                .iter()
                .copied()
                .map(DVec3::from_array)
                .collect(),
            edges: self.edges.clone(),
        }
    }
}

pub(in crate::session::components) struct Preview {
    plan: FreePartitionPlan,
    definitions: u64,
    material_generation: u64,
    material_sources: compiler::asset_sources::AssetSourceGuard,
    cubemap_bindings: ridge_bindings::Plan,
}

fn exterior_mapping(plan: &FreePartitionPlan) -> BTreeMap<u32, Vec<u32>> {
    let mut mapping = BTreeMap::<u32, Vec<u32>>::new();
    for fragment in &plan.fragments {
        for face in &fragment.faces {
            if face.exterior {
                let sides = mapping.entry(face.source_side).or_default();
                if !sides.contains(&face.side) {
                    sides.push(face.side);
                }
            }
        }
    }
    mapping
}

pub(super) fn saved_surfaces(
    metadata: &map_core::free_partition_metadata::Metadata,
    parts: &[subobject::FreePartition],
) -> (
    BTreeMap<PartitionRef, String>,
    BTreeMap<PartitionRef, subobject::PartitionTexture>,
) {
    let mut materials = BTreeMap::new();
    let mut textures = BTreeMap::new();
    for part in parts {
        if let Some(intent) = metadata.entries.get(&(part.solid, part.side)) {
            for surface in &intent.surfaces {
                let target = PartitionRef {
                    solid: part.solid,
                    partition: surface.piece,
                };
                if let Some(material) = &surface.material {
                    materials.insert(target.clone(), material.clone());
                }
                if let Some(texture) = surface.texture {
                    textures.insert(target, texture);
                }
            }
        }
    }
    (materials, textures)
}

impl Draft {
    pub(super) fn free_surfaces(&self) -> Vec<FreePieceSurface> {
        let keys = self
            .materials
            .keys()
            .chain(self.textures.keys())
            .collect::<BTreeSet<_>>();
        keys.into_iter()
            .map(|target| FreePieceSurface {
                solid: target.solid,
                piece: target.partition,
                material: self.materials.get(target).cloned(),
                texture: self.textures.get(target).copied(),
            })
            .collect()
    }

    pub(super) fn free_report(&self, document: &Document, preview: Option<&Preview>) -> Value {
        let output_brushes = preview.map_or(0, |preview| preview.plan.fragments.len());
        json!({"kind":"free_edges","ridge_supported":false,"edge_parameters":[null,null],"rise":0.0,"roof_texture_lock":false,"neighbor_texture_lock":false,
            "output_brushes":output_brushes,"added_brushes":output_brushes.saturating_sub(self.free_partitions.len()),"fragment_count":output_brushes,
            "can_pick_pieces":true,"picking_error":null,"temporary":true,
            "cubemap_bindings":preview.map(|preview|preview.cubemap_bindings.report()),
            "selection":self.selection.iter().map(|piece|json!({"solid":piece.solid,"partition":piece.partition})).collect::<Vec<_>>(),
            "partitions":self.free_partitions.iter().map(|part| {
                let owner = match part.owner { ObjectId::Solid(id)=>format!("solid:{id}"),ObjectId::Entity(id)=>format!("entity:{id}") };
                json!({"solid":part.solid,"source_side":part.side,"source_corner_count":part.corners.len(),"requested_endpoints":part.cut.vertices.iter().map(|point|point.to_array()).collect::<Vec<_>>(),"endpoint_vertices":part.endpoint_vertices,"max_endpoint_correction":part.max_endpoint_correction,"numeric_tolerance":part.numeric_tolerance,"owner":owner,"plane":part.plane.map(|point|point.to_array()),
                    "edges":part.edges.iter().map(|edge|json!({"vertices":edge.vertices,"authored":edge.authored,"boundary":edge.boundary})).collect::<Vec<_>>(),
                    "vertices":part.vertices.iter().map(|vertex|json!({"id":vertex.id,"position":vertex.position.to_array(),"preview_position":vertex.position.to_array(),"source":{"kind":"face","coordinates":vertex.coordinates}})).collect::<Vec<_>>(),
                    "loops":part.pieces.iter().map(|piece| {
                        let target=PartitionRef{solid:part.solid,partition:piece.id};
                        json!({"id":piece.id,"vertices":piece.vertices,"shape":"triangle","source_side":part.side,
                            "top_material":self.materials.get(&target).map(|name|name.to_ascii_lowercase()),"effective_material":self.effective_piece_material(document,part.solid,part.side,piece.id).map(str::to_ascii_lowercase),"inherited":!self.materials.contains_key(&target),
                            "texture_plan":self.textures.get(&target).copied().map(textures::texture_json),"effective_texture":self.effective_piece_texture_json(document,part.solid,part.side,piece.id),"texture_inherited":!self.textures.contains_key(&target),"selected":self.selection.contains(&target)})
                    }).collect::<Vec<_>>()})
            }).collect::<Vec<_>>(),
            "fragments":preview.map(|preview|preview.plan.fragments.iter().map(|fragment|json!({"source_solid":fragment.source,"solid":fragment.solid.id,
                "owner":match fragment.owner {ObjectId::Solid(_)=>"world".to_string(),ObjectId::Entity(id)=>format!("entity:{id}")},
                "bounds":map_core::query::solid_bounds(&fragment.solid).map(|(mins,maxs)|json!({"mins":mins.to_array(),"maxs":maxs.to_array()})),
                "faces":fragment.faces.iter().map(|face| {
                    let side=fragment.solid.sides.iter().find(|side|side.id==face.side).unwrap();
                    json!({"side":face.side,"source_side":face.source_side,"exterior":face.exterior,"partition":face.piece,
                        "material":side.material.to_ascii_lowercase(),"plane":side.plane.0.map(|point|point.to_array()),
                        "uaxis":{"axis":side.u_axis.axis.to_array(),"offset":side.u_axis.offset,"scale":side.u_axis.scale},
                        "vaxis":{"axis":side.v_axis.axis.to_array(),"offset":side.v_axis.offset,"scale":side.v_axis.scale},
                        "rotation":side.rotation,"lightmap_scale":side.lightmap_scale,"smoothing_groups":side.smoothing_groups})
                }).collect::<Vec<_>>()
            })).collect::<Vec<_>>())})
    }
}

impl Session {
    pub fn preview_component_face_cuts(
        &mut self,
        cuts: &[FaceCut],
        stamp: &str,
    ) -> Result<u64, String> {
        self.check_component_stamp(stamp)?;
        let sources = cuts
            .iter()
            .map(|cut| Component {
                solid: cut.solid,
                sides: vec![cut.side],
            })
            .collect::<BTreeSet<_>>();
        if sources != self.geometry.components.selection || sources.len() != cuts.len() {
            return Err(
                "Supply one face-cut graph for every selected face, with one face per brush".into(),
            );
        }
        let requests = cuts.iter().map(FaceCut::request).collect::<Vec<_>>();
        let parts = subobject::plan_free_partitions(&self.document, &requests)
            .map_err(|error| error.to_string())?;
        let selection = parts
            .iter()
            .flat_map(|part| {
                part.pieces.iter().map(|piece| PartitionRef {
                    solid: part.solid,
                    partition: piece.id,
                })
            })
            .collect();
        self.install_free_partition_preview(Draft {
            partitions: Vec::new(),
            free_partitions: parts,
            rise: 0.0,
            roof_texture_lock: false,
            neighbor_texture_lock: false,
            selection,
            materials: BTreeMap::new(),
            textures: BTreeMap::new(),
        })
    }

    pub fn append_component_face_cut(
        &mut self,
        token: u64,
        solid: u32,
        side: u32,
        segment: [[f64; 3]; 2],
    ) -> Result<u64, String> {
        let mut draft = self.checked_partition(token)?.clone();
        if draft
            .materials
            .keys()
            .chain(draft.textures.keys())
            .any(|target| target.solid == solid)
        {
            return Err(
                "Use source material and source placement before changing this cut mesh".into(),
            );
        }
        let part = draft
            .free_partitions
            .iter()
            .find(|part| part.solid == solid && part.side == side)
            .ok_or("Choose a source face from the free-edge draft")?;
        let mut request = part.cut();
        let first = request.vertices.len() as u32;
        request.vertices.extend(segment.map(DVec3::from_array));
        request.edges.push([first, first + 1]);
        let mut parts = subobject::plan_free_partitions(&self.document, &[request])
            .map_err(|error| error.to_string())?;
        let part = parts
            .pop()
            .ok_or("Face cut did not produce a source partition")?;
        draft.selection.retain(|target| target.solid != solid);
        draft
            .selection
            .extend(part.pieces.iter().map(|piece| PartitionRef {
                solid,
                partition: piece.id,
            }));
        *draft
            .free_partitions
            .iter_mut()
            .find(|part| part.solid == solid)
            .unwrap() = part;
        self.install_free_partition_preview(draft)
    }

    pub(in crate::session::components) fn install_free_partition_preview(
        &mut self,
        draft: Draft,
    ) -> Result<u64, String> {
        let preview = self.prepare_free_partition_preview(draft)?;
        Ok(self.install_component_preview(preview))
    }

    fn prepare_free_partition_preview(
        &self,
        draft: Draft,
    ) -> Result<super::super::Preview, String> {
        self.document
            .preview_snapshot()
            .edit("check saved free face cuts", |tx| draft.store(tx))
            .map_err(|error| error.to_string())?;
        let sources = draft
            .free_partitions
            .iter()
            .map(|part| part.solid)
            .collect::<BTreeSet<_>>();
        let consumed = draft
            .free_partitions
            .iter()
            .map(|part| (part.solid, part.side))
            .collect();
        self.check_fragment_references(&sources, &BTreeSet::new(), &consumed)?;
        let surfaces = draft.free_surfaces();
        let mut candidate = self.document.preview_snapshot();
        let plan = candidate
            .edit("preview free face cuts", |tx| {
                subobject::materialize_free_partitions(tx, &draft.free_partitions, &surfaces)
            })
            .map_err(|error| error.to_string())?;
        let material_sources = self.capture_fragment_materials(
            &sources.into_iter().collect::<Vec<_>>(),
            plan.fragments.iter().flat_map(|fragment| {
                fragment
                    .solid
                    .sides
                    .iter()
                    .map(|side| side.material.as_str())
            }),
        )?;
        let cubemap_bindings =
            self.prepare_fragment_cubemap_bindings(&mut candidate, &exterior_mapping(&plan))?;
        let free_partition = Preview {
            plan,
            definitions: self.definitions_serial,
            material_generation: self.materials.generation(),
            material_sources,
            cubemap_bindings,
        };
        Ok(super::super::Preview {
            partition: Some(draft),
            free_partition: Some(free_partition),
            ..super::super::Preview::new(
                self.document.preview_snapshot(),
                self.geometry.components.selection.clone(),
            )
        })
    }

    pub(super) fn resume_free_component_partitions(
        &mut self,
        sources: &BTreeSet<(u32, u32)>,
        metadata: &map_core::free_partition_metadata::Metadata,
        stamp: &str,
    ) -> Result<u64, String> {
        if sources
            .iter()
            .any(|source| !metadata.entries.contains_key(source))
        {
            return Err("Resume boundary partitions and free face cuts separately".into());
        }
        let mut resolved = metadata
            .resolve(&self.document)
            .into_iter()
            .map(|entry| ((entry.solid, entry.side), entry.result))
            .collect::<BTreeMap<_, _>>();
        let parts = sources
            .iter()
            .map(|source| {
                resolved
                    .remove(source)
                    .ok_or("Saved face cuts do not exist")?
            })
            .collect::<Result<Vec<_>, _>>()?;
        let (materials, textures) = saved_surfaces(metadata, &parts);
        let selection = parts
            .iter()
            .flat_map(|part| {
                part.pieces.iter().map(|piece| PartitionRef {
                    solid: part.solid,
                    partition: piece.id,
                })
            })
            .collect();
        let draft = Draft {
            partitions: Vec::new(),
            free_partitions: parts,
            rise: 0.0,
            roof_texture_lock: false,
            neighbor_texture_lock: false,
            selection,
            materials,
            textures,
        };
        let mut preview = self.prepare_free_partition_preview(draft)?;
        let ids = sources.iter().map(|source| source.0).collect::<Vec<_>>();
        self.set_component_picking(
            Some(super::super::super::component_picking::Kind::Face),
            &ids,
            stamp,
        )?;
        let selection = sources
            .iter()
            .map(|&(solid, side)| Component {
                solid,
                sides: vec![side],
            })
            .collect::<Vec<_>>();
        self.select_components(&selection, SelectMode::Replace, &self.component_stamp())?;
        preview.selection = self.geometry.components.selection.clone();
        Ok(self.install_component_preview(preview))
    }

    pub(super) fn free_saved_entries(&self) -> Result<Vec<Value>, String> {
        let metadata = map_core::free_partition_metadata::Metadata::parse(self.document.vmf())?;
        Ok(metadata.resolve(&self.document).into_iter().map(|entry| {
            let intent = metadata.entries.get(&(entry.solid,entry.side)).unwrap();
            json!({"kind":"free_edges","solid":entry.solid,"side":entry.side,"source_corner_count":intent.corners.len(),"corners":intent.corners,"coordinate_space":"source_corner_barycentric","vertices":intent.vertices,"world_vertices":entry.result.as_ref().ok().map(|part|part.cut.vertices.iter().map(|point|point.to_array()).collect::<Vec<_>>()),"endpoint_corners":intent.endpoint_corners,"endpoint_vertices":entry.result.as_ref().ok().map(|part|&part.endpoint_vertices),"mesh_vertices":entry.result.as_ref().ok().map(|part|part.vertices.iter().map(|vertex|json!({"id":vertex.id,"position":vertex.position.to_array(),"coordinates":vertex.coordinates})).collect::<Vec<_>>()),"edges":intent.edges,
                "pieces":entry.result.as_ref().ok().map(|part|part.pieces.iter().map(|piece|json!({"id":piece.id,"vertices":piece.vertices})).collect::<Vec<_>>()),
                "surfaces":intent.surfaces.iter().map(|surface|json!({"partition":surface.piece,"material":surface.material,"texture":surface.texture.map(textures::texture_json)})).collect::<Vec<_>>(),
                "valid":entry.result.is_ok(),"error":entry.result.err()})
        }).collect())
    }

    pub(in crate::session::components) fn free_partition_stale_error(
        &self,
        preview: &Preview,
    ) -> Option<String> {
        if let Some(error) = preview.cubemap_bindings.stale_error(self) {
            return Some(error);
        }
        if preview.definitions != self.definitions_serial {
            return Some("Definitions changed; preview the face cuts again".into());
        }
        if preview.material_generation != self.materials.generation() {
            return Some("Material assets were reloaded; preview the face cuts again".into());
        }
        preview
            .material_sources
            .verify_metadata()
            .err()
            .map(|error| error.to_string())
    }

    pub(in crate::session::components) fn commit_component_face_cuts(
        &mut self,
        actor: Actor,
        token: u64,
    ) -> Result<usize, String> {
        let preview = self.checked_component_preview(token)?;
        let free = preview
            .free_partition
            .as_ref()
            .ok_or("Preview the face cuts before Apply")?;
        if let Some(error) = self.free_partition_stale_error(free) {
            return Err(error);
        }
        let draft = preview.partition.as_ref().unwrap().clone();
        let sources = draft
            .free_partitions
            .iter()
            .map(|part| part.solid)
            .collect();
        let consumed = draft
            .free_partitions
            .iter()
            .map(|part| (part.solid, part.side))
            .collect();
        self.check_fragment_references(&sources, &BTreeSet::new(), &consumed)?;
        let expected = free.plan.clone();
        let mapping = exterior_mapping(&expected);
        let writes = free.cubemap_bindings.writes.clone();
        let cubemap_sources = free.cubemap_bindings.sources.clone();
        let materials = free.material_sources.clone();
        let game = self.game.clone();
        let faces = expected
            .fragments
            .iter()
            .flat_map(|fragment| {
                fragment
                    .faces
                    .iter()
                    .filter(|face| face.piece.is_some())
                    .map(|face| map_core::surface::FaceId {
                        solid: fragment.solid.id,
                        side: face.side,
                    })
            })
            .collect::<Vec<_>>();
        let count = self.edit(actor, "apply free face cuts", |tx| {
            if ridge_bindings::writes_for_mapping(tx.vmf(), &mapping).map_err(EditError::Invalid)?
                != writes
            {
                return Err(EditError::Invalid(
                    "Cubemap bindings changed; preview the face cuts again".into(),
                ));
            }
            let actual = subobject::materialize_free_partitions(
                tx,
                &draft.free_partitions,
                &draft.free_surfaces(),
            )?;
            if actual != expected {
                return Err(EditError::Invalid(
                    "Face-cut geometry or allocation changed; preview the face cuts again".into(),
                ));
            }
            ridge_bindings::apply(tx, &writes)?;
            materials
                .verify_sources(&game, &compiler::Control::default())
                .map_err(|error| EditError::Invalid(error.to_string()))?;
            for sources in &cubemap_sources {
                sources
                    .verify(&game, &compiler::Control::default())
                    .map_err(EditError::Invalid)?;
            }
            Ok(actual.fragments.len())
        })?;
        self.select_faces(&faces, SelectMode::Replace)?;
        Ok(count)
    }
}
