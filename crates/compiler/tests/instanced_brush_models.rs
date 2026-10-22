mod common;

use std::path::Path;

use compiler::instances::{self, PreparedInstances, SourceContext};
use compiler::{Control, LightingMode, Options, SkyQuality};
use glam::DVec3;
use map_core::{Document, ObjectId, Plane, edit};
use source_formats::bsp::{self, Bsp, LightingKind, Model, RgbExp32, TexInfo};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::keyvalues::Node;
use source_formats::vmf::{Entity, UvAxis, Vmf, format_vec3};

type Bounds = ([f64; 3], [f64; 3]);
const SOURCE: [Bounds; 2] = [
    ([-64.0, -16.0, -48.0], [64.0, 0.0, 48.0]),
    ([-80.0, 64.0, -64.0], [64.0, 80.0, 56.0]),
];
const PIVOT: DVec3 = DVec3::new(16.0, 8.0, 4.0);
const NESTED_SIDE_ORDER: [usize; 6] = [3, 2, 5, 4, 0, 1];

fn fixture() -> (tempfile::TempDir, GameFs) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/validation/instanced-brush-models");
    std::fs::create_dir_all(&root).unwrap();
    let directory = tempfile::tempdir_in(root).unwrap();
    for folder in [
        "maps",
        "published",
        "game/materials/dev",
        "game/materials/test",
        "game/materials/tools",
    ] {
        std::fs::create_dir_all(directory.path().join(folder)).unwrap();
    }
    for (name, text) in [
        (
            "dev/dev_measuregeneric01",
            "LightmappedGeneric { $basetexture test/white }",
        ),
        (
            "test/model",
            "Patch { include materials/test/model_base.vmt }",
        ),
        (
            "test/model_base",
            "LightmappedGeneric { $basetexture test/white }",
        ),
        ("tools/toolsblack", "UnlitGeneric {}"),
    ] {
        std::fs::write(
            directory.path().join(format!("game/materials/{name}.vmt")),
            text,
        )
        .unwrap();
    }
    let mut texture = vec![0_u8; 84];
    texture[..4].copy_from_slice(b"VTF\0");
    for (offset, value) in [(4, 7_u32), (8, 2), (12, 80), (57, u32::MAX)] {
        texture[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    for offset in [16, 18, 24, 63] {
        texture[offset..offset + 2].copy_from_slice(&1_u16.to_le_bytes());
    }
    for offset in [32, 36, 40] {
        texture[offset..offset + 4].copy_from_slice(&0.25_f32.to_le_bytes());
    }
    texture[56] = 1;
    texture[80..].copy_from_slice(&[255; 4]);
    std::fs::write(
        directory.path().join("game/materials/test/white.vtf"),
        texture,
    )
    .unwrap();
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().join("game")),
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

fn entity(vmf: &mut Vmf, id: u32) -> &mut Entity {
    vmf.entities
        .iter_mut()
        .find(|entity| entity.id == id)
        .unwrap()
}

fn source_axes() -> [UvAxis; 2] {
    [
        UvAxis {
            axis: DVec3::new(1.0, 2.0, 2.0) / 3.0,
            offset: 7.0,
            scale: 0.5,
        },
        UvAxis {
            axis: DVec3::new(-2.0, 2.0, -1.0) / 3.0,
            offset: -11.0,
            scale: 2.0,
        },
    ]
}

fn texture(entity: &mut Entity, axes: [UvAxis; 2]) {
    for side in entity.solids.iter_mut().flat_map(|solid| &mut solid.sides) {
        side.u_axis = axes[0];
        side.v_axis = axes[1];
        side.lightmap_scale = 16;
    }
}

fn model(document: &mut Document, bounds: &[Bounds], pivot: DVec3) -> u32 {
    let id = document
        .edit("compound brush model", |tx| {
            let mut solids = Vec::new();
            for &(mins, maxs) in bounds {
                let ObjectId::Solid(id) = edit::create_block(
                    tx,
                    DVec3::from_array(mins),
                    DVec3::from_array(maxs),
                    "test/model",
                )?
                else {
                    unreachable!()
                };
                solids.push(id);
            }
            let ObjectId::Entity(id) = edit::tie_to_entity(
                tx,
                &solids,
                "func_brush",
                &[
                    ("origin".into(), format_vec3(pivot)),
                    ("angles".into(), "0 0 0".into()),
                    ("spawnflags".into(), "2".into()),
                    ("Solidity".into(), "0".into()),
                    ("StartDisabled".into(), "0".into()),
                    ("InputFilter".into(), "0".into()),
                ],
            )?
            else {
                unreachable!()
            };
            Ok(id)
        })
        .unwrap();
    document
        .edit("attached projections", |tx| {
            texture(tx.entity_mut(id)?, source_axes());
            Ok(())
        })
        .unwrap();
    id
}

fn child(bounds: &[Bounds], pivot: DVec3) -> Vmf {
    let mut document = Document::empty();
    model(&mut document, bounds, pivot);
    document.into_vmf()
}

fn room() -> Vmf {
    let mut source = common::room();
    for solid in &mut source.world.solids {
        for side in &mut solid.sides {
            for point in &mut side.plane.0 {
                *point += DVec3::splat(1024.0);
            }
            side.lightmap_scale = 64;
        }
    }
    for entity in &mut source.entities {
        let origin = entity.origin().unwrap() + DVec3::splat(1024.0);
        entity.properties.set("origin", format_vec3(origin));
    }
    source
}

fn lit_room() -> Vmf {
    let mut document = Document::new(room());
    model(
        &mut document,
        &[([1040.0, 1008.0, 1056.0], [1168.0, 1136.0, 1072.0])],
        DVec3::new(1080.0, 1032.0, 1040.0),
    );
    document
        .edit("independent direct channels", |tx| {
            for (class, style, ldr, hdr) in [
                ("light", "0", "255 0 0 255", "0 0 255 255"),
                ("light_spot", "1", "0 255 0 160", "255 0 0 160"),
            ] {
                let mut properties = vec![
                    ("style".into(), style.into()),
                    ("_light".into(), ldr.into()),
                    ("_lightHDR".into(), hdr.into()),
                    ("_constant_attn".into(), "1".into()),
                    ("_linear_attn".into(), "0".into()),
                    ("_quadratic_attn".into(), "0".into()),
                ];
                if class == "light_spot" {
                    properties.extend([
                        ("pitch".into(), "-90".into()),
                        ("_inner_cone".into(), "45".into()),
                        ("_cone".into(), "60".into()),
                    ]);
                }
                edit::create_entity(
                    tx,
                    class,
                    Some(DVec3::new(1104.0, 1072.0, 1264.0)),
                    &properties,
                    Vec::new(),
                )?;
            }
            Ok(())
        })
        .unwrap();
    document.into_vmf()
}

fn resolve<'a>(vmf: &'a Vmf, context: &SourceContext, game: &GameFs) -> PreparedInstances<'a> {
    let mut report = compiler::Report::default();
    let prepared =
        instances::resolve_for_compile(vmf, Some(context), game, &Control::default(), &mut report)
            .unwrap();
    assert!(!report.has_errors(), "{report:?}");
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

