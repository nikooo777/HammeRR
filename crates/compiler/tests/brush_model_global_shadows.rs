mod common;
#[path = "common/detail_model.rs"]
mod detail_model;

use std::collections::BTreeSet;
use std::path::Path;

use compiler::instances::{self, SourceContext};
use compiler::{Control, LightingMode, Options, SkyQuality};
use glam::DVec3;
use map_core::{Document, ObjectId, Plane, edit};
use source_formats::bsp::{self, Bsp, LeafAmbient, LightingKind, Model, RgbExp32, TexInfo};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::keyvalues::Node;
use source_formats::vmf::{Entity, UvAxis, Vmf, format_vec3};

type Bounds = ([f64; 3], [f64; 3]);
const FLAG: &str = "vrad_brush_cast_shadows";
const WHITE: &str = "LightmappedGeneric { %notooltexture 1 $reflectivity \"[1 1 1]\" }";
const BLACK: &str = "LightmappedGeneric { %notooltexture 1 $reflectivity \"[0 0 0]\" }";
const PIVOT: DVec3 = DVec3::new(32.0, -16.0, 96.0);
const SOURCE: [Bounds; 2] = [
    ([64.0, -64.0, 32.0], [80.0, 64.0, 160.0]),
    ([-16.0, -48.0, 48.0], [0.0, 48.0, 144.0]),
];
const POSED: [Bounds; 2] = [
    ([-48.0, -80.0, 48.0], [80.0, 48.0, 64.0]),
    ([-32.0, -64.0, 128.0], [64.0, 32.0, 144.0]),
];

fn fixture() -> (tempfile::TempDir, GameFs) {
    let directory = tempfile::tempdir().unwrap();
    for name in ["materials/test", "materials/tools", "maps", "published"] {
        std::fs::create_dir_all(directory.path().join(name)).unwrap();
    }
    for (name, text) in [
        ("white", WHITE),
        ("black", BLACK),
        ("model_base", WHITE),
        ("model", "Patch { include materials/test/model_base.vmt }"),
        ("nodraw", "LightmappedGeneric { %compilenodraw 1 }"),
        ("alpha", "LightmappedGeneric { $alphatest 1 }"),
        (
            "nodraw_alpha",
            "LightmappedGeneric { %compilenodraw 1 $alphatest 1 }",
        ),
        ("translucent", "LightmappedGeneric { $translucent 1 }"),
        ("additive", "LightmappedGeneric { $additive 1 }"),
        ("origin", "LightmappedGeneric { %compileorigin 1 }"),
    ] {
        std::fs::write(
            directory.path().join(format!("materials/test/{name}.vmt")),
            text,
        )
        .unwrap();
    }
    std::fs::write(
        directory.path().join("materials/tools/toolsblack.vmt"),
        BLACK,
    )
    .unwrap();
    std::fs::write(
        directory.path().join("materials/tools/toolsskybox.vmt"),
        "UnlitGeneric { %compilesky 1 }",
    )
    .unwrap();
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().to_owned()),
    });
    (directory, game)
}

fn cutout_assets(directory: &Path) {
    for (name, alpha) in [
        ("mask", [0, 255, 255, 255]),
        ("clear", [0; 4]),
        ("covered", [255; 4]),
    ] {
        let material = format!("LightmappedGeneric {{ $basetexture test/{name} $alphatest 1 }}");
        if name == "mask" {
            std::fs::write(directory.join("materials/test/mask_base.vmt"), &material).unwrap();
        }
        std::fs::write(
            directory.join(format!("materials/test/{name}.vmt")),
            if name == "mask" {
                "Patch { include materials/test/mask_base.vmt }"
            } else {
                &material
            },
        )
        .unwrap();
        let mut bytes = vec![0_u8; 80];
        bytes[..4].copy_from_slice(b"VTF\0");
        for (offset, value) in [
            (4, 7_u32),
            (8, 2),
            (12, 80),
            (20, 0x1000 | source_formats::vtf::FLAG_NOMIP),
        ] {
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
        for alpha in alpha {
            bytes.extend_from_slice(&[255, 255, 255, alpha]);
        }
        std::fs::write(directory.join(format!("materials/test/{name}.vtf")), bytes).unwrap();
    }
}

fn mask(vmf: &Vmf, name: &str) -> Vmf {
    let mut result = vmf.clone();
    let mut replaced = 0;
    for side in result
        .entities
        .iter_mut()
        .flat_map(|entity| &mut entity.solids)
        .flat_map(|solid| &mut solid.sides)
    {
        if side.material.eq_ignore_ascii_case("test/mask") {
            side.material = format!("test/{name}");
            replaced += 1;
        }
    }
    assert!(replaced > 0, "control did not replace any mask faces");
    result
}

fn cutout_options(bounces: u8) -> Options {
    Options {
        brush_texture_shadows: true,
        ..options(bounces)
    }
}

fn room() -> Vmf {
    let mut vmf = common::room();
    for side in vmf
        .world
        .solids
        .iter_mut()
        .flat_map(|solid| &mut solid.sides)
    {
        side.material = "test/white".into();
        side.lightmap_scale = 32;
    }
    vmf
}

fn model(document: &mut Document, bounds: &[Bounds], pivot: DVec3, material: &str) -> u32 {
    document
        .edit("brush shadow fixture", |tx| {
            let mut solids = Vec::new();
            for &(mins, maxs) in bounds {
                let ObjectId::Solid(id) = edit::create_block(
                    tx,
                    DVec3::from_array(mins),
                    DVec3::from_array(maxs),
                    material,
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
                    (FLAG.into(), "1".into()),
                ],
            )?
            else {
                unreachable!()
            };
            Ok(id)
        })
        .unwrap()
}

fn entity(vmf: &mut Vmf, id: u32) -> &mut Entity {
    vmf.entities.iter_mut().find(|item| item.id == id).unwrap()
}

fn point(vmf: &mut Vmf, class: &str, origin: &str, fields: &[(&str, &str)]) -> u32 {
    let id = vmf.object_ids().chain(vmf.side_ids()).max().unwrap_or(0) + 1;
    let mut item = Entity::new(id, class);
    item.properties.set("origin", origin);
    for &(key, value) in fields {
        item.properties.set(key, value);
    }
    vmf.entities.push(item);
    id
}

fn lights(vmf: &mut Vmf) {
    for (class, style, ldr, hdr) in [
        ("light", "0", "255 0 0 150", "0 0 255 120"),
        ("light_spot", "1", "0 255 0 80", "255 0 0 60"),
    ] {
        point(
            vmf,
            class,
            "0 0 224",
            &[
                ("style", style),
                ("_light", ldr),
                ("_lightHDR", hdr),
                ("_constant_attn", "1"),
                ("_linear_attn", "0"),
                ("_quadratic_attn", "0"),
                ("angles", "-90 0 0"),
                ("_inner_cone", "45"),
                ("_cone", "60"),
            ],
        );
    }
}

fn options(bounces: u8) -> Options {
    Options {
        lighting: LightingMode::DirectLdrHdr,
        sky_quality: SkyQuality::Fast,
        bounces,
        ..Default::default()
    }
}

fn built(vmf: &Vmf, game: &GameFs, options: Options) -> compiler::Output {
    let before = vmf.clone();
    let output = compiler::compile_with_options(vmf, game, options);
    successful(&output);
    assert_eq!(vmf, &before);
    output
}

fn successful(output: &compiler::Output) -> &Bsp {
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.leak.is_none(), "{:?}", output.report);
    output.bsp.as_ref().unwrap()
}

