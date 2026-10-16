use std::collections::BTreeMap;

use source_formats::vmf::Vmf;

use super::{DisplacementMesh, lod, neighbors::SurfaceGrid};
use crate::{BrushGeometry, surface::FaceId};

#[derive(Default)]
pub struct Prepared {
    pub meshes: BTreeMap<FaceId, DisplacementMesh>,
    pub issues: BTreeMap<FaceId, String>,
}

impl Prepared {
    pub fn get(&self, face: &FaceId) -> Option<&DisplacementMesh> {
        if self.issues.contains_key(face) {
            return None;
        }
        self.meshes.get(face)
    }
}

pub fn prepare(vmf: &Vmf) -> Prepared {
    let mut prepared = Prepared::default();
    let mut world = Vec::new();
    let solids = vmf.world.solids.iter().map(|solid| (solid, true)).chain(
        vmf.entities
            .iter()
            .filter(|entity| !entity.hidden)
            .flat_map(|entity| {
                entity
                    .solids
                    .iter()
                    .map(move |solid| (solid, entity.classname() == Some("func_detail")))
            }),
    );
    for (solid, sewn) in solids.filter(|(solid, _)| {
        !solid.hidden && solid.sides.iter().any(|side| side.displacement.is_some())
    }) {
        let geometry = BrushGeometry::from_solid(solid);
        for side in solid
            .sides
            .iter()
            .filter(|side| side.displacement.is_some())
        {
            let id = FaceId {
                solid: solid.id,
                side: side.id,
            };
            let mesh = geometry
                .as_ref()
                .map_err(|error| error.to_string())
                .and_then(|geometry| {
                    geometry
                        .faces
                        .iter()
                        .find(|face| solid.sides[face.side].id == side.id)
                        .ok_or_else(|| "Terrain has no brush surface".to_owned())
                })
                .and_then(|face| {
                    DisplacementMesh::build(face, side.displacement.as_ref().unwrap())
                        .map_err(|error| error.to_string())
                });
            match mesh {
                Ok(mesh) => {
                    prepared.meshes.insert(id, mesh);
                    if sewn {
                        world.push(id);
                    }
                }
                Err(error) => {
                    prepared.issues.insert(id, error);
                }
            }
        }
    }
    let grids = world
        .iter()
        .map(|id| {
            let mesh = &prepared.meshes[id];
            let size = mesh.size;
            SurfaceGrid {
                power: (size - 1).ilog2(),
                corners: [0, (size - 1) * size, size * size - 1, size - 1]
                    .map(|index| mesh.base_positions[index].as_vec3().as_dvec3()),
                positions: &mesh.positions,
            }
        })
        .collect::<Vec<_>>();
    let patches = super::neighbors::find_spans(&grids, &mut |_| Ok(()))
        .and_then(|links| lod::prepare(&grids, &links.pairs, &mut |_| Ok(())));
    match patches {
        Ok((patches, _)) => {
            for (id, patch) in world.into_iter().zip(patches) {
                if let Some(patch) = patch {
                    prepared.meshes.get_mut(&id).unwrap().positions = patch.positions;
                }
            }
        }
        Err(error) => {
            for id in world {
                prepared.issues.insert(id, error.clone());
            }
        }
    }
    prepared
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Document, ObjectId, displacement::edit, query, region};
    use glam::DVec3;

    fn curved(half: bool, corner: u32) -> (Document, Vec<FaceId>) {
        let mut document = Document::empty();
        let mut patches = vec![(0.0, 0.0, 128.0, 2)];
        if half {
            patches.extend([(128.0, 0.0, 64.0, 3), (128.0, 64.0, 128.0, 4)]);
        } else {
            patches.push((128.0, 0.0, 128.0, 4));
        }
        let mut faces = Vec::new();
        for (x, y, end_y, power) in patches {
            let ObjectId::Solid(solid) = document
                .edit("terrain brush", |tx| {
                    crate::edit::create_block(
                        tx,
                        DVec3::new(x, y, 0.0),
                        DVec3::new(x + 128.0, end_y, 16.0),
                        "test/terrain",
                    )
                })
                .unwrap()
            else {
                unreachable!()
            };
            let side = document
                .solid(solid)
                .unwrap()
                .sides
                .iter()
                .find(|side| crate::Plane::from_points(side.plane.0).unwrap().normal == DVec3::Z)
                .unwrap()
                .id;
            let face = FaceId { solid, side };
            document
                .edit("terrain", |tx| edit::create(tx, &[face], power, corner))
                .unwrap();
            faces.push(face);
        }
        let mut vmf = document.vmf().clone();
        for solid in &mut vmf.world.solids {
            let geometry = BrushGeometry::from_solid(solid).unwrap();
            for face in &geometry.faces {
                let Some(info) = &mut solid.sides[face.side].displacement else {
                    continue;
                };
                let mesh = DisplacementMesh::build(face, info).unwrap();
                info.normals = Some(vec![vec![DVec3::Z; mesh.size]; mesh.size]);
                info.distances = Some(
                    mesh.positions
                        .chunks(mesh.size)
                        .map(|row| {
                            row.iter()
                                .map(|p| p.y * p.y / 512.0 + (p.x - 128.0).powi(2) / 1024.0)
                                .collect()
                        })
                        .collect(),
                );
            }
        }
        (Document::new(vmf), faces)
    }

    #[test]
    fn curved_full_and_half_seams_follow_coarse_chords_without_reauthoring() {
        for half in [false, true] {
            for corner in 0..4 {
                let (document, faces) = curved(half, corner);
                let before = document.vmf().write(&Default::default());
                let raw = edit::inspect(&document, &faces).unwrap();
                let surfaces = prepare(document.vmf());
                assert!(surfaces.issues.is_empty(), "{:?}", surfaces.issues);
                let mut moved = 0;
                for grid in &raw {
                    let mesh = surfaces.get(&grid.face).unwrap();
                    assert_eq!(mesh.base_positions, grid.mesh.base_positions);
                    assert_eq!(mesh.alphas, grid.mesh.alphas);
                    assert_eq!(mesh.size, grid.mesh.size);
                    for (point, source) in mesh.positions.iter().zip(&grid.mesh.positions) {
                        moved += usize::from(point.distance(*source) > 1e-6);
                        if point.x == 128.0 {
                            let low = (point.y / 32.0).floor() * 32.0;
                            let t = (point.y - low) / 32.0;
                            let height =
                                16.0 + (low * low * (1.0 - t) + (low + 32.0).powi(2) * t) / 512.0;
                            assert!((point.z - height).abs() < 1e-9, "{half} {corner}: {point}");
                        }
                    }
                }
                assert!(moved > 0);
                assert_eq!(document.vmf().write(&Default::default()), before);
            }
        }
    }

    #[test]
    fn direct_indexed_and_volume_queries_use_the_displayed_curved_seam() {
        for half in [false, true] {
            let (document, faces) = curved(half, 1);
            let fine = faces[1];
            let ray = query::Ray {
                origin: DVec3::new(128.0, 8.0, 128.0),
                direction: DVec3::NEG_Z,
            };
            let direct = query::pick_face(&document, &ray, |solid| solid.id == fine.solid).unwrap();
            let indexed =
                crate::picking::Picker::new(&document, |solid| solid.id == fine.solid, false)
                    .pick(&ray)
                    .unwrap();
            assert_eq!(direct.solid, Some(fine.solid));
            assert_eq!(direct.side, Some(fine.side));
            assert_eq!(direct.point, DVec3::new(128.0, 8.0, 16.5));
            assert_eq!(direct, indexed);
            let selected = region::Region::from_bounds(
                DVec3::new(127.99, 7.99, 16.49),
                DVec3::new(128.01, 8.01, 16.51),
            )
            .unwrap()
            .select(&document, region::Match::Intersecting, true, |solid| {
                solid.id == fine.solid
            });
            assert_eq!(selected.faces, vec![fine]);
        }
    }

    #[test]
    fn displayed_grid_triangles_cover_the_same_surface_as_native_lod_triangles() {
        for half in [false, true] {
            let (document, faces) = curved(half, 3);
            let raw = edit::inspect(&document, &faces).unwrap();
            let grids = raw
                .iter()
                .map(|grid| {
                    let mesh = &grid.mesh;
                    let size = mesh.size;
                    SurfaceGrid {
                        power: grid.power,
                        corners: [0, (size - 1) * size, size * size - 1, size - 1]
                            .map(|index| mesh.base_positions[index]),
                        positions: &mesh.positions,
                    }
                })
                .collect::<Vec<_>>();
            let links = super::super::neighbors::find_spans(&grids, &mut |_| Ok(())).unwrap();
            let (patches, _) = lod::prepare(&grids, &links.pairs, &mut |_| Ok(())).unwrap();
            let surfaces = prepare(document.vmf());
            for (grid, patch) in raw.iter().zip(patches) {
                let Some(patch) = patch else { continue };
                let mesh = surfaces.get(&grid.face).unwrap();
                for triangle in mesh.triangles() {
                    let point = triangle
                        .map(|index| mesh.positions[index])
                        .into_iter()
                        .sum::<DVec3>()
                        / 3.0;
                    let ray = query::Ray {
                        origin: point + DVec3::Z * 256.0,
                        direction: DVec3::NEG_Z,
                    };
                    let distance = patch
                        .indices
                        .iter()
                        .filter_map(|triangle| {
                            query::ray_triangle(&ray, triangle.map(|index| patch.positions[index]))
                        })
                        .min_by(f64::total_cmp)
                        .unwrap();
                    assert!((distance - 256.0).abs() < 1e-8);
                }
            }
        }
    }
}
