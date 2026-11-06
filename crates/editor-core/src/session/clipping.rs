use std::collections::{BTreeMap, BTreeSet};

use super::{components, components::ridge_bindings, previews};
use glam::{DVec2, DVec3};
use map_core::clip::{self, ClipResult, Keep, TerrainBlendMode, TerrainMode};
use map_core::{Document, EditError, ObjectId, Plane};
use render::Projection;
use serde_json::Value;

use super::{Actor, SelectMode, Session};

mod context;
mod materials;

pub struct ClipRequest<'a> {
    pub ids: &'a [ObjectId],
    pub plane: Plane,
    pub keep: Keep,
    pub material: Option<String>,
    pub preview: bool,
    pub terrain_mode: Option<TerrainMode>,
    pub blend_mode: Option<TerrainBlendMode>,
    pub round_blend: Option<bool>,
    pub partition_neighbors: Option<bool>,
}

#[derive(Clone)]
pub struct ClipPreview {
    pub document: Document,
    pub ids: Vec<ObjectId>,
    pub plane: Plane,
    pub keep: Keep,
    pub material: Option<String>,
    pub terrain_mode: TerrainMode,
    pub blend_mode: TerrainBlendMode,
    pub partition_neighbors: bool,
    pub bounds: [DVec3; 2],
    pub result: ClipResult,
    generation: u64,
    revision: u64,
    definitions: u64,
    terrain_mapping: BTreeMap<u32, Vec<u32>>,
    cubemap_bindings: ridge_bindings::Plan,
    terrain_context: Option<std::sync::Arc<context::Context>>,
    terrain_materials: Option<std::sync::Arc<materials::Prepared>>,
}

impl ClipPreview {
    pub fn cubemap_bindings_report(&self) -> Value {
        self.cubemap_bindings.report()
    }

    pub fn terrain_context_report(&self) -> Option<Value> {
        self.terrain_context
            .as_ref()
            .map(|context| context.report())
    }

    pub fn terrain_materials_report(&self) -> Option<Value> {
        self.terrain_materials
            .as_ref()
            .map(|materials| materials.report())
    }

    fn stale_error(&self, session: &Session) -> Option<String> {
        if self.terrain_mode != session.clip_terrain_mode {
            return Some("Terrain cut mode changed; preview the cut again".into());
        }
        if self.blend_mode != session.clip_blend_mode {
            return Some("Terrain blend mode changed; preview the cut again".into());
        }
        if self.partition_neighbors != session.clip_partition_neighbors {
            return Some("Neighbor partition setting changed; preview the cut again".into());
        }
        self.source_error(session)
    }

    fn source_error(&self, session: &Session) -> Option<String> {
        if self.generation != session.document_generation
            || self.revision != session.document.revision()
        {
            return Some("The map changed; draw the cut again".into());
        }
        if !self.result.terrain.is_empty() && self.definitions != session.definitions_serial {
            return Some("Definitions changed; preview the terrain cut again".into());
        }
        if let Some(context) = &self.terrain_context
            && let Err(error) = context.current(session)
        {
            return Some(error);
        }
        if let Some(materials) = &self.terrain_materials
            && let Err(error) = materials.current(session)
        {
            return Some(error);
        }
        self.cubemap_bindings.stale_error(session)
    }
}

impl Session {
    pub fn can_clip_selection(&self) -> bool {
        clip::sources(&self.document, &self.selected_ids()).is_ok()
    }

    pub fn clip_tool(&self) -> bool {
        self.clip_tool
    }

    pub fn clip_terrain_mode(&self) -> TerrainMode {
        self.clip_terrain_mode
    }

    pub fn clip_blend_mode(&self) -> TerrainBlendMode {
        self.clip_blend_mode
    }

    pub fn clip_round_blend(&self) -> bool {
        self.clip_blend_mode == TerrainBlendMode::NearestByte
    }