fn sample(
    bsp: &Bsp,
    model: usize,
    target: DVec3,
    normal: DVec3,
    kind: LightingKind,
    style: u8,
) -> RgbExp32 {
    sample_style(bsp, model, target, normal, kind, style)
        .expect("fixed receiver lost its native style")
}

fn sample_style(
    bsp: &Bsp,
    model: usize,
    target: DVec3,
    normal: DVec3,
    kind: LightingKind,
    style: u8,
) -> Option<RgbExp32> {
    let model = bsp.records::<Model>().unwrap()[model];
    let faces = bsp.lighting_faces(kind).unwrap();
    let planes = bsp.records::<bsp::Plane>().unwrap();
    let infos = bsp.records::<TexInfo>().unwrap();
    let lighting = bsp.lighting(kind).unwrap().unwrap();
    for face in &faces[model.first_face as usize..(model.first_face + model.face_count) as usize] {
        let plane = planes[face.plane as usize];
        let plane_normal = DVec3::from_array(plane.normal.map(f64::from));
        if plane_normal.distance(normal) > 1e-6
            || (plane_normal.dot(target) - f64::from(plane.dist)).abs() > 1e-6
        {
            continue;
        }
        let info = &infos[face.texinfo as usize];
        let frame = compiler::lighting::LuxelFrame::new(
            Plane {
                normal: plane_normal,
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
                if frame.position(s, t).unwrap().distance(target) <= 1e-6 {
                    let data = lighting.face(face, info).unwrap().unwrap();
                    let index = data.styles().iter().position(|&value| value == style)?;
                    return Some(data.samples(index, 0).unwrap()[t * width + s]);
                }
            }
        }
    }
    panic!("missing fixed local receiver {target:?}, normal {normal:?}")
}

fn geometry_and_probe_locations(a: &Bsp, b: &Bsp) {
    for index in [
        bsp::lump::PLANES,
        bsp::lump::VERTEXES,
        bsp::lump::EDGES,
        bsp::lump::SURFEDGES,
        bsp::lump::MODELS,
        bsp::lump::BRUSHES,
        bsp::lump::BRUSHSIDES,
        bsp::lump::TEXINFO,
        bsp::lump::NODES,
        bsp::lump::LEAFS,
        bsp::lump::LEAFFACES,
        bsp::lump::LEAFBRUSHES,
        bsp::lump::VISIBILITY,
        bsp::lump::PHYSCOLLIDE,
        bsp::lump::PHYSDISP,
        bsp::lump::VERTNORMALS,
        bsp::lump::VERTNORMALINDICES,
        bsp::lump::AREAS,
        bsp::lump::AREAPORTALS,
    ] {
        assert!(
            a.lumps[index] == b.lumps[index],
            "geometry/contact lump {index}"
        );
    }
    for kind in [LightingKind::Ldr, LightingKind::Hdr] {
        let LeafAmbient::Indexed {
            indices: ai,
            samples: a,
        } = a.leaf_ambient(kind).unwrap().unwrap()
        else {
            panic!()
        };
        let LeafAmbient::Indexed {
            indices: bi,
            samples: b,
        } = b.leaf_ambient(kind).unwrap().unwrap()
        else {
            panic!()
        };
        assert_eq!(ai, bi);
        assert_eq!(a.len(), b.len());
        assert!(!a.is_empty());
        for (a, b) in a.iter().zip(&b) {
            assert_eq!((a.position, a.padding), (b.position, b.padding));
        }
    }
}

fn model_colors(bsp: &Bsp, index: usize, kind: LightingKind) -> Vec<(Vec<u8>, Vec<RgbExp32>)> {
    let model = bsp.records::<Model>().unwrap()[index];
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
            let colors = (0..data.styles().len())
                .flat_map(|style| {
                    (0..data.maps())
                        .flat_map(move |map| data.samples(style, map).unwrap().iter().copied())
                })
                .collect();
            (data.styles().to_vec(), colors)
        })
        .collect()
}

fn literal_local_vertices(bsp: &Bsp) {
    let model = bsp.records::<Model>().unwrap()[1];
    let faces = bsp.lighting_faces(LightingKind::Ldr).unwrap();
    let edges = bsp.records::<bsp::Edge>().unwrap();
    let surfedges = bsp.surfedges().unwrap();
    let vertices = bsp.vertices().unwrap();
    let mut actual = BTreeSet::new();
    for face in &faces[model.first_face as usize..(model.first_face + model.face_count) as usize] {
        for &edge in &surfedges
            [face.first_edge as usize..face.first_edge as usize + face.edge_count as usize]
        {
            let edge_record = edges[edge.unsigned_abs() as usize];
            actual.insert(
                vertices[usize::from(edge_record.0[usize::from(edge < 0)])].map(f32::to_bits),
            );
        }
    }
    let mut expected = BTreeSet::new();
    for (mins, maxs) in SOURCE {
        for x in [mins[0], maxs[0]] {
            for y in [mins[1], maxs[1]] {
                for z in [mins[2], maxs[2]] {
                    expected.insert(
                        (DVec3::new(x, y, z) - PIVOT)
                            .as_vec3()
                            .to_array()
                            .map(f32::to_bits),
                    );
                }
            }
        }
    }
    assert_eq!(actual, expected);
}

