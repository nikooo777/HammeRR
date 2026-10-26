use source_formats::bsp::{Bsp, lump};
use source_formats::keyvalues::{FlatBlockLimits, parse_flat_blocks_bounded};
use source_formats::vmf::{Connection, Properties};

use super::*;

const TEAMS: [&str; 2] = ["info_player_terrorist", "info_player_counterterrorist"];

fn spawn(id: u32, class: &str, origin: &str, angles: Option<&str>) -> Entity {
    let mut entity = Entity::new(id, class);
    entity.properties.set("origin", origin);
    if let Some(angles) = angles {
        entity.properties.set("angles", angles);
    }
    entity
}

fn spawn_fixture(entities: Vec<Entity>) -> (tempfile::TempDir, Vmf, Vmf, GameFs) {
    let (directory, mut parent, _, game) = fixture();
    parent
        .entities
        .retain(|entity| !TEAMS.contains(&entity.classname().unwrap_or("")));
    parent
        .entities
        .last_mut()
        .unwrap()
        .properties
        .set("origin", "24 -16 16");
    let child = helper_file(entities);
    write_child(directory.path(), "child.vmf", &child);
    (directory, parent, child, game)
}

fn literal_spawns(parent: &Vmf, entities: Vec<Entity>) -> Vmf {
    let mut document = Document::new(parent.clone());
    document
        .edit("literal player spawn control", |tx| {
            for mut entity in entities {
                entity.id = tx.new_object_id()?;
                entity.editor = None;
                tx.add_entity(entity)?;
            }
            for helper in parent.entities.iter().filter(|entity| {
                entity
                    .classname()
                    .is_some_and(|class| class.eq_ignore_ascii_case("func_instance"))
            }) {
                tx.remove(ObjectId::Entity(helper.id))?;
            }
            Ok(())
        })
        .unwrap();
    document.into_vmf()
}

fn spawn_output(
    parent: &Vmf,
    game: &GameFs,
    context: Option<&SourceContext>,
    lighting: crate::LightingMode,
) -> crate::Output {
    let output = compile_with_source_controlled(
        parent,
        game,
        Options {
            lighting,
            ..Options::default()
        },
        None,
        context,
        &Control::default(),
    );
    assert!(
        output.bsp.is_some() && !output.report.has_errors(),
        "{:?}",
        output.report
    );
    output
}

fn emitted_spawns(bsp: &Bsp) -> Vec<Entity> {
    parse_flat_blocks_bounded(
        bsp.entities().unwrap(),
        FlatBlockLimits {
            max_bytes: 1024 * 1024,
            max_blocks: 1024,
            max_pairs: 16_384,
        },
    )
    .unwrap()
    .into_iter()
    .filter_map(|block| {
        let properties = Properties(block);
        let class = properties.get("classname")?;
        if !TEAMS.contains(&class) {
            return None;
        }
        let mut entity = Entity::new(
            properties.get("hammerid").unwrap().parse::<u32>().unwrap(),
            class,
        );
        entity.properties = properties;
        Some(entity)
    })
    .collect()
}

fn assert_pose(actual: &Entity, expected: &Entity) {
    assert_eq!(actual.classname(), expected.classname());
    assert!(
        actual
            .origin()
            .unwrap()
            .abs_diff_eq(expected.origin().unwrap(), 1e-8),
        "{actual:?} != {expected:?}"
    );
    let actual = source_matrix(vector(actual, "angles").unwrap());
    let expected = source_matrix(vector(expected, "angles").unwrap());
    for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
        assert!(
            (actual * axis).abs_diff_eq(expected * axis, 1e-9),
            "{actual:?} != {expected:?}"
        );
    }
}

