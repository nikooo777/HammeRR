use std::path::PathBuf;

use source_formats::gamefs::GameFs;
use source_formats::vmf::{Entity, Vmf};

use crate::session::{Actor, Session, compilation::BuildSettingsUpdate};

pub fn fixture() -> (tempfile::TempDir, GameFs, Vmf) {
    let (directory, game) = crate::test_assets::game();
    std::fs::create_dir(directory.path().join("maps")).unwrap();
    let mut vmf = crate::test_assets::room();
    for side in vmf
        .world
        .solids
        .iter_mut()
        .flat_map(|solid| &mut solid.sides)
    {
        side.material = "test/checker".into();
        side.lightmap_scale = 128;
    }
    for (id, class, properties) in [
        (
            9001,
            "light",
            vec![
                ("origin", "-64 0 160"),
                ("style", "3"),
                ("_light", "255 0 0 510"),
                ("_lightHDR", "0 255 0 765"),
                ("_lightscaleHDR", "2"),
                ("_constant_attn", "1"),
                ("_linear_attn", "0"),
                ("_quadratic_attn", "0"),
                ("_distance", "512"),
            ],
        ),
        (
            9002,
            "light_spot",
            vec![
                ("origin", "64 32 192"),
                ("style", "7"),
                ("_light", "0 0 255 63.75"),
                ("_lightHDR", "255 0 255 191.25"),
                ("_lightscaleHDR", "1"),
                ("_constant_attn", "2"),
                ("_linear_attn", "0.01"),
                ("_quadratic_attn", "0.0001"),
                ("_distance", "384"),
                ("angles", "0 90 0"),
                ("pitch", "-90"),
                ("_inner_cone", "60"),
                ("_cone", "60"),
                ("_exponent", "2"),
            ],
        ),
        (
            9003,
            "light_environment",
            vec![
                ("origin", "-16 48 224"),
                ("angles", "0 0 0"),
                ("pitch", "-90"),
                ("_light", "0 255 0 1020"),
                ("_lightHDR", "255 0 0 2040"),
                ("_lightscaleHDR", "0.5"),
                ("_ambient", "0 0 255 127.5"),
                ("_ambientHDR", "255 255 0 510"),
                ("_AmbientScaleHDR", "0.25"),
                ("SunSpreadAngle", "0"),
            ],
        ),
    ] {
        let mut entity = Entity::new(id, class);
        for (key, value) in properties {
            entity.properties.set(key, value);
        }
        vmf.entities.push(entity);
    }
    (directory, game, vmf)
}

pub fn compiled(dual: bool) -> (tempfile::TempDir, Session, PathBuf) {
    let (directory, game, vmf) = fixture();
    let source = directory.path().join("maps/lights.vmf");
    std::fs::write(&source, vmf.write(&Default::default())).unwrap();
    std::fs::write(source.with_extension("lin"), b"previous pointfile").unwrap();
    let mut session = Session::new(Ok(game));
    session.open(Actor::User, &source, false).unwrap();
    session
        .update_build_settings(
            Actor::User,
            BuildSettingsUpdate {
                lighting: Some(if dual { "direct_ldr_hdr" } else { "direct_ldr" }),
                physics: Some("none"),
                sky_quality: Some("fast"),
                bounces: Some(0),
                bake_static_prop_vertices: Some(false),
                pack_sky_assets: Some(false),
                pack_water_assets: Some(false),
                pack_static_prop_assets: Some(false),
                generate_reflections: Some(false),
                refine_ambient_probes: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
    let completed = session.begin_compile(Actor::User, None).unwrap().run();
    session.finish_compile(Actor::User, &completed);
    assert!(
        completed.as_ref().unwrap().published(),
        "{:?}",
        session.compile_status()
    );
    (directory, session, source)
}
