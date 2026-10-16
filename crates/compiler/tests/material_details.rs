mod common;
#[path = "common/detail_model.rs"]
mod detail_model;

use glam::{DQuat, DVec3, EulerRot};
use map_core::displacement::DisplacementMesh;
use map_core::{BrushGeometry, Document, ObjectId, Plane, compile_region, edit, surface::FaceId};
use source_formats::bsp::{self, Bsp, Leaf, Model as BspModel};
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
        "detail { tufts { density 64 group { item { sprite \"0 0 32 32 256\" detailOrientation 0.5 } } } }",
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
fn func_detail_brush_and_terrain_surfaces_work_while_runtime_models_reject() {
    for (class, terrain, expected) in [
        ("func_detail", false, None),
        (
            "func_brush",
            false,
            Some("runtime brush models is unsupported"),
        ),
        ("func_detail", true, None),
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
fn compile_regions_scatter_only_inside_open_clipped_world_and_detail_surfaces() {
    let bounds = compile_region::Bounds {
        mins: DVec3::new(-128.0, -128.0, -8.0),
        maxs: DVec3::new(128.0, 128.0, 160.0),
    };
    for detail in [false, true] {
        let fixture = fixture();
        std::fs::create_dir_all(fixture.directory.path().join("materials/tools")).unwrap();
        std::fs::write(
            fixture
                .directory
                .path()
                .join("materials/tools/toolsblack.vmt"),
            "UnlitGeneric { }",
        )
        .unwrap();
        dictionary(
            &fixture,
            if detail { 4096 } else { 512 },
            "model models/props/test.mdl upright 1 minAngle 0 maxAngle 0",
        );
        let mut source = fixture.source.clone();
        if detail {
            source.world.solids[0]
                .sides
                .iter_mut()
                .find(|side| side.id == fixture.floor_side)
                .unwrap()
                .material = "dev/dev_measuregeneric01".into();
        }
        let mut document = Document::new(source);
        if detail {
            document
                .edit("crossing detail surface", |tx| {
                    let ObjectId::Solid(solid) = edit::create_block(
                        tx,
                        DVec3::new(-192.0, 32.0, 0.0),
                        DVec3::new(192.0, 96.0, 24.0),
                        "dev/dev_measuregeneric01",
                    )?
                    else {
                        unreachable!()
                    };
                    tx.solid_mut(solid)?
                        .sides
                        .iter_mut()
                        .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
                        .unwrap()
                        .material = MATERIAL.into();
                    edit::tie_to_entity(tx, &[solid], "func_detail", &[])?;
                    Ok(())
                })
                .unwrap();
        }
        let full = compiled(document.vmf(), &fixture.game).bsp.unwrap();
        let full_details = full.detail_props().unwrap().unwrap();
        assert!(
            full_details
                .props
                .iter()
                .any(|prop| !bounds.contains(DVec3::from_array(prop.origin.map(f64::from))))
        );
        document
            .edit("active scatter compile region", |tx| {
                compile_region::apply(
                    tx,
                    Some(compile_region::Definition {
                        enabled: true,
                        bounds,
                    }),
                )
            })
            .unwrap();
        let source_before = document.vmf().write(&Default::default());
        let revision_before = document.revision();
        let undo_before = document
            .undo_labels()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let redo_before = document
            .redo_labels()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let output = compiled(document.vmf(), &fixture.game);
        let summary = output.stats.compile_region.as_ref().unwrap();
        assert_eq!(summary.mins, bounds.mins.to_array());
        assert_eq!(summary.maxs, bounds.maxs.to_array());
        let bsp = output.bsp.unwrap();
        let details = bsp.detail_props().unwrap().unwrap();
        assert!(!details.props.is_empty() && details.props.len() < full_details.props.len());
        let world = bsp.records::<BspModel>().unwrap()[0];
        let world_bounds = compile_region::Bounds {
            mins: DVec3::from_array(world.mins.map(f64::from)),
            maxs: DVec3::from_array(world.maxs.map(f64::from)),
        };
        let leaves = bsp.records::<Leaf>().unwrap();
        for prop in &details.props {
            let point = DVec3::from_array(prop.origin.map(f64::from));
            assert!(bounds.contains(point) && world_bounds.contains(point));
            assert_eq!(point.z, if detail { 24.0 } else { 0.0 });
            let leaf = leaves[usize::from(prop.leaf)];
            assert_eq!(leaf.contents & bsp::contents::SOLID, 0);
            assert!(leaf.cluster >= 0);
            let leaf_bounds = compile_region::Bounds {
                mins: DVec3::from_array(leaf.mins.map(f64::from)),
                maxs: DVec3::from_array(leaf.maxs.map(f64::from)),
            };
            assert!(leaf_bounds.contains(point));
        }
        let repeated = compiler::compile_controlled(
            document.vmf(),
            &fixture.game,
            &compiler::Control::default(),
        );
        assert!(!repeated.report.has_errors(), "{:?}", repeated.report);
        assert_eq!(repeated.bsp.unwrap().to_bytes(), bsp.to_bytes());
        assert_eq!(document.vmf().write(&Default::default()), source_before);
        assert_eq!(document.revision(), revision_before);
        assert_eq!(
            document
                .undo_labels()
                .map(str::to_owned)
                .collect::<Vec<_>>(),
            undo_before
        );
        assert_eq!(
            document
                .redo_labels()
                .map(str::to_owned)
                .collect::<Vec<_>>(),
            redo_before
        );
        assert!(document.undo().is_some());
        assert_eq!(compile_region::active(document.vmf()).unwrap(), None);
    }
}

fn add_scatter_terrain(fixture: &mut Fixture, power: u32, corner: u32) -> u32 {
    fixture.source.world.solids[0]
        .sides
        .iter_mut()
        .find(|side| side.id == fixture.floor_side)
        .unwrap()
        .material = "dev/dev_measuregeneric01".into();
    let mut document = Document::new(fixture.source.clone());
    let id = document
        .edit("model detail terrain", |tx| {
            let ObjectId::Solid(solid) = edit::create_block(
                tx,
                DVec3::new(-128.0, 32.0, 0.0),
                DVec3::new(128.0, 160.0, 24.0),
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
            map_core::displacement::edit::create(tx, &[FaceId { solid, side }], power, corner)?;
            Ok(solid)
        })
        .unwrap();
    fixture.source = document.vmf().clone();
    let solid = fixture
        .source
        .world
        .solids
        .iter_mut()
        .find(|solid| solid.id == id)
        .unwrap();
    let geometry = BrushGeometry::from_solid(solid).unwrap();
    let face = geometry
        .faces
        .iter()
        .find(|face| solid.sides[face.side].displacement.is_some())
        .unwrap();
    let info = solid.sides[face.side].displacement.as_mut().unwrap();
    let mesh = DisplacementMesh::build(face, info).unwrap();
    let size = mesh.size;
    info.normals = Some(vec![vec![DVec3::Z; size]; size]);
    info.distances = Some(
        (0..size)
            .map(|row| {
                (0..size)
                    .map(|col| (row * row + 2 * col) as f64 * 0.125)
                    .collect()
            })
            .collect(),
    );
    info.alphas = Some(
        mesh.base_positions
            .chunks(size)
            .map(|row| {
                row.iter()
                    .map(|point| (point.x + 128.0) / 256.0 * 255.0)
                    .collect()
            })
            .collect(),
    );
    id
}

fn terrain_info_mut(source: &mut Vmf, solid: u32) -> &mut source_formats::vmf::DispInfo {
    source
        .world
        .solids
        .iter_mut()
        .find(|candidate| candidate.id == solid)
        .unwrap()
        .sides
        .iter_mut()
        .find_map(|side| side.displacement.as_mut())
        .unwrap()
}

fn blend_dictionary(fixture: &Fixture, density: u32) {
    std::fs::write(fixture.directory.path().join("detail.vbsp"), format!(
        "detail {{ tufts {{ density {density} low {{ alpha 0 item {{ model models/props/test.mdl }} }} high {{ alpha 1 item {{ model models/props/other.mdl }} }} }} }}")).unwrap();
}

fn assert_native_terrain_contacts(bsp: &Bsp) {
    let data = bsp.displacements().unwrap().unwrap();
    let mut triangles = Vec::new();
    for (index, info) in data.infos.iter().enumerate() {
        let points = data
            .positions(index, bsp.displacement_quad(index).unwrap())
            .unwrap();
        for indices in info.allowed_triangle_indices().unwrap() {
            triangles.push(indices.map(|vertex| DVec3::from_array(points[vertex].map(f64::from))));
        }
    }
    let leaves = bsp.records::<Leaf>().unwrap();
    for prop in bsp.detail_props().unwrap().unwrap().props {
        let point = DVec3::from_array(prop.origin.map(f64::from));
        let normal = triangles
            .iter()
            .find_map(|&[a, b, c]| {
                let normal = (c - a).cross(b - a).normalize();
                if (point - a).dot(normal).abs() > 0.002 {
                    return None;
                }
                let ab = b - a;
                let ac = c - a;
                let p = point - a;
                let denominator = ab.length_squared() * ac.length_squared() - ab.dot(ac).powi(2);
                let u = (p.dot(ab) * ac.length_squared() - p.dot(ac) * ab.dot(ac)) / denominator;
                let v = (p.dot(ac) * ab.length_squared() - p.dot(ab) * ab.dot(ac)) / denominator;
                (u >= -1e-4 && v >= -1e-4 && u + v <= 1.0 + 1e-4).then_some(normal)
            })
            .expect("Generated detail origin is not on a final native terrain triangle");
        assert!(normal.z > 0.5);
        let values = prop.angles.map(|value| f64::from(value).to_radians());
        let rotation = DQuat::from_euler(EulerRot::ZYX, values[1], values[0], values[2]);
        assert!((rotation * DVec3::Z - normal).length() < 1e-4);
        let leaf = leaves[usize::from(prop.leaf)];
        assert_eq!(leaf.contents & bsp::contents::SOLID, 0);
        assert!(leaf.cluster >= 0);
    }
}

#[test]
fn terrain_uses_whole_base_area_final_native_triangles_and_interpolated_alpha_groups() {
    for (power, corner) in [(2, 0), (3, 1), (4, 2), (4, 3)] {
        let mut fixture = fixture();
        add_scatter_terrain(&mut fixture, power, corner);
        blend_dictionary(&fixture, 8192);
        let before = fixture.source.write(&Default::default());
        let output = compiled(&fixture.source, &fixture.game);
        assert_eq!(output.stats.detail_props, 268);
        assert_eq!(output.stats.detail_prop_models, 2);
        let bsp = output.bsp.unwrap();
        assert_native_terrain_contacts(&bsp);
        let props = bsp.detail_props().unwrap().unwrap();
        for (negative, expected) in [(true, MODEL), (false, "models/props/other.mdl")] {
            let region = props
                .props
                .iter()
                .filter(|prop| {
                    if negative {
                        prop.origin[0] < -96.0
                    } else {
                        prop.origin[0] > 96.0
                    }
                })
                .collect::<Vec<_>>();
            let selected = region
                .iter()
                .filter(|prop| props.model_name(usize::from(prop.model)).unwrap() == expected)
                .count();
            assert!(region.len() > 15 && selected * 10 > region.len() * 6);
        }
        let reopened = Vmf::parse(&before).unwrap();
        assert_eq!(
            compiled(&reopened, &fixture.game).bsp.unwrap().to_bytes(),
            bsp.to_bytes()
        );
        assert_eq!(
            compiled(&fixture.source, &fixture.game)
                .bsp
                .unwrap()
                .to_bytes(),
            bsp.to_bytes()
        );
        assert_eq!(fixture.source.write(&Default::default()), before);
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
                "Unrelated terrain lump {index} changed"
            );
        }
    }
}

#[test]
fn terrain_scatter_accepts_sheared_and_sloped_parallelogram_bases() {
    let mut fixture = fixture();
    let solid = add_scatter_terrain(&mut fixture, 3, 3);
    blend_dictionary(&fixture, 8192);
    let terrain = fixture
        .source
        .world
        .solids
        .iter_mut()
        .find(|candidate| candidate.id == solid)
        .unwrap();
    for side in &mut terrain.sides {
        for point in &mut side.plane.0 {
            point.x += point.y * 0.125;
            point.z += point.y * 0.0625;
        }
        if let Some(info) = &mut side.displacement {
            info.start_position.x += info.start_position.y * 0.125;
            info.start_position.z += info.start_position.y * 0.0625;
        }
    }
    let before = fixture.source.write(&Default::default());
    let output = compiled(&fixture.source, &fixture.game);
    assert!(output.stats.detail_props > 250);
    assert_native_terrain_contacts(&output.bsp.unwrap());
    assert_eq!(fixture.source.write(&Default::default()), before);
}

#[test]
fn terrain_alpha_endpoints_and_empty_groups_never_fall_back_to_another_group() {
    let mut fixture = fixture();
    let solid = add_scatter_terrain(&mut fixture, 2, 0);
    for (alpha, expected) in [(0.0, MODEL), (255.0, "models/props/other.mdl")] {
        blend_dictionary(&fixture, 8192);
        terrain_info_mut(&mut fixture.source, solid).alphas = Some(vec![vec![alpha; 5]; 5]);
        let output = compiled(&fixture.source, &fixture.game);
        assert_eq!(output.stats.detail_props, 268);
        let details = output.bsp.unwrap().detail_props().unwrap().unwrap();
        assert_eq!(details.models.len(), 1);
        assert_eq!(details.model_name(0).unwrap(), expected);
    }
    std::fs::write(fixture.directory.path().join("detail.vbsp"),
        "detail { tufts { density 8192 low { alpha 0 } high { alpha 1 item { model models/props/test.mdl } } } }").unwrap();
    terrain_info_mut(&mut fixture.source, solid).alphas = Some(vec![vec![0.0; 5]; 5]);
    let empty = compiled(&fixture.source, &fixture.game);
    assert_eq!(empty.stats.detail_props, 0);
    assert_eq!(empty.stats.detail_prop_models, 0);
    let details = empty.bsp.unwrap().detail_props().unwrap().unwrap();
    assert!(details.props.is_empty() && details.models.is_empty());
    terrain_info_mut(&mut fixture.source, solid).alphas = Some(vec![vec![127.5; 5]; 5]);
    let half = compiled(&fixture.source, &fixture.game);
    assert!((95..=175).contains(&half.stats.detail_props));
}

#[test]
fn unused_unsupported_alpha_groups_do_not_reject_flat_or_terrain_endpoint_selections() {
    let mut fixture = fixture();
    std::fs::write(fixture.directory.path().join("detail.vbsp"),
        "detail { tufts { density 8192 low { alpha 0 sprite { sprite \"0 0 128 128 512\" sprite_shape cross } } high { alpha 1 } } }").unwrap();
    let flat = compiled(&fixture.source, &fixture.game);
    assert_eq!(flat.stats.detail_props, 0);
    assert_eq!(flat.stats.detail_prop_models, 0);
    let solid = add_scatter_terrain(&mut fixture, 2, 0);
    terrain_info_mut(&mut fixture.source, solid).alphas = Some(vec![vec![255.0; 5]; 5]);
    let empty = compiled(&fixture.source, &fixture.game);
    assert_eq!(empty.stats.detail_props, 0);
    std::fs::write(fixture.directory.path().join("detail.vbsp"),
        "detail { tufts { density 8192 low { alpha 0 sprite { sprite \"0 0 128 128 512\" detailOrientation 0.5 } } high { alpha 1 model { model models/props/test.mdl } } } }").unwrap();
    let populated = compiled(&fixture.source, &fixture.game);
    assert_eq!(populated.stats.detail_props, 268);
    let path = fixture.directory.path().join("selected-unsupported.bsp");
    let previous = populated.bsp.unwrap().to_bytes();
    std::fs::write(&path, &previous).unwrap();
    terrain_info_mut(&mut fixture.source, solid).alphas = Some(vec![vec![0.0; 5]; 5]);
    let before = fixture.source.write(&Default::default());
    let rejected = compiler::compile(&fixture.source, &fixture.game);
    assert!(
        rejected.bsp.is_none()
            && rejected
                .report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("integer"))
    );
    assert!(!rejected.write(&path).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), previous);
    assert_eq!(fixture.source.write(&Default::default()), before);
}

