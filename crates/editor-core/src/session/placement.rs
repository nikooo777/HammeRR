use std::collections::{BTreeMap, BTreeSet, HashSet};

use glam::{DQuat, DVec3, EulerRot};
use map_core::{
    BrushGeometry, DisplacementMesh, ObjectId,
    edit::Transform,
    query::{self, Ray, SurfaceHit},
    subobject::{self, Component},
};
use serde_json::{Value, json};
use source_formats::vmf::Solid;

use super::{Actor, Session};

const MAX_POINTS: usize = 1_000_000;
const CORNER_SNAP_ROUNDOFF_ULPS: f64 = 4.0;

#[cfg(any(test, feature = "test-support"))]
pub(crate) mod fixtures;

impl State {
    pub(super) fn close_draft(&self) -> Option<serde_json::Value> {
        self.draft
            .as_ref()
            .map(|draft| serde_json::json!([self.serial, draft.token, draft.plan.is_some()]))
    }
}

#[cfg(test)]
pub(crate) mod tests;

#[cfg(test)]
mod model_snap_tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Floor,
    Ceiling,
    Ray,
    BrushCorner,
}

impl Method {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "floor" => Ok(Self::Floor),
            "ceiling" => Ok(Self::Ceiling),
            "ray" => Ok(Self::Ray),
            "brush_corner" => Ok(Self::BrushCorner),
            _ => Err("Choose floor, ceiling, ray or brush_corner placement".into()),
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Floor => "floor",
            Self::Ceiling => "ceiling",
            Self::Ray => "ray",
            Self::BrushCorner => "brush_corner",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Options {
    pub align: bool,
    pub offset: f64,
    pub texture_lock: bool,
    pub source_corner: Option<Component>,
}

#[derive(Default)]
pub(super) struct State {
    serial: u64,
    draft: Option<Draft>,
}

struct Draft {
    token: u64,
    generation: u64,
    revision: u64,
    scene: u64,
    ids: Vec<ObjectId>,
    options: Options,
    plan: Option<Plan>,
    source_position: Option<DVec3>,
}

pub(super) struct Plan {
    method: Method,
    pub(super) surface: SurfaceHit,
    pub(super) transform: Transform,
    pub(super) support_points: usize,
    pub(super) contact: f64,
    corner: Option<CornerSnap>,
}

struct CornerSnap {
    source: DVec3,
    target: CornerTarget,
    position: DVec3,
}

enum CornerTarget {
    Brush(Component),
    Terrain(query::TerrainVertex),
    Model(ModelCornerTarget),
}

struct ModelCornerTarget {
    entity: ObjectId,
    path: String,
    checksum: i32,
    body: u32,
    skin: usize,
    scale: f32,
    origin: DVec3,
    rotation: DQuat,
    part: usize,
    triangle: usize,
    vertex: u32,
    position: DVec3,
}

struct PickedSurface {
    surface: SurfaceHit,
    model: Option<ModelCornerTarget>,
}

impl CornerTarget {
    fn report(&self) -> Value {
        match self {
            Self::Brush(component) => {
                let mut report = super::components::component_json(component);
                report["kind"] = json!("brush_corner");
                report
            }
            Self::Terrain(vertex) => json!({
                "kind":"terrain_vertex", "solid":vertex.face.solid, "side":vertex.face.side,
                "triangle":vertex.triangle, "index":vertex.index, "grid_size":vertex.grid_size,
                "row":vertex.index / vertex.grid_size, "column":vertex.index % vertex.grid_size,
                "provenance":"prepared_surface"
            }),
            Self::Model(target) => json!({
                "kind":"model_vertex", "entity":object_id(target.entity), "model":target.path,
                "checksum":target.checksum, "body":target.body, "skin":target.skin,
                "scale":target.scale, "origin":target.origin.to_array(), "rotation":target.rotation.to_array(),
                "part":target.part, "triangle":target.triangle, "vertex":target.vertex,
                "provenance":"cached_decoded_model_mesh"
            }),
        }
    }
}

pub(super) struct SupportPoint {
    point: DVec3,
    helper_offset: DVec3,
}

fn object_id(id: ObjectId) -> String {
    match id {
        ObjectId::Solid(id) => format!("solid:{id}"),
        ObjectId::Entity(id) => format!("entity:{id}"),
    }
}

fn precise_corner(solid: &Solid, component: &Component, approximate: DVec3) -> Option<DVec3> {
    let mut sides = component.sides.clone();
    sides.sort_unstable();
    let planes = sides
        .iter()
        .map(|id| {
            let side = solid.sides.iter().find(|side| side.id == *id)?;
            Some((map_core::Plane::from_points(side.plane.0)?, side.plane.0[1]))
        })
        .collect::<Option<Vec<_>>>()?;
    let reference = approximate;
    let mut best = None;
    let mut largest = 0.0;
    for (a, first) in planes.iter().enumerate() {
        for (b, second) in planes.iter().enumerate().skip(a + 1) {
            for (c, third) in planes.iter().enumerate().skip(b + 1) {
                let [na, nb, nc] = [first.0.normal, second.0.normal, third.0.normal];
                let determinant = na.dot(nb.cross(nc));
                if !determinant.is_finite() || determinant.abs() <= largest {
                    continue;
                }
                largest = determinant.abs();
                best = Some([a, b, c]);
            }
        }
    }
    let indices = best?;
    let [na, nb, nc] = indices.map(|index| planes[index].0.normal);
    let determinant = na.dot(nb.cross(nc));
    let distances = indices.map(|index| planes[index].0.normal.dot(planes[index].1 - reference));
    let point = reference
        + (distances[0] * nb.cross(nc) + distances[1] * nc.cross(na) + distances[2] * na.cross(nb))
            / determinant;
    if !point.is_finite() || !point.abs_diff_eq(approximate, map_core::plane::ON_EPSILON) {
        return None;
    }
    for (plane, origin) in planes {
        let scale = (plane.normal.abs() * (point.abs() + origin.abs())).element_sum();
        let error = CORNER_SNAP_ROUNDOFF_ULPS * f64::EPSILON * scale;
        if plane.normal.dot(point - origin).abs() > error {
            return None;
        }
    }
    Some(point)
}

fn push(points: &mut Vec<SupportPoint>, point: DVec3, helper_offset: DVec3) -> Result<(), String> {
    if points.len() >= MAX_POINTS || !point.is_finite() || !helper_offset.is_finite() {
        return Err("Surface placement exceeds one million finite support points".into());
    }
    points.push(SupportPoint {
        point,
        helper_offset,
    });
    Ok(())
}

fn solid_points(points: &mut Vec<SupportPoint>, solid: &Solid) -> Result<(), String> {
    if solid.hidden {
        return Err("Show selected brushes before placing them".into());
    }
    let geometry = BrushGeometry::from_solid(solid).map_err(|error| error.to_string())?;
    let displaced = solid.sides.iter().any(|side| side.displacement.is_some());
    for face in &geometry.faces {
        if let Some(info) = &solid.sides[face.side].displacement {
            for point in DisplacementMesh::build(face, info)
                .map_err(|error| error.to_string())?
                .positions
            {
                push(points, point, DVec3::ZERO)?;
            }
        } else if !displaced {
            for &point in &face.vertices {
                push(points, point, DVec3::ZERO)?;
            }
        }
    }
    Ok(())
}

impl Session {
    pub(super) fn surface_position_picking(&self) -> bool {
        self.placement.draft.as_ref().is_some_and(|draft| {
            draft.plan.is_none() && self.checked_placement(draft.token).is_ok()
        })
    }

