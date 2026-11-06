use glam::DVec3;
use source_formats::vmf::DispInfo;

use super::paint::PaintField;
use super::{FIELD_WORK, PlanarField, Work, invalid, orient, paint, same, triangles};
use crate::clip::TerrainBlendError;
use crate::{EditError, displacement::DisplacementMesh};

const BASE_WORK: usize = 16;
const LABEL_WORK: usize = 64;
const TRIANGLE_WORK: usize = 64;
const AABB_WORK: usize = 8;
const SAT_WORK: usize = 18 * 32;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Label {
    raw: Option<u16>,
    native: u16,
    secondary: Option<bool>,
}

fn power(mesh: &DisplacementMesh) -> Result<u32, EditError> {
    let power = (2..=4)
        .find(|&power| mesh.size == (1 << power) + 1)
        .ok_or_else(|| invalid("Terrain triangle labels require power 2, 3 or 4"))?;
    let vertices = mesh.size * mesh.size;
    if mesh.base_positions.len() != vertices
        || mesh.positions.len() != vertices
        || mesh.alphas.len() != vertices
    {
        return Err(invalid(
            "Terrain triangle labels require complete source and child grids",
        ));
    }
    Ok(power)
}

fn read_labels(
    info: &DispInfo,
    mesh: &DisplacementMesh,
    has_secondary_surface: bool,
    work: &mut Work,
) -> Result<Vec<Label>, EditError> {
    if info.power != power(mesh)? {
        return Err(invalid(
            "Terrain triangle label power differs from its grid",
        ));
    }
    let count = 2 * (mesh.size - 1).pow(2);
    work.consume(LABEL_WORK * count).map_err(invalid)?;
    let raw = triangles::source_tags(info)?;
    let native = triangles::native_tags(info, mesh)?;
    if native.len() != count || raw.as_ref().is_some_and(|raw| raw.len() != count) {
        return Err(invalid(
            "Terrain triangle labels differ from their source order",
        ));
    }
    mesh.triangles()
        .enumerate()
        .map(|(index, triangle)| {
            Ok(Label {
                raw: raw.as_ref().map(|raw| raw[index]),
                native: native[index],
                secondary: has_secondary_surface
                    .then(|| paint::triangle_class(triangle.map(|index| mesh.alphas[index] as f32)))
                    .transpose()?,
            })
        })
        .collect()
}

struct Projected {
    points: [DVec3; 3],
    bounds: [[f64; 2]; 2],
    sign: i128,
}

fn projected_bounds(points: &[DVec3], axes: [usize; 2]) -> [[f64; 2]; 2] {
    axes.map(|axis| {
        let mut bounds = [points[0][axis]; 2];
        for point in &points[1..] {
            bounds[0] = bounds[0].min(point[axis]);
            bounds[1] = bounds[1].max(point[axis]);
        }
        bounds
    })
}

fn bounds_overlap(first: [[f64; 2]; 2], second: [[f64; 2]; 2]) -> bool {
    first
        .iter()
        .zip(second)
        .all(|(a, b)| a[1] > b[0] && b[1] > a[0])
}

impl Projected {
    fn new(points: [DVec3; 3], axes: [usize; 2]) -> Result<Self, EditError> {
        let sign = orient(points[0], points[1], points[2], axes)?;
        if sign == 0 {
            return Err(invalid(
                "Terrain triangle labels require positive-area triangles",
            ));
        }
        let bounds = projected_bounds(&points, axes);
        Ok(Self {
            points,
            bounds,
            sign,
        })
    }

