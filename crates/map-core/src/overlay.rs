use std::collections::BTreeSet;

use glam::DVec3;
use source_formats::{
    bsp,
    gamefs::asset_relative,
    vmf::{Entity, Vmf, format_number, format_vec3, parse_vec3},
};

use crate::{
    BrushGeometry, EditError, ObjectId, Transaction, checks::SOURCE_COORDINATE_LIMIT, edit,
    plane::ON_EPSILON, surface::FaceId,
};

const BASIS_TOLERANCE: f64 = 0.01;
const QUAD_AREA_EPSILON: f64 = 1e-8;
const COPLANAR_NORMAL_TOLERANCE: f64 = 1e-10;

#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    pub material: String,
    pub targetname: Option<String>,
    pub render_order: u16,
    pub u: [f32; 2],
    pub v: [f32; 2],
    pub fade_min: f32,
    pub fade_max: f32,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            material: String::new(),
            targetname: None,
            render_order: 0,
            u: [0.0, 1.0],
            v: [0.0, 1.0],
            fade_min: -1.0,
            fade_max: 0.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Definition {
    pub options: Options,
    pub sides: Vec<u32>,
    pub basis_origin: [f64; 3],
    pub basis_normal: [f64; 3],
    pub basis_u: [f64; 3],
    pub basis_v: [f64; 3],
    pub uv_points: [[f64; 3]; 4],
}

impl Definition {
    pub fn corners(&self) -> [[f64; 3]; 4] {
        let origin = DVec3::from_array(self.basis_origin);
        let u = DVec3::from_array(self.basis_u);
        let v = DVec3::from_array(self.basis_v);
        self.uv_points
            .map(|point| (origin + u * point[0] + v * point[1]).to_array())
    }

    pub fn fade_squared(&self) -> Result<[f32; 2], String> {
        Ok([
            fade(self.options.fade_min, "fademindist")?,
            fade(self.options.fade_max, "fademaxdist")?,
        ])
    }

    pub fn validate(&self) -> Result<(), String> {
        let normal = native_vector(self.basis_normal, "BasisNormal")?;
        let u = native_vector(self.basis_u, "BasisU")?;
        let v = native_vector(self.basis_v, "BasisV")?;
        if [normal, u, v]
            .iter()
            .any(|axis| (axis.length_squared() - 1.0).abs() > BASIS_TOLERANCE)
            || normal
                .dot(u)
                .abs()
                .max(normal.dot(v).abs())
                .max(u.dot(v).abs())
                > BASIS_TOLERANCE
        {
            return Err(
                "Overlay BasisNormal, BasisU and BasisV must form perpendicular unit axes".into(),
            );
        }
        let origin = native_vector(self.basis_origin, "BasisOrigin")?;
        let mut points = [[0.0f32; 2]; 4];
        for (index, point) in self.uv_points.iter().enumerate() {
            let uv = native_vector(*point, &format!("uv{index}"))?;
            let world = origin + u * uv.x + v * uv.y;
            if world.abs().max_element() > SOURCE_COORDINATE_LIMIT {
                return Err(format!(
                    "Overlay projected corners must fit ±{SOURCE_COORDINATE_LIMIT} units"
                ));
            }
            points[index] = [uv.x as f32, uv.y as f32];
        }
        if origin.abs().max_element() > SOURCE_COORDINATE_LIMIT {
            return Err(format!(
                "Overlay BasisOrigin must fit ±{SOURCE_COORDINATE_LIMIT} units"
            ));
        }
        let mut turn = 0.0f64;
        for index in 0..4 {
            let [a, b, c] = [index, (index + 1) % 4, (index + 2) % 4].map(|index| points[index]);
            let cross = (f64::from(b[0]) - f64::from(a[0])) * (f64::from(c[1]) - f64::from(b[1]))
                - (f64::from(b[1]) - f64::from(a[1])) * (f64::from(c[0]) - f64::from(b[0]));
            if cross.abs() < QUAD_AREA_EPSILON || (turn != 0.0 && cross.signum() != turn.signum()) {
                return Err(
                    "Overlay uv0–uv3 must form a nondegenerate convex quadrilateral in perimeter order"
                        .into(),
                );
            }
            turn = cross;
        }
        for (key, value) in [
            ("StartU", self.options.u[0]),
            ("EndU", self.options.u[1]),
            ("StartV", self.options.v[0]),
            ("EndV", self.options.v[1]),
        ] {
            native_number(value, key)?;
        }
        if self.options.u[0] == self.options.u[1] || self.options.v[0] == self.options.v[1] {
            return Err("Overlay U/V start and end must differ".into());
        }
        if self.options.render_order > 3 {
            return Err("Overlay RenderOrder must be an integer from 0 to 3".into());
        }
        self.fade_squared()?;
        let sides = self.sides.iter().copied().collect::<BTreeSet<_>>();
        side_budget(sides.len())?;
        if sides.is_empty() {
            return Err("Overlay requires at least one visible source side in sides".into());
        }
        material(&self.options.material)?;
        if self
            .options
            .targetname
            .as_ref()
            .is_some_and(|name| name.contains(['"', '\0']))
        {
            return Err("Overlay targetname cannot contain quotes or NUL".into());
        }
        Ok(())
    }
}

