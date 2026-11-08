use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use glam::DVec3;
use map_core::clip::{ClipOptions, TerrainBlendMode, TerrainMode};
use map_core::{Document, EditError, ObjectId, carve, subobject};
use serde_json::{Value, json};

use super::clipping::{context, materials};
use super::components::{self, ridge_bindings};
use super::{Actor, SelectMode, Session};

pub struct CarveRequest<'a> {
    pub cutters: &'a [u32],
    pub targets: &'a [u32],
    pub cut_material: &'a carve::CutMaterialPolicy,
    pub terrain_mode: TerrainMode,
    pub blend_mode: TerrainBlendMode,
    pub partition_neighbors: bool,
    pub stamp: &'a str,
}

impl CarveRequest<'_> {
    fn options<'a>(
        &self,
        materials: Option<&'a materials::Prepared>,
        context: Option<&'a context::Context>,
    ) -> ClipOptions<'a> {
        ClipOptions {
            terrain_mode: self.terrain_mode,
            blend_mode: self.blend_mode,
            partition_neighbors: self.partition_neighbors,
            textures: materials.map(|materials| &materials.textures),
            terrain_context: context.map(context::Context::vmf),
        }
    }
}

pub struct Source {
    pub solid: u32,
    pub owner: ObjectId,
    pub mins: DVec3,
    pub maxs: DVec3,
    pub material: String,
    pub terrain: bool,
    pub terrain_faces: usize,
    pub carrier_bounds: Option<[DVec3; 2]>,
}

#[derive(Clone)]
pub struct Preview {
    pub document: Document,
    pub result: carve::Result,
    pub(super) render_error: Option<String>,
    token: u64,
    targets: Vec<u32>,
    scope: Vec<u32>,
    generation: u64,
    revision: u64,
    definitions: u64,
    face_mapping: BTreeMap<u32, Vec<u32>>,
    cubemap_bindings: ridge_bindings::Plan,
    terrain_context: Option<Arc<context::Context>>,
    terrain_materials: Option<Arc<materials::Prepared>>,
}

#[derive(Default)]
pub struct State {
    pub preview: Option<Preview>,
    serial: u64,
}

pub fn result_json(result: &carve::Result) -> Value {
    let mut report = super::clipping::terrain_report(
        &result.terrain,
        result.blend_mode,
        result.partition_neighbors,
    );
    let mut brush = json!({"cutter": result.cutter, "cutters":result.cutters,"cut_material":cut_material_json(&result.cut_material),
        "terrain_mode":super::clipping::terrain_mode_name(result.terrain_mode),"side_mapping":result.side_mapping,
        "changes": result.changes.iter().map(|change| json!({"source": change.source, "fragments": change.fragments})).collect::<Vec<_>>(),
        "removed": result.removed, "unchanged": result.unchanged,
        "objects": result.objects.iter().map(|id| match id {ObjectId::Solid(id) => format!("solid:{id}"), ObjectId::Entity(id) => format!("entity:{id}")}).collect::<Vec<_>>(),
        "faces": result.faces.iter().map(|face| json!({"solid": face.solid, "side": face.side, "source_solid": face.source_solid, "source_side": face.source_side, "surface_source_solid":face.surface_source_solid,"surface_source_side":face.surface_source_side,"cutter": face.cutter,"structural":face.structural})).collect::<Vec<_>>()});
    report
        .as_object_mut()
        .unwrap()
        .extend(std::mem::take(brush.as_object_mut().unwrap()));
    report
}

pub fn cut_material_json(policy: &carve::CutMaterialPolicy) -> Value {
    match policy {
        carve::CutMaterialPolicy::Cutter => json!({"policy":"cutter"}),
        carve::CutMaterialPolicy::Target => json!({"policy":"target"}),
        carve::CutMaterialPolicy::Material(material) => {
            json!({"policy":"material","material":material})
        }
    }
}

