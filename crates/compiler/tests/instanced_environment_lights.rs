mod common;

use std::path::Path;

use compiler::instances::{self, PreparedInstances, SourceContext};
use compiler::lighting::LuxelFrame;
use compiler::{Control, LightingMode, Options, SkyQuality};
use glam::DVec3;
use map_core::{Document, Plane, edit};
use source_formats::bsp::{self, Bsp, LightingKind, TexInfo, WorldLight};
use source_formats::fgd::Catalog;
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::keyvalues::Node;
use source_formats::vmf::{Entity, Vmf};

const MODES: [LightingMode; 3] = [
    LightingMode::Fullbright,
    LightingMode::DirectLdr,
    LightingMode::DirectLdrHdr,
];
const CHANNEL_FIELDS: [(&str, &str); 7] = [
    ("_light", "255 0 0 200"),
    ("_ambient", "0 0 255 40"),
    ("_lightHDR", "0 255 0 600"),
    ("_lightscaleHDR", "0.5"),
    ("_ambientHDR", "255 0 0 20"),
    ("_AmbientScaleHDR", "2"),
    ("SunSpreadAngle", "0"),
];

fn next_id(source: &Vmf) -> u32 {
    source.object_ids().chain(source.side_ids()).max().unwrap() + 1
}

fn instance(source: &mut Vmf, file: &str, origin: &str, angles: &str) -> u32 {
    let id = next_id(source);
    let mut entity = Entity::new(id, "func_instance");
    for (key, value) in [("file", file), ("origin", origin), ("angles", angles)] {
        entity.properties.set(key, value);
    }
    source.entities.push(entity);
    id
}

fn environment(origin: &str, angles: Option<&str>, pitch: &str) -> Entity {
    let mut entity = Entity::new(2, "light_environment");
    entity.properties.set("origin", origin);
    if let Some(angles) = angles {
        entity.properties.set("angles", angles);
    }
    entity.properties.set("pitch", pitch);
    for (key, value) in CHANNEL_FIELDS {
        entity.properties.set(key, value);
    }
    entity
}

fn leaf(entity: Entity) -> Vmf {
    let mut source = Document::empty().into_vmf();
    source.entities.push(entity);
    source
}

fn write_source(directory: &Path, name: &str, source: &Vmf) -> Vec<u8> {
    let bytes = source.write(&Default::default()).into_bytes();
    std::fs::write(directory.join(name), &bytes).unwrap();
    bytes
}

fn saved_parent(directory: &Path, parent: &Vmf) -> SourceContext {
    write_source(directory, "parent.vmf", parent);
    SourceContext::capture(&directory.join("parent.vmf"), &Control::default()).unwrap()
}

fn game(directory: &Path) -> GameFs {
    for (name, text) in [
        ("sky", "UnlitGeneric { %compilesky 1 }"),
        ("tools/toolsblack", "UnlitGeneric { }"),
        ("dev/dev_measuregeneric01", "LightmappedGeneric { }"),
    ] {
        let path = directory.join(format!("materials/{name}.vmt"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.to_owned()),
    });
    game
}

fn room(sky: bool, shade: bool) -> Vmf {
    let mut document = Document::new(common::room());
    if shade {
        document
            .edit("shadow caster", |tx| {
                edit::create_block(
                    tx,
                    DVec3::new(-32.0, -128.0, 96.0),
                    DVec3::new(32.0, 128.0, 112.0),
                    "dev/dev_measuregeneric01",
                )
            })
            .unwrap();
    }
    let mut source = document.into_vmf();
    for solid in &mut source.world.solids {
        let ceiling = solid.sides.iter().all(|side| side.plane.0[0].z >= 256.0);
        for side in &mut solid.sides {
            side.lightmap_scale = 32;
            if sky && ceiling && Plane::from_points(side.plane.0).unwrap().normal.z < -0.99 {
                side.material = "sky".into();
            }
        }
    }
    source.world.properties.set("skyname", "root_sky");
    source
}

fn options(mode: LightingMode) -> Options {
    Options {
        lighting: mode,
        sky_quality: SkyQuality::Fast,
        ..Options::default()
    }
}

fn compile_saved(
    source: &Vmf,
    game: &GameFs,
    mode: LightingMode,
    context: &SourceContext,
) -> compiler::Output {
    compiler::compile_with_source_controlled(
        source,
        game,
        options(mode),
        None,
        Some(context),
        &Control::default(),
    )
}

fn successful(output: compiler::Output) -> Bsp {
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.leak.is_none());
    Bsp::parse(&output.bsp.unwrap().to_bytes()).unwrap()
}

fn resolve<'a>(source: &'a Vmf, context: &SourceContext) -> PreparedInstances<'a> {
    let mut report = compiler::Report::default();
    let prepared =
        instances::resolve(source, Some(context), &Control::default(), &mut report).unwrap();
    assert!(!report.has_errors(), "{report:?}");
    prepared
}

fn literal(parent: &Vmf, entities: &[Entity]) -> Vmf {
    let first = next_id(parent);
    let mut source = parent.clone();
    source
        .entities
        .retain(|entity| entity.classname() != Some("func_instance"));
    for (index, entity) in entities.iter().enumerate() {
        let mut entity = entity.clone();
        entity.id = first + u32::try_from(index).unwrap();
        entity.editor = None;
        source.entities.push(entity);
    }
    source
}

