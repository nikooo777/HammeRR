use map_core::picking::Picker;
use map_core::region::Selection;
use serde_json::{Value, json};

use super::*;

pub const SAMPLE_SPACING: u32 = 2;
pub const MAX_UPDATE_SAMPLES: usize = 32_768;
pub const MAX_UPDATE_VISITS: usize = 1_048_576;
pub const MAX_STROKE_SAMPLES: usize = 262_144;
pub const MAX_MATCHES: usize = 16_384;
pub const MAX_POINTS: usize = 256;
pub const MAX_VIEWPORT: u32 = 16_384;
pub const MIN_RADIUS: f64 = 2.0;
pub const MAX_RADIUS: f64 = 64.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Operation {
    #[default]
    Selection,
    Material,
    Projection,
}

impl Operation {
    pub fn name(self) -> &'static str {
        match self {
            Self::Selection => "selection",
            Self::Material => "material",
            Self::Projection => "projection",
        }
    }
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "selection" => Ok(Self::Selection),
            "material" => Ok(Self::Material),
            "projection" => Ok(Self::Projection),
            _ => Err("Paint operation must be selection, material or projection".into()),
        }
    }
    fn texture_mode(self) -> Option<surface::PaintMode> {
        match self {
            Self::Selection => None,
            Self::Material => Some(surface::PaintMode::Material),
            Self::Projection => Some(surface::PaintMode::Projection),
        }
    }
}

pub struct TextureDraft {
    pub sample: surface::SurfaceSample,
    pub document: Document,
    pub changed: usize,
    edits: Vec<(FaceId, SurfaceEdit)>,
}

#[derive(Clone, Copy)]
pub struct Spec {
    pub camera: Camera,
    pub pointer: Pointer,
    pub radius: f64,
    pub mode: SelectMode,
    pub faces: bool,
}

pub struct Stroke {
    pub token: u64,
    pub spec: Spec,
    pub operation: Operation,
    pub texture: Option<TextureDraft>,
    pub cursor: DVec2,
    pub objects: BTreeSet<ObjectId>,
    pub faces: BTreeSet<FaceId>,
    samples: HashSet<(u32, u32)>,
    picker: Picker,
    generation: u64,
    revision: u64,
    shared_camera: Camera,
    groups: Option<super::groups::SelectionIndex>,
}

pub struct State {
    pub enabled: bool,
    pub radius: f64,
    pub operation: Operation,
    serial: u64,
    pub(super) stroke: Option<Stroke>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            enabled: false,
            radius: 12.0,
            operation: Operation::Selection,
            serial: 0,
            stroke: None,
        }
    }
}

fn validate_radius(radius: f64) -> Result<(), String> {
    if !radius.is_finite() || !(MIN_RADIUS..=MAX_RADIUS).contains(&radius) {
        return Err(format!(
            "Paint radius must be {MIN_RADIUS}–{MAX_RADIUS} screen pixels"
        ));
    }
    Ok(())
}

fn validate_pointer(pointer: Pointer) -> Result<(), String> {
    pointer.validate()?;
    if pointer.size.0 > MAX_VIEWPORT
        || pointer.size.1 > MAX_VIEWPORT
        || pointer.position.abs().max_element() > f64::from(MAX_VIEWPORT) * 4.0
    {
        return Err(format!(
            "Paint viewport must be at most {MAX_VIEWPORT} pixels per side with nearby pointer coordinates"
        ));
    }
    Ok(())
}

