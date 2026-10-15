use super::*;
use glam::DVec3;
use map_core::ObjectId;
use source_formats::fgd::Catalog;

pub(crate) fn pcf(name: &str) -> Vec<u8> {
    let dictionary = [
        "DmElement",
        "DmeParticleSystemDefinition",
        "particleSystemDefinitions",
        "material",
        "max_particles",
    ];
    let mut data = b"<!-- dmx encoding binary 2 format pcf 1 -->\n\0".to_vec();
    data.extend((dictionary.len() as i16).to_le_bytes());
    for value in dictionary {
        data.extend(value.as_bytes());
        data.push(0);
    }
    data.extend(2i32.to_le_bytes());
    for (kind, name) in [(0i16, "root"), (1, name)] {
        data.extend(kind.to_le_bytes());
        data.extend(name.as_bytes());
        data.push(0);
        data.extend([0; 16]);
    }
    data.extend(1i32.to_le_bytes());
    data.extend(2i16.to_le_bytes());
    data.push(15);
    data.extend(1i32.to_le_bytes());
    data.extend(1i32.to_le_bytes());
    data.extend(2i32.to_le_bytes());
    data.extend(3i16.to_le_bytes());
    data.push(5);
    data.extend(b"particle/fire.vmt\0");
    data.extend(4i16.to_le_bytes());
    data.push(2);
    data.extend(100i32.to_le_bytes());
    data
}

pub fn fixture() -> (tempfile::TempDir, Session, u32) {
    let (directory, game) = crate::test_assets::game();
    std::fs::create_dir_all(directory.path().join("particles")).unwrap();
    std::fs::write(
        directory.path().join("particles/particles_manifest.txt"),
        "particles_manifest { file !particles/test.pcf file particles/sparks.pcf }",
    )
    .unwrap();
    std::fs::write(
        directory.path().join("particles/test.pcf"),
        pcf("test_flame"),
    )
    .unwrap();
    std::fs::write(
        directory.path().join("particles/sparks.pcf"),
        pcf("test_sparks"),
    )
    .unwrap();
    let mut session = Session::new(Ok(game));
    session.set_definitions(Actor::User,Catalog::parse("@PointClass = info_particle_system : \"Particle system\" [ effect_name(string) : \"Particle effect\" : \"\" targetname(string) : \"Name\" : \"\" ]\n@PointClass = info_target [ targetname(string) : \"Name\" : \"\" ]").unwrap());
    let ObjectId::Entity(id) = session
        .create_entity(
            Actor::User,
            "info_particle_system",
            Some(DVec3::ZERO),
            BTreeMap::from([
                ("targetname".into(), "particle_demo".into()),
                ("unrelated".into(), "preserved".into()),
            ]),
        )
        .unwrap()
    else {
        unreachable!()
    };
    (directory, session, id)
}
