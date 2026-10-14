mod common;

use compiler::{LightingMode, Options};
use glam::DVec3;
use map_core::{Document, Plane, edit};
use source_formats::bsp::{self, Bsp, Face, LightingKind, TexInfo};
use source_formats::vmf::Vmf;

fn add_light(document: &mut Document, classname: &str, fields: &[(&str, &str)]) {
    let mut properties = vec![
        ("_constant_attn".into(), "1".into()),
        ("_quadratic_attn".into(), "0".into()),
    ];
    properties.extend(
        fields
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string())),
    );
    document
        .edit("light", |tx| {
            edit::create_entity(
                tx,
                classname,
                Some(DVec3::new(0.0, 0.0, 128.0)),
                &properties,
                Vec::new(),
            )
        })
        .unwrap();
}

fn compile(vmf: &Vmf) -> compiler::Output {
    compiler::compile_with_options(
        vmf,
        &common::empty_game(),
        Options {
            lighting: LightingMode::DirectLdr,
            ..Default::default()
        },
    )
}

fn floor_channels(bsp: &Bsp) -> Vec<(u8, DVec3, DVec3)> {
    let planes = bsp.records::<bsp::Plane>().unwrap();
    let infos = bsp.records::<TexInfo>().unwrap();
    let lighting = bsp.lighting(LightingKind::Ldr).unwrap().unwrap();
    for face in bsp.records::<Face>().unwrap() {
        let plane = planes[face.plane as usize];
        if plane.normal[2] < 0.99 || plane.dist.abs() > 0.01 {
            continue;
        }
        let info = &infos[face.texinfo as usize];
        let frame = compiler::lighting::LuxelFrame::new(
            Plane {
                normal: DVec3::Z,
                dist: 0.0,
            },
            info,
            face.lightmap_mins,
            face.lightmap_size,
        )
        .unwrap();
        let [width, height] = frame.dimensions();
        for t in 0..height {
            for s in 0..width {
                if frame.position(s, t).unwrap().length() > 1e-6 {
                    continue;
                }
                let light = lighting.face(&face, info).unwrap().unwrap();
                return light
                    .styles()
                    .iter()
                    .enumerate()
                    .map(|(index, &style)| {
                        (
                            style,
                            DVec3::from_array(
                                light.samples(index, 0).unwrap()[t * width + s].linear(),
                            ),
                            DVec3::from_array(light.average(index).unwrap().linear()),
                        )
                    })
                    .collect();
            }
        }
    }
    panic!("No floor receiver at the origin")
}

#[test]
fn named_and_builtin_styles_keep_separate_native_channels_and_reversed_averages() {
    let mut document = Document::new(common::room());
    add_light(
        &mut document,
        "light",
        &[
            ("_light", "255 0 0 100"),
            ("targetname", "red"),
            ("style", "10"),
        ],
    );
    add_light(
        &mut document,
        "light_spot",
        &[
            ("_light", "255 255 255 20"),
            ("targetname", "spot"),
            ("angles", "-90 0 0"),
        ],
    );
    add_light(
        &mut document,
        "light",
        &[("_light", "0 255 0 50"), ("style", "1")],
    );
    add_light(&mut document, "light", &[("_light", "0 0 255 40")]);
    let source = document.vmf().clone();
    let output = compile(&source);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert_eq!(output.stats.switchable_light_groups, 2);
    assert_eq!(output.stats.light_styles, 4);
    assert!(
        output.stats.styled_faces > 0
            && output.stats.light_style_maps > output.stats.lighting_faces
    );
    let bsp = output.bsp.unwrap();
    let channels = floor_channels(&bsp);
    assert_eq!(
        channels.iter().map(|channel| channel.0).collect::<Vec<_>>(),
        [0, 1, 32, 33]
    );
    for ((_, sample, average), expected) in channels.iter().zip([
        DVec3::new(0.0, 0.0, 40.0),
        DVec3::new(0.0, 50.0, 0.0),
        DVec3::new(100.0, 0.0, 0.0),
        DVec3::splat(20.0),
    ]) {
        assert_eq!(*sample, expected);
        assert_eq!(average.cmpeq(DVec3::ZERO), expected.cmpeq(DVec3::ZERO));
        assert!(average.min_element() >= 0.0 && average.max_element() <= expected.max_element());
    }
    let world = bsp.world_lights(LightingKind::Ldr).unwrap();
    assert_eq!(
        world.iter().map(|light| light.style).collect::<Vec<_>>(),
        [32, 33, 1, 0]
    );
    assert_eq!(world[1].kind, bsp::world_light_kind::SPOT);
    let entities = bsp.entities().unwrap();
    assert!(entities.contains("\"style\" \"32\"") && entities.contains("\"defaultstyle\" \"10\""));
    assert_eq!(*document.vmf(), source);
    assert!(bsp.to_bytes() == compile(&source).bsp.unwrap().to_bytes());
}

