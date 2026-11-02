use std::collections::{BTreeMap, BTreeSet};

use glam::{DMat3, DQuat, DVec3, EulerRot};
use map_core::{ObjectId, repeat, surface::FaceId};
use serde_json::{Value, json};

use super::{Intent, PivotSource, Policies, Session};

pub const MAX_FACES: usize = 32;
const MAX_TRIANGLES: usize = 16_384;
const MAX_TARGET_SIDES: usize = 2048;
const MAX_SOLID_SIDES: usize = 128;
const MAX_PREPARED_TERRAIN_FACES: usize = 128;
const MAX_PREPARED_TERRAIN_SIDES: usize = 8192;
const MAX_TERRAIN_BATCH_FACES: usize = 16;

#[derive(Clone, Debug, PartialEq)]
pub struct ScatterRequest {
    pub count: u32,
    pub seed: u64,
    pub align_normal: bool,
    pub heading: [f64; 2],
    pub scale: [f64; 2],
    pub normal_offset: f64,
    pub texture_lock: bool,
    pub pivot_source: Option<PivotSource>,
    pub pivot: Option<DVec3>,
    pub policies: Policies,
    pub source_generation: u64,
}

impl ScatterRequest {
    fn validate(&self) -> Result<(), String> {
        if self.count == 0 || self.count > repeat::MAX_COPIES {
            return Err("Scatter requires 1–64 additional copies".into());
        }
        if self
            .heading
            .iter()
            .any(|value| !value.is_finite() || value.abs() > 360.0)
            || self.heading[0] > self.heading[1]
        {
            return Err("Scatter heading needs an ordered finite range within ±360 degrees".into());
        }
        if self
            .scale
            .iter()
            .any(|value| !value.is_finite() || *value <= 0.0)
            || self.scale[0] > self.scale[1]
        {
            return Err("Scatter scale needs an ordered finite positive uniform range".into());
        }
        if !self.normal_offset.is_finite()
            || self.normal_offset.abs() > map_core::primitive::MAX_COORDINATE
        {
            return Err("Scatter normal offset must be finite and within ±65536".into());
        }
        Ok(())
    }

    fn report(&self) -> Value {
        json!({"count":self.count,"seed":self.seed,"align_normal":self.align_normal,
            "heading":self.heading,"scale":self.scale,"normal_offset":self.normal_offset,
            "texture_lock":self.texture_lock,"pivot_source":self.pivot_source,
            "pivot":self.pivot.map(|point|point.to_array()),"policies":self.policies,
            "source_generation":self.source_generation})
    }
}

struct Triangle {
    face: FaceId,
    index: usize,
    positions: [DVec3; 3],
    normal: DVec3,
    end: f64,
}

struct Sample {
    face: FaceId,
    triangle: usize,
    point: DVec3,
    normal: DVec3,
    heading: f64,
    scale: f64,
}

pub(super) struct Description {
    pub(super) request: ScatterRequest,
    faces: Vec<FaceId>,
    samples: Vec<Sample>,
    pivot: DVec3,
    anchor_policy: &'static str,
    source_up: DVec3,
    triangles: usize,
    area: f64,
}

impl Description {
    pub(super) fn report(&self, report: &mut Value) {
        report["request"] = self.request.report();
        report["faces"] = json!(
            self.faces
                .iter()
                .map(|face| json!({"solid":face.solid,"side":face.side}))
                .collect::<Vec<_>>()
        );
        report["samples"] = json!(self.samples.iter().enumerate().map(|(index,sample)|json!({
            "copy":index+1,"face":{"solid":sample.face.solid,"side":sample.face.side},
            "triangle":sample.triangle,"point":sample.point.to_array(),"normal":sample.normal.to_array(),
            "heading":sample.heading,"scale":sample.scale,
        })).collect::<Vec<_>>());
        report["pivot"] = json!(self.pivot.to_array());
        report["anchor_policy"] = json!(self.anchor_policy);
        report["source_up"] = json!(self.source_up.to_array());
        report["texture_lock"] = json!(self.request.texture_lock);
        report["surface_triangles"] = json!(self.triangles);
        report["surface_area"] = json!(self.area);
        report["sampling_policy"] = json!(
            "SplitMix64 seed; canonical face order; area-weighted authored brush fans and prepared full-resolution sewn terrain triangles, with uniform barycentric samples. The source pivot lands at the sample plus normal offset. Alignment maps the first stable source frame's local +Z to the sampled normal; heading then rotates about that normal. Without alignment heading rotates about world +Z. Copies can overlap; no settling, clearance or collision prevention is performed."
        );
    }
}

pub(super) fn limits() -> Value {
    json!({"faces":MAX_FACES,"triangles":MAX_TRIANGLES,"target_sides":MAX_TARGET_SIDES,
        "solid_sides":MAX_SOLID_SIDES,"prepared_terrain_faces":MAX_PREPARED_TERRAIN_FACES,
        "prepared_terrain_sides":MAX_PREPARED_TERRAIN_SIDES,"heading_degrees":[-360,360],
        "normal_offset":map_core::primitive::MAX_COORDINATE})
}

