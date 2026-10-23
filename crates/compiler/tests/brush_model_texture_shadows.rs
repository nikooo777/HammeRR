mod common;

use std::{collections::BTreeSet, path::Path};

use compiler::{Control, LightingMode, Options, SkyQuality, instances};
use glam::{DMat3, DVec3};
use map_core::{Document, ObjectId, Plane, edit};
use source_formats::bsp::{self, Bsp, LightingKind, Model, RgbExp32, TexInfo};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::vmf::{Entity, UvAxis, Vmf, format_vec3};

const PIVOT: DVec3 = DVec3::new(64.5, -32.25, 96.125);
const MIXED: [f64; 9] = [
    0.7697511313200572,
    0.5389855446957562,
    -0.3420201433256687,
    -0.5162121193659353,
    0.8407726623973335,
    0.16317591116653482,
    0.37551064385876115,
    0.050950100827363065,
    0.9254165783983234,
];

fn texture(pixels: [[u8; 4]; 4]) -> Vec<u8> {
    let mut bytes = vec![0_u8; 80];
    bytes[..4].copy_from_slice(b"VTF\0");
    for (offset, value) in [(4, 7_u32), (8, 2), (12, 80), (20, 0x1000)] {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    for (offset, value) in [(16, 2_u16), (18, 2), (24, 1), (63, 1)] {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }
    for offset in [32, 36, 40] {
        bytes[offset..offset + 4].copy_from_slice(&0.25_f32.to_le_bytes());
    }
    bytes[56] = 1;
    bytes[57..61].copy_from_slice(&(-1_i32).to_le_bytes());
    for pixel in pixels {
        bytes.extend_from_slice(&pixel);
    }
    bytes
}

fn game() -> (tempfile::TempDir, GameFs) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/validation/brush-model-texture-shadows");
    std::fs::create_dir_all(&root).unwrap();
    let directory = tempfile::Builder::new()
        .prefix("fixture-")
        .tempdir_in(root)
        .unwrap();
    let game_root = directory.path().join("game");
    let materials = game_root.join("materials");
    for name in ["dev", "test"] {
        std::fs::create_dir_all(materials.join(name)).unwrap();
    }
    for (name, material) in [
        (
            "dev/dev_measuregeneric01",
            "LightmappedGeneric { $basetexture test/white }",
        ),
        (
            "test/bump",
            "LightmappedGeneric { $basetexture test/white $bumpmap test/normal }",
        ),
        ("test/sky", "UnlitGeneric { %compilesky 1 }"),
    ] {
        std::fs::write(materials.join(format!("{name}.vmt")), material).unwrap();
    }
    for (name, alpha) in [
        ("asymmetric", [0, 255, 255, 255]),
        ("clear", [0; 4]),
        ("opaque", [255; 4]),
    ] {
        std::fs::write(
            materials.join(format!("test/{name}.vmt")),
            format!("LightmappedGeneric {{ $basetexture test/{name} $alphatest 1 }}"),
        )
        .unwrap();
        std::fs::write(
            materials.join(format!("test/{name}.vtf")),
            texture(alpha.map(|alpha| [255, 255, 255, alpha])),
        )
        .unwrap();
    }
    for (name, pixel) in [("white", [255; 4]), ("normal", [128, 128, 255, 255])] {
        std::fs::write(
            materials.join(format!("test/{name}.vtf")),
            texture([pixel; 4]),
        )
        .unwrap();
    }
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(game_root),
    });
    (directory, game)
}

fn room() -> Document {
    let mut source = common::room();
    for side in source
        .world
        .solids
        .iter_mut()
        .flat_map(|solid| &mut solid.sides)
    {
        side.lightmap_scale = 64;
    }
    Document::new(source)
}

