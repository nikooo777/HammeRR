use std::collections::BTreeMap;

use glam::DVec3;
use source_formats::vmf::DispInfo;

use super::field::{BasePlane, ScalarField};
use super::{Exact, FIELD_WORK, Point, Work, invalid, key, labels, orient, same};
use crate::{EditError, displacement::DisplacementMesh};

const NORMAL: usize = 0;
const DISTANCE: usize = 3;
const OFFSET: usize = 4;
const OFFSET_NORMAL: usize = 7;
const COMPONENTS: usize = 10;
const SAMPLE_WORK: usize = 32;
const POSITION_WORK: usize = 9 * FIELD_WORK;

type Values = [f64; COMPONENTS];
type Fields = [Option<ScalarField>; COMPONENTS];

struct Triangle {
    points: [DVec3; 3],
    bounds: [[f64; 2]; 2],
    projected: labels::Projected,
    fields: Fields,
    sign: i128,
}

#[derive(Default)]
pub(super) struct Editing {
    base: Option<BasePlane>,
    axes: [usize; 2],
    active: [bool; COMPONENTS],
    samples: BTreeMap<Point, Values>,
    triangles: Vec<Triangle>,
}

fn native(value: f64) -> Result<f64, EditError> {
    let encoded = value as f32;
    if !value.is_finite() || encoded.is_subnormal() || f64::from(encoded) != value {
        return Err(invalid(
            "Retiled editing fields must remain exact finite native floats",
        ));
    }
    Ok(value)
}

fn vector(rows: &Option<Vec<Vec<DVec3>>>, size: usize) -> Result<(), EditError> {
    if rows
        .as_ref()
        .is_some_and(|rows| rows.len() != size || rows.iter().any(|row| row.len() != size))
    {
        return Err(invalid(
            "Terrain editing vector fields must match their source grid",
        ));
    }
    Ok(())
}

fn position(base: DVec3, values: Values) -> Result<DVec3, EditError> {
    let mut position = [0.0; 3];
    for (axis, output) in position.iter_mut().enumerate() {
        let displaced = native(
            Exact::float(values[NORMAL + axis])?
                .mul(Exact::float(values[DISTANCE])?)?
                .value()?,
        )?;
        let offset = native(
            Exact::float(base[axis])?
                .add(Exact::float(values[OFFSET + axis])?)?
                .value()?,
        )?;
        *output = native(
            Exact::float(offset)?
                .add(Exact::float(displaced)?)?
                .value()?,
        )?;
    }
    Ok(DVec3::from_array(position))
}

fn bounds(points: &[DVec3], axes: [usize; 2]) -> [[f64; 2]; 2] {
    axes.map(|axis| {
        points.iter().fold([points[0][axis]; 2], |bounds, point| {
            [bounds[0].min(point[axis]), bounds[1].max(point[axis])]
        })
    })
}

fn overlapping(first: [[f64; 2]; 2], second: [[f64; 2]; 2]) -> bool {
    first
        .iter()
        .zip(second)
        .all(|(a, b)| a[1] >= b[0] && b[1] >= a[0])
}

fn fields(
    points: [DVec3; 3],
    values: [Values; 3],
    active: [bool; COMPONENTS],
    work: &mut Work,
) -> Result<Fields, EditError> {
    work.consume(8 * FIELD_WORK * active.iter().filter(|&&active| active).count())
        .map_err(invalid)?;
    let mut fields = [None; COMPONENTS];
    for (component, field) in fields.iter_mut().enumerate() {
        if active[component] {
            *field = Some(ScalarField::new(
                [points[0], points[1], points[2], points[0]],
                [
                    values[0][component],
                    values[1][component],
                    values[2][component],
                    values[0][component],
                ],
            )?);
        }
    }
    Ok(fields)
}

impl Editing {
    pub(super) fn new(
        info: &DispInfo,
        mesh: &DisplacementMesh,
        work: &mut Work,
    ) -> Result<Self, EditError> {
        vector(&info.offset_normals, mesh.size)?;
        if info
            .offset_normals
            .as_ref()
            .is_none_or(|rows| rows.iter().flatten().all(|&value| value == DVec3::ZERO))
        {
            return Ok(Self::default());
        }
        if info.elevation != 0.0 {
            return Err(invalid(
                "Nonzero terrain editing metadata requires zero native elevation",
            ));
        }
        for rows in [&info.normals, &info.offsets, &info.offset_normals] {
            vector(rows, mesh.size)?;
        }
        if info.distances.as_ref().is_some_and(|rows| {
            rows.len() != mesh.size || rows.iter().any(|row| row.len() != mesh.size)
        }) {
            return Err(invalid(
                "Terrain editing distance fields must match their source grid",
            ));
        }
        let mut active = [false; COMPONENTS];
        active[NORMAL..DISTANCE].fill(info.normals.is_some());
        active[DISTANCE] = info.distances.is_some();
        active[OFFSET..OFFSET_NORMAL].fill(info.offsets.is_some());
        active[OFFSET_NORMAL..].fill(info.offset_normals.is_some());
        if info
            .offset_normals
            .as_ref()
            .is_some_and(|rows| rows.iter().flatten().any(|&value| value == DVec3::ZERO))
        {
            return Err(invalid(
                "Terrain retiling cannot preserve mixed zero and nonzero native editing normals",
            ));
        }
        let mut result = Self {
            active,
            ..Default::default()
        };
        work.consume((SAMPLE_WORK + POSITION_WORK) * mesh.base_positions.len())
            .map_err(invalid)?;
        let mut values = Vec::with_capacity(mesh.base_positions.len());
        for (index, &point) in mesh.base_positions.iter().enumerate() {
            let row = index / mesh.size;
            let column = index % mesh.size;
            let mut value = [0.0; COMPONENTS];
            for (rows, first) in [
                (&info.normals, NORMAL),
                (&info.offsets, OFFSET),
                (&info.offset_normals, OFFSET_NORMAL),
            ] {
                if let Some(rows) = rows {
                    for (axis, component) in rows[row][column].to_array().into_iter().enumerate() {
                        value[first + axis] = native(component)?;
                    }
                }
            }
            if let Some(rows) = &info.distances {
                value[DISTANCE] = native(rows[row][column])?;
            }
            if !same(position(point, value)?, mesh.positions[index]) {
                return Err(invalid(
                    "Source native editing components do not reconstruct their exact authored positions",
                ));
            }
            if result.samples.insert(key(point), value).is_some() {
                return Err(invalid(
                    "Terrain editing fields require unique original base samples",
                ));
            }
            values.push(value);
        }
        let (base, axes) = labels::projection(mesh, work)?;
        result.axes = axes;
        result.base = Some(base);
        for triangle in mesh.triangles() {
            let points = triangle.map(|index| mesh.base_positions[index]);
            result.triangles.push(Triangle {
                points,
                bounds: bounds(&points, result.axes),
                projected: labels::Projected::new(points, result.axes)?,
                fields: fields(points, triangle.map(|index| values[index]), active, work)?,
                sign: orient(points[0], points[1], points[2], result.axes)?,
            });
        }
        Ok(result)
    }

