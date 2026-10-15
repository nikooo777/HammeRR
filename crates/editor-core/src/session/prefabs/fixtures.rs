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
