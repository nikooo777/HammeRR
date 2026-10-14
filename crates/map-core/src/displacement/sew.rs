use std::collections::BTreeMap;

use glam::DVec3;
use source_formats::vmf::DispInfo;

use super::{
    edit::{TerrainGrid, inspect},
    neighbors::{SurfaceGrid, edge_indices, find},
};
use crate::{
    Document, EditError,
    surface::{FaceId, side},
};

pub struct Plan {
    pub edits: Vec<(FaceId, DispInfo)>,
    pub pairs: usize,
    pub vertices: usize,
}

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
            corners: corners(grid),
            positions: &grid.mesh.base_positions,
        })
        .collect::<Vec<_>>();
    let neighbors = find(&views, &mut |_| Ok(())).map_err(invalid)?;
    if neighbors.ambiguous > 0 || neighbors.unequal_power > 0 {
        return Err(invalid(
            "Sewing needs unambiguous matching full edges with equal grid powers",
        ));
    }
    if neighbors.pairs.is_empty() {
        return Err(invalid(
            "No matching equal-power full terrain edges were found",
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
        let first = edge_indices(grids[pair.first].mesh.size, pair.first_edge).map_err(invalid)?;
        let second =
            edge_indices(grids[pair.second].mesh.size, pair.second_edge).map_err(invalid)?;
        for (a, b) in first.into_iter().zip(second.into_iter().rev()) {
            let a = root(&mut parents, starts[pair.first] + a);
            let b = root(&mut parents, starts[pair.second] + b);
            let (low, high) = if a < b { (a, b) } else { (b, a) };
            parents[high] = low;
        }
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
    let mut edits = Vec::with_capacity(grids.len());
    for (index, grid) in grids.iter().enumerate() {
        let mut info = side(document, grid.face)
            .unwrap()
            .displacement
            .clone()
            .unwrap();
        if info.subdivided
            || !info.extra.is_empty()
            || info.normals.is_some() != info.distances.is_some()
            || info.offset_normals.as_ref().is_some_and(|rows| {
                rows.len() != grid.mesh.size
                    || rows.iter().any(|row| {
                        row.len() != grid.mesh.size || row.iter().any(|p| *p != DVec3::ZERO)
                    })
            })
        {
            return Err(invalid(
                "Terrain subdivision, opaque blocks or offset-normal caches need remapping before sewing",
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
            let root = root(&mut parents, starts[index] + vertex);
            let sum = &sums[&root];
            if sum.count < 2 {
                continue;
            }
            let row = vertex / grid.mesh.size;
            let col = vertex % grid.mesh.size;
            let point = if sum.same_point {
                sum.first_point
            } else {
                sum.point / sum.count as f64
            };
            let change = point - grid.mesh.positions[vertex];
            let updated = offsets[row][col] + change;
            if !updated.is_finite() {
                return Err(invalid("Sewn terrain offsets must be finite"));
            }
            offsets[row][col] = updated;
            moved |= change != DVec3::ZERO;
            if blend {
                let value = if sum.same_alpha {
                    sum.first_alpha
                } else {
                    sum.alpha / sum.count as f64
                };
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
        vertices,
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
    fn incompatible_powers_and_missing_neighbors_reject_without_source_changes() {
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
        assert!(plan(&doc, &faces, false).is_err());
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
}