    pub fn set_clip_blend_mode(
        &mut self,
        actor: Actor,
        mode: TerrainBlendMode,
    ) -> Result<(), String> {
        if mode != TerrainBlendMode::Exact && self.clip_terrain_mode != TerrainMode::Fragments {
            return Err("Native float and Nearest byte require Retile surface triangles".into());
        }
        if self.clip_blend_mode == mode {
            return Ok(());
        }
        self.cancel_clip_preview();
        self.clip_blend_mode = mode;
        self.view_serial += 1;
        self.log(
            actor,
            match mode {
                TerrainBlendMode::Exact => "Blend samples: Exact fields",
                TerrainBlendMode::NativeFloat => "Blend samples: Native float",
                TerrainBlendMode::NearestByte => "Blend samples: Nearest byte",
            },
        );
        Ok(())
    }

    pub fn set_clip_blend_rounding(&mut self, actor: Actor, enabled: bool) -> Result<(), String> {
        self.set_clip_blend_mode(
            actor,
            if enabled {
                TerrainBlendMode::NearestByte
            } else {
                TerrainBlendMode::Exact
            },
        )
    }

    pub fn clip_partition_neighbors(&self) -> bool {
        self.clip_partition_neighbors
    }

    pub fn set_clip_neighbor_partition(
        &mut self,
        actor: Actor,
        enabled: bool,
    ) -> Result<(), String> {
        if enabled && !self.clip_terrain_mode.allows_neighbor_partition() {
            return Err(
                "Partition selected neighbors is unavailable for Retile flat terrain".into(),
            );
        }
        if self.clip_partition_neighbors == enabled {
            return Ok(());
        }
        self.cancel_clip_preview();
        self.clip_partition_neighbors = enabled;
        self.view_serial += 1;
        self.log(
            actor,
            if enabled {
                "Partition selected neighbors: on"
            } else {
                "Partition selected neighbors: off"
            },
        );
        Ok(())
    }

    pub fn set_clip_terrain_mode(&mut self, actor: Actor, mode: TerrainMode) -> Result<(), String> {
        if self.clip_terrain_mode == mode {
            return Ok(());
        }
        self.cancel_clip_preview();
        self.clip_terrain_mode = mode;
        self.clip_blend_mode = TerrainBlendMode::Exact;
        self.clip_partition_neighbors = false;
        self.view_serial += 1;
        self.log(
            actor,
            match mode {
                TerrainMode::Grid => "Terrain cut: Existing grid",
                TerrainMode::Flat => "Terrain cut: Retile flat terrain",
                TerrainMode::Cells => "Terrain cut: Retile grid cells",
                TerrainMode::Fragments => "Terrain cut: Retile surface triangles",
            },
        );
        Ok(())
    }

    pub fn set_clip_tool(&mut self, actor: Actor, enabled: bool) -> Result<(), String> {
        if enabled && !self.can_clip_selection() {
            return Err("Select brushes or brush entities before starting a cut".into());
        }
        if self.clip_tool == enabled {
            return Ok(());
        }
        let camera = (enabled && self.camera.projection == Projection::Perspective)
            .then(|| self.camera_for_projection(Projection::Top));
        if let Some(camera) = camera {
            self.view_camera_candidates(camera)?;
        }
        self.clear_geometry_previews_except(&[previews::Kind::Face, previews::Kind::Paint]);

        self.clear_volume_selection();

        self.cancel_area_selection();
        self.clip_tool = enabled;
        if enabled {
            self.geometry.paint.enabled = false;
            self.spatial.draw_block = false;
            self.cancel_block_preview();
            self.cancel_face_preview();
            if let Some(camera) = camera {
                self.try_set_camera(camera)?;
                self.log(actor, format!("{} view", Projection::Top.name()));
            }
        }
        self.view_serial += 1;
        Ok(())
    }

    pub fn clip_epoch(&self) -> u64 {
        self.clip_epoch
    }

    pub fn clip_preview(&self) -> Option<&ClipPreview> {
        self.geometry.clip_preview.as_ref()
    }

