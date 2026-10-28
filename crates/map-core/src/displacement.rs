use glam::DVec3;
use source_formats::vmf::DispInfo;

use crate::brush::Face;

pub mod brush;
pub mod collision;
pub mod edit;
pub mod lod;
pub mod native;
pub mod neighbors;
pub mod resample;
pub mod sew;
pub mod smoothing;
pub mod stroke;
pub mod subdivide;
pub mod surface;
pub mod triangles;

pub const MAX_GRID_VERTICES: usize = 289;

#[derive(Debug, Clone, Copy)]
pub enum VertexValue {
    Position(DVec3),
    WorldZ(f64),
}

#[derive(Debug, Clone, Copy)]
pub struct VertexSet {
    pub vertex: usize,
    pub value: VertexValue,
}

#[derive(Debug, Clone)]
pub enum Operation {
    Resolution(u32),
    Sewn(Box<source_formats::vmf::DispInfo>),
    Subdivided(Box<source_formats::vmf::DispInfo>),
    Collision(collision::Settings),
    Smoothing(Option<f64>),
    TriangleTags {
        triangles: Vec<usize>,
        settings: triangles::Settings,
    },
    Masked {
        operation: Box<Operation>,
        vertices: Vec<usize>,
    },
    Sculpt {
        brush: brush::Brush,
        operation: brush::Sculpt,
    },
    Blend {
        brush: brush::Brush,
        operation: brush::Blend,
    },
    Stroke {
        spec: stroke::Spec,
        points: Vec<DVec3>,
    },
    VertexMove {
        vertices: Vec<usize>,
        offset: DVec3,
    },
    VertexSet {
        vertices: Vec<VertexSet>,
    },
}

/// The surface of a displaced face: a `size` x `size` vertex grid.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplacementMesh {
    pub size: usize,
    /// Row-major, `positions[row * size + col]`.
    pub positions: Vec<DVec3>,
    /// The same grid before displacement, on the flat face. Texture
    /// coordinates are taken from these.
    pub base_positions: Vec<DVec3>,
    /// Blend weights for two-texture blending, 0..=255.
    pub alphas: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisplacementError {
    NotAQuad { vertices: usize },
    UnsupportedPower(u32),
    BadRows { field: &'static str },
    InvalidValues { field: &'static str },
    UnpairedDeformation,
}

impl std::fmt::Display for DisplacementError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAQuad { vertices } => {
                write!(f, "displaced face has {vertices} vertices, expected 4")
            }
            Self::UnsupportedPower(p) => write!(f, "unsupported displacement power {p}"),
            Self::BadRows { field } => write!(f, "{field} rows don't match the grid size"),
            Self::InvalidValues { field } => write!(
                f,
                "Terrain {field} contains nonfinite or unrepresentable values"
            ),
            Self::UnpairedDeformation => {
                write!(f, "Terrain normals and distances must be provided together")
            }
        }
    }
}

impl std::error::Error for DisplacementError {}

fn grid<'a, T>(
    rows: Option<&'a [Vec<T>]>,
    size: usize,
    field: &'static str,
) -> Result<Option<impl Iterator<Item = &'a T>>, DisplacementError> {
    let Some(rows) = rows else {
        return Ok(None);
    };
    if rows.len() != size || rows.iter().any(|r| r.len() != size) {
        return Err(DisplacementError::BadRows { field });
    }
    Ok(Some(rows.iter().flatten()))
}

impl DisplacementMesh {
    fn validate_edit_bounds(&self) -> Result<(), crate::EditError> {
        if self
            .positions
            .iter()
            .any(|point| point.abs().max_element() > crate::primitive::MAX_COORDINATE)
            || self
                .alphas
                .iter()
                .any(|alpha| !(0.0..=255.0).contains(alpha))
        {
            return Err(crate::EditError::Invalid(
                "Terrain positions must fit ±65536 and blend values must be 0–255".into(),
            ));
        }
        Ok(())
    }

    fn offset_normals_need_remapping(&self, info: &DispInfo) -> bool {
        info.offset_normals.as_ref().is_some_and(|rows| {
            rows.len() != self.size
                || rows.iter().any(|row| {
                    row.len() != self.size || row.iter().any(|point| *point != DVec3::ZERO)
                })
        })
    }