fn compound(document: &mut Document, angles: &str, bump: bool) {
    document
        .edit("compound model", |tx| {
            let mut solids = Vec::new();
            for (mins, maxs, material) in [
                (
                    [-80.0, -80.0, 64.0],
                    [80.0, 80.0, 80.0],
                    "dev/dev_measuregeneric01",
                ),
                (
                    [-112.0, -112.0, 128.0],
                    [112.0, 112.0, 136.0],
                    "test/asymmetric",
                ),
            ] {
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
                    side.lightmap_scale = 64;
                    let normal = Plane::from_points(side.plane.0).unwrap().normal;
                    if material == "test/asymmetric" && normal.z.abs() > 0.99 {
                        side.u_axis = UvAxis {
                            axis: DVec3::X,
                            offset: 0.0,
                            scale: 64.0,
                        };
                        side.v_axis = UvAxis {
                            axis: DVec3::Y,
                            offset: 0.0,
                            scale: 64.0,
                        };
                    } else if bump && material != "test/asymmetric" && normal.z > 0.99 {
                        side.material = "test/bump".into();
                    }
                }
                solids.push(id);
            }
            edit::tie_to_entity(
                tx,
                &solids,
                "func_brush",
                &[
                    ("origin".into(), format_vec3(PIVOT)),
                    ("angles".into(), angles.into()),
                ],
            )
        })
        .unwrap();
}

fn light(
    document: &mut Document,
    origin: DVec3,
    style: u8,
    spot_pitch: Option<i32>,
    ldr: &str,
    hdr: &str,
) {
    let mut properties = vec![
        ("_light".into(), ldr.into()),
        ("_lightHDR".into(), hdr.into()),
        ("_constant_attn".into(), "1".into()),
        ("_linear_attn".into(), "0".into()),
        ("_quadratic_attn".into(), "0".into()),
        ("style".into(), style.to_string()),
    ];
    if let Some(pitch) = spot_pitch {
        properties.extend([
            ("angles".into(), format!("{pitch} 0 0")),
            ("_inner_cone".into(), "45".into()),
            ("_cone".into(), "60".into()),
        ]);
    }
    document
        .edit("light", |tx| {
            edit::create_entity(
                tx,
                if spot_pitch.is_some() {
                    "light_spot"
                } else {
                    "light"
                },
                Some(origin),
                &properties,
                Vec::new(),
            )
        })
        .unwrap();
}

fn with_mask(source: &Vmf, name: &str) -> Vmf {
    let mut source = source.clone();
    for side in source
        .entities
        .iter_mut()
        .flat_map(|entity| &mut entity.solids)
        .flat_map(|solid| &mut solid.sides)
    {
        if side.material.eq_ignore_ascii_case("test/asymmetric") {
            side.material = format!("test/{name}");
        }
    }
    source
}

fn separate_pane(source: &Vmf) -> Vmf {
    let mut source = source.clone();
    let model = source
        .entities
        .iter_mut()
        .find(|entity| !entity.solids.is_empty())
        .unwrap();
    assert_eq!(model.solids.len(), 2);
    let pane = model.solids.pop().unwrap();
    let mut document = Document::new(source);
    document
        .edit("separate pane", |tx| {
            edit::create_entity(
                tx,
                "func_brush",
                Some(DVec3::new(-70.25, 83.5, 56.125)),
                &[],
                vec![pane],
            )
        })
        .unwrap();
    document.vmf().clone()
}

fn compiled(
    source: &Vmf,
    game: &GameFs,
    enabled: bool,
    mode: LightingMode,
    bounces: u8,
) -> compiler::Output {
    let original = source.clone();
    let output = compiler::compile_with_options(
        source,
        game,
        Options {
            lighting: mode,
            brush_texture_shadows: enabled,
            sky_quality: SkyQuality::Fast,
            bounces,
            ..Default::default()
        },
    );
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.bsp.is_some());
    assert_eq!(*source, original);
    output
}