fn native_number(value: f32, key: &str) -> Result<f32, String> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(format!("Overlay {key} must be a finite native number"))
    }
}

fn native_vector(value: [f64; 3], key: &str) -> Result<DVec3, String> {
    let value = DVec3::from_array(value);
    if value.is_finite() && value.as_vec3().is_finite() {
        Ok(value)
    } else {
        Err(format!(
            "Overlay {key} must contain three finite native numbers"
        ))
    }
}

fn fade(value: f32, key: &str) -> Result<f32, String> {
    let value = native_number(value, key)?;
    let squared = if value > 0.0 { value * value } else { value };
    if !squared.is_finite() {
        return Err(format!(
            "Overlay {key} squared distance exceeds native float bounds"
        ));
    }
    Ok(squared)
}

fn material(raw: &str) -> Result<String, String> {
    let material =
        asset_relative(raw.trim()).map_err(|error| format!("Overlay material: {error}"))?;
    let material = material.strip_prefix("materials/").unwrap_or(&material);
    let material = material.strip_suffix(".vmt").unwrap_or(material);
    if material.is_empty() || material.contains(['"', '\n', '\r']) {
        return Err("Overlay material must name a relative game material".into());
    }
    Ok(material.to_owned())
}

fn number(entity: &Entity, key: &str, default: f32) -> Result<f32, String> {
    entity.properties.get(key).map_or(Ok(default), |raw| {
        raw.trim()
            .parse::<f32>()
            .map_err(|_| format!("Overlay {key} must be a finite native number"))
    })
}

fn side_budget(count: usize) -> Result<(), String> {
    if count > bsp::MAX_OVERLAY_FACES {
        return Err(format!(
            "Overlay sources exceed their {}-side allocation budget",
            bsp::MAX_OVERLAY_FACES
        ));
    }
    Ok(())
}

fn vector(entity: &Entity, key: &str) -> Result<[f64; 3], String> {
    entity
        .properties
        .get(key)
        .and_then(parse_vec3)
        .map(|value| value.to_array())
        .ok_or_else(|| format!("Overlay {key} must contain three finite native numbers"))
}

pub fn parse(entity: &Entity) -> Result<Definition, String> {
    if entity.solids.iter().any(|solid| !solid.hidden) {
        return Err("info_overlay is a point helper; remove its brush geometry".into());
    }
    let mut sides = BTreeSet::new();
    for raw in entity
        .properties
        .get("sides")
        .unwrap_or("")
        .split_whitespace()
    {
        let side = raw
            .parse::<u32>()
            .map_err(|_| "Overlay sides must contain integer VMF side IDs")?;
        sides.insert(side);
        side_budget(sides.len())?;
    }
    let mut points = [[0.0; 3]; 4];
    for (index, point) in points.iter_mut().enumerate() {
        *point = vector(entity, &format!("uv{index}"))?;
    }
    let mut definition = Definition {
        options: Options {
            material: entity.properties.get("material").unwrap_or("").into(),
            targetname: entity
                .properties
                .get("targetname")
                .filter(|name| !name.trim().is_empty())
                .map(str::to_owned),
            render_order: entity
                .properties
                .get("RenderOrder")
                .unwrap_or("0")
                .trim()
                .parse::<u16>()
                .map_err(|_| "Overlay RenderOrder must be an integer from 0 to 3")?,
            u: [number(entity, "StartU", 0.0)?, number(entity, "EndU", 1.0)?],
            v: [number(entity, "StartV", 0.0)?, number(entity, "EndV", 1.0)?],
            fade_min: number(entity, "fademindist", -1.0)?,
            fade_max: number(entity, "fademaxdist", 0.0)?,
        },
        sides: sides.into_iter().collect(),
        basis_origin: vector(entity, "BasisOrigin")?,
        basis_normal: vector(entity, "BasisNormal")?,
        basis_u: vector(entity, "BasisU")?,
        basis_v: vector(entity, "BasisV")?,
        uv_points: points,
    };
    definition.validate()?;
    definition.options.material = material(&definition.options.material)?;
    Ok(definition)
}

