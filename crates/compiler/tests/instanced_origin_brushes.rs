mod common;

use std::path::Path;

use compiler::instances::{self, BrushModelPreparation, PreparedInstances, SourceContext};
use compiler::{Control, LightingMode, Options, SkyQuality};
use glam::DVec3;
use map_core::{Document, ObjectId, edit};
use source_formats::bsp::{self, Bsp, LightingKind, Model, TexInfo};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::keyvalues::Node;
use source_formats::vmf::{Entity, Solid, UvAxis, Vmf, format_vec3};

type Bounds = ([f64; 3], [f64; 3]);
const BODIES: [Bounds; 2] = [
    ([16.0, 8.0, 32.0], [48.0, 24.0, 48.0]),
    ([-32.0, 32.0, 64.0], [-16.0, 48.0, 96.0]),
];
const PIVOT_VOLUME: Bounds = ([0.0, 0.0, 0.0], [8.0, 4.0, 2.0]);
const NATIVE_POSITION_EPSILON: f64 = 1e-4;
const PROJECTION_EPSILON: f64 = 1e-5;

fn fixture() -> (tempfile::TempDir, GameFs) {
    let directory = tempfile::tempdir().unwrap();
    for folder in [
        "maps",
        "published",
        "materials/dev",
        "materials/test",
        "materials/tools",
    ] {
        std::fs::create_dir_all(directory.path().join(folder)).unwrap();
    }
    for (name, text) in [
        (
            "dev/dev_measuregeneric01",
            "LightmappedGeneric { %notooltexture 1 $reflectivity \"[.5 .5 .5]\" }",
        ),
        (
            "test/body",
            "LightmappedGeneric { %notooltexture 1 $reflectivity \"[.5 .5 .5]\" }",
        ),
        (
            "tools/toolsorigin",
            "LightmappedGeneric { %notooltexture 1 $reflectivity \"[.5 .5 .5]\" }",
        ),
        (
            "test/pivot",
            "Patch { include materials/test/pivot_only_base.vmt }",
        ),
        ("test/pivot_only_base", "UnlitGeneric { %compileorigin 1 }"),
        ("test/liquid", "LightmappedGeneric { %compilewater 1 }"),
        ("tools/toolsblack", "UnlitGeneric {}"),
    ] {
        std::fs::write(directory.path().join(format!("materials/{name}.vmt")), text).unwrap();
    }
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().to_owned()),
    });
    (directory, game)
}

fn write(directory: &Path, name: &str, vmf: &Vmf) -> Vec<u8> {
    let bytes = vmf.write(&Default::default()).into_bytes();
    std::fs::write(directory.join("maps").join(name), &bytes).unwrap();
    bytes
}

fn saved(directory: &Path, vmf: &Vmf) -> SourceContext {
    write(directory, "parent.vmf", vmf);
    SourceContext::capture(&directory.join("maps/parent.vmf"), &Control::default()).unwrap()
}

fn instance(vmf: &mut Vmf, file: &str, origin: &str, angles: &str) -> u32 {
    let id = vmf.object_ids().chain(vmf.side_ids()).max().unwrap() + 1;
    let mut entity = Entity::new(id, "func_instance");
    for (key, value) in [("file", file), ("origin", origin), ("angles", angles)] {
        entity.properties.set(key, value);
    }
    vmf.entities.push(entity);
    id
}

fn axes() -> [UvAxis; 2] {
    [
        UvAxis {
            axis: DVec3::new(1.0, 2.0, 2.0) / 3.0,
            offset: 7.0,
            scale: 1.0,
        },
        UvAxis {
            axis: DVec3::new(-2.0, 2.0, -1.0) / 3.0,
            offset: -11.0,
            scale: 2.0,
        },
    ]
}

fn model(document: &mut Document, parts: &[(Bounds, &str)], origin: Option<DVec3>) -> u32 {
    document
        .edit("literal compound", |tx| {
            let mut solids = Vec::new();
            for &((mins, maxs), material) in parts {
                let ObjectId::Solid(id) = edit::create_block(
                    tx,
                    DVec3::from_array(mins),
                    DVec3::from_array(maxs),
                    material,
                )?
                else {
                    unreachable!()
                };
                for side in &mut tx.solid_mut(id)?.sides {
                    [side.u_axis, side.v_axis] = axes();
                    side.lightmap_scale = 16;
                }
                solids.push(id);
            }
            let mut properties = vec![
                ("angles".into(), "0 0 0".into()),
                ("spawnflags".into(), "2".into()),
                ("Solidity".into(), "0".into()),
                ("StartDisabled".into(), "0".into()),
                ("InputFilter".into(), "0".into()),
            ];
            if let Some(origin) = origin {
                properties.push(("origin".into(), format_vec3(origin)));
            }
            let ObjectId::Entity(id) = edit::tie_to_entity(tx, &solids, "func_brush", &properties)?
            else {
                unreachable!()
            };
            Ok(id)
        })
        .unwrap()
}

