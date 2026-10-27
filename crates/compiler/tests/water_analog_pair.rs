mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use compiler::{Options, PhysicsMode};
use glam::DVec3;
use map_core::{Document, ObjectId, Plane, edit};
use source_formats::bsp::{
    Brush, Bsp, CollisionHeader, Leaf, Model, Node as BspNode, contents, lump,
};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::keyvalues::{self, FlatBlockLimits, Node};

struct Pair {
    document: Document,
    owners: [u32; 2],
    brushes: [u32; 2],
    ordinary: u32,
}

fn game() -> (tempfile::TempDir, GameFs) {
    let directory = tempfile::tempdir().unwrap();
    for folder in ["materials/dev", "materials/tools", "scripts"] {
        std::fs::create_dir_all(directory.path().join(folder)).unwrap();
    }
    for (name, text) in [
        ("nodraw", "UnlitGeneric { %compilenodraw 1 }"),
        ("water", "Water { $abovewater 1 }"),
        (
            "slime",
            "LightmappedGeneric { %compileslime 1 $bottommaterial slime }",
        ),
        ("dev/dev_measuregeneric01", "LightmappedGeneric {}"),
        ("tools/toolsblack", "UnlitGeneric { $color \"[0 0 0]\" }"),
    ] {
        std::fs::write(directory.path().join(format!("materials/{name}.vmt")), text).unwrap();
    }
    std::fs::write(
        directory
            .path()
            .join("scripts/surfaceproperties_manifest.txt"),
        "surfaceproperties_manifest { file scripts/base.txt }",
    )
    .unwrap();
    std::fs::write(
        directory.path().join("scripts/base.txt"),
        "default { density 1000 }",
    )
    .unwrap();
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().to_owned()),
    });
    (directory, game)
}

fn add_box(
    document: &mut Document,
    name: &str,
    class: &str,
    mins: DVec3,
    maxs: DVec3,
    origin: DVec3,
) -> (u32, u32) {
    document
        .edit("named nodraw body", |tx| {
            let ObjectId::Solid(brush) = edit::create_block(tx, mins, maxs, "nodraw")? else {
                unreachable!()
            };
            let mut properties = vec![
                ("origin".into(), source_formats::vmf::format_vec3(origin)),
                ("targetname".into(), name.into()),
            ];
            properties.extend(
                compiler::WATER_ANALOG_DEFAULTS
                    .map(|(key, value)| (key.to_owned(), value.to_owned())),
            );
            let ObjectId::Entity(owner) = edit::tie_to_entity(tx, &[brush], class, &properties)?
            else {
                unreachable!()
            };
            Ok((owner, brush))
        })
        .unwrap()
}

fn pair(reversed: bool) -> Pair {
    let mut document = Document::new(common::room());
    let west = add_box(
        &mut document,
        "west_pool",
        "func_movelinear",
        DVec3::new(-160.0, -48.0, 32.0),
        DVec3::new(-96.0, 48.0, 128.0),
        DVec3::new(-128.0, 0.0, 80.0),
    );
    let ordinary = add_box(
        &mut document,
        "ordinary_body",
        "func_brush",
        DVec3::new(-16.0, 128.0, 48.0),
        DVec3::new(16.0, 160.0, 80.0),
        DVec3::new(0.0, 144.0, 64.0),
    );
    let east = add_box(
        &mut document,
        "east_pool",
        "func_movelinear",
        DVec3::new(96.0, -32.0, 48.0),
        DVec3::new(160.0, 32.0, 112.0),
        DVec3::new(128.0, 0.0, 80.0),
    );
    if reversed {
        let mut vmf = document.into_vmf();
        let west_index = vmf
            .entities
            .iter()
            .position(|entity| entity.id == west.0)
            .unwrap();
        let east_index = vmf
            .entities
            .iter()
            .position(|entity| entity.id == east.0)
            .unwrap();
        vmf.entities.swap(west_index, east_index);
        document = Document::new(vmf);
    }
    Pair {
        document,
        owners: [west.0, east.0],
        brushes: [west.1, east.1],
        ordinary: ordinary.0,
    }
}

