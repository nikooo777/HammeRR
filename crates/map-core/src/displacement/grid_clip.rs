use glam::DVec3;
use source_formats::vmf::{DispInfo, Solid};

use super::{ALLOWED_VERTEX_MASK_WORDS, DisplacementMesh, native, neighbors, smoothing, triangles};
use crate::clip::Keep;
use crate::surface::FaceId;
use crate::{BrushGeometry, Document, EditError, ObjectId, Plane};

const SAMPLE_EPSILON: f64 = 1e-5;
const CUT_EPSILON: f64 = 1e-7;

mod network;
pub(crate) use network::{MAX_NEIGHBORS, Replacement, certify};

#[derive(Clone, Copy)]
pub(crate) struct Child {
    pub row: usize,
    pub column: usize,
}

pub(crate) struct Plan {
    pub parts: Vec<Solid>,
    pub source: FaceId,
    pub source_power: u32,
    pub output_power: u32,
    pub vertices: usize,
    pub triangles: usize,
    pub children: Vec<Child>,
}

fn invalid(message: impl Into<String>) -> EditError {
    EditError::Invalid(message.into())
}

fn checked_rows<T>(rows: &Option<Vec<Vec<T>>>, size: usize) -> Result<(), EditError> {
    if rows
        .as_ref()
        .is_some_and(|rows| rows.len() != size || rows.iter().any(|row| row.len() != size))
    {
        return Err(invalid("Terrain grid fields have invalid row dimensions"));
    }
    Ok(())
}

fn slice<T: Clone>(
    rows: &Option<Vec<Vec<T>>>,
    row: usize,
    column: usize,
    cells: usize,
) -> Option<Vec<Vec<T>>> {
    rows.as_ref().map(|rows| {
        rows[row..=row + cells]
            .iter()
            .map(|row| row[column..=column + cells].to_vec())
            .collect()
    })
}

fn validate(info: &DispInfo, mesh: &DisplacementMesh) -> Result<(), EditError> {
    if !(3..=4).contains(&info.power) {
        return Err(invalid(
            "Terrain clipping requires power 3 or 4 and an existing grid half-line",
        ));
    }
    checked_rows(&info.normals, mesh.size)?;
    checked_rows(&info.distances, mesh.size)?;
    checked_rows(&info.offsets, mesh.size)?;
    checked_rows(&info.offset_normals, mesh.size)?;
    checked_rows(&info.alphas, mesh.size)?;
    if info
        .offset_normals
        .iter()
        .flatten()
        .flatten()
        .any(|normal| !normal.is_finite())
    {
        return Err(invalid("Terrain offset normals must be finite"));
    }
    if !info.extra.is_empty()
        || info.extra_properties.iter().any(|(key, _)| {
            !["smooth", "mintess", "uaxis", "vaxis"]
                .iter()
                .any(|known| key.eq_ignore_ascii_case(known))
        })
    {
        return Err(invalid(
            "Opaque terrain payloads cannot be remapped to clipped grids",
        ));
    }
    if info.allowed_verts.as_ref().is_some_and(|mask| {
        mask.len() != ALLOWED_VERTEX_MASK_WORDS || mask.iter().any(|word| *word != -1)
    }) {
        return Err(invalid(
            "Restricted terrain vertex masks cannot be remapped to clipped grids",
        ));
    }
    smoothing::angle(info).map_err(invalid)?;
    mesh.validate_edit_bounds()?;
    triangles::native_tags(info, mesh)?;
    Ok(())
}

fn point_segment_distance(point: DVec3, [a, b]: [DVec3; 2]) -> f64 {
    let edge = b - a;
    let length = edge.length_squared();
    if length == 0.0 {
        return point.distance(a);
    }
    point.distance(a + edge * ((point - a).dot(edge) / length).clamp(0.0, 1.0))
}

fn touches(first: &[DVec3], second: &[DVec3]) -> bool {
    (0..first.len()).any(|a| {
        let first = [first[a], first[(a + 1) % first.len()]];
        (0..second.len()).any(|b| {
            let second = [second[b], second[(b + 1) % second.len()]];
            first
                .into_iter()
                .any(|point| point_segment_distance(point, second) <= neighbors::EDGE_TOLERANCE)
                || second
                    .into_iter()
                    .any(|point| point_segment_distance(point, first) <= neighbors::EDGE_TOLERANCE)
        })
    })
}

