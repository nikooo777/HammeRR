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
