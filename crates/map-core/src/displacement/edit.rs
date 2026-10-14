use std::collections::BTreeSet;

use source_formats::vmf::{DispInfo, Properties};

use crate::surface::FaceId;
use crate::{BrushGeometry, Document, EditError, ObjectId, Transaction};
use glam::DVec3;

struct Target {
    face: FaceId,
    side: usize,
    corners: [DVec3; 4],
    normal: DVec3,
}

fn targets(document: &Document, faces: &[FaceId]) -> Result<Vec<Target>, EditError> {
    let faces = faces.iter().copied().collect::<BTreeSet<_>>();
    if faces.is_empty() || faces.len() > 16 {
        return Err(EditError::Invalid(
            "Select 1–16 world quad faces for terrain".into(),
        ));
    }
    let mut solids = BTreeSet::new();
    let mut planned = Vec::with_capacity(faces.len());
    for target in faces {
        if !solids.insert(target.solid) {
            return Err(EditError::Invalid(
                "Create terrain on one face per parent brush".into(),
            ));
        }
        if document.owner_of_solid(target.solid) != Some(ObjectId::Solid(target.solid)) {
            return Err(EditError::Invalid(
                "Terrain creation requires world brushes".into(),
            ));
        }
        let solid = document
            .solid(target.solid)
            .ok_or_else(|| EditError::Invalid("Terrain parent brush is missing".into()))?;
        if solid.sides.iter().any(|side| side.displacement.is_some()) {
            return Err(EditError::Invalid(
                "This brush already has terrain; existing terrain is preserved".into(),
            ));
        }
        let matches = solid
            .sides
            .iter()
            .enumerate()
            .filter(|(_, side)| side.id == target.side)
            .collect::<Vec<_>>();
        let [(side_index, _)] = matches.as_slice() else {
            return Err(EditError::Invalid(
                "Terrain face is missing or ambiguous".into(),
            ));
        };
        let geometry = BrushGeometry::from_solid(solid)
            .map_err(|error| EditError::Invalid(error.to_string()))?;
        let face = geometry
            .faces
            .iter()
            .find(|face| face.side == *side_index)
            .ok_or_else(|| EditError::Invalid("Terrain needs an effective quad face".into()))?;
        if face.vertices.len() != 4 {
            return Err(EditError::Invalid(
                "Terrain creation requires four-corner faces".into(),
            ));
        }
        planned.push(Target {
            face: target,
            side: *side_index,
            corners: face.vertices.as_slice().try_into().unwrap(),
            normal: face.plane.normal,
        });
    }
    Ok(planned)
}

pub struct CreationFace {
    pub face: FaceId,
    pub corners: [DVec3; 4],
}

pub fn creation_faces(
    document: &Document,
    faces: &[FaceId],
) -> Result<Vec<CreationFace>, EditError> {
    Ok(targets(document, faces)?
        .into_iter()
        .map(|target| CreationFace {
            face: target.face,
            corners: target.corners,
        })
        .collect())
}

pub fn validate(document: &Document, faces: &[FaceId]) -> Result<(), EditError> {
    targets(document, faces).map(|_| ())
}

pub fn create(
    tx: &mut Transaction<'_>,
    faces: &[FaceId],
    power: u32,
    start_corner: u32,
) -> Result<usize, EditError> {
    if !(2..=4).contains(&power) {
        return Err(EditError::Invalid("Terrain power must be 2, 3 or 4".into()));
    }
    if start_corner > 3 {
        return Err(EditError::Invalid(
            "Terrain start corner must be 0, 1, 2 or 3".into(),
        ));
    }
    let planned = targets(tx.document(), faces)?;
    let count = planned.len();
    let size = (1 << power) + 1;
    for target in planned {
        let info = DispInfo {
            power,
            start_position: target.corners[start_corner as usize],
            flags: Some(0),
            elevation: 0.0,
            subdivided: false,
            normals: Some(vec![vec![target.normal; size]; size]),
            distances: Some(vec![vec![0.0; size]; size]),
            offsets: None,
            offset_normals: None,
            alphas: Some(vec![vec![0.0; size]; size]),
            triangle_tags: None,
            allowed_verts: None,
            extra_properties: Properties::default(),
            extra: Vec::new(),
        };
        tx.solid_mut(target.face.solid)?.sides[target.side].displacement = Some(info);
    }
    Ok(count)
}

