use glam::DVec3;
use source_formats::vmf::Solid;

use super::{
    Admitted, CUT_EPSILON, Child, Context, Exact, FIELD_WORK, Keep, Request, SIZE, TRIANGLES,
    TerrainPlan, TriangleField, VERTICES, Work, admit, carrier_base, certify_height_normals,
    connections, data, height_sheet, invalid, native, paint, reconstructed, region, same, stored,
    uv, weighted,
};
use crate::displacement::{grid_clip::grid_line, resample};
use crate::surface::FaceId;
use crate::{BrushGeometry, Document, EditError, Plane};

struct Cut {
    rows: bool,
    index: usize,
    first_back: bool,
}

impl Cut {
    fn back(&self, row: usize, column: usize) -> bool {
        let coordinate = if self.rows { row } else { column };
        (coordinate < self.index) == self.first_back
    }

    fn boundary(&self, row: usize, column: usize, sample: usize) -> bool {
        let coordinate = if self.rows {
            row * (SIZE - 1) + sample / SIZE
        } else {
            column * (SIZE - 1) + sample % SIZE
        };
        coordinate == self.index * (SIZE - 1)
    }
}

struct Prepared<'a> {
    source: Admitted<'a>,
    axis: usize,
    axes: [usize; 2],
    sign: i128,
    cut: Cut,
}

fn cut(source: &Admitted<'_>, plane: Plane, work: &mut Work) -> Result<Cut, EditError> {
    for index in 1..source.mesh.size - 1 {
        for rows in [true, false] {
            work.consume((source.positions.len() + 1) * source.mesh.positions.len())
                .map_err(invalid)?;
            let Some(first_back) =
                grid_line(&source.mesh.positions, source.mesh.size, plane, rows, index)
            else {
                continue;
            };
            if source.positions.iter().all(|positions| {
                grid_line(positions, source.mesh.size, plane, rows, index) == Some(first_back)
            }) {
                return Ok(Cut {
                    rows,
                    index,
                    first_back,
                });
            }
        }
    }
    Err(invalid(
        "Cell terrain cuts require a complete internal original row or column on the requested plane",
    ))
}

fn prepare<'a>(
    document: &Document,
    solid: &'a Solid,
    geometry: &'a BrushGeometry,
    plane: Plane,
    work: &mut Work,
) -> Result<Option<Prepared<'a>>, EditError> {
    let Some(source) = admit(document, solid, geometry, plane, work)? else {
        return Ok(None);
    };
    let sheet = height_sheet(&source, work)?;
    let mesh = &source.mesh;
    let cells = mesh.size - 1;
    let indices = [0, cells * mesh.size, mesh.size * mesh.size - 1, cells];
    let corners = indices.map(|index| mesh.base_positions[index]);
    for index in 0..4 {
        let next = corners[(index + 1) % 4];
        if sheet
            .axes
            .iter()
            .filter(|&&axis| corners[index][axis] != next[axis])
            .count()
            != 1
        {
            return Err(invalid(
                "Cell terrain requires a world-axis rectangular base cage",
            ));
        }
    }
    let cut = cut(&source, plane, work)?;
    Ok(Some(Prepared {
        source,
        axis: sheet.axis,
        axes: sheet.axes,
        sign: sheet.sign,
        cut,
    }))
}

pub(crate) fn texture_face(
    document: &Document,
    solid: &Solid,
    geometry: &BrushGeometry,
    plane: Plane,
    work: &mut Work,
) -> Result<Option<FaceId>, EditError> {
    Ok(
        prepare(document, solid, geometry, plane, work)?.map(|prepared| FaceId {
            solid: solid.id,
            side: prepared.source.side.id,
        }),
    )
}

fn retained(keep: Keep, back: bool) -> bool {
    !matches!((keep, back), (Keep::Back, false) | (Keep::Front, true))
}

pub(super) struct Sample {
    pub(super) base: DVec3,
    pub(super) point: DVec3,
    pub(super) alpha: f64,
}

