use map_core::displacement::{
    neighbors::{self, SurfaceGrid},
    smoothing,
};

use super::*;

#[derive(Default)]
pub(super) struct Prepared {
    pub meshes: HashMap<FaceId, Displacement>,
    pub issues: Vec<TerrainIssue>,
    pub geometries: HashMap<u32, Result<BrushGeometry, String>>,
    pub unshaded: HashMap<FaceId, DisplacementMesh>,
}

impl Prepared {
    fn issue(&mut self, face: FaceId, message: String) {
        self.issues.push(TerrainIssue { face, message });
    }
}

pub(super) fn prepare(vmf: &Vmf) -> Prepared {
    let mut prepared = Prepared::default();
    let mut surfaces = map_core::displacement::surface::prepare(vmf);
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
        let brush = match BrushGeometry::from_solid(solid) {
            Ok(brush) => brush,
            Err(error) => {
                for side in solid
                    .sides
                    .iter()
                    .filter(|side| side.displacement.is_some())
                {
                    prepared.issue(
                        FaceId {
                            solid: solid.id,
                            side: side.id,
                        },
                        error.to_string(),
                    );
                }
                prepared.geometries.insert(solid.id, Err(error.to_string()));
                continue;
            }
        };
        for face in &brush.faces {
            let side = &solid.sides[face.side];
            let Some(info) = &side.displacement else {
                continue;
            };
            let id = FaceId {
                solid: solid.id,
                side: side.id,
            };
            let mesh = match DisplacementMesh::build(face, info) {
                Ok(mesh) => mesh,
                Err(error) => {
                    prepared.issue(id, error.to_string());
                    continue;
                }
            };
            if let Some(error) = surfaces.issues.get(&id) {
                prepared.issue(id, error.clone());
                prepared.unshaded.insert(id, mesh);
                continue;
            }
            let mesh = surfaces.meshes.remove(&id).unwrap_or(mesh);
            let angle = match smoothing::angle(info) {
                Ok(angle) => angle,
                Err(error) => {
                    prepared.issue(id, error);
                    prepared.unshaded.insert(id, mesh);
                    continue;
                }
            };
            if sewn {
                world.push((id, mesh, angle));
            } else {
                let normals = source_formats::bsp::DisplacementInfo {
                    power: info.power as i32,
                    ..Default::default()
                }
                .vertex_normals(
                    &mesh
                        .positions
                        .iter()
                        .map(|point| point.to_array())
                        .collect::<Vec<_>>(),
                );
                match normals {
                    Ok(normals) => {
                        prepared.meshes.insert(id, Displacement { mesh, normals });
                    }
                    Err(error) => {
                        prepared.issue(id, error.to_string());
                        prepared.unshaded.insert(id, mesh);
                    }
                }
            }
        }
        prepared.geometries.insert(solid.id, Ok(brush));
    }
    let grids = world
        .iter()
        .map(|(_, mesh, _)| SurfaceGrid {
            power: (mesh.size - 1).ilog2(),
            corners: [
                0,
                (mesh.size - 1) * mesh.size,
                mesh.size * mesh.size - 1,
                mesh.size - 1,
            ]
            .map(|index| mesh.base_positions[index].as_vec3().as_dvec3()),
            positions: &mesh.positions,
        })
        .collect::<Vec<_>>();
    let angles = world.iter().map(|(_, _, angle)| *angle).collect::<Vec<_>>();
    let normals = neighbors::find_spans(&grids, &mut |_| Ok(())).and_then(|links| {
        neighbors::vertex_normals_with_smoothing(&grids, &links.pairs, &angles, &mut |_| Ok(()))
    });
    match normals {
        Ok(normals) => {
            for ((id, mesh, _), normals) in world.into_iter().zip(normals) {
                prepared.meshes.insert(id, Displacement { mesh, normals });
            }
        }
        Err(error) => {
            for (face, mesh, _) in world {
                prepared.issue(face, error.clone());
                prepared.unshaded.insert(face, mesh);
            }
        }
    }
    prepared
}

pub(super) fn lighting_error(
    mesh: &DisplacementMesh,
    normals: &[[f64; 3]],
) -> Option<&'static str> {
    if mesh.positions.iter().any(|point| {
        !point.is_finite() || point.abs().max_element() > map_core::checks::SOURCE_COORDINATE_LIMIT
    }) {
        return Some("Terrain authored-light receivers must fit finite Source compile coordinates");
    }
    if normals.len() != mesh.positions.len() {
        return Some("Terrain authored lighting needs complete prepared normals");
    }
    let normals = normals
        .iter()
        .map(|normal| Vec3::from_array(normal.map(|v| v as f32)))
        .collect::<Vec<_>>();
    if normals.iter().any(|normal| {
        let squared = normal.length_squared();
        !normal.is_finite() || !squared.is_finite() || squared < 1e-12
    }) {
        return Some("Terrain authored lighting needs finite nonzero packed normals");
    }
    for triangle in mesh.triangles() {
        if let Some(reason) = crate::lighting::triangle_error(
            triangle.map(|index| mesh.positions[index].as_vec3()),
            triangle.map(|index| normals[index]),
        ) {
            return Some(reason);
        }
    }
    None
}
