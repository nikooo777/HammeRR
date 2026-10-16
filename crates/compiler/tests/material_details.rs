mod common;
#[path = "common/detail_model.rs"]
mod detail_model;

use glam::DVec3;
use map_core::{Document, ObjectId, Plane, edit, surface::FaceId};
use source_formats::bsp::{self, Bsp, Leaf};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::vmf::{Entity, Vmf};

const MATERIAL: &str = "test/scatter";
const MODEL: &str = "models/props/test.mdl";

struct Fixture {
    directory: tempfile::TempDir,
    game: GameFs,
    source: Vmf,
    floor_side: u32,
}

fn fixture() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    detail_model::write(directory.path(), "test");
    detail_model::write(directory.path(), "other");
    std::fs::create_dir_all(directory.path().join("materials/test")).unwrap();
    std::fs::write(
        directory.path().join("materials/test/scatter.vmt"),
        "UnlitGeneric { %detailtype tufts }",
    )
    .unwrap();
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().to_owned()),
    });
    let mut source = common::room();
    let floor = source.world.solids[0]
        .sides
        .iter_mut()
        .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
        .unwrap();
    floor.material = MATERIAL.into();
    let floor_side = floor.id;
    let fixture = Fixture {
        directory,
        game,
        source,
        floor_side,
    };
    dictionary(
        &fixture,
        64,
        "model models/props/test.mdl upright 1 minAngle 0 maxAngle 0",
    );
    fixture
}

fn dictionary(fixture: &Fixture, density: u32, model: &str) {
    std::fs::write(
        fixture.directory.path().join("detail.vbsp"),
        format!(
            "detail {{ tufts {{ density {density} group {{ alpha 1 item {{ {model} }} }} }} }}"
        ),
    )
    .unwrap();
}

fn compiled(source: &Vmf, game: &GameFs) -> compiler::Output {
    let output = compiler::compile(source, game);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.bsp.is_some());
    output
}

#[test]
fn material_details_use_real_records_stable_face_seeds_and_shared_explicit_models() {
    let mut fixture = fixture();
    let original = fixture.source.write(&Default::default());
    let first = compiled(&fixture.source, &fixture.game);
    assert_eq!(first.stats.detail_props, 16);
    assert_eq!(first.stats.detail_prop_models, 1);
    let bytes = first.bsp.unwrap().to_bytes();
    let bsp = Bsp::parse(&bytes).unwrap();
    let details = bsp.detail_props().unwrap().unwrap();
    let leaves = bsp.records::<Leaf>().unwrap();
    assert_eq!(details.model_name(0).unwrap(), MODEL);
    for prop in &details.props {
        assert_eq!(prop.origin[2], 0.0);
        assert!(
            prop.origin[..2]
                .iter()
                .all(|value| (-256.0..=256.0).contains(value))
        );
        assert_eq!(prop.angles[0], 0.0);
        assert_eq!(prop.angles[2], 0.0);
        assert_eq!(prop.kind, 0);
        assert_eq!(prop.scale, 1.0);
        assert_eq!(prop.lighting.rgb, [255; 3]);
        let leaf = leaves[usize::from(prop.leaf)];
        assert_eq!(leaf.contents & bsp::contents::SOLID, 0);
        assert!(leaf.cluster >= 0);
    }
    assert_eq!(fixture.source.write(&Default::default()), original);
    assert_eq!(
        compiled(&fixture.source, &fixture.game)
            .bsp
            .unwrap()
            .to_bytes(),
        bytes
    );
    let reopened = Vmf::parse(&original).unwrap();
    assert_eq!(
        compiled(&reopened, &fixture.game).bsp.unwrap().to_bytes(),
        bytes
    );
    std::fs::write(
        fixture.directory.path().join("materials/test/scatter.vmt"),
        "UnlitGeneric { }",
    )
    .unwrap();
    let baseline = compiled(&fixture.source, &fixture.game).bsp.unwrap();
    for index in 0..bsp::LUMP_COUNT {
        assert_eq!(
            bsp.lump(index),
            baseline.lump(index),
            "Unrelated lump {index} changed"
        );
    }
    assert_eq!(bsp.game_lump(*b"sprp"), baseline.game_lump(*b"sprp"));
    std::fs::write(
        fixture.directory.path().join("materials/test/scatter.vmt"),
        "UnlitGeneric { %detailtype tufts }",
    )
    .unwrap();
    fixture.source.world.solids[0]
        .sides
        .iter_mut()
        .find(|side| side.id == fixture.floor_side)
        .unwrap()
        .id += 100_000;
    let reseeded = compiled(&fixture.source, &fixture.game).bsp.unwrap();
    assert_ne!(
        reseeded.detail_props().unwrap().unwrap().props,
        details.props
    );
    let id = fixture
        .source
        .object_ids()
        .chain(fixture.source.side_ids())
        .max()
        .unwrap()
        + 1;
    let mut explicit = Entity::new(id, "prop_detail");
    explicit.properties.set("model", MODEL);
    explicit.properties.set("origin", "0 0 32");
    fixture.source.entities.push(explicit);
    let mixed = compiled(&fixture.source, &fixture.game);
    assert_eq!(mixed.stats.detail_props, 17);
    assert_eq!(mixed.stats.detail_prop_models, 1);
}

