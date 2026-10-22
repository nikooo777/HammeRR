mod common;
#[path = "common/static_prop_model.rs"]
mod static_prop_model;

use std::collections::BTreeSet;
use std::path::Path;

use compiler::instances::SourceContext;
use compiler::{Control, LightingMode, Options};
use map_core::Document;
use source_formats::bsp::{self, Bsp, LightingKind, TexInfo};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::keyvalues::Node;
use source_formats::vmf::{Entity, Vmf};

const OWNER: u32 = 200;

fn light(id: u32, class: &str, origin: &str, style: &str, ldr: &str, hdr: &str) -> Entity {
    let mut entity = Entity::new(id, class);
    for (key, value) in [
        ("origin", origin),
        ("style", style),
        ("_light", ldr),
        ("_lightHDR", hdr),
        ("_lightscaleHDR", "1"),
        ("_constant_attn", "1"),
        ("_linear_attn", "0"),
        ("_quadratic_attn", "0"),
    ] {
        entity.properties.set(key, value);
    }
    if class == "light_spot" {
        for (key, value) in [
            ("angles", "0 25 0"),
            ("pitch", "-60"),
            ("_inner_cone", "45"),
            ("_cone", "70"),
            ("_exponent", "1"),
        ] {
            entity.properties.set(key, value);
        }
    }
    entity
}

fn instance(id: u32, file: &str, origin: &str) -> Entity {
    let mut entity = Entity::new(id, "func_instance");
    for (key, value) in [("file", file), ("origin", origin), ("angles", "0 0 0")] {
        entity.properties.set(key, value);
    }
    entity
}

fn write(directory: &Path, name: &str, vmf: &Vmf) -> Vec<u8> {
    let bytes = vmf.write(&Default::default()).into_bytes();
    std::fs::write(directory.join(name), &bytes).unwrap();
    bytes
}

struct Fixture {
    directory: tempfile::TempDir,
    game: GameFs,
    root: Vmf,
    child: Vmf,
}

impl Fixture {
    fn new(lights: Vec<Entity>, nested: bool, bump: bool) -> Self {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(directory.path().join("materials/dev")).unwrap();
        std::fs::create_dir_all(directory.path().join("materials/tools")).unwrap();
        std::fs::write(
            directory
                .path()
                .join("materials/dev/dev_measuregeneric01.vmt"),
            if bump {
                "LightmappedGeneric { $bumpmap dev/normal }"
            } else {
                "LightmappedGeneric {}"
            },
        )
        .unwrap();
        std::fs::write(
            directory.path().join("materials/tools/toolsblack.vmt"),
            "UnlitGeneric {}",
        )
        .unwrap();
        let mut game = common::empty_game();
        game.mounts.push(MountedPath {
            ids: vec!["game".into()],
            mount: Mount::Directory(directory.path().to_owned()),
        });
        let mut root = common::room();
        for side in root
            .world
            .solids
            .iter_mut()
            .flat_map(|solid| &mut solid.sides)
        {
            side.lightmap_scale = 128;
        }
        root.entities.push(light(
            100,
            "light",
            "0 0 128",
            "0",
            "0 255 0 20",
            "0 0 255 30",
        ));
        root.entities.push(if nested {
            let mut middle = Document::empty().into_vmf();
            middle.entities = vec![
                instance(20, "leaf.vmf", "16 -8 4"),
                instance(21, "leaf.vmf", "-64 48 12"),
            ];
            write(directory.path(), "middle.vmf", &middle);
            instance(OWNER, "middle.vmf", "24 16 0")
        } else {
            instance(OWNER, "leaf.vmf", "0 0 0")
        });
        let mut child = Document::empty().into_vmf();
        child.entities = lights;
        Self {
            directory,
            game,
            root,
            child,
        }
    }

    fn save(&self) -> SourceContext {
        write(self.directory.path(), "leaf.vmf", &self.child);
        write(self.directory.path(), "parent.vmf", &self.root);
        SourceContext::capture(
            &self.directory.path().join("parent.vmf"),
            &Control::default(),
        )
        .unwrap()
    }

    fn compile(&self, source: &SourceContext, options: Options) -> compiler::Output {
        compiler::compile_with_source_controlled(
            &self.root,
            &self.game,
            options,
            None,
            Some(source),
            &Control::default(),
        )
    }

