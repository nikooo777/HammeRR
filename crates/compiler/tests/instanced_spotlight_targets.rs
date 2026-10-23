mod common;

use std::path::Path;

use compiler::instances::{self, SourceContext};
use compiler::{Control, LightingMode, Options, SkyQuality};
use glam::DVec3;
use map_core::Document;
use source_formats::bsp::{Bsp, LightingKind};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::keyvalues::Node;
use source_formats::vmf::{Entity, Vmf};

const MODES: [LightingMode; 3] = [
    LightingMode::Fullbright,
    LightingMode::DirectLdr,
    LightingMode::DirectLdrHdr,
];

fn next_id(source: &Vmf) -> u32 {
    source.object_ids().chain(source.side_ids()).max().unwrap() + 1
}

fn target(id: u32, origin: &str, name: &str) -> Entity {
    let mut entity = Entity::new(id, "info_target");
    for (key, value) in [
        ("origin", origin),
        ("targetname", name),
        ("angles", "0 0 0"),
        ("spawnflags", "0"),
    ] {
        entity.properties.set(key, value);
    }
    entity
}

fn spotlight(id: u32, origin: &str, name: &str) -> Entity {
    let mut entity = Entity::new(id, "light_spot");
    for (key, value) in [
        ("origin", origin),
        ("target", name),
        ("angles", "0 0 0"),
        ("pitch", "-45"),
        ("_light", "255 0 0 400"),
        ("_lightHDR", "0 255 0 800"),
        ("_lightscaleHDR", "0.5"),
        ("_inner_cone", "20"),
        ("_cone", "35"),
        ("_exponent", "2"),
        ("_constant_attn", "1"),
    ] {
        entity.properties.set(key, value);
    }
    entity
}

fn leaf() -> Vmf {
    let mut source = Document::empty().into_vmf();
    source.entities = vec![target(2, "8 24 64", "aim"), spotlight(3, "8 24 128", "aim")];
    source
}

fn instance(source: &mut Vmf, file: &str, name: &str, origin: &str, angles: &str) -> u32 {
    let id = next_id(source);
    let mut entity = Entity::new(id, "func_instance");
    for (key, value) in [
        ("file", file),
        ("targetname", name),
        ("fixup_style", "0"),
        ("origin", origin),
        ("angles", angles),
    ] {
        entity.properties.set(key, value);
    }
    source.entities.push(entity);
    id
}

fn write(directory: &Path, name: &str, source: &Vmf) -> Vec<u8> {
    let bytes = source.write(&Default::default()).into_bytes();
    std::fs::write(directory.join(name), &bytes).unwrap();
    bytes
}

fn context(directory: &Path, parent: &Vmf) -> SourceContext {
    write(directory, "parent.vmf", parent);
    SourceContext::capture(&directory.join("parent.vmf"), &Control::default()).unwrap()
}

fn game(directory: &Path) -> GameFs {
    let path = directory.join("materials/dev/dev_measuregeneric01.vmt");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, "LightmappedGeneric { }").unwrap();
    let path = directory.join("materials/tools/toolsblack.vmt");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, "UnlitGeneric { }").unwrap();
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.to_owned()),
    });
    game
}

fn options(lighting: LightingMode) -> Options {
    Options {
        lighting,
        sky_quality: SkyQuality::Fast,
        ..Options::default()
    }
}

fn compile(
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
    output.bsp.unwrap()
}

fn literal(parent: &Vmf, entities: Vec<Entity>) -> Vmf {
    let first = next_id(parent);
    let mut source = parent.clone();
    source
        .entities
        .retain(|entity| entity.classname() != Some("func_instance"));
    for (index, mut entity) in entities.into_iter().enumerate() {
        entity.id = first + u32::try_from(index).unwrap();
        source.entities.push(entity);
    }
    source
}

