use glam::DVec3;
use map_core::ObjectId;
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

impl From<Endpoint> for measurements::Endpoint {
    fn from(endpoint: Endpoint) -> Self {
        match endpoint {
            Endpoint::Start => Self::Start,
            Endpoint::End => Self::End,
        }
    }
}

#[derive(
    Clone, Copy, Debug, Default, serde::Deserialize, serde::Serialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum PickMode {
    #[default]
    Surface,
    Plane,
}

const MAX_PICK_VIEWPORT_SIZE: u32 = super::viewport_pick::MAX_DIMENSION;

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
    mode: PickMode,
    pub(super) context: super::viewport_pick::Context,
}

fn measurement_json(measurement: &Measurement, resolver: &mut measurements::Resolver<'_>) -> Value {
    let start = resolver.endpoint(measurement, measurements::Endpoint::Start);
    let end = resolver.endpoint(measurement, measurements::Endpoint::End);
    let resolved = resolver.measurement(measurement);
    let attachment = |endpoint, point: &Result<DVec3, String>| {
        if let Ok(Some(reference)) = measurement.surface_reference(endpoint) {
            let mut report = match &reference.target {
                measurements::SurfaceTarget::Brush { vertex_sides } => {
                    json!({"kind":"brush_surface","vertex_sides":vertex_sides,"weights":reference.weights})
                }
                measurements::SurfaceTarget::Terrain {
                    triangle,
                    grid_size,
                    indices,
                    cage_corners,
                } => {
                    json!({"kind":"terrain_surface","triangle":triangle,"grid_size":grid_size,"indices":indices,"cage_corners":cage_corners,"weights":reference.weights})
                }
            };
            report["object"] = json!(crate::identity::id_string(reference.owner));
            report["solid"] = json!(reference.face.solid);
            report["side"] = json!(reference.face.side);
            report["available"] = json!(point.is_ok());
            report["error"] = json!(point.as_ref().err());
            return Some(report);
        }
        measurement
            .origin_reference(endpoint)
            .ok()
            .flatten()
            .map(|id| {
                match measurement.local_offset(endpoint).ok().flatten() {
                    Some(offset) => json!({"kind":"entity_local","object":format!("entity:{id}"),
                        "local_offset":offset.to_array(),"available":point.is_ok(),"error":point.as_ref().err()}),
                    None => json!({"kind":"entity_origin","object":format!("entity:{id}"),
                        "available":point.is_ok(),"error":point.as_ref().err()}),
                }
            })
    };
    let attachments = json!({"start":attachment(measurements::Endpoint::Start, &start),"end":attachment(measurements::Endpoint::End, &end)});
    json!({"id":measurement.id,"name":measurement.name,
        "start":start.ok().map(|point|point.to_array()),"end":end.ok().map(|point|point.to_array()),
        "stored_start":measurement.start.to_array(),"stored_end":measurement.end.to_array(),
        "attachments":attachments,
        "available":resolved.is_ok(),"error":resolved.as_ref().err(),
        "distance":resolved.as_ref().ok().map(Measurement::distance),
        "components":resolved.as_ref().ok().map(|measurement|measurement.components().to_array())})
}

