use glam::{DVec2, DVec3};

use crate::{Plane, query::Ray};

pub const MAX_COORDINATE: f64 = crate::measurements::MAX_COORDINATE;
pub const MIN_GRID: f64 = 0.125;
pub const MAX_GRID: f64 = 4096.0;
pub const MAX_HALF_CELLS: i32 = 64;
const ORTHOGONAL_TOLERANCE: f64 = 1e-8;
const PARALLEL_TOLERANCE: f64 = 1e-10;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    origin: DVec3,
    normal: DVec3,
    tangent: DVec3,
    bitangent: DVec3,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    frame: Frame,
    spacing: f64,
    half_extent: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Intersection {
    pub point: DVec3,
    pub coordinates: DVec2,
    pub distance: f64,
    pub snapped: bool,
}

pub fn validate_point(point: DVec3) -> Result<(), String> {
    if !point.is_finite() || point.abs().max_element() > MAX_COORDINATE {
        return Err(format!(
            "Workplane points must be finite and within ±{MAX_COORDINATE} units"
        ));
    }
    Ok(())
}

pub fn validate_grid_spacing(spacing: f64) -> Result<(), String> {
    if !spacing.is_finite() || !(MIN_GRID..=MAX_GRID).contains(&spacing) {
        return Err(format!(
            "Grid spacing must be between {MIN_GRID} and {MAX_GRID} units"
        ));
    }
    Ok(())
}

fn normalized(axis: DVec3, label: &str) -> Result<DVec3, String> {
    if !axis.is_finite() || axis == DVec3::ZERO {
        return Err(format!("Workplane {label} must be a finite nonzero vector"));
    }
    (axis / axis.abs().max_element())
        .try_normalize()
        .filter(|value| value.is_finite())
        .ok_or_else(|| format!("Workplane {label} could not be normalized"))
}

impl Default for Frame {
    fn default() -> Self {
        Self {
            origin: DVec3::ZERO,
            normal: DVec3::Z,
            tangent: DVec3::X,
            bitangent: DVec3::Y,
        }
    }
}

impl Frame {
    pub fn new(origin: DVec3, normal: DVec3, tangent: Option<DVec3>) -> Result<Self, String> {
        validate_point(origin)?;
        let normal = normalized(normal, "normal")?;
        let tangent = match tangent {
            Some(tangent) => {
                let tangent = normalized(tangent, "tangent")?;
                if normal.dot(tangent).abs() > ORTHOGONAL_TOLERANCE {
                    return Err("Workplane tangent must be perpendicular to its normal".into());
                }
                tangent
            }
            None => {
                let axis = if normal.x.abs() <= normal.y.abs() && normal.x.abs() <= normal.z.abs() {
                    DVec3::X
                } else if normal.y.abs() <= normal.z.abs() {
                    DVec3::Y
                } else {
                    DVec3::Z
                };
                normalized(axis - normal * axis.dot(normal), "default tangent")?
            }
        };
        let bitangent = normalized(normal.cross(tangent), "bitangent")?;
        let tangent = normalized(bitangent.cross(normal), "tangent")?;
        Ok(Self {
            origin,
            normal,
            tangent,
            bitangent,
        })
    }

    pub fn origin(self) -> DVec3 {
        self.origin
    }
    pub fn normal(self) -> DVec3 {
        self.normal
    }
    pub fn tangent(self) -> DVec3 {
        self.tangent
    }
    pub fn bitangent(self) -> DVec3 {
        self.bitangent
    }

    pub fn with_origin(self, origin: DVec3) -> Result<Self, String> {
        validate_point(origin)?;
        Ok(Self { origin, ..self })
    }

    pub fn coordinates(self, point: DVec3) -> Result<DVec3, String> {
        validate_point(point)?;
        let relative = point - self.origin;
        let coordinates = DVec3::new(
            relative.dot(self.tangent),
            relative.dot(self.bitangent),
            self.normal.dot(relative),
        );
        if !coordinates.is_finite() {
            return Err("Workplane coordinates are not finite".into());
        }
        Ok(coordinates)
    }

    pub fn point_at(self, coordinates: DVec2) -> Result<DVec3, String> {
        if !coordinates.is_finite() {
            return Err("Workplane coordinates must be finite".into());
        }
        let point = self.origin + self.tangent * coordinates.x + self.bitangent * coordinates.y;
        validate_point(point)?;
        Ok(point)
    }

    pub fn intersect(self, ray: Ray) -> Result<Intersection, String> {
        if !ray.origin.is_finite() {
            return Err("Workplane ray origin must be finite".into());
        }
        let direction = normalized(ray.direction, "ray direction")?;
        let denominator = self.normal.dot(direction);
        if denominator.abs() <= PARALLEL_TOLERANCE {
            return Err("Ray is parallel or too close to parallel to the workplane".into());
        }
        let distance = self.normal.dot(self.origin - ray.origin) / denominator;
        if !distance.is_finite() || distance < 0.0 {
            return Err("Workplane intersection is behind the ray or is not finite".into());
        }
        let point = ray.origin + direction * distance;
        let coordinates = self.coordinates(point)?;
        let plane = Plane {
            normal: self.normal,
            dist: self.normal.dot(self.origin),
        };
        if plane.distance_to(point).abs() > crate::plane::ON_EPSILON {
            return Err("Workplane intersection lost plane precision".into());
        }
        let coordinates = coordinates.truncate();
        Ok(Intersection {
            point: self.point_at(coordinates)?,
            coordinates,
            distance,
            snapped: false,
        })
    }
}

impl Default for Grid {
    fn default() -> Self {
        Self {
            frame: Frame::default(),
            spacing: 16.0,
            half_extent: 512.0,
        }
    }
}

impl Grid {
    pub fn new(frame: Frame, spacing: f64, half_extent: f64) -> Result<Self, String> {
        validate_grid_spacing(spacing)?;
        if !half_extent.is_finite() || !(MIN_GRID..=MAX_COORDINATE).contains(&half_extent) {
            return Err(format!(
                "Workplane grid half extent must be between {MIN_GRID} and {MAX_COORDINATE} units"
            ));
        }
        for u in [-half_extent, half_extent] {
            for v in [-half_extent, half_extent] {
                frame.point_at(DVec2::new(u, v))?;
            }
        }
        Ok(Self {
            frame,
            spacing,
            half_extent,
        })
    }

    pub fn frame(self) -> Frame {
        self.frame
    }
    pub fn spacing(self) -> f64 {
        self.spacing
    }
    pub fn half_extent(self) -> f64 {
        self.half_extent
    }

    pub fn display_spacing(self) -> f64 {
        let mut spacing = self.spacing;
        while self.half_extent / spacing > f64::from(MAX_HALF_CELLS) {
            spacing *= 2.0;
        }
        spacing
    }

    pub fn intersect(self, ray: Ray, snap: bool) -> Result<Intersection, String> {
        let mut hit = self.frame.intersect(ray)?;
        if snap {
            hit.coordinates = (hit.coordinates / self.spacing).round() * self.spacing;
            hit.point = self.frame.point_at(hit.coordinates)?;
            hit.snapped = true;
        }
        Ok(hit)
    }
}