fn water(pair: &Pair) -> Document {
    let mut document = Document::new(pair.document.vmf().clone());
    document
        .edit("two water roles", |tx| {
            for owner in pair.owners {
                tx.entity_mut(owner)?
                    .properties
                    .set("classname", compiler::WATER_ANALOG_CLASSNAME);
            }
            Ok(())
        })
        .unwrap();
    document
}

fn refused(
    document: &Document,
    game: &GameFs,
    physics: PhysicsMode,
    owners: &[u32],
    message: &str,
) -> compiler::Output {
    let before = document.vmf().clone();
    let revision = document.revision();
    let history = document
        .undo_labels()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("pair.vmf");
    let path = source.with_extension("bsp");
    let pointfile = source.with_extension("lin");
    let source_bytes = before.write(&Default::default());
    std::fs::write(&source, &source_bytes).unwrap();
    std::fs::write(&path, b"previous BSP").unwrap();
    std::fs::write(&pointfile, b"previous LIN").unwrap();
    let output = compiler::compile_with_options(
        document.vmf(),
        game,
        Options {
            physics,
            ..Default::default()
        },
    );
    assert!(
        output.bsp.is_none() && output.leak.is_none(),
        "{:?}",
        output.report
    );
    for owner in owners {
        let object = format!("entity:{owner}");
        let origin = document.entity(*owner).unwrap().origin();
        assert!(
            output.report.diagnostics.iter().any(|diagnostic| {
                diagnostic.level == compiler::Level::Error
                    && diagnostic.object.as_deref() == Some(object.as_str())
                    && diagnostic.position == origin
                    && diagnostic.message.contains(message)
            }),
            "{:?}",
            output.report
        );
    }
    assert!(
        !output.report.diagnostics.iter().any(|diagnostic| {
            diagnostic.message.contains("vphysics.so")
                || diagnostic.message.contains("hammerr-physics")
        }),
        "{:?}",
        output.report
    );
    assert_eq!(output.stats.physics_bytes, 0);
    assert!(!output.write(&path).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), b"previous BSP");
    assert_eq!(std::fs::read(&pointfile).unwrap(), b"previous LIN");
    assert_eq!(std::fs::read_to_string(&source).unwrap(), source_bytes);
    assert_eq!(document.vmf(), &before);
    assert_eq!(document.revision(), revision);
    assert_eq!(
        document
            .undo_labels()
            .map(str::to_owned)
            .collect::<Vec<_>>(),
        history
    );
    output
}

fn only_physics_requirement(output: &compiler::Output) {
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.level == compiler::Level::Error)
            .all(|diagnostic| diagnostic
                .message
                .contains("CS:S engine physics is required")),
        "{:?}",
        output.report
    );
}

fn regional(document: Document) -> Document {
    let mut vmf = document.into_vmf();
    vmf.cordon = Some(Node::block(
        "cordon",
        vec![
            Node::string("active", "1"),
            Node::string("mins", "-192 -64 0"),
            Node::string("maxs", "192 64 160"),
        ],
    ));
    Document::new(vmf)
}

fn add_third(document: &mut Document) -> u32 {
    add_box(
        document,
        "third_pool",
        compiler::WATER_ANALOG_CLASSNAME,
        DVec3::new(-32.0, -160.0, 32.0),
        DVec3::new(32.0, -96.0, 96.0),
        DVec3::new(0.0, -128.0, 64.0),
    )
    .0
}

