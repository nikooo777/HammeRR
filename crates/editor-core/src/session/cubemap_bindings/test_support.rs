use glam::DVec3;
use map_core::{ObjectId, edit, surface::FaceId};

use crate::session::{Actor, SelectMode, Session};

pub fn fixture() -> (tempfile::TempDir, Session, u32, u32, FaceId) {
    let (assets, game) = crate::test_assets::game();
    std::fs::write(
        assets.path().join("materials/test/checker.vmt"),
        "LightmappedGeneric { $basetexture test/checker $envmap env_cubemap }",
    )
    .unwrap();
    let mut session = Session::new(Ok(game));
    let (near, far, face) = session
        .edit(Actor::Agent, "reflection fixture", |tx| {
            let ObjectId::Solid(solid) = edit::create_block(
                tx,
                DVec3::new(-64.0, -64.0, 0.0),
                DVec3::new(64.0, 64.0, 16.0),
                "test/checker",
            )?
            else {
                unreachable!()
            };
            let side = tx
                .solid_mut(solid)?
                .sides
                .iter()
                .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
                .unwrap()
                .id;
            let ObjectId::Entity(near) = edit::create_entity(
                tx,
                "env_cubemap",
                Some(DVec3::new(-16.0, 0.0, 96.0)),
                &[("cubemapsize".into(), "3".into())],
                Vec::new(),
            )?
            else {
                unreachable!()
            };
            let ObjectId::Entity(far) = edit::create_entity(
                tx,
                "env_cubemap",
                Some(DVec3::new(128.0, 0.0, 96.0)),
                &[
                    ("cubemapsize".into(), "3".into()),
                    ("targetname".into(), "authored_sample".into()),
                    ("custom".into(), "preserve me".into()),
                ],
                Vec::new(),
            )?
            else {
                unreachable!()
            };
            Ok((near, far, FaceId { solid, side }))
        })
        .unwrap();
    session.select_faces(&[face], SelectMode::Replace).unwrap();
    (assets, session, near, far, face)
}