fn sample(
    source: &Admitted<'_>,
    row: usize,
    column: usize,
    index: usize,
) -> Result<Sample, EditError> {
    sample_window(source, row, column, index, 0, 0, 1)
}

pub(super) fn sample_window(
    source: &Admitted<'_>,
    row: usize,
    column: usize,
    index: usize,
    child_row: usize,
    child_column: usize,
    divisions: usize,
) -> Result<Sample, EditError> {
    if !matches!(divisions, 1 | 2)
        || child_row >= divisions
        || child_column >= divisions
        || index >= VERTICES
        || row >= source.mesh.size - 1
        || column >= source.mesh.size - 1
    {
        return Err(invalid(
            "Terrain sampling requires a valid whole-cell or quarter-cell window",
        ));
    }
    let denominator = ((SIZE - 1) * divisions) as f64;
    let (indices, weights) = resample::weights(
        source.mesh.size,
        row as f64 + (child_row * (SIZE - 1) + index / SIZE) as f64 / denominator,
        column as f64 + (child_column * (SIZE - 1) + index % SIZE) as f64 / denominator,
    );
    let mut exact = [0; 3];
    for (output, weight) in exact.iter_mut().zip(weights) {
        let value = weight * denominator;
        if !(0.0..=denominator).contains(&value) || value.fract() != 0.0 {
            return Err(invalid(
                "Cell terrain sampling requires exact dyadic triangle weights",
            ));
        }
        *output = value as i128;
    }
    let denominator = ((SIZE - 1) * divisions) as i128;
    Ok(Sample {
        base: weighted(
            &indices.map(|index| source.mesh.base_positions[index]),
            &exact,
            denominator,
        )?,
        point: weighted(
            &indices.map(|index| source.mesh.positions[index]),
            &exact,
            denominator,
        )?,
        alpha: weighted(
            &indices.map(|index| DVec3::splat(source.mesh.alphas[index])),
            &exact,
            denominator,
        )?
        .x,
    })
}