    pub fn clip_preview_error(&self) -> Option<String> {
        self.geometry
            .clip_preview
            .as_ref()
            .and_then(|preview| preview.stale_error(self))
    }

    pub fn cancel_clip_preview(&mut self) {
        self.clip_drag = None;
        self.clip_epoch += 1;
        if self.clip_points.take().is_some() {
            self.scene_serial += 1;
            self.view_serial += 1;
        }
        self.clear_clip_preview();
    }

    pub fn clear_clip_preview(&mut self) {
        self.clip_drag = None;
        if self.geometry.clip_preview.take().is_some() {
            self.clip_epoch += 1;
            self.scene_serial += 1;
            self.view_serial += 1;
        }
    }

    pub fn clip_line_plane(
        &self,
        projection: Projection,
        start: DVec2,
        end: DVec2,
    ) -> Result<Plane, String> {
        let (horizontal, vertical, depth) = projection
            .axes()
            .ok_or("Choose Top, Front or Side to draw a cut")?;
        if !start.is_finite() || !end.is_finite() {
            return Err("Cut endpoints must be finite".into());
        }
        let mut a = DVec3::ZERO;
        a[horizontal] = start.x;
        a[vertical] = start.y;
        a[depth] = self.spatial.elevation;
        let mut b = a;
        b[horizontal] = end.x;
        b[vertical] = end.y;
        a = self.spatial.snap_point(a);
        b = self.spatial.snap_point(b);
        let mut c = a;
        c[depth] += 64.0;
        Plane::from_points([a, b, c]).ok_or("Draw a nonzero cut line after snapping".into())
    }

    pub fn clip_brushes(
        &mut self,
        actor: Actor,
        ids: &[ObjectId],
        plane: Plane,
        keep: Keep,
        material: Option<String>,
        preview: bool,
    ) -> Result<ClipResult, String> {
        self.clip_brushes_request(
            actor,
            ClipRequest {
                ids,
                plane,
                keep,
                material,
                preview,
                terrain_mode: None,
                blend_mode: None,
                round_blend: None,
                partition_neighbors: None,
            },
        )
    }

    pub fn clip_brushes_request(
        &mut self,
        actor: Actor,
        mut request: ClipRequest<'_>,
    ) -> Result<ClipResult, String> {
        if request.blend_mode.is_some() && request.round_blend.is_some() {
            return Err("Provide blend_mode or round_blend, not both".into());
        }
        request.plane = clip::normalized_plane(request.plane).map_err(|error| error.to_string())?;
        let terrain_mode = request.terrain_mode.unwrap_or(self.clip_terrain_mode);
        let blend_mode = request
            .blend_mode
            .or_else(|| {
                request.round_blend.map(|enabled| {
                    if enabled {
                        TerrainBlendMode::NearestByte
                    } else {
                        TerrainBlendMode::Exact
                    }
                })
            })
            .unwrap_or(if terrain_mode == self.clip_terrain_mode {
                self.clip_blend_mode
            } else {
                TerrainBlendMode::Exact
            });
        if blend_mode != TerrainBlendMode::Exact && terrain_mode != TerrainMode::Fragments {
            return Err("Native float and Nearest byte require Retile surface triangles".into());
        }
        let partition_neighbors = request
            .partition_neighbors
            .unwrap_or(terrain_mode == self.clip_terrain_mode && self.clip_partition_neighbors);
        if partition_neighbors && !terrain_mode.allows_neighbor_partition() {
            return Err(
                "Partition selected neighbors is unavailable for Retile flat terrain".into(),
            );
        }
        if request.preview
            && let Some(previous) = &self.geometry.clip_preview
            && previous.ids == request.ids
            && previous.plane == request.plane
            && previous.keep == request.keep
            && previous.material == request.material
            && previous.terrain_mode == terrain_mode
            && previous.blend_mode == blend_mode
            && previous.partition_neighbors == partition_neighbors
        {
            if let Some(error) = previous.stale_error(self) {
                return Err(error);
            }
            return Ok(previous.result.clone());
        }
        let mut prepared =
            self.prepare_clip(&request, terrain_mode, blend_mode, partition_neighbors)?;
        if request.preview {
            let result = prepared.result.clone();
            self.select(request.ids, SelectMode::Replace)?;
            prepared.cubemap_bindings.retain_after_selection(self);
            if !result.terrain.is_empty() {
                self.retire_clipped_terrain_state();
            }
            self.clip_drag = None;
            self.clear_geometry_previews_except(&[previews::Kind::Clip, previews::Kind::Paint]);

            self.clear_volume_selection();
            self.cancel_area_selection();
            self.clip_tool = true;
            self.clip_points = None;
            self.spatial.draw_block = false;

            self.cancel_block_preview();
            self.clip_terrain_mode = terrain_mode;
            self.clip_blend_mode = blend_mode;
            self.clip_partition_neighbors = partition_neighbors;
            self.geometry.clip_preview = Some(prepared);
            self.clip_epoch += 1;
            self.scene_serial += 1;
            self.view_serial += 1;
            Ok(result)
        } else {
            self.apply_prepared_clip(actor, prepared)
        }
    }

