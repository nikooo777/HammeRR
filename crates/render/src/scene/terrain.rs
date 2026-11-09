use map_core::displacement::{
    native,
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

    pub(super) fn apply_normal_overrides(
        &mut self,
        vmf: &Vmf,
        overrides: &BTreeMap<FaceId, TerrainNormalOverride>,
    ) -> Result<(), String> {
        if overrides.len() > source_formats::bsp::MAX_DISPLACEMENTS {
            return Err("Terrain normal overrides exceed the native face limit".into());
        }
        let mut vertices = 0usize;
        let mut triangles = 0usize;
        for replacement in overrides.values() {
            if replacement.positions.len() != replacement.normals.len() {
                return Err(
                    "Terrain normal overrides require complete position and normal arrays".into(),
                );
            }
            vertices = vertices
                .checked_add(replacement.positions.len())
                .ok_or("Terrain normal override vertex count overflow")?;
            triangles = triangles
                .checked_add(replacement.triangles.len())
                .ok_or("Terrain normal override triangle count overflow")?;
            if vertices > crate::references::MAX_VERTICES
                || triangles > 2 * crate::references::MAX_VERTICES
            {
                return Err(
                    "Terrain normal overrides exceed their vertex or triangle limit".into(),
                );
            }
        }
        if overrides.is_empty() {
            return Ok(());
        }
        vmf.validate_ids().map_err(|error| {
            format!("Terrain normal overrides require unique source IDs: {error}")
        })?;
        let eligible = native::visible_terrain_sources(vmf)
            .filter(|(_, sewn)| *sewn)
            .flat_map(|(solid, _)| {
                solid
                    .sides
                    .iter()
                    .filter(|side| side.displacement.is_some())
                    .map(move |side| FaceId {
                        solid: solid.id,
                        side: side.id,
                    })
            })
            .collect::<BTreeSet<_>>();
        for (face, replacement) in overrides {
            let invalid = |reason: &str| {
                format!(
                    "solid:{} side:{}: terrain normal override {reason}",
                    face.solid, face.side
                )
            };
            if !eligible.contains(face) {
                return Err(invalid(
                    "requires source-visible world or func_detail terrain",
                ));
            }
            let prepared = self
                .meshes
                .get(face)
                .ok_or_else(|| invalid("has no complete prepared shaded surface"))?;
            if prepared.normals.len() != prepared.mesh.positions.len()
                || replacement.positions.len() != prepared.mesh.positions.len()
            {
                return Err(invalid("does not match the prepared vertex count"));
            }
            if prepared
                .mesh
                .positions
                .iter()
                .zip(&replacement.positions)
                .any(|(actual, expected)| {
                    !expected.iter().all(|value| value.is_finite())
                        || actual.as_vec3().to_array().map(f32::to_bits)
                            != expected.map(f32::to_bits)
                })
                || !replacement
                    .triangles
                    .iter()
                    .copied()
                    .eq(prepared.mesh.triangles())
            {
                return Err(invalid(
                    "does not match the prepared positions and triangles",
                ));
            }
            if replacement
                .normals
                .iter()
                .any(|normal| !valid_normal(Vec3::from_array(normal.map(|value| value as f32))))
            {
                return Err(invalid("requires finite nonzero packed normals"));
            }
        }
        for (face, replacement) in overrides {
            self.meshes
                .get_mut(face)
                .ok_or("Validated terrain normal override lost its prepared surface")?
                .normals
                .clone_from(&replacement.normals);
        }
        Ok(())
    }
}

fn valid_normal(normal: Vec3) -> bool {
    let squared = normal.length_squared();
    normal.is_finite() && squared.is_finite() && squared >= 1e-12
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
            let Some(mesh) = surfaces.meshes.remove(&id) else {
                prepared.issue(id, "Terrain has no prepared native surface".into());
                prepared.unshaded.insert(id, mesh);
                continue;
            };
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
            .map(|index| native::stored_vertex(mesh.base_positions[index])),
            positions: &mesh.positions,
        })
        .collect::<Vec<_>>();
    let angles = world.iter().map(|(_, _, angle)| *angle).collect::<Vec<_>>();
    let faces = world.iter().map(|(face, _, _)| *face).collect::<Vec<_>>();
    let normals = surfaces
        .neighborhood
        .links_for(&faces, &mut |_| Ok(()))
        .and_then(|links| {
            neighbors::vertex_normals_with_corners(
                &grids,
                &links.pairs,
                &links.corners,
                &angles,
                &mut |_| Ok(()),
            )
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

pub(crate) fn receiver_error(
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
    if normals.iter().any(|normal| !valid_normal(*normal)) {
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