#[test]
fn disjoint_pair_keeps_each_dry_internal_brush_and_requires_engine_physics_for_both_owners() {
    let (_directory, game) = game();
    for reversed in [false, true] {
        let pair = pair(reversed);
        let document = water(&pair);
        let mut report = compiler::Report::default();
        let map = compiler::map::Map::load(document.vmf(), &game, &mut report).unwrap();
        assert!(!report.has_errors(), "{report:?}");
        for (owner, brush) in pair.owners.into_iter().zip(pair.brushes) {
            let entity = map
                .entities
                .iter()
                .find(|entity| entity.id == owner)
                .unwrap();
            assert_eq!(entity.brushes.len(), 1);
            let actual = &map.brushes[entity.brushes[0]];
            assert_eq!(actual.id, brush);
            assert_eq!(actual.contents, contents::SOLID);
            assert!(actual.sides.iter().all(|side| !side.visible));
        }
        let ordinary = map
            .entities
            .iter()
            .find(|entity| entity.id == pair.ordinary)
            .unwrap();
        assert_eq!(ordinary.classname, "func_brush");
        refused(
            &document,
            &game,
            PhysicsMode::None,
            &pair.owners,
            "CS:S engine physics is required",
        );
    }
}

#[test]
fn third_retained_pool_refuses_with_its_original_identity_before_native_work() {
    let (_directory, game) = game();
    let pair = pair(false);
    let mut document = water(&pair);
    let owner = add_third(&mut document);
    refused(
        &document,
        &game,
        PhysicsMode::Engine,
        &[owner],
        "at most 2 retained stationary water volumes",
    );
}

#[test]
fn omitted_third_pool_does_not_spend_the_retained_allowance_and_all_hidden_output_stays_dry() {
    let (_directory, game) = game();
    let pair = pair(false);
    let mut document = water(&pair);
    let third = add_third(&mut document);
    document
        .edit("hidden third", |tx| {
            let hidden = tx.entity_mut(third)?;
            hidden.hidden = true;
            hidden.properties.set("origin", "NaN 0 0");
            Ok(())
        })
        .unwrap();
    let mut report = compiler::Report::default();
    compiler::map::Map::load(document.vmf(), &game, &mut report).unwrap();
    assert!(!report.has_errors(), "{report:?}");
    let output = refused(
        &document,
        &game,
        PhysicsMode::None,
        &pair.owners,
        "CS:S engine physics is required",
    );
    only_physics_requirement(&output);

    let mut document = water(&pair);
    let third = add_third(&mut document);
    document
        .edit("invalid exterior third", |tx| {
            tx.entity_mut(third)?.properties.set("speed", "100");
            Ok(())
        })
        .unwrap();
    let document = regional(document);
    let output = refused(
        &document,
        &game,
        PhysicsMode::None,
        &pair.owners,
        "CS:S engine physics is required",
    );
    only_physics_requirement(&output);

    let mut document = water(&pair);
    document
        .edit("hide pair", |tx| {
            for owner in pair.owners {
                tx.entity_mut(owner)?.hidden = true;
            }
            Ok(())
        })
        .unwrap();
    let actual = compiler::compile(document.vmf(), &game);
    let mut expected = pair.document.vmf().clone();
    expected
        .entities
        .retain(|entity| !pair.owners.contains(&entity.id));
    let expected = compiler::compile(&expected, &game);
    assert!(!actual.report.has_errors(), "{:?}", actual.report);
    assert!(!expected.report.has_errors(), "{:?}", expected.report);
    assert_eq!(
        actual.bsp.unwrap().to_bytes(),
        expected.bsp.unwrap().to_bytes()
    );
}