    fn placement_source_corner(
        &mut self,
        ids: &[ObjectId],
        options: &Options,
    ) -> Result<Option<DVec3>, String> {
        let Some(component) = &options.source_corner else {
            return Ok(None);
        };
        if !(3..=subobject::MAX_SIDES).contains(&component.sides.len()) {
            return Err("A source corner needs 3–64 unique incident face IDs".into());
        }
        if ids.len() != 1 || options.align || options.offset != 0.0 {
            return Err(
                "Corner snapping needs one selected owner, zero offset and no alignment".into(),
            );
        }
        let hidden = self.hidden_brushes_for(false);
        let (owner, solid) =
            subobject::solid(&self.document, component.solid).map_err(|error| error.to_string())?;
        if ids[0] != owner
            || hidden.contains(&solid.id)
            || solid.hidden
            || matches!(owner, ObjectId::Entity(id) if self.document.entity(id).is_none_or(|entity| entity.hidden))
            || solid.sides.iter().any(|side| side.displacement.is_some())
        {
            return Err(
                "Choose a visible flat source brush corner belonging to the selected owner".into(),
            );
        }
        let topology = subobject::topology(solid).map_err(|error| error.to_string())?;
        let indices = topology
            .resolve(&component.sides)
            .map_err(|error| error.to_string())?;
        if indices.len() != 1 {
            return Err("Choose exactly one real source corner, rather than a face or edge".into());
        }
        precise_corner(solid, component, topology.vertices[indices[0]].position)
            .map(Some)
            .ok_or("Source corner planes have no finite intersection".into())
    }

