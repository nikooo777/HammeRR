use std::collections::BTreeMap;
use std::path::PathBuf;

use glam::DVec3;
use map_core::{Document, ObjectId};
use source_formats::gamefs::{Mount, MountedPath};

use crate::session::{Actor, SelectMode, Session};

pub struct Authored {
    pub directory: tempfile::TempDir,
    pub session: Session,
    pub source: PathBuf,
    pub child: PathBuf,
    pub owner: u32,
    pub spawns: [u32; 2],
}

pub fn fixture(actor: Actor, mixed: bool) -> Authored {
    let (directory, mut game, source, child) = super::instance_map(false);
    if !mixed {
        std::fs::write(&child, Document::empty().vmf().write(&Default::default())).unwrap();
    }
    let tools = game.game_dir.join("materials/tools");
    std::fs::create_dir_all(&tools).unwrap();
    std::fs::write(tools.join("toolsblack.vmt"), "LightmappedGeneric {}").unwrap();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(game.game_dir.clone()),
    });
    let mut session = Session::new(Ok(game));
    session.set_definitions(
        actor,
        source_formats::fgd::Catalog::parse(
            r#"
        @BaseClass = Angles [ angles(angle) : "Angles" : "0 0 0" ]
        @PointClass base(Angles) = info_player_terrorist [ ]
        @PointClass base(Angles) = info_player_counterterrorist [ ]
    "#,
        )
        .unwrap(),
    );
    session.open(actor, &child, true).unwrap();
    let mut spawns = [0; 2];
    for (index, classname) in ["info_player_terrorist", "info_player_counterterrorist"]
        .into_iter()
        .enumerate()
    {
        let ObjectId::Entity(id) = session
            .create_entity(
                actor,
                classname,
                Some(DVec3::new(if index == 0 { 16.0 } else { -16.0 }, 8.0, 32.0)),
                BTreeMap::new(),
            )
            .unwrap()
        else {
            unreachable!()
        };
        spawns[index] = id;
    }
    session
        .set_entity_properties(
            actor,
            spawns[1],
            &[("angles".into(), Some("0 0 -90".into()))],
        )
        .unwrap();
    session.save(actor, None).unwrap();
    session.open(actor, &child, false).unwrap();
    assert_eq!(
        session
            .document()
            .entity(spawns[0])
            .unwrap()
            .properties
            .get("angles"),
        Some("0 0 0")
    );
    for id in spawns {
        assert!(
            session
                .document()
                .entity(id)
                .unwrap()
                .properties
                .iter()
                .all(|(key, _)| matches!(key, "classname" | "origin" | "angles"))
        );
    }
    session.open(actor, &source, false).unwrap();
    let owner = session.document().vmf().entities.last().unwrap().id;
    session
        .set_entity_properties(actor, owner, &[("origin".into(), Some("24 -16 16".into()))])
        .unwrap();
    session.save(actor, None).unwrap();
    session.open(actor, &source, false).unwrap();
    session.set_view(render::ViewOptions {
        instance_contents: true,
        ..session.view()
    });
    session
        .select(&[ObjectId::Entity(owner)], SelectMode::Replace)
        .unwrap();
    Authored {
        directory,
        session,
        source,
        child,
        owner,
        spawns,
    }
}