fn capsule_samples(
    start: DVec2,
    end: DVec2,
    size: (u32, u32),
    radius: f64,
    existing: &HashSet<(u32, u32)>,
    pending: &mut BTreeSet<(u32, u32)>,
    visits: &mut usize,
) -> Result<(), String> {
    let delta = end - start;
    let major = usize::from(delta.y.abs() > delta.x.abs());
    let minor = 1 - major;
    let extent = [size.0, size.1];
    let step = f64::from(SAMPLE_SPACING);
    let low = ((start[major].min(end[major]) - radius - 0.5) / step)
        .ceil()
        .max(0.0) as u32;
    let high = ((start[major].max(end[major]) + radius - 0.5) / step)
        .floor()
        .min(f64::from((extent[major] - 1) / SAMPLE_SPACING)) as i64;
    if high < i64::from(low) {
        return Ok(());
    }
    let padding = if delta[major].abs() < 1e-9 {
        radius
    } else {
        radius * (1.0 + (delta[minor] / delta[major]).abs())
    };
    for a in low..=high as u32 {
        let coordinate = f64::from(a) * step + 0.5;
        let t = if delta[major].abs() < 1e-9 {
            0.0
        } else {
            ((coordinate - start[major]) / delta[major]).clamp(0.0, 1.0)
        };
        let center = start[minor] + delta[minor] * t;
        let lo = ((center - padding - 0.5) / step).ceil().max(0.0) as u32;
        let hi = ((center + padding - 0.5) / step)
            .floor()
            .min(f64::from((extent[minor] - 1) / SAMPLE_SPACING)) as i64;
        if hi < i64::from(lo) {
            continue;
        }
        for b in lo..=hi as u32 {
            *visits += 1;
            if *visits > MAX_UPDATE_VISITS {
                return Err(
                    "Paint update traverses too many pixels. Send fewer or shorter segments."
                        .into(),
                );
            }
            let sample = if major == 0 { (a, b) } else { (b, a) };
            if existing.contains(&sample) || pending.contains(&sample) {
                continue;
            }
            let point = DVec2::new(
                f64::from(sample.0) * step + 0.5,
                f64::from(sample.1) * step + 0.5,
            );
            let t = if delta.length_squared() < 1e-12 {
                0.0
            } else {
                ((point - start).dot(delta) / delta.length_squared()).clamp(0.0, 1.0)
            };
            if point.distance_squared(start + delta * t) > radius * radius + 1e-8 {
                continue;
            }
            if pending.len() >= MAX_UPDATE_SAMPLES
                || existing.len() + pending.len() >= MAX_STROKE_SAMPLES
            {
                return Err("Paint stroke sample limit reached. Apply or cancel this stroke, then start another; use shorter updates for long paths.".into());
            }
            pending.insert(sample);
        }
    }
    Ok(())
}

impl Session {
    pub fn paint_enabled(&self) -> bool {
        self.paint.enabled
    }
    pub fn paint_operation(&self) -> Operation {
        self.paint.operation
    }

    pub fn paint_radius(&self) -> f64 {
        self.paint.radius
    }
    pub fn paint_stroke(&self) -> Option<&Stroke> {
        self.paint.stroke.as_ref()
    }

    pub fn set_paint_tool(
        &mut self,
        actor: Actor,
        enabled: bool,
        radius: f64,
    ) -> Result<(), String> {
        self.set_paint_operation(actor, enabled, radius, Operation::Selection)
    }

    pub fn set_paint_operation(
        &mut self,
        actor: Actor,
        enabled: bool,
        radius: f64,
        operation: Operation,
    ) -> Result<(), String> {
        validate_radius(radius)?;
        if self.paint.enabled == enabled
            && self.paint.radius == radius
            && self.paint.operation == operation
        {
            return Ok(());
        }
        self.cancel_area_selection();
        if enabled {
            self.cancel_block_preview();
            self.cancel_face_preview();
            self.cancel_clip_preview();
            self.clear_hollow_preview();
            self.clear_paste_preview();
            self.clear_repeat_preview();
            self.clear_carve_preview();
            self.clear_merge_preview();
            self.clear_component_edit();
            self.clear_primitive_preview();
            self.clear_volume_selection();
            self.cancel_transform_preview();
            self.transform_tool.mode = None;
            self.clip_tool = false;
            self.spatial.draw_block = false;
        }
        self.paint.enabled = enabled;
        self.paint.radius = radius;
        self.paint.operation = operation;
        self.view_serial += 1;
        self.log(
            actor,
            if enabled {
                "paint brush enabled"
            } else {
                "paint brush disabled"
            },
        );
        Ok(())
    }

    pub fn clear_paint_stroke(&mut self) {
        if self.paint.stroke.take().is_some() {
            self.scene_serial += 1;
            self.view_serial += 1;
        }
    }

    fn checked_paint_stroke(&self, token: u64) -> Result<&Stroke, String> {
        let stroke = self
            .paint_stroke()
            .filter(|stroke| stroke.token == token)
            .ok_or("Paint stroke was closed or replaced")?;
        if stroke.generation != self.document_generation
            || stroke.revision != self.document.revision()
            || stroke.shared_camera != self.camera
        {
            return Err("Map or camera changed; start a new paint stroke".into());
        }
        Ok(stroke)
    }

    pub fn begin_paint_selection(&mut self, spec: Spec) -> Result<u64, String> {
        self.begin_stroke(spec, Operation::Selection)
    }

    pub fn begin_texture_stroke(
        &mut self,
        spec: Spec,
        operation: Operation,
    ) -> Result<u64, String> {
        if operation.texture_mode().is_none() {
            return Err("Texture strokes require material or projection mode".into());
        }
        self.begin_stroke(
            Spec {
                faces: true,
                mode: SelectMode::Replace,
                ..spec
            },
            operation,
        )
    }