pub(crate) fn plan(
    document: &Document,
    solid: &Solid,
    geometry: &BrushGeometry,
    request: Request<'_>,
    context: &mut Context<'_>,
) -> Result<Option<TerrainPlan>, EditError> {
    let work = &mut context.work;
    let Some(prepared) = prepare(document, solid, geometry, request.plane, work)? else {
        return Ok(None);
    };
    let source = &prepared.source;
    let cells = source.mesh.size - 1;
    work.consume(cells * cells).map_err(invalid)?;
    let count = (0..cells)
        .flat_map(|row| (0..cells).map(move |column| (row, column)))
        .filter(|&(row, column)| retained(request.keep, prepared.cut.back(row, column)))
        .count();
    if count > crate::clip::MAX_TERRAIN_PATCHES {
        return Err(invalid(format!(
            "Cell terrain cut exceeds {} output patches",
            crate::clip::MAX_TERRAIN_PATCHES
        )));
    }
    let identity = FaceId {
        solid: solid.id,
        side: source.side.id,
    };
    let texture = request.textures.and_then(|textures| textures.get(&identity)).copied()
        .ok_or_else(|| invalid("Cell terrain clipping requires guarded native texture projection and mapping dimensions"))?;
    let labels = TriangleField::surface(
        source.info,
        &source.mesh,
        texture.has_secondary_surface,
        work,
    )?;
    uv(
        &source.data,
        source.face,
        source.mesh.positions.len(),
        texture,
        work,
    )?;
    let mut parts = Vec::with_capacity(count);
    let mut children = Vec::with_capacity(count);
    for row in 0..cells {
        for column in 0..cells {
            let back = prepared.cut.back(row, column);
            if !retained(request.keep, back) {
                continue;
            }
            let indices = [
                row * source.mesh.size + column,
                (row + 1) * source.mesh.size + column,
                (row + 1) * source.mesh.size + column + 1,
                row * source.mesh.size + column + 1,
            ];
            let mut base = [DVec3::ZERO; 4];
            for (point, index) in base.iter_mut().zip(indices) {
                *point = stored(source.mesh.base_positions[index])?;
            }
            let (mut part, base_mesh) =
                carrier_base(solid, source.side, source.face, base, request.material)?;
            let side_index = part
                .sides
                .iter()
                .position(|side| side.id == source.side.id)
                .unwrap();
            work.consume(4 * FIELD_WORK * VERTICES).map_err(invalid)?;
            let samples = (0..VERTICES)
                .map(|index| sample(source, row, column, index))
                .collect::<Result<Vec<_>, _>>()?;
            let mut offsets = Vec::with_capacity(VERTICES);
            for (sample, &point) in samples.iter().zip(&base_mesh.base_positions) {
                if !same(sample.base, point)
                    || prepared
                        .axes
                        .iter()
                        .any(|&axis| sample.point[axis] != point[axis])
                {
                    return Err(invalid(
                        "Cell terrain child orientation differs from its exact original cell",
                    ));
                }
                let mut offset = DVec3::ZERO;
                offset[prepared.axis] = Exact::float(sample.point[prepared.axis])?
                    .sub(Exact::float(point[prepared.axis])?)?
                    .value()?;
                offsets.push(offset);
            }
            let info = part.sides[side_index].displacement.as_mut().unwrap();
            info.offsets = Some(offsets.chunks(SIZE).map(<[_]>::to_vec).collect());
            info.alphas = source.info.alphas.as_ref().map(|_| {
                samples
                    .chunks(SIZE)
                    .map(|row| row.iter().map(|sample| sample.alpha).collect())
                    .collect()
            });
            let shape =
                BrushGeometry::from_solid(&part).map_err(|error| invalid(error.to_string()))?;
            let face = shape
                .faces
                .iter()
                .find(|face| face.side == side_index)
                .ok_or_else(|| invalid("Cell terrain carrier lost its source face"))?;
            let info = part.sides[side_index].displacement.as_ref().unwrap();
            let (_, mesh) = native::checked_mesh(face, info).map_err(invalid)?;
            let (data, stored_base) = data(face, &mesh, super::POWER)?;
            let profiles = reconstructed(&data, stored_base, work)?;
            let plane = if back {
                request.plane
            } else {
                Plane {
                    normal: -request.plane.normal,
                    dist: -request.plane.dist,
                }
            };
            work.consume((profiles.len() + 1) * VERTICES)
                .map_err(invalid)?;
            for positions in profiles.iter().chain(std::iter::once(&mesh.positions)) {
                for (index, (&point, sample)) in positions.iter().zip(&samples).enumerate() {
                    if !same(point, sample.point)
                        || plane.distance_to(point) > CUT_EPSILON
                        || prepared.cut.boundary(row, column, index)
                            && plane.distance_to(point).abs() > CUT_EPSILON
                    {
                        return Err(invalid(
                            "Cell terrain must preserve exact source samples and native retained-half separation",
                        ));
                    }
                }
            }
            paint::validate_data(&mesh, &data, work)?;
            let mapped = labels.map(&mesh, work)?;
            let info = part.sides[side_index].displacement.as_mut().unwrap();
            info.triangle_tags = mapped.tags.clone();
            mapped.verify(info, &mesh, work)?;
            certify_height_normals(&data, &profiles, prepared.axis, work)?;
            uv(&data, face, mesh.positions.len(), texture, work)?;
            let child = Child {
                base,
                positions: mesh.positions,
            };
            region(
                &indices.map(|index| source.mesh.positions[index]),
                std::slice::from_ref(&child),
                prepared.axes,
                prepared.sign,
                plane,
                None,
                work,
            )?;
            children.push(child);
            parts.push(part);
        }
    }
    connections(&children, work)?;
    Ok(Some(TerrainPlan {
        vertices: parts.len() * VERTICES,
        triangles: parts.len() * TRIANGLES,
        parts,
        source: identity,
        source_power: source.info.power,
        output_power: Some(super::POWER),
        children: None,
        blend_error: None,
    }))
}