#[test]
fn team_spawns_match_literal_translation_and_noncommuting_position_and_orientation_bases() {
    for nested in [false, true] {
        let (directory, mut parent, child, game) = spawn_fixture(vec![
            spawn(20, TEAMS[0], "4 12 48", None),
            spawn(21, TEAMS[1], "-8 4 32", Some("0 0 90")),
        ]);
        let expected = if nested {
            write_child(
                directory.path(),
                "middle.vmf",
                &helper_file(vec![nested_helper(100, "child.vmf", "48 -16 32", "0 90 0")]),
            );
            let helper = parent.entities.last_mut().unwrap();
            helper.properties.set("file", "middle.vmf");
            helper.properties.set("origin", "0 0 160");
            helper.properties.set("angles", "90 0 0");
            vec![
                spawn(20, TEAMS[0], "80 -12 124", Some("0 90 90")),
                spawn(21, TEAMS[1], "64 -24 116", Some("0 90 180")),
            ]
        } else {
            vec![
                spawn(20, TEAMS[0], "28 -4 64", None),
                spawn(21, TEAMS[1], "16 -12 48", Some("0 0 90")),
            ]
        };
        let before = parent.clone();
        let original_child = std::fs::read(directory.path().join("child.vmf")).unwrap();
        let owner = parent.entities.last().unwrap().id;
        let context = source(directory.path());
        let prepared = prepare(
            &parent,
            Some(&context),
            &Control::default(),
            &mut Report::default(),
        )
        .unwrap();
        let stats = prepared.stats().unwrap();
        assert_eq!(
            (stats.player_spawns, stats.instances, stats.files),
            (2, if nested { 2 } else { 1 }, if nested { 2 } else { 1 })
        );
        assert!(prepared.generated_brushes().next().is_none());
        let generated = prepared
            .vmf()
            .entities
            .iter()
            .filter(|entity| prepared.root_entity(entity.id).is_some())
            .collect::<Vec<_>>();
        for ((actual, expected), original) in generated.iter().zip(&expected).zip(&child.entities) {
            assert_pose(actual, expected);
            assert_eq!(prepared.root_entity(actual.id), Some(owner));
            let mut report = Report::default();
            report.diagnostics.push(Diagnostic::warning(
                "spawn witness",
                Some(format!("entity:{}", actual.id)),
                actual.origin(),
            ));
            prepared.annotate(&mut report);
            assert_eq!(
                report.diagnostics[0].object,
                Some(format!("entity:{owner}"))
            );
            assert!(
                report.diagnostics[0]
                    .message
                    .contains(&format!("instance child.vmf, child object {}", original.id))
            );
        }
        let flat = literal_spawns(&parent, expected.clone());
        for lighting in [
            crate::LightingMode::Fullbright,
            crate::LightingMode::DirectLdrHdr,
        ] {
            let actual = spawn_output(&parent, &game, Some(&context), lighting);
            let expected = spawn_output(&flat, &game, None, lighting);
            assert_eq!(actual.stats.instances.as_ref().unwrap().player_spawns, 2);
            assert!(actual.static_prop_sources.is_none());
            let actual = actual.bsp.unwrap();
            let expected = expected.bsp.unwrap();
            let actual_spawns = emitted_spawns(&actual);
            let expected_spawns = emitted_spawns(&expected);
            assert_eq!(actual_spawns.len(), 2);
            for (actual, expected) in actual_spawns.iter().zip(&expected_spawns) {
                assert_pose(actual, expected);
                assert_eq!(actual.id, expected.id);
            }
            for (index, (actual, expected)) in actual.lumps.iter().zip(&expected.lumps).enumerate()
            {
                if index != lump::ENTITIES {
                    assert_eq!(
                        actual, expected,
                        "lump {index}, nested {nested}, {lighting:?}"
                    );
                }
            }
            if !nested {
                assert_eq!(actual.to_bytes(), expected.to_bytes());
            }
        }
        assert_eq!(parent, before);
        assert_eq!(
            std::fs::read(directory.path().join("child.vmf")).unwrap(),
            original_child
        );
    }
}