struct Random(u64);

impl Random {
    fn unit(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut value = self.0;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
        value ^= value >> 31;
        (value >> 11) as f64 / (1_u64 << 53) as f64
    }

    fn range(&mut self, [low, high]: [f64; 2]) -> f64 {
        low + (high - low) * self.unit()
    }
}

fn add_triangle(
    triangles: &mut Vec<Triangle>,
    face: FaceId,
    index: usize,
    positions: [DVec3; 3],
) -> Result<(), String> {
    if triangles.len() >= MAX_TRIANGLES {
        return Err("Scatter targets exceed 16384 prepared triangles".into());
    }
    if positions.iter().any(|point| {
        !point.is_finite() || point.abs().max_element() > map_core::primitive::MAX_COORDINATE
    }) {
        return Err("Scatter target vertices must be finite and within ±65536".into());
    }
    let cross = (positions[2] - positions[0]).cross(positions[1] - positions[0]);
    let double_area = cross.length();
    if !double_area.is_finite() || double_area <= 1e-10 {
        return Err(format!(
            "Scatter target {}:{} has a degenerate triangle",
            face.solid, face.side
        ));
    }
    let previous = triangles.last().map_or(0.0, |triangle| triangle.end);
    let end = previous + double_area * 0.5;
    if !end.is_finite() || end <= previous {
        return Err("Scatter target areas exceed the supported finite sampling precision".into());
    }
    triangles.push(Triangle {
        face,
        index,
        positions,
        normal: cross / double_area,
        end,
    });
    Ok(())
}

impl Session {
    fn scatter_triangles(
        &mut self,
        sources: &[ObjectId],
        faces: &[FaceId],
    ) -> Result<Vec<Triangle>, String> {
        if faces.is_empty() || faces.len() > MAX_FACES {
            return Err("Choose 1–32 authored brush or terrain target faces".into());
        }
        if faces.iter().copied().collect::<BTreeSet<_>>().len() != faces.len() {
            return Err("Scatter target faces must be distinct".into());
        }
        let mut target_solids = BTreeSet::new();
        let mut terrain = Vec::new();
        let mut side_count = 0;
        for &face in faces {
            let (owner, solid) = map_core::subobject::solid(&self.document, face.solid)
                .map_err(|error| error.to_string())?;
            if sources.contains(&owner) || sources.contains(&ObjectId::Solid(face.solid)) {
                return Err(
                    "Scatter target faces must belong to objects outside the source assembly"
                        .into(),
                );
            }
            if solid.sides.len() > MAX_SOLID_SIDES {
                return Err("Scatter target brushes support at most 128 sides each".into());
            }
            if target_solids.insert(solid.id) {
                side_count += solid.sides.len();
                if side_count > MAX_TARGET_SIDES {
                    return Err("Scatter targets exceed 2048 authored brush sides".into());
                }
            }
            let side = solid
                .sides
                .iter()
                .find(|side| side.id == face.side)
                .ok_or("Scatter target face does not belong to its authored brush")?;
            let material = side.material.clone();
            if side.displacement.is_some() {
                terrain.push(face);
            }
            if solid.hidden
                || matches!(owner, ObjectId::Entity(id) if self.document.entity(id).is_none_or(|entity|entity.hidden))
                || !self.working_solid_visible(&self.document, face.solid)
            {
                return Err(format!(
                    "Scatter target {}:{} is hidden or outside the working region",
                    face.solid, face.side
                ));
            }
            if !self.view.tool_textures {
                let id = self.materials.load(&self.game, &material);
                if self.materials.get(id).tool {
                    return Err("Scatter target uses a hidden tool material".into());
                }
            }
        }
        let mut triangles = Vec::new();
        let mut geometry = BTreeMap::new();
        for &face in faces.iter().filter(|face| !terrain.contains(face)) {
            if let std::collections::btree_map::Entry::Vacant(entry) = geometry.entry(face.solid) {
                let solid = map_core::surface::solid(&self.document, face.solid)
                    .ok_or("Scatter target brush is missing")?;
                entry.insert(
                    map_core::BrushGeometry::from_solid(solid)
                        .map_err(|error| error.to_string())?,
                );
            }
            let solid = map_core::surface::solid(&self.document, face.solid)
                .ok_or("Scatter target brush is missing")?;
            let polygon = geometry[&face.solid]
                .faces
                .iter()
                .find(|polygon| solid.sides[polygon.side].id == face.side)
                .ok_or("Scatter target face has no visible polygon")?;
            for index in 1..polygon.vertices.len() - 1 {
                add_triangle(
                    &mut triangles,
                    face,
                    index - 1,
                    [
                        polygon.vertices[0],
                        polygon.vertices[index],
                        polygon.vertices[index + 1],
                    ],
                )?;
            }
        }
        if !terrain.is_empty() {
            let mut terrain_faces = 0;
            let mut terrain_sides = 0;
            for solid in self
                .document
                .vmf()
                .solids()
                .filter(|solid| solid.sides.iter().any(|side| side.displacement.is_some()))
            {
                terrain_faces += solid
                    .sides
                    .iter()
                    .filter(|side| side.displacement.is_some())
                    .count();
                terrain_sides += solid.sides.len();
                if terrain_faces > MAX_PREPARED_TERRAIN_FACES
                    || terrain_sides > MAX_PREPARED_TERRAIN_SIDES
                    || solid.sides.len() > MAX_SOLID_SIDES
                {
                    return Err("Scatter terrain preparation exceeds 128 terrain faces, 8192 brush sides or 128 sides per terrain brush".into());
                }
            }
            for batch in terrain.chunks(MAX_TERRAIN_BATCH_FACES) {
                for row in self.terrain_surface_triangles(batch, false)? {
                    for triangle in row.triangles {
                        if triangle.removed || triangle.unknown_tags != 0 {
                            return Err("Scatter does not support terrain triangle removal or unknown triangle tags".into());
                        }
                        add_triangle(&mut triangles, row.face, triangle.id, triangle.positions)?;
                    }
                }
            }
        }
        if triangles.is_empty() {
            return Err("Scatter targets have no supported nondegenerate triangles".into());
        }
        Ok(triangles)
    }