fn literal_bsp(actual: &Bsp, expected: &Bsp, label: &str) {
    for (index, (actual, expected)) in actual.lumps.iter().zip(&expected.lumps).enumerate() {
        assert!(actual == expected, "{label}: native lump {index} differs");
    }
    assert!(
        actual.to_bytes() == expected.to_bytes(),
        "{label}: complete BSP differs"
    );
}

fn align_ids(literal: &mut Entity, actual: &Entity) {
    literal.id = actual.id;
    assert_eq!(literal.solids.len(), actual.solids.len());
    for (literal, actual) in literal.solids.iter_mut().zip(&actual.solids) {
        literal.id = actual.id;
        assert_eq!(literal.sides.len(), actual.sides.len());
        for (literal, actual) in literal.sides.iter_mut().zip(&actual.sides) {
            literal.id = actual.id;
        }
    }
}

fn model_colors(
    bsp: &Bsp,
    model: usize,
    kind: LightingKind,
) -> Vec<(Vec<u8>, usize, Vec<RgbExp32>)> {
    let model = bsp.records::<Model>().unwrap()[model];
    let faces = bsp.lighting_faces(kind).unwrap();
    let infos = bsp.records::<TexInfo>().unwrap();
    let lighting = bsp.lighting(kind).unwrap().unwrap();
    faces[model.first_face as usize..(model.first_face + model.face_count) as usize]
        .iter()
        .map(|face| {
            let data = lighting
                .face(face, &infos[face.texinfo as usize])
                .unwrap()
                .unwrap();
            let mut colors = Vec::new();
            for style in 0..data.styles().len() {
                for map in 0..data.maps() {
                    colors.extend_from_slice(data.samples(style, map).unwrap());
                }
            }
            (data.styles().to_vec(), data.maps(), colors)
        })
        .collect()
}