#[test]
fn touching_overlapping_and_native_rounded_pairs_refuse_before_cooking() {
    let (_directory, game) = game();
    let pair = pair(false);
    for gap in [0.0, -8.0, 0.000_002] {
        let mut document = water(&pair);
        let second_mins = DVec3::new(-96.0 + gap, -32.0, 48.0);
        let second_maxs = second_mins + DVec3::splat(64.0);
        document
            .edit("pair separation", |tx| {
                let solid = map_core::edit::block_solid(tx, second_mins, second_maxs, "nodraw")?;
                let entity = tx.entity_mut(pair.owners[1])?;
                entity.solids = vec![solid];
                entity.properties.set(
                    "origin",
                    source_formats::vmf::format_vec3(DVec3::new(second_mins.x, 0.0, 80.0)),
                );
                Ok(())
            })
            .unwrap();
        if gap > 0.0 {
            assert!(second_mins.x > -96.0 + 1e-6);
            assert_eq!(second_mins.x as f32, -96.0f32);
        }
        refused(
            &document,
            &game,
            PhysicsMode::Engine,
            &pair.owners,
            "conservative bounds touch or overlap solid:",
        );
    }
}

#[test]
fn pair_rejects_world_controls_and_other_retained_liquids_before_native_work() {
    let (_directory, game) = game();
    let pair = pair(false);
    for (key, value) in [
        (compiler::WATER_CURRENT_VELOCITY_KEY, "64 0 0"),
        (compiler::WATER_CURRENT_VELOCITY_KEY, "0 0 0"),
        (compiler::WATER_DAMPING_KEY, "0.005"),
        (compiler::WATER_DAMPING_KEY, "0"),
    ] {
        let mut vmf = water(&pair).into_vmf();
        vmf.world.properties.set(key, value);
        refused(
            &Document::new(vmf),
            &game,
            PhysicsMode::Engine,
            &pair.owners,
            "authored world water current or damping cannot be combined with stationary water volumes",
        );
    }
    for material in ["water", "slime"] {
        let mut document = water(&pair);
        let ObjectId::Solid(liquid) = document
            .edit("other liquid", |tx| {
                let liquid = edit::create_block(
                    tx,
                    DVec3::new(-32.0, -160.0, 16.0),
                    DVec3::new(32.0, -96.0, 64.0),
                    "nodraw",
                )?;
                let ObjectId::Solid(id) = liquid else {
                    unreachable!()
                };
                for side in &mut tx.solid_mut(id)?.sides {
                    if Plane::from_points(side.plane.0).unwrap().normal.z > 0.99 {
                        side.material = material.into();
                    }
                }
                Ok(liquid)
            })
            .unwrap()
        else {
            unreachable!()
        };
        refused(
            &document,
            &game,
            PhysicsMode::Engine,
            &pair.owners,
            &format!(
                "other WATER or SLIME brush geometry solid:{liquid} cannot be combined with stationary water volumes"
            ),
        );
        document
            .edit("hide other liquid", |tx| {
                tx.solid_mut(liquid)?.hidden = true;
                Ok(())
            })
            .unwrap();
        let output = refused(
            &document,
            &game,
            PhysicsMode::None,
            &pair.owners,
            "CS:S engine physics is required",
        );
        only_physics_requirement(&output);
        document
            .edit("restore exterior liquid", |tx| {
                tx.solid_mut(liquid)?.hidden = false;
                Ok(())
            })
            .unwrap();
        let output = refused(
            &regional(document),
            &game,
            PhysicsMode::None,
            &pair.owners,
            "CS:S engine physics is required",
        );
        only_physics_requirement(&output);
    }
}

fn entity_rows(bsp: &Bsp) -> Vec<BTreeMap<String, String>> {
    keyvalues::parse_flat_blocks_bounded(
        bsp.entities().unwrap(),
        FlatBlockLimits {
            max_bytes: 1024 * 1024,
            max_blocks: 1024,
            max_pairs: 16_384,
        },
    )
    .unwrap()
    .into_iter()
    .map(|row| row.into_iter().collect())
    .collect()
}

