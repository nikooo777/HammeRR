use glam::DVec3;
use source_formats::vmf::Solid;

use super::{
    Admitted, BaseChart, CUT_EPSILON, CarrierGrid, Child, Context, Exact, FIELD_WORK, Keep,
    PartitionRequest, Request, SIZE, TerrainPlan, TriangleField, VERTICES, WindowFields, Work,
    admit, admit_source, carrier_grid, certify_local_normals, connections, data, intrinsic_source,
    invalid, native, network, paint, partition_counts, reconstructed, region_base, same, stored,
    uv, weighted, window, window::Window, window_connections,
};
use crate::clip::{TerrainBlendMode, TerrainMode};
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

    fn boundary(&self, window: Window, size: usize, sample: usize) -> bool {
        let coordinate = if self.rows {
            window.row * (size - 1) + window.row_cells * (sample / size)
        } else {
            window.column * (size - 1) + window.column_cells * (sample % size)
        };
        coordinate == self.index * (size - 1)
    }
}

struct Prepared<'a> {
    source: Admitted<'a>,
    sheet: BaseChart,
    cut: Cut,
}

struct Retained<'a> {
    cut: &'a Cut,
    plane: Plane,
    keep: Keep,
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
    let sheet = cell_sheet(&source, work)?;
    let cut = cut(&source, plane, work)?;
    Ok(Some(Prepared { source, sheet, cut }))
}

fn cell_sheet(source: &Admitted<'_>, work: &mut Work) -> Result<BaseChart, EditError> {
    let sheet = intrinsic_source(source, work)?;
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
                "Cell terrain requires a rectangular base cage in its exact plane chart",
            ));
        }
    }
    Ok(sheet)
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

impl Sample {
    pub(super) fn offset(&self, base: DVec3) -> Result<DVec3, EditError> {
        if !same(self.base, base) {
            return Err(invalid(
                "Terrain cell samples differ from their exact child base grid",
            ));
        }
        let mut offset = DVec3::ZERO;
        for axis in 0..3 {
            offset[axis] = Exact::float(self.point[axis])?
                .sub(Exact::float(base[axis])?)?
                .value()?;
        }
        Ok(offset)
    }
}

fn sample_grid(source: &Admitted<'_>, window: Window, index: usize) -> Result<Sample, EditError> {
    if !(2..=4).contains(&window.output_power) || window.row_cells == 0 || window.column_cells == 0
    {
        return Err(invalid(
            "Terrain samples require a bounded native grid window",
        ));
    }
    let cells = 1usize << window.output_power;
    let size = cells + 1;
    if index >= size * size
        || window.row + window.row_cells >= source.mesh.size
        || window.column + window.column_cells >= source.mesh.size
    {
        return Err(invalid(
            "Terrain samples lie outside their source grid window",
        ));
    }
    sample_at(
        source,
        window.row as f64 + (window.row_cells * (index / size)) as f64 / cells as f64,
        window.column as f64 + (window.column_cells * (index % size)) as f64 / cells as f64,
        cells,
    )
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
    let denominator = (SIZE - 1) * divisions;
    sample_at(
        source,
        row as f64 + (child_row * (SIZE - 1) + index / SIZE) as f64 / denominator as f64,
        column as f64 + (child_column * (SIZE - 1) + index % SIZE) as f64 / denominator as f64,
        denominator,
    )
}