#[test]
fn terrain_scatter_keeps_whole_region_sheets_and_rejects_crossing_sheets_atomically() {
    let mut fixture = fixture();
    add_scatter_terrain(&mut fixture, 3, 1);
    blend_dictionary(&fixture, 4096);
    std::fs::create_dir_all(fixture.directory.path().join("materials/tools")).unwrap();
    std::fs::write(
        fixture
            .directory
            .path()
            .join("materials/tools/toolsblack.vmt"),
        "UnlitGeneric { }",
    )
    .unwrap();
    let mut document = Document::new(fixture.source.clone());
    let bounds = compile_region::Bounds {
        mins: DVec3::new(-160.0, -128.0, -8.0),
        maxs: DVec3::new(160.0, 192.0, 160.0),
    };
    document
        .edit("whole scatter terrain region", |tx| {
            compile_region::apply(
                tx,
                Some(compile_region::Definition {
                    enabled: true,
                    bounds,
                }),
            )
        })
        .unwrap();
    let before = document.vmf().write(&Default::default());
    let history = document
        .undo_labels()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let output = compiled(document.vmf(), &fixture.game);
    let bsp = output.bsp.unwrap();
    assert_native_terrain_contacts(&bsp);
    assert!(
        bsp.detail_props()
            .unwrap()
            .unwrap()
            .props
            .iter()
            .all(|prop| bounds.contains(DVec3::from_array(prop.origin.map(f64::from))))
    );
    assert_eq!(document.vmf().write(&Default::default()), before);
    assert_eq!(
        document
            .undo_labels()
            .map(str::to_owned)
            .collect::<Vec<_>>(),
        history
    );
    let destination = fixture.directory.path().join("terrain-region.bsp");
    std::fs::write(&destination, bsp.to_bytes()).unwrap();
    let previous = std::fs::read(&destination).unwrap();
    document
        .edit("crossing scatter terrain region", |tx| {
            compile_region::apply(
                tx,
                Some(compile_region::Definition {
                    enabled: true,
                    bounds: compile_region::Bounds {
                        maxs: DVec3::new(64.0, 192.0, 160.0),
                        ..bounds
                    },
                }),
            )
        })
        .unwrap();
    let source = document.vmf().write(&Default::default());
    let history = document
        .undo_labels()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    let rejected = compiler::compile(document.vmf(), &fixture.game);
    assert!(
        rejected.bsp.is_none()
            && rejected
                .report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic
                    .message
                    .contains("Terrain crosses a compile-region boundary"))
    );
    assert!(!rejected.write(&destination).unwrap());
    assert_eq!(std::fs::read(&destination).unwrap(), previous);
    assert_eq!(document.vmf().write(&Default::default()), source);
    assert_eq!(
        document
            .undo_labels()
            .map(str::to_owned)
            .collect::<Vec<_>>(),
        history
    );
}