#[test]
fn requested_root_cutouts_use_literal_posed_uvs_for_world_other_model_and_self() {
    let (directory, game) = fixture();
    cutout_assets(directory.path());
    let pivot = DVec3::new(64.0, -32.0, 96.0);
    let mut document = Document::new(room());
    let caster = model(
        &mut document,
        &[
            ([-80.0, -80.0, 64.0], [80.0, 80.0, 80.0]),
            ([-112.0, -112.0, 128.0], [112.0, 112.0, 136.0]),
        ],
        pivot,
        "test/white",
    );
    let receiver = model(
        &mut document,
        &[([-192.0, -240.0, 8.0], [192.0, 64.0, 24.0])],
        DVec3::ZERO,
        "test/white",
    );
    let mut source = document.into_vmf();
    entity(&mut source, receiver).properties.remove(FLAG);
    let item = entity(&mut source, caster);
    item.properties.set("angles", "0 90 0");
    for side in &mut item.solids[1].sides {
        side.material = "test/mask".into();
        if Plane::from_points(side.plane.0).unwrap().normal.z.abs() > 0.99 {
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
        }
    }
    lights(&mut source);
    for light in source
        .entities
        .iter_mut()
        .filter(|entity| entity.solids.is_empty())
    {
        light.properties.set("origin", "32 -96 224");
    }
    let mut off = source.clone();
    entity(&mut off, caster).properties.set(FLAG, "0");
    let mut literal = source.clone();
    let item = entity(&mut literal, caster);
    item.properties.set("origin", "0 0 0");
    item.properties.set("angles", "0 0 0");
    for side in item.solids.iter_mut().flat_map(|solid| &mut solid.sides) {
        for point in &mut side.plane.0 {
            *point = DVec3::new(32.0 - point.y, point.x - 96.0, point.z);
        }
        for axis in [&mut side.u_axis, &mut side.v_axis] {
            axis.axis = DVec3::new(-axis.axis.y, axis.axis.x, axis.axis.z);
            axis.offset -= axis.axis.dot(DVec3::new(32.0, -96.0, 0.0)) / axis.scale;
        }
    }
    let actual = built(&source, &game, cutout_options(0));
    let clear = built(&mask(&source, "clear"), &game, cutout_options(0));
    let covered = built(&mask(&source, "covered"), &game, cutout_options(0));
    let isolated = built(&off, &game, cutout_options(0));
    let expected = built(&literal, &game, cutout_options(0));
    assert_eq!(actual.stats.brush_shadow_models, 1);
    geometry_and_probe_locations(successful(&actual), successful(&isolated));
    for kind in [LightingKind::Ldr, LightingKind::Hdr] {
        for (x, y, transparent) in [
            (128.0, 128.0, true),
            (-128.0, 128.0, false),
            (128.0, -128.0, false),
        ] {
            for (model, z) in [(0, 0.0), (2, 24.0)] {
                let target = DVec3::new(32.0 - y, x - 96.0, z);
                for style in [0, 1] {
                    let lit = sample(successful(&clear), model, target, DVec3::Z, kind, style);
                    assert!(lit.linear().into_iter().any(|value| value > 0.0));
                    let dark =
                        sample_style(successful(&covered), model, target, DVec3::Z, kind, style);
                    assert!(dark.is_none_or(|color| color.linear() == [0.0; 3]));
                    let color =
                        sample_style(successful(&actual), model, target, DVec3::Z, kind, style);
                    if transparent {
                        assert_eq!(color, Some(lit));
                    } else {
                        assert!(color.is_none_or(|color| color.linear() == [0.0; 3]));
                    }
                    assert_eq!(
                        color,
                        sample_style(successful(&expected), model, target, DVec3::Z, kind, style)
                    );
                    assert_eq!(
                        sample(successful(&isolated), model, target, DVec3::Z, kind, style),
                        lit
                    );
                }
            }
            let local = DVec3::new(x / 2.0, y / 2.0, 80.0) - pivot;
            let lit = sample(successful(&clear), 1, local, DVec3::Z, kind, 0);
            assert!(lit.linear().into_iter().any(|value| value > 0.0));
            assert_eq!(
                sample(successful(&actual), 1, local, DVec3::Z, kind, 0),
                if transparent {
                    lit
                } else {
                    RgbExp32::default()
                }
            );
        }
    }
}

#[test]
fn root_cutouts_filter_sky_ambient_and_reflected_global_segments() {
    let (directory, game) = fixture();
    cutout_assets(directory.path());
    let mut sky_room = room();
    for side in sky_room
        .world
        .solids
        .iter_mut()
        .flat_map(|solid| &mut solid.sides)
    {
        side.material = "test/black".into();
    }
    for side in &mut sky_room.world.solids[1].sides {
        side.material = "tools/toolsskybox".into();
    }
    let mut document = Document::new(sky_room);
    model(
        &mut document,
        &[([-256.0, -256.0, 112.0], [256.0, 256.0, 144.0])],
        DVec3::new(48.0, -32.0, 64.0),
        "test/mask",
    );
    let mut sky = document.into_vmf();
    point(
        &mut sky,
        "light_environment",
        "0 0 224",
        &[
            ("angles", "0 0 0"),
            ("pitch", "-90"),
            ("_light", "255 0 0 100"),
            ("_ambient", "0 0 255 100"),
            ("_lightHDR", "0 255 0 80"),
            ("_ambientHDR", "255 0 0 60"),
        ],
    );
    let clear = built(&mask(&sky, "clear"), &game, cutout_options(1));
    let covered = built(&mask(&sky, "covered"), &game, cutout_options(1));
    assert_eq!(
        (
            clear.stats.brush_shadow_models,
            covered.stats.brush_shadow_models
        ),
        (1, 1)
    );
    geometry_and_probe_locations(successful(&clear), successful(&covered));
    for kind in [LightingKind::Ldr, LightingKind::Hdr] {
        let color = sample(successful(&clear), 0, DVec3::ZERO, DVec3::Z, kind, 0).linear();
        assert!(color[0] > 0.0 && color[if kind == LightingKind::Ldr { 2 } else { 1 }] > 0.0);
        assert_eq!(
            sample(successful(&covered), 0, DVec3::ZERO, DVec3::Z, kind, 0).linear(),
            [0.0; 3]
        );
        let LeafAmbient::Indexed { samples: a, .. } =
            successful(&clear).leaf_ambient(kind).unwrap().unwrap()
        else {
            panic!()
        };
        let LeafAmbient::Indexed { samples: b, .. } =
            successful(&covered).leaf_ambient(kind).unwrap().unwrap()
        else {
            panic!()
        };
        assert!(
            a.iter()
                .flat_map(|sample| sample.cube)
                .any(|color| color.linear().into_iter().any(|value| value > 0.0))
        );
        assert!(
            b.iter()
                .flat_map(|sample| sample.cube)
                .all(|color| color.linear() == [0.0; 3])
        );
    }

    let mut bounce_room = room();
    for side in bounce_room
        .world
        .solids
        .iter_mut()
        .skip(1)
        .flat_map(|solid| &mut solid.sides)
    {
        side.material = "test/black".into();
    }
    let mut document = Document::new(bounce_room);
    let source = model(
        &mut document,
        &[([-128.0, -128.0, 128.0], [128.0, 128.0, 144.0])],
        DVec3::ZERO,
        "test/white",
    );
    model(
        &mut document,
        &[([-256.0, -256.0, 16.0], [256.0, 256.0, 24.0])],
        DVec3::ZERO,
        "test/mask",
    );
    let mut bounce = document.into_vmf();
    let item = entity(&mut bounce, source);
    item.properties.set("classname", "func_detail");
    item.properties.remove(FLAG);
    point(
        &mut bounce,
        "light_spot",
        "0 0 32",
        &[
            ("angles", "90 0 0"),
            ("_inner_cone", "15"),
            ("_cone", "20"),
            ("_light", "255 0 0 160"),
            ("_lightHDR", "0 0 255 96"),
            ("_constant_attn", "1"),
            ("_linear_attn", "0"),
            ("_quadratic_attn", "0"),
        ],
    );
    let clear = built(&mask(&bounce, "clear"), &game, cutout_options(1));
    let covered = built(&mask(&bounce, "covered"), &game, cutout_options(1));
    let mut model_source = mask(&bounce, "clear");
    let item = entity(&mut model_source, source);
    item.properties.set("classname", "func_brush");
    item.properties.set(FLAG, "1");
    for side in item.solids.iter_mut().flat_map(|solid| &mut solid.sides) {
        side.material = "test/covered".into();
    }
    let model_source = built(&model_source, &game, cutout_options(1));
    for (kind, channel) in [(LightingKind::Ldr, 0), (LightingKind::Hdr, 2)] {
        assert!(
            sample(successful(&clear), 0, DVec3::ZERO, DVec3::Z, kind, 0).linear()[channel] > 0.0
        );
        assert_eq!(
            sample(successful(&covered), 0, DVec3::ZERO, DVec3::Z, kind, 0).linear(),
            [0.0; 3]
        );
        assert!(
            model_colors(successful(&model_source), 1, kind)
                .iter()
                .flat_map(|(_, colors)| colors)
                .any(|color| color.linear()[channel] > 0.0)
        );
        assert_eq!(
            sample(successful(&model_source), 0, DVec3::ZERO, DVec3::Z, kind, 0).linear(),
            [0.0; 3]
        );
    }
}

