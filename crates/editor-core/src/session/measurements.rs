use glam::DVec3;
use map_core::measurements::{self, Measurement, Measurements};
use map_core::query::{Ray, SurfaceHit};
use render::Camera;
use serde_json::{Value, json};

use super::{Actor, Pointer, Session};

#[derive(Clone, Copy, Debug, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Endpoint {
    Start,
    End,
}

const MAX_PICK_VIEWPORT_SIZE: u32 = 8192;

#[derive(Clone, Debug)]
pub struct Request {
    pub id: Option<u32>,
    pub name: String,
    pub start: DVec3,
    pub end: DVec3,
}

pub(super) struct State {
    serial: u64,
    show: bool,
    draft: Option<Draft>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            serial: 0,
            show: true,
            draft: None,
        }
    }
}

impl State {
    pub(super) fn has_draft(&self) -> bool {
        self.draft.is_some()
    }

    pub(super) fn reset_document(&mut self) {
        self.draft = None;
        self.serial += 1;
    }
}

struct Draft {
    token: u64,
    generation: u64,
    revision: u64,
    owner: Actor,
    measurement: Measurement,
    replace: bool,
    picking: Option<Picking>,
    last_pick: Option<(Endpoint, SurfaceHit)>,
}

struct Picking {
    endpoint: Endpoint,
    scene: u64,
    view: u64,
    camera_serial: u64,
    session_camera: Camera,
    camera: Camera,
    size: (u32, u32),
}

fn measurement_json(measurement: &Measurement) -> Value {
    json!({"id":measurement.id,"name":measurement.name,
        "start":measurement.start.to_array(),"end":measurement.end.to_array(),
        "distance":measurement.distance(),"components":measurement.components().to_array()})
}

impl Session {
    pub fn measurements(&self) -> Result<Measurements, String> {
        Measurements::parse(self.document.vmf())
    }

