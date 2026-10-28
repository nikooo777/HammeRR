mod common;

use compiler::materials::Materials;
use glam::DVec3;
use map_core::{Document, ObjectId, edit};
use source_formats::bsp::{
    Brush, BrushSide, Bsp, Face, Leaf, Model, Node, Plane, TexData, TexInfo, contents, surf,
};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::vmf::Vmf;

const MATERIAL: &str = "feedback/pass_bullets";
const GRATE_CONTENTS: i32 = contents::GRATE | contents::TRANSLUCENT;
const PLAYER_BRUSH_MASK: i32 = contents::SOLID
    | contents::MOVEABLE
    | contents::WINDOW
    | contents::PLAYERCLIP
    | contents::GRATE;
const GENERIC_SHOT_MASK: i32 = contents::SOLID
    | contents::MOVEABLE
    | contents::MONSTER
    | contents::WINDOW
    | contents::DEBRIS
    | contents::HITBOX;
const CSS_SHOT_MASK: i32 = GENERIC_SHOT_MASK | contents::GRATE;

fn game(fields: &str) -> (tempfile::TempDir, GameFs) {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/validation/950-pass-bullets");
    std::fs::create_dir_all(&root).unwrap();
    let directory = tempfile::tempdir_in(root).unwrap();
    let materials = directory.path().join("materials/feedback");
    std::fs::create_dir_all(&materials).unwrap();
    std::fs::write(
        materials.join("pass_bullets.vmt"),
        format!("LightmappedGeneric {{ %compilepassbullets 1 {fields} }}"),
    )
    .unwrap();
    std::fs::write(
        materials.join("nodraw.vmt"),
        "LightmappedGeneric { %compilenodraw 1 }",
    )
    .unwrap();
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().to_owned()),
    });
    (directory, game)
}

fn scene(material: &str, mixed_nodraw: bool) -> Vmf {
    let mut document = Document::new(common::room());
    document
        .edit("grate collision", |tx| {
            let ObjectId::Solid(solid) = edit::create_block(
                tx,
                DVec3::new(-96.0, 64.0, 48.0),
                DVec3::new(96.0, 96.0, 80.0),
                material,
            )?
            else {
                unreachable!()
            };
            if mixed_nodraw {
                tx.solid_mut(solid)?.sides[0].material = "feedback/nodraw".into();
            }
            Ok(())
        })
        .unwrap();
    document.vmf().clone()
}

fn leaf_at(bsp: &Bsp, point: DVec3) -> usize {
    let nodes = bsp.records::<Node>().unwrap();
    let planes = bsp.records::<Plane>().unwrap();
    let mut next = bsp.records::<Model>().unwrap()[0].head_node;
    while next >= 0 {
        let node = nodes[next as usize];
        let plane = planes[node.plane as usize];
        let distance =
            DVec3::from_array(plane.normal.map(f64::from)).dot(point) - f64::from(plane.dist);
        next = node.children[usize::from(distance < 0.0)];
    }
    (-1 - next) as usize
}

fn assert_grate(bsp: &Bsp, material: &str, surface: i32, detail: bool) {
    let brushes = bsp.records::<Brush>().unwrap();
    let expected = GRATE_CONTENTS | if detail { contents::DETAIL } else { 0 };
    let grate = brushes
        .iter()
        .position(|brush| brush.contents == expected)
        .expect("compiled GRATE collision brush");
    assert_eq!(
        brushes
            .iter()
            .filter(|brush| brush.contents & contents::GRATE != 0)
            .count(),
        1
    );
    let sides = bsp.records::<BrushSide>().unwrap();
    let brush = brushes[grate];
    assert_eq!(brush.contents & PLAYER_BRUSH_MASK, contents::GRATE);
    assert_eq!(brush.contents & GENERIC_SHOT_MASK, 0);
    assert_eq!(brush.contents & CSS_SHOT_MASK, contents::GRATE);
    let range = brush.first_side as usize..(brush.first_side + brush.side_count) as usize;
    assert!(sides[range].iter().filter(|side| side.bevel == 0).count() >= 6);
    let names = bsp.texdata_strings().unwrap();
    let texdata = bsp.records::<TexData>().unwrap();
    let infos = bsp.records::<TexInfo>().unwrap();
    let is_material = |index: usize| {
        names[texdata[infos[index].texdata as usize].name_index as usize]
            .eq_ignore_ascii_case(material)
    };
    let material_infos = infos
        .iter()
        .enumerate()
        .filter(|(index, _)| is_material(*index))
        .collect::<Vec<_>>();
    assert!(!material_infos.is_empty());
    for (_, info) in material_infos {
        assert_eq!(info.flags, surface);
    }
    assert!(
        bsp.records::<Face>()
            .unwrap()
            .iter()
            .any(|face| is_material(face.texinfo as usize))
    );
    let leaves = bsp.records::<Leaf>().unwrap();
    let leaf = leaves[leaf_at(bsp, DVec3::new(0.0, 80.0, 64.0))];
    assert_eq!(leaf.contents & contents::SOLID, 0);
    assert!(leaf.cluster >= 0);
    if !detail {
        assert_eq!(leaf.contents, expected);
    }
    let leaf_brushes = bsp.leaf_brushes().unwrap();
    let range = usize::from(leaf.first_leaf_brush)
        ..usize::from(leaf.first_leaf_brush) + usize::from(leaf.leaf_brush_count);
    assert!(leaf_brushes[range].contains(&(grate as u16)));
}

