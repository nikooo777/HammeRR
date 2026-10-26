use glam::DVec3;
use map_core::{Document, edit};

use crate::session::{Actor, Session, compilation::BuildSettingsUpdate};

pub fn compiled() -> (tempfile::TempDir, Session, std::path::PathBuf) {
    let (directory, game, source) = super::super::compiled_world_lights::test_support::fixture();
    let mut document = Document::new(source);
    document
        .edit("Visibility fixture partition", |tx| {
            edit::create_block(
                tx,
                DVec3::new(120.0, -256.0, 0.0),
                DVec3::new(136.0, 64.0, 256.0),
                "test/checker",
            )?;
            Ok(())
        })
        .unwrap();
    let source = directory.path().join("maps/visibility.vmf");
    std::fs::write(&source, document.vmf().write(&Default::default())).unwrap();
    let mut session = Session::new(Ok(game));
    session.open(Actor::User, &source, false).unwrap();
    session
        .update_build_settings(
            Actor::User,
            BuildSettingsUpdate {
                lighting: Some("fullbright"),
                physics: Some("none"),
                generate_reflections: Some(false),
                pack_water_assets: Some(false),
                pack_sky_assets: Some(false),
                pack_static_prop_assets: Some(false),
                bake_static_prop_vertices: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
    let result = session.begin_compile(Actor::User, None).unwrap().run();
    session.finish_compile(Actor::User, &result);
    assert!(
        result.as_ref().unwrap().published(),
        "{:?}",
        session.compile_status()
    );
    let report = session
        .inspect_compiled_visibility(&super::Query::default())
        .unwrap();
    assert_eq!(report.build["status"], "current");
    assert!(report.summary.unwrap().portals > 0);
    (directory, session, source)
}