fn sample(
    bsp: &Bsp,
    model: usize,
    authored: DVec3,
    kind: LightingKind,
    style: u8,
    map: usize,
) -> RgbExp32 {
    let target = authored - PIVOT;
    let model = bsp.records::<Model>().unwrap()[model];
    let faces = bsp.lighting_faces(kind).unwrap();
    let planes = bsp.records::<bsp::Plane>().unwrap();
    let infos = bsp.records::<TexInfo>().unwrap();
    let lighting = bsp.lighting(kind).unwrap().unwrap();
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
        let [width, height] = frame.dimensions();
        for t in 0..height {
            for s in 0..width {
                if frame.position(s, t).unwrap().distance(target) < 1e-6 {
                    let data = lighting.face(face, info).unwrap().unwrap();
                    assert!(map < data.maps());
                    return data
                        .styles()
                        .iter()
                        .position(|&value| value == style)
                        .map(|index| data.samples(index, map).unwrap()[t * width + s])
                        .unwrap_or_default();
                }
            }
        }
    }
    panic!("Missing model receiver {authored}, channel {kind:?}, style {style}, map {map}");
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
    let mut result = Vec::new();
    for face in &faces[model.first_face as usize..(model.first_face + model.face_count) as usize] {
        let Some(data) = lighting.face(face, &infos[face.texinfo as usize]).unwrap() else {
            continue;
        };
        let mut colors = Vec::new();
        for style in 0..data.styles().len() {
            colors.push(*data.average(style).unwrap());
            for map in 0..data.maps() {
                colors.extend_from_slice(data.samples(style, map).unwrap());
            }
        }
        result.push((data.styles().to_vec(), data.maps(), colors));
    }
    result
}

fn world_leaves(bsp: &Bsp) -> BTreeSet<usize> {
    let nodes = bsp.records::<bsp::Node>().unwrap();
    let leaves = bsp.records::<bsp::Leaf>().unwrap();
    let mut pending = vec![bsp.records::<Model>().unwrap()[0].head_node];
    let mut seen = BTreeSet::new();
    let mut found = BTreeSet::new();
    while let Some(index) = pending.pop() {
        if index < 0 {
            let leaf = usize::try_from(-1_i64 - i64::from(index)).unwrap();
            assert!(leaf < leaves.len());
            found.insert(leaf);
        } else {
            let index = index as usize;
            assert!(seen.insert(index), "cycle or shared node in world tree");
            pending.extend(nodes[index].children);
        }
    }
    found
}

fn assert_isolated(before: &Bsp, after: &Bsp) {
    for (index, (before, after)) in before.lumps.iter().zip(&after.lumps).enumerate() {
        if ![
            bsp::lump::FACES,
            bsp::lump::FACES_HDR,
            bsp::lump::LIGHTING,
            bsp::lump::LIGHTING_HDR,
        ]
        .contains(&index)
        {
            assert_eq!(before, after, "Changed non-lightmap payload {index}");
        }
    }
    for kind in [LightingKind::Ldr, LightingKind::Hdr] {
        let before_faces = before.lighting_faces(kind).unwrap();
        let after_faces = after.lighting_faces(kind).unwrap();
        assert_eq!(before_faces.len(), after_faces.len());
        for (mut before, mut after) in before_faces.into_iter().zip(after_faces) {
            before.styles = [0; 4];
            after.styles = [0; 4];
            before.light_offset = 0;
            after.light_offset = 0;
            assert_eq!(before, after, "Changed face geometry in {kind:?}");
        }
        for model in [0, 2] {
            assert_eq!(
                model_colors(before, model, kind),
                model_colors(after, model, kind),
                "Changed unrelated model {model}, {kind:?}"
            );
        }
    }
}