    fn begin_stroke(&mut self, spec: Spec, operation: Operation) -> Result<u64, String> {
        validate_pointer(spec.pointer)?;
        validate_radius(spec.radius)?;
        if !spec.camera.position.is_finite()
            || !spec.camera.yaw_degrees.is_finite()
            || !spec.camera.pitch_degrees.is_finite()
            || !(-90.0..=90.0).contains(&spec.camera.pitch_degrees)
            || !spec.camera.fov_y_degrees.is_finite()
            || !(1.0..179.0).contains(&spec.camera.fov_y_degrees)
            || !spec.camera.ortho_height.is_finite()
            || !(Camera::MIN_ORTHO_HEIGHT..=Camera::MAX_ORTHO_HEIGHT)
                .contains(&spec.camera.ortho_height)
        {
            return Err(
                "Paint camera must have finite coordinates and valid projection settings".into(),
            );
        }

        if spec.pointer.position.min_element() < 0.0
            || spec.pointer.position.x >= f64::from(spec.pointer.size.0)
            || spec.pointer.position.y >= f64::from(spec.pointer.size.1)
        {
            return Err("Start a paint stroke inside the viewport".into());
        }
        let hidden = self.hidden_tool_brushes();
        let picker = Picker::new(
            &self.document,
            |solid| !hidden.contains(&solid.id),
            !spec.faces,
        );
        let token = self.paint.serial + 1;
        let texture = if operation.texture_mode().is_some() {
            Some(TextureDraft {
                sample: self
                    .surface_sample
                    .clone()
                    .ok_or("Select one source face and sample its texture first")?,
                document: self.document.preview_snapshot(),
                changed: 0,
                edits: Vec::new(),
            })
        } else {
            None
        };
        let mut stroke = Stroke {
            token,
            spec,
            operation,
            texture,
            cursor: spec.pointer.position,
            objects: BTreeSet::new(),
            faces: BTreeSet::new(),
            samples: HashSet::new(),
            picker,
            generation: self.document_generation,
            revision: self.document.revision(),
            shared_camera: self.camera,
            groups: if spec.faces {
                None
            } else {
                self.group_selection_index()
            },
        };
        update_stroke(&mut stroke, &self.document, &[spec.pointer.position])?;
        self.cancel_area_selection();
        self.cancel_block_preview();
        self.cancel_face_preview();
        self.cancel_clip_preview();
        self.clear_hollow_preview();
        self.clear_paste_preview();
        self.clear_repeat_preview();
        self.clear_carve_preview();
        self.clear_merge_preview();
        self.clear_component_edit();
        self.clear_primitive_preview();
        self.clear_volume_selection();
        self.cancel_transform_preview();
        self.clip_tool = false;
        self.spatial.draw_block = false;
        self.paint.enabled = true;
        self.paint.radius = spec.radius;
        self.paint.operation = operation;
        self.transform_tool.mode = None;
        self.paint.serial = token;
        self.paint.stroke = Some(stroke);
        self.scene_serial += 1;
        self.view_serial += 1;
        Ok(token)
    }

    pub fn update_paint_selection(&mut self, token: u64, points: &[DVec2]) -> Result<(), String> {
        let stroke = self.checked_paint_stroke(token)?;
        if stroke.operation != Operation::Selection {
            return Err("Use update_texture_stroke for a texture stroke".into());
        }
        self.update_active_stroke(token, points)
    }

    pub fn update_texture_stroke(&mut self, token: u64, points: &[DVec2]) -> Result<(), String> {
        if self.checked_paint_stroke(token)?.texture.is_none() {
            return Err("This is a selection stroke".into());
        }
        self.update_active_stroke(token, points)
    }

    pub fn update_active_stroke(&mut self, token: u64, points: &[DVec2]) -> Result<(), String> {
        self.checked_paint_stroke(token)?;
        let changed = update_stroke(self.paint.stroke.as_mut().unwrap(), &self.document, points)?;
        if changed {
            self.scene_serial += 1;
        }
        self.view_serial += 1;
        Ok(())
    }

    pub fn commit_texture_stroke(&mut self, actor: Actor, token: u64) -> Result<usize, String> {
        if self.checked_paint_stroke(token)?.texture.is_none() {
            return Err("This is a selection stroke".into());
        }
        let texture = self.paint.stroke.take().unwrap().texture.unwrap();
        self.scene_serial += 1;
        self.view_serial += 1;
        if texture.changed == 0 {
            return Ok(0);
        }
        self.apply_face_edits(actor, texture.edits)
    }

    pub fn commit_active_stroke(&mut self, actor: Actor, token: u64) -> Result<(), String> {
        if self.checked_paint_stroke(token)?.texture.is_some() {
            self.commit_texture_stroke(actor, token)?;
        } else {
            self.commit_paint_selection(actor, token)?;
        }
        Ok(())
    }

