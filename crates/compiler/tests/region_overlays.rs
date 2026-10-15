mod common;
#[path = "common/overlays.rs"]
mod overlay_fixture;

use compiler::Control;
use glam::DVec3;
use map_core::{Document, ObjectId, Plane, compile_region, edit, surface::FaceId};
use overlay_fixture::add_overlay;
use source_formats::{
    bsp::Face,
    gamefs::{GameFs, Mount, MountedPath},
    vmf::{Entity, Vmf},
};

struct Fixture {
    directory: tempfile::TempDir,
    game: GameFs,
    source: Vmf,
    sides: [u32; 3],
    world: u32,
    detail: u32,
    terrain: u32,
}

fn fixture() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(directory.path().join("materials/tools")).unwrap();
    std::fs::write(
        directory.path().join("materials/tools/toolsblack.vmt"),
        "UnlitGeneric { }",
    )
    .unwrap();
    let mut game = common::empty_game();
    game.game_dir = directory.path().to_owned();
    game.base_dir = directory.path().to_owned();
    game.mounts = vec![MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().to_owned()),
    }];
    let mut document = Document::new(common::room());
    let (world, detail, terrain, sides) = document
        .edit("overlay region surfaces", |tx| {
            let mut ids = Vec::new();
            let mut sides = Vec::new();
            for x in [-112.0, -32.0, 48.0] {
                let ObjectId::Solid(id) = edit::create_block(
                    tx,
                    DVec3::new(x, 32.0, 0.0),
                    DVec3::new(x + 64.0, 96.0, 32.0),
                    "dev/dev_measuregeneric01",
                )?
                else {
                    unreachable!()
                };
                let side = tx
                    .solid_mut(id)?
                    .sides
                    .iter()
                    .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
                    .unwrap()
                    .id;
                ids.push(id);
                sides.push(side);
            }
            let ObjectId::Entity(detail) = edit::tie_to_entity(tx, &[ids[1]], "func_detail", &[])?
            else {
                unreachable!()
            };
            map_core::displacement::edit::create(
                tx,
                &[FaceId {
                    solid: ids[2],
                    side: sides[2],
                }],
                2,
                0,
            )?;
            compile_region::apply(
                tx,
                Some(compile_region::Definition {
                    enabled: true,
                    bounds: compile_region::Bounds {
                        mins: DVec3::new(-128.0, -128.0, 0.0),
                        maxs: DVec3::new(128.0, 128.0, 256.0),
                    },
                }),
            )?;
            Ok((ids[0], detail, ids[2], sides.try_into().unwrap()))
        })
        .unwrap();
    let source = Vmf::parse(&document.vmf().write(&Default::default())).unwrap();
    Fixture {
        directory,
        game,
        source,
        sides,
        world,
        detail,
        terrain,
    }
}

fn helper(source: &mut Vmf) -> &mut Entity {
    source.entities.last_mut().unwrap()
}

fn reject(source: &Vmf, game: &GameFs, path: &std::path::Path, message: &str) {
    let before = source.clone();
    let previous = std::fs::read(path).unwrap();
    for controlled in [false, true] {
        let output = if controlled {
            compiler::compile_controlled(source, game, &Control::default())
        } else {
            compiler::compile(source, game)
        };
        assert!(output.bsp.is_none() && output.report.has_errors());
        assert!(
            output
                .report
                .diagnostics
                .iter()
                .any(|item| item.message.contains(message)),
            "{message}: {:?}",
            output.report
        );
        assert!(!output.write(path).unwrap());
        assert_eq!(std::fs::read(path).unwrap(), previous);
        assert_eq!(source, &before);
    }
}

#[test]
fn whole_overlays_keep_outside_markers_and_bind_retained_world_detail_and_terrain() {
    let fixture = fixture();
    for sides in [
        &fixture.sides[..1],
        &fixture.sides[1..2],
        &fixture.sides[2..],
        &fixture.sides[..],
    ] {
        let source = add_overlay(fixture.source.clone(), sides);
        let before = source.clone();
        let output = compiler::compile(&source, &fixture.game);
        assert!(!output.report.has_errors(), "{:?}", output.report);
        assert!(output.leak.is_none());
        let bsp = output.bsp.as_ref().unwrap();
        let data = bsp.overlay_data().unwrap().unwrap();
        assert_eq!(output.stats.overlays, 1);
        assert_eq!(data.overlays.len(), 1);
        let overlay = data.overlays[0];
        assert!(overlay.face_count() >= sides.len());
        assert_eq!(output.stats.overlay_face_refs, overlay.face_count());
        assert_eq!(overlay.origin, [0.0, 0.0, 32.0]);
        assert_eq!(overlay.basis_normal, [0.0, 0.0, 1.0]);
        let terrain = bsp.displacements().unwrap().unwrap();
        assert_eq!(
            overlay.faces[..overlay.face_count()].contains(&i32::from(terrain.infos[0].face)),
            sides.contains(&fixture.sides[2])
        );
        let faces = bsp.records::<Face>().unwrap();
        for face in &overlay.faces[..overlay.face_count()] {
            assert!((*face as usize) < faces.len());
        }
        let entity = source.entities.last().unwrap();
        assert_eq!(entity.origin(), Some(DVec3::new(1000.0, 0.0, 32.0)));
        assert!(
            output
                .stats
                .compile_region
                .as_ref()
                .unwrap()
                .included
                .contains(&format!("entity:{}", entity.id))
        );
        let entities = bsp.entities().unwrap();
        assert!(entities.contains("\"classname\" \"info_overlay_accessor\""));
        assert!(entities.contains("\"origin\" \"0 0 32\""));
        assert!(!entities.contains("\"classname\" \"info_overlay\""));
        let controlled = compiler::compile_controlled(&source, &fixture.game, &Control::default());
        assert!(!controlled.report.has_errors(), "{:?}", controlled.report);
        assert_eq!(controlled.bsp.unwrap().to_bytes(), bsp.to_bytes());
        assert_eq!(source, before);
    }
}

