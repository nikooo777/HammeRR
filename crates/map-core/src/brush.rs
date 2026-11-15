use glam::{DVec2, DVec3};
use source_formats::vmf::{Side, Solid};

use crate::plane::{ON_EPSILON, Plane};

pub(crate) mod precise;
pub use precise::{
    has_precise_vertices, prepare_precise_edit, refresh_precise, transform_precise,
    validate_precise, validate_precise_vmf,
};

/// One visible polygon of a brush.
#[derive(Debug, Clone, PartialEq)]
pub struct Face {
    /// Index into `Solid::sides`.
    pub side: usize,
    pub plane: Plane,
    /// Convex polygon, wound clockwise when viewed from outside the brush.
    pub vertices: Vec<DVec3>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BrushGeometry {
    pub faces: Vec<Face>,
    pub mins: DVec3,
    pub maxs: DVec3,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrushError {
    /// The side's three plane points are collinear.
    DegeneratePlane {
        side: usize,
    },
    /// The half-spaces don't enclose any volume.
    NoVolume,
    InvalidPrecision {
        message: String,
    },
}

impl std::fmt::Display for BrushError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DegeneratePlane { side } => write!(f, "side {side} has collinear plane points"),
            Self::NoVolume => f.write_str("brush planes enclose no volume"),
            Self::InvalidPrecision { message } => f.write_str(message),
        }
    }
}

impl std::error::Error for BrushError {}

impl BrushGeometry {
    /// Intersects the VMF plane half-spaces; editor vertex caches do not define the solid.
    pub fn from_solid(solid: &Solid) -> Result<Self, BrushError> {
        let planes = solid
            .sides
            .iter()
            .enumerate()
            .map(|(i, side)| {
                Plane::from_points(side.plane.0).ok_or(BrushError::DegeneratePlane { side: i })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Self::from_planes(&planes)
    }

    pub fn from_planes(planes: &[Plane]) -> Result<Self, BrushError> {
        Self::from_planes_with_epsilon(planes, ON_EPSILON)
    }

    /// Uses strict sidedness and exact duplicate suppression for collision geometry.
    pub fn from_planes_strict(planes: &[Plane]) -> Result<Self, BrushError> {
        Self::from_planes_with_epsilon(planes, 0.0)
    }

    fn from_planes_with_epsilon(planes: &[Plane], epsilon: f64) -> Result<Self, BrushError> {
        let mut faces = Vec::with_capacity(planes.len());
        'faces: for (i, plane) in planes.iter().enumerate() {
            let mut winding = plane.base_winding();
            for (j, other) in planes.iter().enumerate() {
                if i == j {
                    continue;
                }
                if other == plane || (epsilon != 0.0 && other.approx_eq(plane)) {
                    // A duplicated plane: the first occurrence owns the face.
                    if j < i {
                        continue 'faces;
                    }
                    continue;
                }
                winding = other.clip_behind_with_epsilon(&winding, epsilon);
                if winding.is_empty() {
                    continue 'faces;
                }
            }
            let vertices = dedup_ring(winding, epsilon);
            if vertices.len() >= 3 {
                faces.push(Face {
                    side: i,
                    plane: *plane,
                    vertices,
                });
            }
        }

        if faces.len() < 4 {
            return Err(BrushError::NoVolume);
        }
        let (mins, maxs) = faces
            .iter()
            .flat_map(|f| &f.vertices)
            .fold((DVec3::MAX, DVec3::MIN), |(lo, hi), &p| {
                (lo.min(p), hi.max(p))
            });
        Ok(Self { faces, mins, maxs })
    }

    pub fn center(&self) -> DVec3 {
        (self.mins + self.maxs) * 0.5
    }
}

/// Drops consecutive vertices within the chosen tolerance, including the
/// wrap-around pair.
fn dedup_ring(mut points: Vec<DVec3>, epsilon: f64) -> Vec<DVec3> {
    points.dedup_by(|a, b| a == b || a.distance_squared(*b) < epsilon * epsilon);
    while points.len() > 1
        && (points[0] == points[points.len() - 1]
            || points[0].distance_squared(points[points.len() - 1]) < epsilon * epsilon)
    {
        points.pop();
    }
    points
}

/// Texture coordinates of `point` on `side`, in texels. Divide by the
/// texture's width and height to get normalized UVs.
pub fn texel_uv(side: &Side, point: DVec3) -> DVec2 {
    let project =
        |axis: &source_formats::vmf::UvAxis| point.dot(axis.axis) / axis.scale + axis.offset;
    DVec2::new(project(&side.u_axis), project(&side.v_axis))
}
