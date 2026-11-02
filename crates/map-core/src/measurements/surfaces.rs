use std::collections::{BTreeSet, HashSet};

use glam::DVec3;
use source_formats::keyvalues::Node;
use source_formats::vmf::{Solid, Vmf, parse_vec3};

use super::{Endpoint, KEY, Measurement, exact_point, field, validate_point};
use crate::query::SurfaceHit;
use crate::surface::FaceId;
use crate::{Document, ObjectId, displacement, subobject};

const WEIGHT_EPSILON: f64 = 1e-9;
const WEIGHT_SUM_EPSILON: f64 = 64.0 * f64::EPSILON;
const MAX_ID_LIST_BYTES: usize = subobject::MAX_SIDES * 11;

#[derive(Clone, Debug, PartialEq)]
pub struct SurfaceReference {
    pub owner: ObjectId,
    pub face: FaceId,
    pub weights: [f64; 3],
    pub target: SurfaceTarget,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SurfaceTarget {
    Brush {
        vertex_sides: [Vec<u32>; 3],
    },
    Terrain {
        triangle: usize,
        grid_size: usize,
        indices: [usize; 3],
        cage_corners: [Vec<u32>; 4],
    },
}

impl Endpoint {
    pub(super) fn surface_key(self) -> &'static str {
        match self {
            Self::Start => "start_surface",
            Self::End => "end_surface",
        }
    }
}

fn owner_text(owner: ObjectId) -> String {
    match owner {
        ObjectId::Solid(id) => format!("solid:{id}"),
        ObjectId::Entity(id) => format!("entity:{id}"),
    }
}

fn parse_owner(text: &str) -> Result<ObjectId, String> {
    let (kind, id) = text.split_once(':').ok_or("Invalid surface owner")?;
    let id = id
        .parse::<u32>()
        .ok()
        .filter(|id| *id != 0)
        .ok_or("Invalid surface owner ID")?;
    match kind {
        "solid" => Ok(ObjectId::Solid(id)),
        "entity" => Ok(ObjectId::Entity(id)),
        _ => Err("Surface owner must be a brush or entity".into()),
    }
}

fn id_list(nodes: &[Node], key: &str) -> Result<Vec<u32>, String> {
    let value = field(nodes, key)?;
    if value.len() > MAX_ID_LIST_BYTES {
        return Err("Surface vertex identity exceeds the face limit".into());
    }
    let ids = value
        .split_whitespace()
        .map(|value| {
            value
                .parse::<u32>()
                .map_err(|_| "Invalid surface face ID".to_owned())
        })
        .collect::<Result<Vec<_>, _>>()?;
    validate_sides(&ids)?;
    Ok(ids)
}

fn validate_sides(sides: &[u32]) -> Result<(), String> {
    if !(3..=subobject::MAX_SIDES).contains(&sides.len())
        || sides.contains(&0)
        || sides.windows(2).any(|pair| pair[0] >= pair[1])
    {
        return Err(
            "Surface vertex identity requires 3–64 sorted, unique positive face IDs".into(),
        );
    }
    Ok(())
}

fn ids_text(ids: &[u32]) -> String {
    ids.iter().map(u32::to_string).collect::<Vec<_>>().join(" ")
}

fn validate_weights(weights: [f64; 3]) -> Result<(), String> {
    if weights
        .iter()
        .any(|weight| !weight.is_finite() || !(0.0..=1.0).contains(weight))
        || (weights.iter().sum::<f64>() - 1.0).abs() > WEIGHT_SUM_EPSILON
    {
        return Err("Surface attachment weights must be finite, nonnegative and sum to one".into());
    }
    Ok(())
}

fn weighted_point(points: [DVec3; 3], weights: [f64; 3]) -> Result<DVec3, String> {
    validate_weights(weights)?;
    let normal = (points[1] - points[0]).cross(points[2] - points[0]);
    if !normal.is_finite() || normal.try_normalize().is_none() {
        return Err("Surface attachment triangle collapsed".into());
    }
    let point =
        points[0] + (points[1] - points[0]) * weights[1] + (points[2] - points[0]) * weights[2];
    validate_point(point)?;
    Ok(point)
}