struct SourceFace {
    normal: DVec3,
    vertices: Vec<DVec3>,
    samples: Vec<DVec3>,
}

fn source_face(vmf: &Vmf, face: FaceId) -> Result<SourceFace, String> {
    let (solid, owner) = vmf
        .world
        .solids
        .iter()
        .map(|solid| (solid, None))
        .chain(
            vmf.entities
                .iter()
                .flat_map(|entity| entity.solids.iter().map(move |solid| (solid, Some(entity)))),
        )
        .find(|(solid, _)| solid.id == face.solid)
        .ok_or("Overlay target brush does not exist")?;
    if solid.hidden || owner.is_some_and(|entity| entity.hidden) {
        return Err("Overlay fitting requires a visible source face".into());
    }
    if owner.is_some_and(|entity| entity.classname() != Some("func_detail")) {
        return Err("Overlay fitting supports world and func_detail faces only".into());
    }
    let index = solid
        .sides
        .iter()
        .position(|side| side.id == face.side)
        .ok_or("Overlay face does not belong to the target brush")?;
    let geometry = BrushGeometry::from_solid(solid).map_err(|error| error.to_string())?;
    let target = geometry
        .faces
        .iter()
        .find(|target| target.side == index)
        .ok_or("Overlay target has no effective surface")?;
    if target
        .vertices
        .iter()
        .any(|point| !point.is_finite() || target.plane.distance_to(*point).abs() > ON_EPSILON)
    {
        return Err("Overlay fitting requires one convex planar quad face".into());
    }
    if target
        .vertices
        .iter()
        .any(|point| point.abs().max_element() > SOURCE_COORDINATE_LIMIT)
    {
        return Err(format!(
            "Overlay source vertices must fit ±{SOURCE_COORDINATE_LIMIT} units"
        ));
    }
    if let Some(info) = &solid.sides[index].displacement {
        return terrain_source(vmf, face, target, info);
    }
    if solid.sides.iter().any(|side| side.displacement.is_some()) {
        return Err("Overlay fitting does not support terrain brushes' cage faces".into());
    }
    Ok(SourceFace {
        normal: target.plane.normal,
        vertices: target.vertices.clone(),
        samples: Vec::new(),
    })
}

pub fn is_terrain(vmf: &Vmf, face: FaceId) -> bool {
    vmf.solids()
        .find(|solid| solid.id == face.solid)
        .and_then(|solid| solid.sides.iter().find(|side| side.id == face.side))
        .is_some_and(|side| side.displacement.is_some())
}

fn terrain_source(
    vmf: &Vmf,
    face: FaceId,
    target: &crate::brush::Face,
    info: &source_formats::vmf::DispInfo,
) -> Result<SourceFace, String> {
    use crate::displacement::{native, surface};
    if vmf.entities.iter().any(|entity| {
        !entity.hidden
            && entity
                .classname()
                .is_some_and(|class| class.eq_ignore_ascii_case("func_instance"))
    }) {
        return Err("Terrain overlay authoring cannot prove isolation with visible instance references; hide them in saved map visibility or use an imported overlay. Temporary editor hiding does not qualify".into());
    }
    let (base, mesh) = native::checked_mesh(target, info)?;
    crate::displacement::smoothing::angle(info)?;
    crate::displacement::triangles::native_tags(info, &mesh).map_err(|error| error.to_string())?;
    let prepared = surface::prepare(vmf);
    let current = prepared
        .get(&face)
        .ok_or("Overlay target has no prepared terrain surface")?;
    let bounds = |mesh: &crate::displacement::DisplacementMesh| {
        mesh.base_positions
            .iter()
            .map(|point| native::stored_vertex(*point))
            .fold((DVec3::MAX, DVec3::MIN), |(mins, maxs), point| {
                (mins.min(point), maxs.max(point))
            })
    };
    let (mins, maxs) = bounds(&mesh);
    if prepared.meshes.iter().any(|(other, mesh)| {
        let (other_mins, other_maxs) = bounds(mesh);
        *other != face
            && (mins - DVec3::splat(ON_EPSILON)).cmple(other_maxs).all()
            && (maxs + DVec3::splat(ON_EPSILON)).cmpge(other_mins).all()
    }) {
        return Err("Overlay terrain fitting requires an isolated patch; separate touching or overlapping terrain base bounds, or use an imported overlay".into());
    }
    if current.positions != mesh.positions {
        return Err("Overlay fitting does not support seam-adjusted terrain".into());
    }
    let (normal, corners) =
        planar_terrain_quad(&current.positions, mesh.size, target.plane.normal)?;
    let native = native::positions(
        base,
        bsp::DisplacementInfo {
            start: native::stored_vertex(mesh.base_positions[0])
                .as_vec3()
                .to_array(),
            power: info.power as i32,
            ..Default::default()
        },
        native::vertices(&mesh),
    )?
    .into_iter()
    .map(|point| DVec3::from_array(point.map(f64::from)))
    .collect::<Vec<_>>();
    planar_terrain_quad(&native, mesh.size, normal)?;
    let origin = corners[0];
    if native
        .iter()
        .any(|point| normal.dot(*point - origin).abs() > ON_EPSILON)
        || corners
            .iter()
            .zip([
                0,
                (mesh.size - 1) * mesh.size,
                mesh.size * mesh.size - 1,
                mesh.size - 1,
            ])
            .any(|(corner, index)| !corner.abs_diff_eq(native[index], ON_EPSILON))
    {
        return Err("Overlay terrain differs from native surface precision".into());
    }
    Ok(SourceFace {
        normal,
        vertices: corners,
        samples: current.positions.iter().copied().chain(native).collect(),
    })
}

