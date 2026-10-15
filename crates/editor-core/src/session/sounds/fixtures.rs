use super::*;
use glam::DVec3;
use map_core::ObjectId;
use source_formats::fgd::Catalog;

pub(crate) fn wave() -> Vec<u8> {
    let mut data = b"RIFF".to_vec();
    data.extend(836u32.to_le_bytes());
    data.extend(b"WAVEfmt ");
    data.extend(16u32.to_le_bytes());
    data.extend(1u16.to_le_bytes());
    data.extend(1u16.to_le_bytes());
    data.extend(8000u32.to_le_bytes());
    data.extend(8000u32.to_le_bytes());
    data.extend(1u16.to_le_bytes());
    data.extend(8u16.to_le_bytes());
    data.extend(b"data");
    data.extend(800u32.to_le_bytes());
    data.extend((0..800).map(|sample| if sample % 16 < 8 { 64 } else { 192 }));
    data
}

pub fn fixture() -> (tempfile::TempDir, Session, u32) {
    let (directory, game) = crate::test_assets::game();
    std::fs::create_dir_all(directory.path().join("sound/ambient")).unwrap();
    std::fs::create_dir_all(directory.path().join("scripts")).unwrap();
    std::fs::write(directory.path().join("sound/ambient/one.wav"), wave()).unwrap();
    std::fs::write(directory.path().join("sound/ambient/two.wav"), wave()).unwrap();
    std::fs::write(
        directory.path().join("scripts/game_sounds_manifest.txt"),
        "game_sounds_manifest { preload_file scripts/audio.txt }",
    )
    .unwrap();
    std::fs::write(directory.path().join("scripts/audio.txt"),"Test.Rain { volume 0.7 pitch 100 rndwave { wave *#ambient/one.wav wave ambient/two.wav } }").unwrap();
    let mut session = Session::new(Ok(game));
    session.set_definitions(Actor::User,Catalog::parse("@PointClass = ambient_generic : \"Ambient sound\" [ message(sound) : \"Sound Name\" : \"\" ]").unwrap());
    let ObjectId::Entity(id) = session
        .create_entity(
            Actor::User,
            "ambient_generic",
            Some(DVec3::ZERO),
            BTreeMap::new(),
        )
        .unwrap()
    else {
        unreachable!()
    };
    (directory, session, id)
}
