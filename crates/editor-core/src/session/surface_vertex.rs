use std::cell::RefCell;

use glam::{DQuat, DVec3};
use map_core::{
    Document, ObjectId, query,
    subobject::{self, Component},
    surface::FaceId,
};
use render::models::{Instance, MeshSurfaceHit};
use serde::{Serialize, Serializer};
use serde_json::{Value, json};
use source_formats::vmf::Solid;

use super::Session;

pub(super) const CORNER_SNAP_ROUNDOFF_ULPS: f64 = 4.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Reference {
    scope: String,
    generation: u64,
    owner: ObjectId,
    target: Target,
}

#[derive(Clone, Debug, PartialEq)]
enum Target {
    Brush {
        component: Component,
        hit_side: u32,
    },
    Terrain {
        vertex: query::TerrainVertex,
        cage_corners: [Vec<u32>; 4],
    },
    Model(ModelPick),
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct ModelPick {
    entity: ObjectId,
    path: String,
    identity: Option<[u8; 32]>,
    generation: u64,
    content: u64,
    checksum: i32,
    body: u32,
    skin: usize,
    scale: f32,
    origin: DVec3,
    rotation: DQuat,
    part: usize,
    triangle: usize,
    vertex: u32,
}

impl ModelPick {
    pub(super) fn new(
        instance: &Instance,
        hit: MeshSurfaceHit,
        generation: u64,
        content: u64,
    ) -> Self {
        Self {
            entity: instance.id,
            path: instance.asset.path.clone(),
            identity: instance.asset.source_identity(),
            generation,
            content,
            checksum: instance.asset.model.header.checksum,
            body: instance.body,
            skin: instance.skin,
            scale: instance.scale,
            origin: instance.origin,
            rotation: instance.rotation,
            part: hit.part,
            triangle: hit.triangle,
            vertex: hit.vertex,
        }
    }
}

pub(super) struct PickedSurface {
    pub surface: query::SurfaceHit,
    pub model: Option<ModelPick>,
}

impl Reference {
    pub fn object(&self) -> ObjectId {
        self.owner
    }

