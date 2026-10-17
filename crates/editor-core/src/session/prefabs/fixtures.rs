use super::*;
use crate::session::Session;

pub fn fixture() -> (Session, tempfile::TempDir, ObjectId) {
    let directory = tempfile::tempdir().unwrap();
    let (mut session, id, _) = crate::test_assets::clipboard();
    session.prefabs.store = Ok(Store {
        directory: directory.path().into(),
    });
    (session, directory, id)
}

pub fn textured_fixture() -> (Session, tempfile::TempDir, tempfile::TempDir, ObjectId) {
    let (assets, game) = crate::test_assets::game();
    let library = tempfile::tempdir().unwrap();
    let mut session = Session::new(Ok(game));
    session.prefabs.store = Ok(Store {
        directory: library.path().to_owned(),
    });
    let brush = session
        .create_block(
            Actor::User,
            glam::DVec3::ZERO,
            glam::DVec3::new(96.0, 64.0, 24.0),
            "test/checker",
            false,
        )
        .unwrap();
    session
        .select(&[brush], crate::session::SelectMode::Replace)
        .unwrap();
    (session, library, assets, brush)
}