fn assert_channels(bsp: &Bsp, mode: LightingMode, origin: [f32; 3], direction: [f32; 3]) {
    for kind in [LightingKind::Ldr, LightingKind::Hdr] {
        let enabled = mode != LightingMode::Fullbright
            && (kind == LightingKind::Ldr || mode == LightingMode::DirectLdrHdr);
        let actual = bsp.world_lights(kind).unwrap();
        if !enabled {
            assert!(actual.is_empty());
            assert!(bsp.lighting(kind).unwrap().is_none());
            continue;
        }
        let (sun, ambient) = match kind {
            LightingKind::Ldr => ([200.0 / 255.0, 0.0, 0.0], [0.0, 0.0, 40.0 / 255.0]),
            LightingKind::Hdr => ([0.0, 300.0 / 255.0, 0.0], [40.0 / 255.0, 0.0, 0.0]),
        };
        let expected = [
            WorldLight {
                origin,
                normal: direction,
                intensity: sun,
                cluster: -1,
                kind: bsp::world_light_kind::SKY,
                texinfo: -1,
                owner: -1,
                ..Default::default()
            },
            WorldLight {
                origin,
                intensity: ambient,
                cluster: -1,
                kind: bsp::world_light_kind::SKY_AMBIENT,
                texinfo: -1,
                owner: -1,
                ..Default::default()
            },
        ];
        assert_eq!(actual, expected, "{mode:?}/{kind:?}");
        assert!(bsp.lighting(kind).unwrap().is_some());
        assert!(bsp.leaf_ambient(kind).unwrap().is_some());
    }
}

fn floor_sample(bsp: &Bsp, kind: LightingKind, point: DVec3) -> DVec3 {
    let faces = bsp.lighting_faces(kind).unwrap();
    let infos = bsp.records::<TexInfo>().unwrap();
    let planes = bsp.records::<bsp::Plane>().unwrap();
    let lighting = bsp.lighting(kind).unwrap().unwrap();
    for face in &faces {
        let plane = planes[face.plane as usize];
        if plane.normal[2] < 0.99 || plane.dist != 0.0 {
            continue;
        }
        let info = &infos[face.texinfo as usize];
        let frame = LuxelFrame::new(
            Plane {
                normal: DVec3::from_array(plane.normal.map(f64::from)),
                dist: f64::from(plane.dist),
            },
            info,
            face.lightmap_mins,
            face.lightmap_size,
        )
        .unwrap();
        let samples = lighting.face(face, info).unwrap().unwrap();
        let [width, height] = frame.dimensions();
        for t in 0..height {
            for s in 0..width {
                if frame.position(s, t).unwrap().distance(point) < 0.001 {
                    return DVec3::from_array(
                        samples.samples(0, 0).unwrap()[t * width + s].linear(),
                    );
                }
            }
        }
    }
    panic!("No {kind:?} floor sample at {point}")
}

fn assert_error(output: &compiler::Output, owner: u32, original: u32, reason: &str) {
    assert!(output.report.has_errors() && output.bsp.is_none() && output.leak.is_none());
    assert!(
        output.report.diagnostics.iter().any(|diagnostic| {
            diagnostic.object == Some(format!("entity:{owner}"))
                && diagnostic.message.contains("leaf.vmf")
                && diagnostic
                    .message
                    .to_ascii_lowercase()
                    .contains(&reason.to_ascii_lowercase())
                && [
                    format!("Child environment light {original}"),
                    format!("child object {original}"),
                ]
                .iter()
                .any(|identity| diagnostic.message.contains(identity))
        }),
        "{reason}: {:?}",
        output.report
    );
}

fn assert_preserved(output: &compiler::Output, directory: &Path) {
    let path = directory.join("previous.bsp");
    std::fs::write(&path, b"previous BSP").unwrap();
    std::fs::write(path.with_extension("lin"), b"previous LIN").unwrap();
    assert!(!output.write(&path).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), b"previous BSP");
    assert_eq!(
        std::fs::read(path.with_extension("lin")).unwrap(),
        b"previous LIN"
    );
}