    fn literal(&self, lights: Vec<Entity>) -> Vmf {
        let mut literal = self.root.clone();
        literal.entities.retain(|entity| entity.id != OWNER);
        literal.entities.extend(lights);
        literal.validate_ids().unwrap();
        literal
    }
}

fn options(lighting: LightingMode) -> Options {
    Options {
        lighting,
        ..Default::default()
    }
}

fn successful(output: &compiler::Output) -> &Bsp {
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.leak.is_none());
    output.bsp.as_ref().unwrap()
}

fn exact_literal(actual: &compiler::Output, literal: &Vmf, game: &GameFs, options: Options) {
    let expected = compiler::compile_with_options(literal, game, options);
    let actual = successful(actual).to_bytes();
    let expected = successful(&expected).to_bytes();
    if actual != expected {
        let directory = tempfile::Builder::new()
            .prefix("767-literal-failure-")
            .tempdir()
            .unwrap()
            .keep();
        std::fs::write(directory.join("actual.bsp"), actual).unwrap();
        std::fs::write(directory.join("literal.bsp"), expected).unwrap();
        write(&directory, "literal.vmf", literal);
        panic!("Complete BSP literal mismatch for {options:?}; retained at {directory:?}");
    }
}

fn channels(bsp: &Bsp, kind: LightingKind, expected: &[u8], maps: usize) {
    let infos = bsp.records::<TexInfo>().unwrap();
    let lighting = bsp.lighting(kind).unwrap().unwrap();
    let mut seen = BTreeSet::new();
    let mut complete = false;
    for face in bsp.lighting_faces(kind).unwrap() {
        let Some(light) = lighting.face(&face, &infos[face.texinfo as usize]).unwrap() else {
            continue;
        };
        assert_eq!(light.maps(), maps);
        assert!(
            light
                .styles()
                .windows(2)
                .all(|styles| styles[0] < styles[1])
        );
        assert!(light.styles().iter().all(|style| expected.contains(style)));
        complete |= light.styles() == expected;
        seen.extend(light.styles().iter().copied());
    }
    assert!(complete, "No {kind:?} receiver has {expected:?}");
    assert_eq!(seen.into_iter().collect::<Vec<_>>(), expected);
}

fn energy(bsp: &Bsp, kind: LightingKind, style: u8, direction: usize) -> f64 {
    let infos = bsp.records::<TexInfo>().unwrap();
    let lighting = bsp.lighting(kind).unwrap().unwrap();
    bsp.lighting_faces(kind)
        .unwrap()
        .iter()
        .filter_map(|face| lighting.face(face, &infos[face.texinfo as usize]).unwrap())
        .filter_map(|light| {
            let index = light.styles().iter().position(|&value| value == style)?;
            Some(
                light
                    .samples(index, direction)
                    .unwrap()
                    .iter()
                    .map(|sample| sample.linear().into_iter().sum::<f64>())
                    .sum::<f64>(),
            )
        })
        .sum()
}