    pub fn preview_surface_scatter(
        &mut self,
        sources: &[ObjectId],
        faces: &[FaceId],
        request: ScatterRequest,
        snapshot: &str,
    ) -> Result<Value, String> {
        if request.source_generation != self.document_generation {
            return Err(
                "Scatter source or face capture belongs to a replaced map; capture it again".into(),
            );
        }
        self.check_repeat_request(sources, request.count, snapshot)?;
        request.validate()?;
        let (pivot, anchor_policy) = if request.pivot_source.is_none() && request.pivot.is_none() {
            match sources {
                [ObjectId::Entity(id)]
                    if self
                        .document
                        .entity(*id)
                        .is_some_and(|entity| entity.solids.is_empty()) =>
                {
                    (
                        map_core::measurements::entity_origin(
                            self.document.vmf(),
                            ObjectId::Entity(*id),
                        )?,
                        "point_entity_origin",
                    )
                }
                _ => (self.selection_pivot(sources)?, "selection_center"),
            }
        } else {
            (
                self.resolve_transform_pivot_request(sources, request.pivot_source, request.pivot)?,
                match PivotSource::from_request(request.pivot_source, request.pivot)? {
                    PivotSource::SelectionCenter => "selection_center",
                    PivotSource::CurrentTransformPivot => "current_transform_pivot",
                    PivotSource::CustomWorldPoint => "custom_world_point",
                },
            )
        };
        let source_up = self
            .transform_basis(sources, true)?
            .z_axis
            .try_normalize()
            .ok_or("Scatter source needs a finite nondegenerate local frame")?;
        let mut faces = faces.to_vec();
        faces.sort();
        let triangles = self.scatter_triangles(sources, &faces)?;
        let area = triangles.last().unwrap().end;
        let mut random = Random(request.seed);
        let mut samples = Vec::with_capacity(request.count as usize);
        let mut placements = Vec::with_capacity(request.count as usize);
        for _ in 0..request.count {
            let selector = random.unit() * area;
            let triangle = triangles
                .get(triangles.partition_point(|triangle| triangle.end <= selector))
                .ok_or("Scatter area lookup exceeded the supported sampling precision")?;
            let root = random.unit().sqrt();
            let edge = random.unit();
            let [a, b, c] = triangle.positions;
            let point = a * (1.0 - root) + b * (root * (1.0 - edge)) + c * (root * edge);
            let heading = random.range(request.heading);
            let scale = random.range(request.scale);
            let normal = triangle.normal;
            let axis = if request.align_normal {
                normal
            } else {
                DVec3::Z
            };
            let aligned = if request.align_normal {
                DQuat::from_rotation_arc(source_up, normal)
            } else {
                DQuat::IDENTITY
            };
            let rotation = DQuat::from_axis_angle(axis, heading.to_radians()) * aligned;
            let (z, y, x) = rotation.to_euler(EulerRot::ZYX);
            placements.push(repeat::Placement {
                translation: point + normal * request.normal_offset - pivot,
                rotation: DVec3::new(x, y, z).map(f64::to_degrees),
                scale: DVec3::splat(scale),
                pivot,
                basis: DMat3::IDENTITY,
                texture_lock: request.texture_lock,
            });
            samples.push(Sample {
                face: triangle.face,
                triangle: triangle.index,
                point,
                normal,
                heading,
                scale,
            });
        }
        let plan = repeat::Plan {
            placements,
            policies: request.policies.core(),
        };
        self.prepare_repeat_preview(
            sources,
            plan,
            Intent::SurfaceScatter(Description {
                request,
                faces,
                samples,
                pivot,
                anchor_policy,
                source_up,
                triangles: triangles.len(),
                area,
            }),
        )
    }
}