#[test]
fn explicit_global_compound_pose_matches_literal_blockers_and_keeps_opt_out_isolation() {
    let (_directory, game) = fixture();
    let mut document = Document::new(room());
    let caster = model(&mut document, &SOURCE, PIVOT, "test/model");
    let receiver = model(
        &mut document,
        &[([-160.0, -16.0, 8.0], [160.0, 16.0, 24.0])],
        DVec3::ZERO,
        "test/white",
    );
    let mut on = document.into_vmf();
    entity(&mut on, caster).properties.set("angles", "90 90 0");
    entity(&mut on, receiver).properties.remove(FLAG);
    lights(&mut on);
    let mut off = on.clone();
    entity(&mut off, caster).properties.set(FLAG, "0");
    let mut absent = off.clone();
    entity(&mut absent, caster).properties.remove(FLAG);
    let off_output = built(&off, &game, options(0));
    let absent_output = built(&absent, &game, options(0));
    let on_output = built(&on, &game, options(0));
    assert_eq!(
        (
            off_output.stats.brush_shadow_models,
            absent_output.stats.brush_shadow_models,
            on_output.stats.brush_shadow_models
        ),
        (0, 0, 1)
    );
    assert!(on_output.stats.lighting_total_work > off_output.stats.lighting_total_work);
    let (off_bsp, absent_bsp, on_bsp) = (
        successful(&off_output),
        successful(&absent_output),
        successful(&on_output),
    );
    for index in 0..off_bsp.lumps.len() {
        if index != bsp::lump::ENTITIES {
            assert!(
                off_bsp.lumps[index] == absent_bsp.lumps[index],
                "opt-out lump {index}"
            );
        }
    }
    assert_eq!(off_bsp.game_lumps, absent_bsp.game_lumps);
    let missing_materials = built(&off, &common::empty_game(), Options::default());
    assert_eq!(missing_materials.stats.brush_shadow_models, 0);
    geometry_and_probe_locations(off_bsp, on_bsp);
    let native = on_bsp.records::<Model>().unwrap()[1];
    assert_eq!(native.mins, [-48.0, -48.0, -64.0]);
    assert_eq!(native.maxs, [48.0, 80.0, 64.0]);
    assert_eq!(native.origin, [0.0; 3]);
    literal_local_vertices(on_bsp);

    let mut literal = Document::new(off.clone());
    literal
        .edit("independent posed world blockers", |tx| {
            for (mins, maxs) in POSED {
                let ObjectId::Solid(id) = edit::create_block(
                    tx,
                    DVec3::from_array(mins),
                    DVec3::from_array(maxs),
                    "test/nodraw",
                )?
                else {
                    unreachable!()
                };
                edit::tie_to_entity(tx, &[id], "func_detail", &[])?;
            }
            Ok(())
        })
        .unwrap();
    let literal_output = built(literal.vmf(), &game, options(0));
    let literal_bsp = successful(&literal_output);
    for (kind, colors) in [
        (LightingKind::Ldr, [[150.0, 0.0, 0.0], [0.0, 80.0, 0.0]]),
        (LightingKind::Hdr, [[0.0, 0.0, 120.0], [60.0, 0.0, 0.0]]),
    ] {
        assert_eq!(
            model_colors(off_bsp, 1, kind),
            model_colors(on_bsp, 1, kind)
        );
        for (model, target) in [(0, DVec3::ZERO), (2, DVec3::new(0.0, 0.0, 24.0))] {
            for (style, expected) in colors.into_iter().enumerate() {
                assert_eq!(
                    sample(off_bsp, model, target, DVec3::Z, kind, style as u8).linear(),
                    expected
                );
                let dark = sample(on_bsp, model, target, DVec3::Z, kind, style as u8);
                assert_eq!(dark.linear(), [0.0; 3]);
                assert_eq!(
                    dark,
                    sample(literal_bsp, model, target, DVec3::Z, kind, style as u8)
                );
            }
        }
        for (model, outside) in [
            (0, DVec3::new(192.0, 128.0, 0.0)),
            (2, DVec3::new(-128.0, 0.0, 24.0)),
        ] {
            for style in [0, 1] {
                let clear = sample(off_bsp, model, outside, DVec3::Z, kind, style);
                assert!(clear.linear().into_iter().any(|value| value > 0.0));
                assert_eq!(sample(on_bsp, model, outside, DVec3::Z, kind, style), clear);
                assert_eq!(
                    sample(literal_bsp, model, outside, DVec3::Z, kind, style),
                    clear
                );
            }
        }
        assert_eq!(
            sample(on_bsp, 1, DVec3::new(32.0, 32.0, 16.0), -DVec3::X, kind, 0).linear(),
            [0.0; 3]
        );
        assert!(
            sample(on_bsp, 1, DVec3::new(-48.0, 32.0, 16.0), -DVec3::X, kind, 0)
                .linear()
                .into_iter()
                .any(|value| value > 0.0)
        );
    }
}