    fn corner_translation_bounds(&self, owner: ObjectId, delta: DVec3) -> Result<(), String> {
        let check = |point: DVec3| {
            let point = point + delta;
            if !point.is_finite() || point.abs().max_element() > crate::spatial::MAX_EXTENT {
                return Err(
                    "Corner snapping would move part of the selected owner outside ±65536 units"
                        .to_string(),
                );
            }
            Ok(())
        };
        let brushes = match owner {
            ObjectId::Solid(id) => {
                std::slice::from_ref(self.document.solid(id).ok_or("Source brush is missing")?)
            }
            ObjectId::Entity(id) => self
                .document
                .entity(id)
                .ok_or("Source owner is missing")?
                .solids
                .as_slice(),
        };
        for brush in brushes {
            let geometry = BrushGeometry::from_solid(brush).map_err(|error| error.to_string())?;
            for point in [geometry.mins, geometry.maxs] {
                check(point)?;
            }
            for (index, side) in brush.sides.iter().enumerate() {
                let Some(info) = &side.displacement else {
                    continue;
                };
                let face = geometry
                    .faces
                    .iter()
                    .find(|face| face.side == index)
                    .ok_or_else(|| {
                        format!(
                            "Source brush {} terrain face {} has no effective geometry",
                            brush.id, side.id
                        )
                    })?;
                let mesh = DisplacementMesh::build(face, info).map_err(|error| {
                    format!(
                        "Source brush {} terrain face {}: {error}",
                        brush.id, side.id
                    )
                })?;
                for point in mesh.positions {
                    check(point)?;
                }
            }
        }
        Ok(())
    }

    fn placement_context(&self) -> Result<(), String> {
        let own = self.placement.draft.as_ref().is_some_and(|draft| {
            self.surface_transform_owner() == Some(draft.token)
                && self
                    .geometry
                    .transform_preview
                    .as_ref()
                    .is_some_and(|preview| std::ptr::eq(self.render_document(), &preview.document))
        });
        if !std::ptr::eq(self.render_document(), &self.document) && !own {
            return Err(
                "Apply or cancel the other geometry preview before surface placement".into(),
            );
        }
        Ok(())
    }

    pub(super) fn placement_points_in(
        &self,
        document: &map_core::Document,
        ids: &[ObjectId],
    ) -> Result<Vec<SupportPoint>, String> {
        if ids.is_empty() {
            return Err("Select objects to place".into());
        }
        let models = self
            .model_instances(document)
            .instances
            .into_iter()
            .map(|instance| (instance.id, instance))
            .collect::<BTreeMap<_, _>>();
        let hidden = document
            .vmf()
            .entities
            .iter()
            .filter(|entity| entity.hidden)
            .flat_map(|entity| entity.solids.iter().map(|solid| solid.id))
            .collect::<HashSet<_>>();
        let mut points = Vec::new();
        for &id in ids {
            match id {
                ObjectId::Solid(id) => {
                    if hidden.contains(&id) {
                        return Err("Show the owning entity before placing its brush".into());
                    }
                    solid_points(
                        &mut points,
                        document.solid(id).ok_or("Selected brush is missing")?,
                    )?;
                }
                ObjectId::Entity(id) => {
                    let entity = document.entity(id).ok_or("Selected entity is missing")?;
                    if entity.hidden {
                        return Err("Show selected entities before placing them".into());
                    }
                    if !entity.solids.is_empty() {
                        for solid in &entity.solids {
                            if ids.contains(&ObjectId::Solid(solid.id)) {
                                return Err("Select the owning entity or its child brushes, rather than both".into());
                            }
                            solid_points(&mut points, solid)?;
                        }
                    } else if let Some(instance) = models
                        .get(&ObjectId::Entity(id))
                        .filter(|instance| instance.has_geometry())
                    {
                        let model = &instance.asset.model;
                        let mut used = vec![false; model.vertices.len()];
                        for part in model.parts_for_body(instance.body) {
                            for &index in &part.indices {
                                used[index as usize] = true;
                            }
                        }
                        for (vertex, used) in model.vertices.iter().zip(used) {
                            if used {
                                push(&mut points, instance.point(vertex.position), DVec3::ZERO)?;
                            }
                        }
                    } else {
                        let (low, high) = self
                            .object_bounds_in(document, &[ObjectId::Entity(id)])
                            .ok_or("Selected entity has no finite placement bounds")?;
                        let center = (low + high) * 0.5;
                        for x in [low.x, high.x] {
                            for y in [low.y, high.y] {
                                for z in [low.z, high.z] {
                                    push(&mut points, center, DVec3::new(x, y, z) - center)?;
                                }
                            }
                        }
                    }
                }
            }
        }
        if points.is_empty() {
            return Err("Selected objects have no placement geometry".into());
        }
        Ok(points)
    }