#[test]
fn grate_brushes_preserve_native_contents_surfaces_and_leaf_collision_membership() {
    for (fields, surface, detail) in [
        ("", 0, false),
        ("$alphatest 1 $surfaceprop metalgrate", 0, false),
        ("$translucent 1 $surfaceprop chainlink", surf::TRANS, false),
        ("$alphatest 1 %compiledetail 1", 0, true),
    ] {
        let (_directory, game) = game(fields);
        let vmf = scene(MATERIAL, true);
        let original = vmf.clone();
        let output = compiler::compile(&vmf, &game);
        assert!(!output.report.has_errors(), "{fields}: {:?}", output.report);
        assert!(output.leak.is_none(), "{fields}: {:?}", output.report);
        assert!(
            !output
                .report
                .diagnostics
                .iter()
                .any(|item| item.message.contains("unsupported compile flags"))
        );
        let bytes = output.bsp.unwrap().to_bytes();
        assert_grate(&Bsp::parse(&bytes).unwrap(), MATERIAL, surface, detail);
        assert_eq!(vmf, original);
    }
}

#[test]
fn grate_cannot_seal_a_world_and_conflicting_materials_fail_with_their_source_path() {
    let (_directory, game) = game("");
    let mut vmf = common::room();
    for side in &mut vmf.world.solids[0].sides {
        side.material = MATERIAL.into();
    }
    let output = compiler::compile(&vmf, &game);
    assert!(output.leak.is_some());
    assert!(output.bsp.is_none());
    for fields in ["%compilewater 1", "%compilenolight 1", "%compilefog 1"] {
        let (_directory, game) = self::game(fields);
        let error = Materials::new(&game).load(MATERIAL).unwrap_err();
        assert!(
            error.contains("materials/feedback/pass_bullets.vmt"),
            "{error}"
        );
        assert!(
            error.contains("%compilepassbullets")
                && error.contains(fields.split_whitespace().next().unwrap()),
            "{error}"
        );
        let output = compiler::compile(&scene(MATERIAL, false), &game);
        assert!(output.bsp.is_none() && output.report.has_errors());
    }
    let (directory, game) = self::game("");
    let destination = directory.path().join("previous.bsp");
    std::fs::write(&destination, b"previous successful output").unwrap();
    for flag in ["%compileclip", "%compilewater", "%compileladder"] {
        std::fs::write(
            directory.path().join("materials/feedback/special.vmt"),
            format!("LightmappedGeneric {{ {flag} 1 }}"),
        )
        .unwrap();
        let mut vmf = scene(MATERIAL, false);
        vmf.world.solids.last_mut().unwrap().sides[0].material = "feedback/special".into();
        let before = vmf.clone();
        let output = compiler::compile(&vmf, &game);
        assert!(output.bsp.is_none() && output.report.has_errors());
        assert!(
            output.report.diagnostics.iter().any(|item| {
                item.message.contains("GRATE material cannot share a brush")
                    && item.object.is_some()
                    && item.position.is_some()
            }),
            "{flag}: {:?}",
            output.report
        );
        assert!(!output.write(&destination).unwrap());
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            b"previous successful output"
        );
        assert_eq!(vmf, before);
    }
    std::fs::write(
        directory.path().join("materials/feedback/special.vmt"),
        "LightmappedGeneric { %compileorigin 1 }",
    )
    .unwrap();
    for origin_first in [false, true] {
        let mut vmf = scene(MATERIAL, false);
        let solid = vmf.world.solids.last_mut().unwrap();
        let id = solid.id;
        let index = if origin_first {
            0
        } else {
            solid.sides.len() - 1
        };
        solid.sides[index].material = "feedback/special".into();
        let mut document = Document::new(vmf);
        document
            .edit("mixed origin", |tx| {
                edit::tie_to_entity(tx, &[id], "func_brush", &[])?;
                Ok(())
            })
            .unwrap();
        let before = document.vmf().clone();
        let output = compiler::compile(document.vmf(), &game);
        assert!(output.bsp.is_none() && output.report.has_errors());
        assert!(
            output.report.diagnostics.iter().any(|item| {
                item.message.contains("mixes GRATE and ORIGIN")
                    && item.message.contains(&id.to_string())
            }),
            "{:?}",
            output.report
        );
        assert_eq!(document.vmf(), &before);
    }
}

#[test]
#[ignore = "requires CSS_DIR pointing to the installed Counter-Strike Source root"]
fn installed_stock_grates_keep_their_original_materials_and_native_fields() {
    let root = std::env::var_os("CSS_DIR").expect("set CSS_DIR to the installed CSS root");
    let game =
        GameFs::from_gameinfo(std::path::Path::new(&root).join("cstrike/gameinfo.txt")).unwrap();
    for (material, alpha, surface) in [
        ("de_nuke/nuke_metalgrate_01", "$alphatest", 0),
        ("metal/metalgrate004a", "$alphatest", 0),
        ("metal/metalfence007a", "$translucent", surf::TRANS),
    ] {
        let text = game
            .read(&format!("materials/{material}.vmt"))
            .unwrap()
            .unwrap();
        let original =
            source_formats::vmt::Material::parse(std::str::from_utf8(&text).unwrap()).unwrap();
        assert!(original.compile_flag("%compilepassbullets"));
        assert_eq!(original.get(alpha), Some("1"));
        let vmf = scene(material, false);
        let before = vmf.clone();
        let output = compiler::compile(&vmf, &game);
        assert!(
            !output.report.has_errors(),
            "{material}: {:?}",
            output.report
        );
        assert!(output.leak.is_none());
        assert!(
            !output
                .report
                .diagnostics
                .iter()
                .any(|item| item.message.contains("unsupported compile flags"))
        );
        assert_grate(
            &Bsp::parse(&output.bsp.unwrap().to_bytes()).unwrap(),
            material,
            surface,
            false,
        );
        assert_eq!(vmf, before);
    }
}