fn planar_terrain_quad(
    positions: &[DVec3],
    size: usize,
    facing: DVec3,
) -> Result<(DVec3, Vec<DVec3>), String> {
    let corners = [0, (size - 1) * size, size * size - 1, size - 1].map(|index| positions[index]);
    let normal = (corners[3] - corners[0])
        .cross(corners[1] - corners[0])
        .try_normalize()
        .ok_or("Overlay terrain requires a nondegenerate planar quad")?;
    if normal.dot(facing) <= 0.0
        || positions.iter().any(|point| {
            !point.is_finite()
                || point.abs().max_element() > SOURCE_COORDINATE_LIMIT
                || normal.dot(*point - corners[0]).abs() > ON_EPSILON
        })
    {
        return Err("Overlay fitting supports planar terrain only; sculpted or nonplanar terrain is unavailable".into());
    }
    for row in 0..size {
        for column in 0..size {
            let point = positions[row * size + column];
            for edge in 0..4 {
                let a = corners[edge];
                let b = corners[(edge + 1) % 4];
                if a.distance_squared(b) < QUAD_AREA_EPSILON
                    || (a - b).cross(point - a).dot(normal) / a.distance(b) < -ON_EPSILON
                {
                    return Err("Overlay terrain must stay inside its convex quad boundary".into());
                }
            }
            let edge = if column == 0 {
                Some((corners[0], corners[1]))
            } else if column == size - 1 {
                Some((corners[3], corners[2]))
            } else if row == 0 {
                Some((corners[0], corners[3]))
            } else if row == size - 1 {
                Some((corners[1], corners[2]))
            } else {
                None
            };
            if edge.is_some_and(|(a, b)| {
                let edge = b - a;
                let t = ((point - a).dot(edge) / edge.length_squared()).clamp(0.0, 1.0);
                !point.abs_diff_eq(a + edge * t, ON_EPSILON)
            }) {
                return Err("Overlay terrain requires four straight quad boundary edges".into());
            }
        }
    }
    let triangles = bsp::DisplacementInfo {
        power: (size - 1).ilog2() as i32,
        ..Default::default()
    }
    .triangle_indices()
    .map_err(|error| error.to_string())?;
    if triangles.iter().any(|[a, b, c]| {
        (positions[*a] - positions[*b])
            .cross(positions[*c] - positions[*b])
            .dot(normal)
            < QUAD_AREA_EPSILON
    }) {
        return Err("Overlay terrain contains collapsed or folded surface triangles".into());
    }
    Ok((normal, corners.to_vec()))
}

pub fn fit(vmf: &Vmf, face: FaceId, options: Options) -> Result<Definition, String> {
    vmf.validate_ids().map_err(|error| error.to_string())?;
    let source = source_face(vmf, face)?;
    let definition = fit_source(face, source.normal, &source.vertices, options)?;
    if is_terrain(vmf, face) {
        validate_editable_projection(&definition, &definition)?;
    }
    Ok(definition)
}

fn fit_source(
    face: FaceId,
    normal: DVec3,
    vertices: &[DVec3],
    mut options: Options,
) -> Result<Definition, String> {
    let [a, b, c, d] = vertices else {
        return Err("Overlay fitting requires one convex planar quad face".into());
    };
    let origin = (*a + *b + *c + *d) * 0.25;
    let u = (*d - *a)
        .try_normalize()
        .ok_or("Overlay fitting requires nondegenerate quad edges")?;
    let v = normal.cross(u);
    let points = [a, b, c, d].map(|point| {
        let local = *point - origin;
        [local.dot(u), local.dot(v), 0.0]
    });
    options.material = material(&options.material)?;
    options.targetname = options.targetname.filter(|name| !name.trim().is_empty());
    let definition = Definition {
        options,
        sides: vec![face.side],
        basis_origin: origin.to_array(),
        basis_normal: normal.to_array(),
        basis_u: u.to_array(),
        basis_v: v.to_array(),
        uv_points: points,
    };
    definition.validate()?;
    Ok(definition)
}