fn contains(corners: &[DVec3], point: DVec3) -> bool {
    let Some(plane) = Plane::from_points([corners[0], corners[1], corners[2]]) else {
        return true;
    };
    plane.distance_to(point).abs() <= neighbors::EDGE_TOLERANCE
        && (0..corners.len()).all(|index| {
            let a = corners[index];
            let edge = corners[(index + 1) % corners.len()] - a;
            edge.cross(plane.normal).dot(point - a) >= -neighbors::EDGE_TOLERANCE * edge.length()
        })
}

fn resolved_instances(document: &Document) -> Result<(), EditError> {
    let vmf = document.vmf();
    if vmf.entities.iter().any(|entity| {
        entity
            .classname()
            .is_some_and(|class| class.eq_ignore_ascii_case("func_instance"))
    }) {
        return Err(invalid(
            "Terrain clipping requires resolved instance contents; collapse map instances before checking terrain isolation",
        ));
    }
    Ok(())
}

fn half_line(mesh: &DisplacementMesh, plane: Plane, rows: bool) -> Option<bool> {
    let middle = mesh.size / 2;
    let mut negative = [false; 2];
    let mut positive = [false; 2];
    for (index, point) in mesh.positions.iter().enumerate() {
        let coordinate = if rows {
            index / mesh.size
        } else {
            index % mesh.size
        };
        let distance = plane.distance_to(*point);
        if coordinate == middle {
            if distance.abs() > CUT_EPSILON {
                return None;
            }
        } else {
            if distance.abs() <= CUT_EPSILON {
                return None;
            }
            let half = usize::from(coordinate > middle);
            negative[half] |= distance < -CUT_EPSILON;
            positive[half] |= distance > CUT_EPSILON;
        }
    }
    match (negative, positive) {
        ([true, false], [false, true]) => Some(true),
        ([false, true], [true, false]) => Some(false),
        _ => None,
    }
}

fn carrier_plane(a: DVec3, b: DVec3, normal: DVec3) -> Result<Plane, EditError> {
    let normal = (b - a)
        .cross(normal)
        .try_normalize()
        .ok_or_else(|| invalid("Terrain grid half-line has no carrier plane"))?;
    Ok(Plane {
        normal,
        dist: normal.dot(a),
    })
}

fn behind(mut plane: Plane, point: DVec3) -> Plane {
    if plane.distance_to(point) > 0.0 {
        plane.normal = -plane.normal;
        plane.dist = -plane.dist;
    }
    plane
}

fn child_info(info: &DispInfo, mesh: &DisplacementMesh, row: usize, column: usize) -> DispInfo {
    let cells = (mesh.size - 1) / 2;
    let mut result = info.clone();
    result.power -= 1;
    result.start_position = mesh.base_positions[row * mesh.size + column];
    result.normals = slice(&info.normals, row, column, cells);
    result.distances = slice(&info.distances, row, column, cells);
    result.offsets = slice(&info.offsets, row, column, cells);
    result.offset_normals = slice(&info.offset_normals, row, column, cells);
    result.alphas = slice(&info.alphas, row, column, cells);
    result.triangle_tags = info.triangle_tags.as_ref().map(|rows| {
        rows[row..row + cells]
            .iter()
            .map(|row| row[column * 2..(column + cells) * 2].to_vec())
            .collect()
    });
    result
}