fn barycentric(points: [DVec3; 3], point: DVec3) -> Option<[f64; 3]> {
    let a = points[1] - points[0];
    let b = points[2] - points[0];
    let normal = a.cross(b);
    let unit = normal.try_normalize()?;
    if unit.dot(point - points[0]).abs() > crate::plane::ON_EPSILON {
        return None;
    }
    let axis = (0..3).max_by(|&a, &b| normal[a].abs().total_cmp(&normal[b].abs()))?;
    let delta = point - points[0];
    let u = delta.cross(b)[axis] / normal[axis];
    let v = a.cross(delta)[axis] / normal[axis];
    let weights = [1.0 - u - v, u, v];
    if weights.iter().any(|weight| {
        !weight.is_finite() || *weight < -WEIGHT_EPSILON || *weight > 1.0 + WEIGHT_EPSILON
    }) {
        return None;
    }
    let mut weights = weights.map(|weight| weight.clamp(0.0, 1.0));
    let sum = weights.iter().sum::<f64>();
    for weight in &mut weights {
        *weight /= sum;
    }
    let reconstructed = weighted_point(points, weights).ok()?;
    reconstructed
        .abs_diff_eq(point, crate::plane::ON_EPSILON)
        .then_some(weights)
}

fn surface_solid(vmf: &Vmf, owner: ObjectId, face: FaceId) -> Result<&Solid, String> {
    let mut matches = vmf
        .world
        .solids
        .iter()
        .map(|solid| (ObjectId::Solid(solid.id), solid))
        .chain(vmf.entities.iter().flat_map(|entity| {
            entity
                .solids
                .iter()
                .map(move |solid| (ObjectId::Entity(entity.id), solid))
        }))
        .filter(|(_, solid)| solid.id == face.solid);
    let (actual, solid) = matches.next().ok_or("Attached surface brush is missing")?;
    if matches.next().is_some() || vmf.object_ids().filter(|id| *id == face.solid).count() != 1 {
        return Err("Attached surface brush has an ambiguous duplicate ID".into());
    }
    if actual != owner {
        return Err("Attached surface brush ownership changed".into());
    }
    if let ObjectId::Entity(id) = owner {
        let mut owners = vmf.entities.iter().filter(|entity| entity.id == id);
        let entity = owners.next().ok_or("Attached surface owner is missing")?;
        if owners.next().is_some()
            || vmf
                .object_ids()
                .filter(|candidate| *candidate == id)
                .count()
                != 1
        {
            return Err("Attached surface owner has an ambiguous duplicate ID".into());
        }
        if entity.hidden {
            return Err("Saved-hidden surface owners are unavailable".into());
        }
    }
    if solid.hidden {
        return Err("Saved-hidden surface owners are unavailable".into());
    }
    if solid
        .sides
        .iter()
        .filter(|side| side.id == face.side)
        .count()
        != 1
    {
        return Err("Attached surface face is missing or ambiguous".into());
    }
    Ok(solid)
}