#[test]
fn repeated_team_spawns_retain_occurrence_ids_and_true_leaf_owners() {
    let (directory, mut parent, _, _) = spawn_fixture(vec![
        spawn(20, TEAMS[0], "4 12 48", None),
        spawn(21, TEAMS[1], "-8 4 32", None),
    ]);
    write_child(
        directory.path(),
        "middle.vmf",
        &helper_file(vec![
            nested_helper(100, "child.vmf", "-64 0 0", "0 0 0"),
            nested_helper(101, "child.vmf", "64 0 0", "0 0 0"),
        ]),
    );
    let root = parent.entities.last_mut().unwrap();
    root.properties.set("file", "middle.vmf");
    root.properties.set("origin", "0 0 0");
    let first = root.id;
    let second = first + 1;
    parent
        .entities
        .push(nested_helper(second, "middle.vmf", "0 80 0", "0 0 0"));
    let context = source(directory.path());
    let prepared = prepare(
        &parent,
        Some(&context),
        &Control::default(),
        &mut Report::default(),
    )
    .unwrap();
    let stats = prepared.stats().unwrap();
    assert_eq!(
        (stats.player_spawns, stats.instances, stats.files),
        (8, 6, 2)
    );
    let entities = prepared
        .vmf()
        .entities
        .iter()
        .filter(|entity| prepared.root_entity(entity.id).is_some())
        .collect::<Vec<_>>();
    assert_eq!(
        entities
            .iter()
            .map(|entity| entity.id)
            .collect::<BTreeSet<_>>()
            .len(),
        8
    );
    let mut index = 0;
    for (owner, y) in [(first, 0), (second, 80)] {
        for x in [-64, 64] {
            for (original, class, position) in [
                (
                    20,
                    TEAMS[0],
                    DVec3::new(f64::from(x + 4), f64::from(y + 12), 48.0),
                ),
                (
                    21,
                    TEAMS[1],
                    DVec3::new(f64::from(x - 8), f64::from(y + 4), 32.0),
                ),
            ] {
                let entity = entities[index];
                assert_eq!(entity.classname(), Some(class));
                assert_eq!(entity.origin(), Some(position));
                assert_eq!(
                    prepared.provenance[&format!("entity:{}", entity.id)].legacy,
                    (owner, "child.vmf".into(), original, None)
                );
                index += 1;
            }
        }
    }
}

#[test]
fn hidden_spawns_are_ignored_and_regions_select_transformed_boundary_origins() {
    let mut hidden = spawn(22, TEAMS[0], "NaN 0 0", Some("inf 0 0"));
    hidden.hidden = true;
    hidden.properties.set("spawnflags", "1");
    let (directory, mut parent, _, game) = spawn_fixture(vec![
        spawn(20, TEAMS[0], "-512 0 64", None),
        spawn(21, TEAMS[1], "-384 0 96", None),
        hidden,
    ]);
    let first = parent.entities.last_mut().unwrap();
    first.properties.set("origin", "512 0 0");
    let second = first.id + 1;
    parent
        .entities
        .push(nested_helper(second, "child.vmf", "0 0 0", "0 0 0"));
    parent.cordon = Some(Node::block(
        "cordon",
        vec![
            Node::string("active", "1"),
            Node::string("mins", "-128 -128 0"),
            Node::string("maxs", "128 128 256"),
        ],
    ));
    let flat = literal_spawns(
        &parent,
        vec![
            spawn(20, TEAMS[0], "0 0 64", None),
            spawn(21, TEAMS[1], "128 0 96", None),
            spawn(22, TEAMS[0], "-512 0 64", None),
            spawn(23, TEAMS[1], "-384 0 96", None),
        ],
    );
    let context = source(directory.path());
    let actual = spawn_output(
        &parent,
        &game,
        Some(&context),
        crate::LightingMode::Fullbright,
    );
    let expected = spawn_output(&flat, &game, None, crate::LightingMode::Fullbright);
    assert_eq!(actual.stats.instances.as_ref().unwrap().player_spawns, 4);
    assert!(actual.stats.compile_region.is_some());
    let actual = actual.bsp.unwrap();
    let emitted = emitted_spawns(&actual);
    assert_eq!(emitted.len(), 2);
    assert_eq!(emitted[1].origin(), Some(DVec3::new(128.0, 0.0, 96.0)));
    assert_eq!(actual.to_bytes(), expected.bsp.unwrap().to_bytes());
    assert!(!directory.path().join("models").exists());
}