#[test]
fn shared_asymmetric_alpha_uses_each_models_literal_inverse_pose() {
    let (_directory, game) = game();
    let poses = [
        ("0 0 0", DMat3::IDENTITY),
        ("0 90 0", DMat3::from_cols(DVec3::Y, DVec3::NEG_X, DVec3::Z)),
        ("20 35 10", DMat3::from_cols_array(&MIXED)),
    ];
    let witness = DVec3::new(21.027969674181833, 20.799189544389677, 145.28810081686933);
    let authored = poses[2].1.transpose() * (witness - PIVOT) + PIVOT;
    assert!(authored.distance(DVec3::new(12288.0 / 287.0, 12288.0 / 287.0, 128.0)) < 1e-10);
    for axis in 0..2 {
        assert_eq!((authored[axis] / 64.0).rem_euclid(2.0).floor(), 0.0);
        assert_eq!(
            ((witness[axis] + PIVOT[axis]) / 64.0)
                .rem_euclid(2.0)
                .floor(),
            1.0
        );
    }
    let mut document = room();
    for (style, (angles, rotation)) in poses.into_iter().enumerate() {
        compound(&mut document, angles, false);
        let origin = rotation * (DVec3::new(0.0, 0.0, 224.0) - PIVOT) + PIVOT;
        light(
            &mut document,
            origin,
            style as u8,
            None,
            "255 0 0 120",
            "255 0 0 120",
        );
    }
    let source = document.vmf();
    let clear = compiled(
        &with_mask(source, "clear"),
        &game,
        true,
        LightingMode::DirectLdr,
        0,
    );
    let masked = compiled(source, &game, true, LightingMode::DirectLdr, 0);
    for style in 0..3 {
        for (x, y, transparent) in [
            (64.0_f64, 64.0, true),
            (-64.0, 64.0, false),
            (64.0, -64.0, false),
            (-64.0, -64.0, false),
        ] {
            for height in [128.0, 136.0] {
                let fraction = (224.0 - height) / 143.5;
                let u = (x * fraction / 64.0).rem_euclid(2.0).floor() as usize;
                let v = (y * fraction / 64.0).rem_euclid(2.0).floor() as usize;
                assert_eq!([0, 255, 255, 255][2 * v + u] == 0, transparent);
            }
            let point = DVec3::new(x, y, 80.0);
            let expected = sample(
                clear.bsp.as_ref().unwrap(),
                style + 1,
                point,
                LightingKind::Ldr,
                style as u8,
                0,
            );
            assert!(
                expected.rgb[0] > 0 && expected.rgb[1..] == [0, 0],
                "pose {style}, receiver {point}: {expected:?}"
            );
            assert_eq!(
                sample(
                    masked.bsp.as_ref().unwrap(),
                    style + 1,
                    point,
                    LightingKind::Ldr,
                    style as u8,
                    0
                ),
                if transparent {
                    expected
                } else {
                    RgbExp32::default()
                },
                "pose {style}, receiver {point}"
            );
        }
    }
}