#[test]
fn repeated_translated_targets_match_independent_complete_literal_bsps_in_all_modes() {
    let directory = tempfile::tempdir().unwrap();
    let game = game(directory.path());
    let child = leaf();
    let bytes = write(directory.path(), "leaf.vmf", &child);
    let mut parent = common::room();
    let a = instance(&mut parent, "leaf.vmf", "a", "-56 0 0", "0 0 0");
    let b = instance(&mut parent, "leaf.vmf", "b", "56 0 0", "0 0 0");
    let context = context(directory.path(), &parent);
    let before = parent.clone();
    let expected = literal(
        &parent,
        vec![
            target(2, "-48 24 64", "a-aim"),
            spotlight(3, "-48 24 128", "a-aim"),
            target(4, "64 24 64", "b-aim"),
            spotlight(5, "64 24 128", "b-aim"),
        ],
    );
    let mut report = compiler::Report::default();
    let prepared =
        instances::resolve(&parent, Some(&context), &Control::default(), &mut report).unwrap();
    assert!(!report.has_errors());
    assert_eq!(prepared.stats().unwrap().spotlight_targets, 2);
    assert_eq!(prepared.stats().unwrap().spot_lights, 2);
    assert_eq!(prepared.generated_brushes().count(), 0);
    for (index, owner) in [a, a, b, b].into_iter().enumerate() {
        assert_eq!(
            prepared.root_entity(next_id(&parent) + u32::try_from(index).unwrap()),
            Some(owner)
        );
    }
    for mode in MODES {
        let actual = successful(compile(&parent, &game, mode, &context));
        let expected = successful(compiler::compile_with_options(
            &expected,
            &game,
            options(mode),
        ));
        assert_eq!(actual.to_bytes(), expected.to_bytes(), "{mode:?}");
        for kind in [LightingKind::Ldr, LightingKind::Hdr] {
            let lights = actual.world_lights(kind).unwrap();
            if mode == LightingMode::Fullbright
                || (mode == LightingMode::DirectLdr && kind == LightingKind::Hdr)
            {
                assert!(lights.is_empty());
                continue;
            }
            assert_eq!(lights.len(), 2);
            for (light, x) in lights.iter().zip([-48.0, 64.0]) {
                assert_eq!(light.origin, [x, 24.0, 128.0]);
                assert_eq!(light.normal, [0.0, 0.0, -1.0]);
                assert_eq!(
                    light.intensity,
                    if kind == LightingKind::Ldr {
                        [400.0 / 255.0, 0.0, 0.0]
                    } else {
                        [0.0, 400.0 / 255.0, 0.0]
                    }
                );
            }
        }
    }
    assert_eq!(parent, before);
    assert_eq!(
        std::fs::read(directory.path().join("leaf.vmf")).unwrap(),
        bytes
    );
}

#[test]
fn nested_noncommuting_target_poses_keep_the_independent_signed_permutation() {
    let directory = tempfile::tempdir().unwrap();
    let game = game(directory.path());
    let mut child = leaf();
    child.entities[0].properties.set("origin", "8 -4 16");
    child.entities[1].properties.set("origin", "24 -4 16");
    write(directory.path(), "leaf.vmf", &child);
    let mut middle = Document::empty().into_vmf();
    instance(&mut middle, "leaf.vmf", "inner", "16 0 0", "90 0 0");
    write(directory.path(), "middle.vmf", &middle);
    let mut parent = common::room();
    instance(&mut parent, "middle.vmf", "outer", "64 80 128", "0 90 0");
    let context = context(directory.path(), &parent);
    let mut report = compiler::Report::default();
    let prepared =
        instances::resolve(&parent, Some(&context), &Control::default(), &mut report).unwrap();
    let endpoints = prepared
        .vmf()
        .entities
        .iter()
        .filter(|entity| matches!(entity.classname(), Some("info_target" | "light_spot")))
        .collect::<Vec<_>>();
    assert_eq!(endpoints.len(), 2);
    for (entity, expected) in endpoints.iter().zip([
        DVec3::new(68.0, 112.0, 120.0),
        DVec3::new(68.0, 112.0, 104.0),
    ]) {
        assert!(entity.origin().unwrap().distance(expected) < 1e-10);
    }
    assert_eq!(
        endpoints[0].properties.get("targetname"),
        Some("outer-inner-aim")
    );
    assert_eq!(
        endpoints[1].properties.get("target"),
        Some("outer-inner-aim")
    );
    let actual = successful(compile(
        &parent,
        &game,
        LightingMode::DirectLdrHdr,
        &context,
    ));
    let expected = literal(
        &parent,
        vec![
            target(2, "68 112 120", "outer-inner-aim"),
            spotlight(3, "68 112 104", "outer-inner-aim"),
        ],
    );
    let expected = successful(compiler::compile_with_options(
        &expected,
        &game,
        options(LightingMode::DirectLdrHdr),
    ));
    assert_eq!(actual.game_lumps, expected.game_lumps);
    for (index, (actual, expected)) in actual.lumps.iter().zip(&expected.lumps).enumerate().skip(1)
    {
        assert_eq!(actual, expected, "lump {index}");
    }
    for kind in [LightingKind::Ldr, LightingKind::Hdr] {
        let lights = actual.world_lights(kind).unwrap();
        assert_eq!(lights.len(), 1);
        assert_eq!(lights[0].origin, [68.0, 112.0, 104.0]);
        assert_eq!(lights[0].normal, [0.0, 0.0, 1.0]);
    }
}