impl SurfaceReference {
    pub fn capture(document: &Document, surface: SurfaceHit) -> Result<Self, String> {
        validate_point(surface.hit.point)?;
        let face = FaceId {
            solid: surface
                .hit
                .solid
                .ok_or("Surface attachments require an authored brush or terrain hit")?,
            side: surface
                .hit
                .side
                .ok_or("Surface attachments require an authored brush or terrain face")?,
        };
        let owner = surface.hit.object;
        let solid = surface_solid(document.vmf(), owner, face)?;
        let side = solid
            .sides
            .iter()
            .find(|side| side.id == face.side)
            .unwrap();
        let reference = if side.displacement.is_some() {
            let vertex = surface
                .terrain_vertex
                .ok_or("Terrain attachment has no prepared triangle")?;
            if vertex.face != face {
                return Err("Terrain attachment source face changed".into());
            }
            let mut resolver = Resolver::new(document.vmf());
            let mesh = resolver.terrain_mesh(face)?;
            if mesh.size != vertex.grid_size {
                return Err("Terrain attachment grid changed".into());
            }
            let indices = displacement::triangles::indices((mesh.size - 1).ilog2())
                .map_err(|error| error.to_string())?
                .get(vertex.triangle)
                .copied()
                .ok_or("Terrain attachment triangle is unavailable")?;
            let weights = barycentric(
                indices.map(|index| mesh.positions[index]),
                surface.hit.point,
            )
            .ok_or("Picked point is outside the prepared terrain triangle")?;
            Self {
                owner,
                face,
                weights,
                target: SurfaceTarget::Terrain {
                    triangle: vertex.triangle,
                    grid_size: mesh.size,
                    indices,
                    cage_corners: subobject::terrain_cage_corners(solid, face, mesh)
                        .map_err(|error| error.to_string())?,
                },
            }
        } else {
            let topology = subobject::topology(solid).map_err(|error| error.to_string())?;
            let boundary = topology
                .faces
                .iter()
                .find(|candidate| candidate.side == face.side)
                .ok_or("Brush attachment face is unavailable")?;
            let mut triangle = None;
            for pair in boundary.vertices[1..].windows(2) {
                let vertices =
                    [boundary.vertices[0], pair[0], pair[1]].map(|index| &topology.vertices[index]);
                if let Some(weights) =
                    barycentric(vertices.map(|vertex| vertex.position), surface.hit.point)
                {
                    triangle = Some((vertices.map(|vertex| vertex.sides.clone()), weights));
                    break;
                }
            }
            let (vertex_sides, weights) =
                triangle.ok_or("Picked point is outside the authored brush face")?;
            Self {
                owner,
                face,
                weights,
                target: SurfaceTarget::Brush { vertex_sides },
            }
        };
        reference.validate()?;
        Resolver::new(document.vmf()).surface(&reference)?;
        Ok(reference)
    }

    fn validate(&self) -> Result<(), String> {
        if self.face.solid == 0
            || self.face.side == 0
            || matches!(self.owner, ObjectId::Solid(id) if id != self.face.solid)
            || matches!(self.owner, ObjectId::Entity(0))
        {
            return Err(
                "Surface attachment requires positive, consistent owner/brush/face IDs".into(),
            );
        }
        validate_weights(self.weights)?;
        let keys: &[Vec<u32>] = match &self.target {
            SurfaceTarget::Brush { vertex_sides } => vertex_sides,
            SurfaceTarget::Terrain {
                triangle,
                grid_size,
                indices,
                cage_corners,
            } => {
                if ![5, 9, 17].contains(grid_size)
                    || *triangle >= (grid_size - 1) * (grid_size - 1) * 2
                    || indices.iter().any(|index| *index >= grid_size * grid_size)
                    || indices.iter().copied().collect::<HashSet<_>>().len() != 3
                {
                    return Err("Terrain attachment triangle/grid identity is invalid".into());
                }
                cage_corners
            }
        };
        for (index, sides) in keys.iter().enumerate() {
            validate_sides(sides)?;
            if !sides.contains(&self.face.side) || keys[..index].contains(sides) {
                return Err(
                    "Surface attachment vertex keys must be distinct members of its face".into(),
                );
            }
        }
        Ok(())
    }

