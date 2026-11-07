use glam::DVec3;
use render::{Camera, Projection};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{Actor, Session};

pub const MAX_PANES: usize = 4;
pub const MAX_CANVAS_PIXELS: u64 = 16_777_216;

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Layout {
    #[default]
    Single,
    TwoColumns,
    TwoRows,
    Quad,
}

#[derive(Clone, Copy, Debug)]
pub struct Pane {
    pub index: usize,
    pub camera: Camera,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct PaneRect {
    pub index: usize,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Layout {
    pub fn pane_count(self) -> usize {
        match self {
            Self::Single => 1,
            Self::TwoColumns | Self::TwoRows => 2,
            Self::Quad => MAX_PANES,
        }
    }

    pub fn rectangles(self, size: (u32, u32)) -> Result<Vec<PaneRect>, String> {
        let (width, height) = size;
        if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_CANVAS_PIXELS {
            return Err(format!(
                "Viewport canvas needs positive dimensions and at most {MAX_CANVAS_PIXELS} pixels"
            ));
        }
        let columns = matches!(self, Self::TwoColumns | Self::Quad);
        let rows = matches!(self, Self::TwoRows | Self::Quad);
        let mut result = Vec::with_capacity(self.pane_count());
        for index in 0..self.pane_count() {
            let right = columns && index % 2 == 1;
            let bottom = rows && if columns { index >= 2 } else { index == 1 };
            let x = if right { width / 2 } else { 0 };
            let y = if bottom { height / 2 } else { 0 };
            let pane_width = if columns {
                if right { width - width / 2 } else { width / 2 }
            } else {
                width
            };
            let pane_height = if rows {
                if bottom {
                    height - height / 2
                } else {
                    height / 2
                }
            } else {
                height
            };
            if pane_width == 0 || pane_height == 0 {
                return Err("Canvas is too small for the selected view layout".into());
            }
            result.push(PaneRect {
                index,
                x,
                y,
                width: pane_width,
                height: pane_height,
            });
        }
        Ok(result)
    }
}

pub(super) struct State {
    pub(super) layout: Layout,
    pub(super) active: usize,
    pub(super) linked: bool,
    pub(super) cameras: [Camera; MAX_PANES],
    pub(super) serial: u64,
    pub(super) error: Option<String>,
}

impl State {
    pub(super) fn new(camera: Camera) -> Self {
        Self {
            layout: Layout::Single,
            active: 0,
            linked: false,
            cameras: [
                camera,
                Camera::planar(Projection::Top, glam::Vec3::ZERO, 512.0),
                Camera::planar(Projection::Front, glam::Vec3::ZERO, 512.0),
                Camera::planar(Projection::Side, glam::Vec3::ZERO, 512.0),
            ],
            serial: 1,
            error: None,
        }
    }
}

pub fn camera_report(camera: &Camera) -> Value {
    json!({"position":camera.position.to_array(),"yaw":camera.yaw_degrees,
        "pitch":camera.pitch_degrees,"fov":camera.fov_y_degrees,"forward":camera.forward().to_array(),
        "projection":camera.projection.name(),"ortho_height":camera.ortho_height,
        "center":(camera.projection!=Projection::Perspective).then(||(camera.position+camera.forward()*Camera::ORTHO_DISTANCE).to_array())})
}

impl Session {
    pub fn view_layout(&self) -> Layout {
        self.view_layout.layout
    }
    pub fn active_view_pane(&self) -> usize {
        self.view_layout.active
    }
    pub fn view_layout_serial(&self) -> u64 {
        self.view_layout.serial
    }
    pub fn linked_orthographic_views(&self) -> bool {
        self.view_layout.linked
    }

    pub fn view_pane_camera(&self, index: usize) -> Result<Camera, String> {
        self.view_layout
            .cameras
            .get(index)
            .copied()
            .ok_or_else(|| format!("View pane index must be below {MAX_PANES}"))
    }

    pub fn visible_view_panes(&self) -> Vec<Pane> {
        (0..self.view_layout.layout.pane_count())
            .map(|index| Pane {
                index,
                camera: self.view_layout.cameras[index],
            })
            .collect()
    }

    fn view_layout_stamp(&self) -> String {
        format!(
            "view-layout:{}:{}:{}",
            self.buffer_review_scope, self.document_generation, self.view_layout.serial
        )
    }

    fn check_view_layout_stamp(&self, expected: Option<&str>) -> Result<(), String> {
        self.ensure_not_closing()?;
        if expected.is_some_and(|stamp| stamp != self.view_layout_stamp()) {
            return Err(
                "View layout or pane cameras changed; inspect the view layout again".into(),
            );
        }
        Ok(())
    }

    fn view_layout_transition_ready(&self) -> Result<(), String> {
        if self.transform_drag.is_some()
            || self.clip_drag.is_some()
            || self.component_drag().is_some()
            || self.area_preview.is_some()
            || self.volume_preview().is_some()
            || self.block_preview.is_some()
            || self.paint_stroke().is_some()
            || self.terrain_stroke_active()
            || self.terrain_move_dragging()
            || self.overlay_corner_handle_dragging()
            || self.measurement_picking()
            || self.linked_path_picking()
            || self.transform_pivot_picking()
            || self.entity_surface_picking()
            || self.rope_surface_picking()
            || self.surface_position_picking()
        {
            return Err(
                "Finish or cancel the captured viewport interaction before changing view panes"
                    .into(),
            );
        }
        Ok(())
    }

    pub fn view_layout_snapshot(&self) -> Value {
        let blocker = self.view_layout_transition_ready().err();
        json!({"snapshot":self.view_layout_stamp(),"serial":self.view_layout.serial,
            "layout":self.view_layout.layout,"active_pane":self.view_layout.active,"linked_orthographic":self.view_layout.linked,
            "panes":self.view_layout.cameras.iter().enumerate().map(|(index,camera)|json!({
                "index":index,"visible":index<self.view_layout.layout.pane_count(),"active":index==self.view_layout.active,"camera":camera_report(camera)
            })).collect::<Vec<_>>(),"can_switch":!self.closing && blocker.is_none(),"switch_error":blocker,"camera_error":self.view_layout.error,
            "limits":{"panes":MAX_PANES,"canvas_pixels":MAX_CANVAS_PIXELS,"min_ortho_height":Camera::MIN_ORTHO_HEIGHT,"max_ortho_height":Camera::MAX_ORTHO_HEIGHT},
            "link_policy":"Active orthographic pan applies the same world displacement to other orthographic panes; zoom applies its ratio within each camera's existing height limits. Perspective cameras remain independent. Frame all uses visible panes. New/Open retains layout, active index, linking and inactive projections; the active projection follows the accepted map camera."})
    }

    pub fn update_view_layout(
        &mut self,
        actor: Actor,
        layout: Option<Layout>,
        linked: Option<bool>,
        expected: Option<&str>,
    ) -> Result<Value, String> {
        self.check_view_layout_stamp(expected)?;
        if layout.is_none() && linked.is_none() {
            return Err("Choose a layout or linked orthographic setting".into());
        }
        let layout = layout.unwrap_or(self.view_layout.layout);
        let linked = linked.unwrap_or(self.view_layout.linked);
        if layout != self.view_layout.layout {
            self.view_layout_transition_ready()?;
            self.view_layout.layout = layout;
            if self.view_layout.active >= layout.pane_count() {
                self.activate_view_pane_inner(0);
            }
            self.view_serial += 1;
        }
        self.view_layout.linked = linked;
        self.view_layout.serial += 1;
        self.view_layout.error = None;
        self.log(actor, "Changed view layout");
        Ok(self.view_layout_snapshot())
    }

    fn activate_view_pane_inner(&mut self, index: usize) {
        let previous = self.camera;
        self.saved_cameras[self.camera.projection.index()] = Some(self.camera);
        self.view_layout.active = index;
        self.camera = self.view_layout.cameras[index];
        self.clip_epoch += 1;
        self.interaction_epoch += 1;
        self.camera_serial += 1;
        self.working_region_camera_changed(previous);
    }

    pub fn activate_view_pane(
        &mut self,
        actor: Actor,
        index: usize,
        expected: Option<&str>,
    ) -> Result<Value, String> {
        self.check_view_layout_stamp(expected)?;
        if index >= self.view_layout.layout.pane_count() {
            return Err("Choose a visible view pane".into());
        }
        if index != self.view_layout.active {
            self.view_layout_transition_ready()?;
            self.activate_view_pane_inner(index);
            self.view_layout.serial += 1;
            self.view_serial += 1;
            self.view_layout.error = None;
            self.log(actor, format!("Activated view pane {}", index + 1));
        }
        Ok(self.view_layout_snapshot())
    }

    pub fn camera_for_view_pane_projection(
        &self,
        index: usize,
        projection: Projection,
    ) -> Result<Camera, String> {
        let camera = self.view_pane_camera(index)?;
        Ok(if camera.projection == projection {
            camera
        } else {
            self.camera_for_projection(projection)
        })
    }

    pub(super) fn view_camera_candidates(
        &self,
        camera: Camera,
    ) -> Result<[Camera; MAX_PANES], String> {
        camera.validate(1.0)?;
        let mut cameras = self.view_layout.cameras;
        cameras[self.view_layout.active] = camera;
        let previous = self.camera;
        if self.view_layout.linked
            && previous.projection != Projection::Perspective
            && camera.projection == previous.projection
        {
            let delta = camera.position.as_dvec3() - previous.position.as_dvec3();
            let ratio = f64::from(camera.ortho_height) / f64::from(previous.ortho_height);
            for (index, linked) in cameras.iter_mut().enumerate() {
                if index == self.view_layout.active || linked.projection == Projection::Perspective
                {
                    continue;
                }
                linked.position = (linked.position.as_dvec3() + delta).as_vec3();
                linked.ortho_height = (f64::from(linked.ortho_height) * ratio).clamp(
                    f64::from(Camera::MIN_ORTHO_HEIGHT),
                    f64::from(Camera::MAX_ORTHO_HEIGHT),
                ) as f32;
                linked.validate(1.0)?;
            }
        }
        Ok(cameras)
    }

    pub(super) fn reject_view_camera(&mut self, error: String) {
        self.view_layout.error = Some(error);
        self.view_layout.serial += 1;
        self.camera_serial += 1;
    }

    pub fn set_view_pane_camera(
        &mut self,
        actor: Actor,
        index: usize,
        camera: Camera,
        aspect: f32,
        expected: Option<&str>,
    ) -> Result<Value, String> {
        self.check_view_layout_stamp(expected)?;
        self.view_pane_camera(index)?;
        super::inspection_framing::validate_aspect(aspect)?;
        camera.validate(aspect)?;
        if index == self.view_layout.active {
            self.view_layout_transition_ready()?;
            self.try_set_camera(camera)?;
        } else if self.view_layout.cameras[index] != camera {
            self.view_layout.cameras[index] = camera;
            self.view_layout.serial += 1;
            self.view_layout.error = None;
        }
        self.log(actor, format!("Changed view pane {} camera", index + 1));
        Ok(self.view_layout_snapshot())
    }

    pub fn set_view_pane_projection(
        &mut self,
        actor: Actor,
        index: usize,
        projection: Projection,
        expected: Option<&str>,
    ) -> Result<Value, String> {
        self.check_view_layout_stamp(expected)?;
        let camera = self.camera_for_view_pane_projection(index, projection)?;
        self.set_view_pane_camera(actor, index, camera, 1.0, None)
    }

    pub(super) fn view_framing_bounds(&self) -> Option<(DVec3, DVec3)> {
        if self.selection.is_empty() && self.face_selection.is_empty() {
            let document = self.render_document();
            self.framing_bounds_in(document, &document.objects().collect::<Vec<_>>(), None)
        } else if !self.face_mode {
            self.framing_bounds_in(self.render_document(), &self.selected_ids(), None)
        } else {
            self.selection_bounds_in(self.render_document())
        }
    }

    pub fn frame_all_views(
        &mut self,
        actor: Actor,
        aspects: [f32; MAX_PANES],
        expected: Option<&str>,
    ) -> Result<Value, String> {
        self.check_view_layout_stamp(expected)?;
        self.view_layout_transition_ready()?;
        for aspect in &aspects[..self.view_layout.layout.pane_count()] {
            super::inspection_framing::validate_aspect(*aspect)?;
        }
        self.preflight_instance_contents();
        let (mins, maxs) = self.view_framing_bounds().ok_or("No objects to frame")?;
        let mut cameras = self.view_layout.cameras;
        for (index, camera) in cameras
            .iter_mut()
            .enumerate()
            .take(self.view_layout.layout.pane_count())
        {
            *camera = camera.frame_bounds(mins.as_vec3(), maxs.as_vec3(), aspects[index]);
            camera.validate(aspects[index])?;
        }
        let previous = self.camera;
        self.view_layout.cameras = cameras;
        self.camera = cameras[self.view_layout.active];
        self.camera_serial += 1;
        self.clip_epoch += 1;
        self.interaction_epoch += 1;
        self.view_layout.serial += 1;
        self.view_layout.error = None;
        self.working_region_camera_changed(previous);
        self.log(actor, "Framed all visible views");
        Ok(self.view_layout_snapshot())
    }

    pub(super) fn reseed_view_layout_cameras(&mut self) {
        let active = self.view_layout.active;
        let projections = self.view_layout.cameras.map(|camera| camera.projection);
        let mut cameras = projections.map(|projection| self.camera_for_projection(projection));
        cameras[active] = self.camera;
        self.view_layout.cameras = cameras;
        self.view_layout.serial += 1;
        self.view_layout.error = None;
    }
}
