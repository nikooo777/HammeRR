use glam::DVec3;
use source_formats::vmf::Solid;

use super::cells::{Sample, sample_window};
use super::{
    Admitted, BaseChart, CUT_EPSILON, Child, Context, FIELD_WORK, Keep, PaintField,
    PartitionRequest, PlanarField, Request, SIZE, TRIANGLES, TerrainPlan, TerrainTexture,
    TriangleField, VERTICES, Work, admit, admit_source, carrier_base, carrier_field,
    certify_local_normals, connections, data, intrinsic_source, invalid, native, orient, paint,
    partition_counts, quads, reconstructed, region, region_base, same, split_ring, stored, uv,
};
use crate::clip::{TerrainBlendError, TerrainBlendMode, TerrainMode};
use crate::surface::FaceId;
use crate::{BrushGeometry, Document, EditError, Plane};

struct Prepared<'a> {
    source: Admitted<'a>,
    sheet: BaseChart,
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
    let sheet = intrinsic_source(&source, work)?;
    Ok(Some(Prepared { source, sheet }))
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

fn sides(points: &[DVec3], plane: Plane) -> [bool; 2] {
    let mut result = [false; 2];
    for &point in points {
        let distance = plane.distance_to(point);
        result[0] |= distance < -CUT_EPSILON;
        result[1] |= distance > CUT_EPSILON;
    }
    result
}

fn half_plane(plane: Plane, back: bool) -> Plane {
    if back {
        plane
    } else {
        Plane {
            normal: -plane.normal,
            dist: -plane.dist,
        }
    }
}

enum Group {
    Cell {
        row: usize,
        column: usize,
        plane: Option<Plane>,
        ring: [DVec3; 4],
    },
    Triangle {
        field: Box<PlanarField>,
        paint: Box<PaintField>,
        rings: [Vec<DVec3>; 2],
    },
}

fn add_count(count: &mut usize, amount: usize) -> Result<(), EditError> {
    *count = count
        .checked_add(amount)
        .ok_or_else(|| invalid("Terrain fragment output count overflow"))?;
    if *count > crate::clip::MAX_TERRAIN_PATCHES {
        return Err(invalid(format!(
            "Terrain fragment cut exceeds {} output patches",
            crate::clip::MAX_TERRAIN_PATCHES
        )));
    }
    Ok(())
}

fn groups(
    prepared: &Prepared<'_>,
    plane: Plane,
    keep: Keep,
    work: &mut Work,
) -> Result<(Vec<Group>, usize), EditError> {
    let source = &prepared.source;
    let cells = source.mesh.size - 1;
    let triangles = source.mesh.triangles().collect::<Vec<_>>();
    work.consume(triangles.len() + cells * cells)
        .map_err(invalid)?;
    let mut groups = Vec::new();
    let mut count = 0;
    for row in 0..cells {
        for column in 0..cells {
            let ring = cell_ring(source, row, column);
            let classification = sides(&ring, plane);
            match classification {
                [false, false] => {
                    return Err(invalid(
                        "Terrain fragment cut cannot assign a positive-area cell entirely within cut-plane precision",
                    ));
                }
                [true, false] | [false, true] => {
                    let back = classification[0];
                    if retained(keep, back) {
                        add_count(&mut count, 4)?;
                        groups.push(Group::Cell {
                            row,
                            column,
                            plane: Some(half_plane(plane, back)),
                            ring: cell_base_ring(source, row, column),
                        });
                    }
                }
                [true, true] => {
                    let first = 2 * (row * cells + column);
                    for &triangle in &triangles[first..first + 2] {
                        let ring = triangle.map(|index| source.mesh.positions[index]);
                        let classification = sides(&ring, plane);
                        if classification == [false, false] {
                            return Err(invalid(
                                "Terrain fragment cut cannot assign a coplanar original triangle",
                            ));
                        }
                        if classification != [true, true] && !retained(keep, classification[0]) {
                            continue;
                        }
                        work.consume(16 * FIELD_WORK).map_err(invalid)?;
                        let field = Box::new(PlanarField::triangle(
                            triangle.map(|index| source.mesh.base_positions[index]),
                            ring,
                        )?);
                        let axis = field.world_axis();
                        let axes = [(axis + 1) % 3, (axis + 2) % 3];
                        let paint = Box::new(PaintField::triangle(
                            triangle.map(|index| source.mesh.base_positions[index]),
                            triangle.map(|index| source.mesh.alphas[index]),
                            work,
                        )?);
                        let mut rings = if classification == [true, true] {
                            split_ring(&ring, &field, axes, plane, work)?
                        } else {
                            let mut rings = [Vec::new(), Vec::new()];
                            rings[usize::from(!classification[0])] = ring.to_vec();
                            rings
                        };
                        for back in [true, false] {
                            let ring = &mut rings[usize::from(!back)];
                            if !retained(keep, back) {
                                ring.clear();
                            } else if !ring.is_empty() {
                                if !(3..=4).contains(&ring.len()) {
                                    return Err(invalid(
                                        "Clipped original terrain triangles require three or four corners",
                                    ));
                                }
                                add_count(&mut count, ring.len())?;
                            }
                        }
                        groups.push(Group::Triangle {
                            field,
                            paint,
                            rings,
                        });
                    }
                }
            }
        }
    }
    Ok((groups, count))
}