    pub(super) fn placement_surface(
        &mut self,
        ids: &[ObjectId],
        ray: Ray,
        reject_ambiguous: bool,
    ) -> Result<SurfaceHit, String> {
        self.picked_placement_surface(ids, ray, reject_ambiguous)
            .map(|picked| picked.surface)
    }

    fn picked_placement_surface(
        &mut self,
        ids: &[ObjectId],
        ray: Ray,
        reject_ambiguous: bool,
    ) -> Result<PickedSurface, String> {
        let direction = ray
            .direction
            .try_normalize()
            .filter(|_| ray.origin.is_finite())
            .ok_or("Surface ray must have a finite origin and a nonzero finite direction")?;
        let ray = Ray {
            origin: ray.origin,
            direction,
        };
        let excluded = ids.iter().copied().collect::<HashSet<_>>();
        let mut solids = ids
            .iter()
            .filter_map(|id| {
                if let ObjectId::Solid(id) = id {
                    Some(*id)
                } else {
                    None
                }
            })
            .collect::<HashSet<_>>();
        for entity in &self.document.vmf().entities {
            if excluded.contains(&ObjectId::Entity(entity.id)) {
                solids.extend(entity.solids.iter().map(|solid| solid.id));
            }
        }
        let hidden = self.hidden_brushes_for(false);
        let ordinary = query::pick_surface(&self.document, &ray, |solid| {
            !solids.contains(&solid.id) && !hidden.contains(&solid.id)
        });
        if reject_ambiguous
            && ordinary.is_none()
            && query::pick_face(&self.document, &ray, |solid| {
                !solids.contains(&solid.id) && !hidden.contains(&solid.id)
            })
            .is_some()
        {
            return Err("The first visible brush hit has ambiguous or invalid geometry".into());
        }
        ordinary
            .into_iter()
            .map(|surface| PickedSurface {
                surface,
                model: None,
            })
            .chain(
                self.working_evaluation(&self.document)
                    .models
                    .instances
                    .iter()
                    .filter(|instance| {
                        !excluded.contains(&instance.id)
                            && self.working_object_visible(&self.document, instance.id)
                    })
                    .filter_map(|instance| {
                        let hit = instance.pick_mesh_surface(&ray)?;
                        Some(PickedSurface {
                            surface: hit.surface,
                            model: Some(ModelCornerTarget {
                                entity: instance.id,
                                path: instance.asset.path.clone(),
                                checksum: instance.asset.model.header.checksum,
                                body: instance.body,
                                skin: instance.skin,
                                scale: instance.scale,
                                origin: instance.origin,
                                rotation: instance.rotation,
                                part: hit.part,
                                triangle: hit.triangle,
                                vertex: hit.vertex,
                                position: hit.position,
                            }),
                        })
                    }),
            )
            .min_by(|a, b| a.surface.hit.distance.total_cmp(&b.surface.hit.distance))
            .ok_or("The ray did not hit a visible brush, terrain sheet or supported model".into())
    }