fn removal_targets(
    document: &Document,
    faces: &[FaceId],
) -> Result<Vec<(FaceId, usize)>, EditError> {
    let faces = faces.iter().copied().collect::<BTreeSet<_>>();
    if faces.is_empty() || faces.len() > 16 {
        return Err(EditError::Invalid(
            "Select 1–16 terrain faces to remove".into(),
        ));
    }
    let mut planned = Vec::with_capacity(faces.len());
    for target in faces {
        let solid = document
            .solid(target.solid)
            .ok_or_else(|| EditError::Invalid("Terrain parent brush is missing".into()))?;
        let matches = solid
            .sides
            .iter()
            .enumerate()
            .filter(|(_, side)| side.id == target.side)
            .collect::<Vec<_>>();
        let [(index, side)] = matches.as_slice() else {
            return Err(EditError::Invalid(
                "Terrain face is missing or ambiguous".into(),
            ));
        };
        if side.displacement.is_none() {
            return Err(EditError::Invalid(
                "Every selected face must have terrain".into(),
            ));
        }
        planned.push((target, *index));
    }
    Ok(planned)
}

pub struct TerrainGrid {
    pub face: FaceId,
    pub power: u32,
    pub flags: u32,
    pub mesh: super::DisplacementMesh,
}

pub fn inspect(document: &Document, faces: &[FaceId]) -> Result<Vec<TerrainGrid>, EditError> {
    removal_targets(document, faces)?
        .into_iter()
        .map(|(target, index)| {
            let solid = document.solid(target.solid).unwrap();
            let geometry = BrushGeometry::from_solid(solid)
                .map_err(|error| EditError::Invalid(error.to_string()))?;
            let face = geometry
                .faces
                .iter()
                .find(|face| face.side == index)
                .ok_or_else(|| EditError::Invalid("Terrain needs an effective quad face".into()))?;
            let info = solid.sides[index].displacement.as_ref().unwrap();
            let mesh = super::DisplacementMesh::build(face, info)
                .map_err(|error| EditError::Invalid(error.to_string()))?;
            if mesh.positions.iter().any(|p| !p.is_finite())
                || mesh.alphas.iter().any(|a| !a.is_finite())
            {
                return Err(EditError::Invalid(
                    "Terrain grid contains nonfinite positions or blend values".into(),
                ));
            }
            Ok(TerrainGrid {
                face: target,
                power: info.power,
                flags: info.flags.unwrap_or(0),
                mesh,
            })
        })
        .collect()
}

pub fn validate_removal(document: &Document, faces: &[FaceId]) -> Result<(), EditError> {
    removal_targets(document, faces).map(|_| ())
}

