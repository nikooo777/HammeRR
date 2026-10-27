use super::*;

use super::fixtures::{fixture, wave};

#[test]
fn sound_choices_share_catalog_guards_cached_reads_and_atomic_undo() {
    let (directory, mut session, id) = fixture();
    let catalog = session.browse_sounds("", "all", 0, 100).unwrap();
    assert_eq!(catalog["total"], 3);
    assert_eq!(
        session.browse_sounds("rain", "scripts", 0, 100).unwrap()["total"],
        1
    );
    assert!(session.browse_sounds("", "all", 0, 101).is_err());
    let report = session.inspect_sound("Test.Rain", 1).unwrap();
    assert_eq!(report["path"], "ambient/two.wav");
    assert_eq!(report["duration"], 0.1);
    let source = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let token = session.begin_sound_choice(id, "message", false).unwrap()["token"]
        .as_u64()
        .unwrap();
    assert_eq!(
        session.begin_sound_choice(id, "message", true).unwrap()["token"],
        token
    );
    session.choose_sound(token, "Test.Rain", 1).unwrap();
    assert!(session.choose_sound(token, "Test.Rain", 99).is_err());
    session.reload_material_assets(Actor::Agent);
    assert_eq!(session.current_sound_choice()["sound"]["variant"], 1);
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.document().undo_labels().count(), history);
    session.apply_sound_choice(Actor::Agent, token).unwrap();
    assert_eq!(
        session
            .document()
            .entity(id)
            .unwrap()
            .properties
            .get("message"),
        Some("Test.Rain")
    );
    assert_eq!(session.document().undo_labels().count(), history + 1);
    assert_eq!(
        session.browse_sounds("", "used", 0, 100).unwrap()["total"],
        1
    );
    session.undo(Actor::User).unwrap();
    assert_eq!(session.document().vmf(), &source);
    let old = session.begin_sound_choice(id, "message", false).unwrap()["token"]
        .as_u64()
        .unwrap();
    let token = session.begin_sound_choice(id, "message", false).unwrap()["token"]
        .as_u64()
        .unwrap();
    assert!(session.cancel_sound_choice(old).is_err());
    session.choose_sound(token, "ambient/one.wav", 0).unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(directory.path().join("sound/ambient/one.wav"))
        .unwrap()
        .set_len(MAX_AUDIO_BYTES as u64 + 1)
        .unwrap();
    assert_eq!(
        session.sound_choice(token).unwrap()["sound"]["encoded_bytes"],
        844
    );
    assert!(session.apply_sound_choice(Actor::User, token).is_err());
    assert_eq!(session.document().vmf(), &source);
    std::fs::write(directory.path().join("sound/ambient/one.wav"), wave()).unwrap();
    session.reload_sound_assets();
    session.reload_material_assets(Actor::User);
    assert_eq!(session.current_sound_choice()["valid"], false);
    session.cancel_sound_choice(token).unwrap();
}

#[test]
fn malformed_wave_fields_warn_and_preserve_raw_sound_browsing() {
    let (directory, mut session, _) = fixture();
    let text = "Test.Rain { wave ambient/one.wav rndwave ambient/two.wav }";
    let path = directory.path().join("scripts/audio.txt");
    std::fs::write(&path, text).unwrap();
    let source = session.document().vmf().clone();
    let report = session.browse_sounds("", "all", 0, 100).unwrap();
    assert_eq!(report["total"], 2);
    let warnings = report["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1);
    let warning = warnings[0].as_str().unwrap();
    assert!(warning.contains("scripts/audio.txt"));
    assert!(warning.contains("Test.Rain: rndwave must be a block"));
    assert!(session.inspect_sound("Test.Rain", 0).is_err());
    assert_eq!(
        session.inspect_sound("ambient/one.wav", 0).unwrap()["path"],
        "ambient/one.wav"
    );
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(std::fs::read_to_string(path).unwrap(), text);
}

#[test]
fn invalid_manifests_and_ambiguous_names_preserve_raw_file_exploration() {
    let (directory, mut session, _) = fixture();
    std::fs::write(
        directory.path().join("scripts/game_sounds_manifest.txt"),
        "broken",
    )
    .unwrap();
    let report = session.browse_sounds("", "all", 0, 100).unwrap();
    assert_eq!(report["total"], 2);
    assert!(!report["warnings"].as_array().unwrap().is_empty());
    std::fs::write(directory.path().join("scripts/game_sounds_manifest.txt"),"game_sounds_manifest { preload_file scripts/audio.txt preload_file scripts/duplicate.txt }").unwrap();
    std::fs::write(
        directory.path().join("scripts/duplicate.txt"),
        "test.rain { wave ambient/one.wav }",
    )
    .unwrap();
    session.reload_sound_assets();
    assert_eq!(
        session.browse_sounds("rain", "scripts", 0, 100).unwrap()["total"],
        2
    );
    assert!(session.inspect_sound("Test.Rain", 0).is_err());
    assert!(session.inspect_sound("*#ambient/one.wav", 0).is_ok());
    assert!(session.inspect_sound("../../outside.wav", 0).is_err());
}

#[test]
fn audio_export_uses_bounded_pcm_and_shared_volume_without_map_edits() {
    if std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .status()
        .is_err()
    {
        return;
    }
    let (_directory, mut session, _) = fixture();
    let source = session.document().vmf().clone();
    let camera = session.camera();
    session.set_sound_preview_volume(0.25).unwrap();
    let first = session.render_sound_preview("Test.Rain", 0, None).unwrap();
    let second = session
        .render_sound_preview("Test.Rain", 0, Some(0.0))
        .unwrap();
    assert_eq!(first.report["preview"]["volume"], 0.25);
    assert_eq!(first.report["preview"]["sample_rate"], 44100);
    assert_ne!(first.bytes, second.bytes);
    assert!(first.bytes.len() < 2 * 1024 * 1024);
    assert!(session.set_sound_preview_volume(f32::NAN).is_err());
    assert!(
        session
            .render_sound_preview("Test.Rain", 0, Some(1.1))
            .is_err()
    );
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.camera(), camera);
}
