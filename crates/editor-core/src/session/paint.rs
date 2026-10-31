use super::previews;
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
    filters: Option<u64>,
    generation: u64,
    revision: u64,
    shared_camera: Camera,
    groups: Option<super::groups::SelectionIndex>,
    object_mode: super::selection_filters::ObjectSelectionMode,
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
        self.geometry.paint.enabled
    }
    pub fn paint_operation(&self) -> Operation {
        self.geometry.paint.operation
    }

    pub fn paint_radius(&self) -> f64 {
        self.geometry.paint.radius
    }
    pub fn paint_stroke(&self) -> Option<&Stroke> {
        self.geometry.paint.stroke.as_ref()
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
        if self.geometry.paint.enabled == enabled
            && self.geometry.paint.radius == radius
            && self.geometry.paint.operation == operation
        {
            return Ok(());
        }
        self.cancel_area_selection();
        if enabled {
            self.cancel_block_preview();
            self.clear_geometry_previews_except(&[previews::Kind::Paint]);

            self.clear_volume_selection();

            self.transform_tool.mode = None;
            self.clip_tool = false;
            self.spatial.draw_block = false;
        }
        self.geometry.paint.enabled = enabled;
        self.geometry.paint.radius = radius;
        self.geometry.paint.operation = operation;
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
        if self.geometry.paint.stroke.take().is_some() {
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
            || stroke
                .filters
                .is_some_and(|revision| revision != self.selection_filter_revision())
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
        let hidden = self.hidden_brushes_for(false);
        let evaluation = self.working_evaluation(&self.document);
        let bounds = self.display_bounds_in(&self.document);
        let object_selection = !spec.faces && operation == Operation::Selection;
        let filters = self.selection_filters();
        let solids = object_selection
            .then(|| filters.allowed_solids(&self.document))
            .flatten();
        let picker = Picker::with_entity_bounds(
            &self.document,
            |solid| {
                !hidden.contains(&solid.id)
                    && solids.as_ref().is_none_or(|ids| ids.contains(&solid.id))
            },
            !spec.faces,
            |entity| {
                evaluation.objects.contains(&ObjectId::Entity(entity.id))
                    && (!object_selection
                        || filters.allows(&self.document, ObjectId::Entity(entity.id)))
            },
            |entity| {
                bounds
                    .get(&ObjectId::Entity(entity.id))
                    .copied()
                    .or_else(|| query::entity_bounds(entity))
            },
        );
        let token = self.geometry.paint.serial + 1;
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
            filters: object_selection.then(|| self.selection_filter_revision()),
            generation: self.document_generation,
            revision: self.document.revision(),
            shared_camera: self.camera,
            object_mode: self.object_selection_mode(),
            groups: if spec.faces {
                None
            } else {
                self.group_selection_index()
            },
        };
        update_stroke(&mut stroke, &self.document, &[spec.pointer.position])?;
        self.cancel_area_selection();
        self.cancel_block_preview();
        self.clear_geometry_previews_except(&[previews::Kind::Paint]);

        self.clear_volume_selection();

        self.clip_tool = false;
        self.spatial.draw_block = false;
        self.geometry.paint.enabled = true;
        self.geometry.paint.radius = spec.radius;
        self.geometry.paint.operation = operation;
        self.transform_tool.mode = None;
        self.geometry.paint.serial = token;
        self.geometry.paint.stroke = Some(stroke);
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
        let changed = update_stroke(
            self.geometry.paint.stroke.as_mut().unwrap(),
            &self.document,
            points,
        )?;
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
        let texture = self.geometry.paint.stroke.take().unwrap().texture.unwrap();
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
        let stroke = self.geometry.paint.stroke.take().unwrap();
        let matches = Selection {
            objects: stroke.objects.into_iter().collect(),
            faces: stroke.faces.into_iter().collect(),
        };
        self.scene_serial += 1;
        self.view_serial += 1;
        if stroke.spec.faces {
            self.select_faces(&matches.faces, stroke.spec.mode)?;
        } else {
            self.select_picked_objects(&matches.objects, stroke.spec.mode)?;
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
        json!({"selection_filters":self.selection_filter_state(),"enabled":self.geometry.paint.enabled,"sample_material":self.surface_sample.as_ref().map(|sample|&sample.material),"operation":self.geometry.paint.operation.name(),"radius":self.geometry.paint.radius,"preview":preview,
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
                let object = stroke.object_mode.hit_object(hit);
                if let Some(groups) = &stroke.groups {
                    objects.extend(groups.members(object));
                } else {
                    objects.insert(object);
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

impl State {
    pub(super) fn close_draft(&self) -> Option<serde_json::Value> {
        self.stroke.as_ref().map(|stroke| {
            serde_json::json!([
                self.serial,
                stroke.token,
                stroke.cursor.to_array(),
                stroke.samples.len()
            ])
        })
    }
}