pub(crate) fn plan(
    document: &Document,
    solid: &Solid,
    geometry: &BrushGeometry,
    plane: Plane,
    keep: Keep,
    material: &str,
) -> Result<Option<Plan>, EditError> {
    let displaced = solid
        .sides
        .iter()
        .enumerate()
        .filter(|(_, side)| side.displacement.is_some())
        .collect::<Vec<_>>();
    let [(side_index, side)] = displaced.as_slice() else {
        return Err(invalid(
            "Terrain clipping requires one displaced face per brush",
        ));
    };
    let face = geometry
        .faces
        .iter()
        .find(|face| face.side == *side_index)
        .ok_or_else(|| invalid("Terrain has no effective base face"))?;
    let info = side.displacement.as_ref().unwrap();
    let mesh = DisplacementMesh::build(face, info).map_err(|error| invalid(error.to_string()))?;
    let front = mesh
        .positions
        .iter()
        .any(|point| plane.distance_to(*point) > CUT_EPSILON);
    let back = mesh
        .positions
        .iter()
        .any(|point| plane.distance_to(*point) < -CUT_EPSILON);
    if !front || !back {
        return Ok(None);
    }
    let (_, mesh) = native::checked_mesh(face, info).map_err(invalid)?;
    if document.owner_of_solid(solid.id) != Some(ObjectId::Solid(solid.id)) {
        return Err(invalid("Terrain clipping requires a world brush"));
    }
    validate(info, &mesh)?;
    crate::subobject::fragments::admit_source(solid)?;
    resolved_instances(document)?;
    let (rows, first_back) = half_line(&mesh, plane, true)
        .map(|first_back| (true, first_back))
        .or_else(|| half_line(&mesh, plane, false).map(|first_back| (false, first_back)))
        .ok_or_else(|| {
            invalid("Cut plane must follow a complete existing terrain grid half-line, with each half wholly on one side")
        })?;
    let cells = (mesh.size - 1) / 2;
    let last = mesh.size - 1;
    let row_plane = carrier_plane(
        mesh.base_positions[cells * mesh.size],
        mesh.base_positions[cells * mesh.size + last],
        face.plane.normal,
    )?;
    let column_plane = carrier_plane(
        mesh.base_positions[cells],
        mesh.base_positions[last * mesh.size + cells],
        face.plane.normal,
    )?;
    let original_tags = triangles::native_tags(info, &mesh)?;
    let mut parts = Vec::new();
    let mut children = Vec::new();
    for back in [true, false] {
        if matches!((keep, back), (Keep::Back, false) | (Keep::Front, true)) {
            continue;
        }
        for (row, column) in [(0, 0), (0, cells), (cells, 0), (cells, cells)] {
            let second = if rows { row != 0 } else { column != 0 };
            if (first_back != second) != back {
                continue;
            }
            let center = mesh.base_positions[(row + cells / 2) * mesh.size + column + cells / 2];
            let part = crate::clip::half(solid, behind(row_plane, center), material)?;
            let mut part = crate::clip::half(&part, behind(column_plane, center), material)?;
            let index = part
                .sides
                .iter()
                .position(|candidate| candidate.id == side.id)
                .ok_or_else(|| invalid("Terrain child lost its base face"))?;
            part.sides[index].displacement = Some(child_info(info, &mesh, row, column));
            let geometry =
                BrushGeometry::from_solid(&part).map_err(|error| invalid(error.to_string()))?;
            let face = geometry
                .faces
                .iter()
                .find(|face| face.side == index)
                .ok_or_else(|| invalid("Terrain child has no effective base face"))?;
            let child = part.sides[index].displacement.as_ref().unwrap();
            let (_, output) = native::checked_mesh(face, child).map_err(invalid)?;
            for child_row in 0..output.size {
                for child_column in 0..output.size {
                    let target = child_row * output.size + child_column;
                    let source = (row + child_row) * mesh.size + column + child_column;
                    let original_uv = crate::brush::texel_uv(side, mesh.base_positions[source]);
                    let child_uv = crate::brush::texel_uv(side, output.base_positions[target]);
                    if output.positions[target].distance(mesh.positions[source]) > SAMPLE_EPSILON
                        || output.base_positions[target].distance(mesh.base_positions[source])
                            > SAMPLE_EPSILON
                        || output.alphas[target] != mesh.alphas[source]
                        || !original_uv.is_finite()
                        || !child_uv.is_finite()
                        || (original_uv - child_uv).abs().max_element()
                            > crate::texture_deform::TEXEL_EPSILON
                    {
                        return Err(invalid(
                            "Terrain carrier cannot preserve its existing grid samples and base UV",
                        ));
                    }
                }
            }
            let tags = triangles::native_tags(child, &output)?;
            for child_row in 0..cells {
                for child_column in 0..cells {
                    for triangle in 0..2 {
                        if tags[(child_row * cells + child_column) * 2 + triangle]
                            != original_tags
                                [((row + child_row) * last + column + child_column) * 2 + triangle]
                        {
                            return Err(invalid(
                                "Terrain child cannot preserve physical triangle tags",
                            ));
                        }
                    }
                }
            }
            parts.push(part);
            children.push(Child { row, column });
        }
    }
    let count = parts.len();
    Ok(Some(Plan {
        parts,
        source: FaceId {
            solid: solid.id,
            side: side.id,
        },
        source_power: info.power,
        output_power: info.power - 1,
        vertices: count * (cells + 1) * (cells + 1),
        triangles: count * cells * cells * 2,
        children,
    }))
}