#[test]
fn translated_nested_appearances_match_complete_literals_in_all_lighting_modes() {
    let mut fixture = Fixture::new(
        vec![
            light(10, "light", "-48 16 112", "1", "255 0 0 60", "0 255 0 120"),
            light(
                11,
                "light_spot",
                "32 -16 160",
                "2",
                "0 0 255 80",
                "255 0 0 140",
            ),
        ],
        true,
        false,
    );
    fixture.root.entities.insert(
        3,
        light(101, "light", "0 0 144", "1", "255 0 0 40", "0 255 0 50"),
    );
    let literal = fixture.literal(vec![
        light(201, "light", "-8 24 116", "1", "255 0 0 60", "0 255 0 120"),
        light(
            202,
            "light_spot",
            "72 -8 164",
            "2",
            "0 0 255 80",
            "255 0 0 140",
        ),
        light(203, "light", "-88 80 124", "1", "255 0 0 60", "0 255 0 120"),
        light(
            204,
            "light_spot",
            "-8 48 172",
            "2",
            "0 0 255 80",
            "255 0 0 140",
        ),
    ]);
    let source = fixture.save();
    let before = fixture.root.clone();
    let saved = ["parent.vmf", "middle.vmf", "leaf.vmf"]
        .map(|name| std::fs::read(fixture.directory.path().join(name)).unwrap());
    for mode in [
        LightingMode::Fullbright,
        LightingMode::DirectLdr,
        LightingMode::DirectLdrHdr,
    ] {
        let output = fixture.compile(&source, options(mode));
        exact_literal(&output, &literal, &fixture.game, options(mode));
        let stats = output.stats.instances.as_ref().unwrap();
        assert_eq!(
            (
                stats.instances,
                stats.files,
                stats.max_expansion_depth,
                stats.point_lights,
                stats.spot_lights
            ),
            (3, 2, 2, 2, 2)
        );
        assert_eq!(output.stats.switchable_light_groups, 0);
        let bsp = successful(&output);
        assert!(!bsp.entities().unwrap().contains("func_instance"));
        if mode == LightingMode::Fullbright {
            continue;
        }
        assert_eq!(output.stats.light_styles, 3);
        for kind in [LightingKind::Ldr, LightingKind::Hdr] {
            if kind == LightingKind::Hdr && mode != LightingMode::DirectLdrHdr {
                continue;
            }
            channels(bsp, kind, &[0, 1, 2], 1);
            let world = bsp.world_lights(kind).unwrap();
            assert_eq!(
                world.iter().map(|light| light.style).collect::<Vec<_>>(),
                [0, 1, 1, 2, 1, 2]
            );
            assert_eq!(
                world.iter().map(|light| light.origin).collect::<Vec<_>>(),
                [
                    [0.0, 0.0, 128.0],
                    [0.0, 0.0, 144.0],
                    [-8.0, 24.0, 116.0],
                    [72.0, -8.0, 164.0],
                    [-88.0, 80.0, 124.0],
                    [-8.0, 48.0, 172.0]
                ]
            );
            for (index, light) in world.iter().enumerate() {
                assert_eq!(light.owner, -1);
                assert_eq!(
                    light.kind,
                    if [3, 5].contains(&index) {
                        bsp::world_light_kind::SPOT
                    } else {
                        bsp::world_light_kind::POINT
                    }
                );
                let color = match (kind, light.style) {
                    (LightingKind::Ldr, 0) | (LightingKind::Hdr, 1) => 1,
                    (LightingKind::Ldr, 1) | (LightingKind::Hdr, 2) => 0,
                    _ => 2,
                };
                assert!(light.intensity[color] > 0.0);
                assert!(
                    (0..3)
                        .filter(|&axis| axis != color)
                        .all(|axis| light.intensity[axis] == 0.0)
                );
            }
        }
        if mode == LightingMode::DirectLdrHdr {
            assert_ne!(
                bsp.lump(bsp::lump::LIGHTING).data,
                bsp.lump(bsp::lump::LIGHTING_HDR).data
            );
        }
    }
    assert_eq!(fixture.root, before);
    for (name, bytes) in ["parent.vmf", "middle.vmf", "leaf.vmf"]
        .into_iter()
        .zip(saved)
    {
        assert_eq!(
            std::fs::read(fixture.directory.path().join(name)).unwrap(),
            bytes
        );
    }
}

#[test]
fn normal_and_last_offered_appearance_keep_literal_identity_and_global_channels() {
    for style in ["0", "11"] {
        let fixture = Fixture::new(
            vec![
                light(10, "light", "0 0 128", style, "255 0 0 60", "0 255 0 120"),
                light(
                    11,
                    "light_spot",
                    "32 0 160",
                    style,
                    "0 0 255 80",
                    "255 0 0 140",
                ),
            ],
            false,
            false,
        );
        let literal = fixture.literal(vec![
            light(201, "light", "0 0 128", style, "255 0 0 60", "0 255 0 120"),
            light(
                202,
                "light_spot",
                "32 0 160",
                style,
                "0 0 255 80",
                "255 0 0 140",
            ),
        ]);
        let source = fixture.save();
        let options = options(LightingMode::DirectLdrHdr);
        let output = fixture.compile(&source, options);
        exact_literal(&output, &literal, &fixture.game, options);
        assert_eq!(output.stats.switchable_light_groups, 0);
        for kind in [LightingKind::Ldr, LightingKind::Hdr] {
            channels(
                successful(&output),
                kind,
                if style == "0" { &[0] } else { &[0, 11] },
                1,
            );
            assert_eq!(
                successful(&output)
                    .world_lights(kind)
                    .unwrap()
                    .iter()
                    .map(|light| light.style)
                    .collect::<Vec<_>>(),
                [
                    0,
                    style.parse::<i32>().unwrap(),
                    style.parse::<i32>().unwrap()
                ]
            );
        }
    }
}

