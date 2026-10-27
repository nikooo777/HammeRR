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

pub struct InstanceFixture {
    pub assets: tempfile::TempDir,
    pub directory: tempfile::TempDir,
    pub session: Session,
    pub near: u32,
    pub far: u32,
    pub face: FaceId,
    pub owner: u32,
    pub child_helper: u32,
}

pub fn instance_fixture() -> InstanceFixture {
    let (assets, mut session, near, far, face) = fixture();
    std::fs::write(
        assets.path().join("materials/test/child.vmt"),
        "LightmappedGeneric { $basetexture test/checker $envmap env_cubemap }",
    )
    .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let mut child = map_core::Document::empty();
    let child_helper = child
        .edit("child cubemap and brush", |tx| {
            edit::create_block(
                tx,
                DVec3::new(256.0, -16.0, 0.0),
                DVec3::new(288.0, 16.0, 16.0),
                "test/child",
            )?;
            let ObjectId::Entity(helper) = edit::create_entity(
                tx,
                "env_cubemap",
                Some(DVec3::new(0.0, 0.0, 48.0)),
                &[("cubemapsize".into(), "3".into())],
                Vec::new(),
            )?
            else {
                unreachable!()
            };
            Ok(helper)
        })
        .unwrap();
    std::fs::write(
        directory.path().join("leaf.vmf"),
        child.vmf().write(&Default::default()),
    )
    .unwrap();
    let mut middle = map_core::Document::empty().into_vmf();
    for (id, origin) in [(2, "0 0 0"), (3, "512 0 0")] {
        let mut helper = source_formats::vmf::Entity::new(id, "func_instance");
        helper.properties.set("file", "leaf.vmf");
        helper.properties.set("origin", origin);
        middle.entities.push(helper);
    }
    std::fs::write(
        directory.path().join("middle.vmf"),
        middle.write(&Default::default()),
    )
    .unwrap();
    let owner = session
        .edit(Actor::Agent, "reference child", |tx| {
            let ObjectId::Entity(owner) = edit::create_entity(
                tx,
                "func_instance",
                Some(DVec3::ZERO),
                &[("file".into(), "middle.vmf".into())],
                Vec::new(),
            )?
            else {
                unreachable!()
            };
            Ok(owner)
        })
        .unwrap();
    session
        .save(Actor::Agent, Some(&directory.path().join("parent.vmf")))
        .unwrap();
    InstanceFixture {
        assets,
        directory,
        session,
        near,
        far,
        face,
        owner,
        child_helper,
    }
}