fn child(origin: Option<DVec3>) -> Vmf {
    let mut document = Document::empty();
    model(
        &mut document,
        &[
            (BODIES[0], "test/body"),
            (PIVOT_VOLUME, "test/pivot"),
            (BODIES[1], "tools/toolsorigin"),
        ],
        origin,
    );
    document.into_vmf()
}

fn room() -> Vmf {
    let mut vmf = common::room();
    for side in vmf
        .world
        .solids
        .iter_mut()
        .flat_map(|solid| &mut solid.sides)
    {
        side.lightmap_scale = 64;
    }
    vmf
}

fn resolve<'a>(vmf: &'a Vmf, context: &SourceContext, game: &GameFs) -> PreparedInstances<'a> {
    let mut report = compiler::Report::default();
    let prepared =
        instances::resolve_for_compile(vmf, Some(context), game, &Control::default(), &mut report)
            .unwrap();
    assert!(!report.has_errors(), "{report:?}");
    assert_eq!(
        prepared.brush_model_preparation(),
        BrushModelPreparation::Resolved
    );
    assert!(prepared.compilation_vmf().is_ok());
    prepared
}

fn compile(
    vmf: &Vmf,
    game: &GameFs,
    context: &SourceContext,
    options: Options,
) -> compiler::Output {
    compiler::compile_with_source_controlled(
        vmf,
        game,
        options,
        None,
        Some(context),
        &Control::default(),
    )
}

fn successful(output: &compiler::Output) -> &Bsp {
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.leak.is_none());
    output.bsp.as_ref().unwrap()
}

fn same_bsp(actual: &Bsp, expected: &Bsp) {
    for (index, (actual, expected)) in actual.lumps.iter().zip(&expected.lumps).enumerate() {
        assert!(actual == expected, "native lump {index} differs");
    }
    assert!(
        actual.to_bytes() == expected.to_bytes(),
        "complete BSP differs"
    );
}

fn align_ids(literal: &mut Entity, generated: &Entity) {
    literal.id = generated.id;
    assert_eq!(literal.solids.len(), generated.solids.len());
    for (literal, generated) in literal.solids.iter_mut().zip(&generated.solids) {
        literal.id = generated.id;
        assert_eq!(literal.sides.len(), generated.sides.len());
        for (literal, generated) in literal.sides.iter_mut().zip(&generated.sides) {
            literal.id = generated.id;
        }
    }
}