#[test]
fn named_groups_share_style_and_bounce_while_conflicting_states_reject() {
    let mut document = Document::new(common::room());
    for brightness in ["255 0 0 100", "255 0 0 80"] {
        add_light(
            &mut document,
            "light",
            &[
                ("_light", brightness),
                ("targetname", "shared"),
                ("pattern", "am"),
                ("spawnflags", "1"),
            ],
        );
    }
    let source = document.vmf().clone();
    let result = compile(&source);
    assert!(!result.report.has_errors(), "{:?}", result.report);
    assert_eq!(result.stats.switchable_light_groups, 1);
    assert_eq!(
        floor_channels(&result.bsp.unwrap())[1].1,
        DVec3::new(180.0, 0.0, 0.0)
    );
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("guarded.bsp");
    std::fs::write(&path, b"previous BSP").unwrap();
    for (key, value) in [
        ("pattern", "az"),
        ("spawnflags", "0"),
        ("pattern", "A"),
        ("style", "32"),
        ("style", "0.5"),
    ] {
        let mut bad = source.clone();
        bad.entities.last_mut().unwrap().properties.set(key, value);
        let result = compile(&bad);
        assert!(result.report.has_errors(), "{key} {value}");
        assert!(!result.write(&path).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), b"previous BSP");
    }
    let result = compiler::compile_with_options(
        &source,
        &common::empty_game(),
        Options {
            lighting: LightingMode::DirectLdr,
            bounces: 1,
            ..Default::default()
        },
    );
    assert!(!result.report.has_errors(), "{:?}", result.report);
    assert!(result.stats.bounce_samples > 0);
    assert!(floor_channels(&result.bsp.unwrap())[1].1.x > 180.0);
    assert_eq!(*document.vmf(), source);
}

#[test]
fn face_and_named_group_limits_reject_before_publication() {
    let mut document = Document::new(common::room());
    for name in ["one", "two", "three"] {
        add_light(
            &mut document,
            "light",
            &[("_light", "255 255 255 20"), ("targetname", name)],
        );
    }
    let boundary = compile(document.vmf());
    assert!(!boundary.report.has_errors(), "{:?}", boundary.report);
    assert_eq!(floor_channels(&boundary.bsp.unwrap()).len(), 4);
    add_light(
        &mut document,
        "light",
        &[("_light", "255 255 255 20"), ("targetname", "four")],
    );
    let result = compile(document.vmf());
    assert!(result.report.has_errors() && result.bsp.is_none());
    assert!(format!("{:?}", result.report).contains("four light styles"));
    let mut document = Document::new(common::room());
    for index in 0..32 {
        add_light(
            &mut document,
            "light",
            &[
                ("_light", "255 255 255 0"),
                ("targetname", &format!("group_{index}")),
            ],
        );
    }
    let boundary = compile(document.vmf());
    assert!(!boundary.report.has_errors(), "{:?}", boundary.report);
    assert_eq!(boundary.stats.switchable_light_groups, 32);
    assert_eq!(
        boundary
            .bsp
            .unwrap()
            .world_lights(LightingKind::Ldr)
            .unwrap()
            .last()
            .unwrap()
            .style,
        63
    );
    add_light(
        &mut document,
        "light",
        &[("_light", "255 255 255 0"), ("targetname", "overflow")],
    );
    let result = compile(document.vmf());
    assert!(result.report.has_errors() && result.bsp.is_none());
    assert!(format!("{:?}", result.report).contains("32 named light groups"));
}