fn cell_ring(source: &Admitted<'_>, row: usize, column: usize) -> [DVec3; 4] {
    [
        row * source.mesh.size + column,
        (row + 1) * source.mesh.size + column,
        (row + 1) * source.mesh.size + column + 1,
        row * source.mesh.size + column + 1,
    ]
    .map(|index| source.mesh.positions[index])
}

fn cell_base_ring(source: &Admitted<'_>, row: usize, column: usize) -> [DVec3; 4] {
    [
        row * source.mesh.size + column,
        (row + 1) * source.mesh.size + column,
        (row + 1) * source.mesh.size + column + 1,
        row * source.mesh.size + column + 1,
    ]
    .map(|index| source.mesh.base_positions[index])
}

enum Expected<'a> {
    Triangle(&'a PlanarField, &'a PaintField),
    Window(&'a [Sample]),
}

struct Certificate<'a> {
    side: u32,
    texture: TerrainTexture,
    labels: &'a TriangleField,
    blend_mode: TerrainBlendMode,
}

impl Certificate<'_> {
    fn child(
        &self,
        mut part: Solid,
        base: [DVec3; 4],
        plane: Option<Plane>,
        expected: Expected<'_>,
        work: &mut Work,
    ) -> Result<(Solid, Child, TerrainBlendError), EditError> {
        let side_index = part
            .sides
            .iter()
            .position(|side| side.id == self.side)
            .unwrap();
        let shape = BrushGeometry::from_solid(&part).map_err(|error| invalid(error.to_string()))?;
        let face = shape
            .faces
            .iter()
            .find(|face| face.side == side_index)
            .ok_or_else(|| invalid("Terrain fragment carrier lost its source face"))?;
        let info = part.sides[side_index].displacement.as_ref().unwrap();
        let (_, mesh) = native::checked_mesh(face, info).map_err(invalid)?;
        let corners =
            [0, (SIZE - 1) * SIZE, VERTICES - 1, SIZE - 1].map(|index| mesh.base_positions[index]);
        if corners.iter().zip(base).any(|(&a, b)| !same(a, b)) {
            return Err(invalid(
                "Terrain fragment child orientation differs from its planned cage",
            ));
        }
        let (data, stored_base) = data(face, &mesh, super::POWER)?;
        let profiles = reconstructed(&data, stored_base, work)?;
        work.consume((2 * FIELD_WORK + profiles.len() + 1) * VERTICES)
            .map_err(invalid)?;
        for (index, &point) in mesh.positions.iter().enumerate() {
            let matches = match &expected {
                Expected::Triangle(field, _) => {
                    same(field.base_position(point)?, mesh.base_positions[index])
                        && same(field.position(mesh.base_positions[index])?, point)
                }
                Expected::Window(samples) => {
                    same(samples[index].base, mesh.base_positions[index])
                        && same(samples[index].point, point)
                        && samples[index].alpha == mesh.alphas[index]
                }
            };
            if !matches
                || plane.is_some_and(|plane| plane.distance_to(point) > CUT_EPSILON)
                || profiles
                    .iter()
                    .any(|positions| !same(positions[index], point))
            {
                return Err(invalid(
                    "Terrain fragments must preserve exact source samples and native retained-half separation",
                ));
            }
        }
        match expected {
            Expected::Triangle(_, paint) if self.blend_mode == TerrainBlendMode::Exact => {
                paint.certify(&mesh, &data, work)?;
            }
            _ => paint::validate_data(&mesh, &data, work)?,
        }
        let mapped = self.labels.map(&mesh, work)?;
        let info = part.sides[side_index].displacement.as_mut().unwrap();
        info.triangle_tags = mapped.tags.clone();
        mapped.verify(info, &mesh, work)?;
        certify_local_normals(&data, &profiles, work)?;
        uv(&data, face, mesh.positions.len(), self.texture, work)?;
        Ok((
            part,
            Child {
                base,
                base_positions: mesh.base_positions,
                positions: mesh.positions,
            },
            mapped.blend_error,
        ))
    }
}