impl Session {
    pub fn measurement_picking(&self) -> bool {
        self.measurements
            .draft
            .as_ref()
            .is_some_and(|draft| draft.picking.is_some())
    }

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
        if !picking.context.is_current(self) {
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
        let mut resolver = measurements::Resolver::new(self.document.vmf());
        let (entries, metadata_error) = match self.measurements() {
            Ok(metadata) => (
                metadata
                    .measurements
                    .iter()
                    .map(|measurement| measurement_json(measurement, &mut resolver))
                    .collect::<Vec<_>>(),
                None,
            ),
            Err(error) => (Vec::new(), Some(error)),
        };
        let draft = self.measurements.draft.as_ref().map(|draft| {
            let picking = draft.picking.as_ref().map(|picking| json!({
                "endpoint":picking.endpoint,"mode":picking.mode,"width":picking.context.size.0,"height":picking.context.size.1,
                "valid":self.checked_measurement_pick(draft.owner,draft.token).is_ok(),
                "error":self.checked_measurement_pick(draft.owner,draft.token).err()}));
            let last_pick = draft.last_pick.map(|(endpoint, surface)| json!({"endpoint":endpoint,
                "target":crate::identity::id_string(surface.hit.object),"solid":surface.hit.solid,
                "side":surface.hit.side,"point":surface.hit.point.to_array(),"normal":surface.normal.to_array()}));
            json!({"token":draft.token,"owner":draft.owner.label(),"replace":draft.replace,
                "measurement":measurement_json(&draft.measurement, &mut resolver),
                "picking":picking,"last_pick":last_pick,
                "valid":self.checked_measurement(draft.token).is_ok(),
                "error":self.checked_measurement(draft.token).err()})
        });
        json!({"snapshot":self.measurement_stamp(),"pick_snapshot":self.measurement_pick_stamp(),"generation":self.document_generation,
            "revision":self.document.revision(),"measurements":entries,"metadata_error":metadata_error,
            "metadata_preserved":true,"draft":draft,"show_guides":self.measurements.show,
            "coordinate_source":"committed_document",
            "edit_error":self.measurement_context().err(),
            "limits":{"measurements":measurements::MAX_MEASUREMENTS,"name_characters":measurements::MAX_NAME_CHARS,
                "coordinate_extent":measurements::MAX_COORDINATE,"pick_viewport_dimension":MAX_PICK_VIEWPORT_SIZE},
            "attachments":"Attach origin/local follows point-entity translation or rigid orientation; model scale, parenting and animated mesh anchors are unsupported. Attach surface preserves the last picked authored brush/terrain point using stable face-vertex identities and triangle weights. Brush geometry must retain the selected ordinary face and its three incident-side vertex keys; terrain must retain its prepared triangle, grid size and ordered quad-cage identities. Compatible moves/transforms/sculpting resolve current geometry; removed, ambiguous, saved-hidden or incompatible targets are unavailable without stored-world fallback. Rulers persist through VMF save/reopen and Undo. Detach freezes only a resolvable point. Explicit world_endpoint replaces any attachment, including an unavailable one. Reports use committed coordinates; guides follow the render document during geometry previews.",
            "picking":"Surface mode picks the first visible brush, terrain triangle or supported model mesh at its actual point. Surface picks stay world points until explicit Attach surface; model hits cannot become durable surface anchors. Plane mode explicitly intersects the configured workplane and uses its snapping setting; parallel/backward rays fail without fallback. Misses and stale picks preserve endpoints. Guides never enter picking."})
    }

