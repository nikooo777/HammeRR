use std::collections::{BTreeMap, BTreeSet};

use super::{components, components::ridge_bindings, previews};
use glam::{DVec2, DVec3};
use map_core::clip::{self, ClipResult, Keep};
use map_core::{Document, EditError, ObjectId, Plane};
use render::Projection;
use serde_json::Value;

use super::{Actor, SelectMode, Session};

#[derive(Clone)]
pub struct ClipPreview {
    pub document: Document,
    pub ids: Vec<ObjectId>,
    pub plane: Plane,
    pub keep: Keep,
    pub material: Option<String>,
    pub bounds: [DVec3; 2],
    pub result: ClipResult,
    generation: u64,
    revision: u64,
    definitions: u64,
    terrain_mapping: BTreeMap<u32, Vec<u32>>,
    cubemap_bindings: ridge_bindings::Plan,
}

impl ClipPreview {
    pub fn cubemap_bindings_report(&self) -> Value {
        self.cubemap_bindings.report()
    }

    fn stale_error(&self, session: &Session) -> Option<String> {
        if self.generation != session.document_generation
            || self.revision != session.document.revision()
        {
            return Some("The map changed; draw the cut again".into());
        }
        if !self.result.terrain.is_empty() && self.definitions != session.definitions_serial {
            return Some("Definitions changed; preview the terrain cut again".into());
        }
        self.cubemap_bindings.stale_error(session)
    }
}

impl Session {
    pub fn can_clip_selection(&self) -> bool {
        !self.selection.is_empty()
            && self.selection.iter().all(|id| match id {
                ObjectId::Solid(id) => self.document.solid(*id).is_some(),
                ObjectId::Entity(id) => self
                    .document
                    .entity(*id)
                    .is_some_and(|entity| !entity.solids.is_empty()),
            })
    }

    pub fn clip_tool(&self) -> bool {
        self.clip_tool
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
        let plane = clip::normalized_plane(plane).map_err(|error| error.to_string())?;
        if preview
            && let Some(previous) = &self.geometry.clip_preview
            && previous.ids == ids
            && previous.plane == plane
            && previous.keep == keep
            && previous.material == material
        {
            if let Some(error) = previous.stale_error(self) {
                return Err(error);
            }
            return Ok(previous.result.clone());
        }
        let mut prepared = self.prepare_clip(ids, plane, keep, material)?;
        if preview {
            let result = prepared.result.clone();
            self.select(ids, SelectMode::Replace)?;
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
        ids: &[ObjectId],
        plane: Plane,
        keep: Keep,
        material: Option<String>,
    ) -> Result<ClipPreview, String> {
        let has_terrain = ids.iter().any(|id| match id {
            ObjectId::Solid(id) => self
                .document
                .solid(*id)
                .is_some_and(|solid| solid.sides.iter().any(|side| side.displacement.is_some())),
            ObjectId::Entity(id) => self.document.entity(*id).is_some_and(|entity| {
                entity
                    .solids
                    .iter()
                    .any(|solid| solid.sides.iter().any(|side| side.displacement.is_some()))
            }),
        });
        if has_terrain {
            components::admit_fragment_snapshot(self.document.vmf())?;
        }
        let mut document = self.document.preview_snapshot();
        let result = document
            .edit("preview clip", |tx| {
                clip::clip(tx, ids, plane, keep, material.as_deref())
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
        Ok(ClipPreview {
            document,
            ids: ids.to_vec(),
            plane,
            keep,
            material,
            bounds: [mins, maxs],
            result,
            generation: self.document_generation,
            revision: self.document.revision(),
            definitions: self.definitions_serial,
            terrain_mapping,
            cubemap_bindings,
        })
    }

    fn retire_clipped_terrain_state(&mut self) {
        self.retire_terrain_move_handles();
        self.working_region_end_terrain_stroke();
        let _ = self.clear_terrain_triangle_highlight(None);
    }

    fn apply_prepared_clip(
        &mut self,
        actor: Actor,
        preview: ClipPreview,
    ) -> Result<ClipResult, String> {
        if let Some(error) = preview.stale_error(self) {
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
            let result = clip::clip(
                tx,
                &preview.ids,
                preview.plane,
                preview.keep,
                preview.material.as_deref(),
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
            Ok(result)
        })?;
        if !result.terrain.is_empty() {
            self.retire_clipped_terrain_state();
        }
        let mut selection = preview.ids;
        selection.extend(&result.objects);
        self.select(&selection, SelectMode::Replace)?;
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
        let preview = self.geometry.clip_preview.take().unwrap();
        self.scene_serial += 1;
        self.view_serial += 1;
        self.apply_prepared_clip(actor, preview)
    }
}