#[test]
fn translated_environment_matches_complete_literal_geometry_physics_and_both_channels() {
    let directory = tempfile::tempdir().unwrap();
    let game = game(directory.path());
    let mut child = leaf(environment("8 -4 160", Some("0 0 0"), "-45"));
    child.world.properties.set("skyname", "ignored_child_sky");
    let child_bytes = write_source(directory.path(), "leaf.vmf", &child);
    let mut parent = room(true, true);
    let owner = instance(&mut parent, "leaf.vmf", "16 24 0", "0 0 0");
    let context = saved_parent(directory.path(), &parent);
    let before = parent.clone();
    let prepared = resolve(&parent, &context);
    let generated = prepared
        .vmf()
        .entities
        .iter()
        .find(|entity| entity.classname() == Some("light_environment"))
        .unwrap();
    assert_eq!(generated.id, next_id(&parent));
    assert_eq!(prepared.root_entity(generated.id), Some(owner));
    assert_eq!(generated.properties.get("origin"), Some("24 20 160"));
    assert_eq!(generated.properties.get("angles"), Some("0 0 0"));
    assert_eq!(generated.properties.get("pitch"), Some("-45"));
    for (key, value) in CHANNEL_FIELDS {
        assert_eq!(generated.properties.get(key), Some(value));
        assert!(generated.properties.iter().any(|(actual, _)| actual == key));
    }
    assert_eq!(prepared.stats().unwrap().environment_lights, 1);
    assert_eq!(prepared.stats().unwrap().point_lights, 0);
    assert_eq!(prepared.stats().unwrap().spot_lights, 0);
    assert_eq!(prepared.generated_brushes().count(), 0);
    assert_eq!(
        prepared.vmf().world.properties.get("skyname"),
        Some("root_sky")
    );
    let expected = literal(&parent, &[environment("24 20 160", Some("0 0 0"), "-45")]);
    let diagonal = std::f32::consts::FRAC_1_SQRT_2;
    for mode in MODES {
        let output = compile_saved(&parent, &game, mode, &context);
        assert_eq!(
            output.stats.instances.as_ref().unwrap().environment_lights,
            1
        );
        assert!(output.report.diagnostics.iter().any(|diagnostic| {
            diagnostic.object == Some(format!("entity:{owner}"))
                && diagnostic
                    .message
                    .contains("root world settings remain authoritative")
        }));
        let actual = successful(output);
        let expected = successful(compiler::compile_with_options(
            &expected,
            &game,
            options(mode),
        ));
        assert_eq!(actual.to_bytes(), expected.to_bytes(), "{mode:?}");
        assert_channels(
            &actual,
            mode,
            [24.0, 20.0, 160.0],
            [diagonal, 0.0, -diagonal],
        );
    }
    assert_eq!(parent, before);
    assert_eq!(
        std::fs::read(directory.path().join("leaf.vmf")).unwrap(),
        child_bytes
    );
}

#[test]
fn nested_noncommuting_rotation_has_an_independent_literal_direction_and_full_bsp() {
    let directory = tempfile::tempdir().unwrap();
    let game = game(directory.path());
    let child = leaf(environment("8 -4 16", Some("0 315 0"), "0"));
    write_source(directory.path(), "leaf.vmf", &child);
    let mut middle = Document::empty().into_vmf();
    instance(&mut middle, "leaf.vmf", "16 0 0", "90 0 0");
    write_source(directory.path(), "middle.vmf", &middle);
    let mut parent = room(true, true);
    let owner = instance(&mut parent, "middle.vmf", "64 80 128", "0 90 0");
    let context = saved_parent(directory.path(), &parent);
    let prepared = resolve(&parent, &context);
    assert_eq!(prepared.stats().unwrap().files, 2);
    assert_eq!(prepared.stats().unwrap().instances, 2);
    assert_eq!(prepared.stats().unwrap().environment_lights, 1);
    assert_eq!(prepared.generated_brushes().count(), 0);
    let generated = prepared
        .vmf()
        .entities
        .iter()
        .find(|entity| entity.classname() == Some("light_environment"))
        .unwrap();
    assert_eq!(prepared.root_entity(generated.id), Some(owner));
    assert_eq!(generated.id, next_id(&parent));
    assert_eq!(generated.origin(), Some(DVec3::new(68.0, 112.0, 120.0)));
    let theta = 315.0_f64.to_radians();
    let expected_angle = theta.cos().atan2(-theta.sin()).to_degrees();
    let literal_angles = format!("{expected_angle} 0 -90");
    let literal_pitch = (-expected_angle).to_string();
    let expected = literal(
        &parent,
        &[environment(
            "68 112 120",
            Some(&literal_angles),
            &literal_pitch,
        )],
    );
    let diagonal = std::f32::consts::FRAC_1_SQRT_2;
    for mode in MODES {
        let actual = successful(compile_saved(&parent, &game, mode, &context));
        let expected = successful(compiler::compile_with_options(
            &expected,
            &game,
            options(mode),
        ));
        assert_eq!(actual.to_bytes(), expected.to_bytes(), "{mode:?}");
        assert_channels(
            &actual,
            mode,
            [68.0, 112.0, 120.0],
            [diagonal, 0.0, -diagonal],
        );
    }
}

