use std::collections::BTreeMap;

use source_formats::{bsp::DisplacementCorner, vmf::Vmf};

use super::{
    DisplacementMesh, lod, native,
    neighbors::{NeighborPair, SurfaceGrid},
};
use crate::{BrushGeometry, surface::FaceId};

#[derive(Debug, Default)]
pub struct NeighborGraph {
    faces: Vec<FaceId>,
    pairs: Vec<NeighborPair>,
    corners: Vec<[DisplacementCorner; 4]>,
}

pub struct NeighborLinks {
    pub pairs: Vec<NeighborPair>,
    pub corners: Vec<[DisplacementCorner; 4]>,
}

impl NeighborGraph {
    pub fn new(
        faces: Vec<FaceId>,
        pairs: Vec<NeighborPair>,
        corners: Vec<[DisplacementCorner; 4]>,
    ) -> Result<Self, String> {
        let count = faces.len();
        if count > source_formats::bsp::MAX_DISPLACEMENTS
            || corners.len() != count
            || faces
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != count
        {
            return Err(
                "Terrain neighbor graph requires unique bounded source faces and valid pairs"
                    .into(),
            );
        }
        super::neighbors::corner_links(&pairs, &corners, &mut |_| Ok(()))?;
        Ok(Self {
            faces,
            pairs,
            corners,
        })
    }

    pub fn links_for(
        &self,
        faces: &[FaceId],
        consume: &mut impl FnMut(usize) -> Result<(), String>,
    ) -> Result<NeighborLinks, String> {
        if faces.len() > self.faces.len() {
            return Err("Terrain normals lost their original neighbor graph faces".into());
        }
        consume(self.faces.len() * 5 + self.pairs.len() + faces.len())?;
        let indices = faces
            .iter()
            .enumerate()
            .map(|(index, face)| (*face, index))
            .collect::<BTreeMap<_, _>>();
        let remap = self
            .faces
            .iter()
            .map(|face| indices.get(face).copied())
            .collect::<Vec<_>>();
        if indices.len() != faces.len()
            || remap.iter().filter(|index| index.is_some()).count() != faces.len()
        {
            return Err("Terrain normals lost their original neighbor graph faces".into());
        }
        let pairs = self
            .pairs
            .iter()
            .filter_map(|pair| {
                Some(NeighborPair {
                    first: remap[pair.first]?,
                    second: remap[pair.second]?,
                    ..*pair
                })
            })
            .collect::<Vec<_>>();
        let mut corners = vec![[DisplacementCorner::default(); 4]; faces.len()];
        for (source, records) in self.corners.iter().enumerate() {
            let Some(index) = remap[source] else {
                continue;
            };
            for (corner, record) in records.iter().enumerate() {
                let target = &mut corners[index][corner];
                target.padding = record.padding;
                for &other in &record.neighbors[..usize::from(record.count)] {
                    consume(1)?;
                    if let Some(other) = remap[usize::from(other)] {
                        target.neighbors[usize::from(target.count)] = other as u16;
                        target.count += 1;
                    }
                }
            }
        }
        super::neighbors::corner_links(&pairs, &corners, consume)?;
        Ok(NeighborLinks { pairs, corners })
    }
}

#[derive(Default)]
pub struct Prepared {
    pub meshes: BTreeMap<FaceId, DisplacementMesh>,
    pub issues: BTreeMap<FaceId, String>,
    pub neighborhood: NeighborGraph,
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
                .and_then(|face| native::stored_mesh(face, side.displacement.as_ref().unwrap()));
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
                    .map(|index| native::stored_vertex(mesh.base_positions[index])),
                positions: &mesh.positions,
            }
        })
        .collect::<Vec<_>>();
    let patches = super::neighbors::native_neighborhood(&grids, &mut |_| Ok(())).and_then(
        |(links, native)| {
            let (patches, _) = lod::prepare(&grids, &links.pairs, &mut |_| Ok(()))?;
            let neighborhood = NeighborGraph::new(world.clone(), links.pairs, native.corners)?;
            Ok((patches, neighborhood))
        },
    );
    match patches {
        Ok((patches, neighborhood)) => {
            prepared.neighborhood = neighborhood;
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