#[test]
fn receiverless_caster_blocks_sun_sky_ambient_and_detail_without_removing_probe_locations() {
    let (directory, game) = fixture();
    detail_model::write(directory.path(), "receiver");
    let mut source = room();
    for side in source
        .world
        .solids
        .iter_mut()
        .flat_map(|solid| &mut solid.sides)
    {
        side.material = "test/black".into();
    }
    for side in &mut source.world.solids[1].sides {
        side.material = "tools/toolsskybox".into();
    }
    let mut document = Document::new(source);
    let caster = model(
        &mut document,
        &[([-256.0, -256.0, 112.0], [256.0, 256.0, 144.0])],
        DVec3::new(48.0, -32.0, 64.0),
        "test/nodraw",
    );
    let mut on = document.into_vmf();
    point(
        &mut on,
        "prop_detail",
        "0 0 16",
        &[("model", "models/props/receiver.mdl")],
    );
    point(
        &mut on,
        "light_environment",
        "0 0 224",
        &[
            ("angles", "0 0 0"),
            ("pitch", "-90"),
            ("_light", "255 0 0 100"),
            ("_ambient", "0 0 255 100"),
            ("_lightHDR", "0 255 0 80"),
            ("_ambientHDR", "255 0 0 60"),
        ],
    );
    let mut off = on.clone();
    entity(&mut off, caster).properties.set(FLAG, "0");
    let a = built(&off, &game, options(1));
    let b = built(&on, &game, options(1));
    assert_eq!(
        (b.stats.brush_shadow_models, b.stats.lit_brush_models),
        (1, 0)
    );
    let (a, b) = (successful(&a), successful(&b));
    geometry_and_probe_locations(a, b);
    for kind in [LightingKind::Ldr, LightingKind::Hdr] {
        let clear = sample(a, 0, DVec3::ZERO, DVec3::Z, kind, 0).linear();
        assert!(clear[0] > 0.0);
        assert!(clear[if kind == LightingKind::Ldr { 2 } else { 1 }] > 0.0);
        assert_eq!(
            sample(b, 0, DVec3::ZERO, DVec3::Z, kind, 0).linear(),
            [0.0; 3]
        );
        let LeafAmbient::Indexed { samples: clear, .. } = a.leaf_ambient(kind).unwrap().unwrap()
        else {
            panic!()
        };
        let LeafAmbient::Indexed { samples: dark, .. } = b.leaf_ambient(kind).unwrap().unwrap()
        else {
            panic!()
        };
        assert_eq!(clear.len(), 1);
        assert_eq!(clear[0].position, [128; 3]);
        assert!(
            clear
                .iter()
                .flat_map(|sample| sample.cube)
                .flat_map(|color| color.linear())
                .any(|value| value > 0.0)
        );
        assert!(
            dark.iter()
                .flat_map(|sample| sample.cube)
                .all(|color| color.linear() == [0.0; 3])
        );
    }
    let clear = a.detail_props().unwrap().unwrap();
    let dark = b.detail_props().unwrap().unwrap();
    assert_eq!((clear.props.len(), dark.props.len()), (1, 1));
    assert!(clear.props[0].lighting.linear()[0] > 0.0);
    assert!(clear.props[0].lighting.linear()[1] > 0.0);
    assert_eq!(dark.props[0].lighting.linear(), [0.0; 3]);
    assert_eq!(clear.props[0].origin, dark.props[0].origin);
    assert_eq!(clear.props[0].leaf, dark.props[0].leaf);
}

#[test]
fn global_models_block_reflected_segments_without_becoming_world_radiance_sources() {
    let (_directory, game) = fixture();
    let mut base = room();
    for side in base
        .world
        .solids
        .iter_mut()
        .skip(1)
        .flat_map(|solid| &mut solid.sides)
    {
        side.material = "test/black".into();
    }
    let mut document = Document::new(base);
    let source = model(
        &mut document,
        &[([-128.0, -128.0, 128.0], [128.0, 128.0, 144.0])],
        DVec3::ZERO,
        "test/white",
    );
    let mut isolated = document.into_vmf();
    entity(&mut isolated, source).properties.set(FLAG, "0");
    point(
        &mut isolated,
        "light_spot",
        "0 0 32",
        &[
            ("angles", "90 0 0"),
            ("_inner_cone", "15"),
            ("_cone", "20"),
            ("_light", "255 0 0 160"),
            ("_lightHDR", "0 0 255 96"),
            ("_constant_attn", "1"),
            ("_linear_attn", "0"),
            ("_quadratic_attn", "0"),
        ],
    );
    let mut global = isolated.clone();
    entity(&mut global, source).properties.set(FLAG, "1");
    let mut world = isolated.clone();
    let item = entity(&mut world, source);
    item.properties.set("classname", "func_detail");
    item.properties.remove(FLAG);
    let mut blocked = Document::new(world.clone());
    model(
        &mut blocked,
        &[([-256.0, -256.0, 16.0], [256.0, 256.0, 24.0])],
        DVec3::ZERO,
        "test/nodraw",
    );
    let a = built(&isolated, &game, options(1));
    let b = built(&global, &game, options(1));
    let c = built(&world, &game, options(1));
    let d = built(blocked.vmf(), &game, options(1));
    geometry_and_probe_locations(successful(&a), successful(&b));
    for (kind, channel) in [(LightingKind::Ldr, 0), (LightingKind::Hdr, 2)] {
        for output in [&a, &b, &d] {
            assert_eq!(
                sample(successful(output), 0, DVec3::ZERO, DVec3::Z, kind, 0).linear(),
                [0.0; 3]
            );
        }
        assert!(sample(successful(&c), 0, DVec3::ZERO, DVec3::Z, kind, 0).linear()[channel] > 0.0);
        assert!(
            model_colors(successful(&a), 1, kind)
                .iter()
                .flat_map(|(_, colors)| colors)
                .any(|color| color.linear()[channel] > 0.0)
        );
    }
}

fn region(vmf: &mut Vmf) {
    vmf.cordon = Some(Node::block(
        "cordon",
        vec![
            Node::string("active", "1"),
            Node::string("mins", "-128 -128 4"),
            Node::string("maxs", "128 128 252"),
        ],
    ));
}

#[test]
fn requested_source_class_fields_and_opaque_materials_are_checked_before_region_omission() {
    let (_directory, game) = fixture();
    let mut document = Document::new(room());
    let id = model(
        &mut document,
        &[([512.0, -32.0, 64.0], [576.0, 32.0, 80.0])],
        DVec3::new(544.0, 0.0, 72.0),
        "test/model",
    );
    let mut valid = document.into_vmf();
    region(&mut valid);
    let accepted = built(&valid, &game, Options::default());
    assert_eq!(accepted.stats.brush_shadow_models, 0);
    assert_eq!(successful(&accepted).records::<Model>().unwrap().len(), 1);
    assert!(accepted.report.diagnostics.iter().any(|item| item.message
        == "Brush global shadows: 1 admitted Shadows Yes source request(s); not baked in Fullbright"));
    for (key, value) in [
        (FLAG, "2"),
        ("classname", "func_wall"),
        ("origin", ""),
        ("angles", "NaN 0 0"),
        ("parentname", "mover"),
        ("Solidity", "1"),
        ("material", "alpha"),
        ("material", "nodraw_alpha"),
        ("material", "translucent"),
        ("material", "additive"),
        ("material", "origin"),
        ("material", "missing"),
    ] {
        let mut invalid = valid.clone();
        let caster = entity(&mut invalid, id);
        if key == "material" {
            for side in caster.solids.iter_mut().flat_map(|solid| &mut solid.sides) {
                side.material = format!("test/{value}");
            }
        } else {
            caster.properties.set(key, value);
        }
        let before = invalid.clone();
        let output = compiler::compile_with_options(&invalid, &game, Options::default());
        assert_eq!(invalid, before);
        assert!(
            output.report.has_errors() && output.bsp.is_none() && output.leak.is_none(),
            "{key}={value}: {:?}",
            output.report
        );
        let owners: Vec<_> = std::iter::once(format!("entity:{id}"))
            .chain(
                entity(&mut invalid, id)
                    .solids
                    .iter()
                    .map(|solid| format!("solid:{}", solid.id)),
            )
            .collect();
        assert!(
            output.report.diagnostics.iter().any(|item| item
                .message
                .contains("Brush global shadows:")
                && item
                    .object
                    .as_ref()
                    .is_some_and(|owner| owners.contains(owner))),
            "{key}={value}: {:?}",
            output.report
        );
    }
}