    pub fn commit_paint_selection(
        &mut self,
        actor: Actor,
        token: u64,
    ) -> Result<Selection, String> {
        if self.checked_paint_stroke(token)?.operation != Operation::Selection {
            return Err("Use commit_texture_stroke for a texture stroke".into());
        }
        let stroke = self.paint.stroke.take().unwrap();
        let matches = Selection {
            objects: stroke.objects.into_iter().collect(),
            faces: stroke.faces.into_iter().collect(),
        };
        self.scene_serial += 1;
        self.view_serial += 1;
        if stroke.spec.faces {
            self.select_faces(&matches.faces, stroke.spec.mode)?;
        } else {
            self.select_grouped(&matches.objects, stroke.spec.mode)?;
        }
        self.log(
            actor,
            format!(
                "paint selection: {} matches",
                matches.objects.len() + matches.faces.len()
            ),
        );
        Ok(matches)
    }

    pub fn cancel_texture_stroke(&mut self, token: u64) -> Result<(), String> {
        if self.checked_paint_stroke(token)?.texture.is_none() {
            return Err("This is a selection stroke".into());
        }
        self.cancel_paint_selection(token)
    }

    pub fn cancel_paint_selection(&mut self, token: u64) -> Result<(), String> {
        self.checked_paint_stroke(token)?;
        self.clear_paint_stroke();
        Ok(())
    }

    pub fn paint_selection_json(&self, offset: usize, limit: usize) -> Value {
        let preview = self.paint_stroke().map(|stroke| json!({
            "operation":stroke.operation.name(), "texture":stroke.texture.as_ref().map(|texture|json!({"material":texture.sample.material,"source":{"solid":texture.sample.face.solid,"side":texture.sample.face.side},"faces_changed":texture.changed})),
            "token":stroke.token.to_string(), "radius":stroke.spec.radius,"viewport":stroke.spec.pointer.size,"cursor":stroke.cursor.to_array(),
            "selection_mode":if stroke.spec.faces {"faces"} else {"objects"},
            "mode":match stroke.spec.mode {SelectMode::Replace=>"replace",SelectMode::Add=>"add",SelectMode::Remove=>"remove",SelectMode::Toggle=>"toggle"},
            "camera":{"projection":stroke.spec.camera.projection.name(),"position":stroke.spec.camera.position.to_array(),"yaw":stroke.spec.camera.yaw_degrees,"pitch":stroke.spec.camera.pitch_degrees,"fov":stroke.spec.camera.fov_y_degrees,"ortho_height":stroke.spec.camera.ortho_height},
            "generation":stroke.generation,"revision":stroke.revision,"sample_count":stroke.samples.len(),
            "object_count":stroke.objects.len(),"face_count":stroke.faces.len(),"offset":offset,
            "objects":stroke.objects.iter().skip(offset).take(limit.clamp(1,500)).map(|id|match id {ObjectId::Solid(id)=>format!("solid:{id}"),ObjectId::Entity(id)=>format!("entity:{id}")}).collect::<Vec<_>>(),
            "faces":stroke.faces.iter().skip(offset).take(limit.clamp(1,500)).map(|face|json!({"solid":face.solid,"side":face.side})).collect::<Vec<_>>()
        }));
        json!({"enabled":self.paint.enabled,"sample_material":self.surface_sample.as_ref().map(|sample|&sample.material),"operation":self.paint.operation.name(),"radius":self.paint.radius,"preview":preview,
            "limits":{"min_radius":MIN_RADIUS,"max_radius":MAX_RADIUS,"max_viewport_side":MAX_VIEWPORT,"points_per_update":MAX_POINTS,"new_samples_per_update":MAX_UPDATE_SAMPLES,"pixel_visits_per_update":MAX_UPDATE_VISITS,"samples_per_stroke":MAX_STROKE_SAMPLES,"matches":MAX_MATCHES},
            "sample_spacing":SAMPLE_SPACING,"visibility":"Nearest visible hit per sample; hidden objects/brushes and hidden tool brushes are skipped. Faces ignore point helpers; terrain uses actual displaced sheets, excluding ordinary parent faces/volumes."})
    }
}