    fn prepare_clip(
        &self,
        request: &ClipRequest<'_>,
        terrain_mode: TerrainMode,
        blend_mode: TerrainBlendMode,
        partition_neighbors: bool,
    ) -> Result<ClipPreview, String> {
        let ids = request.ids;
        let plane = request.plane;
        let keep = request.keep;
        let material = request.material.clone();
        let has_terrain = clip::sources(&self.document, ids)
            .map_err(|error| error.to_string())?
            .iter()
            .any(|source| {
                source
                    .solid
                    .sides
                    .iter()
                    .any(|side| side.displacement.is_some())
            });
        if has_terrain {
            components::admit_fragment_snapshot(self.document.vmf())?;
        }
        let mut retile_faces = if terrain_mode != TerrainMode::Grid {
            clip::terrain_texture_faces(&self.document, ids, plane, terrain_mode)
                .map_err(|error| error.to_string())?
        } else {
            Vec::new()
        };
        let terrain_context = if has_terrain {
            let visible_retile_crossing = retile_faces.iter().any(|face| {
                map_core::displacement::native::terrain_source_is_visible(
                    self.document.vmf(),
                    face.solid,
                )
            });
            context::Context::prepare(self, ids, plane, terrain_mode, visible_retile_crossing)?
        } else {
            None
        };
        if partition_neighbors
            && matches!(terrain_mode, TerrainMode::Cells | TerrainMode::Fragments)
        {
            let partition_faces = clip::terrain_partition_faces(
                &self.document,
                ids,
                plane,
                keep,
                terrain_mode,
                terrain_context.as_ref().map(|context| context.vmf()),
            )
            .map_err(|error| error.to_string())?;
            retile_faces.extend(partition_faces);
            retile_faces.sort_unstable();
            retile_faces.dedup();
        }
        let terrain_materials = materials::Prepared::prepare(self, &retile_faces)?;
        let mut document = self.document.preview_snapshot();
        let result = document
            .edit("preview clip", |tx| {
                clip::clip_with_options(
                    tx,
                    ids,
                    plane,
                    keep,
                    material.as_deref(),
                    clip::ClipOptions {
                        terrain_mode,
                        blend_mode,
                        partition_neighbors,
                        textures: terrain_materials
                            .as_ref()
                            .map(|materials| &materials.textures),
                        terrain_context: terrain_context.as_ref().map(|context| context.vmf()),
                    },
                )
            })
            .map_err(|error| error.to_string())?;
        let terrain_sources = result
            .terrain
            .iter()
            .map(|cut| cut.source.solid)
            .collect::<BTreeSet<_>>();
        let mut terrain_mapping = BTreeMap::new();
        if !terrain_sources.is_empty() {
            self.check_fragment_references(&terrain_sources, &BTreeSet::new(), &BTreeSet::new())?;
            for source in terrain_sources {
                let (_, solid) = map_core::subobject::solid(&self.document, source)
                    .map_err(|error| error.to_string())?;
                for side in &solid.sides {
                    let targets = result
                        .side_mapping
                        .get(&side.id)
                        .ok_or("Terrain cut is missing a source face replacement")?;
                    terrain_mapping.insert(side.id, targets.clone());
                }
            }
        }
        let cubemap_bindings =
            self.prepare_fragment_cubemap_bindings(&mut document, &terrain_mapping)?;
        let (mins, maxs) = map_core::query::bounds_of(&self.document, ids)
            .ok_or("Selected brushes have no bounds")?;
        if let Some(context) = &terrain_context {
            context.verify()?;
        }
        if let Some(materials) = &terrain_materials {
            materials.verify(&self.game)?;
        }
        Ok(ClipPreview {
            document,
            ids: ids.to_vec(),
            plane,
            keep,
            material,
            terrain_mode,
            blend_mode,
            partition_neighbors,
            bounds: [mins, maxs],
            result,
            generation: self.document_generation,
            revision: self.document.revision(),
            definitions: self.definitions_serial,
            terrain_mapping,
            cubemap_bindings,
            terrain_context,
            terrain_materials,
        })
    }

