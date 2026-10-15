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

pub fn fit(vmf: &Vmf, face: FaceId, mut options: Options) -> Result<Definition, String> {
    vmf.validate_ids().map_err(|error| error.to_string())?;
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
    if solid.sides.iter().any(|side| side.displacement.is_some()) {
        return Err("Overlay fitting does not support terrain brushes".into());
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
    let [a, b, c, d] = target.vertices.as_slice() else {
        return Err("Overlay fitting requires one convex planar quad face".into());
    };
    if target
        .vertices
        .iter()
        .any(|point| !point.is_finite() || target.plane.distance_to(*point).abs() > ON_EPSILON)
    {
        return Err("Overlay fitting requires one convex planar quad face".into());
    }
    let origin = (*a + *b + *c + *d) * 0.25;
    let u = (*d - *a)
        .try_normalize()
        .ok_or("Overlay fitting requires nondegenerate quad edges")?;
    let normal = target.plane.normal;
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

pub fn fit_existing(vmf: &Vmf, id: u32) -> Result<(FaceId, Definition), String> {
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
    let existing = parse(entity)?;
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

pub fn fit_rebind(vmf: &Vmf, id: u32, source: FaceId) -> Result<Definition, String> {
    let (previous, existing) = fit_existing(vmf, id)?;
    if previous == source {
        return Err("Choose a different destination face, or use Refit selected overlay".into());
    }
    fit(vmf, source, existing.options)
}

pub fn refit(tx: &mut Transaction<'_>, id: u32) -> Result<ObjectId, EditError> {
    let (face, definition) = fit_existing(tx.document().vmf(), id).map_err(EditError::Invalid)?;
    write_projection(tx, id, face, &definition)
}

pub fn rebind(tx: &mut Transaction<'_>, id: u32, source: FaceId) -> Result<ObjectId, EditError> {
    let definition = fit_rebind(tx.document().vmf(), id, source).map_err(EditError::Invalid)?;
    write_projection(tx, id, source, &definition)
}

fn write_projection(
    tx: &mut Transaction<'_>,
    id: u32,
    face: FaceId,
    definition: &Definition,
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
    entity.properties.set("sides", face.side.to_string());
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
    let definition = fit(tx.document().vmf(), face, options).map_err(EditError::Invalid)?;
    let options = &definition.options;
    let mut properties = vec![
        ("material".into(), options.material.clone()),
        ("sides".into(), face.side.to_string()),
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