#[test]
fn child_compound_self_cutouts_match_literal_and_preserve_unrelated_lighting() {
    let (directory, game) = game();
    let mut leaf = Document::empty();
    compound(&mut leaf, "0 0 0", false);
    leaf.edit("unrelated child receiver", |tx| {
        let ObjectId::Solid(id) = edit::create_block(
            tx,
            DVec3::new(-32.0, -32.0, 32.0),
            DVec3::new(32.0, 32.0, 48.0),
            "dev/dev_measuregeneric01",
        )?
        else {
            unreachable!()
        };
        for side in &mut tx.solid_mut(id)?.sides {
            side.lightmap_scale = 64;
        }
        edit::tie_to_entity(
            tx,
            &[id],
            "func_brush",
            &[("origin".into(), format_vec3(PIVOT))],
        )
    })
    .unwrap();
    let leaf = leaf.into_vmf();
    let child_path = directory.path().join("child.vmf");
    let child_bytes = leaf.write(&Default::default()).into_bytes();
    std::fs::write(&child_path, &child_bytes).unwrap();
    let mut base = room();
    light(
        &mut base,
        DVec3::new(32.0, 0.0, 224.0),
        0,
        None,
        "255 0 0 120",
        "0 0 255 240",
    );
    let mut parent = base.into_vmf();
    let helper_id = parent.object_ids().chain(parent.side_ids()).max().unwrap() + 1;
    let mut helper = Entity::new(helper_id, "func_instance");
    helper.properties.set("file", "child.vmf");
    helper.properties.set("origin", "32 0 0");
    helper.properties.set("angles", "0 0 0");
    let mut literal = parent.clone();
    parent.entities.push(helper);
    let parent_path = directory.path().join("parent.vmf");
    std::fs::write(&parent_path, parent.write(&Default::default())).unwrap();
    let source = instances::SourceContext::capture(&parent_path, &Control::default()).unwrap();
    let reference = instances::resolve(
        &parent,
        Some(&source),
        &Control::default(),
        &mut compiler::Report::default(),
    )
    .unwrap();
    assert_eq!(
        reference.brush_model_preparation(),
        instances::BrushModelPreparation::Deferred
    );
    assert!(reference.compilation_vmf().is_err());
    assert!(
        instances::resolve_for_compile(
            &parent,
            Some(&source),
            &game,
            &Control::default(),
            &mut compiler::Report::default()
        )
        .is_err()
    );
    let generated = reference
        .vmf()
        .entities
        .iter()
        .filter(|entity| reference.root_entity(entity.id) == Some(helper_id))
        .collect::<Vec<_>>();
    assert_eq!(generated.len(), leaf.entities.len());
    for (original, identity) in leaf.entities.iter().zip(generated) {
        let mut model = original.clone();
        model.id = identity.id;
        model
            .properties
            .set("origin", format_vec3(PIVOT + DVec3::X * 32.0));
        for (solid, ids) in model.solids.iter_mut().zip(&identity.solids) {
            solid.id = ids.id;
            for (side, id) in solid.sides.iter_mut().zip(&ids.sides) {
                side.id = id.id;
                for point in &mut side.plane.0 {
                    point.x += 32.0;
                }
                for axis in [&mut side.u_axis, &mut side.v_axis] {
                    axis.offset -= 32.0 * axis.axis.x / axis.scale;
                }
            }
        }
        literal.entities.push(model);
    }
    let options = Options {
        lighting: LightingMode::DirectLdrHdr,
        brush_texture_shadows: true,
        sky_quality: SkyQuality::Fast,
        ..Default::default()
    };
    let actual = compiler::compile_with_source_controlled(
        &parent,
        &game,
        options,
        None,
        Some(&source),
        &Control::default(),
    );
    assert!(!actual.report.has_errors(), "{:?}", actual.report);
    let expected = compiler::compile_with_options(&literal, &game, options);
    assert!(!expected.report.has_errors(), "{:?}", expected.report);
    assert_eq!(
        actual.bsp.as_ref().unwrap().to_bytes(),
        expected.bsp.as_ref().unwrap().to_bytes()
    );
    assert_eq!(actual.stats.brush_shadow_models, 0);
    assert_eq!(actual.stats.instances.as_ref().unwrap().brush_models, 2);
    let texture_path = directory.path().join("game/materials/test/asymmetric.vtf");
    let mut controls = Vec::new();
    for alpha in [0, 255] {
        std::fs::write(&texture_path, texture([[255, 255, 255, alpha]; 4])).unwrap();
        let control = compiler::compile_with_source_controlled(
            &parent,
            &game,
            options,
            None,
            Some(&source),
            &Control::default(),
        );
        assert!(!control.report.has_errors(), "{:?}", control.report);
        assert_eq!(control.stats.brush_shadow_models, 0);
        assert_isolated(actual.bsp.as_ref().unwrap(), control.bsp.as_ref().unwrap());
        controls.push(control);
    }
    for kind in [LightingKind::Ldr, LightingKind::Hdr] {
        for (x, transparent) in [(64.0, true), (-64.0, false)] {
            let target = DVec3::new(x, 64.0, 80.0);
            let clear = sample(controls[0].bsp.as_ref().unwrap(), 1, target, kind, 0, 0);
            let covered = sample(controls[1].bsp.as_ref().unwrap(), 1, target, kind, 0, 0);
            assert!(clear.linear().into_iter().any(|value| value > 0.0));
            assert_eq!(covered.linear(), [0.0; 3]);
            assert_eq!(
                sample(actual.bsp.as_ref().unwrap(), 1, target, kind, 0, 0),
                if transparent { clear } else { covered }
            );
        }
    }
    assert_eq!(std::fs::read(child_path).unwrap(), child_bytes);
}