#[test]
fn player_spawn_admission_keeps_the_canonical_neutral_point_subset() {
    for class in TEAMS {
        let mut valid = spawn(20, class, "0 0 64", None);
        spawns::admit(&valid).unwrap();
        for (key, value) in [
            ("angles", "15 25 35"),
            ("angle", "0"),
            ("spawnflags", "0"),
            ("targetname", ""),
            ("target", ""),
            ("parentname", ""),
            ("name", ""),
            ("globalname", ""),
            ("killtarget", ""),
        ] {
            valid.properties.set(key, value);
        }
        spawns::admit(&valid).unwrap();
        for (key, value) in [
            ("origin", "NaN 0 0"),
            ("origin", "16385 0 0"),
            ("origin", "1e50 0 0"),
            ("origin", "0 0"),
            ("angles", "0 inf 0"),
            ("angles", "0 0 0 0"),
            ("angle", "-1"),
            ("angle", "NaN"),
            ("spawnflags", "1"),
            ("spawnflags", "0.0"),
            ("targetname", "named"),
            ("target", " "),
            ("parentname", "parent"),
            ("name", "fallback"),
            ("globalname", "global"),
            ("killtarget", "target"),
            ("enabled", "1"),
            ("team", "2"),
            ("model", ""),
        ] {
            let mut invalid = valid.clone();
            invalid.properties.set(key, value);
            assert!(spawns::admit(&invalid).is_err(), "{class}: {key}={value}");
        }
        for key in [
            "classname",
            "origin",
            "angles",
            "angle",
            "spawnflags",
            "targetname",
        ] {
            let mut invalid = valid.clone();
            invalid
                .properties
                .0
                .iter_mut()
                .find(|(name, _)| name == key)
                .unwrap()
                .0 = key.to_uppercase();
            assert!(
                spawns::admit(&invalid).unwrap_err().contains("canonical"),
                "{class}, {key}"
            );
        }
        let mut invalid = valid.clone();
        invalid.properties.0.push(("origin".into(), "1 2 3".into()));
        assert!(spawns::admit(&invalid).unwrap_err().contains("repeats"));
        let mut invalid = valid.clone();
        invalid.properties.0.push(("Origin".into(), "1 2 3".into()));
        assert!(spawns::admit(&invalid).is_err());
        let mut invalid = valid.clone();
        invalid.properties.remove("origin");
        assert!(spawns::admit(&invalid).is_err());
        for issue in ["brush", "opaque", "connections"] {
            let mut invalid = valid.clone();
            match issue {
                "brush" => invalid
                    .solids
                    .push(test_common::room().world.solids.remove(0)),
                "opaque" => invalid.extra.push(Node::block("unknown", Vec::new())),
                "connections" => {
                    invalid.connections = Some(vec![Connection {
                        output: "OnUser1".into(),
                        value: "other,Kill,,0,-1".into(),
                    }])
                }
                _ => unreachable!(),
            }
            assert!(spawns::admit(&invalid).is_err(), "{issue}");
        }
    }
    for class in [
        "Info_Player_Terrorist",
        "info_player_start",
        "info_player_logo",
        "hostage_entity",
    ] {
        let entity = spawn(20, class, "0 0 64", None);
        assert!(entities::kind(&entity).is_none());
        assert!(spawns::admit(&entity).is_err());
    }
    assert!(
        serde_json::to_value(InstanceStats::default())
            .unwrap()
            .get("player_spawns")
            .is_none()
    );
}

#[test]
fn spawn_source_and_intermediate_bounds_refuse_without_replacing_prior_outputs() {
    let (directory, mut parent, _, game) = spawn_fixture(vec![spawn(20, TEAMS[0], "0 0 64", None)]);
    write_child(
        directory.path(),
        "middle.vmf",
        &helper_file(vec![nested_helper(100, "child.vmf", "16380 0 0", "0 0 0")]),
    );
    let helper = parent.entities.last_mut().unwrap();
    helper.properties.set("file", "middle.vmf");
    helper.properties.set("origin", "-16380 0 0");
    let owner = helper.id;
    let destination = directory.path().join("previous.bsp");
    std::fs::write(&destination, b"previous BSP").unwrap();
    std::fs::write(destination.with_extension("lin"), b"previous LIN").unwrap();
    let context = source(directory.path());
    for position in ["8 0 64", "16385 0 64"] {
        write_child(
            directory.path(),
            "child.vmf",
            &helper_file(vec![spawn(20, TEAMS[0], position, None)]),
        );
        let output = compiled(&parent, &game, &context);
        assert!(output.bsp.is_none() && output.leak.is_none());
        let diagnostic = output
            .report
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.level == crate::Level::Error)
            .unwrap();
        assert_eq!(diagnostic.object, Some(format!("entity:{owner}")));
        assert!(
            diagnostic.message.contains("player spawn")
                && diagnostic.message.contains("bound")
                && diagnostic.message.contains("child.vmf"),
            "{diagnostic:?}"
        );
        assert!(!output.write(&destination).unwrap());
        assert_eq!(std::fs::read(&destination).unwrap(), b"previous BSP");
        assert_eq!(
            std::fs::read(destination.with_extension("lin")).unwrap(),
            b"previous LIN"
        );
    }
}