#[test]
fn native_dictionary_selection_uses_alpha_one_normalized_amounts_and_sparse_weights() {
    let mut fixture = fixture();
    std::fs::create_dir(fixture.directory.path().join("scripts")).unwrap();
    fixture
        .source
        .world
        .properties
        .set("detailvbsp", "scripts/custom.vbsp");
    std::fs::write(fixture.directory.path().join("scripts/custom.vbsp"),
        "detail { unrelated { group { sprite { sprite \"0 0 32 32 256\" } } } tufts { density 512 low { alpha 0 item { model models/props/other.mdl } } high { alpha 1 first { model models/props/test.mdl amount 3 upright 1 } second { model models/props/other.mdl amount 1 upright 1 } } } }")
        .unwrap();
    let output = compiled(&fixture.source, &fixture.game);
    let details = output.bsp.unwrap().detail_props().unwrap().unwrap();
    assert_eq!(details.props.len(), 134);
    assert_eq!(details.models.len(), 2);
    let first = details
        .props
        .iter()
        .filter(|prop| details.model_name(usize::from(prop.model)).unwrap() == MODEL)
        .count();
    assert!(
        (75..=120).contains(&first),
        "3:1 weighted selection emitted {first}/134"
    );
    dictionary(
        &fixture,
        512,
        "model models/props/test.mdl amount 0 upright 1",
    );
    fixture
        .source
        .world
        .properties
        .set("detailvbsp", "detail.vbsp");
    let empty = compiled(&fixture.source, &fixture.game);
    assert_eq!(empty.stats.detail_props, 0);
    assert!(
        empty
            .bsp
            .unwrap()
            .detail_props()
            .unwrap()
            .unwrap()
            .props
            .is_empty()
    );
    dictionary(
        &fixture,
        512,
        "model models/props/test.mdl amount 0.25 upright 1",
    );
    let sparse = compiled(&fixture.source, &fixture.game);
    assert!((15..=55).contains(&sparse.stats.detail_props));
}

