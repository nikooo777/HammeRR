use std::collections::BTreeMap;

use glam::DVec3;
use source_formats::vmf::DispInfo;

use super::{
    edit::{TerrainGrid, inspect},
    neighbors::{SurfaceGrid, common_span_indices, find_spans},
};
use crate::{
    Document, EditError,
    surface::{FaceId, side},
};

pub struct Plan {
    pub edits: Vec<(FaceId, DispInfo)>,
    pub pairs: usize,
    pub corner_joins: usize,
    pub vertices: usize,
    pub interpolated_vertices: usize,
}

const POSITION_EPSILON: f64 = 1e-9;

struct Average {
    point: DVec3,
    alpha: f64,
    count: usize,
    first_point: DVec3,
    first_alpha: f64,
    same_point: bool,
    same_alpha: bool,
}

fn invalid(message: impl Into<String>) -> EditError {
    EditError::Invalid(message.into())
}

fn root(parents: &mut [usize], mut index: usize) -> usize {
    while parents[index] != index {
        parents[index] = parents[parents[index]];
        index = parents[index];
    }
    index
}

fn corners(grid: &TerrainGrid) -> [DVec3; 4] {
    let n = grid.mesh.size;
    let points = &grid.mesh.base_positions;
    [
        points[0],
        points[n * (n - 1)],
        points[n * n - 1],
        points[n - 1],
    ]
}