#[test]
fn compound_cutouts_preserve_channels_bump_maps_other_scopes_and_source_custody() {
    let (directory, game) = game();
    let mut document = room();
    compound(&mut document, "0 0 0", true);
    document
        .edit("unrelated receiver", |tx| {
            let ObjectId::Solid(id) = edit::create_block(
                tx,
                DVec3::new(-32.0, -32.0, 32.0),
                DVec3::new(32.0, 32.0, 48.0),
                "dev/dev_measuregeneric01",
            )?
            else {
                unreachable!()
            };
            for side in &mut tx.solid_mut(id)?.sides {
                side.lightmap_scale = 64;
            }
            edit::tie_to_entity(
                tx,
                &[id],
                "func_brush",
                &[("origin".into(), format_vec3(PIVOT))],
            )
        })
        .unwrap();
    light(
        &mut document,
        DVec3::new(0.0, 0.0, 224.0),
        0,
        None,
        "255 0 0 120",
        "0 0 255 240",
    );
    light(
        &mut document,
        DVec3::new(0.0, 0.0, 224.0),
        1,
        Some(-90),
        "0 255 0 80",
        "255 0 0 160",
    );
    let source = document.vmf();
    let clear = compiled(
        &with_mask(source, "clear"),
        &game,
        true,
        LightingMode::DirectLdrHdr,
        0,
    );
    let opaque_source = with_mask(source, "opaque");
    let opaque = compiled(&opaque_source, &game, true, LightingMode::DirectLdrHdr, 0);
    let masked = compiled(source, &game, true, LightingMode::DirectLdrHdr, 0);
    let disabled = compiled(source, &game, false, LightingMode::DirectLdrHdr, 0);
    let separate = compiled(
        &separate_pane(&opaque_source),
        &game,
        true,
        LightingMode::DirectLdrHdr,
        0,
    );
    assert_isolated(disabled.bsp.as_ref().unwrap(), masked.bsp.as_ref().unwrap());
    for kind in [LightingKind::Ldr, LightingKind::Hdr] {
        for style in [0, 1] {
            let active = match (kind, style) {
                (LightingKind::Ldr, 0) | (LightingKind::Hdr, 1) => 0,
                (LightingKind::Ldr, 1) => 1,
                (LightingKind::Hdr, 0) => 2,
                _ => unreachable!(),
            };
            assert!(
                sample(
                    masked.bsp.as_ref().unwrap(),
                    2,
                    DVec3::new(0.0, 0.0, 48.0),
                    kind,
                    style,
                    0
                )
                .rgb[active]
                    > 0
            );
            for x in [64.0, -64.0] {
                let point = DVec3::new(x, 64.0, 80.0);
                for map in 0..4 {
                    let expected = sample(clear.bsp.as_ref().unwrap(), 1, point, kind, style, map);
                    assert!(
                        expected.rgb[active] > 0,
                        "open x={x}, {kind:?}, style={style}, map={map}: {expected:?}"
                    );
                    assert!(
                        expected
                            .rgb
                            .iter()
                            .enumerate()
                            .all(|(axis, &value)| axis == active || value == 0)
                    );
                    assert_eq!(
                        sample(masked.bsp.as_ref().unwrap(), 1, point, kind, style, map),
                        if x > 0.0 {
                            expected
                        } else {
                            RgbExp32::default()
                        }
                    );
                    assert_eq!(
                        sample(separate.bsp.as_ref().unwrap(), 1, point, kind, style, map),
                        expected
                    );
                    for control in [&opaque, &disabled] {
                        assert_eq!(
                            sample(control.bsp.as_ref().unwrap(), 1, point, kind, style, map),
                            RgbExp32::default()
                        );
                    }
                }
            }
        }
    }
    let destination = directory.path().join("preserved.bsp");
    assert!(masked.write(&destination).unwrap());
    let published = std::fs::read(&destination).unwrap();
    let path = directory.path().join("game/materials/test/asymmetric.vtf");
    let mut bytes = std::fs::read(&path).unwrap();
    bytes[83] = 255;
    std::fs::write(path, bytes).unwrap();
    assert!(masked.write(&destination).is_err());
    assert_eq!(std::fs::read(destination).unwrap(), published);
}