#[test]
fn resolved_origin_compounds_match_complete_literal_geometry_uvs_and_dual_lighting() {
    let (directory, game) = fixture();
    let mut middle = Document::empty().into_vmf();
    instance(&mut middle, "leaf.vmf", "16 32 0", "0 90 0");
    let middle_bytes = write(directory.path(), "middle.vmf", &middle);
    let mut base = room();
    let mut light = Entity::new(
        base.object_ids().chain(base.side_ids()).max().unwrap() + 1,
        "light",
    );
    for (key, value) in [
        ("origin", "0 0 240"),
        ("_light", "255 0 0 200"),
        ("_lightHDR", "0 0 255 400"),
        ("_constant_attn", "1"),
        ("_linear_attn", "0"),
        ("_quadratic_attn", "0"),
    ] {
        light.properties.set(key, value);
    }
    base.entities.push(light);
    let mut parent = base.clone();
    let owner = instance(&mut parent, "middle.vmf", "80 64 96", "90 0 0");
    let before = parent.clone();
    let mut expected = None;
    for explicit in [Some(DVec3::new(-80.0, -60.0, 40.0)), None] {
        let mut leaf = child(explicit);
        leaf.entities[0]
            .properties
            .set("vrad_brush_cast_shadows", "1");
        let leaf_bytes = write(directory.path(), "leaf.vmf", &leaf);
        let context = saved(directory.path(), &parent);
        let reference = instances::resolve(
            &parent,
            Some(&context),
            &Control::default(),
            &mut compiler::Report::default(),
        )
        .unwrap();
        assert_eq!(
            reference.brush_model_preparation(),
            BrushModelPreparation::Deferred
        );
        assert!(reference.compilation_vmf().is_err());
        assert_eq!(reference.stats().unwrap().consumed_origin_brushes, 0);
        let prepared = resolve(&parent, &context, &game);
        let stats = prepared.stats().unwrap();
        assert_eq!(
            (
                stats.instances,
                stats.files,
                stats.max_expansion_depth,
                stats.brush_models
            ),
            (2, 2, 2, 1)
        );
        assert_eq!(stats.consumed_origin_brushes, 1);
        assert_eq!(prepared.generated_brushes().count(), 0);
        let generated = prepared
            .vmf()
            .entities
            .iter()
            .find(|entity| prepared.root_entity(entity.id) == Some(owner))
            .unwrap();
        assert_eq!(generated.origin(), Some(DVec3::new(81.0, 100.0, 82.0)));
        assert_eq!(generated.solids.len(), 2);
        assert_eq!(generated.solids[1].id - generated.solids[0].id, 2);
        assert_eq!(
            generated.solids[1].sides[0].id - generated.solids[0].sides[0].id,
            12
        );
        for (solid, original) in generated
            .solids
            .iter()
            .zip([&leaf.entities[0].solids[0], &leaf.entities[0].solids[2]])
        {
            for (side, original_side) in solid.sides.iter().zip(&original.sides) {
                assert_eq!(
                    prepared.face_origin(side.id),
                    Some((owner, "leaf.vmf", original.id, original_side.id))
                );
                for (point, old) in side.plane.0.iter().zip(original_side.plane.0) {
                    assert!(
                        point.abs_diff_eq(
                            DVec3::new(old.z + 80.0, old.x + 96.0, old.y + 80.0),
                            1e-9
                        )
                    );
                }
            }
        }
        let mut literal = Document::new(base.clone());
        let id = model(
            &mut literal,
            &[
                (([112.0, 112.0, 88.0], [128.0, 144.0, 104.0]), "test/body"),
                (
                    ([144.0, 64.0, 112.0], [176.0, 80.0, 128.0]),
                    "tools/toolsorigin",
                ),
            ],
            Some(DVec3::new(81.0, 100.0, 82.0)),
        );
        literal
            .edit("literal projection and plane order", |tx| {
                let entity = tx.entity_mut(id)?;
                entity.properties.set("vrad_brush_cast_shadows", "1");
                entity.properties.remove("origin");
                entity.properties.set("origin", "81 100 82");
                for solid in &mut entity.solids {
                    let sides = solid.sides.clone();
                    solid.sides = [3, 2, 5, 4, 0, 1]
                        .map(|index| sides[index].clone())
                        .to_vec();
                    for side in &mut solid.sides {
                        side.u_axis = UvAxis {
                            axis: DVec3::new(2.0, 1.0, 2.0) / 3.0,
                            offset: -395.0 / 3.0,
                            scale: 1.0,
                        };
                        side.v_axis = UvAxis {
                            axis: DVec3::new(-1.0, -2.0, 2.0) / 3.0,
                            offset: 23.0 / 3.0,
                            scale: 2.0,
                        };
                    }
                }
                align_ids(entity, generated);
                Ok(())
            })
            .unwrap();
        let options = Options {
            lighting: LightingMode::DirectLdrHdr,
            bounces: 0,
            sky_quality: SkyQuality::Fast,
            ..Default::default()
        };
        if expected.is_none() {
            let output = compiler::compile_with_options(&literal.into_vmf(), &game, options);
            expected = Some(successful(&output).clone());
        }
        let output = compile(&parent, &game, &context, options);
        let bsp = successful(&output);
        same_bsp(bsp, expected.as_ref().unwrap());
        assert_eq!(output.stats.brushes, 8);
        assert_eq!(output.stats.brush_shadow_models, 1);
        let models = bsp.records::<Model>().unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[1].origin, [0.0; 3]);
        assert_eq!(models[1].mins, [31.0, -36.0, 6.0]);
        assert_eq!(models[1].maxs, [95.0, 44.0, 46.0]);
        let infos = bsp.records::<TexInfo>().unwrap();
        for (kind, active) in [(LightingKind::Ldr, 0), (LightingKind::Hdr, 2)] {
            let faces = bsp.lighting_faces(kind).unwrap();
            let lighting = bsp.lighting(kind).unwrap().unwrap();
            let mut positive = false;
            for face in &faces[models[1].first_face as usize
                ..(models[1].first_face + models[1].face_count) as usize]
            {
                let data = lighting
                    .face(face, &infos[face.texinfo as usize])
                    .unwrap()
                    .unwrap();
                for sample in data.samples(0, 0).unwrap() {
                    positive |= sample.rgb[active] > 0;
                    assert!(
                        (0..3)
                            .filter(|&channel| channel != active)
                            .all(|channel| sample.rgb[channel] == 0)
                    );
                }
            }
            assert!(positive, "missing {kind:?} model lighting");
        }
        assert!(
            bsp.records::<bsp::Brush>()
                .unwrap()
                .iter()
                .all(|brush| brush.contents & bsp::contents::ORIGIN == 0)
        );
        assert_eq!(parent, before);
        assert_eq!(
            std::fs::read(directory.path().join("maps/leaf.vmf")).unwrap(),
            leaf_bytes
        );
        assert_eq!(
            std::fs::read(directory.path().join("maps/middle.vmf")).unwrap(),
            middle_bytes
        );
    }
}