fn top_sample(
    bsp: &Bsp,
    model: usize,
    pivot: DVec3,
    point: DVec3,
    kind: LightingKind,
    style: u8,
) -> RgbExp32 {
    let model = bsp.records::<Model>().unwrap()[model];
    let faces = bsp.lighting_faces(kind).unwrap();
    let planes = bsp.records::<bsp::Plane>().unwrap();
    let infos = bsp.records::<TexInfo>().unwrap();
    let lighting = bsp.lighting(kind).unwrap().unwrap();
    let target = point - pivot;
    let mut nearest: Option<(f64, RgbExp32)> = None;
    for face in &faces[model.first_face as usize..(model.first_face + model.face_count) as usize] {
        let plane = planes[face.plane as usize];
        if plane.normal[2] < 0.99 || (f64::from(plane.dist) - target.z).abs() > 1e-6 {
            continue;
        }
        let info = &infos[face.texinfo as usize];
        let frame = compiler::lighting::LuxelFrame::new(
            Plane {
                normal: DVec3::Z,
                dist: f64::from(plane.dist),
            },
            info,
            face.lightmap_mins,
            face.lightmap_size,
        )
        .unwrap();
        let data = lighting.face(face, info).unwrap().unwrap();
        let channel = data.styles().iter().position(|&value| value == style);
        let [width, height] = frame.dimensions();
        for t in 0..height {
            for s in 0..width {
                let distance = frame.position(s, t).unwrap().distance(target);
                if nearest.is_none_or(|(old, _)| distance < old) {
                    let value = channel.map_or(RgbExp32::default(), |index| {
                        data.samples(index, 0).unwrap()[t * width + s]
                    });
                    nearest = Some((distance, value));
                }
            }
        }
    }
    let (distance, value) = nearest.expect("missing horizontal model receiver");
    assert!(
        distance < 24.0,
        "model {model:?}, receiver {point}, nearest {distance}"
    );
    value
}