pub fn fit_faces(vmf: &Vmf, faces: &[FaceId], options: Options) -> Result<Definition, String> {
    side_budget(faces.len())?;
    let first = *faces
        .first()
        .ok_or("Overlay fitting requires at least one source face")?;
    let definition = fit(vmf, first, options)?;
    fit_sources(vmf, faces, definition)
}

fn fit_sources(
    vmf: &Vmf,
    faces: &[FaceId],
    mut definition: Definition,
) -> Result<Definition, String> {
    if faces.len() == 1 {
        return Ok(definition);
    }
    if faces.iter().copied().collect::<BTreeSet<_>>().len() != faces.len() {
        return Err("Overlay fitting requires distinct source faces".into());
    }
    let (mins, maxs) = coplanar_bounds(vmf, faces, &definition)?;
    definition.sides = faces.iter().map(|face| face.side).collect();
    definition.sides.sort_unstable();
    definition.uv_points = [
        [mins[0], mins[1], 0.0],
        [mins[0], maxs[1], 0.0],
        [maxs[0], maxs[1], 0.0],
        [maxs[0], mins[1], 0.0],
    ];
    definition.validate()?;
    validate_editable_projection(&definition, &definition)?;
    Ok(definition)
}

fn coplanar_bounds(
    vmf: &Vmf,
    faces: &[FaceId],
    definition: &Definition,
) -> Result<([f64; 2], [f64; 2]), String> {
    let origin = DVec3::from_array(definition.basis_origin);
    let normal = DVec3::from_array(definition.basis_normal);
    let u = DVec3::from_array(definition.basis_u);
    let v = DVec3::from_array(definition.basis_v);
    let mut mins = [f64::INFINITY; 2];
    let mut maxs = [f64::NEG_INFINITY; 2];
    for &face in faces {
        let source = source_face(vmf, face)
            .map_err(|error| format!("Overlay brush {} face {}: {error}", face.solid, face.side))?;
        if normal.dot(source.normal) < 1.0 - COPLANAR_NORMAL_TOLERANCE
            || source
                .vertices
                .iter()
                .chain(&source.samples)
                .any(|point| normal.dot(*point - origin).abs() > ON_EPSILON)
        {
            return Err(format!(
                "Overlay brush {} face {} must be coplanar with the first source and face the same direction",
                face.solid, face.side
            ));
        }
        for point in source.vertices {
            let local = point - origin;
            for (axis, coordinate) in [local.dot(u), local.dot(v)].into_iter().enumerate() {
                mins[axis] = mins[axis].min(coordinate);
                maxs[axis] = maxs[axis].max(coordinate);
            }
        }
    }
    Ok((mins, maxs))
}

fn existing_definition(vmf: &Vmf, id: u32) -> Result<Definition, String> {
    vmf.validate_ids().map_err(|error| error.to_string())?;
    let entity = vmf
        .entities
        .iter()
        .find(|entity| entity.id == id)
        .ok_or("Overlay entity does not exist")?;
    if entity.hidden
        || !entity
            .classname()
            .is_some_and(|class| class.eq_ignore_ascii_case("info_overlay"))
    {
        return Err("Refit requires one visible info_overlay entity".into());
    }
    for key in [
        "origin",
        "sides",
        "BasisOrigin",
        "BasisNormal",
        "BasisU",
        "BasisV",
        "uv0",
        "uv1",
        "uv2",
        "uv3",
    ] {
        if entity.properties.get_all(key).count() > 1 {
            return Err(format!("Overlay refit requires one {key} field"));
        }
    }
    parse(entity)
}

pub fn refit_sources(vmf: &Vmf, id: u32) -> Result<(Vec<FaceId>, Definition), String> {
    let existing = existing_definition(vmf, id)?;
    let faces = resolve_sources(vmf, &existing)?;
    Ok((faces, existing))
}

fn resolve_sources(vmf: &Vmf, existing: &Definition) -> Result<Vec<FaceId>, String> {
    existing
        .sides
        .iter()
        .map(|side| {
            let solid = vmf
                .solids()
                .find(|solid| solid.sides.iter().any(|candidate| candidate.id == *side))
                .ok_or_else(|| format!("Overlay bound source side {side} does not exist"))?;
            Ok(FaceId {
                solid: solid.id,
                side: *side,
            })
        })
        .collect()
}