fn update_stroke(
    stroke: &mut Stroke,
    document: &Document,
    points: &[DVec2],
) -> Result<bool, String> {
    if points.is_empty() || points.len() > MAX_POINTS {
        return Err(format!("Supply 1–{MAX_POINTS} pointer samples"));
    }
    let mut pending = BTreeSet::new();
    let mut visits = 0;
    let mut previous = stroke.cursor;
    for &point in points {
        validate_pointer(Pointer {
            position: point,
            ..stroke.spec.pointer
        })?;
        capsule_samples(
            previous,
            point,
            stroke.spec.pointer.size,
            stroke.spec.radius,
            &stroke.samples,
            &mut pending,
            &mut visits,
        )?;
        previous = point;
    }
    let mut objects = stroke.objects.clone();
    let mut faces = stroke.faces.clone();
    for &(x, y) in &pending {
        let pointer = Pointer {
            position: DVec2::new(
                f64::from(x * SAMPLE_SPACING) + 0.5,
                f64::from(y * SAMPLE_SPACING) + 0.5,
            ),
            ..stroke.spec.pointer
        };
        let ndc = pointer.ndc();
        let (origin, direction) = stroke.spec.camera.ray(
            ndc.x as f32,
            ndc.y as f32,
            pointer.size.0 as f32 / pointer.size.1 as f32,
        );
        if let Some(hit) = stroke.picker.pick(&Ray {
            origin: origin.as_dvec3(),
            direction: direction.as_dvec3(),
        }) {
            if stroke.spec.faces {
                if let (Some(solid), Some(side)) = (hit.solid, hit.side) {
                    faces.insert(FaceId { solid, side });
                }
            } else {
                if let Some(groups) = &stroke.groups {
                    objects.extend(groups.members(hit.object));
                } else {
                    objects.insert(hit.object);
                }
            }
        }
        if objects.len() + faces.len() > MAX_MATCHES {
            return Err(format!(
                "Paint stroke supports at most {MAX_MATCHES} matches. Apply or cancel it, then start another."
            ));
        }
    }
    let matches_changed = stroke.objects != objects || stroke.faces != faces;
    if matches_changed && let Some(texture) = &mut stroke.texture {
        let new_faces = faces.difference(&stroke.faces).copied().collect::<Vec<_>>();
        let mode = stroke.operation.texture_mode().unwrap();
        let edits = new_faces
            .iter()
            .map(|face| {
                surface::paint_edit(document, *face, &texture.sample, mode)
                    .map(|edit| (*face, edit))
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        let mut preview = texture.document.preview_snapshot();
        let changed = preview
            .edit("preview texture stroke", |tx| {
                edits.iter().try_fold(0, |count, (face, patch)| {
                    surface::edit_faces(tx, &[*face], patch).map(|changed| count + changed)
                })
            })
            .map_err(|error| error.to_string())?;
        texture.document = preview;
        texture.edits.extend(edits);
        texture.changed += changed;
    }
    stroke.objects = objects;
    stroke.faces = faces;
    stroke.samples.extend(pending);
    stroke.cursor = previous;
    Ok(matches_changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;
    use map_core::edit;

    fn screen(camera: Camera, point: DVec3) -> DVec2 {
        let projected = camera
            .view_projection(4.0 / 3.0)
            .project_point3(point.as_vec3());
        DVec2::new(
            f64::from(projected.x + 1.0) * 400.0,
            f64::from(1.0 - projected.y) * 300.0,
        )
    }

    fn fixture(projection: Projection) -> (Session, [ObjectId; 2], [DVec2; 2], ObjectId) {
        let mut session = Session::new(Err("no assets".into()));
        let camera = if projection == Projection::Perspective {
            Camera::looking_at(Vec3::new(-300.0, -400.0, 260.0), Vec3::ZERO)
        } else {
            Camera::planar(projection, Vec3::ZERO, 384.0)
        };
        let centers = [-64.0, 64.0].map(|distance| camera.right().as_dvec3() * distance);
        let mut ids = Vec::new();
        for center in centers {
            ids.push(
                session
                    .create_block(
                        Actor::User,
                        center - DVec3::splat(16.0),
                        center + DVec3::splat(16.0),
                        "brick/test",
                        false,
                    )
                    .unwrap(),
            );
            let behind = center + camera.forward().as_dvec3() * 128.0;
            session
                .create_block(
                    Actor::User,
                    behind - DVec3::splat(8.0),
                    behind + DVec3::splat(8.0),
                    "brick/test",
                    false,
                )
                .unwrap();
        }
        let ObjectId::Solid(second) = ids[1] else {
            unreachable!()
        };
        ids[1] = session
            .edit(Actor::User, "brush entity", |tx| {
                edit::tie_to_entity(tx, &[second], "func_detail", &[])
            })
            .unwrap();
        let hidden = session
            .create_block(
                Actor::User,
                DVec3::splat(-16.0),
                DVec3::splat(16.0),
                "brick/test",
                false,
            )
            .unwrap();
        session
            .set_visibility(Actor::User, "hide", &[hidden])
            .unwrap();
        let light = session
            .edit(Actor::User, "point helper", |tx| {
                edit::create_entity(
                    tx,
                    "light",
                    Some(camera.up().as_dvec3() * 64.0),
                    &[],
                    Vec::new(),
                )
            })
            .unwrap();
        session.select(&[], SelectMode::Replace).unwrap();
        session.set_camera(camera);
        (
            session,
            [ids[0], ids[1]],
            centers.map(|point| screen(camera, point)),
            light,
        )
    }

    fn spec(session: &Session, position: DVec2, faces: bool, mode: SelectMode) -> Spec {
        Spec {
            camera: session.camera(),
            pointer: Pointer {
                position,
                size: (800, 600),
            },
            radius: 6.0,
            mode,
            faces,
        }
    }

    fn texture_source(session: &mut Session) -> FaceId {
        let ObjectId::Solid(id) = session
            .create_block(
                Actor::User,
                DVec3::new(-16.0, 112.0, -16.0),
                DVec3::new(16.0, 144.0, 16.0),
                "metal/sample",
                false,
            )
            .unwrap()
        else {
            unreachable!()
        };
        let side = session
            .document
            .solid(id)
            .unwrap()
            .sides
            .iter()
            .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
            .unwrap()
            .id;
        let face = FaceId { solid: id, side };
        session
            .edit_faces(
                Actor::User,
                &[face],
                SurfaceEdit {
                    shift_u: Some(13.5),
                    shift_v: Some(-21.0),
                    scale_u: Some(0.5),
                    scale_v: Some(-0.25),
                    rotation: Some(37.0),
                    lightmap_scale: Some(8),
                    ..Default::default()
                },
                false,
                false,
            )
            .unwrap();
        session.sample_face_texture(face).unwrap();
        face
    }

    #[test]
    fn texture_strokes_preview_atomic_edits_preserve_selection_and_undo_once() {
        for operation in [Operation::Material, Operation::Projection] {
            let (mut session, _, points, light) = fixture(Projection::Top);
            let source = texture_source(&mut session);
            session.select(&[light], SelectMode::Replace).unwrap();
            let before = session.document.vmf().clone();
            let history = session.document.undo_labels().count();
            let spec = spec(&session, points[0], false, SelectMode::Toggle);
            let token = session.begin_texture_stroke(spec, operation).unwrap();
            session
                .update_texture_stroke(token, &[points[1], points[0], points[1]])
                .unwrap();
            let stroke = session.paint_stroke().unwrap();
            assert_eq!(stroke.faces.len(), 2);
            let texture = stroke.texture.as_ref().unwrap();
            assert_eq!(texture.changed, 2);
            assert_eq!(texture.edits.len(), 2);
            assert_eq!(texture.document.undo_labels().count(), 1);
            let preview = session.render_document().vmf().clone();
            for face in &stroke.faces {
                let old = surface::side(&session.document, *face).unwrap();
                let new = surface::side(&texture.document, *face).unwrap();
                let mut expected = old.clone();
                expected.material = session.surface_sample().unwrap().material.clone();
                if operation == Operation::Projection {
                    let sampled = surface::side(&session.document, source).unwrap();
                    expected.u_axis = sampled.u_axis;
                    expected.v_axis = sampled.v_axis;
                    expected.rotation = sampled.rotation;
                }
                assert_eq!(*new, expected);
            }
            assert_eq!(session.document.vmf(), &before);
            assert_eq!(session.selected_ids(), vec![light]);
            assert_eq!(session.document.undo_labels().count(), history);
            assert!(session.commit_paint_selection(Actor::Agent, token).is_err());
            assert_eq!(
                session.commit_texture_stroke(Actor::Agent, token).unwrap(),
                2
            );
            assert_eq!(session.document.vmf(), &preview);
            assert_eq!(session.selected_ids(), vec![light]);
            assert_eq!(session.document.undo_labels().count(), history + 1);
            session.undo(Actor::User).unwrap();
            assert_eq!(session.document.vmf(), &before);
            session.redo(Actor::User).unwrap();
            assert_eq!(session.document.vmf(), &preview);
            let token = session.begin_texture_stroke(spec, operation).unwrap();
            session.update_texture_stroke(token, &[points[1]]).unwrap();
            assert_eq!(
                session.commit_texture_stroke(Actor::User, token).unwrap(),
                0
            );
            assert_eq!(session.document.undo_labels().count(), history + 1);
        }
    }

    #[test]
    fn failed_texture_updates_and_starts_preserve_the_existing_draft() {
        let (mut session, _, points, _) = fixture(Projection::Top);
        let spec = spec(&session, points[0], true, SelectMode::Replace);
        let selection = session.begin_paint_selection(spec).unwrap();
        assert!(
            session
                .begin_texture_stroke(spec, Operation::Material)
                .is_err()
        );
        assert_eq!(session.paint_stroke().unwrap().token, selection);
        let source = texture_source(&mut session);
        let token = session
            .begin_texture_stroke(spec, Operation::Material)
            .unwrap();
        let before = session.paint_selection_json(0, 500);
        let preview = session.render_document().vmf().clone();
        assert!(
            session
                .update_texture_stroke(token, &[points[1], DVec2::splat(f64::NAN)])
                .is_err()
        );
        assert_eq!(session.paint_selection_json(0, 500), before);
        assert_eq!(session.render_document().vmf(), &preview);
        let mut bad = spec;
        bad.radius = 100.0;
        assert!(
            session
                .begin_texture_stroke(bad, Operation::Projection)
                .is_err()
        );
        assert_eq!(session.paint_stroke().unwrap().token, token);
        let bad_projection = Spec {
            camera: Camera::planar(Projection::Front, Vec3::ZERO, 384.0),
            ..spec
        };
        assert!(
            session
                .begin_texture_stroke(bad_projection, Operation::Projection)
                .is_err()
        );
        assert_eq!(session.paint_stroke().unwrap().token, token);
        let empty = Spec {
            pointer: Pointer {
                position: DVec2::new(40.0, 40.0),
                ..bad_projection.pointer
            },
            ..bad_projection
        };
        let token = session
            .begin_texture_stroke(empty, Operation::Projection)
            .unwrap();
        let before = session.paint_selection_json(0, 500);
        let preview = session.render_document().vmf().clone();
        assert!(session.update_texture_stroke(token, &[points[0]]).is_err());
        assert_eq!(session.paint_selection_json(0, 500), before);
        assert_eq!(session.render_document().vmf(), &preview);
        let camera = Camera::looking_at(Vec3::new(-300.0, -400.0, 260.0), Vec3::ZERO);
        let top = screen(camera, DVec3::new(-64.0, 0.0, 16.0));
        let wall = screen(camera, DVec3::new(64.0, -16.0, 0.0));
        let spec_perspective = Spec {
            camera,
            radius: 2.0,
            pointer: Pointer {
                position: top,
                ..spec.pointer
            },
            ..spec
        };
        let token = session
            .begin_texture_stroke(spec_perspective, Operation::Projection)
            .unwrap();
        let before = session.paint_selection_json(0, 500);
        let preview = session.render_document().vmf().clone();
        assert!(session.update_texture_stroke(token, &[wall]).is_err());
        assert_eq!(session.paint_selection_json(0, 500), before);
        assert_eq!(session.render_document().vmf(), &preview);
        session.sample_face_texture(source).unwrap();
        assert!(session.paint_stroke().is_none());
        assert!(session.commit_texture_stroke(Actor::Agent, token).is_err());
        let token = session
            .begin_texture_stroke(spec, Operation::Projection)
            .unwrap();
        let before = session.document.vmf().clone();
        session.cancel_texture_stroke(token).unwrap();
        assert_eq!(session.document.vmf(), &before);
    }

    #[test]
    fn visible_strokes_match_sparse_and_dense_samples_in_every_projection() {
        for projection in [
            Projection::Perspective,
            Projection::Top,
            Projection::Front,
            Projection::Side,
        ] {
            let (mut session, expected, points, _) = fixture(projection);
            let map = session.document.vmf().clone();
            let history = session.document.undo_labels().count();
            let spec = spec(&session, points[0], false, SelectMode::Replace);
            let token = session.begin_paint_selection(spec).unwrap();
            session.update_paint_selection(token, &[points[1]]).unwrap();
            let samples = session.paint_stroke().unwrap().samples.clone();
            let matches = session.paint_stroke().unwrap().objects.clone();
            assert_eq!(matches, expected.into_iter().collect());
            assert!(session.selected_ids().is_empty());
            session.cancel_paint_selection(token).unwrap();
            let token = session.begin_paint_selection(spec).unwrap();
            let dense = (1..=32)
                .map(|step| points[0].lerp(points[1], step as f64 / 32.0))
                .collect::<Vec<_>>();
            session.update_paint_selection(token, &dense).unwrap();
            assert_eq!(session.paint_stroke().unwrap().samples, samples);
            assert_eq!(session.paint_stroke().unwrap().objects, matches);
            let serial = session.scene_serial();
            session
                .update_paint_selection(token, &[points[0], points[1]])
                .unwrap();
            assert_eq!(session.paint_stroke().unwrap().samples, samples);
            assert_eq!(session.scene_serial(), serial);
            session.commit_paint_selection(Actor::Agent, token).unwrap();
            assert_eq!(session.selected_ids(), expected);
            assert_eq!(session.document.vmf(), &map);
            assert_eq!(session.document.undo_labels().count(), history);
        }
    }

    #[test]
    fn stroke_combination_counts_each_hit_once_and_faces_ignore_point_helpers() {
        for mode in [
            SelectMode::Replace,
            SelectMode::Add,
            SelectMode::Remove,
            SelectMode::Toggle,
        ] {
            let (mut session, ids, points, light) = fixture(Projection::Top);
            session.select(&[ids[0]], SelectMode::Replace).unwrap();
            let token = session
                .begin_paint_selection(spec(&session, points[0], false, mode))
                .unwrap();
            session
                .update_paint_selection(token, &[points[1], points[0], points[1]])
                .unwrap();
            session.commit_paint_selection(Actor::User, token).unwrap();
            assert_eq!(
                session.selected_ids(),
                match mode {
                    SelectMode::Replace | SelectMode::Add => ids.to_vec(),
                    SelectMode::Remove => Vec::new(),
                    SelectMode::Toggle => vec![ids[1]],
                }
            );
            let point = screen(
                session.camera(),
                session
                    .document
                    .entity(match light {
                        ObjectId::Entity(id) => id,
                        _ => unreachable!(),
                    })
                    .unwrap()
                    .origin()
                    .unwrap(),
            );
            let token = session
                .begin_paint_selection(spec(&session, point, false, SelectMode::Replace))
                .unwrap();
            assert_eq!(
                session.paint_stroke().unwrap().objects,
                [light].into_iter().collect()
            );
            session.cancel_paint_selection(token).unwrap();
            let token = session
                .begin_paint_selection(spec(&session, points[0], true, SelectMode::Replace))
                .unwrap();
            session
                .update_paint_selection(token, &[points[1], point])
                .unwrap();
            assert!(session.paint_stroke().unwrap().objects.is_empty());
            assert_eq!(session.paint_stroke().unwrap().faces.len(), 2);
            let faces = session
                .commit_paint_selection(Actor::Agent, token)
                .unwrap()
                .faces;
            assert_eq!(session.selected_faces(), faces);
        }
    }

    #[test]
    fn paint_guards_reject_invalid_updates_and_preserve_foreign_strokes() {
        let (mut session, ids, points, _) = fixture(Projection::Top);
        let spec = spec(&session, points[0], false, SelectMode::Replace);
        let old = session.begin_paint_selection(spec).unwrap();
        let token = session.begin_paint_selection(spec).unwrap();
        let before = session.paint_selection_json(0, 500);
        assert!(session.cancel_paint_selection(old).is_err());
        assert!(session.commit_paint_selection(Actor::User, old).is_err());
        assert!(
            session
                .update_paint_selection(token, &[points[1], DVec2::splat(f64::NAN)])
                .is_err()
        );
        assert!(
            session
                .update_paint_selection(token, &vec![points[1]; MAX_POINTS + 1])
                .is_err()
        );
        assert!(
            session
                .begin_paint_selection(Spec {
                    radius: f64::NAN,
                    ..spec
                })
                .is_err()
        );
        let mut invalid = spec;
        invalid.camera.fov_y_degrees = f32::NAN;
        assert!(session.begin_paint_selection(invalid).is_err());
        assert_eq!(session.paint_selection_json(0, 500), before);
        let mut view = session.view();
        view.surface = render::SurfaceView::Flat;
        session.set_view(view);
        assert!(session.paint_stroke().is_some());
        session.select(&[ids[0]], SelectMode::Replace).unwrap();
        assert!(session.paint_stroke().is_none());
        let token = session.begin_paint_selection(spec).unwrap();
        session.set_camera(session.camera());
        assert!(session.commit_paint_selection(Actor::Agent, token).is_err());
        let token = session.begin_paint_selection(spec).unwrap();
        session
            .set_visibility(Actor::User, "hide", &[ids[0]])
            .unwrap();
        assert!(session.commit_paint_selection(Actor::Agent, token).is_err());
        let token = session.begin_paint_selection(spec).unwrap();
        let mut view = session.view();
        view.tool_textures = !view.tool_textures;
        session.set_view(view);
        assert!(session.cancel_paint_selection(token).is_err());
        let token = session.begin_paint_selection(spec).unwrap();
        session
            .set_transform_tool(Actor::Agent, session.transform_tool())
            .unwrap();
        assert!(!session.paint_enabled());
        assert!(session.cancel_paint_selection(token).is_err());
    }

    #[test]
    fn oversized_stroke_updates_leave_the_previous_preview_unchanged() {
        let (mut session, _, _, _) = fixture(Projection::Top);
        let token = session
            .begin_paint_selection(Spec {
                camera: session.camera(),
                pointer: Pointer {
                    position: DVec2::splat(100.0),
                    size: (4096, 4096),
                },
                radius: 64.0,
                mode: SelectMode::Replace,
                faces: false,
            })
            .unwrap();
        let before = session.paint_selection_json(0, 500);
        assert!(
            session
                .update_paint_selection(token, &[DVec2::splat(4000.0)])
                .is_err()
        );
        assert_eq!(session.paint_selection_json(0, 500), before);
        let start = DVec2::splat(100.0);
        let end = DVec2::new(500.0, 100.0);
        session.update_paint_selection(token, &[end]).unwrap();
        let before = session.paint_selection_json(0, 500);
        let repeated = (0..128)
            .map(|index| if index % 2 == 0 { start } else { end })
            .collect::<Vec<_>>();
        assert!(session.update_paint_selection(token, &repeated).is_err());
        assert_eq!(session.paint_selection_json(0, 500), before);
    }
}