#[test]
fn native_empty_alpha_groups_emit_only_the_selected_group_without_fallback() {
    let fixture = fixture();
    let plain_material = fixture.directory.path().join("materials/test/scatter.vmt");
    std::fs::write(&plain_material, "UnlitGeneric { }").unwrap();
    let no_details = compiled(&fixture.source, &fixture.game)
        .bsp
        .unwrap()
        .to_bytes();
    std::fs::write(&plain_material, "UnlitGeneric { %detailtype tufts }").unwrap();
    let path = fixture.directory.path().join("detail.vbsp");
    std::fs::write(&path,
        "detail { tufts { density 64 Group1 { alpha 1 grass { model models/props/test.mdl upright 1 } } Group0 { alpha 0 } } }")
        .unwrap();
    let populated = compiled(&fixture.source, &fixture.game);
    assert_eq!(populated.stats.detail_props, 16);
    assert_eq!(populated.stats.detail_prop_models, 1);
    assert!(
        !populated
            .report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("selects an empty group"))
    );
    for text in [
        "detail { tufts { density 64 Group0 { alpha 0 grass { model models/props/test.mdl upright 1 } } Group1 { alpha 1 } } }",
        "detail { tufts { density 64 Group0 { alpha 0 } } }",
    ] {
        std::fs::write(&path, text).unwrap();
        let output = compiled(&fixture.source, &fixture.game);
        assert_eq!(output.stats.detail_props, 0);
        assert_eq!(output.stats.detail_prop_models, 0);
        assert_eq!(
            output
                .report
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.message.contains("selects an empty group"))
                .count(),
            1
        );
        let bsp = output.bsp.unwrap();
        assert_eq!(bsp.to_bytes(), no_details);
        let details = bsp.detail_props().unwrap().unwrap();
        assert!(details.props.is_empty() && details.models.is_empty());
    }
}

#[test]
fn dictionary_failures_preserve_existing_output_and_source() {
    let fixture = fixture();
    let destination = fixture.directory.path().join("previous.bsp");
    let baseline = compiled(&fixture.source, &fixture.game)
        .bsp
        .unwrap()
        .to_bytes();
    std::fs::write(&destination, &baseline).unwrap();
    for dictionary in [
        "detail { tufts { density NaN group { item { model models/props/test.mdl } } } }",
        "detail { tufts { density -1 group { item { model models/props/test.mdl } } } }",
        "detail { tufts { density 1000000 group { item { model models/props/test.mdl amount 0 } } } }",
        "detail { tufts { density 100000 group { item { model models/props/test.mdl } } } }",
        "detail { tufts { density 64 group { item { sprite \"0 0 32 32 256\" } } } }",
        "detail { tufts { density 64 group { item { model models/props/test.mdl sprite_shape cross } } } }",
        "detail { tufts { density 64 group { item { model models/props/test.mdl detailOrientation 1 } } } }",
        "detail { tufts { density 64 group { item { model models/props/test.mdl amount inf } } } }",
        "detail { tufts { density 64 group { item { model models/props/../test.mdl } } } }",
        "detail { tufts { density 64 group { item { model models/props/missing.mdl } } } }",
        "detail { tufts { density 64 } }",
        "detail { tufts { density 64 group { alpha 1 item { model models/props/test.mdl } } other { alpha 1 item { model models/props/test.mdl } } } }",
        "detail { another { density 64 group { item { model models/props/test.mdl } } } }",
        "detail { tufts { density 64 group { item { model models/props/test.mdl } } }",
    ] {
        std::fs::write(fixture.directory.path().join("detail.vbsp"), dictionary).unwrap();
        let before = fixture.source.write(&Default::default());
        let output = compiler::compile(&fixture.source, &fixture.game);
        assert!(
            output.report.has_errors() && output.bsp.is_none(),
            "{dictionary}: {:?}",
            output.report
        );
        assert!(!output.write(&destination).unwrap());
        assert_eq!(std::fs::read(&destination).unwrap(), baseline);
        assert_eq!(fixture.source.write(&Default::default()), before);
    }
    for path in ["../detail.vbsp", "models/test.mdl", "missing.vbsp"] {
        let mut source = fixture.source.clone();
        source.world.properties.set("detailvbsp", path);
        let output = compiler::compile(&source, &fixture.game);
        assert!(
            output.report.has_errors() && output.bsp.is_none(),
            "{path}: {:?}",
            output.report
        );
    }
    let control = compiler::Control::default();
    control.cancel().unwrap();
    assert!(
        compiler::compile_controlled(&fixture.source, &fixture.game, &control)
            .bsp
            .is_none()
    );
}