#[test]
fn child_sun_direction_sky_and_opaque_shadow_discriminate_both_color_channels() {
    let mut outputs = Vec::new();
    for (sky, yaw, blocked, exposed) in [
        (true, "0 0 0", 32.0, -32.0),
        (true, "0 180 0", -32.0, 32.0),
        (false, "0 0 0", 32.0, -32.0),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let game = game(directory.path());
        let mut source = environment("0 0 128", Some(yaw), "-75");
        source.properties.set("_ambient", "0");
        source.properties.set("_ambientHDR", "0");
        write_source(directory.path(), "leaf.vmf", &leaf(source.clone()));
        let mut parent = room(sky, true);
        instance(&mut parent, "leaf.vmf", "32 16 0", "0 0 0");
        let context = saved_parent(directory.path(), &parent);
        source.properties.set("origin", "32 16 128");
        let expected = literal(&parent, &[source]);
        let actual = successful(compile_saved(
            &parent,
            &game,
            LightingMode::DirectLdrHdr,
            &context,
        ));
        let expected = successful(compiler::compile_with_options(
            &expected,
            &game,
            options(LightingMode::DirectLdrHdr),
        ));
        assert_eq!(actual.to_bytes(), expected.to_bytes());
        for kind in [LightingKind::Ldr, LightingKind::Hdr] {
            assert_eq!(actual.world_lights(kind).unwrap().len(), 1);
            assert_eq!(
                floor_sample(&actual, kind, DVec3::new(blocked, 0.0, 0.0)),
                DVec3::ZERO
            );
            let value = floor_sample(&actual, kind, DVec3::new(exposed, 0.0, 0.0));
            if !sky {
                assert_eq!(value, DVec3::ZERO);
            } else if kind == LightingKind::Ldr {
                assert!(value.x > 100.0, "{value}");
                assert_eq!((value.y, value.z), (0.0, 0.0));
            } else {
                assert!(value.y > 100.0, "{value}");
                assert_eq!((value.x, value.z), (0.0, 0.0));
            }
        }
        outputs.push(actual);
    }
    assert_ne!(
        outputs[0].lumps[bsp::lump::LIGHTING],
        outputs[1].lumps[bsp::lump::LIGHTING]
    );
    for kind in [LightingKind::Ldr, LightingKind::Hdr] {
        let first = outputs[0].world_lights(kind).unwrap();
        let second = outputs[1].world_lights(kind).unwrap();
        assert_ne!(first[0].normal, second[0].normal);
        assert_eq!(first[0].origin, second[0].origin);
        assert_eq!(first[0].intensity, second[0].intensity);
    }
    for index in [
        1, 2, 3, 4, 5, 6, 10, 12, 13, 14, 16, 17, 18, 19, 20, 26, 29, 30, 31, 32, 33, 35,
    ] {
        assert_eq!(
            outputs[0].lumps[index], outputs[1].lumps[index],
            "physical/PVS lump {index}"
        );
    }
}

#[test]
fn hdr_sentinels_scales_zero_ambient_and_soft_spread_follow_existing_root_consumers() {
    for (fields, expected_hdr) in [
        (
            vec![
                ("_lightHDR", "-1 -1 -1 1"),
                ("_lightscaleHDR", "2"),
                ("_ambientHDR", "-1 -1 -1 1"),
                ("_AmbientScaleHDR", "3"),
            ],
            vec![[400.0 / 255.0, 0.0, 0.0], [0.0, 0.0, 120.0 / 255.0]],
        ),
        (
            vec![
                ("_lightHDR", "-1 -1 -1 1"),
                ("_lightscaleHDR", "2"),
                ("_ambient", ""),
                ("_ambientHDR", "-1 -1 -1 1"),
                ("_AmbientScaleHDR", "3"),
            ],
            vec![[400.0 / 255.0, 0.0, 0.0], [600.0 / 255.0, 0.0, 0.0]],
        ),
        (vec![("_ambientHDR", "0")], vec![[0.0, 300.0 / 255.0, 0.0]]),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let game = game(directory.path());
        let mut source = environment("0 0 128", Some("0 0 0"), "-90");
        for (key, value) in fields {
            source.properties.set(key, value);
        }
        write_source(directory.path(), "leaf.vmf", &leaf(source.clone()));
        let mut parent = room(true, true);
        instance(&mut parent, "leaf.vmf", "0 0 0", "0 0 0");
        let context = saved_parent(directory.path(), &parent);
        let expected = literal(&parent, &[source]);
        let actual = successful(compile_saved(
            &parent,
            &game,
            LightingMode::DirectLdrHdr,
            &context,
        ));
        let expected = successful(compiler::compile_with_options(
            &expected,
            &game,
            options(LightingMode::DirectLdrHdr),
        ));
        assert_eq!(actual.to_bytes(), expected.to_bytes());
        assert_eq!(actual.world_lights(LightingKind::Ldr).unwrap().len(), 2);
        assert_eq!(
            actual
                .world_lights(LightingKind::Hdr)
                .unwrap()
                .iter()
                .map(|light| light.intensity)
                .collect::<Vec<_>>(),
            expected_hdr
        );
    }
    let directory = tempfile::tempdir().unwrap();
    let game = game(directory.path());
    let mut hard_source = environment("0 0 128", Some("0 0 0"), "-90");
    hard_source.properties.set("_ambient", "0");
    hard_source.properties.set("_ambientHDR", "0");
    let mut results = Vec::new();
    for spread in ["0", "20"] {
        let mut source = hard_source.clone();
        source.properties.set("SunSpreadAngle", spread);
        write_source(directory.path(), "leaf.vmf", &leaf(source.clone()));
        let mut parent = room(true, true);
        instance(&mut parent, "leaf.vmf", "0 0 0", "0 0 0");
        let context = saved_parent(directory.path(), &parent);
        let expected = literal(&parent, &[source]);
        let actual = successful(compile_saved(
            &parent,
            &game,
            LightingMode::DirectLdrHdr,
            &context,
        ));
        let expected = successful(compiler::compile_with_options(
            &expected,
            &game,
            options(LightingMode::DirectLdrHdr),
        ));
        assert_eq!(actual.to_bytes(), expected.to_bytes());
        results.push(actual);
    }
    for kind in [LightingKind::Ldr, LightingKind::Hdr] {
        assert_eq!(floor_sample(&results[0], kind, DVec3::ZERO), DVec3::ZERO);
        let exposed = floor_sample(&results[0], kind, DVec3::new(128.0, 0.0, 0.0));
        let soft = floor_sample(&results[1], kind, DVec3::ZERO);
        let component = if kind == LightingKind::Ldr { 0 } else { 1 };
        assert!(
            soft[component] > 0.0 && soft[component] < exposed[component],
            "{kind:?}: {soft} / {exposed}"
        );
        assert_eq!(
            floor_sample(&results[1], kind, DVec3::new(128.0, 0.0, 0.0)),
            exposed
        );
        assert_eq!(
            results[0].world_lights(kind).unwrap(),
            results[1].world_lights(kind).unwrap()
        );
    }
}