    /// Rows run from the corner nearest `startposition` along the face's
    /// next edge in winding order; columns run along the previous edge.
    pub fn build(face: &Face, disp: &DispInfo) -> Result<Self, DisplacementError> {
        let &[a, b, c, d] = face.vertices.as_slice() else {
            return Err(DisplacementError::NotAQuad {
                vertices: face.vertices.len(),
            });
        };
        if !(2..=4).contains(&disp.power) {
            return Err(DisplacementError::UnsupportedPower(disp.power));
        }
        if !disp.start_position.is_finite() {
            return Err(DisplacementError::InvalidValues {
                field: "startposition",
            });
        }
        if !disp.elevation.is_finite() {
            return Err(DisplacementError::InvalidValues { field: "elevation" });
        }
        if disp.normals.is_some() != disp.distances.is_some() {
            return Err(DisplacementError::UnpairedDeformation);
        }
        let size = (1 << disp.power) + 1;

        let ring = [a, b, c, d];
        let start = (0..4)
            .min_by(|&i, &j| {
                let di = ring[i].distance_squared(disp.start_position);
                let dj = ring[j].distance_squared(disp.start_position);
                di.total_cmp(&dj)
            })
            .expect("four corners");
        let [c0, c1, c2, c3] = [0, 1, 2, 3].map(|k| ring[(start + k) % 4]);

        let step = 1.0 / (size - 1) as f64;
        let mut positions = Vec::with_capacity(size * size);
        for row in 0..size {
            let t = row as f64 * step;
            let left = c0.lerp(c1, t);
            let right = c3.lerp(c2, t);
            for col in 0..size {
                positions.push(left.lerp(right, col as f64 * step));
            }
        }

        let base_positions = positions.clone();
        let elevation = face.plane.normal * disp.elevation;
        let normals = grid(disp.normals.as_deref(), size, "normals")?;
        let distances = grid(disp.distances.as_deref(), size, "distances")?;
        if let (Some(normals), Some(distances)) = (normals, distances) {
            for ((p, n), dist) in positions.iter_mut().zip(normals).zip(distances) {
                if !n.is_finite() || !dist.is_finite() {
                    return Err(DisplacementError::InvalidValues {
                        field: "normals/distances",
                    });
                }
                *p += *n * *dist;
            }
        }
        if let Some(offsets) = grid(disp.offsets.as_deref(), size, "offsets")? {
            for (p, o) in positions.iter_mut().zip(offsets) {
                *p += *o;
            }
        }
        for p in &mut positions {
            *p += elevation;
        }

        let alphas: Vec<f64> = match grid(disp.alphas.as_deref(), size, "alphas")? {
            Some(alphas) => alphas.copied().collect(),
            None => vec![0.0; size * size],
        };
        if positions
            .iter()
            .chain(&base_positions)
            .any(|point| !point.as_vec3().is_finite())
        {
            return Err(DisplacementError::InvalidValues { field: "positions" });
        }
        if alphas.iter().any(|alpha| !(*alpha as f32).is_finite()) {
            return Err(DisplacementError::InvalidValues {
                field: "blend weights",
            });
        }

        Ok(Self {
            size,
            positions,
            base_positions,
            alphas,
        })
    }

    /// Triangles over the grid, wound like the source face. Diagonals
    /// alternate per quad, as in the engine, so slopes tessellate evenly.
    pub fn triangles(&self) -> impl Iterator<Item = [usize; 3]> + '_ {
        let n = self.size;
        (0..n - 1).flat_map(move |row| {
            (0..n - 1).flat_map(move |col| {
                let a = row * n + col;
                let (b, c, d) = (a + 1, a + n, a + n + 1);
                if (row + col) % 2 == 0 {
                    [[a, c, d], [a, d, b]]
                } else {
                    [[a, c, b], [c, d, b]]
                }
            })
        })
    }

    /// Vertex indices on the grid border.
    pub fn border(&self) -> impl Iterator<Item = usize> + '_ {
        let n = self.size;
        (0..n).flat_map(move |i| [i, (n - 1) * n + i, i * n, i * n + n - 1])
    }

    /// The border vertices in order around the grid, without repeating the
    /// first.
    pub fn outline(&self) -> Vec<usize> {
        let n = self.size;
        let top = 0..n;
        let right = (1..n).map(|r| r * n + n - 1);
        let bottom = (0..n - 1).rev().map(|c| (n - 1) * n + c);
        let left = (1..n - 1).rev().map(|r| r * n);
        top.chain(right).chain(bottom).chain(left).collect()
    }
}
