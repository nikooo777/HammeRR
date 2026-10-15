use super::*;
use glam::DVec3;
use map_core::edit;

pub fn fixture() -> (Session, ObjectId) {
    let mut session = Session::new(Err("No game".into()));
    let source = session
        .edit(Actor::User, "source", |tx| {
            edit::create_entity(
                tx,
                "info_target",
                Some(DVec3::new(16.0, 0.0, 0.0)),
                &[("targetname".into(), "array_part".into())],
                Vec::new(),
            )
        })
        .unwrap();
    session.select(&[source], SelectMode::Replace).unwrap();
    (session, source)
}