fn save(directory: &Path, name: &str, vmf: &Vmf) {
    std::fs::write(
        directory.join("maps").join(name),
        vmf.write(&Default::default()),
    )
    .unwrap();
}

fn context(directory: &Path, vmf: &Vmf) -> SourceContext {
    save(directory, "parent.vmf", vmf);
    SourceContext::capture(&directory.join("maps/parent.vmf"), &Control::default()).unwrap()
}

fn from_source(
    vmf: &Vmf,
    game: &GameFs,
    context: &SourceContext,
    options: Options,
) -> compiler::Output {
    let before = vmf.clone();
    let output = compiler::compile_with_source_controlled(
        vmf,
        game,
        options,
        None,
        Some(context),
        &Control::default(),
    );
    successful(&output);
    assert_eq!(vmf, &before);
    output
}

fn cutout_child(directory: &Path) -> Vmf {
    cutout_assets(directory);
    std::fs::write(
        directory.join("materials/test/pivot.vmt"),
        "UnlitGeneric { %compileorigin 1 }",
    )
    .unwrap();
    let mut document = Document::empty();
    let id = model(
        &mut document,
        &[([-48.0, -64.0, 96.0], [48.0, 64.0, 104.0])],
        DVec3::splat(999.0),
        "test/mask",
    );
    document
        .edit("effective cutout pivot", |tx| {
            let volume = edit::block_solid(
                tx,
                DVec3::new(8.0, 0.0, -8.0),
                DVec3::new(24.0, 16.0, 8.0),
                "test/pivot",
            )?;
            let item = tx.entity_mut(id)?;
            item.properties.set("angles", "0 90 0");
            for side in &mut item.solids[0].sides {
                side.u_axis = UvAxis {
                    axis: DVec3::new(1.0, 2.0, 2.0) / 3.0,
                    offset: -25.0 / 24.0,
                    scale: 64.0,
                };
                side.v_axis = UvAxis {
                    axis: DVec3::new(2.0, 2.0, -1.0) / 3.0,
                    offset: 25.0 / 48.0,
                    scale: 64.0,
                };
            }
            item.solids.push(volume);
            Ok(())
        })
        .unwrap();
    document.into_vmf()
}

#[test]
fn requested_child_cutouts_lower_effective_poses_and_match_independent_repeated_literals() {
    let (directory, game) = fixture();
    let child = cutout_child(directory.path());
    save(directory.path(), "grille.vmf", &child);
    let child_bytes = std::fs::read(directory.path().join("maps/grille.vmf")).unwrap();
    let mut base = room();
    lights(&mut base);
    let mut parent = base.clone();
    let owners = [
        point(
            &mut parent,
            "func_instance",
            "-96 0 216",
            &[("file", "grille.vmf"), ("angles", "180 0 0")],
        ),
        point(
            &mut parent,
            "func_instance",
            "96 0 216",
            &[("file", "grille.vmf"), ("angles", "180 0 0")],
        ),
    ];
    let source = context(directory.path(), &parent);
    let mut report = compiler::Report::default();
    let reference =
        instances::resolve(&parent, Some(&source), &Control::default(), &mut report).unwrap();
    assert_eq!(
        reference.brush_model_preparation(),
        instances::BrushModelPreparation::Deferred
    );
    assert!(reference.compilation_vmf().is_err());
    assert_eq!(reference.generated_brushes().count(), 0);
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
    let mut literal = Document::new(base);
    let ids = [
        model(
            &mut literal,
            &[([-184.0, -56.0, 112.0], [-56.0, 40.0, 120.0])],
            DVec3::new(-112.0, 8.0, 216.0),
            "test/mask",
        ),
        model(
            &mut literal,
            &[([8.0, -56.0, 112.0], [136.0, 40.0, 120.0])],
            DVec3::new(80.0, 8.0, 216.0),
            "test/mask",
        ),
    ];
    let mut literal = literal.into_vmf();
    for (index, (&id, owner)) in ids.iter().zip(owners).enumerate() {
        let generated = reference
            .vmf()
            .entities
            .iter()
            .find(|item| reference.root_entity(item.id) == Some(owner))
            .unwrap();
        let item = entity(&mut literal, id);
        item.id = generated.id;
        let origin = item.properties.get("origin").unwrap().to_owned();
        item.properties.remove("origin");
        item.properties.set("origin", origin);
        let sides = item.solids[0].sides.clone();
        item.solids[0].sides = [1, 0, 5, 4, 3, 2].map(|side| sides[side].clone()).to_vec();
        item.solids[0].id = generated.solids[0].id;
        for (side, identity) in item.solids[0]
            .sides
            .iter_mut()
            .zip(&generated.solids[0].sides)
        {
            side.id = identity.id;
            side.u_axis = UvAxis {
                axis: DVec3::new(2.0, 1.0, -2.0) / 3.0,
                offset: [2.5, 0.5][index],
                scale: 64.0,
            };
            side.v_axis = UvAxis {
                axis: DVec3::new(2.0, 2.0, 1.0) / 3.0,
                offset: [35.0 / 48.0, -61.0 / 48.0][index],
                scale: 64.0,
            };
            assert!(reference.face_origin(side.id).is_some_and(
                |(actual_owner, file, solid, original_side)| {
                    actual_owner == owner
                        && file == "grille.vmf"
                        && solid == child.entities[0].solids[0].id
                        && child.entities[0].solids[0]
                            .sides
                            .iter()
                            .any(|side| side.id == original_side)
                }
            ));
        }
    }
    let actual = from_source(&parent, &game, &source, cutout_options(0));
    let expected = built(&literal, &game, cutout_options(0));
    assert_eq!(
        successful(&actual).to_bytes(),
        successful(&expected).to_bytes()
    );
    let stats = actual.stats.instances.as_ref().unwrap();
    assert_eq!(
        (
            stats.brush_models,
            stats.consumed_origin_brushes,
            actual.stats.brush_shadow_models
        ),
        (2, 2, 2)
    );
    let emitted = successful(&actual).entities().unwrap();
    assert_eq!(emitted.matches("\"angles\" \"0 0 0\"").count(), 2);
    let clear = built(&mask(&literal, "clear"), &game, cutout_options(0));
    let covered = built(&mask(&literal, "covered"), &game, cutout_options(0));
    geometry_and_probe_locations(successful(&clear), successful(&actual));
    for kind in [LightingKind::Ldr, LightingKind::Hdr] {
        for x in [-192.0, 192.0] {
            for (y, transparent) in [(64.0, true), (-96.0, false)] {
                let target = DVec3::new(x, y, 0.0);
                let lit = sample(successful(&clear), 0, target, DVec3::Z, kind, 0);
                assert!(lit.linear().into_iter().any(|value| value > 0.0));
                assert_eq!(
                    sample(successful(&covered), 0, target, DVec3::Z, kind, 0).linear(),
                    [0.0; 3]
                );
                assert_eq!(
                    sample(successful(&actual), 0, target, DVec3::Z, kind, 0),
                    if transparent {
                        lit
                    } else {
                        RgbExp32::default()
                    }
                );
            }
        }
    }
    assert_eq!(
        std::fs::read(directory.path().join("maps/grille.vmf")).unwrap(),
        child_bytes
    );
}