#[test]
fn nested_compounds_match_literal_local_geometry_uvs_and_shared_lighting_consumers() {
    let (directory, game) = fixture();
    let leaf = child(&SOURCE, PIVOT);
    let leaf_bytes = write(directory.path(), "leaf.vmf", &leaf);
    let mut middle = Document::empty().into_vmf();
    instance(&mut middle, "leaf.vmf", "16 32 0", "0 90 0");
    let middle_bytes = write(directory.path(), "middle.vmf", &middle);
    let base = lit_room();
    let mut parent = base.clone();
    let owners = [
        instance(&mut parent, "middle.vmf", "1104 1040 1152", "90 0 0"),
        instance(&mut parent, "middle.vmf", "1104 848 1152", "90 0 0"),
    ];
    let before = parent.clone();
    let context = saved(directory.path(), &parent);
    let prepared = resolve(&parent, &context, &game);
    let stats = prepared.stats().unwrap();
    assert_eq!(
        (stats.instances, stats.files, stats.max_expansion_depth),
        (4, 2, 2)
    );
    assert_eq!(
        (
            stats.brush_models,
            stats.world_brushes,
            stats.detail_brushes
        ),
        (2, 0, 0)
    );
    assert_eq!(prepared.generated_brushes().count(), 0);
    let generated = prepared
        .compilation_vmf()
        .unwrap()
        .entities
        .iter()
        .filter(|entity| prepared.root_entity(entity.id).is_some())
        .collect::<Vec<_>>();
    assert_eq!(generated.len(), 2);
    let mut flat = Document::new(base.clone());
    for (index, &shift) in [0.0, -192.0].iter().enumerate() {
        let pivot = DVec3::new(1108.0, 1088.0 + shift, 1144.0);
        let id = model(
            &mut flat,
            &[
                (
                    [1056.0, 1008.0 + shift, 1120.0],
                    [1152.0, 1136.0 + shift, 1136.0],
                ),
                (
                    [1040.0, 992.0 + shift, 1200.0],
                    [1160.0, 1136.0 + shift, 1216.0],
                ),
            ],
            pivot,
        );
        let actual = generated[index];
        assert_eq!(prepared.root_entity(actual.id), Some(owners[index]));
        assert_eq!(actual.origin(), Some(pivot));
        assert_eq!(actual.properties.get("angles"), Some("0 0 0"));
        assert_eq!(actual.properties.get("Solidity"), Some("0"));
        assert_eq!(actual.solids.len(), 2);
        for (solid, source) in actual.solids.iter().zip(&leaf.entities[0].solids) {
            for (side, original) in solid.sides.iter().zip(&source.sides) {
                assert_eq!(
                    prepared.face_origin(side.id),
                    Some((owners[index], "leaf.vmf", source.id, original.id))
                );
                for (point, old) in side.plane.0.iter().zip(original.plane.0) {
                    let expected =
                        DVec3::new(old.z + 1104.0, old.x + 1072.0 + shift, old.y + 1136.0);
                    assert!(point.abs_diff_eq(expected, 1e-9));
                }
            }
        }
        flat.edit("literal side order and attached UVs", |tx| {
            let literal = tx.entity_mut(id)?;
            for solid in &mut literal.solids {
                let sides = solid.sides.clone();
                solid.sides = NESTED_SIDE_ORDER.map(|side| sides[side].clone()).to_vec();
            }
            texture(
                literal,
                [
                    UvAxis {
                        axis: DVec3::new(2.0, 1.0, 2.0) / 3.0,
                        offset: (-11083.0 - 2.0 * shift) / 3.0,
                        scale: 0.5,
                    },
                    UvAxis {
                        axis: DVec3::new(-1.0, -2.0, 2.0) / 3.0,
                        offset: (455.0 + shift) / 3.0,
                        scale: 2.0,
                    },
                ],
            );
            Ok(())
        })
        .unwrap();
        let mut literal = flat.into_vmf();
        align_ids(entity(&mut literal, id), actual);
        flat = Document::new(literal);
    }
    let flat = flat.into_vmf();
    let mut direct = None;
    for bounces in [0, 1] {
        let options = Options {
            lighting: LightingMode::DirectLdrHdr,
            bounces,
            sky_quality: SkyQuality::Fast,
            ..Default::default()
        };
        let baseline = compiler::compile_with_options(&base, &game, options);
        let expected = compiler::compile_with_options(&flat, &game, options);
        let output = compile(&parent, &game, &context, options);
        let actual = successful(&output);
        literal_bsp(actual, successful(&expected), &format!("{bounces} bounces"));
        assert_eq!(output.stats.instances.as_ref().unwrap().brush_models, 2);
        assert_eq!(output.stats.lit_brush_models, 3);
        let models = actual.records::<Model>().unwrap();
        assert_eq!(models.len(), 4);
        for model in &models[2..] {
            assert_eq!(model.mins, [-68.0, -96.0, -24.0]);
            assert_eq!(model.maxs, [52.0, 48.0, 72.0]);
            assert_eq!(model.origin, [0.0; 3]);
        }
        let faces = actual.lighting_faces(LightingKind::Ldr).unwrap();
        let info = actual.records::<TexInfo>().unwrap()
            [faces[models[2].first_face as usize].texinfo as usize];
        for (row, expected) in info.texture_vecs.iter().zip([
            [4.0_f32 / 3.0, 2.0 / 3.0, 4.0 / 3.0, 101.0 / 3.0],
            [-1.0_f32 / 6.0, -1.0 / 3.0, 1.0 / 3.0, -43.0 / 3.0],
        ]) {
            assert_eq!(*row, expected);
        }
        let local = DVec3::new(6.0, 8.0, -12.0);
        for (row, expected) in info.texture_vecs.iter().zip([31.0, -22.0]) {
            let uv =
                DVec3::new(row[0] as f64, row[1] as f64, row[2] as f64).dot(local) + row[3] as f64;
            assert!((uv - expected).abs() < 1e-5);
        }
        for kind in [LightingKind::Ldr, LightingKind::Hdr] {
            for model in [0, 1] {
                assert_eq!(
                    model_colors(actual, model, kind),
                    model_colors(successful(&baseline), model, kind),
                    "model {model}, {kind:?}, {bounces} bounces"
                );
            }
            if bounces == 0 {
                let pivot = DVec3::new(1108.0, 1088.0, 1144.0);
                for style in [0, 1] {
                    let active = match (kind, style) {
                        (LightingKind::Ldr, 0) => 0,
                        (LightingKind::Ldr, _) => 1,
                        (LightingKind::Hdr, 0) => 2,
                        (LightingKind::Hdr, _) => 0,
                    };
                    let lower = top_sample(
                        actual,
                        2,
                        pivot,
                        DVec3::new(1104.0, 1072.0, 1136.0),
                        kind,
                        style,
                    );
                    assert_eq!(lower.rgb, [0; 3], "own shadow, {kind:?}, style {style}");
                    for (model, pivot, point) in [
                        (2, pivot, DVec3::new(1104.0, 1072.0, 1216.0)),
                        (
                            1,
                            DVec3::new(1080.0, 1032.0, 1040.0),
                            DVec3::new(1104.0, 1072.0, 1072.0),
                        ),
                        (0, DVec3::ZERO, DVec3::new(1104.0, 1072.0, 1024.0)),
                    ] {
                        let value = top_sample(actual, model, pivot, point, kind, style);
                        assert!(
                            value.rgb[active] > 0,
                            "model {model}, {kind:?}, style {style}: {value:?}"
                        );
                        assert!(
                            (0..3)
                                .filter(|&channel| channel != active)
                                .all(|channel| value.rgb[channel] == 0)
                        );
                    }
                }
            }
        }
        let received = top_sample(
            actual,
            2,
            DVec3::new(1108.0, 1088.0, 1144.0),
            DVec3::new(1104.0, 1072.0, 1216.0),
            LightingKind::Ldr,
            0,
        )
        .linear()[0];
        if bounces == 0 {
            direct = Some(received);
        } else {
            assert!(output.stats.model_bounce_samples > 0);
            let direct = direct.take().unwrap();
            assert!(received > direct, "direct={direct}, one bounce={received}");
        }
    }
    assert_eq!(parent, before);
    assert_eq!(
        std::fs::read(directory.path().join("maps/leaf.vmf")).unwrap(),
        leaf_bytes
    );
    assert_eq!(
        std::fs::read(directory.path().join("maps/middle.vmf")).unwrap(),
        middle_bytes
    );

    let mixed_point = |point: DVec3| {
        DVec3::new(
            0.7697511313200572 * point.x - 0.5162121193659353 * point.y
                + 0.37551064385876115 * point.z
                + 1024.0,
            0.5389855446957562 * point.x
                + 0.8407726623973335 * point.y
                + 0.050950100827363065 * point.z
                + 1024.0,
            -0.3420201433256687 * point.x
                + 0.16317591116653482 * point.y
                + 0.9254165783983234 * point.z
                + 1152.0,
        )
    };
    let mut mixed = room();
    instance(&mut mixed, "leaf.vmf", "1024 1024 1152", "20 35 10");
    write(directory.path(), "mixed.vmf", &mixed);
    let mixed_context = SourceContext::capture(
        &directory.path().join("maps/mixed.vmf"),
        &Control::default(),
    )
    .unwrap();
    let prepared = resolve(&mixed, &mixed_context, &game);
    let actual = prepared
        .compilation_vmf()
        .unwrap()
        .entities
        .iter()
        .find(|entity| entity.classname() == Some("func_brush"))
        .unwrap();
    let pivot = mixed_point(PIVOT);
    assert!(actual.origin().unwrap().abs_diff_eq(pivot, 1e-9));
    assert_eq!(actual.properties.get("angles"), Some("0 0 0"));
    let mut mins = DVec3::splat(f64::INFINITY);
    let mut maxs = DVec3::splat(f64::NEG_INFINITY);
    for (solid, source) in actual.solids.iter().zip(&leaf.entities[0].solids) {
        for (side, original) in solid.sides.iter().zip(&source.sides) {
            for (point, old) in side.plane.0.iter().zip(original.plane.0) {
                let expected = mixed_point(old);
                assert!(point.abs_diff_eq(expected, 1e-9));
                mins = mins.min(expected - pivot);
                maxs = maxs.max(expected - pivot);
                for (axis, old_axis) in [&side.u_axis, &side.v_axis]
                    .into_iter()
                    .zip([&original.u_axis, &original.v_axis])
                {
                    let actual_uv = point.dot(axis.axis) / axis.scale + axis.offset;
                    let expected_uv = old.dot(old_axis.axis) / old_axis.scale + old_axis.offset;
                    assert!((actual_uv - expected_uv).abs() < 1e-8);
                }
            }
        }
    }
    let output = compile(&mixed, &game, &mixed_context, Options::default());
    let model = successful(&output).records::<Model>().unwrap()[1];
    assert!(DVec3::from_array(model.mins.map(f64::from)).abs_diff_eq(mins, 1e-4));
    assert!(DVec3::from_array(model.maxs.map(f64::from)).abs_diff_eq(maxs, 1e-4));
    assert_eq!(model.origin, [0.0; 3]);
}

