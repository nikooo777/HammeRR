mod common;
#[path = "common/static_prop_model.rs"]
mod static_prop_model;

use std::path::Path;

use compiler::instances::SourceContext;
use compiler::{Control, LightingMode, Options, OutputLease, SkyQuality, WriteReservation};
use glam::{DMat3, DVec3};
use map_core::{BrushGeometry, Document, ObjectId, Plane, edit};
use source_formats::bsp::{self, Bsp, LeafAmbient, LightingKind, Model, RgbExp32, TexInfo};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::keyvalues::Node;
use source_formats::vmf::{DispInfo, Entity, UvAxis, Vmf, format_vec3};

const SHADOWS: &str = "vrad_brush_cast_shadows";

struct Fixture {
    directory: tempfile::TempDir,
    game: GameFs,
    source: Vmf,
}

fn fixture(receivers: bool) -> Fixture {
    let root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/validation/material-emission");
    std::fs::create_dir_all(&root).unwrap();
    let directory = tempfile::tempdir_in(root).unwrap();
    for folder in [
        "game/materials/dev",
        "game/materials/test",
        "game/materials/tools",
        "game/materials/models/test",
        "game/materials/models/detl",
        "game/models/props",
        "game/scripts",
        "maps",
        "outputs",
    ] {
        std::fs::create_dir_all(directory.path().join(folder)).unwrap();
    }
    for (name, text) in [
        (
            "dev/dev_measuregeneric01",
            "LightmappedGeneric { %notooltexture 1 $reflectivity \"[0.5 0.5 0.5]\" }",
        ),
        (
            "test/emitter",
            "LightmappedGeneric { %notooltexture 1 $reflectivity \"[0 0 0]\" }",
        ),
        (
            "test/opaque",
            "UnlitGeneric { %notooltexture 1 $reflectivity \"[0 0 0]\" }",
        ),
        ("test/pivot", "UnlitGeneric { %compileorigin 1 }"),
        (
            "tools/toolsblack",
            "UnlitGeneric { %notooltexture 1 $color \"[0 0 0]\" $reflectivity \"[0 0 0]\" }",
        ),
        (
            "models/test/one",
            "VertexLitGeneric { $basetexture test/white }",
        ),
        (
            "models/test/two",
            "VertexLitGeneric { $basetexture test/white }",
        ),
    ] {
        std::fs::write(
            directory.path().join(format!("game/materials/{name}.vmt")),
            text,
        )
        .unwrap();
    }
    for name in ["one", "two"] {
        std::fs::write(
            directory
                .path()
                .join(format!("game/materials/models/detl/{name}.vmt")),
            "UnlitGeneric { $basetexture test/white }",
        )
        .unwrap();
    }
    std::fs::write(
        directory.path().join("game/scripts/emission.rad"),
        "ldr: test/emitter 255 0 0 255\nhdr: test/emitter 0 0 255 510\n",
    )
    .unwrap();
    for (extension, data) in ["mdl", "vvd", "dx90.vtx"]
        .into_iter()
        .zip(static_prop_model::model())
    {
        std::fs::write(
            directory
                .path()
                .join(format!("game/models/props/test.{extension}")),
            &data,
        )
        .unwrap();
        let mut detail = data;
        if extension == "mdl" {
            detail[752..764].copy_from_slice(b"models/detl/");
        }
        std::fs::write(
            directory
                .path()
                .join(format!("game/models/props/detail.{extension}")),
            detail,
        )
        .unwrap();
    }
    let mut source = common::room();
    for side in source
        .world
        .solids
        .iter_mut()
        .flat_map(|solid| &mut solid.sides)
    {
        side.lightmap_scale = 128;
        let plane = Plane::from_points(side.plane.0).unwrap();
        if plane.normal.z < -0.99 && (plane.dist + 256.0).abs() < 0.001 {
            side.material = "test/emitter".into();
        }
    }
    source
        .world
        .properties
        .set(compiler::MATERIAL_EMISSION_FILE_KEY, "scripts/emission.rad");
    let mut document = Document::new(source);
    if receivers {
        document
            .edit("receivers", |tx| {
                let ObjectId::Solid(id) = edit::create_block(
                    tx,
                    DVec3::new(-16.0, -16.0, 32.0),
                    DVec3::new(16.0, 16.0, 64.0),
                    "dev/dev_measuregeneric01",
                )?
                else {
                    unreachable!()
                };
                for side in &mut tx.solid_mut(id)?.sides {
                    side.lightmap_scale = 128;
                }
                edit::tie_to_entity(
                    tx,
                    &[id],
                    "func_brush",
                    &[("origin".into(), "0 0 32".into())],
                )?;
                let ObjectId::Solid(id) = edit::create_block(
                    tx,
                    DVec3::new(-160.0, 32.0, 16.0),
                    DVec3::new(-32.0, 160.0, 32.0),
                    "dev/dev_measuregeneric01",
                )?
                else {
                    unreachable!()
                };
                let solid = tx.solid_mut(id)?;
                for side in &mut solid.sides {
                    side.lightmap_scale = 128;
                }
                let geometry = BrushGeometry::from_solid(solid).unwrap();
                let top = geometry
                    .faces
                    .iter()
                    .find(|face| face.plane.normal.z > 0.99)
                    .unwrap();
                solid.sides[top.side].displacement = Some(DispInfo {
                    power: 2,
                    start_position: top.vertices[0],
                    flags: Some(0),
                    elevation: 0.0,
                    subdivided: false,
                    normals: Some(vec![vec![DVec3::Z; 5]; 5]),
                    distances: Some(vec![vec![0.0; 5]; 5]),
                    offsets: None,
                    offset_normals: None,
                    alphas: None,
                    triangle_tags: None,
                    allowed_verts: None,
                    extra_properties: Default::default(),
                    extra: Vec::new(),
                });
                for (class, origin, model) in [
                    ("prop_static", DVec3::new(96.0, 96.0, 48.0), "test"),
                    ("prop_detail", DVec3::new(-96.0, -96.0, 48.0), "detail"),
                ] {
                    edit::create_entity(
                        tx,
                        class,
                        Some(origin),
                        &[
                            ("model".into(), format!("models/props/{model}.mdl")),
                            ("solid".into(), "0".into()),
                        ],
                        Vec::new(),
                    )?;
                }
                Ok(())
            })
            .unwrap();
    }
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().join("game")),
    });
    Fixture {
        directory,
        game,
        source: document.into_vmf(),
    }
}