pub fn plan(document: &Document, faces: &[FaceId], blend: bool) -> Result<Plan, EditError> {
    let grids = inspect(document, faces)?;
    let views = grids
        .iter()
        .map(|grid| SurfaceGrid {
            power: grid.power,
            corners: corners(grid).map(|point| point.as_vec3().as_dvec3()),
            positions: &grid.mesh.base_positions,
        })
        .collect::<Vec<_>>();
    let neighbors = find_spans(&views, &mut |_| Ok(())).map_err(invalid)?;
    if neighbors.ambiguous > 0 {
        return Err(invalid(
            "Sewing needs unambiguous full or whole-to-half terrain edges",
        ));
    }
    let mut starts = Vec::with_capacity(grids.len());
    let mut count = 0;
    for grid in &grids {
        starts.push(count);
        count += grid.mesh.positions.len();
    }
    let mut parents = (0..count).collect::<Vec<_>>();
    for pair in &neighbors.pairs {
        for (a, b) in common_span_indices(
            (grids[pair.first].power, pair.first_edge, pair.first_span),
            (grids[pair.second].power, pair.second_edge, pair.second_span),
        )
        .map_err(invalid)?
        {
            let a = root(&mut parents, starts[pair.first] + a);
            let b = root(&mut parents, starts[pair.second] + b);
            let (low, high) = if a < b { (a, b) } else { (b, a) };
            parents[high] = low;
        }
    }
    let mut corner_groups = BTreeMap::<[u64; 3], Vec<(usize, usize)>>::new();
    for (index, grid) in grids.iter().enumerate() {
        let n = grid.mesh.size;
        for (corner, vertex) in [0, n * (n - 1), n * n - 1, n - 1].into_iter().enumerate() {
            corner_groups
                .entry(super::neighbors::key(views[index].corners[corner]))
                .or_default()
                .push((index, vertex));
        }
    }
    let mut corner_joins = 0;
    for group in corner_groups.values() {
        if group
            .iter()
            .map(|(grid, _)| grid)
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != group.len()
        {
            return Err(invalid(
                "Terrain base corners collapse at Source float precision",
            ));
        }
        for pair in group.windows(2) {
            let a = root(&mut parents, starts[pair[0].0] + pair[0].1);
            let b = root(&mut parents, starts[pair[1].0] + pair[1].1);
            if a != b {
                let (low, high) = if a < b { (a, b) } else { (b, a) };
                parents[high] = low;
                corner_joins += 1;
            }
        }
    }
    if neighbors.pairs.is_empty() && corner_joins == 0 {
        return Err(invalid(
            "No matching full/half terrain borders or base corners were found",
        ));
    }
    let mut sums = BTreeMap::<usize, Average>::new();
    for (index, grid) in grids.iter().enumerate() {
        for (vertex, point) in grid.mesh.positions.iter().enumerate() {
            let root = root(&mut parents, starts[index] + vertex);
            let alpha = grid.mesh.alphas[vertex];
            let sum = sums.entry(root).or_insert(Average {
                point: DVec3::ZERO,
                alpha: 0.0,
                count: 0,
                first_point: *point,
                first_alpha: alpha,
                same_point: true,
                same_alpha: true,
            });
            sum.point += *point;
            sum.alpha += alpha;
            sum.count += 1;
            sum.same_point &= sum.first_point == *point;
            sum.same_alpha &= sum.first_alpha == alpha;
        }
    }
    let vertices = sums.values().filter(|sum| sum.count > 1).count();
    let mut points = grids
        .iter()
        .map(|grid| grid.mesh.positions.clone())
        .collect::<Vec<_>>();
    let mut blends = grids
        .iter()
        .map(|grid| grid.mesh.alphas.clone())
        .collect::<Vec<_>>();
    for (index, grid) in grids.iter().enumerate() {
        for vertex in 0..grid.mesh.positions.len() {
            let owner = root(&mut parents, starts[index] + vertex);
            let sum = &sums[&owner];
            if sum.count < 2 {
                continue;
            }
            points[index][vertex] = if sum.same_point {
                sum.first_point
            } else {
                sum.point / sum.count as f64
            };
            if blend {
                blends[index][vertex] = if sum.same_alpha {
                    sum.first_alpha
                } else {
                    sum.alpha / sum.count as f64
                };
            }
        }
    }
    let mut interpolated_vertices = 0;
    for pair in &neighbors.pairs {
        let a = pair
            .first_span
            .indices(grids[pair.first].power, pair.first_edge)
            .map_err(invalid)?;
        let b = pair
            .second_span
            .indices(grids[pair.second].power, pair.second_edge)
            .map_err(invalid)?;
        if a.len() == b.len() {
            continue;
        }
        let (fine_patch, fine, coarse_patch, mut coarse) = if a.len() > b.len() {
            (pair.first, a, pair.second, b)
        } else {
            (pair.second, b, pair.first, a)
        };
        coarse.reverse();
        let stride = (fine.len() - 1) / (coarse.len() - 1);
        for (step, &vertex) in fine.iter().enumerate() {
            if step % stride == 0 {
                continue;
            }
            let segment = step / stride;
            let t = (step % stride) as f64 / stride as f64;
            points[fine_patch][vertex] = points[coarse_patch][coarse[segment]]
                .lerp(points[coarse_patch][coarse[segment + 1]], t);
            if blend {
                let a = blends[coarse_patch][coarse[segment]];
                let b = blends[coarse_patch][coarse[segment + 1]];
                blends[fine_patch][vertex] = a + (b - a) * t;
            }
            interpolated_vertices += 1;
        }
    }
    let mut edits = Vec::with_capacity(grids.len());
    for (index, grid) in grids.iter().enumerate() {
        let mut info = side(document, grid.face)
            .unwrap()
            .displacement
            .clone()
            .unwrap();
        if !info.extra.is_empty()
            || info.normals.is_some() != info.distances.is_some()
            || info.offset_normals.as_ref().is_some_and(|rows| {
                rows.len() != grid.mesh.size
                    || rows.iter().any(|row| {
                        row.len() != grid.mesh.size || row.iter().any(|p| *p != DVec3::ZERO)
                    })
            })
        {
            return Err(invalid(
                "Opaque terrain blocks or offset-normal caches need remapping before sewing",
            ));
        }
        if grid
            .mesh
            .positions
            .iter()
            .any(|p| p.abs().max_element() > crate::primitive::MAX_COORDINATE)
            || grid.mesh.alphas.iter().any(|a| !(0.0..=255.0).contains(a))
        {
            return Err(invalid(
                "Terrain positions must fit ±65536 and blend values must be 0–255",
            ));
        }
        let mut offsets = info
            .offsets
            .clone()
            .unwrap_or_else(|| vec![vec![DVec3::ZERO; grid.mesh.size]; grid.mesh.size]);
        let mut alphas = info
            .alphas
            .clone()
            .unwrap_or_else(|| vec![vec![0.0; grid.mesh.size]; grid.mesh.size]);
        let mut moved = false;
        let mut painted = false;
        for vertex in 0..grid.mesh.positions.len() {
            let row = vertex / grid.mesh.size;
            let col = vertex % grid.mesh.size;
            let mut change = points[index][vertex] - grid.mesh.positions[vertex];
            if change.length_squared() <= POSITION_EPSILON * POSITION_EPSILON {
                change = DVec3::ZERO;
            }
            let updated = offsets[row][col] + change;
            if !updated.is_finite() {
                return Err(invalid("Sewn terrain offsets must be finite"));
            }
            offsets[row][col] = updated;
            moved |= change != DVec3::ZERO;
            if blend {
                let value = blends[index][vertex];
                painted |= value != alphas[row][col];
                alphas[row][col] = value;
            }
        }
        if moved {
            info.offsets = Some(offsets);
        }
        if painted {
            info.alphas = Some(alphas);
        }
        edits.push((grid.face, info));
    }
    Ok(Plan {
        edits,
        pairs: neighbors.pairs.len(),
        corner_joins,
        vertices,
        interpolated_vertices,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ObjectId, edit};

    fn fixture() -> (Document, Vec<FaceId>) {
        let mut doc = Document::empty();
        let mut faces = Vec::new();
        for (index, (x, y)) in [(-64.0, -64.0), (0.0, -64.0), (-64.0, 0.0), (0.0, 0.0)]
            .into_iter()
            .enumerate()
        {
            let ObjectId::Solid(id) = doc
                .edit("patch", |tx| {
                    edit::create_block(
                        tx,
                        DVec3::new(x, y, 0.0),
                        DVec3::new(x + 64.0, y + 64.0, 16.0),
                        "brick/test",
                    )
                })
                .unwrap()
            else {
                unreachable!()
            };
            let target = FaceId {
                solid: id,
                side: doc
                    .solid(id)
                    .unwrap()
                    .sides
                    .iter()
                    .find(|side| crate::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
                    .unwrap()
                    .id,
            };
            doc.edit("terrain", |tx| {
                super::super::edit::create(tx, &[target], 2, index as u32)
            })
            .unwrap();
            doc.edit("heights", |tx| {
                let solid = tx.solid_mut(id)?;
                let info = solid
                    .sides
                    .iter_mut()
                    .find(|side| side.id == target.side)
                    .unwrap()
                    .displacement
                    .as_mut()
                    .unwrap();
                info.elevation = index as f64 * 16.0;
                info.alphas = Some(vec![vec![index as f64 * 64.0; 5]; 5]);
                Ok(())
            })
            .unwrap();
            faces.push(target);
        }
        (doc, faces)
    }

    #[test]
    fn sewing_four_patch_junctions_averages_once_and_preserves_interiors_and_undo() {
        let (mut doc, faces) = fixture();
        let before = doc.vmf().clone();
        let planned = plan(&doc, &faces, true).unwrap();
        assert_eq!(planned.pairs, 4);
        let mut reversed = faces.clone();
        reversed.reverse();
        assert_eq!(planned.edits, plan(&doc, &reversed, true).unwrap().edits);
        let old = inspect(&doc, &faces).unwrap();
        doc.edit("sew", |tx| {
            for (target, info) in &planned.edits {
                tx.solid_mut(target.solid)?
                    .sides
                    .iter_mut()
                    .find(|side| side.id == target.side)
                    .unwrap()
                    .displacement = Some(info.clone());
            }
            Ok(())
        })
        .unwrap();
        let after = inspect(&doc, &faces).unwrap();
        for (a, b) in old.iter().zip(&after) {
            let center = b
                .mesh
                .base_positions
                .iter()
                .position(|p| p.x == 0.0 && p.y == 0.0)
                .unwrap();
            assert_eq!(b.mesh.positions[center], DVec3::new(0.0, 0.0, 40.0));
            assert_eq!(b.mesh.alphas[center], 96.0);
            for row in 1..4 {
                for col in 1..4 {
                    let i = row * 5 + col;
                    assert_eq!(a.mesh.positions[i], b.mesh.positions[i]);
                    assert_eq!(a.mesh.alphas[i], b.mesh.alphas[i]);
                }
            }
        }
        let second = plan(&doc, &faces, true).unwrap();
        for (target, info) in &second.edits {
            assert_eq!(
                side(&doc, *target).unwrap().displacement.as_ref().unwrap(),
                info
            );
        }
        doc.undo().unwrap();
        assert_eq!(*doc.vmf(), before);
    }

    #[test]
    fn different_powers_sew_but_missing_neighbors_reject_without_source_changes() {
        let (mut doc, faces) = fixture();
        let before = doc.vmf().clone();
        assert!(plan(&doc, &faces[..1], false).is_err());
        assert_eq!(*doc.vmf(), before);
        doc.edit("different power", |tx| {
            let solid = tx.solid_mut(faces[0].solid)?;
            let info = solid
                .sides
                .iter_mut()
                .find(|side| side.id == faces[0].side)
                .unwrap()
                .displacement
                .as_mut()
                .unwrap();
            info.power = 3;
            info.normals = Some(vec![vec![DVec3::Z; 9]; 9]);
            info.distances = Some(vec![vec![0.0; 9]; 9]);
            info.alphas = None;
            Ok(())
        })
        .unwrap();
        assert!(plan(&doc, &faces, false).unwrap().interpolated_vertices > 0);
    }

    #[test]
    fn mixed_full_and_half_borders_interpolate_once_and_preserve_interior_metadata() {
        for split in [false, true] {
            let mut document = Document::empty();
            let mut faces = Vec::new();
            let mut patches = vec![(2, 1, [-64.1, -64.1, 0.0], [0.1, 64.1, 16.0])];
            if split {
                patches.extend([
                    (3, 3, [0.1, -64.1, 0.0], [64.1, 0.0, 16.0]),
                    (4, 2, [0.1, 0.0, 0.0], [64.1, 64.1, 16.0]),
                ]);
            } else {
                patches.push((4, 3, [0.1, -64.1, 0.0], [64.1, 64.1, 16.0]));
            }
            for (index, (power, corner, low, high)) in patches.into_iter().enumerate() {
                let face = document
                    .edit("terrain", |tx| {
                        let ObjectId::Solid(id) = edit::create_block(
                            tx,
                            DVec3::from_array(low),
                            DVec3::from_array(high),
                            "brick/test",
                        )?
                        else {
                            unreachable!()
                        };
                        let solid = tx.document().solid(id).unwrap();
                        let geometry = crate::BrushGeometry::from_solid(solid).unwrap();
                        let face = geometry
                            .faces
                            .iter()
                            .find(|face| face.plane.normal.z > 0.99)
                            .unwrap();
                        let target = FaceId {
                            solid: id,
                            side: solid.sides[face.side].id,
                        };
                        super::super::edit::create(tx, &[target], power, corner)?;
                        let info = tx.solid_mut(id)?.sides[face.side]
                            .displacement
                            .as_mut()
                            .unwrap();
                        info.elevation = index as f64 * 16.0 + 0.1;
                        info.alphas = Some(vec![
                            vec![index as f64 * 100.0 + 0.1; (1 << power) + 1];
                            (1 << power) + 1
                        ]);
                        info.extra_properties.set("custom_grid_note", "keep");
                        Ok(target)
                    })
                    .unwrap();
                faces.push(face);
            }
            let source = document.vmf().clone();
            let before = inspect(&document, &faces).unwrap();
            let original = faces
                .iter()
                .map(|&face| side(&document, face).unwrap().displacement.clone().unwrap())
                .collect::<Vec<_>>();
            let planned = plan(&document, &faces, true).unwrap();
            assert_eq!(planned.pairs, if split { 3 } else { 1 });
            assert!(planned.interpolated_vertices > 0);
            for (index, (_, info)) in planned.edits.iter().enumerate() {
                let mut metadata = info.clone();
                metadata.offsets = original[index].offsets.clone();
                metadata.alphas = original[index].alphas.clone();
                assert_eq!(metadata, original[index]);
            }
            document
                .edit("sew", |tx| {
                    for (face, info) in &planned.edits {
                        tx.solid_mut(face.solid)?
                            .sides
                            .iter_mut()
                            .find(|side| side.id == face.side)
                            .unwrap()
                            .displacement = Some(info.clone());
                    }
                    Ok(())
                })
                .unwrap();
            let after = inspect(&document, &faces).unwrap();
            if split {
                for grid in &after {
                    let vertex = grid
                        .mesh
                        .base_positions
                        .iter()
                        .position(|point| (point.x - 0.1).abs() < 1e-9 && point.y.abs() < 1e-9)
                        .unwrap();
                    assert!((grid.mesh.positions[vertex].z - 32.1).abs() < 1e-9);
                    assert!((grid.mesh.alphas[vertex] - 100.1).abs() < 1e-9);
                }
                let vertex = after[1]
                    .mesh
                    .base_positions
                    .iter()
                    .position(|point| {
                        (point.x - 0.1).abs() < 1e-9 && (point.y + 16.025).abs() < 1e-9
                    })
                    .unwrap();
                assert!((after[1].mesh.positions[vertex].z - 28.1).abs() < 1e-9);
                assert!((after[1].mesh.alphas[vertex] - 75.1).abs() < 1e-9);
            }
            for (a, b) in before.iter().zip(&after) {
                for row in 1..a.mesh.size - 1 {
                    for col in 1..a.mesh.size - 1 {
                        let index = row * a.mesh.size + col;
                        assert_eq!(a.mesh.positions[index], b.mesh.positions[index]);
                        assert_eq!(a.mesh.alphas[index], b.mesh.alphas[index]);
                    }
                }
            }
            let next = plan(&document, &faces, true).unwrap();
            for (face, info) in next.edits {
                assert_eq!(
                    side(&document, face)
                        .unwrap()
                        .displacement
                        .as_ref()
                        .unwrap(),
                    &info
                );
            }
            document.undo().unwrap();
            assert_eq!(document.vmf(), &source);
        }
    }
    #[test]
    fn already_equal_fractional_three_patch_junctions_are_exact_no_ops() {
        let (mut doc, faces) = fixture();
        for face in &faces[..3] {
            doc.edit("same fractional samples", |tx| {
                let info = tx
                    .solid_mut(face.solid)?
                    .sides
                    .iter_mut()
                    .find(|side| side.id == face.side)
                    .unwrap()
                    .displacement
                    .as_mut()
                    .unwrap();
                info.elevation = 0.1;
                info.alphas = Some(vec![vec![0.1; 5]; 5]);
                Ok(())
            })
            .unwrap();
        }
        let plan = plan(&doc, &faces[..3], true).unwrap();
        for (face, info) in plan.edits {
            assert_eq!(
                side(&doc, face).unwrap().displacement.as_ref().unwrap(),
                &info
            );
        }
    }
    #[test]
    fn corner_only_sewing_averages_once_and_preserves_all_other_vertices() {
        let (mut document, faces) = fixture();
        let selected = [faces[0], faces[3]];
        let source = document.vmf().clone();
        let before = inspect(&document, &selected).unwrap();
        let plan = plan(&document, &selected, true).unwrap();
        assert_eq!(
            (
                plan.pairs,
                plan.corner_joins,
                plan.vertices,
                plan.interpolated_vertices
            ),
            (0, 1, 1, 0)
        );
        document
            .edit("sew corner", |tx| {
                for (face, info) in &plan.edits {
                    tx.solid_mut(face.solid)?
                        .sides
                        .iter_mut()
                        .find(|side| side.id == face.side)
                        .unwrap()
                        .displacement = Some(info.clone());
                }
                Ok(())
            })
            .unwrap();
        let after = inspect(&document, &selected).unwrap();
        for (old, new) in before.iter().zip(&after) {
            for index in 0..old.mesh.positions.len() {
                let base = old.mesh.base_positions[index];
                if base.x == 0.0 && base.y == 0.0 {
                    assert_eq!(new.mesh.positions[index], DVec3::new(0.0, 0.0, 40.0));
                    assert_eq!(new.mesh.alphas[index], 96.0);
                } else {
                    assert_eq!(new.mesh.positions[index], old.mesh.positions[index]);
                    assert_eq!(new.mesh.alphas[index], old.mesh.alphas[index]);
                }
            }
        }
        let repeat = super::plan(&document, &selected, true).unwrap();
        for (face, info) in repeat.edits {
            assert_eq!(
                side(&document, face).unwrap().displacement.as_ref(),
                Some(&info)
            );
        }
        document.undo();
        assert_eq!(document.vmf(), &source);
    }
}