impl Source {
    pub(super) fn inspect(document: &Document, id: u32) -> Result<Self, String> {
        let (owner, solid) = subobject::solid(document, id).map_err(|error| error.to_string())?;
        let shape = subobject::topology(solid).map_err(|error| error.to_string())?;
        let (carrier_mins, carrier_maxs) = shape
            .vertices
            .iter()
            .fold((DVec3::MAX, DVec3::MIN), |(mins, maxs), vertex| {
                (mins.min(vertex.position), maxs.max(vertex.position))
            });
        let terrain_faces = solid
            .sides
            .iter()
            .filter(|side| side.displacement.is_some())
            .count();
        let terrain = terrain_faces != 0;
        let (mins, maxs) = if terrain {
            solid
                .sides
                .iter()
                .filter(|side| side.displacement.is_some())
                .try_fold((DVec3::MAX, DVec3::MIN), |(mins, maxs), side| {
                    let (face_mins, face_maxs) = map_core::query::face_bounds(solid, side.id)
                        .ok_or("Terrain surface has invalid bounds")?;
                    Ok::<_, String>((mins.min(face_mins), maxs.max(face_maxs)))
                })?
        } else {
            (carrier_mins, carrier_maxs)
        };
        let material = solid
            .sides
            .iter()
            .find(|side| side.displacement.is_some())
            .unwrap_or(&solid.sides[0])
            .material
            .clone();
        Ok(Self {
            solid: id,
            owner,
            mins,
            maxs,
            material,
            terrain,
            terrain_faces,
            carrier_bounds: terrain.then_some([carrier_mins, carrier_maxs]),
        })
    }
}

impl Session {
    pub fn carve_sources(&self) -> Result<Vec<Source>, String> {
        let ids = if let Some(preview) = &self.geometry.carving.preview {
            preview.scope.clone()
        } else {
            self.component_solids()
        };
        let ids = ids
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if ids.len() > carve::MAX_TARGETS + carve::MAX_CUTTERS {
            return Err("Choose at most 24 brushes: up to 8 cutters and 16 targets".into());
        }
        ids.into_iter()
            .map(|id| Source::inspect(&self.document, id))
            .collect()
    }

    pub fn carve_stamp(&self) -> String {
        format!(
            "{}:{}:{}:{}:{}:{}:{}",
            self.document_generation,
            self.document.revision(),
            self.scene_serial,
            self.geometry.carving.serial,
            self.definitions_serial,
            self.build_content_epoch(),
            self.materials.generation()
        )
    }

    pub fn carve_state(&self) -> Value {
        json!({"handle":self.carve_handle(),"stamp":self.carve_stamp(),"preview": self.geometry.carving.preview.as_ref().map(|preview| {
            let guard_error = self.checked_carve(preview.token).err();
            let mut value = json!({"token":preview.token.to_string(),"generation":preview.generation,"revision":preview.revision,"targets":preview.targets,"state":if guard_error.is_some() {"stale"} else {"ready"},"guard_error":guard_error,"result":result_json(&preview.result),"cubemap_bindings":preview.cubemap_bindings.report()});
            if let Some(context) = &preview.terrain_context {
                value["terrain_context"] = context.report();
                value["terrain_context"]["guard_error"] = json!(guard_error);
            }
            if let Some(materials) = &preview.terrain_materials {
                value["terrain_materials"] = materials.report();
            }
            value
        }),
            "limits":{"cutters":carve::MAX_CUTTERS,"targets":carve::MAX_TARGETS,"fragments":carve::MAX_FRAGMENTS,"sides_per_brush":subobject::MAX_SIDES,"terrain":super::clipping::terrain_limits()},
            "cut_material_policies":{"cutter":"Default. Copy each cutter face's material, world UV projection, lighting and unknown metadata to new cap/carrier faces.","target":"Copy the unique original target face with the same outward normal as each generated face; missing or ambiguous parallel faces reject. Preserve that face's world UV projection, lighting and unknown metadata.","material":"Replace only the cutter face's material; keep its world UV projection, lighting and unknown metadata. Nodraw remains allowed."},
            "terrain_modes":{"grid":"Existing grid: preserve admitted source samples and native LOD.","flat":"Retile flat terrain: preserve admitted affine planar fields.","cells":"Retile grid cells: partition complete source rows and columns, including certified native windows.","fragments":"Retile surface triangles: partition original triangles with the chosen blend policy."},
            "blend_modes":([TerrainBlendMode::Exact,TerrainBlendMode::NativeFloat,TerrainBlendMode::NearestByte].map(|mode| {
                let (raw_limit,packed_limit) = mode.error_limits();
                json!({"mode":super::clipping::blend_mode_name(mode),"raw_limit":raw_limit,"packed_limit":packed_limit})
            })),
            "terrain_policy":"Terrain cutters reject. Each terrain target must have exactly one displaced face. Targets retain their original terrain material, UVs and editing fields. Terrain sheets have open cut borders. Representation and native/resource proof failures reject without selecting another mode.",
            "full_removal_policy":"World brushes may be removed. Empty brush entities reject; delete those owners explicitly."})
    }

