use std::collections::BTreeSet;

use source_formats::vmf::{DispInfo, Properties};

use crate::surface::FaceId;
use crate::{BrushGeometry, Document, EditError, ObjectId, Transaction};
use glam::DVec3;

struct Target {
    face: FaceId,
    side: usize,
    start: DVec3,
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
            start: face.vertices[0],
            normal: face.plane.normal,
        });
    }
    Ok(planned)
}

pub fn validate(document: &Document, faces: &[FaceId]) -> Result<(), EditError> {
    targets(document, faces).map(|_| ())
}

pub fn create(tx: &mut Transaction<'_>, faces: &[FaceId], power: u32) -> Result<usize, EditError> {
    if !(2..=4).contains(&power) {
        return Err(EditError::Invalid("Terrain power must be 2, 3 or 4".into()));
    }
    let planned = targets(tx.document(), faces)?;
    let count = planned.len();
    let size = (1 << power) + 1;
    for target in planned {
        let info = DispInfo {
            power,
            start_position: target.start,
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
                .edit("terrain", |tx| create(tx, &[target], power))
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
                    .edit("bad terrain", |tx| create(tx, &faces, power))
                    .is_err()
            );
            assert_eq!(*document.vmf(), before);
        }
        document
            .edit("terrain", |tx| create(tx, &[target], 2))
            .unwrap();
        let existing = document.vmf().clone();
        assert!(
            document
                .edit("replace", |tx| create(tx, &[target], 4))
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
                .edit("entity terrain", |tx| create(tx, &[target], 2))
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
                .edit("triangle terrain", |tx| create(tx, &[target], 2))
                .is_err()
        );
        assert_eq!(*document.vmf(), before);
    }
}