pub struct BoundSources {
    pub faces: Vec<FaceId>,
    pub references: Vec<FaceId>,
    definition: Definition,
    plane: Definition,
}

pub fn checked_bound_sources(vmf: &Vmf, id: u32) -> Result<BoundSources, String> {
    checked_bound_definition(vmf, existing_definition(vmf, id)?)
}

fn checked_bound_definition(vmf: &Vmf, definition: Definition) -> Result<BoundSources, String> {
    let faces = resolve_sources(vmf, &definition)?;
    let mut references = Vec::new();
    let mut plane = None;
    for &face in &faces {
        let source = source_face(vmf, face)
            .map_err(|error| format!("Overlay brush {} face {}: {error}", face.solid, face.side))?;
        if let Ok(candidate) = fit_source(
            face,
            source.normal,
            &source.vertices,
            definition.options.clone(),
        ) {
            references.push(face);
            if plane.is_none() {
                plane = Some(candidate);
            }
        }
    }
    let plane = plane.ok_or("Overlay editing requires an eligible bound quad")?;
    coplanar_bounds(vmf, &faces, &plane)?;
    Ok(BoundSources {
        faces,
        references,
        definition,
        plane,
    })
}

pub fn fit_existing(vmf: &Vmf, id: u32) -> Result<(FaceId, Definition), String> {
    let existing = existing_definition(vmf, id)?;
    let [side] = existing.sides.as_slice() else {
        return Err("Overlay refit requires exactly one existing bound source side".into());
    };
    let solid = vmf
        .solids()
        .find(|solid| solid.sides.iter().any(|candidate| candidate.id == *side))
        .ok_or("Overlay bound source side does not exist")?;
    let face = FaceId {
        solid: solid.id,
        side: *side,
    };
    let definition = fit(vmf, face, existing.options)?;
    Ok((face, definition))
}

pub fn fit_existing_refit(
    vmf: &Vmf,
    id: u32,
    reference: Option<FaceId>,
) -> Result<(Vec<FaceId>, Definition), String> {
    let existing = existing_definition(vmf, id)?;
    if existing.sides.len() == 1 {
        let (face, definition) = fit_existing(vmf, id)?;
        if reference.is_some_and(|reference| reference != face) {
            return Err("Overlay reference must belong to the existing source bindings".into());
        }
        return Ok((vec![face], definition));
    }
    let (mut faces, existing) = refit_sources(vmf, id)?;
    let reference =
        reference.ok_or("Choose a reference face from the overlay's existing bindings")?;
    let index = faces
        .iter()
        .position(|face| *face == reference)
        .ok_or("Overlay reference must belong to the existing source bindings")?;
    faces.remove(index);
    faces.insert(0, reference);
    let definition = fit_faces(vmf, &faces, existing.options)?;
    Ok((faces, definition))
}

pub fn editable_corners(vmf: &Vmf, id: u32) -> Result<(FaceId, Definition), String> {
    let (faces, definition) = editable_corner_faces(vmf, id)?;
    Ok((faces[0], definition))
}

fn single_corner_sources(vmf: &Vmf, id: u32) -> Result<BoundSources, String> {
    let (face, fitted) = fit_existing(vmf, id)?;
    let entity = vmf.entities.iter().find(|entity| entity.id == id).unwrap();
    let existing = parse(entity)?;
    validate_editable_projection(&existing, &fitted)?;
    Ok(BoundSources {
        faces: vec![face],
        references: vec![face],
        definition: existing,
        plane: fitted,
    })
}

fn corner_sources(vmf: &Vmf, id: u32) -> Result<BoundSources, String> {
    let definition = existing_definition(vmf, id)?;
    if definition.sides.len() == 1 {
        return single_corner_sources(vmf, id);
    }
    let sources = checked_bound_definition(vmf, definition)?;
    validate_editable_projection(&sources.definition, &sources.plane)?;
    Ok(sources)
}

pub fn editable_corner_faces(vmf: &Vmf, id: u32) -> Result<(Vec<FaceId>, Definition), String> {
    let sources = corner_sources(vmf, id)?;
    Ok((sources.faces, sources.definition))
}