    fn parse(node: &Node) -> Result<Self, String> {
        let nodes = node
            .children()
            .ok_or("Surface attachment must be a block")?;
        let target = match field(nodes, "kind")? {
            "brush_surface" => SurfaceTarget::Brush {
                vertex_sides: [
                    id_list(nodes, "vertex0_sides")?,
                    id_list(nodes, "vertex1_sides")?,
                    id_list(nodes, "vertex2_sides")?,
                ],
            },
            "terrain_surface" => {
                let indices = id_list_values(field(nodes, "indices")?)?;
                SurfaceTarget::Terrain {
                    triangle: field(nodes, "triangle")?
                        .parse()
                        .map_err(|_| "Invalid terrain triangle index")?,
                    grid_size: field(nodes, "grid_size")?
                        .parse()
                        .map_err(|_| "Invalid terrain grid size")?,
                    indices: indices
                        .try_into()
                        .map_err(|_| "Terrain triangle needs three vertex indices")?,
                    cage_corners: [
                        id_list(nodes, "cage0_sides")?,
                        id_list(nodes, "cage1_sides")?,
                        id_list(nodes, "cage2_sides")?,
                        id_list(nodes, "cage3_sides")?,
                    ],
                }
            }
            _ => return Err("Unsupported surface attachment kind; metadata is preserved".into()),
        };
        let reference = Self {
            owner: parse_owner(field(nodes, "owner")?)?,
            face: FaceId {
                solid: field(nodes, "solid")?
                    .parse()
                    .map_err(|_| "Invalid surface brush ID")?,
                side: field(nodes, "side")?
                    .parse()
                    .map_err(|_| "Invalid surface face ID")?,
            },
            weights: parse_vec3(field(nodes, "weights")?)
                .ok_or("Invalid surface interpolation weights")?
                .to_array(),
            target,
        };
        reference.validate()?;
        Ok(reference)
    }

    fn validate_owned_node(node: &Node) -> Result<(), String> {
        let reference = Self::parse(node)?;
        let common = ["kind", "owner", "solid", "side", "weights"];
        let target: &[&str] = match reference.target {
            SurfaceTarget::Brush { .. } => &["vertex0_sides", "vertex1_sides", "vertex2_sides"],
            SurfaceTarget::Terrain { .. } => &[
                "triangle",
                "grid_size",
                "indices",
                "cage0_sides",
                "cage1_sides",
                "cage2_sides",
                "cage3_sides",
            ],
        };
        if node.children().unwrap().iter().any(|field| {
            field.as_str().is_none()
                || (!common.contains(&field.key.as_str()) && !target.contains(&field.key.as_str()))
        }) {
            return Err("Unknown nested surface attachment metadata cannot be proven independent of removed IDs".into());
        }
        Ok(())
    }

    fn references_removed(&self, objects: &BTreeSet<u32>, sides: &BTreeSet<u32>) -> bool {
        let owner = match self.owner {
            ObjectId::Solid(id) | ObjectId::Entity(id) => id,
        };
        let keys: &[Vec<u32>] = match &self.target {
            SurfaceTarget::Brush { vertex_sides } => vertex_sides,
            SurfaceTarget::Terrain { cage_corners, .. } => cage_corners,
        };
        objects.contains(&owner)
            || objects.contains(&self.face.solid)
            || sides.contains(&self.face.side)
            || keys.iter().flatten().any(|side| sides.contains(side))
    }

    fn node(&self, endpoint: Endpoint) -> Node {
        let mut nodes = vec![
            Node::string("owner", owner_text(self.owner)),
            Node::string("solid", self.face.solid.to_string()),
            Node::string("side", self.face.side.to_string()),
            Node::string("weights", exact_point(DVec3::from_array(self.weights))),
        ];
        match &self.target {
            SurfaceTarget::Brush { vertex_sides } => {
                nodes.push(Node::string("kind", "brush_surface"));
                for (index, sides) in vertex_sides.iter().enumerate() {
                    nodes.push(Node::string(
                        format!("vertex{index}_sides"),
                        ids_text(sides),
                    ));
                }
            }
            SurfaceTarget::Terrain {
                triangle,
                grid_size,
                indices,
                cage_corners,
            } => {
                nodes.extend([
                    Node::string("kind", "terrain_surface"),
                    Node::string("triangle", triangle.to_string()),
                    Node::string("grid_size", grid_size.to_string()),
                    Node::string(
                        "indices",
                        indices
                            .iter()
                            .map(usize::to_string)
                            .collect::<Vec<_>>()
                            .join(" "),
                    ),
                ]);
                for (index, sides) in cage_corners.iter().enumerate() {
                    nodes.push(Node::string(format!("cage{index}_sides"), ids_text(sides)));
                }
            }
        }
        Node::block(endpoint.surface_key(), nodes)
    }
}

fn id_list_values(value: &str) -> Result<Vec<usize>, String> {
    if value.len() > MAX_ID_LIST_BYTES {
        return Err("Terrain triangle identity exceeds the supported limit".into());
    }
    value
        .split_whitespace()
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|_| "Invalid terrain vertex index".into())
        })
        .collect()
}

