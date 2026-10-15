use map_core::displacement::{
    neighbors::{self, SurfaceGrid},
    smoothing,
};

use super::*;

#[derive(Default)]
pub(super) struct Prepared {
    pub meshes: HashMap<FaceId, Displacement>,
    pub issues: Vec<SceneIssue>,
    pub geometries: HashMap<u32, Result<BrushGeometry, String>>,
    pub unshaded: HashMap<FaceId, DisplacementMesh>,
}

impl Prepared {
    fn issue(&mut self, face: FaceId, message: String) {
        self.issues.push(SceneIssue { face, message });
    }
}

pub(super) fn prepare(vmf: &Vmf) -> Prepared {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vmf {
        let mut document = map_core::Document::empty();
        for x in [0.0, 64.0] {
            let ObjectId::Solid(solid) = document
                .edit("terrain brush", |tx| {
                    map_core::edit::create_block(
                        tx,
                        DVec3::new(x, 0.0, 0.0),
                        DVec3::new(x + 64.0, 64.0, 16.0),
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
                .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal == DVec3::Z)
                .unwrap()
                .id;
            document
                .edit("terrain", |tx| {
                    map_core::displacement::edit::create(tx, &[FaceId { solid, side }], 2, 0)
                })
                .unwrap();
        }
        document.vmf().clone()
    }

    #[test]
    fn bad_terrain_is_reported_and_outlined_without_replacement_shading() {
        let fixture = fixture();
        let fs = GameFs {
            game_dir: Default::default(),
            base_dir: Default::default(),
            mounts: Vec::new(),
        };
        for case in ["smooth", "unpaired", "nonfinite"] {
            let mut vmf = fixture.clone();
            let solid = &mut vmf.world.solids[0];
            let side = solid
                .sides
                .iter_mut()
                .find(|side| side.displacement.is_some())
                .unwrap();
            let face = FaceId {
                solid: solid.id,
                side: side.id,
            };
            let info = side.displacement.as_mut().unwrap();
            match case {
                "smooth" => {
                    info.extra_properties.set("smooth", "broken");
                    info.distances.as_mut().unwrap()[2][2] = 32.0;
                }
                "unpaired" => {
                    info.normals = None;
                }
                "nonfinite" => {
                    info.offsets = Some(vec![vec![DVec3::NAN; 5]; 5]);
                }
                _ => unreachable!(),
            }
            let mesh =
                SceneMesh::build(&vmf, &fs, &mut MaterialLibrary::default(), &HashSet::new());
            assert_eq!(mesh.issues.len(), 1, "{case}");
            assert_eq!(mesh.issues[0].face, face);
            assert_eq!(mesh.stats.displacements, 1);
            if case == "smooth" {
                assert!(mesh.lines.iter().any(|line| line.position[2] == 48.0));
            } else {
                assert_eq!(mesh.lines.len(), 8);
            }
            assert!(
                mesh.issue_summary()
                    .contains(&format!("brush {}", face.solid))
            );
            assert!(
                mesh.vertices
                    .iter()
                    .all(|vertex| vertex.normal == [0.0, 0.0, 1.0]
                        && vertex.position.iter().all(|value| value.is_finite()))
            );
            vmf.world.solids[0].hidden = true;
            assert!(prepare(&vmf).issues.is_empty());
        }
    }

    #[test]
    fn excessive_world_patches_do_not_fall_back_to_independent_normals() {
        let mut vmf = fixture();
        let template = vmf.world.solids.remove(0);
        vmf.world.solids.clear();
        for index in 0..=source_formats::bsp::MAX_DISPLACEMENTS {
            let mut solid = template.clone();
            solid.id = (index * 7 + 2) as u32;
            for (index, side) in solid.sides.iter_mut().enumerate() {
                side.id = solid.id + index as u32 + 1;
            }
            vmf.world.solids.push(solid);
        }
        let fs = GameFs {
            game_dir: Default::default(),
            base_dir: Default::default(),
            mounts: Vec::new(),
        };
        let mesh = SceneMesh::build(&vmf, &fs, &mut MaterialLibrary::default(), &HashSet::new());
        assert_eq!(
            mesh.issues.len(),
            source_formats::bsp::MAX_DISPLACEMENTS + 1
        );
        assert!(
            mesh.issues
                .iter()
                .all(|issue| issue.message.contains("native displacement limit"))
        );
        assert_eq!(mesh.stats.displacements, 0);
        assert!(mesh.vertices.is_empty());
        assert!(mesh.lines.len() > mesh.issues.len() * 8);
        vmf.world.solids.pop();
        let mesh = SceneMesh::build(&vmf, &fs, &mut MaterialLibrary::default(), &HashSet::new());
        assert!(mesh.issues.is_empty());
        assert_eq!(
            mesh.stats.displacements,
            source_formats::bsp::MAX_DISPLACEMENTS
        );
    }
}