fn prism(solid: &mut Solid) {
    solid.sides[3].plane.0 = [
        DVec3::new(0.0, 4.0, 0.0),
        DVec3::new(8.0, 0.0, 0.0),
        DVec3::new(0.0, 4.0, 2.0),
    ];
    solid.sides.remove(4);
}

fn region(vmf: &mut Vmf, min_y: f64) {
    vmf.cordon = Some(Node::block(
        "cordon",
        vec![
            Node::string("active", "1"),
            Node::string("mins", format!("-128 {min_y} 4")),
            Node::string("maxs", "128 128 252"),
        ],
    ));
}

fn corners((mins, maxs): Bounds) -> [DVec3; 8] {
    std::array::from_fn(|index| {
        DVec3::new(
            if index & 1 == 0 { mins[0] } else { maxs[0] },
            if index & 2 == 0 { mins[1] } else { maxs[1] },
            if index & 4 == 0 { mins[2] } else { maxs[2] },
        )
    })
}

fn projected(row: [f32; 4], point: DVec3) -> f64 {
    point.dot(DVec3::new(
        f64::from(row[0]),
        f64::from(row[1]),
        f64::from(row[2]),
    )) + f64::from(row[3])
}

#[test]
fn asymmetric_volume_resolves_before_rotation_and_region_uses_the_resolved_pivot() {
    let (directory, game) = fixture();
    let mut leaf = child(Some(DVec3::new(0.0, -64.0, 0.0)));
    leaf.entities[0].solids.pop();
    prism(&mut leaf.entities[0].solids[1]);
    let source = write(directory.path(), "leaf.vmf", &leaf);
    let mut parent = room();
    let owner = instance(&mut parent, "leaf.vmf", "64 -32 64", "0 45 0");
    let context = saved(directory.path(), &parent);
    let prepared = resolve(&parent, &context, &game);
    let generated = prepared
        .vmf()
        .entities
        .iter()
        .find(|entity| prepared.root_entity(entity.id) == Some(owner))
        .unwrap();
    let root_two = 2.0_f64.sqrt();
    let pivot = DVec3::new(64.0 + root_two, -32.0 + 3.0 * root_two, 65.0);
    let late_center = DVec3::new(64.0 + root_two, -32.0 + 2.0 * root_two, 65.0);
    assert!(
        generated
            .origin()
            .unwrap()
            .abs_diff_eq(pivot, NATIVE_POSITION_EPSILON)
    );
    assert!(generated.origin().unwrap().distance(late_center) > 1.4);
    assert_eq!(generated.solids.len(), 1);
    let source_corners = corners(BODIES[0]);
    let local_corners = source_corners.map(|point| {
        DVec3::new(
            (point.x - point.y - 2.0) / root_two,
            (point.x + point.y - 6.0) / root_two,
            point.z - 1.0,
        )
    });
    let mut report = compiler::Report::default();
    let map =
        compiler::map::Map::load(prepared.compilation_vmf().unwrap(), &game, &mut report).unwrap();
    assert!(!report.has_errors(), "{report:?}");
    let brush = map
        .brushes
        .iter()
        .find(|brush| brush.id == generated.solids[0].id)
        .unwrap();
    let mut seen = [false; 8];
    for side in brush.sides.iter().filter(|side| !side.bevel) {
        let info = &map.texinfos.infos[side.texinfo];
        for &point in &side.winding.as_ref().unwrap().0 {
            let index = local_corners
                .iter()
                .position(|expected| point.abs_diff_eq(*expected, NATIVE_POSITION_EPSILON))
                .expect("unexpected model-local corner");
            seen[index] = true;
            for (row, axis) in axes().iter().enumerate() {
                let texel = source_corners[index].dot(axis.axis) / axis.scale + axis.offset;
                let luxel =
                    (source_corners[index].dot(axis.axis) + axis.offset * axis.scale) / 16.0;
                assert!(
                    (projected(info.texture_vecs[row], point) - texel).abs() < PROJECTION_EPSILON
                );
                assert!(
                    (projected(info.lightmap_vecs[row], point) - luxel).abs() < PROJECTION_EPSILON
                );
            }
        }
    }
    assert!(seen.into_iter().all(|seen| seen));
    let expected_mins = DVec3::new(-10.0 / root_two, 18.0 / root_two, 31.0);
    let expected_maxs = DVec3::new(38.0 / root_two, 66.0 / root_two, 47.0);
    for threshold in [None, Some(-28.5), Some(-27.0)] {
        let mut parent = parent.clone();
        if let Some(threshold) = threshold {
            region(&mut parent, threshold);
        }
        let context = saved(directory.path(), &parent);
        let output = compile(&parent, &game, &context, Options::default());
        let models = successful(&output).records::<Model>().unwrap();
        let retained = threshold != Some(-27.0);
        assert_eq!(models.len(), if retained { 2 } else { 1 });
        assert_eq!(output.stats.instances.as_ref().unwrap().brush_models, 1);
        assert_eq!(
            output
                .stats
                .instances
                .as_ref()
                .unwrap()
                .consumed_origin_brushes,
            1
        );
        if retained {
            assert_eq!(models[1].origin, [0.0; 3]);
            assert!(
                DVec3::from_array(models[1].mins.map(f64::from))
                    .abs_diff_eq(expected_mins, NATIVE_POSITION_EPSILON)
            );
            assert!(
                DVec3::from_array(models[1].maxs.map(f64::from))
                    .abs_diff_eq(expected_maxs, NATIVE_POSITION_EPSILON)
            );
        }
        if threshold.is_some() {
            let selection = output.stats.compile_region.as_ref().unwrap();
            let id = format!("entity:{}", generated.id);
            assert_eq!(selection.included.contains(&id), retained);
            assert_eq!(selection.omitted.contains(&id), !retained);
        }
    }
    assert_eq!(
        std::fs::read(directory.path().join("maps/leaf.vmf")).unwrap(),
        source
    );
}