    fn retire_clipped_terrain_state(&mut self) {
        self.retire_terrain_move_handles();
        self.retire_terrain_stroke();
        let _ = self.clear_terrain_triangle_highlight(None);
    }

    fn apply_prepared_clip(
        &mut self,
        actor: Actor,
        preview: ClipPreview,
    ) -> Result<ClipResult, String> {
        if let Some(error) = preview.source_error(self) {
            return Err(error);
        }
        if !preview.result.terrain.is_empty() {
            self.check_fragment_references(
                &preview
                    .result
                    .terrain
                    .iter()
                    .map(|cut| cut.source.solid)
                    .collect(),
                &BTreeSet::new(),
                &BTreeSet::new(),
            )?;
        }
        let game = self.game.clone();
        let result = self.edit(actor, "clip brushes", |tx| {
            let writes = ridge_bindings::writes_for_mapping(tx.vmf(), &preview.terrain_mapping)
                .map_err(EditError::Invalid)?;
            if writes != preview.cubemap_bindings.writes {
                return Err(EditError::Invalid(
                    "Cubemap bindings changed; preview the terrain cut again".into(),
                ));
            }
            let result = clip::clip_with_options(
                tx,
                &preview.ids,
                preview.plane,
                preview.keep,
                preview.material.as_deref(),
                clip::ClipOptions {
                    terrain_mode: preview.terrain_mode,
                    blend_mode: preview.blend_mode,
                    partition_neighbors: preview.partition_neighbors,
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
            if result != preview.result {
                return Err(EditError::Invalid(
                    "Cut geometry or allocation changed; preview the cut again".into(),
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
        let mut selection = preview.ids;
        selection.extend(&result.objects);
        self.select(&selection, SelectMode::Replace)?;
        self.clip_terrain_mode = preview.terrain_mode;
        self.clip_blend_mode = preview.blend_mode;
        self.clip_partition_neighbors = preview.partition_neighbors;
        self.clip_epoch += 1;
        self.clip_tool = false;
        self.view_serial += 1;
        Ok(result)
    }

    pub fn commit_clip_preview(&mut self, actor: Actor) -> Result<ClipResult, String> {
        if self.clip_drag.is_some() {
            return Err(
                "Finish or cancel positioning the cut plane before applying the cut".into(),
            );
        }
        let preview = self
            .geometry
            .clip_preview
            .as_ref()
            .ok_or("No clip preview to apply")?;
        if let Some(error) = preview.stale_error(self) {
            return Err(error);
        }
        let preview = preview.clone();
        self.apply_prepared_clip(actor, preview)
    }
}