fn sample_at(
    source: &Admitted<'_>,
    row: f64,
    column: f64,
    denominator: usize,
) -> Result<Sample, EditError> {
    let (indices, weights) = resample::weights(source.mesh.size, row, column);
    let mut exact = [0; 3];
    for (output, weight) in exact.iter_mut().zip(weights) {
        let value = weight * denominator as f64;
        if !(0.0..=denominator as f64).contains(&value) || value.fract() != 0.0 {
            return Err(invalid(
                "Cell terrain sampling requires exact dyadic triangle weights",
            ));
        }
        *output = value as i128;
    }
    let denominator = denominator as i128;
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
    let Some(prepared) = prepare(document, solid, geometry, request.plane, &mut context.work)?
    else {
        return Ok(None);
    };
    let windows = window::cut_children(
        prepared.source.info.power,
        prepared.cut.rows,
        prepared.cut.index,
        prepared.cut.first_back,
        request.keep,
    );
    let windows = if windows.is_some()
        && native::terrain_source_is_visible(document.vmf(), solid.id)
        && network::native_window_movement(
            document,
            FaceId {
                solid: solid.id,
                side: prepared.source.side.id,
            },
            Some((request.plane, request.keep)),
            context,
        )? {
        windows
    } else {
        None
    };
    materialize(
        solid,
        &prepared.source,
        &prepared.sheet,
        Some(Retained {
            cut: &prepared.cut,
            plane: request.plane,
            keep: request.keep,
        }),
        windows,
        PartitionRequest {
            material: request.material,
            textures: request.textures,
            blend_mode: request.blend_mode,
        },
        &mut context.work,
    )
    .map(Some)
}

pub(crate) fn partition(
    document: &Document,
    solid: &Solid,
    geometry: &BrushGeometry,
    request: PartitionRequest<'_>,
    context: &mut Context<'_>,
) -> Result<TerrainPlan, EditError> {
    if request.blend_mode != TerrainBlendMode::Exact {
        return Err(invalid(
            "Whole-cell terrain retiling requires exact blend fields",
        ));
    }
    let source = admit_source(document, solid, geometry, &mut context.work)?;
    let windows = window::whole(source.info.power);
    let windows = if windows.is_some()
        && native::terrain_source_is_visible(document.vmf(), solid.id)
        && network::native_window_movement(
            document,
            FaceId {
                solid: solid.id,
                side: source.side.id,
            },
            None,
            context,
        )? {
        windows
    } else {
        None
    };
    if windows.is_none() {
        partition_counts(source.info.power, TerrainMode::Cells)?;
    }
    let sheet = cell_sheet(&source, &mut context.work)?;
    materialize(
        solid,
        &source,
        &sheet,
        None,
        windows,
        request,
        &mut context.work,
    )
}