    pub fn measurement_stamp(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.document_generation,
            self.document.revision(),
            self.measurements.serial,
            self.measurement_context().is_ok()
        )
    }

    fn check_measurement_stamp(&self, snapshot: &str) -> Result<(), String> {
        if snapshot != self.measurement_stamp() {
            return Err("Map or measurement state changed; inspect Measurements again".into());
        }
        Ok(())
    }

    pub fn measurement_pick_stamp(&self) -> String {
        format!(
            "{}:{}:{}:{}:{:?}",
            self.measurement_stamp(),
            self.scene_serial,
            self.view_serial,
            self.camera_serial,
            self.camera
        )
    }

    fn checked_measurement_pick(&self, actor: Actor, token: u64) -> Result<&Picking, String> {
        let draft = self.checked_measurement(token)?;
        if draft.owner != actor {
            return Err("Review the other actor's measurement before picking an endpoint".into());
        }
        let picking = draft
            .picking
            .as_ref()
            .ok_or("Choose Pick start or Pick end first")?;
        if picking.scene != self.scene_serial
            || picking.view != self.view_serial
            || picking.camera_serial != self.camera_serial
            || picking.session_camera != self.camera
        {
            return Err("Scene or camera changed; choose Pick start or Pick end again".into());
        }
        Ok(picking)
    }

    fn measurement_context(&self) -> Result<(), String> {
        if self.closing {
            return Err("The editor is closing".into());
        }
        if !std::ptr::eq(self.render_document(), &self.document) || self.block_preview.is_some() {
            return Err("Apply or cancel the geometry preview before changing measurements".into());
        }
        Ok(())
    }

    fn checked_measurement(&self, token: u64) -> Result<&Draft, String> {
        let draft = self
            .measurements
            .draft
            .as_ref()
            .filter(|draft| draft.token == token)
            .ok_or("Measurement preview was closed or replaced")?;
        if draft.generation != self.document_generation
            || draft.revision != self.document.revision()
        {
            return Err("Map changed; preview the measurement again".into());
        }
        self.measurement_context()?;
        Ok(draft)
    }

    pub fn measurement_state(&self) -> Value {
        let (entries, metadata_error) = match self.measurements() {
            Ok(metadata) => (
                metadata
                    .measurements
                    .iter()
                    .map(measurement_json)
                    .collect::<Vec<_>>(),
                None,
            ),
            Err(error) => (Vec::new(), Some(error)),
        };
        let draft = self.measurements.draft.as_ref().map(|draft| {
            let picking = draft.picking.as_ref().map(|picking| json!({
                "endpoint":picking.endpoint,"width":picking.size.0,"height":picking.size.1,
                "valid":self.checked_measurement_pick(draft.owner,draft.token).is_ok(),
                "error":self.checked_measurement_pick(draft.owner,draft.token).err()}));
            let last_pick = draft.last_pick.map(|(endpoint, surface)| json!({"endpoint":endpoint,
                "target":crate::identity::id_string(surface.hit.object),"solid":surface.hit.solid,
                "side":surface.hit.side,"point":surface.hit.point.to_array(),"normal":surface.normal.to_array()}));
            json!({"token":draft.token,"owner":draft.owner.label(),"replace":draft.replace,
                "measurement":measurement_json(&draft.measurement),
                "picking":picking,"last_pick":last_pick,
                "valid":self.checked_measurement(draft.token).is_ok(),
                "error":self.checked_measurement(draft.token).err()})
        });
        json!({"snapshot":self.measurement_stamp(),"pick_snapshot":self.measurement_pick_stamp(),"generation":self.document_generation,
            "revision":self.document.revision(),"measurements":entries,"metadata_error":metadata_error,
            "metadata_preserved":true,"draft":draft,"show_guides":self.measurements.show,
            "edit_error":self.measurement_context().err(),
            "limits":{"measurements":measurements::MAX_MEASUREMENTS,"name_characters":measurements::MAX_NAME_CHARS,
                "coordinate_extent":measurements::MAX_COORDINATE,"pick_viewport_dimension":MAX_PICK_VIEWPORT_SIZE},
            "picking":"Preview a ruler, then arm start/end picking. Rays and viewport pixels use the first visible brush, terrain triangle or supported model mesh; misses preserve endpoints. No grid/plane fallback, snapping or object attachment. Guides never enter picking."})
    }

    fn measurement_plan(
        &self,
        request: Request,
    ) -> Result<(Measurements, Measurement, bool), String> {
        self.measurement_context()?;
        let mut metadata = self.measurements()?;
        let replace = request.id.is_some();
        let id = request.id.map(Ok).unwrap_or_else(|| metadata.next_id())?;
        let mut measurement = Measurement {
            id,
            name: request.name.trim().into(),
            start: request.start,
            end: request.end,
            extra: Vec::new(),
        };
        if replace {
            let existing = metadata
                .measurements
                .iter_mut()
                .find(|entry| entry.id == id)
                .ok_or("Measurement no longer exists")?;
            measurement.extra = existing.extra.clone();
            *existing = measurement.clone();
        } else {
            metadata.measurements.push(measurement.clone());
        }
        metadata.validate()?;
        Ok((metadata, measurement, replace))
    }

    pub fn preview_measurement(
        &mut self,
        actor: Actor,
        request: Request,
        snapshot: &str,
    ) -> Result<Value, String> {
        self.check_measurement_stamp(snapshot)?;
        if self
            .measurements
            .draft
            .as_ref()
            .is_some_and(|draft| draft.owner != actor)
        {
            return Err(
                "Another actor owns this measurement; Review preview before changing it".into(),
            );
        }
        let (_, measurement, replace) = self.measurement_plan(request)?;
        self.measurements.serial += 1;
        self.measurements.draft = Some(Draft {
            token: self.measurements.serial,
            generation: self.document_generation,
            revision: self.document.revision(),
            owner: actor,
            measurement,
            replace,
            picking: None,
            last_pick: None,
        });
        self.measurements.show = true;
        self.scene_serial += 1;
        Ok(self.measurement_state())
    }

    pub fn begin_measurement_pick(
        &mut self,
        actor: Actor,
        token: u64,
        endpoint: Endpoint,
        camera: Camera,
        size: (u32, u32),
        snapshot: &str,
    ) -> Result<Value, String> {
        if snapshot != self.measurement_pick_stamp() {
            return Err("Measurement picking context changed; inspect Measurements again".into());
        }
        let draft = self.checked_measurement(token)?;
        if draft.owner != actor {
            return Err("Review the other actor's measurement before picking an endpoint".into());
        }
        if size.0 == 0
            || size.1 == 0
            || size.0 > MAX_PICK_VIEWPORT_SIZE
            || size.1 > MAX_PICK_VIEWPORT_SIZE
        {
            return Err("Measurement viewport dimensions must be between 1 and 8192".into());
        }
        let (origin, direction) = camera.ray(0.0, 0.0, size.0 as f32 / size.1 as f32);
        if !origin.is_finite() || !direction.is_finite() || direction.length_squared() == 0.0 {
            return Err("Measurement picking requires a finite camera".into());
        }
        if self.get_entity_placement()["draft"]["picking"] == true
            || self.surface_placement()["picking"] == true
            || self.clip_tool
            || self.spatial.draw_block
            || self.component_picking().is_some()
            || self.component_drag().is_some()
            || self.paint_stroke().is_some()
            || self.transform_drag().is_some()
            || self.area_preview().is_some()
            || self.volume_preview().is_some()
            || self.terrain_move_handles_json()["active"] == true
            || self.terrain_stroke_json()["active"] == true
        {
            return Err(
                "Finish the active viewport tool before picking a measurement endpoint".into(),
            );
        }
        let picking = Picking {
            endpoint,
            scene: self.scene_serial,
            view: self.view_serial,
            camera_serial: self.camera_serial,
            session_camera: self.camera,
            camera,
            size,
        };
        self.measurements.serial += 1;
        let draft = self.measurements.draft.as_mut().unwrap();
        draft.token = self.measurements.serial;
        draft.picking = Some(picking);
        Ok(self.measurement_state())
    }

    pub fn cancel_measurement_pick(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        let draft = self
            .measurements
            .draft
            .as_ref()
            .filter(|draft| draft.token == token)
            .ok_or("Measurement preview was closed or replaced")?;
        if draft.owner != actor {
            return Err(
                "Review the other actor's measurement before cancelling endpoint picking".into(),
            );
        }
        if draft.picking.is_none() {
            return Err("Measurement endpoint picking is not active".into());
        }
        self.measurements.serial += 1;
        let draft = self.measurements.draft.as_mut().unwrap();
        draft.token = self.measurements.serial;
        draft.picking = None;
        Ok(self.measurement_state())
    }

    pub fn pick_measurement_ray(
        &mut self,
        actor: Actor,
        token: u64,
        ray: Ray,
    ) -> Result<Value, String> {
        let endpoint = self.checked_measurement_pick(actor, token)?.endpoint;
        let surface = self.placement_surface(&[], ray, true)?;
        let draft = self.measurements.draft.as_ref().unwrap();
        let mut measurement = draft.measurement.clone();
        match endpoint {
            Endpoint::Start => measurement.start = surface.hit.point,
            Endpoint::End => measurement.end = surface.hit.point,
        }
        measurement.validate()?;
        self.measurements.serial += 1;
        let draft = self.measurements.draft.as_mut().unwrap();
        draft.token = self.measurements.serial;
        draft.measurement = measurement;
        draft.picking = None;
        draft.last_pick = Some((endpoint, surface));
        self.scene_serial += 1;
        Ok(self.measurement_state())
    }

    pub fn pick_measurement_pixel(
        &mut self,
        actor: Actor,
        token: u64,
        pointer: Pointer,
    ) -> Result<Value, String> {
        let picking = self.checked_measurement_pick(actor, token)?;
        let pointer = pointer.validate()?;
        if pointer.size != picking.size {
            return Err("Viewport dimensions changed; choose Pick start or Pick end again".into());
        }
        if pointer.position.x < 0.0
            || pointer.position.y < 0.0
            || pointer.position.x >= f64::from(pointer.size.0)
            || pointer.position.y >= f64::from(pointer.size.1)
        {
            return Err("Measurement pixel must lie inside the captured viewport".into());
        }
        let ndc = pointer.ndc();
        let (origin, direction) = picking.camera.ray(
            ndc.x as f32,
            ndc.y as f32,
            pointer.size.0 as f32 / pointer.size.1 as f32,
        );
        self.pick_measurement_ray(
            actor,
            token,
            Ray {
                origin: origin.as_dvec3(),
                direction: direction.as_dvec3(),
            },
        )
    }

    pub fn review_measurement(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        self.checked_measurement(token)?;
        self.measurements.serial += 1;
        let draft = self.measurements.draft.as_mut().unwrap();
        draft.owner = actor;
        draft.token = self.measurements.serial;
        Ok(self.measurement_state())
    }

    pub fn apply_measurement(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        let draft = self.checked_measurement(token)?;
        if draft.owner != actor {
            return Err(
                "Another actor owns this measurement; Review preview before applying it".into(),
            );
        }
        if draft.picking.is_some() {
            return Err(
                "Pick the measurement endpoint or cancel endpoint picking before Apply".into(),
            );
        }
        let id = draft.measurement.id;
        let request = Request {
            id: draft.replace.then_some(id),
            name: draft.measurement.name.clone(),
            start: draft.measurement.start,
            end: draft.measurement.end,
        };
        let (metadata, _, _) = self.measurement_plan(request)?;
        self.edit(actor, "save measurement", |tx| metadata.write(tx))?;
        self.measurements.draft = None;
        self.measurements.serial += 1;
        self.scene_serial += 1;
        Ok(json!({"id":id,"state":self.measurement_state()}))
    }

    pub fn cancel_measurement(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        let draft = self
            .measurements
            .draft
            .as_ref()
            .filter(|draft| draft.token == token)
            .ok_or("Measurement preview was closed or replaced")?;
        if draft.owner != actor {
            return Err(
                "Another actor owns this measurement; Review preview before cancelling it".into(),
            );
        }
        self.measurements.draft = None;
        self.measurements.serial += 1;
        self.scene_serial += 1;
        Ok(self.measurement_state())
    }

    pub fn delete_measurement(
        &mut self,
        actor: Actor,
        id: u32,
        snapshot: &str,
    ) -> Result<Value, String> {
        self.check_measurement_stamp(snapshot)?;
        self.measurement_context()?;
        if self.measurements.draft.is_some() {
            return Err(
                "Apply or cancel the measurement preview before deleting a measurement".into(),
            );
        }
        let mut metadata = self.measurements()?;
        if !metadata
            .measurements
            .iter()
            .any(|measurement| measurement.id == id)
        {
            return Err("Measurement no longer exists".into());
        }
        metadata
            .measurements
            .retain(|measurement| measurement.id != id);
        self.edit(actor, "delete measurement", |tx| metadata.write(tx))?;
        Ok(self.measurement_state())
    }

    pub fn set_measurement_guides(&mut self, show: bool, snapshot: &str) -> Result<Value, String> {
        self.check_measurement_stamp(snapshot)?;
        if show != self.measurements.show {
            self.measurements.show = show;
            self.measurements.serial += 1;
            self.scene_serial += 1;
        }
        Ok(self.measurement_state())
    }

    pub fn frame_measurement(
        &mut self,
        id: Option<u32>,
        token: Option<u64>,
        aspect: f32,
        snapshot: &str,
    ) -> Result<(), String> {
        self.check_measurement_stamp(snapshot)?;
        if !aspect.is_finite() || !(0.01..=100.0).contains(&aspect) {
            return Err("Viewport aspect must be finite and between 0.01 and 100".into());
        }
        let measurement = match (id, token) {
            (Some(id), None) => self
                .measurements()?
                .measurements
                .into_iter()
                .find(|measurement| measurement.id == id)
                .ok_or("Measurement no longer exists")?,
            (None, Some(token)) => self.checked_measurement(token)?.measurement.clone(),
            _ => return Err("Choose one saved measurement or preview token to frame".into()),
        };
        let padding = DVec3::splat(8.0);
        let mins = measurement.start.min(measurement.end) - padding;
        let maxs = measurement.start.max(measurement.end) + padding;
        let camera = self
            .camera
            .frame_bounds(mins.as_vec3(), maxs.as_vec3(), aspect);
        let matrix = camera.view_projection(aspect);
        for bits in 0..8 {
            let corner = DVec3::new(
                if bits & 1 == 0 { mins.x } else { maxs.x },
                if bits & 2 == 0 { mins.y } else { maxs.y },
                if bits & 4 == 0 { mins.z } else { maxs.z },
            );
            let clip = matrix * corner.as_vec3().extend(1.0);
            let ndc = clip.truncate() / clip.w;
            if !ndc.is_finite() || clip.w <= 0.0 || ndc.x.abs() > 1.001 || ndc.y.abs() > 1.001 {
                return Err("Measurement exceeds this camera's framing range; use Perspective or a wider viewport".into());
            }
        }
        self.set_camera(camera);
        Ok(())
    }

    pub fn append_measurement_guides(&self, mesh: &mut render::SceneMesh) {
        if !self.measurements.show {
            return;
        }
        let Ok(mut metadata) = self.measurements() else {
            return;
        };
        if let Some(draft) = &self.measurements.draft
            && self.checked_measurement(draft.token).is_ok()
        {
            metadata
                .measurements
                .retain(|measurement| measurement.id != draft.measurement.id);
            metadata.measurements.push(draft.measurement.clone());
        }
        for measurement in &metadata.measurements {
            let start = measurement.start;
            let end = measurement.end;
            let x = DVec3::new(end.x, start.y, start.z);
            let xy = DVec3::new(end.x, end.y, start.z);
            for (a, b, color) in [
                (start, x, [235, 110, 110, 230]),
                (x, xy, [120, 215, 125, 230]),
                (xy, end, [115, 165, 245, 230]),
                (start, end, [255, 210, 100, 255]),
            ] {
                append_line(mesh, a, b, color);
            }
            for point in [start, end] {
                for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
                    append_line(
                        mesh,
                        point - axis * 3.0,
                        point + axis * 3.0,
                        [255, 230, 170, 255],
                    );
                }
            }
        }
    }
}

fn append_line(mesh: &mut render::SceneMesh, start: DVec3, end: DVec3, color: [u8; 4]) {
    for point in [start, end] {
        mesh.lines.push(render::scene::LineVertex {
            position: point.as_vec3().to_array(),
            color,
        });
    }
}

#[cfg(test)]
mod tests;
