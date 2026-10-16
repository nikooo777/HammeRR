use std::fs;

use source_formats::gamefs::GameFs;
use source_formats::keyvalues::WriteOptions;

use super::*;
use crate::session::{Actor, PropertyChange, PropertyTarget};

fn fixture(extra: &str) -> (tempfile::TempDir, GameFs) {
    let (directory, game) = crate::test_assets::game();
    let skybox = directory.path().join("materials/skybox");
    fs::create_dir_all(&skybox).unwrap();
    for suffix in render::sky::SUFFIXES {
        fs::write(
            skybox.join(format!("test{suffix}.vmt")),
            format!("Sky {{ $basetexture test/checker $hdrbasetexture skybox/hdr {extra} }}"),
        )
        .unwrap();
    }
    let source = fs::read(directory.path().join("materials/test/checker.vtf")).unwrap();
    let mut hdr = source[..80].to_vec();
    hdr[52..56].copy_from_slice(&24u32.to_le_bytes());
    hdr.resize(80 + 64 * 32 * 8, 0);
    fs::write(skybox.join("hdr.vtf"), hdr).unwrap();
    (directory, game)
}

#[test]
fn structure_readiness_does_not_imply_editor_preview_support() {
    let (_directory, game) =
        fixture("$basetexturetransform \"center .5 .5 scale 1 1 rotate 15 translate 0 0\"");
    let mut vmf = map_core::Document::empty().vmf().clone();
    vmf.world.properties.set("skyname", "test");
    let report = compiler::sky_assets::inspect_sky_assets(&vmf, &game, None);
    assert!(report.ldr_ready && report.hdr_ready);
    assert!(!render::sky::Skybox::load(&game, "test").ready());
}

#[test]
fn session_inspection_preserves_applied_map_history_and_pending_world_draft() {
    let (_directory, game) = fixture("");
    let mut session = Session::new(Ok(game));
    session.begin_property_edit(PropertyTarget::World).unwrap();
    let token = session.property_draft().unwrap().token;
    session
        .update_property_edit(
            token,
            vec![PropertyChange::Set {
                key: "skyname".into(),
                value: "test".into(),
            }],
        )
        .unwrap();
    session.commit_property_edit(Actor::Agent, token).unwrap();
    session.begin_property_edit(PropertyTarget::World).unwrap();
    let token = session.property_draft().unwrap().token;
    session
        .update_property_edit(
            token,
            vec![PropertyChange::Set {
                key: "skyname".into(),
                value: "not_applied".into(),
            }],
        )
        .unwrap();
    let source = session.document().vmf().write(&WriteOptions::default());
    let history = serde_json::to_value(session.inspect_history(0, 50).unwrap()).unwrap();
    let draft = serde_json::to_value(session.property_draft()).unwrap();
    let report = session.inspect_sky_assets();
    assert_eq!(
        serde_json::to_vec(&report).unwrap(),
        serde_json::to_vec(&compiler::sky_assets::inspect_sky_assets(
            session.document().vmf(),
            session.game(),
            session.game_error(),
        ))
        .unwrap(),
    );
    assert_eq!(report.authored_name.as_deref(), Some("test"));
    assert!(report.ldr_ready && report.hdr_ready);
    assert_eq!(
        source,
        session.document().vmf().write(&WriteOptions::default())
    );
    assert_eq!(
        history,
        serde_json::to_value(session.inspect_history(0, 50).unwrap()).unwrap()
    );
    assert_eq!(
        draft,
        serde_json::to_value(session.property_draft()).unwrap()
    );
}
