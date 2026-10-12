use std::f64::consts::{PI, TAU};

use glam::DVec3;
use source_formats::vmf::{Properties, Solid};

use crate::{BrushGeometry, EditError, ObjectId, Plane, Transaction, edit};

pub const MAX_SEGMENTS: u32 = 64;
pub const MAX_FACES: u32 = 256;
pub const MAX_COORDINATE: f64 = 65_536.0;
pub const MIN_SIZE: f64 = 0.125;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    Wedge,
    Cylinder,
    Cone,
    Sphere,
    Arch,
}

impl Shape {
    pub fn parse(value: &str) -> Result<Self, EditError> {
        match value {
            "wedge" => Ok(Self::Wedge),
            "cylinder" => Ok(Self::Cylinder),
            "cone" => Ok(Self::Cone),
            "sphere" => Ok(Self::Sphere),
            "arch" => Ok(Self::Arch),
            _ => Err(invalid("Choose wedge, cylinder, cone, sphere or arch")),
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Wedge => "wedge",
            Self::Cylinder => "cylinder",
            Self::Cone => "cone",
            Self::Sphere => "sphere",
            Self::Arch => "arch",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    pub fn parse(value: &str) -> Result<Self, EditError> {
        match value {
            "x" => Ok(Self::X),
            "y" => Ok(Self::Y),
            "z" => Ok(Self::Z),
            _ => Err(invalid("Primitive axis must be x, y or z")),
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::X => "x",
            Self::Y => "y",
            Self::Z => "z",
        }
    }
    pub fn indices(self) -> [usize; 3] {
        match self {
            Self::X => [1, 2, 0],
            Self::Y => [0, 2, 1],
            Self::Z => [0, 1, 2],
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Spec {
    pub shape: Shape,
    pub mins: DVec3,
    pub maxs: DVec3,
    pub axis: Axis,
    pub segments: u32,
    pub bands: u32,
    pub inset: f64,
    pub start_angle: f64,
    pub sweep_angle: f64,
    pub material: String,
}

impl Default for Spec {
    fn default() -> Self {
        Self {
            shape: Shape::Wedge,
            mins: DVec3::new(-64.0, -64.0, 0.0),
            maxs: DVec3::new(64.0, 64.0, 128.0),
            axis: Axis::Z,
            segments: 12,
            bands: 6,
            inset: 16.0,
            start_angle: 0.0,
            sweep_angle: 180.0,
            material: "TOOLS/TOOLSNODRAW".into(),
        }
    }
}

fn invalid(message: impl Into<String>) -> EditError {
    EditError::Invalid(message.into())
}

impl Spec {
    pub fn validate(&self) -> Result<(), EditError> {
        if !self.mins.is_finite()
            || !self.maxs.is_finite()
            || !(self.maxs - self.mins).cmpge(DVec3::splat(MIN_SIZE)).all()
            || self.mins.abs().max(self.maxs.abs()).max_element() > MAX_COORDINATE
        {
            return Err(invalid(
                "Primitive bounds must be finite, within ±65536 units, and at least 0.125 units thick on every axis",
            ));
        }
        if self.material.trim().is_empty()
            || self.material.chars().any(|c| c.is_control() || c == '"')
        {
            return Err(invalid(
                "Choose a nonempty material path without quotes or control characters",
            ));
        }
        if self.shape != Shape::Wedge && !(3..=MAX_SEGMENTS).contains(&self.segments) {
            return Err(invalid(format!(
                "Curved shapes need 3–{MAX_SEGMENTS} segments"
            )));
        }
        if self.shape == Shape::Sphere
            && (!(2..=32).contains(&self.bands)
                || self
                    .segments
                    .checked_mul(self.bands)
                    .is_none_or(|n| n > MAX_FACES))
        {
            return Err(invalid(format!(
                "Spheres need 2–32 latitude bands and at most {MAX_FACES} faces (segments × bands)"
            )));
        }
        if self.shape == Shape::Arch {
            let [u, v, _] = self.axis.indices();
            let radius = (self.maxs - self.mins) * 0.5;
            if !self.inset.is_finite()
                || self.inset < MIN_SIZE
                || self.inset >= radius[u].min(radius[v])
            {
                return Err(invalid(
                    "Arch wall inset must be at least 0.125 units and smaller than both ellipse radii",
                ));
            }
            if !self.start_angle.is_finite()
                || !self.sweep_angle.is_finite()
                || !(1.0..=360.0).contains(&self.sweep_angle)
            {
                return Err(invalid(
                    "Arch start angle must be finite; sweep must be 1–360 degrees",
                ));
            }
        }
        Ok(())
    }

    fn point(&self, local: DVec3) -> DVec3 {
        let [u, v, w] = self.axis.indices();
        let size = self.maxs - self.mins;
        let mut point = self.mins;
        point[u] += local.x * size[u];
        point[v] += local.y * size[v];
        point[w] += local.z * size[w];
        point
    }

    fn ring(&self, radius: f64, height: f64) -> Vec<DVec3> {
        (0..self.segments)
            .map(|index| {
                let angle = TAU * f64::from(index) / f64::from(self.segments);
                DVec3::new(
                    0.5 + radius * angle.cos(),
                    0.5 + radius * angle.sin(),
                    height,
                )
            })
            .collect()
    }

    fn polygons(&self) -> Vec<Vec<Vec<DVec3>>> {
        match self.shape {
            Shape::Wedge => {
                let a = DVec3::new(0.0, 0.0, 0.0);
                let b = DVec3::new(1.0, 0.0, 0.0);
                let c = DVec3::new(1.0, 1.0, 0.0);
                let d = DVec3::new(0.0, 1.0, 0.0);
                let e = DVec3::new(1.0, 0.0, 1.0);
                let f = DVec3::new(1.0, 1.0, 1.0);
                vec![vec![
                    vec![a, b, c, d],
                    vec![b, e, f, c],
                    vec![a, b, e],
                    vec![d, c, f],
                    vec![a, d, f, e],
                ]]
            }
            Shape::Cylinder => vec![prism(&self.ring(0.5, 0.0), &self.ring(0.5, 1.0))],
            Shape::Cone => {
                let base = self.ring(0.5, 0.0);
                let apex = DVec3::new(0.5, 0.5, 1.0);
                let mut faces = vec![base.clone()];
                for i in 0..base.len() {
                    faces.push(vec![base[i], base[(i + 1) % base.len()], apex]);
                }
                vec![faces]
            }
            Shape::Sphere => {
                let rings = (1..self.bands)
                    .map(|band| {
                        let angle = PI * f64::from(band) / f64::from(self.bands);
                        self.ring(0.5 * angle.sin(), 0.5 + 0.5 * angle.cos())
                    })
                    .collect::<Vec<_>>();
                let mut faces = Vec::new();
                let n = self.segments as usize;
                for i in 0..n {
                    let next = (i + 1) % n;
                    faces.push(vec![DVec3::new(0.5, 0.5, 1.0), rings[0][i], rings[0][next]]);
                    for band in rings.windows(2) {
                        faces.push(vec![band[0][i], band[0][next], band[1][next], band[1][i]]);
                    }
                    faces.push(vec![
                        DVec3::new(0.5, 0.5, 0.0),
                        rings.last().unwrap()[i],
                        rings.last().unwrap()[next],
                    ]);
                }
                vec![faces]
            }
            Shape::Arch => {
                let [u, v, _] = self.axis.indices();
                let size = self.maxs - self.mins;
                let inner = DVec3::new(0.5 - self.inset / size[u], 0.5 - self.inset / size[v], 0.0);
                let point = |angle: f64, radius: DVec3| {
                    DVec3::new(
                        0.5 + radius.x * angle.cos(),
                        0.5 + radius.y * angle.sin(),
                        0.0,
                    )
                };
                (0..self.segments)
                    .map(|index| {
                        let angle = |step: u32| {
                            (self.start_angle.rem_euclid(360.0)
                                + self.sweep_angle * f64::from(step) / f64::from(self.segments))
                            .to_radians()
                        };
                        let a = angle(index);
                        let b = angle(index + 1);
                        let bottom = vec![
                            point(a, DVec3::splat(0.5)),
                            point(b, DVec3::splat(0.5)),
                            point(b, inner),
                            point(a, inner),
                        ];
                        let top = bottom.iter().map(|p| *p + DVec3::Z).collect::<Vec<_>>();
                        prism(&bottom, &top)
                    })
                    .collect()
            }
        }
        .into_iter()
        .map(|brush| {
            brush
                .into_iter()
                .map(|face| face.into_iter().map(|p| self.point(p)).collect())
                .collect()
        })
        .collect()
    }
}

fn prism(bottom: &[DVec3], top: &[DVec3]) -> Vec<Vec<DVec3>> {
    let mut faces = vec![bottom.to_vec(), top.to_vec()];
    for i in 0..bottom.len() {
        let j = (i + 1) % bottom.len();
        faces.push(vec![bottom[i], bottom[j], top[j], top[i]]);
    }
    faces
}

pub(crate) fn convex_planes(faces: Vec<Vec<DVec3>>) -> Result<Vec<[DVec3; 3]>, EditError> {
    let vertices = faces.iter().flatten().copied().collect::<Vec<_>>();
    let center = vertices.iter().sum::<DVec3>() / vertices.len() as f64;
    let mut points = Vec::new();
    let mut planes = Vec::new();
    for face in &faces {
        let mut triangle = [face[0], face[1], face[2]];
        let mut plane = Plane::from_points(triangle).ok_or_else(|| {
            invalid("Primitive has a degenerate face; increase its size or reduce segments")
        })?;
        if plane.distance_to(center) > 0.0 {
            triangle.swap(0, 2);
            plane = Plane::from_points(triangle).unwrap();
        }
        if plane.distance_to(center) >= -1e-6
            || vertices
                .iter()
                .any(|&point| plane.distance_to(point) > 1e-5)
        {
            return Err(invalid(
                "Primitive does not form a convex solid; increase its size or reduce segments",
            ));
        }
        points.push(triangle);
        planes.push(plane);
    }
    let geometry = BrushGeometry::from_planes(&planes)
        .map_err(|error| invalid(format!("Primitive: {error}")))?;
    if geometry.faces.len() != faces.len()
        || !geometry.mins.is_finite()
        || !geometry.maxs.is_finite()
        || !(geometry.maxs - geometry.mins)
            .cmpgt(DVec3::splat(1e-6))
            .all()
    {
        return Err(invalid(
            "Primitive faces collapsed; increase its size or reduce segments",
        ));
    }
    let mins = vertices.iter().copied().fold(DVec3::MAX, DVec3::min);
    let maxs = vertices.iter().copied().fold(DVec3::MIN, DVec3::max);
    if !geometry.mins.abs_diff_eq(mins, 0.002) || !geometry.maxs.abs_diff_eq(maxs, 0.002) {
        return Err(invalid(
            "Primitive exceeds its expected bounds; increase its size or reduce segments",
        ));
    }
    Ok(points)
}

pub fn create(tx: &mut Transaction, spec: &Spec) -> Result<Vec<ObjectId>, EditError> {
    spec.validate()?;
    let planned = spec
        .polygons()
        .into_iter()
        .map(convex_planes)
        .collect::<Result<Vec<_>, _>>()?;
    let mut ids = Vec::with_capacity(planned.len());
    for points in planned {
        let id = tx.new_object_id()?;
        let sides = points
            .into_iter()
            .map(|points| edit::new_side(tx, points, &spec.material))
            .collect::<Result<Vec<_>, _>>()?;
        ids.push(tx.add_solid(Solid {
            id,
            sides,
            editor: Some(Properties(vec![
                ("color".into(), "160 192 224".into()),
                ("visgroupshown".into(), "1".into()),
                ("visgroupautoshown".into(), "1".into()),
            ])),
            hidden: false,
            extra_properties: Properties::default(),
            extra: vec![],
        })?);
    }
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Document, texel_uv};
    use source_formats::vmf::Vmf;

    fn volume(geometry: &BrushGeometry) -> f64 {
        let center = geometry.center();
        geometry
            .faces
            .iter()
            .map(|face| {
                let a = face.vertices[0] - center;
                face.vertices[1..]
                    .windows(2)
                    .map(|edge| a.dot((edge[0] - center).cross(edge[1] - center)).abs() / 6.0)
                    .sum::<f64>()
            })
            .sum()
    }

    #[test]
    fn every_shape_and_axis_produces_closed_convex_brushes_with_valid_uvs() {
        for axis in [Axis::X, Axis::Y, Axis::Z] {
            for shape in [
                Shape::Wedge,
                Shape::Cylinder,
                Shape::Cone,
                Shape::Sphere,
                Shape::Arch,
            ] {
                let mut doc = Document::empty();
                let spec = Spec {
                    shape,
                    axis,
                    mins: DVec3::new(-72.0, 16.0, -32.0),
                    maxs: DVec3::new(120.0, 144.0, 224.0),
                    material: "brick/test".into(),
                    ..Spec::default()
                };
                let before = doc.vmf().clone();
                let ids = doc.edit("primitive", |tx| create(tx, &spec)).unwrap();
                assert_eq!(
                    ids.len(),
                    if shape == Shape::Arch {
                        spec.segments as usize
                    } else {
                        1
                    }
                );
                let mut side_ids = std::collections::BTreeSet::new();
                for id in &ids {
                    let ObjectId::Solid(id) = *id else { panic!() };
                    let solid = doc.solid(id).unwrap();
                    let geometry = BrushGeometry::from_solid(solid).unwrap();
                    assert_eq!(geometry.faces.len(), solid.sides.len());
                    assert!(
                        (geometry.mins - spec.mins)
                            .cmpge(DVec3::splat(-0.002))
                            .all()
                    );
                    assert!(
                        (spec.maxs - geometry.maxs)
                            .cmpge(DVec3::splat(-0.002))
                            .all()
                    );
                    assert!(volume(&geometry) > 0.0);
                    for side in &solid.sides {
                        assert!(side_ids.insert(side.id));
                        assert_eq!(side.material, "BRICK/TEST");
                        assert_eq!(side.lightmap_scale, 16);
                    }
                    for face in &geometry.faces {
                        for &point in &face.vertices {
                            assert!(solid.sides.iter().all(|side| {
                                Plane::from_points(side.plane.0).unwrap().distance_to(point) < 0.002
                            }));
                            assert!(texel_uv(&solid.sides[face.side], point).is_finite());
                            assert!(
                                solid
                                    .sides
                                    .iter()
                                    .filter(|side| Plane::from_points(side.plane.0)
                                        .unwrap()
                                        .distance_to(point)
                                        .abs()
                                        < 0.002)
                                    .count()
                                    >= 3
                            );
                        }
                    }
                }
                let saved = doc.vmf().write(&Default::default());
                let reopened = Vmf::parse(&saved).unwrap();
                assert_eq!(&reopened, doc.vmf());
                doc.undo().unwrap();
                assert_eq!(doc.vmf(), &before);
                doc.redo().unwrap();
                assert_eq!(doc.vmf(), &reopened);
            }
        }
    }

    #[test]
    fn wedge_cylinder_cone_and_arch_volumes_match_their_parameters() {
        for shape in [Shape::Wedge, Shape::Cylinder, Shape::Cone, Shape::Arch] {
            for axis in [Axis::X, Axis::Y, Axis::Z] {
                let spec = Spec {
                    shape,
                    axis,
                    segments: 16,
                    inset: 24.0,
                    start_angle: 23.0,
                    sweep_angle: 270.0,
                    ..Spec::default()
                };
                let mut doc = Document::empty();
                doc.edit("primitive", |tx| create(tx, &spec)).unwrap();
                let actual = doc
                    .vmf()
                    .world
                    .solids
                    .iter()
                    .map(|solid| volume(&BrushGeometry::from_solid(solid).unwrap()))
                    .sum::<f64>();
                let [u, v, w] = axis.indices();
                let size = spec.maxs - spec.mins;
                let (a, b, h) = (size[u] / 2.0, size[v] / 2.0, size[w]);
                let n = f64::from(spec.segments);
                let expected = match shape {
                    Shape::Wedge => size.element_product() / 2.0,
                    Shape::Cylinder => n * 0.5 * (TAU / n).sin() * a * b * h,
                    Shape::Cone => n * (TAU / n).sin() * a * b * h / 6.0,
                    Shape::Arch => {
                        n * 0.5
                            * (spec.sweep_angle.to_radians() / n).sin()
                            * (a * b - (a - spec.inset) * (b - spec.inset))
                            * h
                    }
                    _ => unreachable!(),
                };
                assert!(
                    (actual - expected).abs() < 0.001,
                    "{shape:?}/{axis:?}: {actual} != {expected}"
                );
            }
        }
    }

    #[test]
    fn face_limits_and_small_valid_shapes_are_supported() {
        for (shape, segments, bands) in [
            (Shape::Cylinder, 64, 2),
            (Shape::Cone, 64, 2),
            (Shape::Sphere, 32, 8),
            (Shape::Sphere, 3, 2),
            (Shape::Arch, 64, 2),
        ] {
            let mut doc = Document::empty();
            let spec = Spec {
                shape,
                segments,
                bands,
                sweep_angle: 360.0,
                ..Spec::default()
            };
            doc.edit("primitive", |tx| create(tx, &spec)).unwrap();
        }
        let mut doc = Document::empty();
        let spec = Spec {
            mins: DVec3::ZERO,
            maxs: DVec3::splat(MIN_SIZE),
            ..Spec::default()
        };
        doc.edit("small wedge", |tx| create(tx, &spec)).unwrap();
    }

    #[test]
    fn invalid_parameters_and_geometric_failures_leave_the_document_and_ids_unchanged() {
        let mut specs = vec![
            Spec {
                shape: Shape::Cylinder,
                segments: 2,
                ..Spec::default()
            },
            Spec {
                shape: Shape::Sphere,
                segments: 64,
                bands: 32,
                ..Spec::default()
            },
            Spec {
                shape: Shape::Sphere,
                bands: 1,
                ..Spec::default()
            },
            Spec {
                shape: Shape::Arch,
                inset: 64.0,
                ..Spec::default()
            },
            Spec {
                shape: Shape::Arch,
                inset: f64::NAN,
                ..Spec::default()
            },
            Spec {
                shape: Shape::Arch,
                sweep_angle: 0.0,
                ..Spec::default()
            },
            Spec {
                shape: Shape::Arch,
                start_angle: f64::INFINITY,
                ..Spec::default()
            },
            Spec {
                material: "bad\"path".into(),
                ..Spec::default()
            },
        ];
        for value in [f64::NAN, f64::INFINITY, MAX_COORDINATE + 1.0, -65.0] {
            specs.push(Spec {
                maxs: DVec3::splat(value),
                ..Spec::default()
            });
        }
        for spec in specs {
            let mut doc = Document::empty();
            let before = doc.vmf().clone();
            assert!(doc.edit("invalid", |tx| create(tx, &spec)).is_err());
            assert_eq!(doc.vmf(), &before);
            assert_eq!(doc.revision(), 0);
            let ids = doc
                .edit("valid", |tx| create(tx, &Spec::default()))
                .unwrap();
            assert_eq!(ids, [ObjectId::Solid(2)]);
            assert_eq!(doc.solid(2).unwrap().sides[0].id, 1);
        }
    }
}
