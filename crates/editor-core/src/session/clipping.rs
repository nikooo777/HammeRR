use super::previews;
use glam::{DVec2, DVec3};
use map_core::clip::{self, ClipResult, Keep};
use map_core::{Document, ObjectId, Plane};
use render::Projection;

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
        self.clear_geometry_previews_except(&[previews::Kind::Face, previews::Kind::Paint]);

        self.clear_volume_selection();

        self.cancel_area_selection();
        self.clip_tool = enabled;
        if enabled {
            self.geometry.paint.enabled = false;
            self.spatial.draw_block = false;
            self.cancel_block_preview();
            self.cancel_face_preview();
            if self.camera.projection == Projection::Perspective {
                self.set_projection(actor, Projection::Top);
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
        if preview {
            if let Some(previous) = &self.geometry.clip_preview
                && previous.ids == ids
                && previous.plane == plane
                && previous.keep == keep
                && previous.material == material
            {
                return Ok(previous.result.clone());
            }
            let mut document = self.document.preview_snapshot();
            let result = document
                .edit("preview clip", |tx| {
                    clip::clip(tx, ids, plane, keep, material.as_deref())
                })
                .map_err(|error| error.to_string())?;
            let (mins, maxs) = map_core::query::bounds_of(&self.document, ids)
                .ok_or("Selected brushes have no bounds")?;
            self.select(ids, SelectMode::Replace)?;
            self.clip_drag = None;
            self.clear_geometry_previews_except(&[previews::Kind::Clip, previews::Kind::Paint]);

            self.clear_volume_selection();
            self.cancel_area_selection();
            self.clip_tool = true;
            self.clip_points = None;
            self.spatial.draw_block = false;

            self.cancel_block_preview();
            self.geometry.clip_preview = Some(ClipPreview {
                document,
                ids: ids.to_vec(),
                plane,
                keep,
                material,
                bounds: [mins, maxs],
                result: result.clone(),
                generation: self.document_generation,
                revision: self.document.revision(),
            });
            self.clip_epoch += 1;
            self.scene_serial += 1;
            self.view_serial += 1;
            Ok(result)
        } else {
            let result = self.edit(actor, "clip brushes", |tx| {
                clip::clip(tx, ids, plane, keep, material.as_deref())
            })?;
            let mut selection = ids.to_vec();
            selection.extend(&result.objects);
            self.select(&selection, SelectMode::Replace)?;
            self.clip_tool = false;
            self.view_serial += 1;
            Ok(result)
        }
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
            .take()
            .ok_or("No clip preview to apply")?;
        self.scene_serial += 1;
        self.view_serial += 1;
        if preview.generation != self.document_generation
            || preview.revision != self.document.revision()
        {
            return Err("The map changed; draw the cut again".into());
        }
        self.clip_brushes(
            actor,
            &preview.ids,
            preview.plane,
            preview.keep,
            preview.material,
            false,
        )
    }
}