#[test]
fn child_cutout_refusals_and_original_publication_sources_survive_output_drop() {
    let (directory, game) = fixture();
    let child = cutout_child(directory.path());
    let mut parent = room();
    lights(&mut parent);
    let owner = point(
        &mut parent,
        "func_instance",
        "-96 0 216",
        &[("file", "grille.vmf"), ("angles", "180 0 0")],
    );
    let source = context(directory.path(), &parent);
    for request in [None, Some("0"), Some("01")] {
        let mut invalid = child.clone();
        if let Some(value) = request {
            invalid.entities[0].properties.set(FLAG, value);
        } else {
            invalid.entities[0].properties.remove(FLAG);
        }
        save(directory.path(), "grille.vmf", &invalid);
        let output = compiler::compile_with_source_controlled(
            &parent,
            &game,
            cutout_options(0),
            None,
            Some(&source),
            &Control::default(),
        );
        if request == Some("01") {
            assert!(output.report.has_errors() && output.bsp.is_none());
            assert!(
                output
                    .report
                    .diagnostics
                    .iter()
                    .any(|item| item.object == Some(format!("entity:{owner}"))
                        && item.message.contains("grille.vmf")
                        && item.message.contains("canonical vrad_brush_cast_shadows"))
            );
        } else {
            successful(&output);
            assert_eq!(output.stats.brush_shadow_models, 0);
        }
    }
    save(directory.path(), "grille.vmf", &child);
    for options in [
        options(0),
        Options {
            brush_texture_shadows: true,
            ..Default::default()
        },
    ] {
        let output = compiler::compile_with_source_controlled(
            &parent,
            &game,
            options,
            None,
            Some(&source),
            &Control::default(),
        );
        assert!(output.report.has_errors() && output.bsp.is_none());
    }
    let mut omitted = parent.clone();
    entity(&mut omitted, owner)
        .properties
        .set("origin", "1024 0 216");
    region(&mut omitted);
    let omitted_source = context(directory.path(), &omitted);
    for material in [
        "LightmappedGeneric { $basetexture test/mask $alphatest 1 $translucent 1 }",
        "LightmappedGeneric { $basetexture test/mask $alphatest 1 %compiletrigger 1 }",
        "LightmappedGeneric { $basetexture test/mask $alphatest 1 $envmap env_cubemap }",
    ] {
        std::fs::write(
            directory.path().join("materials/test/mask_base.vmt"),
            material,
        )
        .unwrap();
        let output = compiler::compile_with_source_controlled(
            &omitted,
            &game,
            cutout_options(0),
            None,
            Some(&omitted_source),
            &Control::default(),
        );
        assert!(
            output.report.has_errors() && output.bsp.is_none(),
            "{material}: {:?}",
            output.report
        );
        assert!(output.report.diagnostics.iter().any(|item| {
            item.object == Some(format!("entity:{owner}"))
                && item.message.contains("grille.vmf")
                && item
                    .message
                    .contains(&format!("solid {}", child.entities[0].solids[0].id))
        }));
    }
    cutout_assets(directory.path());
    let mut invalid = child.clone();
    invalid.entities[0]
        .properties
        .set("parentname", "moving_parent");
    save(directory.path(), "grille.vmf", &invalid);
    let source = context(directory.path(), &parent);
    let output = compiler::compile_with_source_controlled(
        &parent,
        &game,
        cutout_options(0),
        None,
        Some(&source),
        &Control::default(),
    );
    assert!(output.report.has_errors() && output.bsp.is_none());
    for mutation in ["child", "include", "texture"] {
        cutout_assets(directory.path());
        save(directory.path(), "grille.vmf", &child);
        let source = context(directory.path(), &parent);
        let destination = directory.path().join("published/child-cutout.bsp");
        std::fs::write(&destination, b"previous BSP").unwrap();
        std::fs::write(destination.with_extension("lin"), b"previous LIN").unwrap();
        let output = from_source(&parent, &game, &source, cutout_options(0));
        let publication = output
            .prepare_write_controlled(&destination, &Control::default())
            .unwrap()
            .unwrap();
        drop(output);
        match mutation {
            "child" => {
                let mut changed = child.clone();
                changed.entities[0].properties.set("angles", "0 91 0");
                save(directory.path(), "grille.vmf", &changed);
            }
            "include" => std::fs::write(
                directory.path().join("materials/test/mask_base.vmt"),
                "LightmappedGeneric { $basetexture test/mask $alphatest 1 $alpha 0.5 }",
            )
            .unwrap(),
            "texture" => std::fs::write(
                directory.path().join("materials/test/mask.vtf"),
                b"changed texture",
            )
            .unwrap(),
            _ => unreachable!(),
        }
        assert!(
            publication.commit(&Control::default()).is_err(),
            "{mutation}"
        );
        assert_eq!(std::fs::read(&destination).unwrap(), b"previous BSP");
        assert_eq!(
            std::fs::read(destination.with_extension("lin")).unwrap(),
            b"previous LIN"
        );
    }
}