    pub(super) fn enabled(&self) -> bool {
        self.base.is_some()
    }

    pub(super) fn apply(
        &self,
        info: &mut DispInfo,
        mesh: &DisplacementMesh,
        work: &mut Work,
    ) -> Result<Option<Vec<DVec3>>, EditError> {
        let Some(base) = self.base else {
            return Ok(None);
        };
        let child_bounds = bounds(&mesh.base_positions, self.axes);
        work.consume(
            SAMPLE_WORK * (self.triangles.len() + mesh.base_positions.len())
                + POSITION_WORK * mesh.base_positions.len(),
        )
        .map_err(invalid)?;
        let candidates = self
            .triangles
            .iter()
            .filter(|triangle| overlapping(child_bounds, triangle.bounds))
            .collect::<Vec<_>>();
        let mut values = Vec::with_capacity(mesh.base_positions.len());
        let mut positions = Vec::with_capacity(mesh.base_positions.len());
        for &point in &mesh.base_positions {
            if !base.contains(point)? {
                return Err(invalid(
                    "Terrain editing samples must remain on their original base plane",
                ));
            }
            let value = if let Some(&value) = self.samples.get(&key(point)) {
                value
            } else {
                work.consume(SAMPLE_WORK * candidates.len())
                    .map_err(invalid)?;
                let mut source = None;
                for &triangle in &candidates {
                    let mut contains = true;
                    for edge in 0..3 {
                        let side = orient(
                            triangle.points[edge],
                            triangle.points[(edge + 1) % 3],
                            point,
                            self.axes,
                        )?;
                        contains &= side == 0 || side == triangle.sign;
                    }
                    if contains {
                        source = Some(triangle);
                        break;
                    }
                }
                let source = source.ok_or_else(|| {
                    invalid("Terrain editing sample has no original source triangle")
                })?;
                work.consume(FIELD_WORK * self.active.iter().filter(|&&active| active).count())
                    .map_err(invalid)?;
                let mut value = [0.0; COMPONENTS];
                for (output, field) in value.iter_mut().zip(&source.fields) {
                    if let Some(field) = field {
                        *output = native(field.value(point)?)?;
                    }
                }
                value
            };
            if self.active[OFFSET_NORMAL]
                && value[OFFSET_NORMAL..].iter().all(|&value| value == 0.0)
            {
                return Err(invalid(
                    "Retiled native editing normals must not introduce a whole-patch zero reset",
                ));
            }
            positions.push(position(point, value)?);
            values.push(value);
        }
        work.consume(labels::AABB_WORK * 2 * (mesh.size - 1).pow(2) * candidates.len())
            .map_err(invalid)?;
        for triangle in mesh.triangles() {
            let points = triangle.map(|index| mesh.base_positions[index]);
            let projected = labels::Projected::new(points, self.axes)?;
            let fields = fields(
                points,
                triangle.map(|index| values[index]),
                self.active,
                work,
            )?;
            let mut matched = false;
            for &source in &candidates {
                if !projected.overlaps(&source.projected, self.axes, work)? {
                    continue;
                }
                work.consume(FIELD_WORK * self.active.iter().filter(|&&active| active).count())
                    .map_err(invalid)?;
                for (field, source) in fields.iter().zip(&source.fields) {
                    if let (Some(field), Some(source)) = (field, source)
                        && !field.matches(source)?
                    {
                        return Err(invalid(
                            "Retiled terrain changed an overlapping original editing field",
                        ));
                    }
                }
                matched = true;
            }
            if !matched {
                return Err(invalid(
                    "Retiled editing triangle has no positive-area original source coverage",
                ));
            }
        }
        for (output, first) in [
            (&mut info.normals, NORMAL),
            (&mut info.offsets, OFFSET),
            (&mut info.offset_normals, OFFSET_NORMAL),
        ] {
            *output = self.active[first].then(|| {
                values
                    .chunks(mesh.size)
                    .map(|row| {
                        row.iter()
                            .map(|value| {
                                DVec3::from_array([
                                    value[first],
                                    value[first + 1],
                                    value[first + 2],
                                ])
                            })
                            .collect()
                    })
                    .collect()
            });
        }
        info.distances = self.active[DISTANCE].then(|| {
            values
                .chunks(mesh.size)
                .map(|row| row.iter().map(|value| value[DISTANCE]).collect())
                .collect()
        });
        Ok(Some(positions))
    }
}