#[test]
fn spawn_child_drift_and_cancellation_preserve_original_sources_and_publication() {
    let (directory, mut parent, child, game) =
        spawn_fixture(vec![spawn(20, TEAMS[0], "0 0 64", None)]);
    write_child(
        directory.path(),
        "middle.vmf",
        &helper_file(vec![nested_helper(100, "child.vmf", "0 0 0", "0 0 0")]),
    );
    parent
        .entities
        .last_mut()
        .unwrap()
        .properties
        .set("file", "middle.vmf");
    let destination = directory.path().join("previous.bsp");
    std::fs::write(&destination, b"previous BSP").unwrap();
    std::fs::write(destination.with_extension("lin"), b"previous LIN").unwrap();
    let before = parent.clone();
    let reservation = crate::WriteReservation::new(&destination, &Control::default()).unwrap();
    let context = source(directory.path());
    let output = spawn_output(
        &parent,
        &game,
        Some(&context),
        crate::LightingMode::Fullbright,
    );
    output
        .instance_dependencies
        .as_ref()
        .unwrap()
        .verify(&Control::default())
        .unwrap();
    let staged = output
        .prepare_write_reserved(reservation, &Control::default())
        .unwrap()
        .unwrap();
    let mut changed = child.clone();
    changed.entities[0].properties.set("origin", "0 0 80");
    write_child(directory.path(), "child.vmf", &changed);
    assert!(staged.commit(&Control::default()).is_err());
    assert_eq!(std::fs::read(&destination).unwrap(), b"previous BSP");
    assert_eq!(
        std::fs::read(destination.with_extension("lin")).unwrap(),
        b"previous LIN"
    );
    write_child(directory.path(), "child.vmf", &child);
    let helper = parent.entities.last().unwrap().clone();
    let control = Control::default();
    let mut report = Report::default();
    let mut expansion = expand::Expansion::new(
        &parent,
        &context,
        &control,
        &mut report,
        PreparationPurpose::Reference,
    );
    let mut document = Document::new(parent.clone());
    let error = document
        .edit("cancel after a real player spawn", |tx| {
            expansion.helper(
                tx,
                &helper,
                &helper,
                &mut Vec::new(),
                &mut vec![context.root.clone()],
            )?;
            assert!(
                tx.vmf()
                    .entities
                    .iter()
                    .any(|entity| entity.classname() == Some(TEAMS[0]))
            );
            control.cancel().unwrap();
            expansion.helper(
                tx,
                &helper,
                &helper,
                &mut Vec::new(),
                &mut vec![context.root.clone()],
            )
        })
        .unwrap_err();
    assert!(error.to_string().contains("cancelled"));
    assert_eq!(document.vmf(), &before);
    assert_eq!(parent, before);
    assert_eq!(
        std::fs::read(directory.path().join("child.vmf")).unwrap(),
        child.write(&Default::default()).as_bytes()
    );
}

#[test]
fn child_player_spawns_seed_ordinary_leaks_with_root_and_leaf_provenance() {
    let (directory, mut parent, _, game) = spawn_fixture(vec![spawn(20, TEAMS[1], "0 0 64", None)]);
    parent.world.solids.remove(1);
    let owner = parent.entities.last().unwrap().id;
    let context = source(directory.path());
    let output = compiled(&parent, &game, &context);
    assert!(output.bsp.is_none());
    let leak = output.leak.as_ref().unwrap();
    assert_eq!(leak.entity, owner);
    assert_eq!(leak.points.first(), Some(&DVec3::new(24.0, -16.0, 80.0)));
    assert!(
        output.report.diagnostics.iter().any(|diagnostic| {
            diagnostic.object == Some(format!("entity:{owner}"))
                && diagnostic
                    .message
                    .contains("instance child.vmf, child object 20")
        }),
        "{:?}",
        output.report
    );
    output
        .instance_dependencies
        .as_ref()
        .unwrap()
        .verify(&Control::default())
        .unwrap();
}

#[test]
fn player_spawns_share_the_combined_object_limit() {
    let (directory, mut parent, _, _) = spawn_fixture(vec![spawn(20, TEAMS[0], "0 0 64", None)]);
    let id = parent.object_ids().max().unwrap() + 1;
    let remaining = MAX_OBJECTS - parent.object_ids().count();
    for offset in 0..remaining {
        parent
            .entities
            .push(Entity::new(id + offset as u32, "info_target"));
    }
    let before = parent.clone();
    let context = source(directory.path());
    let error = prepare(
        &parent,
        Some(&context),
        &Control::default(),
        &mut Report::default(),
    )
    .err()
    .unwrap();
    assert!(error.message.contains("combined objects"), "{error:?}");
    assert_eq!(parent, before);
}