fn materialize(
    solid: &Solid,
    source: &Admitted<'_>,
    sheet: &BaseChart,
    clipping: Option<Retained<'_>>,
    windows: Option<Vec<Window>>,
    request: PartitionRequest<'_>,
    work: &mut Work,
) -> Result<TerrainPlan, EditError> {
    let cells = source.mesh.size - 1;
    work.consume(cells * cells).map_err(invalid)?;
    let native_windows = windows.is_some();
    let layout = windows
        .unwrap_or_else(|| {
            (0..cells)
                .flat_map(|row| {
                    (0..cells).map(move |column| Window {
                        row,
                        column,
                        row_cells: 1,
                        column_cells: 1,
                        output_power: super::POWER,
                    })
                })
                .collect()
        })
        .into_iter()
        .filter(|window| {
            clipping
                .as_ref()
                .is_none_or(|cut| retained(cut.keep, cut.cut.back(window.row, window.column)))
        })
        .collect::<Vec<_>>();
    let count = layout.len();
    if count > crate::clip::MAX_TERRAIN_PATCHES {
        return Err(invalid(format!(
            "Cell terrain cut exceeds {} output patches",
            crate::clip::MAX_TERRAIN_PATCHES
        )));
    }
    let vertices = layout
        .iter()
        .map(|window| ((1usize << window.output_power) + 1).pow(2))
        .sum::<usize>();
    let triangles = layout
        .iter()
        .map(|window| 2 * (1usize << window.output_power).pow(2))
        .sum::<usize>();
    if vertices > crate::clip::MAX_TERRAIN_VERTICES
        || triangles > crate::clip::MAX_TERRAIN_TRIANGLES
    {
        return Err(invalid(
            "Cell terrain cut exceeds its vertex or triangle limit",
        ));
    }
    let output_power = layout
        .first()
        .map(|window| window.output_power)
        .filter(|power| layout.iter().all(|window| window.output_power == *power));
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
    let mut window_fields = native_windows.then(|| WindowFields {
        source: identity,
        layout: layout.clone(),
        mapped: Vec::with_capacity(count),
    });
    for &window in &layout {
        let row = window.row;
        let column = window.column;
        let output_power = window.output_power;
        let size = (1usize << output_power) + 1;
        let vertices = size * size;
        let back = clipping
            .as_ref()
            .is_some_and(|cut| cut.cut.back(row, column));
        let indices = [
            row * source.mesh.size + column,
            (row + window.row_cells) * source.mesh.size + column,
            (row + window.row_cells) * source.mesh.size + column + window.column_cells,
            row * source.mesh.size + column + window.column_cells,
        ];
        let mut base = [DVec3::ZERO; 4];
        for (point, index) in base.iter_mut().zip(indices) {
            *point = stored(source.mesh.base_positions[index])?;
        }
        let (mut part, base_mesh) = carrier_grid(
            solid,
            source.side,
            source.face,
            CarrierGrid {
                corners: base,
                power: output_power,
            },
            request.material,
            &source.editing,
            work,
        )?;
        let side_index = part
            .sides
            .iter()
            .position(|side| side.id == source.side.id)
            .unwrap();
        work.consume(4 * FIELD_WORK * vertices).map_err(invalid)?;
        let samples = (0..vertices)
            .map(|index| sample_grid(source, window, index))
            .collect::<Result<Vec<_>, _>>()?;
        let mut offsets = Vec::with_capacity(vertices);
        for (sample, &point) in samples.iter().zip(&base_mesh.base_positions) {
            offsets.push(sample.offset(point)?);
        }
        let info = part.sides[side_index].displacement.as_mut().unwrap();
        if !source.editing.enabled() {
            info.offsets = Some(offsets.chunks(size).map(<[_]>::to_vec).collect());
        }
        info.alphas = source.info.alphas.as_ref().map(|_| {
            samples
                .chunks(size)
                .map(|row| row.iter().map(|sample| sample.alpha).collect())
                .collect()
        });
        let shape = BrushGeometry::from_solid(&part).map_err(|error| invalid(error.to_string()))?;
        let face = shape
            .faces
            .iter()
            .find(|face| face.side == side_index)
            .ok_or_else(|| invalid("Cell terrain carrier lost its source face"))?;
        let info = part.sides[side_index].displacement.as_ref().unwrap();
        let (_, mesh) = native::checked_mesh(face, info).map_err(invalid)?;
        let (data, stored_base) = data(face, &mesh, output_power)?;
        let profiles = reconstructed(&data, stored_base, work)?;
        let plane = clipping.as_ref().map(|cut| {
            if back {
                cut.plane
            } else {
                Plane {
                    normal: -cut.plane.normal,
                    dist: -cut.plane.dist,
                }
            }
        });
        work.consume((profiles.len() + 1) * vertices)
            .map_err(invalid)?;
        for positions in profiles.iter().chain(std::iter::once(&mesh.positions)) {
            for (index, (&point, sample)) in positions.iter().zip(&samples).enumerate() {
                if !same(point, sample.point)
                    || plane.is_some_and(|plane| plane.distance_to(point) > CUT_EPSILON)
                    || clipping
                        .as_ref()
                        .is_some_and(|cut| cut.cut.boundary(window, size, index))
                        && plane.is_some_and(|plane| plane.distance_to(point).abs() > CUT_EPSILON)
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
        if let Some(fields) = &mut window_fields {
            fields.mapped.push(mapped);
        } else {
            mapped.verify(info, &mesh, work)?;
        }
        certify_local_normals(&data, &profiles, work)?;
        uv(&data, face, mesh.positions.len(), texture, work)?;
        let child = Child {
            base,
            base_positions: mesh.base_positions,
            positions: mesh.positions,
        };
        let ring = indices.map(|index| source.mesh.base_positions[index]);
        region_base(
            &ring,
            std::slice::from_ref(&child),
            sheet.axes,
            sheet.sign,
            work,
        )?;
        children.push(child);
        parts.push(part);
    }
    if native_windows {
        window_connections(&children, work)?;
    } else {
        connections(&children, work)?;
    }
    Ok(TerrainPlan {
        vertices,
        triangles,
        parts,
        source: identity,
        source_power: source.info.power,
        output_power,
        children: None,
        window_fields,
        blend_error: None,
    })
}