#[test]
fn fgd_defaults_optional_angles_and_authored_scalar_spelling_remain_intact() {
    let catalog = Catalog::parse(
        r#"
        @BaseClass = Angles [ angles(angle) : "Pitch Yaw Roll" : "0 0 0" ]
        @PointClass base(Angles) = light_environment [
            pitch(integer) : "Pitch" : 0
            _light(color255) : "Brightness" : "255 255 255 200"
            _ambient(color255) : "Ambient" : "255 255 255 20"
            _lightHDR(color255) : "Brightness HDR" : "-1 -1 -1 1"
            _lightscaleHDR(float) : "Brightness scale HDR" : 1
            _ambientHDR(color255) : "Ambient HDR" : "-1 -1 -1 1"
            _AmbientScaleHDR(float) : "Ambient scale HDR" : 1
            SunSpreadAngle(float) : "Sun spread" : 0
        ]
    "#,
    )
    .unwrap();
    let defaults = catalog.class("light_environment").unwrap().defaults();
    assert_eq!(
        defaults.get("_AmbientScaleHDR").map(String::as_str),
        Some("1")
    );
    assert_eq!(
        defaults.get("SunSpreadAngle").map(String::as_str),
        Some("0")
    );
    for pose in ["FGD default", "absent angles", "finite wide angles"] {
        let directory = tempfile::tempdir().unwrap();
        let game = game(directory.path());
        let mut source = Entity::new(2, "light_environment");
        for (key, value) in &defaults {
            source.properties.set(key, value);
        }
        source.properties.set("origin", "0 0 128");
        source.properties.set("pitch", "-90");
        for key in [
            "targetname",
            "target",
            "parentname",
            "name",
            "globalname",
            "killtarget",
            "lightingorigin",
            "pattern",
        ] {
            source.properties.set(key, "");
        }
        for key in ["style", "defaultstyle", "spawnflags", "angle"] {
            source.properties.set(key, "0");
        }
        if pose == "absent angles" {
            source.properties.remove("angles");
        } else if pose == "finite wide angles" {
            source.properties.set("angles", "0 20000 0");
        }
        write_source(directory.path(), "leaf.vmf", &leaf(source.clone()));
        let mut parent = room(true, false);
        instance(&mut parent, "leaf.vmf", "0 0 0", "0 0 0");
        let context = saved_parent(directory.path(), &parent);
        let prepared = resolve(&parent, &context);
        let generated = prepared
            .vmf()
            .entities
            .iter()
            .find(|entity| entity.classname() == Some("light_environment"))
            .unwrap();
        assert_eq!(
            generated.properties.get("angles"),
            source.properties.get("angles")
        );
        for (key, value) in source.properties.iter() {
            assert_eq!(generated.properties.get(key), Some(value));
        }
        let expected = literal(&parent, &[source]);
        let actual = successful(compile_saved(
            &parent,
            &game,
            LightingMode::DirectLdrHdr,
            &context,
        ));
        let expected = successful(compiler::compile_with_options(
            &expected,
            &game,
            options(LightingMode::DirectLdrHdr),
        ));
        assert_eq!(actual.to_bytes(), expected.to_bytes());
    }
}