pub fn remove(tx: &mut Transaction<'_>, faces: &[FaceId]) -> Result<usize, EditError> {
    let planned = removal_targets(tx.document(), faces)?;
    let count = planned.len();
    for (target, index) in planned {
        tx.solid_mut(target.solid)?.sides[index].displacement = None;
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edit;

    fn fixture() -> (Document, FaceId) {
        let mut document = Document::empty();
        let ObjectId::Solid(solid) = document
            .edit("block", |tx| {
                edit::create_block(tx, DVec3::ZERO, DVec3::splat(64.0), "brick/test")
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
            .find(|side| crate::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
            .unwrap()
            .id;
        (document, FaceId { solid, side })
    }

    #[test]
    fn flat_creation_preserves_side_metadata_and_round_trips_with_one_undo() {
        for power in [2, 3, 4] {
            let (mut document, target) = fixture();
            let before = document.vmf().clone();
            let count = document
                .edit("terrain", |tx| create(tx, &[target], power, 0))
                .unwrap();
            assert_eq!(count, 1);
            let after = document.vmf().clone();
            let solid = document.solid(target.solid).unwrap();
            let side = solid
                .sides
                .iter()
                .find(|side| side.id == target.side)
                .unwrap();
            let mut unchanged = side.clone();
            unchanged.displacement = None;
            assert_eq!(
                unchanged,
                *before.world.solids[0]
                    .sides
                    .iter()
                    .find(|side| side.id == target.side)
                    .unwrap()
            );
            let geometry = BrushGeometry::from_solid(solid).unwrap();
            let face = geometry
                .faces
                .iter()
                .find(|face| solid.sides[face.side].id == target.side)
                .unwrap();
            let mesh =
                super::super::DisplacementMesh::build(face, side.displacement.as_ref().unwrap())
                    .unwrap();
            assert_eq!(mesh.size, (1 << power) + 1);
            assert_eq!(mesh.positions, mesh.base_positions);
            assert!(mesh.alphas.iter().all(|&value| value == 0.0));
            let text = after.write(&Default::default());
            assert_eq!(
                source_formats::vmf::Vmf::parse(&text)
                    .unwrap()
                    .write(&Default::default()),
                text
            );
            document
                .edit("remove terrain", |tx| remove(tx, &[target]))
                .unwrap();
            assert_eq!(*document.vmf(), before);
            document.undo().unwrap();
            assert_eq!(*document.vmf(), after);
            document.undo().unwrap();
            assert_eq!(*document.vmf(), before);
            document.redo().unwrap();
            assert_eq!(*document.vmf(), after);
        }
    }

    #[test]
    fn invalid_mixed_targets_powers_existing_terrain_and_entity_owners_are_atomic() {
        let (mut document, target) = fixture();
        let before = document.vmf().clone();
        for (faces, power) in [
            (vec![target], 5),
            (
                vec![
                    target,
                    FaceId {
                        solid: target.solid,
                        side: 999,
                    },
                ],
                2,
            ),
            (Vec::new(), 2),
        ] {
            assert!(
                document
                    .edit("bad terrain", |tx| create(tx, &faces, power, 0))
                    .is_err()
            );
            assert_eq!(*document.vmf(), before);
        }
        document
            .edit("terrain", |tx| create(tx, &[target], 2, 0))
            .unwrap();
        let existing = document.vmf().clone();
        assert!(
            document
                .edit("replace", |tx| create(tx, &[target], 4, 0))
                .is_err()
        );
        assert_eq!(*document.vmf(), existing);
        document.undo().unwrap();
        document
            .edit("entity", |tx| {
                edit::tie_to_entity(tx, &[target.solid], "func_brush", &[])
            })
            .unwrap();
        let tied = document.vmf().clone();
        assert!(
            document
                .edit("entity terrain", |tx| create(tx, &[target], 2, 0))
                .is_err()
        );
        assert_eq!(*document.vmf(), tied);
    }
    #[test]
    fn triangular_primitive_faces_are_rejected_before_creation() {
        let mut document = Document::empty();
        let objects = document
            .edit("wedge", |tx| {
                crate::primitive::create(tx, &crate::primitive::Spec::default())
            })
            .unwrap();
        let ObjectId::Solid(solid) = objects[0] else {
            unreachable!()
        };
        let shape = document.solid(solid).unwrap();
        let geometry = BrushGeometry::from_solid(shape).unwrap();
        let triangle = geometry
            .faces
            .iter()
            .find(|face| face.vertices.len() == 3)
            .unwrap();
        let target = FaceId {
            solid,
            side: shape.sides[triangle.side].id,
        };
        let before = document.vmf().clone();
        assert!(
            document
                .edit("triangle terrain", |tx| create(tx, &[target], 2, 0))
                .is_err()
        );
        assert_eq!(*document.vmf(), before);
    }
    #[test]
    fn removal_preserves_imported_rows_and_extras_in_undo_and_rejects_mixed_targets() {
        let (mut document, target) = fixture();
        document
            .edit("terrain", |tx| create(tx, &[target], 4, 0))
            .unwrap();
        document
            .edit("imported data", |tx| {
                let solid = tx.solid_mut(target.solid)?;
                let side = solid
                    .sides
                    .iter_mut()
                    .find(|side| side.id == target.side)
                    .unwrap();
                let info = side.displacement.as_mut().unwrap();
                info.distances.as_mut().unwrap()[0][0] = 17.0;
                info.alphas.as_mut().unwrap()[0][0] = 195.0;
                info.extra_properties.set("custom_grid_note", "preserve");
                Ok(())
            })
            .unwrap();
        let imported = document.vmf().clone();
        let ordinary = document
            .solid(target.solid)
            .unwrap()
            .sides
            .iter()
            .find(|side| side.displacement.is_none())
            .unwrap()
            .id;
        assert!(
            document
                .edit("bad removal", |tx| remove(
                    tx,
                    &[
                        target,
                        FaceId {
                            solid: target.solid,
                            side: ordinary
                        }
                    ]
                ))
                .is_err()
        );
        assert_eq!(*document.vmf(), imported);
        document.edit("remove", |tx| remove(tx, &[target])).unwrap();
        assert!(
            document
                .solid(target.solid)
                .unwrap()
                .sides
                .iter()
                .all(|side| side.displacement.is_none())
        );
        document.undo().unwrap();
        assert_eq!(*document.vmf(), imported);
    }
    #[test]
    fn creation_corners_preserve_flat_geometry_winding_and_undo_at_every_power() {
        for angle in [0.0, 0.31] {
            for power in [2, 3, 4] {
                for corner in 0..4 {
                    let (mut document, target) = fixture();
                    document
                        .edit("tilt", |tx| {
                            let rotation = glam::DQuat::from_rotation_y(angle);
                            for side in &mut tx.solid_mut(target.solid)?.sides {
                                for point in &mut side.plane.0 {
                                    *point = rotation * *point;
                                }
                            }
                            Ok(())
                        })
                        .unwrap();
                    let before = document.vmf().clone();
                    let corners = creation_faces(&document, &[target]).unwrap()[0].corners;
                    assert!(
                        document
                            .edit("invalid", |tx| create(tx, &[target], power, 4))
                            .is_err()
                    );
                    assert_eq!(*document.vmf(), before);
                    document
                        .edit("create", |tx| create(tx, &[target], power, corner))
                        .unwrap();
                    let solid = document.solid(target.solid).unwrap();
                    let geometry = BrushGeometry::from_solid(solid).unwrap();
                    let face = geometry
                        .faces
                        .iter()
                        .find(|face| solid.sides[face.side].id == target.side)
                        .unwrap();
                    let info = solid.sides[face.side].displacement.as_ref().unwrap();
                    let mesh = super::super::DisplacementMesh::build(face, info).unwrap();
                    let index = corner as usize;
                    assert_eq!(info.start_position, corners[index]);
                    assert_eq!(mesh.positions, mesh.base_positions);
                    assert_eq!(mesh.positions[0], corners[index]);
                    assert_eq!(
                        mesh.positions[mesh.size * (mesh.size - 1)],
                        corners[(index + 1) % 4]
                    );
                    assert_eq!(mesh.positions[mesh.size - 1], corners[(index + 3) % 4]);
                    assert!(mesh.triangles().all(|tri| {
                        (mesh.positions[tri[2]] - mesh.positions[tri[0]])
                            .cross(mesh.positions[tri[1]] - mesh.positions[tri[0]])
                            .dot(face.plane.normal)
                            > 0.0
                    }));
                    document.undo().unwrap();
                    assert_eq!(*document.vmf(), before);
                }
            }
        }
    }
}