fn region(vmf: &mut Vmf) {
    vmf.cordon = Some(Node::block(
        "cordon",
        vec![
            Node::string("active", "1"),
            Node::string("mins", "896 896 1028"),
            Node::string("maxs", "1152 1152 1276"),
        ],
    ));
}

fn failed(output: &compiler::Output, owner: u32, reason: &str) {
    assert!(output.report.has_errors() && output.bsp.is_none() && output.leak.is_none());
    assert!(
        output.report.diagnostics.iter().any(|diagnostic| {
            diagnostic.object == Some(format!("entity:{owner}"))
                && diagnostic.message.contains("leaf.vmf")
                && diagnostic.message.contains(reason)
        }),
        "{reason}: {:?}",
        output.report
    );
}

fn prior_output(directory: &Path) -> std::path::PathBuf {
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
fn whole_model_region_selection_cannot_hide_source_intermediate_local_or_material_failures() {
    let (directory, game) = fixture();
    let leaf = child(
        &[
            ([-192.0, -32.0, 32.0], [192.0, 32.0, 48.0]),
            ([-48.0, -16.0, 80.0], [48.0, 16.0, 96.0]),
        ],
        DVec3::new(-128.0, 0.0, 4.0),
    );
    write(directory.path(), "leaf.vmf", &leaf);
    let mut base = room();
    region(&mut base);
    let mut parent = base.clone();
    let owners = [
        instance(&mut parent, "leaf.vmf", "1024 1024 1024", "0 0 0"),
        instance(&mut parent, "leaf.vmf", "1023.5 1024 1024", "0 0 0"),
        instance(&mut parent, "leaf.vmf", "1280 1024 1024", "0 0 0"),
    ];
    let context = saved(directory.path(), &parent);
    let prepared = resolve(&parent, &context, &game);
    let generated = owners.map(|owner| {
        prepared
            .compilation_vmf()
            .unwrap()
            .entities
            .iter()
            .find(|entity| prepared.root_entity(entity.id) == Some(owner))
            .unwrap()
    });
    let mut flat = Document::new(base);
    for (index, shift) in [0.0, -0.5, 256.0].into_iter().enumerate() {
        let id = model(
            &mut flat,
            &[
                (
                    [832.0 + shift, 992.0, 1056.0],
                    [1216.0 + shift, 1056.0, 1072.0],
                ),
                (
                    [976.0 + shift, 1008.0, 1104.0],
                    [1072.0 + shift, 1040.0, 1120.0],
                ),
            ],
            DVec3::new(896.0 + shift, 1024.0, 1028.0),
        );
        let mut literal = flat.into_vmf();
        let model = entity(&mut literal, id);
        let mut axes = source_axes();
        axes[0].offset = (-10219.0 - 2.0 * shift) / 3.0;
        axes[1].offset = (479.0 + shift) / 3.0;
        texture(model, axes);
        align_ids(model, generated[index]);
        flat = Document::new(literal);
    }
    let output = compile(&parent, &game, &context, Options::default());
    let expected = compiler::compile_with_options(&flat.into_vmf(), &game, Options::default());
    let bsp = successful(&output);
    literal_bsp(bsp, successful(&expected), "whole-model region");
    assert_eq!(output.stats.instances.as_ref().unwrap().brush_models, 3);
    let selection = output.stats.compile_region.as_ref().unwrap();
    assert!(
        selection
            .included
            .contains(&format!("entity:{}", generated[0].id))
    );
    assert!(
        selection
            .omitted
            .contains(&format!("entity:{}", generated[1].id))
    );
    assert!(
        selection
            .included
            .contains(&format!("entity:{}", generated[2].id))
    );
    let models = bsp.records::<Model>().unwrap();
    assert_eq!(models.len(), 3);
    for model in &models[1..] {
        assert_eq!(model.mins, [-64.0, -32.0, 28.0]);
        assert_eq!(model.maxs, [320.0, 32.0, 92.0]);
    }

    for (name, text) in [
        (
            "custom_origin",
            "Patch { include materials/test/origin_base.vmt }",
        ),
        ("origin_base", "UnlitGeneric { %compileorigin 1 }"),
        ("custom_liquid", "LightmappedGeneric { %compilewater 1 }"),
    ] {
        std::fs::write(
            directory
                .path()
                .join(format!("game/materials/test/{name}.vmt")),
            text,
        )
        .unwrap();
    }
    let destination = prior_output(directory.path());
    for case in [
        "source",
        "intermediate",
        "local",
        "custom_origin",
        "custom_liquid",
        "missing",
    ] {
        let mut leaf = child(&[([32.0, -16.0, 32.0], [64.0, 16.0, 48.0])], DVec3::ZERO);
        let mut parent = room();
        region(&mut parent);
        let (owner, reason) = match case {
            "source" => {
                leaf.entities[0].properties.set("origin", "1000 0 0");
                for side in &mut leaf.entities[0].solids[0].sides {
                    for point in &mut side.plane.0 {
                        point.x += 16968.0;
                    }
                }
                (
                    instance(&mut parent, "leaf.vmf", "-1000 0 0", "0 0 0"),
                    "bounds",
                )
            }
            "intermediate" => {
                let mut middle = Document::empty().into_vmf();
                instance(&mut middle, "leaf.vmf", "16360 0 0", "0 0 0");
                write(directory.path(), "middle.vmf", &middle);
                (
                    instance(&mut parent, "middle.vmf", "-16360 0 0", "0 0 0"),
                    "bounds",
                )
            }
            "local" => {
                leaf.entities[0].properties.set("origin", "-16360 0 0");
                (
                    instance(&mut parent, "leaf.vmf", "0 0 0", "0 0 0"),
                    "local coordinates",
                )
            }
            material => {
                for side in &mut leaf.entities[0].solids[0].sides {
                    side.material = format!("test/{material}");
                }
                (
                    instance(&mut parent, "leaf.vmf", "0 0 0", "0 0 0"),
                    match material {
                        "custom_origin" => "An origin brush requires compiled brush geometry",
                        "missing" => "Origin brush solid",
                        _ => "Child brush-model material",
                    },
                )
            }
        };
        write(directory.path(), "leaf.vmf", &leaf);
        let context = saved(directory.path(), &parent);
        let output = compile(&parent, &game, &context, Options::default());
        failed(&output, owner, reason);
        if case.starts_with("custom_") || case == "missing" {
            assert!(output.report.diagnostics.iter().any(|diagnostic| {
                diagnostic.message.contains(&if case == "custom_origin" {
                    format!("Child brush model {}:", leaf.entities[0].id)
                } else {
                    format!("solid {}", leaf.entities[0].solids[0].id)
                })
            }));
        }
        assert!(!output.write(&destination).unwrap());
        preserved(&destination);
    }
}

#[test]
fn prepared_publication_retains_child_and_classification_material_sources_after_output_drop() {
    for change_child in [true, false] {
        let (directory, game) = fixture();
        let leaf = child(&[([-32.0, -16.0, 32.0], [32.0, 16.0, 48.0])], PIVOT);
        write(directory.path(), "leaf.vmf", &leaf);
        let mut parent = room();
        instance(&mut parent, "leaf.vmf", "1024 1024 1024", "0 0 0");
        let context = saved(directory.path(), &parent);
        let destination = prior_output(directory.path());
        let output = compile(&parent, &game, &context, Options::default());
        successful(&output);
        assert_eq!(output.stats.instances.as_ref().unwrap().brush_models, 1);
        let prepared = output
            .prepare_write_controlled(&destination, &Control::default())
            .unwrap()
            .unwrap();
        drop(output);
        if change_child {
            let mut changed = leaf;
            changed.entities[0].properties.set("origin", "17 8 4");
            write(directory.path(), "leaf.vmf", &changed);
        } else {
            std::fs::write(
                directory.path().join("game/materials/test/model_base.vmt"),
                "UnlitGeneric { %compileorigin 1 }",
            )
            .unwrap();
        }
        assert!(prepared.commit(&Control::default()).is_err());
        preserved(&destination);
    }
}

#[test]
#[ignore = "requires qualified hammerr-physics worker and installed CS:S engine physics via CSS_DIR"]
fn native_engine_compound_child_matches_literal_physics() {
    let install = std::path::PathBuf::from(
        std::env::var_os("CSS_DIR").expect("set CSS_DIR for the explicit child-model native cook"),
    );
    assert!(install.join("bin/linux64/vphysics.so").is_file());
    let (directory, mut game) = fixture();
    game.base_dir = install;
    let scripts = directory.path().join("game/scripts");
    std::fs::create_dir(&scripts).unwrap();
    std::fs::write(
        scripts.join("surfaceproperties_manifest.txt"),
        "surfaceproperties_manifest { file scripts/mass_surfaces.txt }",
    )
    .unwrap();
    std::fs::write(
        scripts.join("mass_surfaces.txt"),
        "default { density 2000 }",
    )
    .unwrap();
    std::fs::write(
        directory
            .path()
            .join("game/materials/test/native_pivot.vmt"),
        "UnlitGeneric { %compileorigin 1 }",
    )
    .unwrap();
    let mut leaf = child(&SOURCE, DVec3::ZERO);
    let mut volume = Document::empty();
    volume
        .edit("native consumed pivot", |tx| {
            edit::create_block(
                tx,
                PIVOT - DVec3::splat(4.0),
                PIVOT + DVec3::splat(4.0),
                "test/native_pivot",
            )
        })
        .unwrap();
    let mut volume = volume.into_vmf().world.solids.pop().unwrap();
    volume.id = leaf.object_ids().max().unwrap() + 1;
    let first_side = leaf.side_ids().max().unwrap() + 1;
    for (offset, side) in volume.sides.iter_mut().enumerate() {
        side.id = first_side + u32::try_from(offset).unwrap();
    }
    leaf.entities[0].solids.push(volume);
    write(directory.path(), "leaf.vmf", &leaf);
    let mut parent = room();
    instance(&mut parent, "leaf.vmf", "1024 1024 1152", "0 0 0");
    let context = saved(directory.path(), &parent);
    let prepared = resolve(&parent, &context, &game);
    assert_eq!(prepared.stats().unwrap().consumed_origin_brushes, 1);
    let actual = prepared
        .compilation_vmf()
        .unwrap()
        .entities
        .iter()
        .find(|entity| prepared.root_entity(entity.id).is_some())
        .unwrap();
    let mut flat = Document::new(room());
    let id = model(
        &mut flat,
        &[
            ([960.0, 1008.0, 1104.0], [1088.0, 1024.0, 1200.0]),
            ([944.0, 1088.0, 1088.0], [1088.0, 1104.0, 1208.0]),
        ],
        DVec3::new(1040.0, 1032.0, 1156.0),
    );
    flat.edit("literal translated UVs and IDs", |tx| {
        let literal = tx.entity_mut(id)?;
        let mut axes = source_axes();
        axes[0].offset = -3577.0;
        axes[1].offset = 181.0;
        texture(literal, axes);
        align_ids(literal, actual);
        Ok(())
    })
    .unwrap();
    let options = Options {
        lighting: LightingMode::Fullbright,
        physics: compiler::PhysicsMode::Engine,
        ..Default::default()
    };
    let literal = compiler::compile_with_options(flat.vmf(), &game, options);
    let expanded = compile(&parent, &game, &context, options);
    for output in [&literal, &expanded] {
        let bsp = successful(output);
        assert_eq!(output.stats.physics_models, 2);
        assert_eq!(output.stats.physics_convexes, 8);
        let physics = bsp.physics().unwrap().unwrap();
        assert_eq!(physics.models.len(), 2);
        assert_eq!(physics.models[1].index, 1);
        assert_eq!(physics.models[1].solids.len(), 1);
        assert!(!physics.models[1].solids[0].body().unwrap().is_empty());
        let model = bsp.records::<Model>().unwrap()[1];
        assert_eq!(model.mins, [-96.0, -24.0, -68.0]);
        assert_eq!(model.maxs, [48.0, 72.0, 52.0]);
    }
    assert_eq!(
        successful(&expanded).lump(bsp::lump::PHYSCOLLIDE),
        successful(&literal).lump(bsp::lump::PHYSCOLLIDE),
        "native world and compound child collision must match the authored literal"
    );
}