fn validate_editable_projection(
    definition: &Definition,
    source: &Definition,
) -> Result<(), String> {
    let normal = DVec3::from_array(source.basis_normal);
    let origin = DVec3::from_array(source.basis_origin);
    let native_origin = DVec3::from_array(definition.basis_origin).as_vec3();
    let native_normal = DVec3::from_array(definition.basis_normal).as_vec3();
    let native_u = DVec3::from_array(definition.basis_u).as_vec3();
    let handedness = if DVec3::from_array(definition.basis_normal)
        .cross(DVec3::from_array(definition.basis_u))
        .dot(DVec3::from_array(definition.basis_v))
        < 0.0
    {
        -1.0
    } else {
        1.0
    };
    let native_v = native_normal.cross(native_u) * handedness;
    let native_points = definition
        .uv_points
        .map(|point| native_origin + native_u * point[0] as f32 + native_v * point[1] as f32);
    if normal.dot(DVec3::from_array(definition.basis_normal)).abs() < 1.0 - BASIS_TOLERANCE
        || definition
            .corners()
            .iter()
            .any(|point| normal.dot(DVec3::from_array(*point) - origin).abs() > ON_EPSILON)
        || native_points.iter().any(|point| {
            normal.dot(point.as_dvec3() - origin).abs() > ON_EPSILON
                || point.abs().max_element() > SOURCE_COORDINATE_LIMIT as f32
        })
    {
        return Err("Overlay corners must lie on the existing bound face plane, including native float precision; use Refit selected overlay after moving its source".into());
    }
    if native_points
        .iter()
        .zip(definition.corners())
        .any(|(native, authored)| {
            !native
                .as_dvec3()
                .abs_diff_eq(DVec3::from_array(authored), ON_EPSILON)
        })
    {
        return Err("Overlay corner frame differs from native projection precision; use Refit selected overlay before editing corners".into());
    }
    let mut turn = 0.0f64;
    for index in 0..4 {
        let [a, b, c] = [index, (index + 1) % 4, (index + 2) % 4].map(|index| native_points[index]);
        let cross = (b - a).cross(c - b).as_dvec3().dot(normal);
        if cross.abs() < QUAD_AREA_EPSILON || (turn != 0.0 && cross.signum() != turn.signum()) {
            return Err(
                "Overlay corners collapse or lose convexity at native world-coordinate precision"
                    .into(),
            );
        }
        turn = cross;
    }
    Ok(())
}

pub fn with_corners(
    vmf: &Vmf,
    id: u32,
    corners: [[f64; 2]; 4],
) -> Result<(FaceId, Definition), String> {
    let (faces, definition) = with_corner_faces(vmf, id, corners)?;
    Ok((faces[0], definition))
}

pub fn with_corner_faces(
    vmf: &Vmf,
    id: u32,
    corners: [[f64; 2]; 4],
) -> Result<(Vec<FaceId>, Definition), String> {
    let sources = corner_sources(vmf, id)?;
    let mut definition = sources.definition;
    for (point, corner) in definition.uv_points.iter_mut().zip(corners) {
        point[0] = corner[0];
        point[1] = corner[1];
    }
    definition.validate()?;
    validate_editable_projection(&definition, &sources.plane)?;
    Ok((sources.faces, definition))
}

pub fn edit_corners(
    tx: &mut Transaction<'_>,
    id: u32,
    corners: [[f64; 2]; 4],
) -> Result<ObjectId, EditError> {
    let (_, definition) =
        with_corners(tx.document().vmf(), id, corners).map_err(EditError::Invalid)?;
    let entity = tx.entity_mut(id)?;
    for (index, point) in definition.uv_points.iter().enumerate() {
        entity
            .properties
            .set(format!("uv{index}"), format_vec3(DVec3::from_array(*point)));
    }
    Ok(ObjectId::Entity(id))
}

pub fn fit_rebind(vmf: &Vmf, id: u32, source: FaceId) -> Result<Definition, String> {
    let (previous, existing) = fit_existing(vmf, id)?;
    if previous == source {
        return Err("Choose a different destination face, or use Refit selected overlay".into());
    }
    fit(vmf, source, existing.options)
}

pub fn rebind_sources(vmf: &Vmf, id: u32) -> Result<(Vec<FaceId>, Definition), String> {
    let existing = existing_definition(vmf, id)?;
    if existing.sides.len() == 1 {
        let (face, definition) = fit_existing(vmf, id)?;
        return Ok((vec![face], definition));
    }
    let bound = checked_bound_definition(vmf, existing)?;
    Ok((bound.faces, bound.definition))
}