#[test]
fn targeted_light_bindings_use_real_local_endpoints_for_template_and_effective_validation() {
    let directory = tempfile::tempdir().unwrap();
    let game = game(directory.path());
    let mut child = leaf();
    for (key, value) in [
        ("_light", "$lamp"),
        ("_inner_cone", "$inner"),
        ("_cone", "$outer"),
        ("_exponent", "$focus"),
    ] {
        child.entities[1].properties.set(key, value);
    }
    let bytes = write(directory.path(), "leaf.vmf", &child);
    let mut parent = common::room();
    let owner = instance(&mut parent, "leaf.vmf", "a", "16 24 0", "0 0 0");
    for (key, value) in [
        ("replace01", "$lamp 255 0 0 400"),
        ("replace02", "$inner 20"),
        ("replace03", "$outer 35"),
        ("replace04", "$focus 2"),
    ] {
        parent
            .entities
            .last_mut()
            .unwrap()
            .properties
            .set(key, value);
    }
    let context = context(directory.path(), &parent);
    let expected = literal(
        &parent,
        vec![
            target(2, "24 48 64", "a-aim"),
            spotlight(3, "24 48 128", "a-aim"),
        ],
    );
    for mode in MODES {
        let actual = successful(compile(&parent, &game, mode, &context));
        let expected = successful(compiler::compile_with_options(
            &expected,
            &game,
            options(mode),
        ));
        assert_eq!(actual.to_bytes(), expected.to_bytes(), "{mode:?}");
    }
    parent
        .entities
        .last_mut()
        .unwrap()
        .properties
        .set("replace02", "$inner 40");
    let output = compile(&parent, &game, LightingMode::Fullbright, &context);
    assert!(output.bsp.is_none() && output.report.has_errors());
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .any(
                |diagnostic| diagnostic.object == Some(format!("entity:{owner}"))
                    && diagnostic.message.contains("inner ≤ outer")
            ),
        "{:?}",
        output.report
    );
    assert_eq!(
        std::fs::read(directory.path().join("leaf.vmf")).unwrap(),
        bytes
    );
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
fn source_target_failures_refuse_before_regions_including_fullbright_and_keep_previous_output() {
    let directory = tempfile::tempdir().unwrap();
    let game = game(directory.path());
    let mut parent = common::room();
    let owner = instance(&mut parent, "leaf.vmf", "a", "1000 0 0", "0 0 0");
    region(&mut parent);
    let context = context(directory.path(), &parent);
    let destination = directory.path().join("previous.bsp");
    std::fs::write(&destination, b"previous BSP").unwrap();
    std::fs::write(destination.with_extension("lin"), b"previous LIN").unwrap();
    for issue in [
        "missing",
        "case",
        "hidden",
        "same point",
        "duplicate",
        "bounds",
        "nonfinite",
        "parent",
        "flags",
        "unknown",
        "whitespace",
        "duplicate field",
        "hdr",
        "pitch",
    ] {
        let mut child = leaf();
        match issue {
            "missing" => {
                child.entities.remove(0);
            }
            "case" => child.entities[1].properties.set("target", "Aim"),
            "hidden" => child.entities[0].hidden = true,
            "same point" => child.entities[0].properties.set("origin", "8 24 128"),
            "duplicate" => child.entities.push(target(4, "12 24 64", "AIM")),
            "bounds" => child.entities[0].properties.set("origin", "17000 0 64"),
            "nonfinite" => child.entities[0].properties.set("origin", "NaN 0 64"),
            "parent" => child.entities[0].properties.set("parentname", "mover"),
            "flags" => child.entities[0].properties.set("spawnflags", "1"),
            "unknown" => child.entities[0].properties.set("runtime_key", "0"),
            "whitespace" => child.entities[1].properties.set("target", " "),
            "duplicate field" => child.entities[0]
                .properties
                .0
                .push(("ORIGIN".into(), "8 24 64".into())),
            "hdr" => child.entities[1].properties.set("_lightHDR", "invalid"),
            "pitch" => child.entities[1].properties.set("pitch", "NaN"),
            _ => unreachable!(),
        }
        let bytes = write(directory.path(), "leaf.vmf", &child);
        for mode in MODES {
            let output = compile(&parent, &game, mode, &context);
            assert!(
                output.report.has_errors() && output.bsp.is_none() && output.leak.is_none(),
                "{issue}/{mode:?}: {:?}",
                output.report
            );
            assert!(
                output
                    .report
                    .diagnostics
                    .iter()
                    .any(
                        |diagnostic| diagnostic.object == Some(format!("entity:{owner}"))
                            && diagnostic.message.contains("leaf.vmf")
                    ),
                "{issue}: {:?}",
                output.report
            );
            assert!(!output.write(&destination).unwrap());
            assert_eq!(std::fs::read(&destination).unwrap(), b"previous BSP");
            assert_eq!(
                std::fs::read(destination.with_extension("lin")).unwrap(),
                b"previous LIN"
            );
        }
        assert_eq!(
            std::fs::read(directory.path().join("leaf.vmf")).unwrap(),
            bytes
        );
    }
}