fn model_leaves(bsp: &Bsp, model: &Model) -> BTreeSet<usize> {
    let nodes = bsp.records::<BspNode>().unwrap();
    let leaves = bsp.records::<Leaf>().unwrap();
    let mut pending = vec![model.head_node];
    let mut seen = BTreeSet::new();
    let mut found = BTreeSet::new();
    while let Some(index) = pending.pop() {
        if index < 0 {
            let leaf = usize::try_from(-1_i64 - i64::from(index)).unwrap();
            assert!(leaf < leaves.len());
            found.insert(leaf);
        } else {
            let index = index as usize;
            assert!(seen.insert(index), "cycle or shared node in model tree");
            pending.extend(nodes[index].children);
        }
    }
    found
}

fn assert_pair_differential(dry: &Bsp, actual: &Bsp, reversed: bool) {
    let dry_models = dry.records::<Model>().unwrap();
    let actual_models = actual.records::<Model>().unwrap();
    assert_eq!(actual_models, dry_models);
    assert_eq!(actual_models.len(), 4);
    let rows = entity_rows(actual);
    let west = if reversed { 3 } else { 1 };
    let east = if reversed { 1 } else { 3 };
    let dry_leaves = dry.records::<Leaf>().unwrap();
    let dry_brushes = dry.records::<Brush>().unwrap();
    let leaf_brushes = dry.leaf_brushes().unwrap();
    let world_leaves = model_leaves(dry, &dry_models[0]);
    let mut owned_brushes = BTreeSet::new();
    let mut owned_leaves = BTreeSet::new();
    for (name, index, mins, maxs) in [
        ("west_pool", west, [-32.0, -48.0, -48.0], [32.0, 48.0, 48.0]),
        ("east_pool", east, [-32.0; 3], [32.0; 3]),
    ] {
        let row = rows
            .iter()
            .find(|row| row.get("targetname").map(String::as_str) == Some(name))
            .unwrap();
        assert_eq!(row["classname"], compiler::WATER_ANALOG_CLASSNAME);
        assert_eq!(row["model"], format!("*{index}"));
        assert_eq!(dry_models[index].mins, mins);
        assert_eq!(dry_models[index].maxs, maxs);
        assert_eq!(dry_models[index].face_count, 0);
        let occupied = model_leaves(dry, &dry_models[index])
            .into_iter()
            .filter(|&leaf| dry_leaves[leaf].contents == contents::SOLID)
            .collect::<Vec<_>>();
        assert_eq!(occupied.len(), 1);
        let leaf = occupied[0];
        assert_ne!(leaf, 0);
        assert!(!world_leaves.contains(&leaf));
        assert!(owned_leaves.insert(leaf));
        assert_eq!(dry_leaves[leaf].leaf_brush_count, 1);
        let brush = usize::from(leaf_brushes[usize::from(dry_leaves[leaf].first_leaf_brush)]);
        assert!(owned_brushes.insert(brush));
        assert_eq!(dry_brushes[brush].contents, contents::SOLID);
    }
    let ordinary = rows
        .iter()
        .find(|row| row.get("targetname").map(String::as_str) == Some("ordinary_body"))
        .unwrap();
    assert_eq!(ordinary["classname"], "func_brush");
    assert_eq!(ordinary["model"], "*2");
    let classname = format!("\"classname\" \"{}\"", compiler::WATER_ANALOG_CLASSNAME);
    assert_eq!(actual.entities().unwrap().matches(&classname).count(), 2);
    assert_eq!(
        actual
            .entities()
            .unwrap()
            .replace(&classname, "\"classname\" \"func_movelinear\""),
        dry.entities().unwrap()
    );
    let actual_brushes = actual.records::<Brush>().unwrap();
    assert_eq!(actual_brushes.len(), dry_brushes.len());
    for (index, actual) in actual_brushes.iter().enumerate() {
        let mut expected = dry_brushes[index];
        if owned_brushes.contains(&index) {
            expected.contents = contents::WATER;
        }
        assert_eq!(*actual, expected, "brush {index}");
    }
    let actual_leaves = actual.records::<Leaf>().unwrap();
    assert_eq!(actual_leaves.len(), dry_leaves.len());
    for (index, actual) in actual_leaves.iter().enumerate() {
        let mut expected = dry_leaves[index];
        if owned_leaves.contains(&index) {
            expected.contents = contents::WATER;
        }
        assert_eq!(*actual, expected, "leaf {index}");
    }
    assert_eq!(actual.version, dry.version);
    assert_eq!(actual.map_revision, dry.map_revision);
    assert_eq!(actual.game_lumps, dry.game_lumps);
    assert_eq!(actual.lumps.len(), source_formats::bsp::LUMP_COUNT);
    assert_eq!(dry.lumps.len(), source_formats::bsp::LUMP_COUNT);
    for (index, (actual, expected)) in actual.lumps.iter().zip(&dry.lumps).enumerate() {
        assert_eq!(actual.version, expected.version, "lump {index}");
        assert_eq!(actual.fourcc, expected.fourcc, "lump {index}");
        if ![lump::ENTITIES, lump::BRUSHES, lump::LEAFS].contains(&index) {
            assert_eq!(actual.data, expected.data, "lump {index}");
        }
    }
    let physics = actual.physics().unwrap().unwrap();
    assert_eq!(
        physics
            .models
            .iter()
            .map(|model| model.index)
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([0, 1, 2, 3])
    );
    for index in [west, east] {
        let body = physics
            .models
            .iter()
            .find(|model| model.index == index)
            .unwrap();
        assert_eq!(body.solids.len(), 1);
        assert!(matches!(
            body.solids[0].header(),
            CollisionHeader::Convex { .. }
        ));
        assert!(!body.solids[0].body().unwrap().is_empty());
        let keys = std::str::from_utf8(body.key_data)
            .unwrap()
            .trim_end_matches('\0');
        let nodes = keyvalues::parse(keys).unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].key, "solid");
        let fields = nodes[0].children().unwrap();
        let indices = fields
            .iter()
            .filter(|field| field.key.eq_ignore_ascii_case("index"))
            .collect::<Vec<_>>();
        assert_eq!(indices.len(), 1);
        assert_eq!(indices[0].as_str(), Some("0"));
    }
}