#[test]
fn invalid_child_pose_static_behavior_and_either_channel_refuse_in_every_mode() {
    for (key, value, reason) in [
        ("pitch", "91", "pitch must be between -90 and 90"),
        ("pitch", "NaN", "pitch must be finite"),
        ("origin", "17000 0 128", "origin"),
        ("angles", "NaN 0 0", "angles"),
        ("angle", "-1", "angle"),
        ("targetname", " ", "empty targetname"),
        ("lightingorigin", "external", "empty lightingorigin"),
        ("pattern", "a", "pattern"),
        ("style", "1", "style"),
        ("spawnflags", "1", "spawnflags"),
        ("_quadratic_attn", "1", "unsupported"),
        ("_cone", "30", "unsupported"),
        ("_light", "NaN", "light"),
        ("_ambient", "invalid", "light"),
        ("_lightHDR", "invalid", "HDR"),
        ("_ambientHDR", "invalid", "HDR"),
        ("_AmbientScaleHDR", "-1", "HDR"),
        ("SunSpreadAngle", "91", "SunSpreadAngle"),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let game = game(directory.path());
        let mut source = environment("0 0 128", Some("0 0 0"), "-45");
        source.properties.set(key, value);
        let original = source.id;
        write_source(directory.path(), "leaf.vmf", &leaf(source));
        let mut parent = room(true, false);
        let owner = instance(&mut parent, "leaf.vmf", "0 0 0", "0 0 0");
        let context = saved_parent(directory.path(), &parent);
        let modes = if matches!(key, "_light" | "_lightHDR") {
            MODES.as_slice()
        } else {
            &MODES[..1]
        };
        for &mode in modes {
            let output = compile_saved(&parent, &game, mode, &context);
            assert_error(&output, owner, original, reason);
            assert_preserved(&output, directory.path());
        }
    }
    for mutation in [
        "missing pitch",
        "missing origin",
        "pose casing",
        "duplicate scalar",
        "zero ambiguity",
        "I/O",
        "opaque",
        "brush",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let game = game(directory.path());
        let mut child = leaf(environment("0 0 128", Some("0 0 0"), "-45"));
        let reason = match mutation {
            "missing pitch" => {
                child.entities[0].properties.remove("pitch");
                "pitch must be finite"
            }
            "missing origin" => {
                child.entities[0].properties.remove("origin");
                "origin"
            }
            "pose casing" => {
                child.entities[0].properties.remove("origin");
                child.entities[0].properties.set("Origin", "0 0 128");
                "canonical"
            }
            "duplicate scalar" => {
                child.entities[0]
                    .properties
                    .0
                    .push(("_LIGHT".into(), "255".into()));
                "repeats"
            }
            "zero ambiguity" => {
                child.entities[0].properties.set("pitch", "0");
                child.entities[0].properties.set("angles", "10 0 0");
                "zero pitch"
            }
            "I/O" => {
                child.entities[0].connections = Some(vec![source_formats::vmf::Connection {
                    output: "OnUser1".into(),
                    value: "other,Kill,,0,1".into(),
                }]);
                "connections"
            }
            "opaque" => {
                child.entities[0]
                    .extra
                    .push(Node::block("unknown", vec![Node::string("key", "value")]));
                "opaque"
            }
            "brush" => {
                let mut block = Document::empty();
                block
                    .edit("invalid helper solid", |tx| {
                        edit::create_block(
                            tx,
                            DVec3::new(-8.0, -8.0, 16.0),
                            DVec3::new(8.0, 8.0, 32.0),
                            "dev/dev_measuregeneric01",
                        )
                    })
                    .unwrap();
                let mut solid = block.into_vmf().world.solids.remove(0);
                solid.id = 10;
                for (index, side) in solid.sides.iter_mut().enumerate() {
                    side.id = 20 + u32::try_from(index).unwrap();
                }
                child.entities[0].solids.push(solid);
                "brush"
            }
            _ => unreachable!(),
        };
        let original = child.entities[0].id;
        write_source(directory.path(), "leaf.vmf", &child);
        let mut parent = room(true, false);
        let owner = instance(&mut parent, "leaf.vmf", "0 0 0", "0 0 0");
        let context = saved_parent(directory.path(), &parent);
        assert_error(
            &compile_saved(&parent, &game, LightingMode::Fullbright, &context),
            owner,
            original,
            reason,
        );
    }
}

#[test]
fn source_and_intermediate_pose_bounds_are_checked_before_global_region_retention() {
    for (source_origin, inner, outer) in [
        ("17000 0 128", "-1000 0 0", "0 0 0"),
        ("600 0 128", "16000 0 0", "-16000 0 0"),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let game = game(directory.path());
        let source = environment(source_origin, Some("0 0 0"), "-45");
        let original = source.id;
        write_source(directory.path(), "leaf.vmf", &leaf(source));
        let mut middle = Document::empty().into_vmf();
        instance(&mut middle, "leaf.vmf", inner, "0 0 0");
        write_source(directory.path(), "middle.vmf", &middle);
        let mut parent = room(true, false);
        let owner = instance(&mut parent, "middle.vmf", outer, "0 0 0");
        region(&mut parent);
        let context = saved_parent(directory.path(), &parent);
        for mode in MODES {
            let output = compile_saved(&parent, &game, mode, &context);
            assert_error(&output, owner, original, "origin");
            assert!(output.stats.compile_region.is_none());
            assert_preserved(&output, directory.path());
        }
    }
}

#[test]
fn ordinary_root_drafts_keep_their_existing_pose_and_selected_channel_rules() {
    let directory = tempfile::tempdir().unwrap();
    let game = game(directory.path());
    let mut source = room(true, false);
    let mut light = environment("0 0 128", Some("0 0 0"), "-45");
    light.id = next_id(&source);
    light.properties.remove("pitch");
    light.properties.set("_lightHDR", "invalid");
    let id = light.id;
    source.entities.push(light);
    let before = source.clone();
    for mode in [LightingMode::Fullbright, LightingMode::DirectLdr] {
        let output = compiler::compile_with_options(&source, &game, options(mode));
        assert!(output.stats.instances.is_none());
        successful(output);
    }
    let output =
        compiler::compile_with_options(&source, &game, options(LightingMode::DirectLdrHdr));
    assert!(output.report.has_errors() && output.bsp.is_none() && output.leak.is_none());
    assert!(output.report.diagnostics.iter().any(|diagnostic| {
        diagnostic.object == Some(format!("entity:{id}")) && diagnostic.message.contains("HDR")
    }));
    assert_preserved(&output, directory.path());
    assert_eq!(source, before);
}