fn options(bounces: u8, receivers: bool) -> Options {
    Options {
        lighting: LightingMode::DirectLdr,
        bounces,
        bake_static_prop_vertices: receivers,
        ..Default::default()
    }
}

fn compile(fixture: &Fixture, source: &Vmf, options: Options) -> compiler::Output {
    let output = compiler::compile_with_options(source, &fixture.game, options);
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.bsp.is_some());
    output
}

fn energy(bsp: &Bsp, kind: LightingKind, model: Option<usize>, terrain: bool) -> DVec3 {
    let models = bsp.records::<Model>().unwrap();
    let faces = bsp.lighting_faces(kind).unwrap();
    let infos = bsp.records::<TexInfo>().unwrap();
    let lighting = bsp.lighting(kind).unwrap().unwrap();
    let range = model
        .map(|index| {
            let item = models[index];
            item.first_face as usize..(item.first_face + item.face_count) as usize
        })
        .unwrap_or(0..faces.len());
    let mut sum = DVec3::ZERO;
    for face in &faces[range] {
        if terrain && face.dispinfo < 0 {
            continue;
        }
        if let Some(light) = lighting.face(face, &infos[face.texinfo as usize]).unwrap() {
            for color in light.samples(0, 0).unwrap() {
                sum += DVec3::from_array(color.linear());
            }
        }
    }
    sum
}

fn interior_source(fixture: &Fixture) -> Document {
    let mut source = fixture.source.clone();
    for side in source
        .world
        .solids
        .iter_mut()
        .flat_map(|solid| &mut solid.sides)
    {
        if side.material == "test/emitter" {
            side.material = "dev/dev_measuregeneric01".into();
        }
    }
    Document::new(source)
}

fn unlit_emitter(fixture: &Fixture) {
    std::fs::write(
        fixture
            .directory
            .path()
            .join("game/materials/test/emitter.vmt"),
        "UnlitGeneric { %notooltexture 1 $reflectivity \"[0 0 0]\" }",
    )
    .unwrap();
}

fn emitter(document: &mut Document, mins: DVec3, maxs: DVec3, pivot: DVec3, angles: &str) -> u32 {
    document
        .edit("interior emission source", |tx| {
            let ObjectId::Solid(id) = edit::create_block(tx, mins, maxs, "test/opaque")? else {
                unreachable!()
            };
            for side in &mut tx.solid_mut(id)?.sides {
                side.lightmap_scale = 128;
                if Plane::from_points(side.plane.0).unwrap().normal == DVec3::NEG_Z {
                    side.material = "test/emitter".into();
                }
            }
            let ObjectId::Entity(id) = edit::tie_to_entity(
                tx,
                &[id],
                "func_brush",
                &[
                    ("origin".into(), format_vec3(pivot)),
                    ("angles".into(), angles.into()),
                    ("spawnflags".into(), "2".into()),
                    ("Solidity".into(), "0".into()),
                    ("StartDisabled".into(), "0".into()),
                    (SHADOWS.into(), "1".into()),
                ],
            )?
            else {
                unreachable!()
            };
            Ok(id)
        })
        .unwrap()
}

fn black_light(document: &mut Document) {
    document
        .edit("authored black light", |tx| {
            edit::create_entity(
                tx,
                "light",
                Some(DVec3::Z * 128.0),
                &[
                    ("_light".into(), "0 0 0 0".into()),
                    ("_lightHDR".into(), "0 0 0 0".into()),
                ],
                Vec::new(),
            )
        })
        .unwrap();
}

fn floor_sample(bsp: &Bsp, kind: LightingKind, target: DVec3) -> RgbExp32 {
    let model = bsp.records::<Model>().unwrap()[0];
    let faces = bsp.lighting_faces(kind).unwrap();
    let planes = bsp.records::<bsp::Plane>().unwrap();
    let infos = bsp.records::<TexInfo>().unwrap();
    let vertices = bsp.vertices().unwrap();
    let edges = bsp.records::<bsp::Edge>().unwrap();
    let surfedges = bsp.surfedges().unwrap();
    let lighting = bsp.lighting(kind).unwrap().unwrap();
    'faces: for face in
        &faces[model.first_face as usize..(model.first_face + model.face_count) as usize]
    {
        let plane = planes[face.plane as usize];
        let normal = DVec3::from_array(plane.normal.map(f64::from));
        if normal != DVec3::Z || plane.dist != 0.0 {
            continue;
        }
        let mut signs = [false; 2];
        for &edge in &surfedges
            [face.first_edge as usize..face.first_edge as usize + face.edge_count as usize]
        {
            let pair = edges[edge.unsigned_abs() as usize].0;
            let a =
                DVec3::from_array(vertices[pair[usize::from(edge < 0)] as usize].map(f64::from));
            let b =
                DVec3::from_array(vertices[pair[usize::from(edge >= 0)] as usize].map(f64::from));
            let cross = (b - a).cross(target - a).z;
            signs[0] |= cross > 1e-6;
            signs[1] |= cross < -1e-6;
            if signs == [true; 2] {
                continue 'faces;
            }
        }
        let info = &infos[face.texinfo as usize];
        let frame = compiler::lighting::LuxelFrame::new(
            Plane { normal, dist: 0.0 },
            info,
            face.lightmap_mins,
            face.lightmap_size,
        )
        .unwrap();
        let [width, height] = frame.dimensions();
        for t in 0..height {
            for s in 0..width {
                if frame.position(s, t).unwrap().distance(target) < 1e-6 {
                    return lighting
                        .face(face, info)
                        .unwrap()
                        .unwrap()
                        .samples(0, 0)
                        .unwrap()[t * width + s];
                }
            }
        }
    }
    panic!("missing fixed floor sample {target:?}")
}