fn move_solid(solid: &mut Solid, offset: DVec3) {
    for point in solid.sides.iter_mut().flat_map(|side| &mut side.plane.0) {
        *point += offset;
    }
}

fn previous_output(directory: &Path) -> std::path::PathBuf {
    let destination = directory.join("published/previous.bsp");
    std::fs::write(&destination, b"previous BSP").unwrap();
    std::fs::write(destination.with_extension("lin"), b"previous LIN").unwrap();
    destination
}

fn preserved(destination: &Path) {
    assert_eq!(std::fs::read(destination).unwrap(), b"previous BSP");
    assert_eq!(
        std::fs::read(destination.with_extension("lin")).unwrap(),
        b"previous LIN"
    );
}

#[test]
fn consumed_sources_cannot_escape_admission_regions_or_owned_publication_guards() {
    let (directory, game) = fixture();
    let destination = previous_output(directory.path());
    for case in [
        "invalid explicit",
        "multiple",
        "hidden only",
        "origin only",
        "missing material",
        "body material",
        "source bounds",
        "intermediate bounds",
        "local bounds",
    ] {
        let mut leaf = child(Some(DVec3::ZERO));
        let mut parent = room();
        region(&mut parent, -128.0);
        let mut file = "leaf.vmf";
        let mut translation = "1000 0 0";
        let original_volume = leaf.entities[0].solids[1].id;
        let reason = match case {
            "invalid explicit" => {
                leaf.entities[0].properties.set("origin", "NaN 0 0");
                "origin"
            }
            "multiple" => {
                let mut duplicate = leaf.entities[0].solids[1].clone();
                duplicate.id = leaf.object_ids().chain(leaf.side_ids()).max().unwrap() + 1;
                for (index, side) in duplicate.sides.iter_mut().enumerate() {
                    side.id = duplicate.id + index as u32 + 1;
                }
                leaf.entities[0].solids.push(duplicate);
                "one visible origin brush"
            }
            "hidden only" => {
                leaf.entities[0].properties.remove("origin");
                leaf.entities[0].solids[1].hidden = true;
                "origin"
            }
            "origin only" => {
                leaf.entities[0]
                    .solids
                    .retain(|solid| solid.id == original_volume);
                "compiled brush geometry"
            }
            "missing material" => {
                for side in &mut leaf.entities[0].solids[1].sides {
                    side.material = "test/missing_pivot".into();
                }
                "material"
            }
            "body material" => {
                for side in &mut leaf.entities[0].solids[0].sides {
                    side.material = "test/liquid".into();
                }
                "material"
            }
            "source bounds" => {
                move_solid(
                    &mut leaf.entities[0].solids[1],
                    DVec3::new(17000.0, 0.0, 0.0),
                );
                "bounds"
            }
            "intermediate bounds" => {
                for point in leaf.entities[0].solids[1]
                    .sides
                    .iter_mut()
                    .flat_map(|side| &mut side.plane.0)
                {
                    point.x *= 20.0;
                }
                let mut middle = Document::empty().into_vmf();
                instance(&mut middle, "leaf.vmf", "16300 0 0", "0 0 0");
                write(directory.path(), "middle.vmf", &middle);
                file = "middle.vmf";
                translation = "-15300 0 0";
                "bounds"
            }
            "local bounds" => {
                move_solid(
                    &mut leaf.entities[0].solids[1],
                    DVec3::new(-16364.0, 0.0, 0.0),
                );
                "local coordinates"
            }
            _ => unreachable!(),
        };
        let owner = instance(&mut parent, file, translation, "0 0 0");
        let leaf_bytes = write(directory.path(), "leaf.vmf", &leaf);
        let context = saved(directory.path(), &parent);
        let output = compile(&parent, &game, &context, Options::default());
        assert!(
            output.report.has_errors() && output.bsp.is_none() && output.leak.is_none(),
            "{case}: {:?}",
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
                        && diagnostic.message.to_ascii_lowercase().contains(reason)
                ),
            "{case}: {:?}",
            output.report
        );
        if let Some(identity) = match case {
            "source bounds" => Some(format!("Child brush {original_volume}")),
            "intermediate bounds" => Some(format!("Consumed origin solid {original_volume}")),
            "missing material" => Some(format!("Origin brush solid {original_volume}")),
            _ => None,
        } {
            assert!(
                output
                    .report
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.message.contains(&identity)),
                "{case}: {:?}",
                output.report
            );
        }
        assert!(!output.write(&destination).unwrap());
        preserved(&destination);
        assert_eq!(
            std::fs::read(directory.path().join("maps/leaf.vmf")).unwrap(),
            leaf_bytes
        );
    }

    for (change_child, omitted, before_prepare) in [
        (true, false, false),
        (false, false, false),
        (false, true, false),
        (false, false, true),
    ] {
        let (directory, game) = fixture();
        let destination = previous_output(directory.path());
        let leaf = child(None);
        write(directory.path(), "leaf.vmf", &leaf);
        let mut parent = room();
        if omitted {
            region(&mut parent, -128.0);
        }
        instance(
            &mut parent,
            "leaf.vmf",
            if omitted { "1000 0 0" } else { "0 0 0" },
            "0 0 0",
        );
        let context = saved(directory.path(), &parent);
        let output = compile(&parent, &game, &context, Options::default());
        assert_eq!(
            successful(&output).records::<Model>().unwrap().len(),
            if omitted { 1 } else { 2 }
        );
        assert_eq!(
            output
                .stats
                .instances
                .as_ref()
                .unwrap()
                .consumed_origin_brushes,
            1
        );
        if before_prepare {
            std::fs::remove_file(directory.path().join("materials/test/pivot_only_base.vmt"))
                .unwrap();
            assert!(
                output
                    .prepare_write_controlled(&destination, &Control::default())
                    .is_err()
            );
            preserved(&destination);
            continue;
        }
        let prepared = output
            .prepare_write_controlled(&destination, &Control::default())
            .unwrap()
            .unwrap();
        drop(output);
        if change_child {
            let mut changed = leaf;
            move_solid(&mut changed.entities[0].solids[1], DVec3::X);
            write(directory.path(), "leaf.vmf", &changed);
        } else {
            std::fs::write(
                directory.path().join("materials/test/pivot_only_base.vmt"),
                "UnlitGeneric { %compileorigin 0 }",
            )
            .unwrap();
        }
        assert!(prepared.commit(&Control::default()).is_err());
        preserved(&destination);
    }
}