pub struct Resolver<'a> {
    vmf: &'a Vmf,
    terrain: Option<displacement::surface::Prepared>,
}

impl<'a> Resolver<'a> {
    pub fn new(vmf: &'a Vmf) -> Self {
        Self { vmf, terrain: None }
    }

    fn terrain_mesh(&mut self, face: FaceId) -> Result<&crate::DisplacementMesh, String> {
        let prepared = self
            .terrain
            .get_or_insert_with(|| displacement::surface::prepare(self.vmf));
        if let Some(error) = prepared.issues.get(&face) {
            return Err(error.clone());
        }
        prepared
            .get(&face)
            .ok_or_else(|| "Attached terrain surface is unavailable".into())
    }

    pub fn surface(&mut self, reference: &SurfaceReference) -> Result<DVec3, String> {
        reference.validate()?;
        let solid = surface_solid(self.vmf, reference.owner, reference.face)?;
        let side = solid
            .sides
            .iter()
            .find(|side| side.id == reference.face.side)
            .unwrap();
        let points = match &reference.target {
            SurfaceTarget::Brush { vertex_sides } => {
                if side.displacement.is_some() {
                    return Err("Attached ordinary brush face became terrain".into());
                }
                let topology = subobject::topology(solid).map_err(|error| error.to_string())?;
                let face = topology
                    .faces
                    .iter()
                    .find(|face| face.side == reference.face.side)
                    .ok_or("Attached brush face is unavailable")?;
                let mut points = [DVec3::ZERO; 3];
                for (point, sides) in points.iter_mut().zip(vertex_sides) {
                    let indices = topology.resolve(sides).map_err(|error| error.to_string())?;
                    if indices.len() != 1 || !face.vertices.contains(&indices[0]) {
                        return Err("Attached brush vertex/face topology changed".into());
                    }
                    *point = topology.vertices[indices[0]].position;
                }
                points
            }
            SurfaceTarget::Terrain {
                triangle,
                grid_size,
                indices,
                cage_corners,
            } => {
                let info = side
                    .displacement
                    .as_ref()
                    .ok_or("Attached terrain face is no longer terrain")?;
                let mesh = self.terrain_mesh(reference.face)?;
                if mesh.size != *grid_size
                    || subobject::terrain_cage_corners(solid, reference.face, mesh)
                        .map_err(|error| error.to_string())?
                        != *cage_corners
                {
                    return Err("Attached terrain grid or cage topology changed".into());
                }
                let triangles = displacement::triangles::inspect(info, mesh)
                    .map_err(|error| error.to_string())?;
                let current = triangles
                    .get(*triangle)
                    .ok_or("Attached terrain triangle is unavailable")?;
                if current.indices != *indices || current.removed || current.unknown_tags != 0 {
                    return Err("Attached terrain triangle topology is unavailable".into());
                }
                current.positions
            }
        };
        weighted_point(points, reference.weights)
    }

    pub fn endpoint(
        &mut self,
        measurement: &Measurement,
        endpoint: Endpoint,
    ) -> Result<DVec3, String> {
        let reference = measurement.surface_reference(endpoint)?;
        if reference.is_some() {
            if measurement.origin_reference(endpoint)?.is_some() {
                return Err(
                    "Measurement endpoints cannot combine entity and surface attachments".into(),
                );
            }
            measurement.local_offset(endpoint)?;
        }
        match reference {
            Some(reference) => self.surface(&reference),
            None => measurement.resolve_entity_endpoint(self.vmf, endpoint),
        }
    }

    pub fn measurement(&mut self, measurement: &Measurement) -> Result<Measurement, String> {
        let mut resolved = measurement.clone();
        resolved.start = self.endpoint(measurement, Endpoint::Start)?;
        resolved.end = self.endpoint(measurement, Endpoint::End)?;
        if resolved.start == resolved.end {
            return Err("Measurement endpoints must be distinct; attached points coincide".into());
        }
        Ok(resolved)
    }
}

