use std::collections::BTreeMap;

use glam::DVec3;
use map_core::ObjectId;

use crate::session::{Actor, Session};

pub struct Authored {
    pub directory: tempfile::TempDir,
    pub session: Session,
    pub source: std::path::PathBuf,
    pub child: std::path::PathBuf,
    pub owner: u32,
    pub helper: u32,
}

pub fn fixture(actor: Actor) -> Authored {
    let (directory, game, source, child) = super::instance_static_props::fixture();
    let mut session = Session::new(Ok(game));
    session.set_definitions(actor, source_formats::fgd::Catalog::parse(r#"
        @PointClass = info_lighting [ targetname(target_source) : "Name" : "" ]
        @PointClass = prop_static [ lightingorigin(target_destination) : "Lighting Origin" : "" ]
        @PointClass = func_instance [
            targetname(target_source) : "Fix Up Name" : ""
            fixup_style(choices) : "Entity Name Fix Up" : 0 = [ 0 : "Prefix" 1 : "Postfix" 2 : "None" ]
        ]
    "#).unwrap());
    session.open(actor, &child, true).unwrap();
    let prop = session.document().vmf().entities[0].id;
    let ObjectId::Entity(helper) = session
        .create_entity(
            actor,
            "info_lighting",
            Some(DVec3::new(96.0, -32.0, 64.0)),
            BTreeMap::from([("targetname".into(), "sample".into())]),
        )
        .unwrap()
    else {
        unreachable!()
    };
    session
        .set_entity_properties(
            actor,
            prop,
            &[("lightingorigin".into(), Some("sample".into()))],
        )
        .unwrap();
    session.save(actor, None).unwrap();
    session.open(actor, &child, false).unwrap();
    assert_eq!(
        session
            .document()
            .entity(helper)
            .unwrap()
            .properties
            .get("targetname"),
        Some("sample")
    );
    assert_eq!(
        session
            .document()
            .entity(prop)
            .unwrap()
            .properties
            .get("lightingorigin"),
        Some("sample")
    );
    session.open(actor, &source, false).unwrap();
    let owner = session.document().vmf().entities.last().unwrap().id;
    session
        .set_entity_properties(
            actor,
            owner,
            &[
                ("targetname".into(), Some("assembly".into())),
                ("fixup_style".into(), Some("0".into())),
            ],
        )
        .unwrap();
    session.save(actor, None).unwrap();
    session.open(actor, &source, false).unwrap();
    Authored {
        directory,
        session,
        source,
        child,
        owner,
        helper,
    }
}