    pub fn report(&self) -> Value {
        let mut report = match &self.target {
            Target::Brush {
                component,
                hit_side,
            } => {
                let mut report = super::components::component_json(component);
                report["kind"] = json!("brush_corner");
                report["hit_side"] = json!(hit_side);
                report
            }
            Target::Terrain {
                vertex,
                cage_corners,
            } => json!({
                "kind":"terrain_vertex", "solid":vertex.face.solid, "side":vertex.face.side,
                "triangle":vertex.triangle, "index":vertex.index, "grid_size":vertex.grid_size,
                "row":vertex.index / vertex.grid_size, "column":vertex.index % vertex.grid_size,
                "cage_corners":cage_corners,"provenance":"prepared_surface"
            }),
            Target::Model(target) => json!({
                "kind":"model_vertex", "entity":crate::identity::id_string(target.entity),
                "model":target.path,"checksum":target.checksum,"body":target.body,"skin":target.skin,
                "scale":target.scale,"origin":target.origin.to_array(),"rotation":target.rotation.to_array(),
                "pose_scope":"captured_at_pick",
                "part":target.part,"triangle":target.triangle,"vertex":target.vertex,
                "source_identity":target.identity.map(|hash|hash.iter().map(|byte|format!("{byte:02x}")).collect::<String>()),
                "asset_generation":target.generation,"content_epoch":target.content,"provenance":"cached_decoded_model_mesh"
            }),
        };
        report["owner"] = json!(crate::identity::id_string(self.owner));
        report["generation"] = json!(self.generation);
        report
    }
}

impl Serialize for Reference {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.report().serialize(serializer)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Key {
    generation: u64,
    revision: u64,
    content: u64,
    definitions: u64,
    models: u64,
}

struct Cached {
    key: Key,
    reference: Reference,
    result: Result<DVec3, String>,
}

#[derive(Default)]
pub(super) struct State {
    cached: RefCell<Option<Cached>>,
}

fn visible_solid(document: &Document, id: u32, expected_owner: ObjectId) -> Result<&Solid, String> {
    let (owner, solid) = subobject::solid(document, id).map_err(|error| error.to_string())?;
    if owner != expected_owner {
        return Err("Surface vertex brush ownership changed; attach the vertex again".into());
    }
    if solid.hidden
        || matches!(owner,ObjectId::Entity(id) if document.entity(id).is_none_or(|entity|entity.hidden))
    {
        return Err("Saved-hidden surface vertex owners are unavailable".into());
    }
    Ok(solid)
}

fn terrain_mesh(document: &Document, face: FaceId) -> Result<map_core::DisplacementMesh, String> {
    let mut prepared = map_core::displacement::surface::prepare(document.vmf());
    if let Some(error) = prepared.issues.remove(&face) {
        return Err(error);
    }
    prepared
        .meshes
        .remove(&face)
        .ok_or_else(|| "Terrain vertex surface is unavailable".into())
}

fn cage_corners(
    solid: &Solid,
    face: FaceId,
    mesh: &map_core::DisplacementMesh,
) -> Result<[Vec<u32>; 4], String> {
    let topology = subobject::topology(solid).map_err(|error| error.to_string())?;
    let boundary = topology
        .faces
        .iter()
        .find(|candidate| candidate.side == face.side)
        .filter(|face| face.vertices.len() == 4)
        .ok_or("Terrain vertex requires an unambiguous quad cage")?;
    let n = mesh.size;
    if ![5, 9, 17].contains(&n) || mesh.base_positions.len() != n * n {
        return Err("Terrain vertex grid is invalid".into());
    }
    let mut result = Vec::with_capacity(4);
    for index in [0, (n - 1) * n, n * n - 1, n - 1] {
        let point = mesh.base_positions[index];
        let mut matches = boundary
            .vertices
            .iter()
            .map(|&index| &topology.vertices[index])
            .filter(|vertex| {
                vertex
                    .position
                    .abs_diff_eq(point, map_core::plane::ON_EPSILON)
            });
        let corner = matches
            .next()
            .ok_or("Terrain grid corner has no cage identity")?;
        if matches.next().is_some() {
            return Err("Terrain grid corner has an ambiguous cage identity".into());
        }
        result.push(corner.sides.clone());
    }
    result
        .try_into()
        .map_err(|_| "Terrain grid corner identities are unavailable".into())
}

impl Session {
    pub(super) fn capture_surface_vertex(
        &self,
        picked: PickedSurface,
    ) -> Result<(Reference, DVec3), String> {
        let PickedSurface { surface, model } = picked;
        let owner = surface.hit.object;
        let target = if let Some(model) = model {
            Target::Model(model)
        } else if let Some(vertex) = surface.terrain_vertex {
            let solid = visible_solid(&self.document, vertex.face.solid, owner)?;
            let mesh = terrain_mesh(&self.document, vertex.face)?;
            Target::Terrain {
                vertex,
                cage_corners: cage_corners(solid, vertex.face, &mesh)?,
            }
        } else {
            let (component, _) = query::face_corner(&self.document, &surface.hit)
                .ok_or("The first visible hit has no unambiguous surface vertex")?;
            Target::Brush {
                component,
                hit_side: surface.hit.side.ok_or("Brush vertex has no source face")?,
            }
        };
        let reference = Reference {
            scope: self.buffer_review_scope.clone(),
            generation: self.document_generation,
            owner,
            target,
        };
        let point = self.resolve_surface_vertex(&reference)?;
        Ok((reference, point))
    }

    pub fn resolve_surface_vertex(&self, reference: &Reference) -> Result<DVec3, String> {
        let key = self.surface_vertex_key();
        if let Some(cached) = self.surface_vertices.cached.borrow().as_ref()
            && cached.key == key
            && cached.reference == *reference
        {
            return cached.result.clone();
        }
        let result = self.resolve_surface_vertex_uncached(reference);
        *self.surface_vertices.cached.borrow_mut() = Some(Cached {
            key: self.surface_vertex_key(),
            reference: reference.clone(),
            result: result.clone(),
        });
        result
    }

    fn surface_vertex_key(&self) -> Key {
        Key {
            generation: self.document_generation,
            revision: self.document.revision(),
            content: self.content_epoch,
            definitions: self.definitions_serial(),
            models: self.models.borrow().generation(),
        }
    }

    pub(super) fn surface_vertex_context_stamp(&self) -> String {
        let key = self.surface_vertex_key();
        format!(
            "surface-vertex:{}:{}:{}:{}:{}",
            key.generation, key.revision, key.content, key.definitions, key.models
        )
    }