#[test]
fn func_detail_surfaces_work_hidden_helpers_vanish_and_terrain_or_runtime_models_reject() {
    for (class, terrain, expected) in [
        ("func_detail", false, None),
        (
            "func_brush",
            false,
            Some("runtime brush models is unsupported"),
        ),
        ("func_detail", true, Some("on terrain is unsupported")),
    ] {
        let fixture = fixture();
        dictionary(
            &fixture,
            1024,
            "model models/props/test.mdl upright 1 minAngle 0 maxAngle 0",
        );
        let mut source = fixture.source.clone();
        source.world.solids[0]
            .sides
            .iter_mut()
            .find(|side| side.id == fixture.floor_side)
            .unwrap()
            .material = "dev/dev_measuregeneric01".into();
        let baseline = compiled(&source, &fixture.game).bsp.unwrap().to_bytes();
        let mut document = Document::new(source);
        let entity = document
            .edit("detail surface", |tx| {
                let ObjectId::Solid(solid) = edit::create_block(
                    tx,
                    DVec3::new(-64.0, -64.0, 0.0),
                    DVec3::new(64.0, 64.0, 16.0),
                    "dev/dev_measuregeneric01",
                )?
                else {
                    unreachable!()
                };
                let side = tx
                    .solid_mut(solid)?
                    .sides
                    .iter_mut()
                    .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
                    .unwrap();
                side.material = MATERIAL.into();
                let side = side.id;
                if terrain {
                    map_core::displacement::edit::create(tx, &[FaceId { solid, side }], 2, 0)?;
                }
                edit::tie_to_entity(tx, &[solid], class, &[])
            })
            .unwrap();
        let mut source = document.vmf().clone();
        let output = compiler::compile(&source, &fixture.game);
        if let Some(expected) = expected {
            assert!(
                output.bsp.is_none()
                    && output
                        .report
                        .diagnostics
                        .iter()
                        .any(|diagnostic| diagnostic.message.contains(expected)),
                "{:?}",
                output.report
            );
        } else {
            assert!(!output.report.has_errors(), "{:?}", output.report);
            assert_eq!(output.stats.detail_props, 16);
            let ObjectId::Entity(id) = entity else {
                unreachable!()
            };
            source
                .entities
                .iter_mut()
                .find(|entity| entity.id == id)
                .unwrap()
                .hidden = true;
            assert_eq!(
                compiled(&source, &fixture.game).bsp.unwrap().to_bytes(),
                baseline
            );
        }
    }
}

#[test]
fn unused_dictionaries_and_hidden_scatter_materials_leave_no_detail_maps_byte_exact() {
    let fixture = fixture();
    let mut source = fixture.source.clone();
    source.world.solids[0]
        .sides
        .iter_mut()
        .find(|side| side.id == fixture.floor_side)
        .unwrap()
        .material = "dev/dev_measuregeneric01".into();
    let before = compiled(&source, &fixture.game).bsp.unwrap().to_bytes();
    std::fs::write(
        fixture.directory.path().join("detail.vbsp"),
        "invalid dictionary",
    )
    .unwrap();
    source.world.properties.set("detailvbsp", "../unused.vbsp");
    let with_unused_setting = compiled(&source, &fixture.game).bsp.unwrap();
    assert_eq!(
        with_unused_setting
            .detail_props()
            .unwrap()
            .unwrap()
            .props
            .len(),
        0
    );
    source.world.properties.set("detailvbsp", "detail.vbsp");
    assert_eq!(
        compiled(&source, &fixture.game).bsp.unwrap().to_bytes(),
        before
    );
    let mut document = Document::new(source);
    document
        .edit("hidden scatter brush", |tx| {
            let ObjectId::Solid(id) = edit::create_block(
                tx,
                DVec3::new(32.0, 32.0, 0.0),
                DVec3::new(96.0, 96.0, 16.0),
                MATERIAL,
            )?
            else {
                unreachable!()
            };
            tx.solid_mut(id)?.hidden = true;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        compiled(document.vmf(), &fixture.game)
            .bsp
            .unwrap()
            .to_bytes(),
        before
    );
}