#[test]
#[ignore = "requires qualified hammerr-physics worker and installed CS:S engine physics via CSS_DIR"]
fn native_pair_matches_literal_dry_bodies_with_only_owned_water_deltas_in_both_root_orders() {
    let root = PathBuf::from(
        std::env::var_os("CSS_DIR").expect("set CSS_DIR for the explicit paired-water native cook"),
    );
    assert!(root.join("bin/linux64/vphysics.so").is_file());
    let (_directory, mut game) = game();
    game.base_dir = root;
    let options = Options {
        physics: PhysicsMode::Engine,
        ..Default::default()
    };
    for reversed in [false, true] {
        let pair = pair(reversed);
        let water = water(&pair);
        let dry_before = pair.document.vmf().clone();
        let water_before = water.vmf().clone();
        let revision = water.revision();
        let history = water.undo_labels().map(str::to_owned).collect::<Vec<_>>();
        let dry = compiler::compile_with_options(pair.document.vmf(), &game, options);
        assert!(!dry.report.has_errors(), "{:?}", dry.report);
        let dry = Bsp::parse(&dry.bsp.unwrap().to_bytes()).unwrap();
        let actual = compiler::compile_with_options(water.vmf(), &game, options);
        assert!(!actual.report.has_errors(), "{:?}", actual.report);
        let actual = Bsp::parse(&actual.bsp.unwrap().to_bytes()).unwrap();
        assert_pair_differential(&dry, &actual, reversed);
        assert_eq!(pair.document.vmf(), &dry_before);
        assert_eq!(water.vmf(), &water_before);
        assert_eq!(water.revision(), revision);
        assert_eq!(
            water.undo_labels().map(str::to_owned).collect::<Vec<_>>(),
            history
        );
    }
}