    fn plan_placement(
        &mut self,
        ids: &[ObjectId],
        method: Method,
        ray: Option<Ray>,
        options: &Options,
    ) -> Result<Plan, String> {
        if !options.offset.is_finite() || options.offset.abs() > 65536.0 {
            return Err("Surface offset must be finite and within ±65536 units".into());
        }
        if (method == Method::BrushCorner) != options.source_corner.is_some() {
            return Err(
                "brush_corner placement requires source_corner; other methods do not accept it"
                    .into(),
            );
        }
        if method == Method::BrushCorner {
            let source = self.placement_source_corner(ids, options)?.unwrap();
            let ray = ray.ok_or("Supply a ray or pick a target corner in the viewport")?;
            let PickedSurface { surface, model } = self.picked_placement_surface(ids, ray, true)?;
            let (target, position) = if let Some(target) = model {
                if self
                    .document
                    .vmf()
                    .entities
                    .iter()
                    .filter(|entity| ObjectId::Entity(entity.id) == target.entity)
                    .count()
                    != 1
                {
                    return Err("The first visible model has an ambiguous entity identity".into());
                }
                let position = target.position;
                (CornerTarget::Model(target), position)
            } else if let Some(vertex) = surface.terrain_vertex {
                (CornerTarget::Terrain(vertex), vertex.position)
            } else {
                let (target, approximate) = query::face_corner(&self.document, &surface.hit)
                    .ok_or("The first visible hit must have unambiguous brush, terrain or model mesh geometry")?;
                let (_, solid) = subobject::solid(&self.document, target.solid)
                    .map_err(|error| error.to_string())?;
                let position = precise_corner(solid, &target, approximate)
                    .ok_or("Target corner planes have no finite intersection")?;
                (CornerTarget::Brush(target), position)
            };
            if !position.is_finite() || position.abs().max_element() > crate::spatial::MAX_EXTENT {
                return Err("Target vertex must be finite and within ±65536 units".into());
            }
            let mut delta = position - source;
            for axis in 0..3 {
                let scale = source[axis]
                    .abs()
                    .max(position[axis].abs())
                    .max(self.spatial.grid_size);
                if delta[axis].abs() <= CORNER_SNAP_ROUNDOFF_ULPS * f64::EPSILON * scale {
                    delta[axis] = 0.0;
                }
            }
            self.corner_translation_bounds(ids[0], delta)?;
            return Ok(Plan {
                method,
                surface,
                support_points: 1,
                contact: 0.0,
                transform: Transform {
                    translation: delta,
                    pivot: source,
                    texture_lock: options.texture_lock,
                    texture_scale_lock: self.transform_tool.texture_scale_lock,
                    ..Transform::default()
                },
                corner: Some(CornerSnap {
                    source,
                    target,
                    position,
                }),
            });
        }
        if method != Method::Ray && ray.is_some() {
            return Err("Ray coordinates are only used for picked-surface placement".into());
        }
        let points = self.placement_points_in(&self.document, ids)?;
        let low = points
            .iter()
            .map(|point| point.point + point.helper_offset)
            .fold(DVec3::splat(f64::INFINITY), DVec3::min);
        let high = points
            .iter()
            .map(|point| point.point + point.helper_offset)
            .fold(DVec3::splat(f64::NEG_INFINITY), DVec3::max);
        let pivot = (low + high) * 0.5;
        let ray = match method {
            Method::Ray => ray.ok_or("Supply a ray or pick a surface in the viewport")?,
            Method::Floor => Ray {
                origin: DVec3::new(pivot.x, pivot.y, high.z + 0.01),
                direction: DVec3::NEG_Z,
            },
            Method::Ceiling => Ray {
                origin: DVec3::new(pivot.x, pivot.y, low.z - 0.01),
                direction: DVec3::Z,
            },
            Method::BrushCorner => unreachable!(),
        };
        let surface = self.placement_surface(ids, ray, false)?;
        contact_plan(
            points,
            self.transform_basis(ids, true)?.z_axis,
            method,
            ray,
            surface,
            options,
            self.transform_tool.texture_scale_lock,
        )
    }

    fn install_placement(
        &mut self,
        actor: Actor,
        ids: Vec<ObjectId>,
        options: Options,
        plan: Plan,
        token: u64,
    ) -> Result<Value, String> {
        let source_position = plan.corner.as_ref().map(|corner| corner.source);
        self.preview_surface_transform(actor, &ids, plan.transform, plan.transform.pivot, token)?;
        self.placement.serial = token;
        self.placement.draft = Some(Draft {
            token,
            generation: self.document_generation,
            revision: self.document.revision(),
            scene: self.scene_serial,
            ids,
            options,
            plan: Some(plan),
            source_position,
        });
        Ok(self.surface_placement())
    }