    fn resolve_surface_vertex_uncached(&self, reference: &Reference) -> Result<DVec3, String> {
        if reference.scope != self.buffer_review_scope
            || reference.generation != self.document_generation
        {
            return Err("Surface vertex belongs to another map; attach it again".into());
        }
        let point = match &reference.target {
            Target::Brush {
                component,
                hit_side,
            } => {
                let solid = visible_solid(&self.document, component.solid, reference.owner)?;
                if solid
                    .sides
                    .iter()
                    .find(|side| side.id == *hit_side)
                    .is_none_or(|side| side.displacement.is_some())
                {
                    return Err("Brush vertex source face changed; attach it again".into());
                }
                let topology = subobject::topology(solid).map_err(|error| error.to_string())?;
                let vertices = topology
                    .resolve(&component.sides)
                    .map_err(|error| error.to_string())?;
                if vertices.len() != 1 {
                    return Err("Surface anchor is no longer one brush vertex".into());
                }
                precise_corner(solid, component, topology.vertices[vertices[0]].position)
                    .ok_or("Surface vertex planes have no precise finite intersection")?
            }
            Target::Terrain {
                vertex,
                cage_corners: captured,
            } => {
                let solid = visible_solid(&self.document, vertex.face.solid, reference.owner)?;
                let mesh = terrain_mesh(&self.document, vertex.face)?;
                if mesh.size != vertex.grid_size
                    || cage_corners(solid, vertex.face, &mesh)? != *captured
                {
                    return Err(
                        "Terrain vertex grid or cage topology changed; attach it again".into(),
                    );
                }
                *mesh
                    .positions
                    .get(vertex.index)
                    .ok_or("Terrain vertex index is no longer available")?
            }
            Target::Model(target) => {
                if target.generation != self.models.borrow().reload_generation() {
                    return Err("Model assets were reloaded; attach the vertex again".into());
                }
                if target.content != self.content_epoch {
                    return Err("Model asset context changed; attach the vertex again".into());
                }
                let ObjectId::Entity(id) = reference.owner else {
                    return Err("Model vertex requires an entity owner".into());
                };
                let mut entities = self
                    .document
                    .vmf()
                    .entities
                    .iter()
                    .filter(|entity| entity.id == id);
                let entity = entities.next().ok_or("Model vertex entity was removed")?;
                if entities.next().is_some() {
                    return Err("Model vertex entity has an ambiguous duplicate ID".into());
                }
                if entity.hidden {
                    return Err("Saved-hidden surface vertex owners are unavailable".into());
                }
                if !entity.solids.is_empty()
                    || crate::sprite_resource::is_source(
                        entity
                            .classname()
                            .and_then(|name| self.definitions().class(name)),
                    )
                {
                    return Err("Surface vertex owner no longer has an authored model mesh".into());
                }
                let path = render::models::model_path(
                    entity
                        .properties
                        .get("model")
                        .ok_or("Model vertex owner has no model")?,
                )?;
                if path != target.path {
                    return Err("Surface vertex model changed; attach it again".into());
                }
                let asset = self.models.borrow_mut().get(&self.game, &path)?;
                if target.identity.is_none() || asset.source_identity() != target.identity {
                    return Err("Model source bytes changed; attach the vertex again".into());
                }
                let instance = Instance::new(entity, asset)?;
                if instance.body != target.body {
                    return Err("Model body changed; attach the vertex again".into());
                }
                let part = instance
                    .asset
                    .model
                    .indexed_parts_for_body(instance.body)
                    .find_map(|(index, part)| (index == target.part).then_some(part))
                    .ok_or("Model vertex part is no longer selected")?;
                let triangle = part
                    .indices
                    .as_chunks::<3>()
                    .0
                    .get(target.triangle)
                    .ok_or("Model vertex triangle is unavailable")?;
                if !triangle.contains(&target.vertex) {
                    return Err("Model vertex no longer belongs to its captured triangle".into());
                }
                let vertex = instance
                    .asset
                    .model
                    .vertices
                    .get(target.vertex as usize)
                    .ok_or("Model vertex index is unavailable")?;
                instance.point(vertex.position)
            }
        };
        crate::pivot_preferences::validate_point(point)?;
        Ok(point)
    }
}

pub(super) fn precise_corner(
    solid: &Solid,
    component: &Component,
    approximate: DVec3,
) -> Option<DVec3> {
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