fn region(source: &mut Vmf) {
    source.cordon = Some(Node::block(
        "cordon",
        vec![
            Node::string("active", "1"),
            Node::string("mins", "-128 -128 4"),
            Node::string("maxs", "128 128 252"),
        ],
    ));
}

#[test]
fn global_root_child_repeated_and_nested_suns_keep_existing_bake_and_fullbright_rules() {
    for arrangement in ["root child", "repeated", "nested repeated"] {
        let directory = tempfile::tempdir().unwrap();
        let game = game(directory.path());
        let source = environment("0 0 128", Some("0 0 0"), "-45");
        let original = source.id;
        write_source(directory.path(), "leaf.vmf", &leaf(source.clone()));
        let mut parent = room(true, false);
        let (owner, expected_count) = match arrangement {
            "root child" => {
                let mut root = source.clone();
                root.id = next_id(&parent);
                parent.entities.push(root);
                (instance(&mut parent, "leaf.vmf", "0 0 0", "0 0 0"), 1)
            }
            "repeated" => {
                instance(&mut parent, "leaf.vmf", "0 0 0", "0 0 0");
                (instance(&mut parent, "leaf.vmf", "600 0 0", "0 0 0"), 2)
            }
            "nested repeated" => {
                let mut middle = Document::empty().into_vmf();
                instance(&mut middle, "leaf.vmf", "0 0 0", "0 0 0");
                instance(&mut middle, "leaf.vmf", "600 0 0", "0 0 0");
                write_source(directory.path(), "middle.vmf", &middle);
                (instance(&mut parent, "middle.vmf", "0 0 0", "0 0 0"), 2)
            }
            _ => unreachable!(),
        };
        region(&mut parent);
        let context = saved_parent(directory.path(), &parent);
        let output = compile_saved(&parent, &game, LightingMode::Fullbright, &context);
        assert_eq!(
            output.stats.instances.as_ref().unwrap().environment_lights,
            expected_count
        );
        let bsp = successful(output);
        assert!(bsp.world_lights(LightingKind::Ldr).unwrap().is_empty());
        for mode in [LightingMode::DirectLdr, LightingMode::DirectLdrHdr] {
            let output = compile_saved(&parent, &game, mode, &context);
            assert_error(&output, owner, original, "light_environment");
            assert_preserved(&output, directory.path());
        }
    }
}