    fn measurement_plan(
        &self,
        request: Request,
    ) -> Result<(Measurements, Measurement, bool), String> {
        self.measurement_context()?;
        let mut metadata = self.measurements()?;
        let replace = request.id.is_some();
        let id = request.id.map(Ok).unwrap_or_else(|| metadata.next_id())?;
        let existing = self
            .measurements
            .draft
            .as_ref()
            .filter(|draft| draft.measurement.id == id && draft.replace == replace)
            .map(|draft| &draft.measurement)
            .or_else(|| metadata.measurements.iter().find(|entry| entry.id == id));
        let mut measurement = Measurement {
            id,
            name: request.name.trim().into(),
            start: request.start,
            end: request.end,
            extra: existing
                .map(|measurement| measurement.extra.clone())
                .unwrap_or_default(),
        };
        if let Some(existing) = existing {
            for endpoint in [measurements::Endpoint::Start, measurements::Endpoint::End] {
                if existing.attached(endpoint)? {
                    let point = existing
                        .resolve_endpoint(self.document.vmf(), endpoint)
                        .unwrap_or_else(|_| existing.stored_endpoint(endpoint));
                    if measurement.stored_endpoint(endpoint) != point {
                        return Err(format!(
                            "The {} endpoint is attached; Detach or explicitly replace it with a world endpoint before editing coordinates",
                            endpoint.label()
                        ));
                    }
                    measurement.set_stored_endpoint(endpoint, existing.stored_endpoint(endpoint));
                }
            }
        }
        if replace {
            let existing = metadata
                .measurements
                .iter_mut()
                .find(|entry| entry.id == id)
                .ok_or("Measurement no longer exists")?;
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
        self.begin_measurement_pick_mode(
            actor,
            token,
            endpoint,
            PickMode::Surface,
            (camera, size),
            snapshot,
        )
    }

    pub fn begin_measurement_pick_mode(
        &mut self,
        actor: Actor,
        token: u64,
        endpoint: Endpoint,
        mode: PickMode,
        view: (Camera, (u32, u32)),
        snapshot: &str,
    ) -> Result<Value, String> {
        let (camera, size) = view;
        if snapshot != self.measurement_pick_stamp() {
            return Err("Measurement picking context changed; inspect Measurements again".into());
        }
        let draft = self.checked_measurement(token)?;
        if draft.owner != actor {
            return Err("Review the other actor's measurement before picking an endpoint".into());
        }
        if draft.measurement.attached(endpoint.into())? {
            return Err(
                "Detach the attached endpoint or explicitly use a world endpoint before picking"
                    .into(),
            );
        }
        let context = super::viewport_pick::Context::capture(self, camera, size, "Measurement")?;
        if self.viewport_pick_conflict() {
            return Err(
                "Finish the active viewport tool before picking a measurement endpoint".into(),
            );
        }
        let picking = Picking {
            endpoint,
            mode,
            context,
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
        let picking = self.checked_measurement_pick(actor, token)?;
        let endpoint = picking.endpoint;
        let (point, surface) = match picking.mode {
            PickMode::Surface => {
                let surface = self.placement_surface(&[], ray, true)?;
                (surface.hit.point, Some(surface))
            }
            PickMode::Plane => (
                DVec3::from_array(self.workplane_intersection(ray, None)?.point),
                None,
            ),
        };
        let draft = self.measurements.draft.as_ref().unwrap();
        let mut measurement = draft.measurement.clone();
        match endpoint {
            Endpoint::Start => measurement.start = point,
            Endpoint::End => measurement.end = point,
        }
        measurement.validate_available_endpoints(self.document.vmf())?;
        self.measurements.serial += 1;
        let draft = self.measurements.draft.as_mut().unwrap();
        draft.token = self.measurements.serial;
        draft.measurement = measurement;
        draft.picking = None;
        draft.last_pick = surface.map(|surface| (endpoint, surface));
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
        let ray = picking.context.ray(pointer)?;
        self.pick_measurement_ray(actor, token, ray)
    }

    pub fn review_measurement(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        self.checked_measurement(token)?;
        self.measurements.serial += 1;
        let draft = self.measurements.draft.as_mut().unwrap();
        draft.owner = actor;
        draft.token = self.measurements.serial;
        Ok(self.measurement_state())
    }

    fn editable_measurement(&self, actor: Actor, token: u64) -> Result<Measurement, String> {
        let draft = self.checked_measurement(token)?;
        if draft.owner != actor {
            return Err(
                "Review the other actor's measurement before changing endpoint attachments".into(),
            );
        }
        if draft.picking.is_some() {
            return Err("Finish or cancel endpoint picking before changing attachments".into());
        }
        Ok(draft.measurement.clone())
    }

    fn replace_measurement_endpoint(&mut self, measurement: Measurement) -> Result<Value, String> {
        measurement.validate_available_endpoints(self.document.vmf())?;
        self.measurements.serial += 1;
        let draft = self.measurements.draft.as_mut().unwrap();
        draft.token = self.measurements.serial;
        draft.measurement = measurement;
        draft.last_pick = None;
        self.scene_serial += 1;
        Ok(self.measurement_state())
    }

    pub fn attach_measurement_origin(
        &mut self,
        actor: Actor,
        token: u64,
        endpoint: Endpoint,
        object: ObjectId,
    ) -> Result<Value, String> {
        let mut measurement = self.editable_measurement(actor, token)?;
        let point = measurements::entity_origin(self.document.vmf(), object)?;
        let ObjectId::Entity(id) = object else {
            unreachable!()
        };
        measurement.set_origin_reference(endpoint.into(), Some(id));
        measurement.set_stored_endpoint(endpoint.into(), point);
        self.replace_measurement_endpoint(measurement)
    }

    pub fn attach_measurement_surface(
        &mut self,
        actor: Actor,
        token: u64,
        endpoint: Endpoint,
    ) -> Result<Value, String> {
        let mut measurement = self.editable_measurement(actor, token)?;
        let (picked_endpoint, surface) = self
            .measurements
            .draft
            .as_ref()
            .unwrap()
            .last_pick
            .ok_or("Pick a brush or terrain surface point before attaching it")?;
        if !matches!(
            (picked_endpoint, endpoint),
            (Endpoint::Start, Endpoint::Start) | (Endpoint::End, Endpoint::End)
        ) {
            return Err(
                "Pick this endpoint on a brush or terrain surface before attaching it".into(),
            );
        }
        let reference = measurements::SurfaceReference::capture(&self.document, surface)?;
        measurement.set_surface_reference(endpoint.into(), reference)?;
        measurement.set_stored_endpoint(endpoint.into(), surface.hit.point);
        self.replace_measurement_endpoint(measurement)
    }

    pub fn attach_measurement_local(
        &mut self,
        actor: Actor,
        token: u64,
        endpoint: Endpoint,
        object: ObjectId,
    ) -> Result<Value, String> {
        let mut measurement = self.editable_measurement(actor, token)?;
        let point = measurement.resolve_endpoint(self.document.vmf(), endpoint.into())?;
        let (origin, basis) = measurements::entity_local_pose(self.document.vmf(), object)?;
        let ObjectId::Entity(id) = object else {
            unreachable!()
        };
        let offset = basis.transpose() * (point - origin);
        measurement.set_local_reference(endpoint.into(), id, offset)?;
        measurement.set_stored_endpoint(endpoint.into(), point);
        self.replace_measurement_endpoint(measurement)
    }

    pub fn detach_measurement_origin(
        &mut self,
        actor: Actor,
        token: u64,
        endpoint: Endpoint,
    ) -> Result<Value, String> {
        let mut measurement = self.editable_measurement(actor, token)?;
        if !measurement.attached(endpoint.into())? {
            return Err("This measurement endpoint is already a world point".into());
        }
        let point = measurement.resolve_endpoint(self.document.vmf(), endpoint.into())?;
        measurement.set_origin_reference(endpoint.into(), None);
        measurement.set_stored_endpoint(endpoint.into(), point);
        self.replace_measurement_endpoint(measurement)
    }

    pub fn set_measurement_world_endpoint(
        &mut self,
        actor: Actor,
        token: u64,
        endpoint: Endpoint,
        point: DVec3,
    ) -> Result<Value, String> {
        let mut measurement = self.editable_measurement(actor, token)?;
        measurement.set_origin_reference(endpoint.into(), None);
        measurement.set_stored_endpoint(endpoint.into(), point);
        self.replace_measurement_endpoint(measurement)
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
        draft.measurement.resolved(self.document.vmf())?;
        let mut metadata = self.measurements()?;
        if draft.replace {
            let existing = metadata
                .measurements
                .iter_mut()
                .find(|measurement| measurement.id == id)
                .ok_or("Measurement no longer exists")?;
            *existing = draft.measurement.clone();
        } else {
            metadata.measurements.push(draft.measurement.clone());
        }
        metadata.validate()?;
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
        let measurement = measurement.resolved(self.document.vmf())?;
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
        self.try_set_camera(camera)?;
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
        let mut resolver = measurements::Resolver::new(self.render_document().vmf());
        for measurement in &metadata.measurements {
            let Ok(measurement) = resolver.measurement(measurement) else {
                continue;
            };
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
                mesh.add_guide_line(a, b, color);
            }
            for point in [start, end] {
                for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
                    mesh.add_guide_line(
                        point - axis * 3.0,
                        point + axis * 3.0,
                        [255, 230, 170, 255],
                    );
                }
            }
        }
    }
}

impl State {
    pub(super) fn close_draft(&self) -> Option<serde_json::Value> {
        self.draft.as_ref().map(|draft| {
            serde_json::json!([
                self.serial,
                draft.token,
                draft.owner.label(),
                draft.picking.is_some()
            ])
        })
    }
}