#[test]
fn own_cutouts_share_sun_sky_and_world_bounce_gates_without_becoming_sources() {
    let (_directory, game) = game();
    let mut source = room().vmf().clone();
    let mut sky_faces = 0;
    for side in source
        .world
        .solids
        .iter_mut()
        .flat_map(|solid| &mut solid.sides)
    {
        let plane = Plane::from_points(side.plane.0).unwrap();
        if plane.normal.x < -0.99 && (plane.dist + 256.0).abs() < 0.01 {
            side.material = "test/sky".into();
            sky_faces += 1;
        }
    }
    assert_eq!(sky_faces, 1);
    let mut document = Document::new(source);
    compound(&mut document, "0 0 0", false);
    light(
        &mut document,
        DVec3::new(0.0, 0.0, 224.0),
        0,
        Some(90),
        "255 0 0 200",
        "255 0 0 200",
    );
    document
        .edit("green sun and blue sky", |tx| {
            edit::create_entity(
                tx,
                "light_environment",
                Some(DVec3::new(0.0, 0.0, 224.0)),
                &[
                    ("_light".into(), "0 255 0 100".into()),
                    ("_ambient".into(), "0 0 255 100".into()),
                    ("angles".into(), "0 180 0".into()),
                    ("pitch".into(), "-30".into()),
                ],
                Vec::new(),
            )
        })
        .unwrap();
    let source = document.vmf();
    let clear = compiled(
        &with_mask(source, "clear"),
        &game,
        true,
        LightingMode::DirectLdr,
        1,
    );
    let opaque_source = with_mask(source, "opaque");
    let opaque = compiled(&opaque_source, &game, true, LightingMode::DirectLdr, 1);
    let separate = compiled(
        &separate_pane(&opaque_source),
        &game,
        true,
        LightingMode::DirectLdr,
        1,
    );
    assert!(clear.stats.model_bounce_samples > 0);
    let point = DVec3::new(0.0, 0.0, 80.0);
    let expected = sample(
        clear.bsp.as_ref().unwrap(),
        1,
        point,
        LightingKind::Ldr,
        0,
        0,
    );
    assert!(
        expected.rgb.iter().all(|&channel| channel > 0),
        "{expected:?}"
    );
    assert_eq!(
        sample(
            separate.bsp.as_ref().unwrap(),
            1,
            point,
            LightingKind::Ldr,
            0,
            0
        ),
        expected
    );
    let blocked = sample(
        opaque.bsp.as_ref().unwrap(),
        1,
        point,
        LightingKind::Ldr,
        0,
        0,
    )
    .linear();
    for (channel, value) in expected.linear().into_iter().enumerate() {
        assert!(
            blocked[channel] < value,
            "channel {channel}: blocked {blocked:?}, clear {expected:?}"
        );
    }
    let reference = clear.bsp.as_ref().unwrap();
    for output in [&opaque, &separate] {
        let bsp = output.bsp.as_ref().unwrap();
        assert_eq!(
            model_colors(reference, 0, LightingKind::Ldr),
            model_colors(bsp, 0, LightingKind::Ldr)
        );
    }
    let reference_ambient = reference.leaf_ambient(LightingKind::Ldr).unwrap().unwrap();
    assert_eq!(
        reference_ambient,
        opaque
            .bsp
            .as_ref()
            .unwrap()
            .leaf_ambient(LightingKind::Ldr)
            .unwrap()
            .unwrap()
    );
    let separate = separate.bsp.as_ref().unwrap();
    let world = world_leaves(reference);
    assert_eq!(world, world_leaves(separate));
    let bsp::LeafAmbient::Indexed {
        indices: reference_indices,
        samples: reference_samples,
    } = reference_ambient
    else {
        panic!("Missing indexed world ambient");
    };
    let bsp::LeafAmbient::Indexed { indices, samples } =
        separate.leaf_ambient(LightingKind::Ldr).unwrap().unwrap()
    else {
        panic!("Missing indexed world ambient");
    };
    assert_eq!(reference_samples, samples);
    for &leaf in &world {
        assert_eq!(
            reference_indices[leaf], indices[leaf],
            "World ambient leaf {leaf}"
        );
    }
    for indices in [&reference_indices, &indices] {
        for (leaf, index) in indices.iter().enumerate() {
            if !world.contains(&leaf) {
                assert_eq!(
                    *index,
                    bsp::LeafAmbientIndex::default(),
                    "Nonworld ambient leaf {leaf}"
                );
            }
        }
    }
}
