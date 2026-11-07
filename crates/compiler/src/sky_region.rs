use std::collections::BTreeSet;
use std::mem::size_of;

use glam::DVec3;
use map_core::ObjectId;
use source_formats::bsp::contents;
use source_formats::gamefs::GameFs;
use source_formats::vmf::{Entity, Solid, Vmf};

use crate::map::{Map, MapEntity, PreparedWorld, RootSide};
use crate::tree::Tree;
use crate::winding::ON_EPSILON;
use crate::{Control, Report, faces, portals, tree};

mod static_props;
mod terrain;

pub use map_core::fog::MAX_SKY_SCALE as MAX_SCALE;
pub use static_props::{
    CPU_RESERVATION_BYTES as PROP_CPU_RESERVATION_BYTES,
    MAX_LOGICAL_PATHS as MAX_PROP_LOGICAL_PATHS, MAX_LOOKUP_ROWS as MAX_PROP_LOOKUP_ROWS,
    MAX_MATERIAL_BYTES as MAX_PROP_MATERIAL_BYTES,
    MAX_METADATA_BYTES as MAX_PROP_SOURCE_METADATA_BYTES, MAX_PARSE_BYTES as MAX_PROP_PARSE_BYTES,
    MAX_PHYSICAL_PATHS as MAX_PROP_PHYSICAL_PATHS, MAX_SOURCE_BYTES as MAX_PROP_SOURCE_BYTES,
    MAX_VERTICES as MAX_PROP_VERTICES, MAX_WORK as MAX_PROP_WORK,
};
pub const MAX_CAMERA_PROPERTIES: usize = 256;
pub const MAX_CAMERA_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub id: u32,
    pub origin: DVec3,
    pub scale: u32,
}

#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct Limits {
    pub source_entities: usize,
    pub source_brushes: usize,
    pub source_sides: usize,
    pub sides_per_brush: usize,
    pub source_bytes: usize,
    pub source_work: usize,
    pub membership_work: usize,
    pub vertices: usize,
    pub cpu_bytes: usize,
    pub tree: tree::Limits,
    pub portals: portals::Limits,
    pub faces: faces::Limits,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            source_entities: 16_384,
            source_brushes: 2_048,
            source_sides: 16_384,
            sides_per_brush: 128,
            source_bytes: 4 * 1024 * 1024,
            source_work: 4_000_000,
            membership_work: 4_000_000,
            vertices: 262_144,
            cpu_bytes: 512 * 1024 * 1024,
            tree: tree::Limits {
                nodes: 8_191,
                pending: 4_096,
                points: 400_000,
                work: 8_000_000,
            },
            portals: portals::Limits {
                portals: 32_768,
                points: 400_000,
                pending: 8_192,
                work: 8_000_000,
            },
            faces: faces::Limits {
                faces: 32_768,
                pending: 8_192,
                points: 400_000,
                work: 8_000_000,
            },
        }
    }
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Stats {
    pub source_brushes: usize,
    pub source_sides: usize,
    pub tree_nodes: usize,
    pub component_leaves: usize,
    pub component_bounds: [[f64; 3]; 2],
    pub portals: usize,
    pub surfaces: usize,
    pub vertices: usize,
    pub cpu_reservation_bytes: usize,
    pub excluded_static_props: usize,
    pub static_prop_models: usize,
    pub static_prop_vertices: usize,
    pub inside_static_props: usize,
    pub static_prop_triangles: usize,
    pub terrain_patches: usize,
    pub terrain_triangles: usize,
    pub terrain_grid_vertices: usize,
    pub excluded_terrain_patches: usize,
}

#[derive(Debug)]
pub struct Surface {
    pub vertices: Vec<DVec3>,
    pub normal: DVec3,
    pub material: String,
    pub texture_vectors: [[f32; 4]; 2],
    pub texture_points: Option<Vec<DVec3>>,
    pub normals: Option<Vec<DVec3>>,
    pub normalized_uvs: Option<Vec<[f32; 2]>>,
    pub source_model: Option<u32>,
    pub flags: i32,
    pub source_sides: Vec<RootSide>,
}

#[derive(Debug)]
pub struct SourceModel {
    pub entity: u32,
    pub model: String,
    pub skin: usize,
}