#[test]
fn scoped_target_names_refuse_root_generated_and_lighting_origin_collisions() {
    for issue in [
        "root target",
        "root other class",
        "root case key",
        "repeated prefix",
        "lighting origin",
        "missing scope",
        "postfix",
        "external reference",
    ] {
        let directory = tempfile::tempdir().unwrap();
        let game = game(directory.path());
        let mut child = leaf();
        let mut parent = common::room();
        let owner = instance(&mut parent, "leaf.vmf", "a", "0 0 0", "0 0 0");
        match issue {
            "root target" | "root other class" | "root case key" => {
                let mut root = target(next_id(&parent), "0 0 64", "A-AIM");
                if issue == "root other class" {
                    root.properties.set("classname", "info_player_start");
                }
                if issue == "root case key" {
                    root.properties.remove("targetname");
                    root.properties.set("TARGETNAME", "A-AIM");
                }
                parent.entities.push(root);
            }
            "repeated prefix" => {
                instance(&mut parent, "leaf.vmf", "a", "64 0 0", "0 0 0");
            }
            "lighting origin" => {
                let mut helper = target(4, "0 0 64", "aim");
                helper.properties.set("classname", "info_lighting");
                helper.properties.remove("spawnflags");
                child.entities.push(helper);
            }
            "missing scope" => {
                parent
                    .entities
                    .last_mut()
                    .unwrap()
                    .properties
                    .remove("targetname");
            }
            "postfix" => parent
                .entities
                .last_mut()
                .unwrap()
                .properties
                .set("fixup_style", "1"),
            "external reference" => {
                child.entities.remove(0);
                parent
                    .entities
                    .push(target(next_id(&parent), "0 0 64", "aim"));
            }
            _ => unreachable!(),
        }
        write(directory.path(), "leaf.vmf", &child);
        let context = context(directory.path(), &parent);
        let output = compile(&parent, &game, LightingMode::Fullbright, &context);
        assert!(
            output.report.has_errors() && output.bsp.is_none(),
            "{issue}: {:?}",
            output.report
        );
        assert!(
            output
                .report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.object == Some(format!("entity:{owner}"))),
            "{issue}: {:?}",
            output.report
        );
    }
}

#[test]
fn intermediate_target_bounds_and_direction_collapse_refuse_before_the_final_pose() {
    let directory = tempfile::tempdir().unwrap();
    let game = game(directory.path());
    let mut child = leaf();
    child.entities[0].properties.set("origin", "16000 0 64");
    child.entities[1].properties.set("origin", "15900 0 128");
    write(directory.path(), "leaf.vmf", &child);
    let mut middle = Document::empty().into_vmf();
    instance(&mut middle, "leaf.vmf", "inner", "1000 0 0", "0 0 0");
    write(directory.path(), "middle.vmf", &middle);
    let mut parent = common::room();
    instance(&mut parent, "middle.vmf", "outer", "-1000 0 0", "0 0 0");
    let context = context(directory.path(), &parent);
    let output = compile(&parent, &game, LightingMode::Fullbright, &context);
    assert!(output.bsp.is_none() && output.report.has_errors());
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("Source compile bounds")),
        "{:?}",
        output.report
    );
    child.entities[0]
        .properties
        .set("origin", "8.000000000000002 0 128");
    child.entities[1].properties.set("origin", "8 0 128");
    write(directory.path(), "leaf.vmf", &child);
    let output = compile(&parent, &game, LightingMode::Fullbright, &context);
    assert!(output.bsp.is_none() && output.report.has_errors() && output.leak.is_none());
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("distinct from its origin")),
        "{:?}",
        output.report
    );
}

#[test]
fn regions_use_ordinary_target_retention_and_zero_counts_stay_omitted() {
    let directory = tempfile::tempdir().unwrap();
    let game = game(directory.path());
    let mut child = leaf();
    child.entities[0].properties.set("origin", "200 24 64");
    write(directory.path(), "leaf.vmf", &child);
    let mut parent = common::room();
    let owner = instance(&mut parent, "leaf.vmf", "a", "0 0 0", "0 0 0");
    region(&mut parent);
    let context = context(directory.path(), &parent);
    let output = compile(&parent, &game, LightingMode::DirectLdr, &context);
    assert!(output.report.has_errors() && output.bsp.is_none());
    assert_eq!(
        output.stats.instances.as_ref().unwrap().spotlight_targets,
        1
    );
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .any(
                |diagnostic| diagnostic.object == Some(format!("entity:{owner}"))
                    && diagnostic
                        .message
                        .contains("Spotlight target must name one visible entity")
            ),
        "{:?}",
        output.report
    );
    let output = compile(&parent, &game, LightingMode::Fullbright, &context);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    let stats = serde_json::to_value(instances::InstanceStats::default()).unwrap();
    assert!(stats.get("spotlight_targets").is_none());
}