fn previous(directory: &Path) -> std::path::PathBuf {
    let path = directory.join("previous.bsp");
    std::fs::write(&path, b"previous BSP").unwrap();
    std::fs::write(path.with_extension("lin"), b"previous LIN").unwrap();
    path
}

fn preserved(path: &Path) {
    assert_eq!(std::fs::read(path).unwrap(), b"previous BSP");
    assert_eq!(
        std::fs::read(path.with_extension("lin")).unwrap(),
        b"previous LIN"
    );
}

fn child_error(output: &compiler::Output, original: u32, field: &str) {
    assert!(output.bsp.is_none() && output.leak.is_none() && output.report.has_errors());
    assert!(
        output.report.diagnostics.iter().any(|diagnostic| {
            diagnostic.object == Some(format!("entity:{OWNER}"))
                && diagnostic.message.contains("leaf.vmf")
                && diagnostic.message.contains(field)
                && [
                    format!("Child light {original}"),
                    format!("Child environment light {original}"),
                    format!("child object {original}"),
                ]
                .iter()
                .any(|text| diagnostic.message.contains(text))
        }),
        "{:?}",
        output.report
    );
}

#[test]
fn hidden_and_region_selection_keep_style_policy_and_source_publication_custody() {
    let mut hidden = light(11, "light", "0 0 128", "32", "255 0 0 60", "0 255 0 120");
    hidden.hidden = true;
    let mut fixture = Fixture::new(
        vec![
            light(10, "light", "0 0 128", "11", "255 0 0 60", "0 255 0 120"),
            hidden,
            light(
                12,
                "light_spot",
                "192 0 160",
                "1",
                "0 0 255 80",
                "255 0 0 140",
            ),
        ],
        false,
        false,
    );
    fixture.root.cordon = Some(Node::block(
        "cordon",
        vec![
            Node::string("active", "1"),
            Node::string("mins", "-128 -128 -8"),
            Node::string("maxs", "128 128 264"),
        ],
    ));
    let literal = fixture.literal(vec![
        light(201, "light", "0 0 128", "11", "255 0 0 60", "0 255 0 120"),
        light(
            202,
            "light_spot",
            "192 0 160",
            "1",
            "0 0 255 80",
            "255 0 0 140",
        ),
    ]);
    let source = fixture.save();
    let lit_options = options(LightingMode::DirectLdrHdr);
    let output = fixture.compile(&source, lit_options);
    exact_literal(&output, &literal, &fixture.game, lit_options);
    assert!(output.stats.compile_region.is_some());
    let stats = output.stats.instances.as_ref().unwrap();
    assert_eq!((stats.point_lights, stats.spot_lights), (1, 1));
    for kind in [LightingKind::Ldr, LightingKind::Hdr] {
        channels(successful(&output), kind, &[0, 11], 1);
        assert_eq!(successful(&output).world_lights(kind).unwrap().len(), 2);
    }
    let path = previous(fixture.directory.path());
    let pending = output
        .prepare_write_controlled(&path, &Control::default())
        .unwrap()
        .unwrap();
    let child_bytes = std::fs::read(fixture.directory.path().join("leaf.vmf")).unwrap();
    let mut changed = child_bytes.clone();
    changed.extend_from_slice(b"\n");
    std::fs::write(fixture.directory.path().join("leaf.vmf"), changed).unwrap();
    assert!(pending.commit(&Control::default()).is_err());
    assert!(output.write(&path).is_err());
    preserved(&path);
    std::fs::write(fixture.directory.path().join("leaf.vmf"), child_bytes).unwrap();
    for style in ["12", "31", "32", "1.0", ""] {
        fixture.child.entities[2].properties.set("style", style);
        let source = fixture.save();
        let bytes = std::fs::read(fixture.directory.path().join("leaf.vmf")).unwrap();
        let output = fixture.compile(&source, options(LightingMode::Fullbright));
        child_error(&output, 12, "style");
        assert!(!output.write(&path).unwrap());
        preserved(&path);
        assert_eq!(
            std::fs::read(fixture.directory.path().join("leaf.vmf")).unwrap(),
            bytes
        );
    }
    let mut environment = Entity::new(12, "light_environment");
    for (key, value) in [
        ("origin", "192 0 160"),
        ("style", "1"),
        ("_light", "255 255 255 100"),
        ("_ambient", "255 255 255 20"),
    ] {
        environment.properties.set(key, value);
    }
    fixture.child.entities[2] = environment;
    let source = fixture.save();
    let output = fixture.compile(&source, options(LightingMode::Fullbright));
    child_error(&output, 12, "style");
    assert!(!output.write(&path).unwrap());
    preserved(&path);
}