#[test]
fn global_marker_survives_regions_while_full_map_leaks_remain_source_owned() {
    let directory = tempfile::tempdir().unwrap();
    let game = game(directory.path());
    let source = environment("600 0 128", Some("0 0 0"), "-90");
    write_source(directory.path(), "leaf.vmf", &leaf(source.clone()));
    let mut parent = room(true, false);
    let owner = instance(&mut parent, "leaf.vmf", "0 0 0", "0 0 0");
    region(&mut parent);
    let context = saved_parent(directory.path(), &parent);
    let expected = literal(&parent, &[source]);
    for mode in MODES {
        let output = compile_saved(&parent, &game, mode, &context);
        assert_eq!(
            output.stats.instances.as_ref().unwrap().environment_lights,
            1
        );
        assert!(output.stats.compile_region.is_some());
        let actual = successful(output);
        let expected = successful(compiler::compile_with_options(
            &expected,
            &game,
            options(mode),
        ));
        assert_eq!(actual.to_bytes(), expected.to_bytes());
    }
    parent.cordon = None;
    let context = saved_parent(directory.path(), &parent);
    let output = compile_saved(&parent, &game, LightingMode::DirectLdrHdr, &context);
    assert!(output.bsp.is_none());
    let leak = output.leak.as_ref().unwrap();
    assert_eq!(leak.entity, owner);
    assert_eq!(leak.points.first(), Some(&DVec3::new(600.0, 0.0, 128.0)));
    assert!(leak.points.len() > 1 && leak.points.iter().all(|point| point.is_finite()));
    let path = directory.path().join("previous.bsp");
    std::fs::write(&path, b"previous BSP").unwrap();
    assert!(!output.write(&path).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), b"previous BSP");
    assert!(
        !std::fs::read(path.with_extension("lin"))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn hidden_environment_has_zero_omitted_count_and_no_output_effect() {
    let directory = tempfile::tempdir().unwrap();
    let game = game(directory.path());
    let mut child = leaf(environment("17000 0 128", Some("NaN 0 0"), "NaN"));
    child.entities[0].hidden = true;
    write_source(directory.path(), "leaf.vmf", &child);
    let mut parent = room(true, false);
    instance(&mut parent, "leaf.vmf", "0 0 0", "0 0 0");
    let context = saved_parent(directory.path(), &parent);
    let prepared = resolve(&parent, &context);
    assert_eq!(prepared.stats().unwrap().environment_lights, 0);
    assert!(
        !serde_json::to_value(prepared.stats().unwrap())
            .unwrap()
            .as_object()
            .unwrap()
            .contains_key("environment_lights")
    );
    let expected = literal(&parent, &[]);
    for mode in MODES {
        let actual = successful(compile_saved(&parent, &game, mode, &context));
        let expected = successful(compiler::compile_with_options(
            &expected,
            &game,
            options(mode),
        ));
        assert_eq!(actual.to_bytes(), expected.to_bytes());
    }
}

#[test]
fn transitive_sources_history_cancellation_and_stale_publication_preserve_custody() {
    let directory = tempfile::tempdir().unwrap();
    let game = game(directory.path());
    let child = leaf(environment("0 0 128", Some("0 0 0"), "-90"));
    let child_bytes = write_source(directory.path(), "leaf.vmf", &child);
    let mut middle = Document::empty().into_vmf();
    instance(&mut middle, "leaf.vmf", "0 0 0", "0 0 0");
    let middle_bytes = write_source(directory.path(), "middle.vmf", &middle);
    let mut parent = room(true, false);
    let owner = instance(&mut parent, "middle.vmf", "0 0 0", "0 0 0");
    let mut document = Document::new(parent);
    document
        .edit("assembly name", |tx| {
            tx.entity_mut(owner)?
                .properties
                .set("targetname", "assembly");
            Ok(())
        })
        .unwrap();
    let before = document.vmf().clone();
    let history = document
        .undo_labels()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let context = saved_parent(directory.path(), document.vmf());
    let parent_bytes = std::fs::read(directory.path().join("parent.vmf")).unwrap();
    let output = compile_saved(document.vmf(), &game, LightingMode::DirectLdrHdr, &context);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.bsp.is_some() && output.leak.is_none());
    assert_eq!(
        output
            .stats
            .instances
            .as_ref()
            .unwrap()
            .dependencies
            .iter()
            .map(|dependency| dependency.path.as_str())
            .collect::<Vec<_>>(),
        ["leaf.vmf", "middle.vmf"]
    );
    let path = directory.path().join("previous.bsp");
    std::fs::write(&path, b"previous BSP").unwrap();
    std::fs::write(path.with_extension("lin"), b"previous LIN").unwrap();
    let cancelled = Control::default();
    cancelled.cancel().unwrap();
    assert!(output.write_controlled(&path, &cancelled).is_err());
    for (name, bytes) in [
        ("leaf.vmf", &child_bytes),
        ("middle.vmf", &middle_bytes),
        ("parent.vmf", &parent_bytes),
    ] {
        let fresh_context = saved_parent(directory.path(), document.vmf());
        let output = compile_saved(
            document.vmf(),
            &game,
            LightingMode::DirectLdrHdr,
            &fresh_context,
        );
        assert!(!output.report.has_errors(), "{:?}", output.report);
        let staged = output
            .prepare_write_controlled(&path, &Control::default())
            .unwrap()
            .unwrap();
        let mut changed = bytes.to_vec();
        changed.extend_from_slice(b"\n");
        std::fs::write(directory.path().join(name), changed).unwrap();
        assert!(staged.commit(&Control::default()).is_err(), "{name}");
        assert!(output.write(&path).is_err(), "{name}");
        assert_eq!(std::fs::read(&path).unwrap(), b"previous BSP");
        assert_eq!(
            std::fs::read(path.with_extension("lin")).unwrap(),
            b"previous LIN"
        );
        std::fs::write(directory.path().join(name), bytes).unwrap();
    }
    let cancelled_output = compiler::compile_with_source_controlled(
        document.vmf(),
        &game,
        options(LightingMode::DirectLdrHdr),
        None,
        Some(&context),
        &cancelled,
    );
    assert!(cancelled_output.bsp.is_none() && cancelled_output.leak.is_none());
    assert!(
        cancelled_output
            .report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("cancelled"))
    );
    assert_eq!(document.vmf(), &before);
    assert_eq!(
        document.undo_labels().collect::<Vec<_>>(),
        history.iter().map(String::as_str).collect::<Vec<_>>()
    );
    assert_eq!(document.redo_labels().count(), 0);
    for (name, bytes) in [
        ("leaf.vmf", child_bytes),
        ("middle.vmf", middle_bytes),
        ("parent.vmf", parent_bytes),
    ] {
        assert_eq!(std::fs::read(directory.path().join(name)).unwrap(), bytes);
    }
}

#[test]
fn repeated_environment_helpers_share_the_existing_object_budget_before_fullbright_compile() {
    let directory = tempfile::tempdir().unwrap();
    let game = game(directory.path());
    let mut child = Document::empty().into_vmf();
    for index in 0..1024 {
        let mut source = environment("0 0 128", Some("0 0 0"), "-90");
        source.id = index + 2;
        child.entities.push(source);
    }
    write_source(directory.path(), "leaf.vmf", &child);
    let mut parent = room(true, false);
    let owners = (0..17)
        .map(|_| instance(&mut parent, "leaf.vmf", "0 0 0", "0 0 0"))
        .collect::<Vec<_>>();
    let overflow_owner = owners[15];
    let context = saved_parent(directory.path(), &parent);
    let output = compile_saved(&parent, &game, LightingMode::Fullbright, &context);
    assert!(output.bsp.is_none() && output.leak.is_none());
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .any(
                |diagnostic| diagnostic.object == Some(format!("entity:{overflow_owner}"))
                    && diagnostic.message.contains("combined objects")
            ),
        "{:?}",
        output.report
    );
    assert_preserved(&output, directory.path());
}