    pub fn preview_surface_placement(
        &mut self,
        actor: Actor,
        ids: &[ObjectId],
        method: Method,
        ray: Option<Ray>,
        options: Options,
    ) -> Result<Value, String> {
        self.placement_context()?;
        let ids = ids
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let plan = self.plan_placement(&ids, method, ray, &options)?;
        self.install_placement(actor, ids, options, plan, self.placement.serial + 1)
    }

    pub fn begin_surface_placement_pick(
        &mut self,
        ids: &[ObjectId],
        options: Options,
    ) -> Result<Value, String> {
        self.placement_context()?;
        let source_position = self.placement_source_corner(ids, &options)?;
        if source_position.is_none() {
            self.placement_points_in(&self.document, ids)?;
        }
        if !options.offset.is_finite() || options.offset.abs() > 65536.0 {
            return Err("Surface offset must be finite and within ±65536 units".into());
        }
        if let Some(draft) = &self.placement.draft
            && self.surface_transform_owner() == Some(draft.token)
        {
            self.cancel_transform_preview();
        }
        let ids = ids
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        self.select(&ids, super::SelectMode::Replace)?;
        self.placement.serial += 1;
        let token = self.placement.serial;
        self.placement.draft = Some(Draft {
            token,
            generation: self.document_generation,
            revision: self.document.revision(),
            scene: self.scene_serial,
            ids,
            options,
            plan: None,
            source_position,
        });
        self.view_serial += 1;
        Ok(self.surface_placement())
    }

    fn checked_placement(&self, token: u64) -> Result<&Draft, String> {
        let draft = self
            .placement
            .draft
            .as_ref()
            .filter(|draft| draft.token == token)
            .ok_or("Surface placement was closed or replaced")?;
        if draft.generation != self.document_generation
            || draft.revision != self.document.revision()
            || draft.scene != self.scene_serial
            || draft.ids != self.selected_ids()
        {
            return Err(
                "Map, selection or scene changed; preview the surface placement again".into(),
            );
        }
        if draft.plan.is_some() && self.surface_transform_owner() != Some(token) {
            return Err("Surface placement was replaced by another transform preview".into());
        }
        Ok(draft)
    }

    pub fn pick_surface_placement(
        &mut self,
        actor: Actor,
        token: u64,
        ray: Ray,
    ) -> Result<Value, String> {
        let draft = self.checked_placement(token)?;
        if draft.plan.is_some() {
            return Err("Start Pick surface again to choose another target".into());
        }
        let (ids, options) = (draft.ids.clone(), draft.options.clone());
        let method = if options.source_corner.is_some() {
            Method::BrushCorner
        } else {
            Method::Ray
        };
        let plan = self.plan_placement(&ids, method, Some(ray), &options)?;
        self.install_placement(actor, ids, options, plan, token)
    }

    pub fn surface_placement(&self) -> Value {
        let Some(draft) = &self.placement.draft else {
            return json!({"token":null,"valid":false,"picking":false});
        };
        let error = self.checked_placement(draft.token).err();
        let plan=draft.plan.as_ref().map(|plan|json!({"method":plan.method.name(),"target":object_id(plan.surface.hit.object),"solid":plan.surface.hit.solid,"side":plan.surface.hit.side,"point":plan.surface.hit.point.to_array(),"normal":plan.surface.normal.to_array(),"pivot":plan.transform.pivot.to_array(),"translation":plan.transform.translation.to_array(),"rotation":plan.transform.rotation.to_array(),"contact_offset":plan.corner.is_none().then_some(plan.contact),"corner":plan.corner.as_ref().map(|corner|json!({"source":corner.source.to_array(),"target":corner.target.report(),"position":corner.position.to_array()})),"support_points":plan.support_points}));
        json!({"token":draft.token,"valid":error.is_none(),"picking":draft.plan.is_none() && error.is_none(),"method":if draft.options.source_corner.is_some(){"brush_corner"}else{draft.plan.as_ref().map_or("ray",|plan|plan.method.name())},"source_corner":draft.options.source_corner.as_ref().map(super::components::component_json),"source_position":draft.source_position.map(|point|point.to_array()),"ids":draft.ids.iter().copied().map(object_id).collect::<Vec<_>>(),"offset":draft.options.offset,"align":draft.options.align,"texture_lock":draft.options.texture_lock,"plan":plan,"error":error})
    }

