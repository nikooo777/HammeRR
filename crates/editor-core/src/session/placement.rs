use std::collections::{BTreeMap, BTreeSet, HashSet};

use glam::{DQuat, DVec3, EulerRot};
use map_core::{
    BrushGeometry, DisplacementMesh, ObjectId,
    edit::Transform,
    query::{self, Ray, SurfaceHit},
};
use serde_json::{Value, json};
use source_formats::vmf::Solid;

use super::{Actor, Session};

const MAX_POINTS: usize = 1_000_000;

#[cfg(any(test, feature = "test-support"))]
pub(crate) mod fixtures;

#[cfg(test)]
pub(crate) mod tests;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Floor,
    Ceiling,
    Ray,
}

impl Method {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "floor" => Ok(Self::Floor),
            "ceiling" => Ok(Self::Ceiling),
            "ray" => Ok(Self::Ray),
            _ => Err("Choose floor, ceiling or ray placement".into()),
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Floor => "floor",
            Self::Ceiling => "ceiling",
            Self::Ray => "ray",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub align: bool,
    pub offset: f64,
    pub texture_lock: bool,
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
}

struct Plan {
    method: Method,
    surface: SurfaceHit,
    transform: Transform,
    support_points: usize,
    contact: f64,
}

struct SupportPoint {
    point: DVec3,
    helper_offset: DVec3,
}

fn object_id(id: ObjectId) -> String {
    match id {
        ObjectId::Solid(id) => format!("solid:{id}"),
        ObjectId::Entity(id) => format!("entity:{id}"),
    }
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

    fn placement_points(&self, ids: &[ObjectId]) -> Result<Vec<SupportPoint>, String> {
        if ids.is_empty() {
            return Err("Select objects to place".into());
        }
        let models = self
            .model_instances(&self.document)
            .instances
            .into_iter()
            .map(|instance| (instance.id, instance))
            .collect::<BTreeMap<_, _>>();
        let hidden = self
            .document
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
                        self.document.solid(id).ok_or("Selected brush is missing")?,
                    )?;
                }
                ObjectId::Entity(id) => {
                    let entity = self
                        .document
                        .entity(id)
                        .ok_or("Selected entity is missing")?;
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
                        let (low, high) = query::entity_bounds(entity)
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

    fn placement_surface(&mut self, ids: &[ObjectId], ray: Ray) -> Result<SurfaceHit, String> {
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
        ordinary
            .into_iter()
            .chain(
                self.working_evaluation(&self.document)
                    .models
                    .instances
                    .iter()
                    .filter(|instance| {
                        !excluded.contains(&instance.id)
                            && self.working_object_visible(&self.document, instance.id)
                    })
                    .filter_map(|instance| instance.pick_surface(&ray)),
            )
            .min_by(|a, b| a.hit.distance.total_cmp(&b.hit.distance))
            .ok_or("The ray did not hit a visible brush, terrain sheet or supported model".into())
    }

    fn plan_placement(
        &mut self,
        ids: &[ObjectId],
        method: Method,
        ray: Option<Ray>,
        options: Options,
    ) -> Result<Plan, String> {
        if !options.offset.is_finite() || options.offset.abs() > 65536.0 {
            return Err("Surface offset must be finite and within ±65536 units".into());
        }
        if method != Method::Ray && ray.is_some() {
            return Err("Ray coordinates are only used for picked-surface placement".into());
        }
        let points = self.placement_points(ids)?;
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
        };
        let surface = self.placement_surface(ids, ray)?;
        let normal = surface.normal;
        let up = self.transform_basis(ids, true)?.z_axis;
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
                * ((normal.dot(surface.hit.point) + options.offset - normal.dot(pivot) - minimum)
                    / dot)
        };
        let (z, y, x) = rotation.to_euler(EulerRot::ZYX);
        let clean = |value: f64| if value.abs() < 1e-9 { 0.0 } else { value };
        let transform = Transform {
            translation: translation.map(clean),
            rotation: DVec3::new(x, y, z).map(|angle| clean(angle.to_degrees())),
            pivot,
            texture_lock: options.texture_lock,
            texture_scale_lock: self.transform_tool.texture_scale_lock,
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
        })
    }

    fn install_placement(
        &mut self,
        actor: Actor,
        ids: Vec<ObjectId>,
        options: Options,
        plan: Plan,
        token: u64,
    ) -> Result<Value, String> {
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
        let plan = self.plan_placement(&ids, method, ray, options)?;
        self.install_placement(actor, ids, options, plan, self.placement.serial + 1)
    }

    pub fn begin_surface_placement_pick(
        &mut self,
        ids: &[ObjectId],
        options: Options,
    ) -> Result<Value, String> {
        self.placement_context()?;
        self.placement_points(ids)?;
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
        let (ids, options) = (draft.ids.clone(), draft.options);
        let plan = self.plan_placement(&ids, Method::Ray, Some(ray), options)?;
        self.install_placement(actor, ids, options, plan, token)
    }

    pub fn surface_placement(&self) -> Value {
        let Some(draft) = &self.placement.draft else {
            return json!({"token":null,"valid":false,"picking":false});
        };
        let error = self.checked_placement(draft.token).err();
        let plan=draft.plan.as_ref().map(|plan|json!({"method":plan.method.name(),"target":object_id(plan.surface.hit.object),"solid":plan.surface.hit.solid,"side":plan.surface.hit.side,"point":plan.surface.hit.point.to_array(),"normal":plan.surface.normal.to_array(),"pivot":plan.transform.pivot.to_array(),"translation":plan.transform.translation.to_array(),"rotation":plan.transform.rotation.to_array(),"contact_offset":plan.contact,"support_points":plan.support_points}));
        json!({"token":draft.token,"valid":error.is_none(),"picking":draft.plan.is_none() && error.is_none(),"ids":draft.ids.iter().copied().map(object_id).collect::<Vec<_>>(),"offset":draft.options.offset,"align":draft.options.align,"texture_lock":draft.options.texture_lock,"plan":plan,"error":error})
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
        if !self.working_object_visible(self.render_document(), plan.surface.hit.object) {
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