    pub(super) fn carve_handle(&self) -> Value {
        Self::edit_handle(
            "carve",
            self.geometry.carving.preview.as_ref().and_then(|preview| {
                self.checked_carve(preview.token)
                    .is_ok()
                    .then_some(preview.token)
            }),
        )
    }

    pub fn carve_report(&self) -> Value {
        let mut report = self.carve_state();
        match self.carve_sources() {
            Ok(sources) => report["sources"] = json!(sources.iter().map(|source| json!({"solid":source.solid,"owner":match source.owner {ObjectId::Solid(id)=>format!("solid:{id}"),ObjectId::Entity(id)=>format!("entity:{id}")},"mins":source.mins.to_array(),"maxs":source.maxs.to_array(),"material":source.material,"terrain":source.terrain,"terrain_faces":source.terrain_faces,"carrier_bounds":source.carrier_bounds.map(|bounds|bounds.map(|point|point.to_array()))})).collect::<Vec<_>>()),
            Err(error) => report["source_error"] = json!(error),
        }
        report
    }

    pub fn clear_carve_preview(&mut self) {
        if self.geometry.carving.preview.take().is_some() {
            self.geometry.carving.serial += 1;
            self.scene_serial += 1;
            self.view_serial += 1;
        }
    }

    pub fn preview_carve(
        &mut self,
        cutter: u32,
        targets: &[u32],
        stamp: &str,
    ) -> Result<(), String> {
        self.preview_carve_many(&[cutter], targets, stamp)
    }

    pub fn preview_carve_many(
        &mut self,
        cutters: &[u32],
        targets: &[u32],
        stamp: &str,
    ) -> Result<(), String> {
        self.preview_carve_many_with_material(
            cutters,
            targets,
            &carve::CutMaterialPolicy::Cutter,
            stamp,
        )
    }

    pub fn preview_carve_with_material(
        &mut self,
        cutter: u32,
        targets: &[u32],
        cut_material: &carve::CutMaterialPolicy,
        stamp: &str,
    ) -> Result<(), String> {
        self.preview_carve_many_with_material(&[cutter], targets, cut_material, stamp)
    }

    pub fn preview_carve_many_with_material(
        &mut self,
        cutters: &[u32],
        targets: &[u32],
        cut_material: &carve::CutMaterialPolicy,
        stamp: &str,
    ) -> Result<(), String> {
        self.preview_carve_request(CarveRequest {
            cutters,
            targets,
            cut_material,
            terrain_mode: TerrainMode::Grid,
            blend_mode: TerrainBlendMode::Exact,
            partition_neighbors: false,
            stamp,
        })
    }