    pub fn cancel_surface_placement(&mut self, token: u64) -> Result<(), String> {
        if !self
            .placement
            .draft
            .as_ref()
            .is_some_and(|draft| draft.token == token)
        {
            return Err("Surface placement was closed or replaced".into());
        }
        if self.surface_transform_owner() == Some(token) {
            self.cancel_transform_preview();
        }
        self.placement.draft = None;
        self.view_serial += 1;
        Ok(())
    }

    pub fn apply_surface_placement(&mut self, actor: Actor, token: u64) -> Result<Value, String> {
        if self.checked_placement(token)?.plan.is_none() {
            return Err("Pick a surface before applying placement".into());
        }
        let report = self.surface_placement();
        self.commit_transform_preview(actor)?;
        self.placement.draft = None;
        Ok(report)
    }

    pub(super) fn append_surface_placement_guide(&self, mesh: &mut render::SceneMesh) {
        let Some(draft) = &self.placement.draft else {
            return;
        };
        if self.checked_placement(draft.token).is_err() {
            return;
        }
        let Some(plan) = &draft.plan else {
            return;
        };
        if !self.working_object_visible(self.render_document(), plan.surface.hit.object)
            || (plan.corner.is_some() && !self.working_selection_visible())
        {
            return;
        }
        if let Some(corner) = &plan.corner {
            for position in [corner.source, corner.position] {
                for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
                    for point in [position - axis * 3.0, position + axis * 3.0] {
                        mesh.lines.push(render::scene::LineVertex {
                            position: point.as_vec3().to_array(),
                            color: [255, 220, 95, 255],
                        });
                    }
                }
            }
            for point in [corner.source, corner.position] {
                mesh.lines.push(render::scene::LineVertex {
                    position: point.as_vec3().to_array(),
                    color: [255, 220, 95, 255],
                });
            }
            return;
        }
        let point = plan.surface.hit.point;
        for end in [point, point + plan.surface.normal * 24.0] {
            mesh.lines.push(render::scene::LineVertex {
                position: end.as_vec3().to_array(),
                color: [255, 220, 95, 255],
            });
        }
        for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
            for sign in [-1.0, 1.0] {
                mesh.lines.push(render::scene::LineVertex {
                    position: (point + axis * sign * 3.0).as_vec3().to_array(),
                    color: [255, 220, 95, 255],
                });
            }
        }
    }
}

pub(super) fn contact_plan(
    points: Vec<SupportPoint>,
    up: DVec3,
    method: Method,
    ray: Ray,
    surface: SurfaceHit,
    options: &Options,
    texture_scale_lock: Option<bool>,
) -> Result<Plan, String> {
    let (low, high) = support_bounds(&points);
    let pivot = (low + high) * 0.5;
    let normal = surface.normal;
    let rotation = if options.align {
        DQuat::from_rotation_arc(up, normal)
    } else {
        DQuat::IDENTITY
    };
    let minimum = points
        .iter()
        .map(|point| normal.dot(rotation * (point.point - pivot) + point.helper_offset))
        .fold(f64::INFINITY, f64::min);
    let translation = if method == Method::Ray {
        surface.hit.point + normal * (options.offset - minimum) - pivot
    } else {
        let dot = normal.dot(ray.direction);
        if dot.abs() < 1e-6 {
            return Err(
                "Surface is parallel to the drop direction; pick a different surface".into(),
            );
        }
        ray.direction
            * ((normal.dot(surface.hit.point) + options.offset - normal.dot(pivot) - minimum) / dot)
    };
    let (z, y, x) = rotation.to_euler(EulerRot::ZYX);
    let clean = |value: f64| if value.abs() < 1e-9 { 0.0 } else { value };
    let transform = Transform {
        translation: translation.map(clean),
        rotation: DVec3::new(x, y, z).map(|angle| clean(angle.to_degrees())),
        pivot,
        texture_lock: options.texture_lock,
        texture_scale_lock,
        ..Transform::default()
    };
    let contact =
        normal.dot(pivot + transform.translation) + minimum - normal.dot(surface.hit.point);
    Ok(Plan {
        method,
        surface,
        transform,
        support_points: points.len(),
        contact,
        corner: None,
    })
}

fn support_bounds(points: &[SupportPoint]) -> (DVec3, DVec3) {
    points
        .iter()
        .map(|point| point.point + point.helper_offset)
        .fold(
            (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY)),
            |(low, high), point| (low.min(point), high.max(point)),
        )
}