#[test]
fn root_cutout_requests_keep_explicit_refusals_child_limits_and_texture_publication_guard() {
    let (directory, game) = fixture();
    cutout_assets(directory.path());
    let mut document = Document::new(room());
    let caster = model(
        &mut document,
        &[([-112.0, -112.0, 128.0], [112.0, 112.0, 136.0])],
        PIVOT,
        "test/mask",
    );
    let mut source = document.into_vmf();
    lights(&mut source);
    for options in [
        options(0),
        Options {
            brush_texture_shadows: true,
            ..Default::default()
        },
    ] {
        let output = compiler::compile_with_options(&source, &game, options);
        assert!(output.report.has_errors() && output.bsp.is_none());
    }
    let mut omitted = source.clone();
    for point in entity(&mut omitted, caster)
        .solids
        .iter_mut()
        .flat_map(|solid| &mut solid.sides)
        .flat_map(|side| &mut side.plane.0)
    {
        point.x += 1024.0;
    }
    region(&mut omitted);
    for material in [
        "LightmappedGeneric { $basetexture test/mask $alphatest 1 $translucent 1 }",
        "LightmappedGeneric { $basetexture test/mask $alphatest 1 %compilenodraw 1 }",
        "LightmappedGeneric { $basetexture test/mask $alphatest 1 $alpha 0.5 }",
        "LightmappedGeneric { $basetexture test/mask $alphatest 1 $alphatestreference NaN }",
    ] {
        std::fs::write(directory.path().join("materials/test/mask.vmt"), material).unwrap();
        let output = compiler::compile_with_options(&omitted, &game, cutout_options(0));
        assert!(
            output.report.has_errors() && output.bsp.is_none(),
            "{material}: {:?}",
            output.report
        );
        assert!(output.report.diagnostics.iter().any(|item| {
            item.message.contains("Brush global shadows:")
                && item
                    .object
                    .as_ref()
                    .is_some_and(|owner| owner.starts_with("solid:"))
        }));
    }
    cutout_assets(directory.path());
    let mut child = Document::empty().into_vmf();
    child.entities.push(
        source
            .entities
            .iter()
            .find(|entity| entity.id == caster)
            .unwrap()
            .clone(),
    );
    child.entities[0].properties.set(FLAG, "0");
    save(directory.path(), "cutout-child.vmf", &child);
    let mut parent = room();
    point(
        &mut parent,
        "func_instance",
        "0 0 0",
        &[("file", "cutout-child.vmf"), ("angles", "0 0 0")],
    );
    let child_context = context(directory.path(), &parent);
    let output = compiler::compile_with_source_controlled(
        &parent,
        &game,
        cutout_options(0),
        None,
        Some(&child_context),
        &Control::default(),
    );
    successful(&output);
    assert_eq!(output.stats.brush_shadow_models, 0);
    std::fs::remove_file(directory.path().join("materials/test/mask.vtf")).unwrap();
    let output = compiler::compile_with_options(&source, &game, cutout_options(0));
    assert!(output.report.has_errors() && output.bsp.is_none());
    cutout_assets(directory.path());
    let source_context = context(directory.path(), &source);
    let destination = directory.path().join("published/cutout.bsp");
    std::fs::write(&destination, b"previous BSP").unwrap();
    std::fs::write(destination.with_extension("lin"), b"previous LIN").unwrap();
    let output = from_source(&source, &game, &source_context, cutout_options(0));
    let publication = output
        .prepare_write_controlled(&destination, &Control::default())
        .unwrap()
        .unwrap();
    drop(output);
    std::fs::write(
        directory.path().join("materials/test/mask.vtf"),
        b"changed texture",
    )
    .unwrap();
    assert!(publication.commit(&Control::default()).is_err());
    assert_eq!(std::fs::read(&destination).unwrap(), b"previous BSP");
    assert_eq!(
        std::fs::read(destination.with_extension("lin")).unwrap(),
        b"previous LIN"
    );
}

#[test]
fn translated_child_matches_complete_literal_bsp_and_retains_original_publication_sources() {
    let (directory, game) = fixture();
    let mut leaf = Document::empty();
    model(
        &mut leaf,
        &[([-64.0, -64.0, 96.0], [64.0, 64.0, 112.0])],
        DVec3::new(16.0, 8.0, 4.0),
        "test/model",
    );
    let leaf = leaf.into_vmf();
    save(directory.path(), "leaf.vmf", &leaf);
    let mut base = room();
    lights(&mut base);
    let mut parent = base.clone();
    let owner = point(
        &mut parent,
        "func_instance",
        "96 -48 16",
        &[("file", "leaf.vmf"), ("angles", "0 0 0")],
    );
    let source = context(directory.path(), &parent);
    let mut report = compiler::Report::default();
    let prepared = instances::resolve_for_compile(
        &parent,
        Some(&source),
        &game,
        &Control::default(),
        &mut report,
    )
    .unwrap();
    assert!(!report.has_errors(), "{report:?}");
    let generated = prepared
        .compilation_vmf()
        .unwrap()
        .entities
        .iter()
        .find(|item| prepared.root_entity(item.id) == Some(owner))
        .unwrap();
    let mut literal = Document::new(base);
    let id = model(
        &mut literal,
        &[([32.0, -112.0, 112.0], [160.0, 16.0, 128.0])],
        DVec3::new(112.0, -40.0, 20.0),
        "test/model",
    );
    let mut literal = literal.into_vmf();
    let authored = entity(&mut literal, id);
    authored.id = generated.id;
    assert_eq!(generated.solids.len(), 1);
    assert_eq!(authored.solids.len(), generated.solids.len());
    for (literal, actual) in authored.solids.iter_mut().zip(&generated.solids) {
        literal.id = actual.id;
        assert_eq!(literal.sides.len(), actual.sides.len());
        for (literal, actual) in literal.sides.iter_mut().zip(&actual.sides) {
            literal.id = actual.id;
            for axis in [&mut literal.u_axis, &mut literal.v_axis] {
                axis.offset -= axis.axis.dot(DVec3::new(96.0, -48.0, 16.0)) / axis.scale;
            }
        }
    }
    let actual = from_source(&parent, &game, &source, options(1));
    let expected = built(&literal, &game, options(1));
    assert_eq!(actual.stats.brush_shadow_models, 1);
    assert_eq!(
        successful(&actual).to_bytes(),
        successful(&expected).to_bytes()
    );
    drop(prepared);

    for change in ["child", "included_material", "omitted_root_material"] {
        save(directory.path(), "leaf.vmf", &leaf);
        std::fs::write(
            directory.path().join("materials/test/model_base.vmt"),
            WHITE,
        )
        .unwrap();
        let mut root = parent.clone();
        if change == "omitted_root_material" {
            root = literal.clone();
            let caster = root
                .entities
                .iter_mut()
                .find(|item| !item.solids.is_empty())
                .unwrap();
            caster.properties.set("origin", "624 -40 20");
            for point in caster
                .solids
                .iter_mut()
                .flat_map(|solid| &mut solid.sides)
                .flat_map(|side| &mut side.plane.0)
            {
                point.x += 512.0;
            }
            region(&mut root);
        }
        let source = context(directory.path(), &root);
        let destination = directory.path().join("published/previous.bsp");
        std::fs::write(&destination, b"previous BSP").unwrap();
        std::fs::write(destination.with_extension("lin"), b"previous LIN").unwrap();
        let output = from_source(&root, &game, &source, Options::default());
        let publication = output
            .prepare_write_controlled(&destination, &Control::default())
            .unwrap()
            .unwrap();
        drop(output);
        if change == "child" {
            let mut changed = leaf.clone();
            changed.entities[0].properties.set("origin", "17 8 4");
            save(directory.path(), "leaf.vmf", &changed);
        } else {
            std::fs::write(
                directory.path().join("materials/test/model_base.vmt"),
                "LightmappedGeneric { $alphatest 1 }",
            )
            .unwrap();
        }
        assert!(publication.commit(&Control::default()).is_err(), "{change}");
        assert_eq!(std::fs::read(&destination).unwrap(), b"previous BSP");
        assert_eq!(
            std::fs::read(destination.with_extension("lin")).unwrap(),
            b"previous LIN"
        );
    }
}