#[test]
fn terrain_scatter_uses_final_mixed_power_lod_surface_without_changing_authored_grids() {
    let mut fixture = fixture();
    fixture.source.world.solids[0]
        .sides
        .iter_mut()
        .find(|side| side.id == fixture.floor_side)
        .unwrap()
        .material = "dev/dev_measuregeneric01".into();
    let mut document = Document::new(fixture.source.clone());
    for (index, power) in [2, 4].into_iter().enumerate() {
        document
            .edit("mixed power detail terrain", |tx| {
                let ObjectId::Solid(solid) = edit::create_block(
                    tx,
                    DVec3::new(-128.0 + index as f64 * 128.0, 32.0, 0.0),
                    DVec3::new(index as f64 * 128.0, 160.0, 24.0),
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
                map_core::displacement::edit::create(
                    tx,
                    &[FaceId { solid, side }],
                    power,
                    index as u32 * 2,
                )?;
                Ok(())
            })
            .unwrap();
    }
    fixture.source = document.vmf().clone();
    for solid in &mut fixture.source.world.solids {
        let geometry = BrushGeometry::from_solid(solid).unwrap();
        for face in &geometry.faces {
            let Some(info) = solid.sides[face.side].displacement.as_mut() else {
                continue;
            };
            let mesh = DisplacementMesh::build(face, info).unwrap();
            info.offsets = Some(
                mesh.positions
                    .chunks(mesh.size)
                    .map(|row| {
                        row.iter()
                            .map(|point| {
                                DVec3::Z * ((point.y - 96.0).powi(2) + point.x.powi(2)) / 512.0
                            })
                            .collect()
                    })
                    .collect(),
            );
            info.alphas = Some(vec![vec![127.5; mesh.size]; mesh.size]);
        }
    }
    blend_dictionary(&fixture, 8192);
    let before = fixture.source.write(&Default::default());
    let output = compiled(&fixture.source, &fixture.game);
    assert!(output.stats.displacement_lod_snapped_vertices > 0);
    assert!(output.stats.displacement_lod_disabled_vertices > 0);
    assert_eq!(output.stats.detail_props, 268);
    let bsp = output.bsp.unwrap();
    let data = bsp.displacements().unwrap().unwrap();
    assert!(data.infos.iter().any(
        |info| info.allowed_triangle_indices().unwrap().len() < info.triangle_count().unwrap()
    ));
    assert_native_terrain_contacts(&bsp);
    assert_eq!(fixture.source.write(&Default::default()), before);
    assert_eq!(
        compiled(&Vmf::parse(&before).unwrap(), &fixture.game)
            .bsp
            .unwrap()
            .to_bytes(),
        bsp.to_bytes()
    );
}

#[test]
fn terrain_scatter_rejects_skewed_bases_and_shares_sample_budget_with_flat_surfaces() {
    let mut fixture = fixture();
    let solid = add_scatter_terrain(&mut fixture, 2, 0);
    dictionary(&fixture, 225000, "model models/props/test.mdl amount 0");
    assert_eq!(
        compiled(&fixture.source, &fixture.game).stats.detail_props,
        0
    );
    fixture.source.world.solids[0]
        .sides
        .iter_mut()
        .find(|side| side.id == fixture.floor_side)
        .unwrap()
        .material = MATERIAL.into();
    fixture
        .source
        .world
        .solids
        .iter_mut()
        .find(|candidate| candidate.id == solid)
        .unwrap()
        .hidden = true;
    assert_eq!(
        compiled(&fixture.source, &fixture.game).stats.detail_props,
        0
    );
    fixture
        .source
        .world
        .solids
        .iter_mut()
        .find(|candidate| candidate.id == solid)
        .unwrap()
        .hidden = false;
    let path = fixture.directory.path().join("previous-terrain.bsp");
    std::fs::write(&path, b"previous BSP").unwrap();
    let before = fixture.source.write(&Default::default());
    let output = compiler::compile(&fixture.source, &fixture.game);
    assert!(
        output.bsp.is_none()
            && output
                .report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("65,536-sample build budget")),
        "{:?}",
        output.report
    );
    assert!(!output.write(&path).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), b"previous BSP");
    assert_eq!(fixture.source.write(&Default::default()), before);
    fixture.source.world.solids[0]
        .sides
        .iter_mut()
        .find(|side| side.id == fixture.floor_side)
        .unwrap()
        .material = "dev/dev_measuregeneric01".into();
    dictionary(&fixture, 8192, "model models/props/test.mdl");
    let terrain = fixture
        .source
        .world
        .solids
        .iter_mut()
        .find(|candidate| candidate.id == solid)
        .unwrap();
    for side in &mut terrain.sides {
        for point in &mut side.plane.0 {
            point.x *= 1.0 - (point.y - 32.0) / 128.0 * 0.25;
        }
        if let Some(info) = &mut side.displacement {
            info.start_position.x *= 1.0 - (info.start_position.y - 32.0) / 128.0 * 0.25;
        }
    }
    let before = fixture.source.write(&Default::default());
    let output = compiler::compile(&fixture.source, &fixture.game);
    assert!(
        output.bsp.is_none()
            && output.report.diagnostics.iter().any(|diagnostic| diagnostic
                .message
                .contains("requires a parallelogram base quad")),
        "{:?}",
        output.report
    );
    assert!(!output.write(&path).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), b"previous BSP");
    assert_eq!(fixture.source.write(&Default::default()), before);
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