#[derive(Debug)]
pub struct Region {
    pub camera: u32,
    pub origin: DVec3,
    pub scale: u32,
    pub stats: Stats,
    pub surfaces: Vec<Surface>,
    pub source_models: Vec<SourceModel>,
    pub source_guard: Option<crate::asset_sources::AssetSourceGuard>,
}

pub fn camera(vmf: &Vmf, id: u32) -> Result<Camera, String> {
    vmf.validate_ids().map_err(|error| error.to_string())?;
    let entity = vmf
        .entities
        .iter()
        .find(|entity| entity.id == id)
        .ok_or_else(|| format!("entity:{id}: selected sky_camera is missing"))?;
    parse_camera(entity)
}

pub fn parse_camera(entity: &Entity) -> Result<Camera, String> {
    let id = entity.id;
    if entity.properties.0.len() > MAX_CAMERA_PROPERTIES {
        return Err(format!(
            "entity:{id}: sky_camera exceeded its property count budget"
        ));
    }
    let mut bytes = 0usize;
    for (key, value) in entity.properties.iter() {
        bytes = bytes
            .checked_add(key.len())
            .and_then(|bytes| bytes.checked_add(value.len()))
            .filter(|&bytes| bytes <= MAX_CAMERA_BYTES)
            .ok_or_else(|| format!("entity:{id}: sky_camera exceeded its property byte budget"))?;
    }
    let mut keys = BTreeSet::new();
    for (key, _) in entity.properties.iter() {
        if !keys.insert(key.to_ascii_lowercase()) {
            return Err(format!(
                "entity:{id}: selected sky_camera has duplicate key {key}"
            ));
        }
    }
    if entity.hidden || entity.classname() != Some("sky_camera") || !entity.solids.is_empty() {
        return Err(format!(
            "entity:{id}: choose a visible root point sky_camera"
        ));
    }
    map_core::fog::validate_sky_camera_binding(entity)
        .map_err(|error| format!("entity:{id}: {error}"))?;
    let origin = entity
        .origin()
        .filter(|origin| origin.is_finite())
        .ok_or_else(|| {
            format!("entity:{id}: sky_camera requires a finite three-coordinate origin")
        })?;
    if origin
        .as_vec3()
        .to_array()
        .iter()
        .any(|value| !value.is_finite() || value.abs() > map_core::fog::MAX_FIELD_MAGNITUDE)
    {
        return Err(format!(
            "entity:{id}: sky_camera origin components must be finite within float32 ±{}",
            map_core::fog::MAX_FIELD_MAGNITUDE
        ));
    }
    let raw = entity.properties.get("scale").ok_or_else(|| {
        format!("entity:{id}: sky_camera requires an explicit integer scale from 1 to {MAX_SCALE}")
    })?;
    if raw.is_empty() || !raw.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!(
            "entity:{id}: sky_camera scale must be an integer from 1 to {MAX_SCALE}"
        ));
    }
    let scale = raw
        .parse::<u32>()
        .ok()
        .filter(|scale| (1..=MAX_SCALE).contains(scale))
        .ok_or_else(|| {
            format!("entity:{id}: sky_camera scale must be an integer from 1 to {MAX_SCALE}")
        })?;
    Ok(Camera { id, origin, scale })
}

pub fn resolve_controlled(
    vmf: &Vmf,
    fs: &GameFs,
    camera_id: u32,
    include: impl Fn(ObjectId) -> bool,
    control: &Control,
) -> Result<Region, String> {
    resolve_limited(vmf, fs, camera_id, include, control, Limits::default())
}

pub fn admit(vmf: &Vmf, limits: Limits, control: &Control) -> Result<Stats, String> {
    preflight(vmf, limits, control)
}