    pub fn preview_carve_request(&mut self, request: CarveRequest<'_>) -> Result<(), String> {
        if request.stamp != self.carve_stamp() {
            return Err(
                "Map, selection or preview changed; inspect the carving state again".into(),
            );
        }
        if request.blend_mode != TerrainBlendMode::Exact
            && request.terrain_mode != TerrainMode::Fragments
        {
            return Err("Native float and Nearest byte require Retile surface triangles".into());
        }
        if request.partition_neighbors && !request.terrain_mode.allows_neighbor_partition() {
            return Err(
                "Partition selected neighbors is unavailable for Retile flat terrain".into(),
            );
        }
        let cutters = request.cutters;
        let targets = request.targets;
        let context_required =
            carve::requires_terrain_context(&self.document, cutters, targets, request.terrain_mode)
                .map_err(|error| error.to_string())?;
        let has_terrain = targets.iter().try_fold(false, |has_terrain, id| {
            let (_, solid) =
                subobject::solid(&self.document, *id).map_err(|error| error.to_string())?;
            Ok::<_, String>(
                has_terrain || solid.sides.iter().any(|side| side.displacement.is_some()),
            )
        })?;
        if has_terrain {
            components::admit_fragment_snapshot(self.document.vmf())?;
        }
        let terrain_context = context::Context::prepare_required(self, context_required)?;
        let mut preparation = carve::Preparation::default();
        let mut texture_faces = carve::terrain_texture_faces_with_preparation(
            &self.document,
            cutters,
            targets,
            &request.options(None, terrain_context.as_deref()),
            &mut preparation,
        )
        .map_err(|error| error.to_string())?;
        let mut terrain_materials = materials::Prepared::prepare(self, &texture_faces)?;
        if request.partition_neighbors {
            loop {
                let required = carve::terrain_partition_faces(
                    &self.document,
                    cutters,
                    targets,
                    request.options(terrain_materials.as_deref(), terrain_context.as_deref()),
                    &mut preparation,
                )
                .map_err(|error| error.to_string())?;
                if required.is_empty() {
                    break;
                }
                let previous_count = texture_faces.len();
                texture_faces.extend(required);
                texture_faces.sort_unstable();
                texture_faces.dedup();
                if texture_faces.len() <= previous_count {
                    return Err("Terrain carve dependency discovery made no progress".into());
                }
                if texture_faces.len() > map_core::clip::MAX_TERRAIN_SOURCES {
                    return Err("Terrain carve exceeds its source limit".into());
                }
                if let Some(previous) = &terrain_materials {
                    previous.verify(&self.game)?;
                }
                let updated = materials::Prepared::prepare(self, &texture_faces)?;
                if let Some(previous) = &terrain_materials {
                    previous.verify(&self.game)?;
                }
                terrain_materials = updated;
            }
        }
        let mut document = self.document.preview_snapshot();
        let result = document
            .edit("preview carve", |tx| {
                carve::carve_many_with_preparation(
                    tx,
                    cutters,
                    targets,
                    request.cut_material,
                    request.options(terrain_materials.as_deref(), terrain_context.as_deref()),
                    &mut preparation,
                )
            })
            .map_err(|error| error.to_string())?;
        let terrain_sources = result
            .terrain
            .iter()
            .map(|cut| cut.source.solid)
            .collect::<BTreeSet<_>>();
        let face_mapping = self.prepare_fragment_mapping(&terrain_sources, &result.side_mapping)?;
        let mut cubemap_bindings =
            self.prepare_fragment_cubemap_bindings(&mut document, &face_mapping)?;
        if let Some(context) = &terrain_context {
            context.verify()?;
        }
        if let Some(materials) = &terrain_materials {
            materials.verify(&self.game)?;
        }
        let requested = targets
            .iter()
            .copied()
            .chain(cutters.iter().copied())
            .collect::<std::collections::BTreeSet<_>>();
        let selected = self.component_solids();
        let scope = if selected.len() <= carve::MAX_TARGETS + carve::MAX_CUTTERS
            && requested.iter().all(|id| selected.contains(id))
        {
            selected
        } else {
            requested.into_iter().collect()
        };
        let owners = scope
            .iter()
            .copied()
            .map(|id| subobject::solid(&self.document, id).map(|(owner, _)| owner))
            .collect::<Result<std::collections::BTreeSet<_>, _>>()
            .map_err(|error| error.to_string())?;
        self.select(&owners.into_iter().collect::<Vec<_>>(), SelectMode::Replace)?;
        cubemap_bindings.retain_after_selection(self);
        if !terrain_sources.is_empty() {
            self.retire_clipped_terrain_state();
        }
        self.cancel_block_preview();
        self.clear_geometry_previews_except(&[]);

        self.cancel_area_selection();
        self.clear_volume_selection();

        self.clip_tool = false;
        self.geometry.paint.enabled = false;
        self.spatial.draw_block = false;
        self.geometry.carving.serial += 1;
        self.geometry.carving.preview = Some(Preview {
            document,
            result,
            render_error: None,
            token: self.geometry.carving.serial,
            targets: targets
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect(),
            scope,
            generation: self.document_generation,
            revision: self.document.revision(),
            definitions: self.definitions_serial,
            face_mapping,
            cubemap_bindings,
            terrain_context,
            terrain_materials,
        });
        self.scene_serial += 1;
        self.view_serial += 1;
        Ok(())
    }

    fn checked_carve(&self, token: u64) -> Result<&Preview, String> {
        let preview = self
            .geometry
            .carving
            .preview
            .as_ref()
            .filter(|preview| preview.token == token)
            .ok_or("Carve preview was closed or replaced")?;
        if preview.generation != self.document_generation
            || preview.revision != self.document.revision()
        {
            return Err("Map changed; preview the carve again".into());
        }
        if !preview.result.terrain.is_empty() && preview.definitions != self.definitions_serial {
            return Err("Definitions changed; preview the terrain carve again".into());
        }
        if let Some(context) = &preview.terrain_context {
            context.current(self)?;
        }
        if let Some(materials) = &preview.terrain_materials {
            materials.current(self)?;
        }
        if let Some(error) = preview.cubemap_bindings.stale_error(self) {
            return Err(error);
        }
        Ok(preview)
    }