fn quarter(
    solid: &Solid,
    source: &Admitted<'_>,
    window: [usize; 4],
    material: &str,
    blend_mode: TerrainBlendMode,
    work: &mut Work,
) -> Result<(Solid, [DVec3; 4], Vec<Sample>), EditError> {
    let [row, column, child_row, child_column] = window;
    work.consume(4 * FIELD_WORK * VERTICES).map_err(invalid)?;
    let mut samples = (0..VERTICES)
        .map(|index| sample_window(source, row, column, index, child_row, child_column, 2))
        .collect::<Result<Vec<_>, _>>()?;
    if blend_mode != TerrainBlendMode::Exact {
        let source_work = if blend_mode == TerrainBlendMode::NearestByte {
            source.mesh.base_positions.len()
        } else {
            0
        };
        work.consume((FIELD_WORK + source_work) * samples.len())
            .map_err(invalid)?;
        for sample in &mut samples {
            if blend_mode != TerrainBlendMode::NearestByte
                || !source.mesh.base_positions.contains(&sample.base)
            {
                sample.alpha = paint::sample_alpha(sample.alpha, blend_mode)?;
            }
        }
    }
    let mut base = [DVec3::ZERO; 4];
    for (point, index) in base
        .iter_mut()
        .zip([0, (SIZE - 1) * SIZE, VERTICES - 1, SIZE - 1])
    {
        *point = stored(samples[index].base)?;
    }
    let (mut part, mesh) = carrier_base(
        solid,
        source.side,
        source.face,
        base,
        material,
        &source.editing,
        work,
    )?;
    let mut offsets = Vec::with_capacity(VERTICES);
    for (sample, &point) in samples.iter().zip(&mesh.base_positions) {
        offsets.push(sample.offset(point)?);
    }
    let info = part
        .sides
        .iter_mut()
        .find(|side| side.id == source.side.id)
        .unwrap()
        .displacement
        .as_mut()
        .unwrap();
    if !source.editing.enabled() {
        info.offsets = Some(offsets.chunks(SIZE).map(<[_]>::to_vec).collect());
    }
    info.alphas = source.info.alphas.as_ref().map(|_| {
        samples
            .chunks(SIZE)
            .map(|row| row.iter().map(|sample| sample.alpha).collect())
            .collect()
    });
    Ok((part, base, samples))
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
    let (groups, count) = groups(&prepared, request.plane, request.keep, work)?;
    materialize(
        solid,
        &prepared,
        groups,
        count,
        Some(request.plane),
        PartitionRequest {
            material: request.material,
            textures: request.textures,
            blend_mode: request.blend_mode,
        },
        work,
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
    let work = &mut context.work;
    let source = admit_source(document, solid, geometry, work)?;
    let (count, _, _) = partition_counts(source.info.power, TerrainMode::Fragments)?;
    let sheet = intrinsic_source(&source, work)?;
    let cells = source.mesh.size - 1;
    work.consume(cells * cells).map_err(invalid)?;
    let mut groups = Vec::with_capacity(cells * cells);
    for row in 0..cells {
        for column in 0..cells {
            groups.push(Group::Cell {
                row,
                column,
                plane: None,
                ring: cell_base_ring(&source, row, column),
            });
        }
    }
    let prepared = Prepared { source, sheet };
    materialize(solid, &prepared, groups, count, None, request, work)
}

fn materialize(
    solid: &Solid,
    prepared: &Prepared<'_>,
    groups: Vec<Group>,
    count: usize,
    cut_plane: Option<Plane>,
    request: PartitionRequest<'_>,
    work: &mut Work,
) -> Result<TerrainPlan, EditError> {
    let source = &prepared.source;
    let identity = FaceId {
        solid: solid.id,
        side: source.side.id,
    };
    let texture = request.textures.and_then(|textures| textures.get(&identity)).copied()
        .ok_or_else(|| invalid("Terrain fragments require guarded native texture projection and mapping dimensions"))?;
    let labels = TriangleField::surface_with_blend_mode(
        source.info,
        &source.mesh,
        texture.has_secondary_surface,
        request.blend_mode,
        work,
    )?;
    uv(
        &source.data,
        source.face,
        source.mesh.positions.len(),
        texture,
        work,
    )?;
    let certificate = Certificate {
        side: source.side.id,
        texture,
        labels: &labels,
        blend_mode: request.blend_mode,
    };
    let mut blend_error = TerrainBlendError::default();
    let mut parts = Vec::with_capacity(count);
    let mut children = Vec::with_capacity(count);
    for group in groups {
        match group {
            Group::Cell {
                row,
                column,
                plane,
                ring,
            } => {
                let first = children.len();
                for child_row in 0..2 {
                    for child_column in 0..2 {
                        let (part, base, samples) = quarter(
                            solid,
                            source,
                            [row, column, child_row, child_column],
                            request.material,
                            request.blend_mode,
                            work,
                        )?;
                        let (part, child, error) = certificate.child(
                            part,
                            base,
                            plane,
                            Expected::Window(&samples),
                            work,
                        )?;
                        blend_error.merge(error)?;
                        parts.push(part);
                        children.push(child);
                    }
                }
                region_base(
                    &ring,
                    &children[first..],
                    prepared.sheet.axes,
                    prepared.sheet.sign,
                    work,
                )?;
            }
            Group::Triangle {
                field,
                paint,
                rings,
            } => {
                let cut_plane = cut_plane.ok_or_else(|| {
                    invalid("Terrain triangle fragments lost their cut-plane proof")
                })?;
                for back in [true, false] {
                    let ring = &rings[usize::from(!back)];
                    if ring.is_empty() {
                        continue;
                    }
                    let first = children.len();
                    let plane = half_plane(cut_plane, back);
                    let axis = field.world_axis();
                    let axes = [(axis + 1) % 3, (axis + 2) % 3];
                    let sign = orient(ring[0], ring[1], ring[2], axes)?;
                    for quad in quads(ring, axes, sign, true)? {
                        work.consume(FIELD_WORK * quad.len()).map_err(invalid)?;
                        let mut base = [DVec3::ZERO; 4];
                        for (point, world) in base.iter_mut().zip(quad) {
                            *point = stored(field.base_position(world)?)?;
                        }
                        let part = carrier_field(
                            solid,
                            source.side,
                            source.face,
                            base,
                            &field,
                            &paint,
                            &source.editing,
                            request.material,
                            request.blend_mode,
                            work,
                        )?;
                        let (part, child, error) = certificate.child(
                            part,
                            base,
                            Some(plane),
                            Expected::Triangle(&field, &paint),
                            work,
                        )?;
                        blend_error.merge(error)?;
                        parts.push(part);
                        children.push(child);
                    }
                    region(
                        ring,
                        &children[first..],
                        axes,
                        sign,
                        plane,
                        Some(&field),
                        work,
                    )?;
                }
            }
        }
    }
    connections(&children, work)?;
    Ok(TerrainPlan {
        vertices: parts.len() * VERTICES,
        triangles: parts.len() * TRIANGLES,
        parts,
        source: identity,
        source_power: source.info.power,
        output_power: Some(super::POWER),
        children: None,
        blend_error: (request.blend_mode != TerrainBlendMode::Exact).then_some(blend_error),
    })
}