pub fn resolve_limited(
    vmf: &Vmf,
    fs: &GameFs,
    camera_id: u32,
    include: impl Fn(ObjectId) -> bool,
    control: &Control,
    limits: Limits,
) -> Result<Region, String> {
    control.checkpoint()?;
    let mut stats = preflight(vmf, limits, control)?;
    let camera = camera(vmf, camera_id)?;
    let mut report = Report::default();
    let prepared = Map::prepare_world_geometry_controlled(vmf, fs, &mut report, control)
        .map_err(|error| preparation_error(error, &report))?;
    require_report(&report)?;
    let PreparedWorld {
        mut map,
        source_sides,
        displacements,
        terrain_surfaces,
    } = prepared;
    let indices = map.entities[0]
        .brushes
        .iter()
        .copied()
        .filter(|&index| map.brushes[index].structural())
        .collect::<Vec<_>>();
    let mut tree = Tree::build_limited(
        &map.brushes,
        &indices,
        &mut map.planes,
        control,
        limits.tree,
    )?;
    validate_seed(&tree, &map, camera, control)?;
    stats.tree_nodes = tree.nodes.len();
    let links = portals::build_limited(&tree, &map.planes, control, limits.portals)?;
    stats.portals = links.len();
    let seed = MapEntity {
        id: camera.id,
        classname: "sky_camera".into(),
        properties: Vec::new(),
        brushes: Vec::new(),
        origin: Some(camera.origin),
    };
    let leak = portals::flood_controlled(
        &mut tree,
        &map.planes,
        &links,
        [&seed],
        &mut report,
        control,
    )?;
    if leak.is_some() {
        return Err(format!(
            "entity:{}: selected sky_camera component leaks to the outside",
            camera.id
        ));
    }
    require_report(&report)?;
    validate_seed(&tree, &map, camera, control)?;
    drop(links);
    let leaves = active_leaves(&tree, control)?;
    if leaves.is_empty() {
        return Err("Selected sky_camera has no completed open component".into());
    }
    stats.component_leaves = leaves.len();
    let (mins, maxs) = leaves
        .iter()
        .fold((DVec3::MAX, DVec3::MIN), |(mins, maxs), &leaf| {
            let (lo, hi) = tree.nodes[leaf].volume.bounds();
            (mins.min(lo), maxs.max(hi))
        });
    stats.component_bounds = [mins.to_array(), maxs.to_array()];
    let selected_leaves = leaves.iter().copied().collect::<BTreeSet<_>>();
    let mut admission = Admission {
        tree: &tree,
        planes: &map.planes,
        leaves: &leaves,
        selected_leaves: &selected_leaves,
        work: 0,
        limit: limits.membership_work,
        control,
        pending_limit: limits.tree.pending,
    };
    let captured_terrain = terrain_surfaces
        .iter()
        .map(|terrain| terrain.source.solid)
        .collect::<BTreeSet<_>>();
    for displacement in displacements {
        if !captured_terrain.contains(&displacement.solid) {
            admission.source(
                "displacement",
                displacement.owner,
                Some(displacement.solid),
                displacement.bounds,
            )?;
        }
    }
    let terrain = terrain::admit(terrain_surfaces, &mut admission, &mut stats)?;
    let terrain_sides = terrain
        .iter()
        .map(|patch| patch.source.side)
        .collect::<BTreeSet<_>>();
    let mut prop_sources = static_props::Capture::new(fs, control);
    admit_original(
        vmf,
        &mut map.materials,
        &mut admission,
        &mut prop_sources,
        &terrain_sides,
    )?;
    let filtered = |side| {
        source_sides
            .get(&side)
            .is_some_and(|source| include(source.owner))
    };
    let output = faces::build_region_limited(&map, &tree, &filtered, control, limits.faces)?;
    stats.surfaces = output.len();
    stats.vertices = output.iter().try_fold(0usize, |total, face| {
        total
            .checked_add(face.winding.0.len())
            .filter(|&n| n <= limits.vertices)
            .ok_or("Sky region exceeded its output vertex budget")
    })?;
    let mut surfaces = Vec::with_capacity(output.len());
    for face in output {
        control.checkpoint()?;
        let info = &map.texinfos.infos[face.texinfo];
        let material = map.materials.get(map.texdata[info.texdata as usize]);
        let sources = face
            .source_sides
            .iter()
            .map(|side| {
                source_sides
                    .get(side)
                    .copied()
                    .ok_or_else(|| format!("side:{side}: sky region lost source ownership"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        if sources.is_empty() {
            return Err("Sky region surface has no original source side".into());
        }
        surfaces.push(Surface {
            vertices: face.winding.0,
            normal: map.planes.get(face.plane).normal,
            material: material.name.clone(),
            texture_vectors: info.texture_vecs,
            texture_points: None,
            normals: None,
            normalized_uvs: None,
            source_model: None,
            flags: info.flags,
            source_sides: sources,
        });
    }
    terrain::append(
        terrain,
        vmf,
        &include,
        &mut surfaces,
        &mut stats,
        limits,
        &mut admission,
    )?;
    let source_models =
        prop_sources.append(&include, &mut surfaces, &mut stats, limits, &mut admission)?;
    control.checkpoint()?;
    let source_guard = prop_sources.finish(&mut stats)?;
    Ok(Region {
        camera: camera.id,
        origin: camera.origin,
        scale: camera.scale,
        stats,
        surfaces,
        source_models,
        source_guard,
    })
}

fn preparation_error(error: String, report: &Report) -> String {
    match report
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.level == crate::Level::Error)
    {
        Some(diagnostic) => format!(
            "{}: {}",
            diagnostic.object.as_deref().unwrap_or("world"),
            diagnostic.message
        ),
        None => error,
    }
}

fn require_report(report: &Report) -> Result<(), String> {
    if report.has_errors() {
        return Err(preparation_error(
            "Sky region preparation reported errors".into(),
            report,
        ));
    }
    Ok(())
}

fn validate_seed(tree: &Tree, map: &Map, camera: Camera, control: &Control) -> Result<(), String> {
    let root = &tree.nodes[0].volume;
    if root
        .sides
        .iter()
        .any(|side| map.planes.get(side.plane).distance(camera.origin) >= -ON_EPSILON)
    {
        return Err(format!(
            "entity:{}: sky_camera origin is outside or on the root volume boundary",
            camera.id
        ));
    }
    let mut pending = vec![(0, false)];
    while let Some((index, boundary)) = pending.pop() {
        control.checkpoint()?;
        let node = &tree.nodes[index];
        if let Some((plane, children)) = node.split {
            let distance = map.planes.get(plane).distance(camera.origin);
            if distance.abs() <= ON_EPSILON {
                pending.extend(children.map(|child| (child, true)));
            } else {
                pending.push((children[usize::from(distance < 0.0)], boundary));
            }
        } else if node.solid() {
            let reason = if boundary {
                "lies on an ambiguous solid or disconnected component boundary"
            } else {
                "is inside solid space"
            };
            return Err(format!("entity:{}: sky_camera origin {reason}", camera.id));
        }
    }
    Ok(())
}

fn active_leaves(tree: &Tree, control: &Control) -> Result<Vec<usize>, String> {
    let mut leaves = Vec::new();
    let mut pending = vec![0];
    while let Some(index) = pending.pop() {
        control.checkpoint()?;
        let node = &tree.nodes[index];
        match node.split {
            Some((_, children)) => pending.extend(children),
            None if !node.solid() => leaves.push(index),
            None => {}
        }
    }
    Ok(leaves)
}

struct Admission<'a> {
    tree: &'a Tree,
    planes: &'a crate::planes::PlaneSet,
    leaves: &'a [usize],
    selected_leaves: &'a BTreeSet<usize>,
    work: usize,
    limit: usize,
    control: &'a Control,
    pending_limit: usize,
}

impl Admission<'_> {
    fn consume(&mut self, amount: usize) -> Result<(), String> {
        self.control.checkpoint()?;
        self.work = self
            .work
            .checked_add(amount)
            .filter(|&work| work <= self.limit)
            .ok_or("Sky region exceeded its unsupported-membership work budget")?;
        Ok(())
    }

    fn outside(&mut self, bounds: (DVec3, DVec3)) -> Result<bool, String> {
        let tree = self.tree;
        let planes = self.planes;
        let leaves = self.leaves;
        for &leaf in leaves {
            let mut disjoint = false;
            for side in &tree.nodes[leaf].volume.sides {
                self.consume(1)?;
                let plane = planes.get(side.plane);
                if plane_bounds(plane, bounds).0 > ON_EPSILON {
                    disjoint = true;
                    break;
                }
            }
            if !disjoint {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn contains(&mut self, bounds: (DVec3, DVec3)) -> Result<bool, String> {
        let tree = self.tree;
        let planes = self.planes;
        for side in &tree.nodes[0].volume.sides {
            self.consume(1)?;
            if plane_bounds(planes.get(side.plane), bounds).1 >= 0.0 {
                return Ok(false);
            }
        }
        let capacity = self.pending_limit.min(tree.nodes.len());
        if capacity == 0 {
            return Err("Sky region exceeded its pending membership node budget".into());
        }
        let mut pending = Vec::new();
        pending
            .try_reserve_exact(capacity)
            .map_err(|error| format!("Sky region membership allocation failed: {error}"))?;
        pending.push(0);
        while let Some(index) = pending.pop() {
            self.consume(1)?;
            let node = &tree.nodes[index];
            if let Some((plane, children)) = node.split {
                let (low, high) = plane_bounds(planes.get(plane), bounds);
                let reached = if low > 0.0 {
                    &children[..1]
                } else if high < 0.0 {
                    &children[1..]
                } else {
                    &children[..]
                };
                if pending.len() + reached.len() > capacity {
                    return Err("Sky region exceeded its pending membership node budget".into());
                }
                pending.extend_from_slice(reached);
            } else if node.solid() || !self.selected_leaves.contains(&index) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn source(
        &mut self,
        kind: &str,
        owner: ObjectId,
        solid: Option<u32>,
        bounds: Option<(DVec3, DVec3)>,
    ) -> Result<(), String> {
        self.control.checkpoint()?;
        let owner = match owner {
            ObjectId::Solid(id) => format!("solid:{id}"),
            ObjectId::Entity(id) => format!("entity:{id}"),
        };
        let label = match solid {
            Some(id) => format!("{owner} solid:{id}"),
            None => owner,
        };
        let Some((mins, maxs)) = bounds
            .filter(|(mins, maxs)| mins.is_finite() && maxs.is_finite() && mins.cmple(*maxs).all())
        else {
            return Err(format!(
                "{label}: unsupported {kind} membership; conservative world bounds are unavailable"
            ));
        };
        if !self.outside((mins, maxs))? {
            return Err(format!(
                "{label}: unsupported {kind} cannot be certified outside the selected sky_camera component"
            ));
        }
        Ok(())
    }
}

fn plane_bounds(plane: &crate::planes::Plane, bounds: (DVec3, DVec3)) -> (f64, f64) {
    let mut low = -plane.dist;
    let mut high = -plane.dist;
    for axis in 0..3 {
        let normal = plane.normal[axis];
        if normal == 0.0 {
            continue;
        }
        let (first, second) = if normal < 0.0 {
            (bounds.1[axis], bounds.0[axis])
        } else {
            (bounds.0[axis], bounds.1[axis])
        };
        let first = normal * first;
        let second = normal * second;
        low = (low + first.next_down()).next_down();
        high = (high + second.next_up()).next_up();
    }
    (low, high)
}

fn solid_bounds(solid: &Solid) -> Option<(DVec3, DVec3)> {
    if solid.sides.iter().any(|side| side.displacement.is_some()) {
        return None;
    }
    crate::map::origins::checked_geometry(solid, &mut Report::default())
        .map(|geometry| (geometry.mins, geometry.maxs))
}

fn admit_original(
    vmf: &Vmf,
    materials: &mut crate::materials::Materials<'_>,
    admission: &mut Admission<'_>,
    props: &mut static_props::Capture<'_>,
    terrain_sides: &BTreeSet<u32>,
) -> Result<(), String> {
    for (owner, solid) in vmf
        .world
        .solids
        .iter()
        .filter(|solid| !solid.hidden)
        .map(|solid| (ObjectId::Solid(solid.id), solid))
        .chain(
            vmf.entities
                .iter()
                .filter(|entity| !entity.hidden && entity.classname() == Some("func_detail"))
                .flat_map(|entity| {
                    entity
                        .solids
                        .iter()
                        .filter(|solid| !solid.hidden)
                        .map(move |solid| (ObjectId::Entity(entity.id), solid))
                }),
        )
    {
        let terrain = solid.sides.iter().any(|side| side.displacement.is_some());
        for side in solid
            .sides
            .iter()
            .filter(|side| !terrain || side.displacement.is_some())
        {
            admission.control.checkpoint()?;
            let material = materials.load(&side.material)?;
            let info = materials.get(material);
            if terrain_sides.contains(&side.id) {
                map_core::displacement::native::validate_material(
                    info.draws,
                    info.contents,
                    info.surface,
                )
                .map_err(|error| format!("solid:{} side:{}: {error}", solid.id, side.id))?;
            }
            if info.detail_type.is_some() {
                admission.source(
                    "material-generated detail geometry (parent brush bounds do not bound scattered models or sprites)",
                    owner,
                    Some(solid.id),
                    None,
                )?;
            }
            let kind = if info.contents & (contents::WATER | contents::SLIME) != 0 {
                Some("water")
            } else if info.contents & contents::AREAPORTAL != 0 {
                Some("portal")
            } else if !info.unsupported_flags.is_empty() {
                Some("material compile semantics")
            } else {
                None
            };
            if let Some(kind) = kind {
                admission.source(kind, owner, Some(solid.id), solid_bounds(solid))?;
                break;
            }
        }
    }
    for entity in vmf.entities.iter().filter(|entity| !entity.hidden) {
        admission.control.checkpoint()?;
        let classname = entity.classname().unwrap_or("");
        if crate::map::unresolved_instance_class(classname) {
            admission.source("instance", ObjectId::Entity(entity.id), None, None)?;
        }
        if classname == "func_detail" {
            continue;
        }
        for solid in entity.solids.iter().filter(|solid| !solid.hidden) {
            let kind = if solid.sides.iter().any(|side| side.displacement.is_some()) {
                "displacement"
            } else if classname.eq_ignore_ascii_case("func_areaportal")
                || classname.eq_ignore_ascii_case("func_areaportalwindow")
            {
                "portal"
            } else {
                "brush entity geometry"
            };
            let bounds =
                if entity.properties.get("angles").is_some_and(|angles| {
                    source_formats::vmf::parse_vec3(angles) != Some(DVec3::ZERO)
                }) {
                    None
                } else {
                    solid_bounds(solid)
                };
            admission.source(kind, ObjectId::Entity(entity.id), Some(solid.id), bounds)?;
        }
        if entity.solids.is_empty() && omitted_point_geometry(entity) {
            if classname == "prop_static" {
                props.admit(entity, admission).map_err(|error| {
                    format!("entity:{}: static-prop sky membership: {error}", entity.id)
                })?;
                continue;
            }
            admission.source(
                "model or projected geometry",
                ObjectId::Entity(entity.id),
                None,
                None,
            )?;
        }
    }
    Ok(())
}

fn omitted_point_geometry(entity: &Entity) -> bool {
    entity
        .properties
        .get("model")
        .is_some_and(|model| !model.is_empty())
        || entity.classname().is_some_and(|classname| {
            classname.starts_with("prop_")
                || [
                    "info_overlay",
                    "infodecal",
                    "env_sprite",
                    "env_sprite_oriented",
                ]
                .iter()
                .any(|name| classname.eq_ignore_ascii_case(name))
        })
}

fn preflight(vmf: &Vmf, limits: Limits, control: &Control) -> Result<Stats, String> {
    if vmf.entities.len() > limits.source_entities
        || vmf.world.groups.len() > limits.source_entities
    {
        return Err("Sky region exceeded its source entity budget".into());
    }
    let mut stats = Stats::default();
    let mut bytes = 0usize;
    let mut work = 0usize;
    let mut source_points = 0usize;
    let mut terrain_vertices = 0usize;
    let mut terrain_patches = 0usize;
    let mut terrain_triangles = 0usize;
    let mut longest_material = 0usize;
    for properties in std::iter::once(&vmf.world.properties)
        .chain(vmf.entities.iter().map(|entity| &entity.properties))
    {
        control.checkpoint()?;
        for (key, value) in properties.iter() {
            bytes = checked_total(
                bytes,
                size_of::<(String, String)>() * 2,
                limits.source_bytes,
                "source byte",
            )?;
            bytes = checked_total(bytes, key.len(), limits.source_bytes, "source byte")?;
            bytes = checked_total(bytes, value.len(), limits.source_bytes, "source byte")?;
        }
    }
    for solid in vmf
        .world
        .solids
        .iter()
        .chain(vmf.entities.iter().flat_map(|entity| &entity.solids))
    {
        control.checkpoint()?;
        let sides = solid.sides.len();
        if sides > limits.sides_per_brush {
            return Err(format!(
                "solid:{}: sky region exceeded its per-brush source side budget",
                solid.id
            ));
        }
        stats.source_brushes = checked_total(
            stats.source_brushes,
            1,
            limits.source_brushes,
            "source brush",
        )?;
        stats.source_sides = checked_total(
            stats.source_sides,
            sides,
            limits.source_sides,
            "source side",
        )?;
        let brush_work = sides
            .checked_mul(sides)
            .and_then(|n| n.checked_mul(sides))
            .and_then(|n| n.checked_mul(sides))
            .ok_or("Sky region source work count overflow")?;
        work = checked_total(
            work,
            brush_work,
            limits.source_work,
            "source conversion work",
        )?;
        source_points = source_points
            .checked_add(
                sides
                    .checked_mul(sides + 3)
                    .ok_or("Sky region source allocation count overflow")?,
            )
            .ok_or("Sky region source allocation count overflow")?;
        for side in &solid.sides {
            if let Some(disp) = &side.displacement {
                let vertices = if (2..=4).contains(&disp.power) {
                    ((1usize << disp.power) + 1).pow(2)
                } else {
                    map_core::displacement::MAX_GRID_VERTICES
                };
                terrain_vertices = checked_total(
                    terrain_vertices,
                    vertices,
                    source_formats::bsp::MAX_DISPLACEMENT_VERTICES,
                    "terrain membership vertex",
                )?;
                terrain_patches = checked_total(
                    terrain_patches,
                    1,
                    source_formats::bsp::MAX_DISPLACEMENTS,
                    "terrain membership patch",
                )?;
                let triangles = if (2..=4).contains(&disp.power) {
                    2 * (1usize << disp.power).pow(2)
                } else {
                    512
                };
                terrain_triangles = checked_total(
                    terrain_triangles,
                    triangles,
                    source_formats::bsp::MAX_DISPLACEMENT_TRIANGLES,
                    "terrain input triangle",
                )?;
            }
            bytes = checked_total(
                bytes,
                side.material.len(),
                limits.source_bytes,
                "source byte",
            )?;
            longest_material = longest_material.max(side.material.len());
        }
    }
    let reservations = [
        (
            usize::from(
                vmf.entities
                    .iter()
                    .any(|entity| !entity.hidden && entity.classname() == Some("prop_static")),
            ),
            static_props::CPU_RESERVATION_BYTES,
        ),
        (
            usize::from(
                vmf.entities
                    .iter()
                    .any(|entity| !entity.hidden && entity.classname() == Some("prop_static")),
            ) * limits.faces.faces,
            size_of::<DVec3>() * 6
                + size_of::<[f32; 2]>() * 3
                + static_props::MAX_MATERIAL_PATH_BYTES
                + 128,
        ),
        (1, crate::materials::MAX_GEOMETRY_MATERIAL_ALLOCATION_BYTES),
        (
            source_points,
            size_of::<DVec3>() * 4
                + size_of::<crate::map::MapSide>() * 12
                + size_of::<crate::planes::Plane>() * 24
                + 512,
        ),
        (stats.source_sides, 256),
        (limits.source_entities, 256),
        (stats.source_brushes, 128),
        (bytes, 4),
        (
            terrain_vertices,
            size_of::<DVec3>() * 12
                + size_of::<source_formats::bsp::DisplacementVertex>() * 3
                + size_of::<[usize; 3]>() * 4
                + 64,
        ),
        (terrain_patches, 4096),
        (
            terrain_triangles.min(limits.faces.faces),
            size_of::<Surface>() * 2
                + size_of::<DVec3>() * 9
                + size_of::<RootSide>()
                + longest_material
                + 256,
        ),
        (limits.tree.pending, size_of::<usize>()),
        (limits.tree.points, size_of::<DVec3>()),
        (limits.tree.nodes, size_of::<tree::Node>() * 2 + 256),
        (limits.portals.points, size_of::<DVec3>()),
        (
            limits.portals.portals,
            size_of::<portals::Portal>() * 2 + 128,
        ),
        (limits.faces.points, size_of::<DVec3>()),
        (
            limits.faces.faces,
            size_of::<faces::Face>() * 4 + size_of::<Surface>() * 2 + longest_material + 256,
        ),
        (limits.vertices, size_of::<DVec3>()),
    ];
    let mut reservation = 0usize;
    for (count, size) in reservations {
        let bytes = count
            .checked_mul(size)
            .ok_or("Sky region CPU reservation overflow")?;
        reservation = checked_total(
            reservation,
            bytes,
            limits.cpu_bytes,
            "aggregate CPU allocation",
        )?;
    }
    stats.cpu_reservation_bytes = reservation;
    Ok(stats)
}

fn checked_total(
    total: usize,
    additional: usize,
    limit: usize,
    kind: &str,
) -> Result<usize, String> {
    total
        .checked_add(additional)
        .filter(|&n| n <= limit)
        .ok_or_else(|| format!("Sky region exceeded its {kind} budget"))
}