impl Measurement {
    pub(super) fn validate_retained_references(
        &self,
        objects: &BTreeSet<u32>,
        sides: &BTreeSet<u32>,
    ) -> Result<(), String> {
        self.validate()?;
        for node in &self.extra {
            if super::origins::known_reference_key(&node.key) {
                continue;
            }
            if [Endpoint::Start.surface_key(), Endpoint::End.surface_key()]
                .contains(&node.key.as_str())
            {
                SurfaceReference::validate_owned_node(node)?;
            } else {
                return Err("Unknown measurement extension metadata cannot be proven independent of removed IDs".into());
            }
        }
        for endpoint in [Endpoint::Start, Endpoint::End] {
            if self
                .origin_reference(endpoint)?
                .is_some_and(|id| objects.contains(&id))
                || self
                    .surface_reference(endpoint)?
                    .is_some_and(|reference| reference.references_removed(objects, sides))
            {
                return Err(format!(
                    "Measurement {} references objects or faces being removed; detach or use a world endpoint first",
                    self.name
                ));
            }
        }
        Ok(())
    }

    pub fn surface_reference(
        &self,
        endpoint: Endpoint,
    ) -> Result<Option<SurfaceReference>, String> {
        let mut nodes = self
            .extra
            .iter()
            .filter(|node| node.key == endpoint.surface_key());
        let Some(node) = nodes.next() else {
            return Ok(None);
        };
        if nodes.next().is_some() {
            return Err(format!(
                "Duplicate measurement {} surface reference",
                endpoint.label()
            ));
        }
        SurfaceReference::parse(node).map(Some)
    }

    pub fn set_surface_reference(
        &mut self,
        endpoint: Endpoint,
        reference: SurfaceReference,
    ) -> Result<(), String> {
        reference.validate()?;
        self.set_origin_reference(endpoint, None);
        self.extra.push(reference.node(endpoint));
        Ok(())
    }

    pub(super) fn clear_surface_reference(&mut self, endpoint: Endpoint) {
        self.extra.retain(|node| node.key != endpoint.surface_key());
    }

    pub fn attached(&self, endpoint: Endpoint) -> Result<bool, String> {
        Ok(self.origin_reference(endpoint)?.is_some()
            || self.surface_reference(endpoint)?.is_some())
    }
}

fn surface_blocks(vmf: &Vmf) -> impl Iterator<Item = &[Node]> {
    vmf.extra
        .iter()
        .filter(|node| node.key == KEY)
        .filter_map(Node::children)
        .flatten()
        .filter(|node| node.key == "measurement")
        .filter_map(Node::children)
        .flatten()
        .filter(|node| {
            [Endpoint::Start.surface_key(), Endpoint::End.surface_key()]
                .contains(&node.key.as_str())
        })
        .filter_map(Node::children)
}

pub(super) fn reserved_object_ids(vmf: &Vmf) -> HashSet<u32> {
    let mut ids = HashSet::new();
    for nodes in surface_blocks(vmf) {
        for node in nodes {
            if node.key == "solid"
                && let Some(id) = node
                    .as_str()
                    .and_then(|value| value.parse().ok())
                    .filter(|id| *id != 0)
            {
                ids.insert(id);
            }
            if node.key == "owner"
                && let Some(owner) = node.as_str().and_then(|value| parse_owner(value).ok())
            {
                ids.insert(match owner {
                    ObjectId::Solid(id) | ObjectId::Entity(id) => id,
                });
            }
        }
    }
    ids
}

pub(crate) fn reserved_side_ids(vmf: &Vmf) -> HashSet<u32> {
    let mut ids = HashSet::new();
    for nodes in surface_blocks(vmf) {
        for node in nodes
            .iter()
            .filter(|node| node.key == "side" || node.key.ends_with("_sides"))
        {
            if let Some(value) = node.as_str() {
                ids.extend(
                    value
                        .split_whitespace()
                        .take(subobject::MAX_SIDES)
                        .filter_map(|value| value.parse::<u32>().ok())
                        .filter(|id| *id != 0),
                );
            }
        }
    }
    ids
}