fn assert_floor_emission_matches(actual: &Bsp, expected: &Bsp) {
    for (kind, channel) in [(LightingKind::Ldr, 0), (LightingKind::Hdr, 2)] {
        for x in [-128.0, 0.0, 128.0] {
            for y in [-128.0, 0.0, 128.0] {
                let target = DVec3::new(x, y, 0.0);
                let actual = floor_sample(actual, kind, target);
                let expected = floor_sample(expected, kind, target);
                assert!(actual.rgb[channel] > 0 && expected.rgb[channel] > 0);
                for other in (0..3).filter(|&other| other != channel) {
                    assert_eq!(actual.rgb[other], 0);
                    assert_eq!(expected.rgb[other], 0);
                }
                let native_step = 2.0_f64.powi(i32::from(actual.exponent.max(expected.exponent)));
                assert!(
                    (actual.linear()[channel] - expected.linear()[channel]).abs() <= native_step,
                    "{kind:?} {target:?}: {actual:?} != {expected:?}"
                );
            }
        }
    }
}

fn instance(source: &mut Vmf, file: &str, origin: &str, angles: &str) {
    let id = source
        .object_ids()
        .chain(source.side_ids())
        .max()
        .unwrap_or(0)
        + 1;
    let mut entity = Entity::new(id, "func_instance");
    for (key, value) in [("file", file), ("origin", origin), ("angles", angles)] {
        entity.properties.set(key, value);
    }
    source.entities.push(entity);
}

fn stationary_options() -> Options {
    Options {
        lighting: LightingMode::DirectLdrHdr,
        sky_quality: SkyQuality::Fast,
        ..options(0, false)
    }
}