#[test]
fn partially_omitted_or_non_emitted_targets_preserve_previous_output() {
    let fixture = fixture();
    let valid = add_overlay(fixture.source.clone(), &fixture.sides);
    let path = fixture.directory.path().join("previous.bsp");
    let previous = compiler::compile(&valid, &fixture.game);
    assert!(previous.write(&path).unwrap(), "{:?}", previous.report);
    for (object, side) in [
        (ObjectId::Solid(fixture.world), fixture.sides[0]),
        (ObjectId::Entity(fixture.detail), fixture.sides[1]),
        (ObjectId::Solid(fixture.terrain), fixture.sides[2]),
    ] {
        let mut document = Document::new(valid.clone());
        document
            .edit("omit overlay target", |tx| {
                edit::translate(tx, &[object], DVec3::X * 512.0, true)
            })
            .unwrap();
        reject(
            document.vmf(),
            &fixture.game,
            &path,
            &format!("Overlay side {side} is absent"),
        );
    }
    let mut buried = valid.clone();
    let bottom = buried.world.solids[0]
        .sides
        .iter()
        .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z < -0.99)
        .unwrap()
        .id;
    helper(&mut buried)
        .properties
        .set("sides", format!("{} {bottom}", fixture.sides[0]));
    reject(&buried, &fixture.game, &path, "has no emitted world face");
}

#[test]
fn malformed_outside_helpers_reach_normal_overlay_validation() {
    let fixture = fixture();
    let valid = add_overlay(fixture.source.clone(), &fixture.sides);
    let path = fixture.directory.path().join("previous.bsp");
    assert!(
        compiler::compile(&valid, &fixture.game)
            .write(&path)
            .unwrap()
    );
    let mut malformed = valid.clone();
    helper(&mut malformed).properties.set("sides", "oops");
    reject(&malformed, &fixture.game, &path, "integer VMF side IDs");
    let mut brush_owned = valid.clone();
    let solid = brush_owned.world.solids.pop().unwrap();
    helper(&mut brush_owned).solids.push(solid);
    reject(
        &brush_owned,
        &fixture.game,
        &path,
        "info_overlay is a point helper",
    );
}

#[test]
fn hidden_helpers_and_disabled_regions_preserve_exact_baselines() {
    let fixture = fixture();
    let baseline = compiler::compile(&fixture.source, &fixture.game)
        .bsp
        .unwrap()
        .to_bytes();
    let mut hidden = add_overlay(fixture.source.clone(), &fixture.sides);
    helper(&mut hidden).hidden = true;
    helper(&mut hidden).properties.set("sides", "malformed");
    assert_eq!(
        compiler::compile(&hidden, &fixture.game)
            .bsp
            .unwrap()
            .to_bytes(),
        baseline
    );
    let mut full = add_overlay(fixture.source, &fixture.sides);
    full.cordon = None;
    let before = full.clone();
    let baseline = compiler::compile(&full, &fixture.game)
        .bsp
        .unwrap()
        .to_bytes();
    let mut document = Document::new(full);
    document
        .edit("disabled overlay region", |tx| {
            compile_region::apply(
                tx,
                Some(compile_region::Definition {
                    enabled: false,
                    bounds: compile_region::Bounds {
                        mins: DVec3::new(-128.0, -128.0, 0.0),
                        maxs: DVec3::new(128.0, 128.0, 256.0),
                    },
                }),
            )
        })
        .unwrap();
    let disabled = document.into_vmf();
    let saved = disabled.clone();
    for source in [&before, &disabled] {
        let output = compiler::compile_controlled(source, &fixture.game, &Control::default());
        assert!(!output.report.has_errors(), "{:?}", output.report);
        assert_eq!(output.bsp.unwrap().to_bytes(), baseline);
    }
    assert_eq!(disabled, saved);
}
