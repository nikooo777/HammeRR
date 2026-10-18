use super::*;

pub fn fixture() -> (Session, ObjectId) {
    let mut session = Session::new(Err("No assets needed for brush placement".into()));
    for (low, high) in [
        ([-256.0, -256.0, -16.0], [256.0, 256.0, 0.0]),
        ([-256.0, -256.0, 128.0], [256.0, 256.0, 144.0]),
        ([-256.0, 64.0, 0.0], [256.0, 80.0, 128.0]),
    ] {
        session
            .create_block(
                Actor::User,
                DVec3::from_array(low),
                DVec3::from_array(high),
                "brick/test",
                false,
            )
            .unwrap();
    }
    let selected = session
        .create_block(
            Actor::User,
            DVec3::new(-8.0, -8.0, 32.0),
            DVec3::new(8.0, 8.0, 48.0),
            "brick/test",
            false,
        )
        .unwrap();
    session
        .select(&[selected], crate::session::SelectMode::Replace)
        .unwrap();
    (session, selected)
}

pub fn terrain_target() -> (Session, ObjectId, map_core::surface::FaceId, Ray) {
    let mut session = Session::new(Err("No assets needed for terrain snapping".into()));
    let mut faces = Vec::new();
    for (x, power) in [(0.0, 2), (128.0, 3)] {
        let ObjectId::Solid(solid) = session
            .create_block(
                Actor::User,
                DVec3::new(x, 0.0, 0.0),
                DVec3::new(x + 128.0, 128.0, 16.0),
                "brick/test",
                false,
            )
            .unwrap()
        else {
            unreachable!()
        };
        let side = session
            .document()
            .solid(solid)
            .unwrap()
            .sides
            .iter()
            .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal == DVec3::Z)
            .unwrap()
            .id;
        let face = map_core::surface::FaceId { solid, side };
        session
            .create_terrain(Actor::User, &[face], power, 1)
            .unwrap();
        session
            .edit(Actor::User, "curve terrain", |tx| {
                let brush = tx.document().solid(solid).unwrap();
                let geometry = BrushGeometry::from_solid(brush)
                    .map_err(|error| map_core::EditError::Invalid(error.to_string()))?;
                let surface = geometry
                    .faces
                    .iter()
                    .find(|face| brush.sides[face.side].id == side)
                    .unwrap();
                let info = brush.sides[surface.side].displacement.as_ref().unwrap();
                let mesh = DisplacementMesh::build(surface, info)
                    .map_err(|error| map_core::EditError::Invalid(error.to_string()))?;
                let info = tx.solid_mut(solid)?.sides[surface.side]
                    .displacement
                    .as_mut()
                    .unwrap();
                info.normals = Some(vec![vec![DVec3::Z; mesh.size]; mesh.size]);
                info.distances = Some(
                    mesh.positions
                        .chunks(mesh.size)
                        .map(|row| row.iter().map(|point| point.y * point.y / 512.0).collect())
                        .collect(),
                );
                Ok(())
            })
            .unwrap();
        faces.push(face);
    }
    let selected = session
        .create_block(
            Actor::User,
            DVec3::new(-96.0, -96.0, 32.0),
            DVec3::new(-80.0, -80.0, 48.0),
            "brick/test",
            false,
        )
        .unwrap();
    session
        .select(&[selected], crate::session::SelectMode::Replace)
        .unwrap();
    (
        session,
        selected,
        faces[1],
        Ray {
            origin: DVec3::new(130.0, 17.0, 128.0),
            direction: DVec3::NEG_Z,
        },
    )
}

pub fn model_target() -> (tempfile::TempDir, Session, ObjectId, ObjectId, Ray) {
    let (directory, game) = crate::test_assets::model_lighting::game();
    let mut session = Session::new(Ok(game));
    let model = session
        .create_entity(
            Actor::User,
            "prop_dynamic",
            Some(DVec3::new(100.0, 200.0, 40.0)),
            BTreeMap::from([
                ("model".into(), "models/test/receiver.mdl".into()),
                ("angles".into(), "0 90 0".into()),
                ("modelscale".into(), "2".into()),
                ("skin".into(), "1".into()),
            ]),
        )
        .unwrap();
    let selected = session
        .create_block(
            Actor::User,
            DVec3::new(-96.0, -96.0, 32.0),
            DVec3::new(-80.0, -80.0, 48.0),
            "test/checker",
            false,
        )
        .unwrap();
    session
        .select(&[selected], crate::session::SelectMode::Replace)
        .unwrap();
    (
        directory,
        session,
        selected,
        model,
        Ray {
            origin: DVec3::new(144.0, 158.0, 200.0),
            direction: DVec3::NEG_Z * 3.0,
        },
    )
}