fn emission_texture(fixture: &Fixture, pixels: [[u8; 4]; 4], flags: u32) -> Vec<u8> {
    let mut bytes = vec![0_u8; 80];
    bytes[..4].copy_from_slice(b"VTF\0");
    for (offset, value) in [
        (4, 7_u32),
        (8, 2),
        (12, 80),
        (20, flags | source_formats::vtf::FLAG_NOMIP),
    ] {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    for (offset, value) in [(16, 2_u16), (18, 2), (24, 1), (63, 1)] {
        bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }
    bytes[36..40].copy_from_slice(&1.0_f32.to_le_bytes());
    bytes[56] = 1;
    bytes[57..61].copy_from_slice(&(-1_i32).to_le_bytes());
    for pixel in pixels {
        bytes.extend_from_slice(&pixel);
    }
    std::fs::write(
        fixture
            .directory
            .path()
            .join("game/materials/test/emission.vtf"),
        &bytes,
    )
    .unwrap();
    std::fs::write(
        fixture
            .directory
            .path()
            .join("game/materials/test/emitter.vmt"),
        "UnlitGeneric { %notooltexture 1 $basetexture test/emission $reflectivity \"[0 0 0]\" }",
    )
    .unwrap();
    bytes
}

fn emission_uv(source: &mut Vmf, u: UvAxis) {
    for side in source
        .world
        .solids
        .iter_mut()
        .chain(
            source
                .entities
                .iter_mut()
                .flat_map(|entity| &mut entity.solids),
        )
        .flat_map(|solid| &mut solid.sides)
        .filter(|side| side.material == "test/emitter")
    {
        side.u_axis = u;
        side.v_axis = UvAxis {
            axis: DVec3::Y,
            offset: 1.0,
            scale: 48.0,
        };
    }
}

fn assert_emission_flux(output: &compiler::Output, red_area: f64, blue_area: f64) {
    let stats = output.stats.material_emission.as_ref().unwrap();
    assert_eq!(stats.source_area, 128.0 * 96.0);
    assert_eq!((stats.source_faces, stats.source_triangles), (1, 2));
    assert_eq!(
        (stats.textured_materials, stats.textured_source_faces),
        (1, 1)
    );
    assert_eq!(stats.texture_rectangles, 2);
    let expected = [
        [
            std::f64::consts::PI * red_area * 0.2195197180748679,
            0.0,
            0.0,
        ],
        [0.0, 0.0, std::f64::consts::PI * blue_area * 2.0],
    ];
    for (actual, expected) in stats
        .channel_flux
        .iter()
        .flatten()
        .zip(expected.iter().flatten())
    {
        let rounding = 128.0 * f64::EPSILON * expected.abs().max(1.0);
        assert!(
            (actual - expected).abs() <= rounding,
            "{actual} != {expected}"
        );
    }
}

#[test]
fn texture_emission_integrates_exact_color_areas_and_keeps_native_uv_phase() {
    let fixture = fixture(false);
    let pixels = [
        [128, 0, 0, 255],
        [0, 0, 255, 255],
        [128, 0, 0, 255],
        [0, 0, 255, 255],
    ];
    emission_texture(&fixture, pixels, 0);
    std::fs::write(
        fixture.directory.path().join("game/scripts/emission.rad"),
        "ldr: texture: test/emitter 255 0 0 255\nhdr: texture: test/emitter 0 0 255 510\n",
    )
    .unwrap();
    let mut document = interior_source(&fixture);
    black_light(&mut document);
    let id = emitter(
        &mut document,
        DVec3::new(-64.0, -48.0, 176.0),
        DVec3::new(64.0, 48.0, 184.0),
        DVec3::new(16.0, -8.0, 160.0),
        "0 0 0",
    );
    let mut source = document.into_vmf();
    emission_uv(
        &mut source,
        UvAxis {
            axis: DVec3::X,
            offset: 1.0,
            scale: 64.0,
        },
    );
    let mut world = Document::new(source.clone());
    world
        .edit("world textured source", |tx| edit::move_to_world(tx, id))
        .unwrap();
    let world = compile(&fixture, world.vmf(), stationary_options());
    let mut posed = source.clone();
    posed
        .entities
        .iter_mut()
        .find(|entity| entity.id == id)
        .unwrap()
        .properties
        .set("angles", "0 90 0");
    let posed = compile(&fixture, &posed, stationary_options());
    for (output, axis) in [(&world, DVec3::X), (&posed, DVec3::Y)] {
        assert_emission_flux(output, 64.0 * 96.0, 64.0 * 96.0);
        let bsp = output.bsp.as_ref().unwrap();
        for (kind, channel) in [(LightingKind::Ldr, 0), (LightingKind::Hdr, 2)] {
            let low = floor_sample(bsp, kind, axis * -128.0);
            let high = floor_sample(bsp, kind, axis * 128.0);
            if kind == LightingKind::Ldr {
                assert!(low.linear()[channel] > high.linear()[channel]);
            } else {
                assert!(high.linear()[channel] > low.linear()[channel]);
            }
            assert!(low.rgb[channel] > 0 && high.rgb[channel] > 0);
            for other in (0..3).filter(|&other| other != channel) {
                assert_eq!((low.rgb[other], high.rgb[other]), (0, 0));
            }
        }
    }
    for (name, axis, scale, offset, flags, red_area) in [
        ("negative repeat", DVec3::X, 64.0, -0.5, 0, 64.0 * 96.0),
        ("partial repeat", DVec3::X, 128.0, 0.25, 0, 96.0 * 96.0),
        ("lower clamp", DVec3::X, 64.0, -0.5, 0x0c, 128.0 * 96.0),
        ("upper clamp", DVec3::X, 64.0, 2.5, 0x0c, 0.0),
        ("constant edge", DVec3::Z, 128.0, -0.375, 0, 0.0),
    ] {
        emission_texture(&fixture, pixels, flags);
        let mut source = source.clone();
        emission_uv(
            &mut source,
            UvAxis {
                axis,
                offset,
                scale,
            },
        );
        let output = compiler::compile_with_options(&source, &fixture.game, stationary_options());
        assert!(!output.report.has_errors(), "{name}: {:?}", output.report);
        assert!(output.bsp.is_some(), "{name}");
        assert_emission_flux(&output, red_area, 128.0 * 96.0 - red_area);
    }
}

#[test]
fn constant_white_texture_is_bit_exact_uniform_emission_and_black_is_valid_zero() {
    let fixture = fixture(true);
    emission_texture(&fixture, [[255; 4]; 4], 0);
    let mut document = Document::new(fixture.source.clone());
    black_light(&mut document);
    let mut source = document.into_vmf();
    emission_uv(
        &mut source,
        UvAxis {
            axis: DVec3::X,
            offset: 1.0,
            scale: 256.0,
        },
    );
    for side in source
        .world
        .solids
        .iter_mut()
        .flat_map(|solid| &mut solid.sides)
    {
        if side.material == "test/emitter" {
            side.v_axis.scale = 256.0;
        }
    }
    let settings = Options {
        bake_static_prop_vertices: true,
        ..stationary_options()
    };
    let uniform = compile(&fixture, &source, settings);
    std::fs::write(
        fixture.directory.path().join("game/scripts/emission.rad"),
        "ldr: texture: test/emitter 255 0 0 255\nhdr: texture: test/emitter 0 0 255 510\n",
    )
    .unwrap();
    let white = compile(&fixture, &source, settings);
    let uniform_stats = uniform.stats.material_emission.as_ref().unwrap();
    let white_stats = white.stats.material_emission.as_ref().unwrap();
    assert_eq!(uniform_stats.channel_flux, white_stats.channel_flux);
    assert_eq!(uniform_stats.source_area, white_stats.source_area);
    assert_eq!(white_stats.texture_rectangles, 1);
    assert_eq!(
        white_stats.integration_triangles,
        uniform_stats.source_triangles
    );
    let mut off = source.clone();
    off.world
        .properties
        .remove(compiler::MATERIAL_EMISSION_FILE_KEY);
    let off = compile(&fixture, &off, settings);
    emission_texture(&fixture, [[0, 0, 0, 255]; 4], 0);
    let black = compile(&fixture, &source, settings);
    let black_stats = black.stats.material_emission.as_ref().unwrap();
    assert_eq!(black_stats.source_area, uniform_stats.source_area);
    assert_eq!(black_stats.channel_flux, [[0.0; 3]; 2]);
    assert_eq!(
        (black_stats.source_faces, black_stats.textured_source_faces),
        (1, 1)
    );
    for (actual, expected) in [(&white, &uniform), (&black, &off)] {
        let actual = actual.bsp.as_ref().unwrap();
        let expected = expected.bsp.as_ref().unwrap();
        for lump in [
            bsp::lump::LIGHTING,
            bsp::lump::LIGHTING_HDR,
            bsp::lump::LEAF_AMBIENT_INDEX,
            bsp::lump::LEAF_AMBIENT_INDEX_HDR,
            bsp::lump::LEAF_AMBIENT_LIGHTING,
            bsp::lump::LEAF_AMBIENT_LIGHTING_HDR,
            bsp::lump::WORLDLIGHTS,
            bsp::lump::WORLDLIGHTS_HDR,
        ] {
            assert_eq!(
                actual.lumps[lump].data, expected.lumps[lump].data,
                "lump {lump}"
            );
        }
        assert_eq!(
            actual.detail_props().unwrap(),
            expected.detail_props().unwrap()
        );
        let actual = actual.pak_data(bsp::PakLimits::default()).unwrap();
        let expected = expected.pak_data(bsp::PakLimits::default()).unwrap();
        let actual = actual
            .entries
            .iter()
            .filter(|entry| entry.name.ends_with(".vhv"));
        let expected = expected
            .entries
            .iter()
            .filter(|entry| entry.name.ends_with(".vhv"));
        let actual = actual
            .map(|entry| (&entry.name, &entry.data))
            .collect::<Vec<_>>();
        let expected = expected
            .map(|entry| (&entry.name, &entry.data))
            .collect::<Vec<_>>();
        assert!(!actual.is_empty());
        assert_eq!(actual, expected);
    }
}

#[test]
fn texture_assets_refuse_before_cordon_and_retain_original_publication_guards() {
    let fixture = fixture(false);
    let bytes = emission_texture(&fixture, [[255; 4]; 4], 0);
    std::fs::write(
        fixture.directory.path().join("game/scripts/emission.rad"),
        "texture: test/emitter 255 255 255 255\n",
    )
    .unwrap();
    let mut document = interior_source(&fixture);
    emitter(
        &mut document,
        DVec3::new(512.0, -32.0, 64.0),
        DVec3::new(576.0, 32.0, 80.0),
        DVec3::new(544.0, 0.0, 72.0),
        "0 0 0",
    );
    let mut source = document.into_vmf();
    source.cordon = Some(Node::block(
        "cordon",
        vec![
            Node::string("active", "1"),
            Node::string("mins", "-128 -128 4"),
            Node::string("maxs", "128 128 252"),
        ],
    ));
    let accepted = compile(&fixture, &source, Options::default());
    assert!(accepted.stats.compile_region.is_some());
    assert_eq!(
        accepted
            .stats
            .material_emission
            .as_ref()
            .unwrap()
            .matched_sides,
        1
    );
    let destination = fixture.directory.path().join("outputs/texture-refused.bsp");
    std::fs::write(&destination, b"previous BSP").unwrap();
    std::fs::write(destination.with_extension("lin"), b"previous LIN").unwrap();
    let publication = accepted
        .prepare_write_controlled(&destination, &Control::default())
        .unwrap()
        .unwrap();
    drop(accepted);
    let texture = fixture
        .directory
        .path()
        .join("game/materials/test/emission.vtf");
    let mut changed = bytes.clone();
    changed[80] ^= 1;
    std::fs::write(&texture, changed).unwrap();
    assert!(publication.commit(&Control::default()).is_err());
    assert_eq!(std::fs::read(&destination).unwrap(), b"previous BSP");
    assert_eq!(
        std::fs::read(destination.with_extension("lin")).unwrap(),
        b"previous LIN"
    );
    let mut animated = bytes.clone();
    animated[24..26].copy_from_slice(&2_u16.to_le_bytes());
    animated.extend_from_slice(&bytes[80..]);
    let mut hdr = bytes[..80].to_vec();
    hdr[52..56].copy_from_slice(&24_u32.to_le_bytes());
    hdr.extend_from_slice(&[0; 32]);
    for (name, replacement) in [
        ("missing", None),
        ("animated", Some(animated)),
        ("HDR", Some(hdr)),
    ] {
        if let Some(replacement) = replacement {
            std::fs::write(&texture, replacement).unwrap();
        } else {
            std::fs::remove_file(&texture).unwrap();
        }
        let output = compiler::compile_with_options(&source, &fixture.game, Options::default());
        assert!(
            output.report.has_errors() && output.bsp.is_none() && output.leak.is_none(),
            "{name}: {:?}",
            output.report
        );
        assert!(output.stats.compile_region.is_none(), "{name}");
        assert!(!output.write(&destination).unwrap(), "{name}");
        assert_eq!(std::fs::read(&destination).unwrap(), b"previous BSP");
        assert_eq!(
            std::fs::read(destination.with_extension("lin")).unwrap(),
            b"previous LIN"
        );
    }
}

#[test]
fn emission_only_ldr_reaches_real_receivers_and_probes_without_worldlights() {
    let fixture = fixture(true);
    let mut off = fixture.source.clone();
    off.world
        .properties
        .remove(compiler::MATERIAL_EMISSION_FILE_KEY);
    let baseline = compile(&fixture, &off, options(0, true));
    let enabled = compile(&fixture, &fixture.source, options(0, true));
    let dark = baseline.bsp.as_ref().unwrap();
    let lit = enabled.bsp.as_ref().unwrap();
    assert_eq!(
        dark.lumps[bsp::lump::WORLDLIGHTS].data,
        lit.lumps[bsp::lump::WORLDLIGHTS].data
    );
    assert!(lit.world_lights(LightingKind::Ldr).unwrap().is_empty());
    assert_eq!(energy(dark, LightingKind::Ldr, None, false), DVec3::ZERO);
    for (model, terrain) in [(Some(0), false), (Some(1), false), (Some(0), true)] {
        let value = energy(lit, LightingKind::Ldr, model, terrain);
        assert!(value.x > 0.0, "{model:?} terrain {terrain}: {value:?}");
        assert_eq!(value.y, 0.0);
        assert_eq!(value.z, 0.0);
    }
    assert!(
        lit.detail_props().unwrap().unwrap().props[0]
            .lighting
            .linear()[0]
            > 0.0
    );
    let ambient = lit.leaf_ambient(LightingKind::Ldr).unwrap().unwrap();
    let LeafAmbient::Indexed { samples, .. } = ambient else {
        panic!("Expected indexed ambient")
    };
    assert!(
        samples
            .iter()
            .flat_map(|sample| sample.cube)
            .any(|color| color.linear()[0] > 0.0)
    );
    let manifest =
        compiler::prop_lighting_assets::read_generated_prop_lighting(lit, &Control::default())
            .unwrap()
            .unwrap();
    let archive = lit.pak_data(bsp::PakLimits::default()).unwrap();
    let asset = &manifest.assets[0];
    let bytes = &archive
        .entries
        .iter()
        .find(|entry| entry.name == asset.path)
        .unwrap()
        .data;
    let vertices =
        source_formats::vhv::File::parse(bytes, source_formats::vhv::Limits::default()).unwrap();
    assert!(
        vertices
            .meshes
            .iter()
            .flat_map(|mesh| &mesh.colors)
            .any(|color| color.red > 0)
    );
    let stats = enabled.stats.material_emission.as_ref().unwrap();
    assert!(stats.source_faces > 0 && stats.source_triangles > 0 && stats.channel_rays[0] > 0);
    assert_eq!(stats.channel_rays[1], 0);
    assert!(
        !lit.entities()
            .unwrap()
            .contains(compiler::MATERIAL_EMISSION_FILE_KEY)
    );
    let mut previous_energy = energy(lit, LightingKind::Ldr, None, false).x;
    for bounces in [1, 4] {
        let bounced = compile(&fixture, &fixture.source, options(bounces, true));
        let bounced_energy = energy(
            bounced.bsp.as_ref().unwrap(),
            LightingKind::Ldr,
            None,
            false,
        )
        .x;
        assert!(bounced_energy > previous_energy);
        previous_energy = bounced_energy;
        assert!(
            bounced
                .bsp
                .as_ref()
                .unwrap()
                .world_lights(LightingKind::Ldr)
                .unwrap()
                .is_empty()
        );
    }
}

#[test]
fn hdr_channels_require_a_real_native_source_and_keep_authored_records() {
    let fixture = fixture(false);
    let mut settings = options(0, false);
    settings.lighting = LightingMode::DirectLdrHdr;
    let refused = compiler::compile_with_options(&fixture.source, &fixture.game, settings);
    assert!(refused.report.has_errors());
    assert!(
        refused
            .report
            .diagnostics
            .iter()
            .any(|item| item.message.contains("real authored"))
    );
    let mut document = Document::new(fixture.source.clone());
    black_light(&mut document);
    let mut off = document.vmf().clone();
    off.world
        .properties
        .remove(compiler::MATERIAL_EMISSION_FILE_KEY);
    let baseline = compile(&fixture, &off, settings);
    let output = compile(&fixture, document.vmf(), settings);
    let bsp = output.bsp.as_ref().unwrap();
    let ldr = energy(bsp, LightingKind::Ldr, None, false);
    let hdr = energy(bsp, LightingKind::Hdr, None, false);
    assert!(ldr.x > 0.0 && ldr.z == 0.0 && hdr.x == 0.0 && hdr.z > ldr.x);
    for kind in [LightingKind::Ldr, LightingKind::Hdr] {
        assert_eq!(
            baseline.bsp.as_ref().unwrap().world_lights(kind).unwrap(),
            bsp.world_lights(kind).unwrap()
        );
        assert_eq!(bsp.world_lights(kind).unwrap().len(), 1);
    }
    let mut padded = document.vmf().clone();
    for side in padded
        .world
        .solids
        .iter_mut()
        .flat_map(|solid| &mut solid.sides)
    {
        if side.material == "test/emitter" {
            side.material = " test/emitter ".into();
        }
    }
    let padded = compile(&fixture, &padded, settings);
    for lump in [
        bsp::lump::LIGHTING,
        bsp::lump::LIGHTING_HDR,
        bsp::lump::LEAF_AMBIENT_LIGHTING,
        bsp::lump::LEAF_AMBIENT_LIGHTING_HDR,
    ] {
        assert_eq!(
            padded.bsp.as_ref().unwrap().lumps[lump].data,
            bsp.lumps[lump].data
        );
    }
}

#[test]
fn original_emission_file_guards_publication_and_work_refusal() {
    let fixture = fixture(false);
    let destination = fixture.directory.path().join("outputs/previous.bsp");
    std::fs::write(&destination, b"previous").unwrap();
    let control = Control::default();
    let _lease = OutputLease::acquire(&destination, &control).unwrap();
    let reservation = WriteReservation::new(&destination, &control).unwrap();
    let output = compile(&fixture, &fixture.source, options(0, false));
    std::fs::write(
        fixture.directory.path().join("game/scripts/emission.rad"),
        "test/emitter 0 255 0 200\n",
    )
    .unwrap();
    assert!(
        output
            .prepare_write_reserved(reservation, &control)
            .is_err()
    );
    assert_eq!(std::fs::read(&destination).unwrap(), b"previous");
    let mut settings = options(0, false);
    settings.lighting_work_limit = 1;
    let failed = compiler::compile_with_options(&fixture.source, &fixture.game, settings);
    assert!(failed.report.has_errors() && failed.bsp.is_none());
    let mut invalid = fixture.source.clone();
    invalid.world.solids[1]
        .sides
        .iter_mut()
        .find(|side| side.material == "test/emitter")
        .unwrap()
        .material = "test/missing".into();
    std::fs::write(
        fixture.directory.path().join("game/scripts/emission.rad"),
        "test/missing 255 0 0 100\n",
    )
    .unwrap();
    let failed = compiler::compile_with_options(&invalid, &fixture.game, options(0, false));
    assert!(failed.report.has_errors() && failed.bsp.is_none());
}

#[test]
fn rotated_unlit_stationary_source_matches_independent_world_geometry_and_actual_area() {
    let fixture = fixture(false);
    unlit_emitter(&fixture);
    let mut document = interior_source(&fixture);
    black_light(&mut document);
    let pivot = DVec3::new(5.5, -7.25, 160.125);
    let id = emitter(
        &mut document,
        DVec3::new(-64.0, -48.0, 176.0),
        DVec3::new(64.0, 48.0, 184.0),
        pivot,
        "20 35 10",
    );
    let mut literal = document.vmf().clone();
    let index = literal
        .entities
        .iter()
        .position(|item| item.id == id)
        .unwrap();
    let mut source = literal.entities.remove(index);
    let rotation = DMat3::from_cols_array(&[
        0.7697511313200572,
        0.5389855446957562,
        -0.3420201433256687,
        -0.5162121193659353,
        0.8407726623973335,
        0.16317591116653482,
        0.37551064385876115,
        0.050950100827363065,
        0.9254165783983234,
    ]);
    for point in source
        .solids
        .iter_mut()
        .flat_map(|solid| &mut solid.sides)
        .flat_map(|side| &mut side.plane.0)
    {
        *point = pivot + rotation * (*point - pivot);
    }
    literal.world.solids.extend(source.solids);
    let actual = compile(&fixture, document.vmf(), stationary_options());
    let expected = compile(&fixture, &literal, stationary_options());
    assert_eq!(actual.stats.lit_brush_models, 0);
    assert_eq!(actual.stats.brush_shadow_models, 1);
    assert_floor_emission_matches(actual.bsp.as_ref().unwrap(), expected.bsp.as_ref().unwrap());
    let declared_area = 128.0 * 96.0;
    let native_resolution = 256.0 * f64::from(f32::EPSILON);
    let area_error =
        4.0 * (128.0 + 96.0) * native_resolution + 8.0 * native_resolution * native_resolution;
    for output in [&actual, &expected] {
        let stats = output.stats.material_emission.as_ref().unwrap();
        assert_eq!(
            (
                stats.matched_sides,
                stats.source_faces,
                stats.source_triangles
            ),
            (1, 1, 2)
        );
        assert!(
            (stats.source_area - declared_area).abs() <= area_error,
            "{} != {declared_area}",
            stats.source_area
        );
        assert!(stats.channel_rays.into_iter().all(|count| count > 0));
        assert_eq!(stats.channel_flux[0][1..], [0.0; 2]);
        assert_eq!(stats.channel_flux[1][..2], [0.0; 2]);
        assert!(
            (stats.channel_flux[0][0] / std::f64::consts::PI - declared_area).abs() <= area_error
        );
        assert!(
            (stats.channel_flux[1][2] / (2.0 * std::f64::consts::PI) - declared_area).abs()
                <= area_error
        );
    }
    for (source, output) in [(document.vmf(), &actual), (&literal, &expected)] {
        let mut off = source.clone();
        off.world
            .properties
            .remove(compiler::MATERIAL_EMISSION_FILE_KEY);
        let baseline = compile(&fixture, &off, stationary_options());
        let enabled = output.bsp.as_ref().unwrap();
        let disabled = baseline.bsp.as_ref().unwrap();
        for (kind, lump) in [
            (LightingKind::Ldr, bsp::lump::WORLDLIGHTS),
            (LightingKind::Hdr, bsp::lump::WORLDLIGHTS_HDR),
        ] {
            assert_eq!(enabled.lumps[lump].data, disabled.lumps[lump].data);
            let lights = enabled.world_lights(kind).unwrap();
            assert_eq!(lights.len(), 1);
            assert_eq!(lights[0].kind, bsp::world_light_kind::POINT);
            assert_eq!(lights[0].origin, [0.0, 0.0, 128.0]);
            assert_eq!(lights[0].intensity, [0.0; 3]);
            assert_eq!(lights[0].style, 0);
        }
    }
}

#[test]
fn stationary_source_keeps_compound_solids_and_distinct_nearby_hulls_in_occlusion() {
    let fixture = fixture(false);
    unlit_emitter(&fixture);
    let mut document = interior_source(&fixture);
    black_light(&mut document);
    let id = emitter(
        &mut document,
        DVec3::new(-64.0, -64.0, 176.0),
        DVec3::new(64.0, 64.0, 184.0),
        DVec3::new(5.5, -7.25, 160.125),
        "0 0 0",
    );
    let clear = compile(&fixture, document.vmf(), stationary_options());
    for same_model in [true, false] {
        let mut blocked = Document::new(document.vmf().clone());
        blocked
            .edit("emission blocker", |tx| {
                let (bottom, top) = if same_model {
                    (128.0, 136.0)
                } else {
                    (175.0, 175.999)
                };
                let solid = edit::block_solid(
                    tx,
                    DVec3::new(-80.0, -80.0, bottom),
                    DVec3::new(80.0, 80.0, top),
                    "test/opaque",
                )?;
                if same_model {
                    tx.entity_mut(id)?.solids.push(solid);
                } else {
                    edit::create_entity(
                        tx,
                        "func_brush",
                        Some(DVec3::ZERO),
                        &[(SHADOWS.into(), "1".into())],
                        vec![solid],
                    )?;
                }
                Ok(())
            })
            .unwrap();
        let output = compile(&fixture, blocked.vmf(), stationary_options());
        assert_eq!(
            output.stats.brush_shadow_models,
            if same_model { 1 } else { 2 }
        );
        assert_eq!(
            output.stats.material_emission.as_ref().unwrap().source_area,
            128.0 * 128.0
        );
        for (kind, channel) in [(LightingKind::Ldr, 0), (LightingKind::Hdr, 2)] {
            assert!(floor_sample(clear.bsp.as_ref().unwrap(), kind, DVec3::ZERO).rgb[channel] > 0);
            assert_eq!(
                floor_sample(output.bsp.as_ref().unwrap(), kind, DVec3::ZERO).linear(),
                [0.0; 3],
                "same_model={same_model} {kind:?}"
            );
        }
    }
}

#[test]
fn nested_child_emission_consumes_origin_pose_once_and_retains_the_original_source() {
    let fixture = fixture(false);
    unlit_emitter(&fixture);
    let mut leaf = Document::empty();
    let source_id = emitter(
        &mut leaf,
        DVec3::new(-48.0, -64.0, 96.0),
        DVec3::new(48.0, 64.0, 104.0),
        DVec3::splat(999.0),
        "0 90 0",
    );
    leaf.edit("effective source pivot", |tx| {
        let pivot = edit::block_solid(
            tx,
            DVec3::new(8.0, 0.0, -8.0),
            DVec3::new(24.0, 16.0, 8.0),
            "test/pivot",
        )?;
        tx.entity_mut(source_id)?.solids.push(pivot);
        Ok(())
    })
    .unwrap();
    let leaf_path = fixture.directory.path().join("maps/source.vmf");
    std::fs::write(&leaf_path, leaf.vmf().write(&Default::default())).unwrap();
    let mut middle = Document::empty().into_vmf();
    instance(&mut middle, "source.vmf", "12 -20 16", "0 0 90");
    std::fs::write(
        fixture.directory.path().join("maps/middle.vmf"),
        middle.write(&Default::default()),
    )
    .unwrap();
    let mut document = interior_source(&fixture);
    black_light(&mut document);
    let mut source = document.vmf().clone();
    instance(&mut source, "middle.vmf", "-32 48 128", "0 0 -90");
    let root = fixture.directory.path().join("maps/parent.vmf");
    std::fs::write(&root, source.write(&Default::default())).unwrap();
    let context = SourceContext::capture(&root, &Control::default()).unwrap();
    let output = compiler::compile_with_source_controlled(
        &source,
        &fixture.game,
        stationary_options(),
        None,
        Some(&context),
        &Control::default(),
    );
    assert!(!output.report.has_errors(), "{:?}", output.report);
    let actual = output.bsp.as_ref().unwrap();
    let stats = output.stats.instances.as_ref().unwrap();
    assert_eq!((stats.brush_models, stats.consumed_origin_brushes), (1, 1));
    assert_eq!(output.stats.brush_shadow_models, 1);
    let id = emitter(
        &mut document,
        DVec3::new(-60.0, 8.0, 244.0),
        DVec3::new(68.0, 104.0, 252.0),
        DVec3::ZERO,
        "0 0 0",
    );
    document
        .edit("literal world source", |tx| edit::move_to_world(tx, id))
        .unwrap();
    let expected = compile(&fixture, document.vmf(), stationary_options());
    assert_floor_emission_matches(actual, expected.bsp.as_ref().unwrap());
    assert_eq!(
        output.stats.material_emission.as_ref().unwrap().source_area,
        128.0 * 96.0
    );
    let destination = fixture.directory.path().join("outputs/previous-child.bsp");
    std::fs::write(&destination, b"previous BSP").unwrap();
    std::fs::write(destination.with_extension("lin"), b"previous LIN").unwrap();
    let publication = output
        .prepare_write_controlled(&destination, &Control::default())
        .unwrap()
        .unwrap();
    drop(output);
    leaf.edit("changed original source", |tx| {
        tx.entity_mut(source_id)?.properties.set("angles", "0 91 0");
        Ok(())
    })
    .unwrap();
    std::fs::write(&leaf_path, leaf.vmf().write(&Default::default())).unwrap();
    assert!(publication.commit(&Control::default()).is_err());
    assert_eq!(std::fs::read(&destination).unwrap(), b"previous BSP");
    assert_eq!(
        std::fs::read(destination.with_extension("lin")).unwrap(),
        b"previous LIN"
    );
}

#[test]
fn unsupported_stationary_emission_requests_refuse_before_cordon_omission() {
    let fixture = fixture(false);
    let mut document = interior_source(&fixture);
    let id = emitter(
        &mut document,
        DVec3::new(512.0, -32.0, 64.0),
        DVec3::new(576.0, 32.0, 80.0),
        DVec3::new(544.0, 0.0, 72.0),
        "0 0 0",
    );
    let mut source = document.into_vmf();
    source.cordon = Some(Node::block(
        "cordon",
        vec![
            Node::string("active", "1"),
            Node::string("mins", "-128 -128 4"),
            Node::string("maxs", "128 128 252"),
        ],
    ));
    let accepted = compile(&fixture, &source, Options::default());
    assert_eq!(
        accepted
            .stats
            .material_emission
            .as_ref()
            .unwrap()
            .matched_sides,
        1
    );
    assert_eq!(accepted.stats.brush_shadow_models, 0);
    assert!(accepted.stats.compile_region.is_some());
    let destination = fixture.directory.path().join("outputs/refused.bsp");
    std::fs::write(&destination, b"previous BSP").unwrap();
    std::fs::write(destination.with_extension("lin"), b"previous LIN").unwrap();
    for (key, value) in [
        (SHADOWS, ""),
        (SHADOWS, "0"),
        (SHADOWS, "2"),
        ("classname", "func_wall"),
        ("classname", "func_door"),
        ("parentname", "mover"),
        ("StartDisabled", "1"),
        ("angles", "NaN 0 0"),
    ] {
        let mut invalid = source.clone();
        let entity = invalid
            .entities
            .iter_mut()
            .find(|item| item.id == id)
            .unwrap();
        if value.is_empty() {
            entity.properties.remove(key);
        } else {
            entity.properties.set(key, value);
        }
        let before = invalid.clone();
        let output = compiler::compile_with_options(&invalid, &fixture.game, Options::default());
        assert_eq!(invalid, before);
        assert!(
            output.report.has_errors() && output.bsp.is_none() && output.leak.is_none(),
            "{key}={value}: {:?}",
            output.report
        );
        assert!(output.stats.compile_region.is_none());
        assert!(
            output
                .report
                .diagnostics
                .iter()
                .any(|item| item.message.contains("Brush global shadows:")
                    || item.message.contains("Shadows Yes")),
            "{key}={value}: {:?}",
            output.report
        );
        assert!(!output.write(&destination).unwrap());
        assert_eq!(std::fs::read(&destination).unwrap(), b"previous BSP");
        assert_eq!(
            std::fs::read(destination.with_extension("lin")).unwrap(),
            b"previous LIN"
        );
    }
}
