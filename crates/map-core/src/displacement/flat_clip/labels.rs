use glam::DVec3;
use source_formats::vmf::DispInfo;

use super::{Work, invalid, orient, paint, same, triangles};
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

impl Projected {
    fn new(points: [DVec3; 3], axes: [usize; 2]) -> Result<Self, EditError> {
        let sign = orient(points[0], points[1], points[2], axes)?;
        if sign == 0 {
            return Err(invalid(
                "Terrain triangle labels require positive-area triangles",
            ));
        }
        let bounds = axes.map(|axis| {
            let mut bounds = [points[0][axis]; 2];
            for point in &points[1..] {
                bounds[0] = bounds[0].min(point[axis]);
                bounds[1] = bounds[1].max(point[axis]);
            }
            bounds
        });
        Ok(Self {
            points,
            bounds,
            sign,
        })
    }

    fn overlaps(&self, other: &Self, axes: [usize; 2], work: &mut Work) -> Result<bool, EditError> {
        if self
            .bounds
            .iter()
            .zip(other.bounds)
            .any(|(a, b)| a[1] <= b[0] || b[1] <= a[0])
        {
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
}

pub(super) struct TriangleField {
    axis: usize,
    coordinate: f64,
    axes: [usize; 2],
    source: Vec<SourceTriangle>,
    has_raw_tags: bool,
    has_secondary_surface: bool,
}

impl TriangleField {
    pub(super) fn new(
        info: &DispInfo,
        mesh: &DisplacementMesh,
        has_secondary_surface: bool,
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
        let labels = read_labels(info, mesh, has_secondary_surface, work)?;
        work.consume(TRIANGLE_WORK * labels.len())
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
                + TRIANGLE_WORK * count
                + AABB_WORK * count * self.source.len(),
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
        let mut labels = Vec::with_capacity(count);
        // The caller proves complete child coverage inside the retained source region.
        for triangle in child.triangles() {
            let projected =
                Projected::new(triangle.map(|index| child.base_positions[index]), self.axes)?;
            let mut label = None;
            for source in &self.source {
                if !projected.overlaps(&source.projected, self.axes, work)? {
                    continue;
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
            power,
            base: child.base_positions.clone(),
            labels,
            has_secondary_surface: self.has_secondary_surface,
        })
    }
}

pub(super) struct Mapped {
    pub(super) tags: Option<Vec<Vec<i32>>>,
    power: u32,
    base: Vec<DVec3>,
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
        let actual = read_labels(info, child, self.has_secondary_surface, work)?;
        if actual != self.labels {
            return Err(invalid(
                "Flat terrain child changed its mapped VMF tags, native flags or collision-material class",
            ));
        }
        Ok(())
    }
}