pub fn fit_rebind_faces(
    vmf: &Vmf,
    id: u32,
    sources: &[FaceId],
) -> Result<(Vec<FaceId>, Definition), String> {
    side_budget(sources.len())?;
    let (previous, existing) = rebind_sources(vmf, id)?;
    if previous.len() == 1 && sources.len() == 1 {
        return Ok((previous, fit_rebind(vmf, id, sources[0])?));
    }
    let definition = fit_faces(vmf, sources, existing.options)?;
    if previous.iter().copied().collect::<BTreeSet<_>>()
        == sources.iter().copied().collect::<BTreeSet<_>>()
    {
        return Err("Choose different destination faces, or use Refit selected overlay".into());
    }
    Ok((previous, definition))
}

pub fn refit(tx: &mut Transaction<'_>, id: u32) -> Result<ObjectId, EditError> {
    let (_, definition) = fit_existing(tx.document().vmf(), id).map_err(EditError::Invalid)?;
    write_projection(tx, id, &definition, true)
}

pub fn refit_with_reference(
    tx: &mut Transaction<'_>,
    id: u32,
    reference: Option<FaceId>,
) -> Result<ObjectId, EditError> {
    let (faces, definition) =
        fit_existing_refit(tx.document().vmf(), id, reference).map_err(EditError::Invalid)?;
    if faces.len() == 1 {
        return refit(tx, id);
    }
    write_projection(tx, id, &definition, false)
}

pub fn rebind(tx: &mut Transaction<'_>, id: u32, source: FaceId) -> Result<ObjectId, EditError> {
    let definition = fit_rebind(tx.document().vmf(), id, source).map_err(EditError::Invalid)?;
    write_projection(tx, id, &definition, true)
}

pub fn rebind_faces(
    tx: &mut Transaction<'_>,
    id: u32,
    sources: &[FaceId],
) -> Result<ObjectId, EditError> {
    let (_, definition) =
        fit_rebind_faces(tx.document().vmf(), id, sources).map_err(EditError::Invalid)?;
    write_projection(tx, id, &definition, true)
}

fn write_projection(
    tx: &mut Transaction<'_>,
    id: u32,
    definition: &Definition,
    update_bindings: bool,
) -> Result<ObjectId, EditError> {
    let entity = tx.entity_mut(id)?;
    for (key, value) in [
        ("origin", definition.basis_origin),
        ("BasisOrigin", definition.basis_origin),
        ("BasisNormal", definition.basis_normal),
        ("BasisU", definition.basis_u),
        ("BasisV", definition.basis_v),
    ] {
        entity
            .properties
            .set(key, format_vec3(DVec3::from_array(value)));
    }
    if update_bindings {
        entity.properties.set(
            "sides",
            definition
                .sides
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(" "),
        );
    }
    for (index, point) in definition.uv_points.iter().enumerate() {
        entity
            .properties
            .set(format!("uv{index}"), format_vec3(DVec3::from_array(*point)));
    }
    Ok(ObjectId::Entity(id))
}

pub fn create(
    tx: &mut Transaction<'_>,
    face: FaceId,
    options: Options,
) -> Result<ObjectId, EditError> {
    create_faces(tx, &[face], options)
}

pub fn create_faces(
    tx: &mut Transaction<'_>,
    faces: &[FaceId],
    options: Options,
) -> Result<ObjectId, EditError> {
    let definition = fit_faces(tx.document().vmf(), faces, options).map_err(EditError::Invalid)?;
    let options = &definition.options;
    let mut properties = vec![
        ("material".into(), options.material.clone()),
        (
            "sides".into(),
            definition
                .sides
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(" "),
        ),
        (
            "BasisOrigin".into(),
            format_vec3(DVec3::from_array(definition.basis_origin)),
        ),
        (
            "BasisNormal".into(),
            format_vec3(DVec3::from_array(definition.basis_normal)),
        ),
        (
            "BasisU".into(),
            format_vec3(DVec3::from_array(definition.basis_u)),
        ),
        (
            "BasisV".into(),
            format_vec3(DVec3::from_array(definition.basis_v)),
        ),
        ("RenderOrder".into(), options.render_order.to_string()),
    ];
    for (index, point) in definition.uv_points.iter().enumerate() {
        properties.push((format!("uv{index}"), format_vec3(DVec3::from_array(*point))));
    }
    for (key, value) in [
        ("StartU", options.u[0]),
        ("EndU", options.u[1]),
        ("StartV", options.v[0]),
        ("EndV", options.v[1]),
        ("fademindist", options.fade_min),
        ("fademaxdist", options.fade_max),
    ] {
        properties.push((key.into(), format_number(f64::from(value))));
    }
    if let Some(name) = &options.targetname {
        properties.push(("targetname".into(), name.clone()));
    }
    edit::create_entity(
        tx,
        "info_overlay",
        Some(DVec3::from_array(definition.basis_origin)),
        &properties,
        Vec::new(),
    )
}

#[cfg(test)]
mod tests;