#[test]
fn styled_bump_bounce_and_four_channel_limit_use_existing_root_consumers() {
    let mut fixture = Fixture::new(
        vec![
            light(10, "light", "0 0 128", "1", "255 0 0 60", "0 255 0 120"),
            light(11, "light", "64 0 128", "2", "0 0 255 80", "255 0 0 140"),
            light(
                12,
                "light_spot",
                "-32 0 160",
                "3",
                "0 255 0 100",
                "0 0 255 160",
            ),
        ],
        false,
        true,
    );
    let literal = fixture.literal(vec![
        light(201, "light", "0 0 128", "1", "255 0 0 60", "0 255 0 120"),
        light(202, "light", "64 0 128", "2", "0 0 255 80", "255 0 0 140"),
        light(
            203,
            "light_spot",
            "-32 0 160",
            "3",
            "0 255 0 100",
            "0 0 255 160",
        ),
    ]);
    let source = fixture.save();
    let direct = fixture.compile(&source, options(LightingMode::DirectLdrHdr));
    let bounced_options = Options {
        bounces: 1,
        ..options(LightingMode::DirectLdrHdr)
    };
    let bounced = fixture.compile(&source, bounced_options);
    exact_literal(&bounced, &literal, &fixture.game, bounced_options);
    assert!(bounced.stats.bump_faces > 0 && bounced.stats.bump_lightmaps > 0);
    assert!(bounced.stats.bounce_samples > 0);
    assert_eq!(
        (
            bounced.stats.light_styles,
            bounced.stats.switchable_light_groups
        ),
        (4, 0)
    );
    for kind in [LightingKind::Ldr, LightingKind::Hdr] {
        channels(successful(&bounced), kind, &[0, 1, 2, 3], 4);
        for style in 1..=3 {
            assert!(
                energy(successful(&bounced), kind, style, 0)
                    > energy(successful(&direct), kind, style, 0)
            );
            assert!(
                (1..4).all(|direction| energy(successful(&bounced), kind, style, direction) > 0.0)
            );
        }
    }
    fixture.child.entities.push(light(
        13,
        "light",
        "0 0 144",
        "4",
        "255 255 255 40",
        "255 255 255 80",
    ));
    let source = fixture.save();
    let refused = fixture.compile(&source, options(LightingMode::DirectLdr));
    assert!(refused.bsp.is_none() && refused.report.has_errors());
    assert!(
        refused
            .report
            .diagnostics
            .iter()
            .any(|row| row.message.contains("four light styles")),
        "{:?}",
        refused.report
    );
    let path = previous(fixture.directory.path());
    assert!(!refused.write(&path).unwrap());
    preserved(&path);

    fixture.child.entities.pop();
    std::fs::create_dir_all(fixture.directory.path().join("models/props")).unwrap();
    std::fs::create_dir_all(fixture.directory.path().join("materials/models/test")).unwrap();
    for name in ["one", "models/test/two"] {
        std::fs::write(
            fixture
                .directory
                .path()
                .join(format!("materials/{name}.vmt")),
            "VertexLitGeneric {}",
        )
        .unwrap();
    }
    for (suffix, bytes) in ["mdl", "vvd", "dx90.vtx"]
        .into_iter()
        .zip(static_prop_model::model())
    {
        std::fs::write(
            fixture
                .directory
                .path()
                .join(format!("models/props/test.{suffix}")),
            bytes,
        )
        .unwrap();
    }
    let mut prop = Entity::new(150, "prop_static");
    for (key, value) in [
        ("model", "models/props/test.mdl"),
        ("origin", "0 0 32"),
        ("solid", "0"),
    ] {
        prop.properties.set(key, value);
    }
    fixture.root.entities.push(prop);
    let source = fixture.save();
    let refused = fixture.compile(
        &source,
        Options {
            bake_static_prop_vertices: true,
            ..options(LightingMode::DirectLdr)
        },
    );
    child_error(&refused, 10, "style-0");
    assert!(!refused.write(&path).unwrap());
    preserved(&path);
}