    pub fn preflight_carve_preview(&mut self) -> bool {
        let error = self
            .geometry
            .carving
            .preview
            .as_ref()
            .and_then(|preview| self.checked_carve(preview.token).err());
        let Some(preview) = &mut self.geometry.carving.preview else {
            return false;
        };
        if preview.render_error == error {
            return false;
        }
        preview.render_error = error;
        self.scene_serial += 1;
        self.view_serial += 1;
        true
    }

    pub fn verify_carve_preview_render(&self) -> Result<(), String> {
        if let Some(preview) = &self.geometry.carving.preview {
            self.checked_carve(preview.token).map_err(|error| {
                format!("Carve preview unavailable: {error}. Preview again or discard.")
            })?;
        }
        Ok(())
    }

    pub(super) fn append_carve_preview_diagnostic(&self, mesh: &mut render::SceneMesh) {
        if let Some(preview) = &self.geometry.carving.preview
            && let Some(error) = &preview.render_error
            && let Some(solid) = preview.targets.first()
        {
            mesh.issues.prepend(render::scene::SceneIssue::Brush {
                solid: *solid,
                message: format!("Carve preview unavailable: {error}. Applied map shown; preview again or discard."),
            });
        }
    }

    pub fn commit_carve(&mut self, actor: Actor, token: u64) -> Result<carve::Result, String> {
        let preview = self.checked_carve(token)?.clone();
        let expected = preview.result.clone();
        let targets = preview.targets.clone();
        let solids = expected
            .changes
            .iter()
            .flat_map(|change| &change.fragments)
            .map(|id| subobject::solid(&preview.document, *id).map(|(_, solid)| solid.clone()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        if !expected.terrain.is_empty() {
            self.check_fragment_references(
                &expected
                    .terrain
                    .iter()
                    .map(|cut| cut.source.solid)
                    .collect(),
                &BTreeSet::new(),
                &BTreeSet::new(),
            )?;
        }
        let game = self.game.clone();
        let result = self.edit(actor, "carve brushes", |tx| {
            let writes = ridge_bindings::writes_for_mapping(tx.vmf(), &preview.face_mapping)
                .map_err(EditError::Invalid)?;
            if writes != preview.cubemap_bindings.writes {
                return Err(EditError::Invalid(
                    "Cubemap bindings changed; preview the terrain carve again".into(),
                ));
            }
            let result = carve::carve_many_with_options(
                tx,
                &expected.cutters,
                &targets,
                &expected.cut_material,
                ClipOptions {
                    terrain_mode: expected.terrain_mode,
                    blend_mode: expected.blend_mode,
                    partition_neighbors: expected.partition_neighbors,
                    textures: preview
                        .terrain_materials
                        .as_ref()
                        .map(|materials| &materials.textures),
                    terrain_context: preview
                        .terrain_context
                        .as_ref()
                        .map(|context| context.vmf()),
                },
            )?;
            if result != expected
                || solids.iter().any(|solid| {
                    subobject::solid(tx.document(), solid.id)
                        .map_or(true, |(_, current)| current != solid)
                })
            {
                return Err(EditError::Invalid(
                    "Brush or face allocation changed; preview the carve again".into(),
                ));
            }
            ridge_bindings::apply(tx, &writes)?;
            for sources in &preview.cubemap_bindings.sources {
                sources
                    .verify(&game, &compiler::Control::default())
                    .map_err(EditError::Invalid)?;
            }
            if let Some(context) = &preview.terrain_context {
                context.verify().map_err(EditError::Invalid)?;
            }
            if let Some(materials) = &preview.terrain_materials {
                materials.verify(&game).map_err(EditError::Invalid)?;
            }
            Ok(result)
        })?;
        if !result.terrain.is_empty() {
            self.retire_clipped_terrain_state();
        }
        self.select(&result.objects, SelectMode::Replace)?;
        Ok(result)
    }

    pub fn cancel_carve(&mut self, token: u64) -> Result<(), String> {
        self.geometry
            .carving
            .preview
            .as_ref()
            .filter(|preview| preview.token == token)
            .ok_or("Carve preview was closed or replaced")?;
        self.clear_carve_preview();
        Ok(())
    }
}

impl State {
    pub(super) fn close_draft(&self) -> Option<serde_json::Value> {
        self.preview
            .as_ref()
            .map(|preview| serde_json::json!([self.serial, preview.token]))
    }
}