    fn overlaps(&self, other: &Self, axes: [usize; 2], work: &mut Work) -> Result<bool, EditError> {
        if !bounds_overlap(self.bounds, other.bounds) {
            return Ok(false);
        }
        work.consume(SAT_WORK).map_err(invalid)?;
        for (first, second) in [(self, other), (other, self)] {
            for edge in 0..3 {
                let mut interior = false;
                for &point in &second.points {
                    if orient(
                        first.points[edge],
                        first.points[(edge + 1) % 3],
                        point,
                        axes,
                    )? == first.sign
                    {
                        interior = true;
                        break;
                    }
                }
                if !interior {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }
}

struct SourceTriangle {
    projected: Projected,
    label: Label,
    surface: Option<PlanarField>,
    paint: Option<PaintField>,
}

pub(super) struct TriangleField {
    axis: usize,
    coordinate: f64,
    axes: [usize; 2],
    source: Vec<SourceTriangle>,
    has_raw_tags: bool,
    has_secondary_surface: bool,
    certify_surface: bool,
    uniform_alpha: Option<f64>,
    round_blend: bool,
}

impl TriangleField {
    pub(super) fn new(
        info: &DispInfo,
        mesh: &DisplacementMesh,
        has_secondary_surface: bool,
        work: &mut Work,
    ) -> Result<Self, EditError> {
        Self::prepare(info, mesh, has_secondary_surface, false, false, work)
    }

    pub(super) fn surface(
        info: &DispInfo,
        mesh: &DisplacementMesh,
        has_secondary_surface: bool,
        work: &mut Work,
    ) -> Result<Self, EditError> {
        Self::surface_with_rounding(info, mesh, has_secondary_surface, false, work)
    }

    pub(super) fn surface_with_rounding(
        info: &DispInfo,
        mesh: &DisplacementMesh,
        has_secondary_surface: bool,
        round_blend: bool,
        work: &mut Work,
    ) -> Result<Self, EditError> {
        Self::prepare(info, mesh, has_secondary_surface, true, round_blend, work)
    }

    fn prepare(
        info: &DispInfo,
        mesh: &DisplacementMesh,
        has_secondary_surface: bool,
        certify_surface: bool,
        round_blend: bool,
        work: &mut Work,
    ) -> Result<Self, EditError> {
        power(mesh)?;
        work.consume(BASE_WORK * mesh.base_positions.len())
            .map_err(invalid)?;
        if mesh.base_positions.iter().any(|point| !point.is_finite()) {
            return Err(invalid("Terrain triangle label coordinates must be finite"));
        }
        let axis = (0..3)
            .find(|&axis| {
                mesh.base_positions
                    .iter()
                    .all(|point| point[axis] == mesh.base_positions[0][axis])
            })
            .ok_or_else(|| invalid("Terrain triangle labels require a world-axis base plane"))?;
        let coordinate = mesh.base_positions[0][axis];
        let axes = [(axis + 1) % 3, (axis + 2) % 3];
        let uniform_alpha = if certify_surface {
            work.consume(BASE_WORK * mesh.alphas.len())
                .map_err(invalid)?;
            let first = mesh.alphas[0];
            if mesh.alphas.iter().all(|&alpha| alpha == first) {
                if !(0.0..=255.0).contains(&first) || first.fract() != 0.0 {
                    return Err(invalid(
                        "Terrain paint requires integral alpha values from 0 to 255",
                    ));
                }
                Some(first)
            } else {
                None
            }
        } else {
            None
        };
        let labels = read_labels(info, mesh, has_secondary_surface, work)?;
        work.consume(
            (TRIANGLE_WORK + usize::from(certify_surface) * 8 * FIELD_WORK) * labels.len(),
        )
        .map_err(invalid)?;
        let source = mesh
            .triangles()
            .zip(labels)
            .map(|(triangle, label)| {
                Ok(SourceTriangle {
                    projected: Projected::new(
                        triangle.map(|index| mesh.base_positions[index]),
                        axes,
                    )?,
                    label,
                    surface: certify_surface
                        .then(|| {
                            PlanarField::triangle(
                                triangle.map(|index| mesh.base_positions[index]),
                                triangle.map(|index| mesh.positions[index]),
                            )
                        })
                        .transpose()?,
                    paint: (certify_surface && uniform_alpha.is_none())
                        .then(|| {
                            PaintField::triangle(
                                triangle.map(|index| mesh.base_positions[index]),
                                triangle.map(|index| mesh.alphas[index]),
                                work,
                            )
                        })
                        .transpose()?,
                })
            })
            .collect::<Result<_, EditError>>()?;
        Ok(Self {
            axis,
            coordinate,
            axes,
            source,
            has_raw_tags: info.triangle_tags.is_some(),
            has_secondary_surface,
            certify_surface,
            uniform_alpha,
            round_blend,
        })
    }

    pub(super) fn map(
        &self,
        child: &DisplacementMesh,
        work: &mut Work,
    ) -> Result<Mapped, EditError> {
        let power = power(child)?;
        let count = 2 * (child.size - 1).pow(2);
        work.consume(
            BASE_WORK * child.base_positions.len()
                + (TRIANGLE_WORK + usize::from(self.certify_surface) * 8 * FIELD_WORK) * count
                + AABB_WORK * self.source.len(),
        )
        .map_err(invalid)?;
        if child
            .base_positions
            .iter()
            .any(|point| !point.is_finite() || point[self.axis] != self.coordinate)
        {
            return Err(invalid(
                "Terrain triangle labels must remain on their original base plane",
            ));
        }
        if let Some(alpha) = self.uniform_alpha {
            work.consume(BASE_WORK * child.alphas.len())
                .map_err(invalid)?;
            if child.alphas.iter().any(|&value| value != alpha) {
                return Err(invalid(
                    "Terrain output changed its constant source paint field",
                ));
            }
        }
        let bounds = projected_bounds(&child.base_positions, self.axes);
        let candidates = self
            .source
            .iter()
            .filter(|source| bounds_overlap(bounds, source.projected.bounds))
            .collect::<Vec<_>>();
        work.consume(AABB_WORK * count * candidates.len())
            .map_err(invalid)?;
        let mut labels = Vec::with_capacity(count);
        let mut blend_error = TerrainBlendError::default();
        // The caller proves complete child coverage inside the retained source region.
        for triangle in child.triangles() {
            let projected =
                Projected::new(triangle.map(|index| child.base_positions[index]), self.axes)?;
            let surface = self
                .certify_surface
                .then(|| {
                    PlanarField::triangle(
                        triangle.map(|index| child.base_positions[index]),
                        triangle.map(|index| child.positions[index]),
                    )
                })
                .transpose()?;
            let paint = (self.certify_surface && self.uniform_alpha.is_none() && !self.round_blend)
                .then(|| {
                    PaintField::triangle(
                        triangle.map(|index| child.base_positions[index]),
                        triangle.map(|index| child.alphas[index]),
                        work,
                    )
                })
                .transpose()?;
            let mut label = None;
            for source in &candidates {
                if !projected.overlaps(&source.projected, self.axes, work)? {
                    continue;
                }
                if let Some(source_surface) = &source.surface {
                    work.consume(FIELD_WORK).map_err(invalid)?;
                    let surface = surface.as_ref().ok_or_else(|| {
                        invalid("Terrain triangle mapping lost its surface field")
                    })?;
                    if !source_surface.matches(surface)? {
                        return Err(invalid(
                            "Terrain output triangle crosses an original surface crease",
                        ));
                    }
                }
                if let Some(source_paint) = &source.paint {
                    if self.round_blend {
                        blend_error.merge(source_paint.rounding_error(
                            projected.points,
                            triangle.map(|index| child.alphas[index]),
                            work,
                        )?)?;
                    } else {
                        let paint = paint.as_ref().ok_or_else(|| {
                            invalid("Terrain triangle mapping lost its paint field")
                        })?;
                        if !source_paint.matches(paint, work)? {
                            return Err(invalid(
                                "Terrain output triangle changed an original raw or native packed paint field",
                            ));
                        }
                    }
                }
                if label.is_some_and(|label| label != source.label) {
                    return Err(invalid(
                        "Flat terrain output triangle crosses different source flags or collision-material classes",
                    ));
                }
                label = Some(source.label);
            }
            labels.push(label.ok_or_else(|| {
                invalid("Flat terrain output triangle has no positive-area source label")
            })?);
        }
        let tags = self
            .has_raw_tags
            .then(|| {
                labels
                    .chunks(2 * (child.size - 1))
                    .map(|row| {
                        row.iter()
                            .map(|label| {
                                label.raw.map(i32::from).ok_or_else(|| {
                                    invalid("Terrain source label lost its explicit VMF tags")
                                })
                            })
                            .collect()
                    })
                    .collect::<Result<_, _>>()
            })
            .transpose()?;
        Ok(Mapped {
            tags,
            blend_error,
            power,
            base: child.base_positions.clone(),
            positions: self.certify_surface.then(|| child.positions.clone()),
            alphas: self.certify_surface.then(|| child.alphas.clone()),
            labels,
            has_secondary_surface: self.has_secondary_surface,
        })
    }
}

pub(super) struct Mapped {
    pub(super) tags: Option<Vec<Vec<i32>>>,
    pub(super) blend_error: TerrainBlendError,
    power: u32,
    base: Vec<DVec3>,
    positions: Option<Vec<DVec3>>,
    alphas: Option<Vec<f64>>,
    labels: Vec<Label>,
    has_secondary_surface: bool,
}

impl Mapped {
    pub(super) fn verify(
        &self,
        info: &DispInfo,
        child: &DisplacementMesh,
        work: &mut Work,
    ) -> Result<(), EditError> {
        if power(child)? != self.power {
            return Err(invalid("Terrain mapped triangle label power changed"));
        }
        work.consume(BASE_WORK * self.base.len()).map_err(invalid)?;
        if child.base_positions.len() != self.base.len()
            || child
                .base_positions
                .iter()
                .zip(&self.base)
                .any(|(&actual, &expected)| !same(actual, expected))
        {
            return Err(invalid("Terrain mapped triangle label geometry changed"));
        }
        if let Some(positions) = &self.positions {
            work.consume(BASE_WORK * positions.len()).map_err(invalid)?;
            if child.positions.len() != positions.len()
                || child
                    .positions
                    .iter()
                    .zip(positions)
                    .any(|(&actual, &expected)| !same(actual, expected))
            {
                return Err(invalid("Terrain mapped triangle surface changed"));
            }
        }
        if let Some(alphas) = &self.alphas {
            work.consume(BASE_WORK * alphas.len()).map_err(invalid)?;
            if child.alphas.len() != alphas.len()
                || child
                    .alphas
                    .iter()
                    .zip(alphas)
                    .any(|(actual, expected)| actual.to_bits() != expected.to_bits())
            {
                return Err(invalid("Terrain mapped triangle paint changed"));
            }
        }
        let actual = read_labels(info, child, self.has_secondary_surface, work)?;
        if actual != self.labels {
            return Err(invalid(
                "Flat terrain child changed its mapped VMF tags, native flags or collision-material class",
            ));
        }
        Ok(())
    }
}
