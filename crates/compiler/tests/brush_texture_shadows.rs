mod common;

use std::path::Path;

use compiler::{LightingMode, Options, SkyQuality};
use glam::DVec3;
use map_core::{BrushGeometry, Document, ObjectId, Plane, edit};
use source_formats::bsp::{self, Bsp, LeafAmbient, LightingKind, RgbExp32, TexInfo};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::vmf::{DispInfo, UvAxis, Vmf};

fn texture(pixels: &[[u8; 4]]) -> Vec<u8> {
    let mut bytes = vec![0_u8; 80];
    bytes[..4].copy_from_slice(b"VTF\0");
    for (offset, value) in [(4, 7_u32), (8, 2), (12, 80)] {
        bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    bytes[16..18].copy_from_slice(&(pixels.len() as u16).to_le_bytes());
    for offset in [18, 24, 63] {
        bytes[offset..offset + 2].copy_from_slice(&1_u16.to_le_bytes());
    }
    for offset in [32, 36, 40] {
        bytes[offset..offset + 4].copy_from_slice(&0.25_f32.to_le_bytes());
    }
    bytes[56] = 1;
    bytes[57..61].copy_from_slice(&(-1_i32).to_le_bytes());
    for pixel in pixels {
        bytes.extend_from_slice(pixel);
    }
    bytes
}

fn game() -> (tempfile::TempDir, GameFs) {
    let root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/validation/brush-texture-shadows");
    std::fs::create_dir_all(&root).unwrap();
    let directory = tempfile::Builder::new()
        .prefix("fixture-")
        .tempdir_in(root)
        .unwrap();
    let game_root = directory.path().join("game");
    let materials = game_root.join("materials");
    for child in ["dev", "test"] {
        std::fs::create_dir_all(materials.join(child)).unwrap();
    }
    for (name, text) in [
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
        std::fs::write(materials.join(format!("{name}.vmt")), text).unwrap();
    }
    for (name, alpha) in [
        ("clear", [0, 0]),
        ("split", [0, 255]),
        ("opaque", [255, 255]),
    ] {
        std::fs::write(
            materials.join(format!("test/{name}.vmt")),
            format!("LightmappedGeneric {{ $basetexture test/{name} $alphatest 1 }}"),
        )
        .unwrap();
        std::fs::write(
            materials.join(format!("test/{name}.vtf")),
            texture(&alpha.map(|alpha| [255, 255, 255, alpha])),
        )
        .unwrap();
    }
    for (name, pixel) in [("white", [255; 4]), ("normal", [128, 128, 255, 255])] {
        std::fs::write(
            materials.join(format!("test/{name}.vtf")),
            texture(&[pixel]),
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

fn room(bump: bool) -> Document {
    let mut source = common::room();
    for side in source
        .world
        .solids
        .iter_mut()
        .flat_map(|solid| &mut solid.sides)
    {
        side.lightmap_scale = 64;
        let plane = Plane::from_points(side.plane.0).unwrap();
        if bump && plane.normal.z > 0.99 && plane.dist.abs() < 0.01 {
            side.material = "test/bump".into();
        }
    }
    Document::new(source)
}

fn light(document: &mut Document, spot: bool, height: f64) {
    let mut properties = vec![
        (
            "_light".into(),
            if spot { "0 255 0 80" } else { "255 0 0 120" }.into(),
        ),
        (
            "_lightHDR".into(),
            if spot { "255 0 0 160" } else { "0 0 255 240" }.into(),
        ),
        ("_constant_attn".into(), "1".into()),
        ("_linear_attn".into(), "0".into()),
        ("_quadratic_attn".into(), "0".into()),
    ];
    if spot {
        properties.extend([
            ("angles".into(), "-90 0 0".into()),
            ("_inner_cone".into(), "45".into()),
            ("_cone".into(), "60".into()),
            ("style".into(), "1".into()),
        ]);
    }
    document
        .edit("light", |tx| {
            edit::create_entity(
                tx,
                if spot { "light_spot" } else { "light" },
                Some(DVec3::new(0.0, 0.0, height)),
                &properties,
                Vec::new(),
            )
        })
        .unwrap();
}

fn pane(source: &Vmf, material: &str, detail: bool, bottom: f64, all_faces: bool) -> Vmf {
    let mut document = Document::new(source.clone());
    document
        .edit("alpha pane", |tx| {
            let ObjectId::Solid(id) = edit::create_block(
                tx,
                DVec3::new(-128.0, -128.0, bottom),
                DVec3::new(128.0, 128.0, bottom + 8.0),
                "dev/dev_measuregeneric01",
            )?
            else {
                unreachable!()
            };
            for side in &mut tx.solid_mut(id)?.sides {
                side.lightmap_scale = 64;
                let horizontal = Plane::from_points(side.plane.0).unwrap().normal.z.abs() > 0.99;
                if horizontal || all_faces {
                    side.material = material.into();
                }
                if horizontal {
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
            if detail {
                edit::tie_to_entity(tx, &[id], "func_detail", &[])?;
            }
            Ok(())
        })
        .unwrap();
    document.vmf().clone()
}

fn compiled(source: &Vmf, game: &GameFs, mode: LightingMode, bounces: u8) -> compiler::Output {
    let original = source.clone();
    let output = compiler::compile_with_options(
        source,
        game,
        Options {
            lighting: mode,
            brush_texture_shadows: true,
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

fn floor(
    bsp: &Bsp,
    kind: LightingKind,
    position: DVec3,
    style: u8,
    direction: usize,
    maps: usize,
) -> RgbExp32 {
    let planes = bsp.records::<bsp::Plane>().unwrap();
    let infos = bsp.records::<TexInfo>().unwrap();
    let lighting = bsp.lighting(kind).unwrap().unwrap();
    for face in bsp.lighting_faces(kind).unwrap() {
        let plane = planes[face.plane as usize];
        if plane.normal[2] < 0.99 || plane.dist.abs() > 0.01 {
            continue;
        }
        let info = &infos[face.texinfo as usize];
        let frame = compiler::lighting::LuxelFrame::new(
            Plane {
                normal: DVec3::Z,
                dist: 0.0,
            },
            info,
            face.lightmap_mins,
            face.lightmap_size,
        )
        .unwrap();
        let [width, height] = frame.dimensions();
        for t in 0..height {
            for s in 0..width {
                if frame.position(s, t).unwrap().distance(position) < 1e-6 {
                    let samples = lighting.face(&face, info).unwrap().unwrap();
                    assert_eq!(samples.maps(), maps);
                    return samples
                        .styles()
                        .iter()
                        .position(|&value| value == style)
                        .map(|index| samples.samples(index, direction).unwrap()[t * width + s])
                        .unwrap_or_default();
                }
            }
        }
    }
    panic!("Missing floor sample at {position}");
}

#[test]
fn detail_cutouts_keep_point_spot_styles_hdr_and_bump_maps_separate() {
    let (directory, game) = game();
    let mut document = room(true);
    light(&mut document, false, 192.0);
    light(&mut document, true, 192.0);
    let source = document.vmf();
    let open = compiled(source, &game, LightingMode::DirectLdrHdr, 0);
    let clear = compiled(
        &pane(source, "test/clear", true, 88.0, false),
        &game,
        LightingMode::DirectLdrHdr,
        0,
    );
    let split = compiled(
        &pane(source, "test/split", true, 88.0, false),
        &game,
        LightingMode::DirectLdrHdr,
        0,
    );
    let opaque = compiled(
        &pane(source, "test/opaque", true, 88.0, false),
        &game,
        LightingMode::DirectLdrHdr,
        0,
    );
    for (x, column) in [(64.0_f64, 0), (-64.0, 1)] {
        for height in [88.0, 96.0] {
            let crossing_x = x * (192.0 - height) / 192.0;
            assert_eq!((crossing_x / 64.0).rem_euclid(2.0).floor() as usize, column);
        }
        for kind in [LightingKind::Ldr, LightingKind::Hdr] {
            for style in [0, 1] {
                for direction in 0..4 {
                    let point = DVec3::new(x, 0.0, 0.0);
                    let expected =
                        floor(open.bsp.as_ref().unwrap(), kind, point, style, direction, 4);
                    let active = match (kind, style) {
                        (LightingKind::Ldr, 0) | (LightingKind::Hdr, 1) => 0,
                        (LightingKind::Ldr, 1) => 1,
                        (LightingKind::Hdr, 0) => 2,
                        _ => unreachable!(),
                    };
                    assert!(
                        expected.rgb[active] > 0,
                        "open receiver x={x}, {kind:?}, style={style}, map={direction}: {expected:?}"
                    );
                    assert!(
                        expected
                            .rgb
                            .iter()
                            .enumerate()
                            .all(|(axis, &value)| axis == active || value == 0)
                    );
                    assert_eq!(
                        floor(
                            clear.bsp.as_ref().unwrap(),
                            kind,
                            point,
                            style,
                            direction,
                            4
                        ),
                        expected
                    );
                    assert_eq!(
                        floor(
                            split.bsp.as_ref().unwrap(),
                            kind,
                            point,
                            style,
                            direction,
                            4
                        ),
                        if column == 0 {
                            expected
                        } else {
                            RgbExp32::default()
                        }
                    );
                    assert_eq!(
                        floor(
                            opaque.bsp.as_ref().unwrap(),
                            kind,
                            point,
                            style,
                            direction,
                            4
                        ),
                        RgbExp32::default()
                    );
                }
            }
        }
    }
    let destination = directory.path().join("preserved.bsp");
    assert!(clear.write(&destination).unwrap());
    let published = std::fs::read(&destination).unwrap();
    let mask = directory.path().join("game/materials/test/clear.vtf");
    let mut bytes = std::fs::read(&mask).unwrap();
    bytes[83] = 255;
    std::fs::write(mask, bytes).unwrap();
    assert!(clear.write(&destination).is_err());
    assert_eq!(std::fs::read(destination).unwrap(), published);
}

fn terrain(document: &mut Document) {
    document
        .edit("flat terrain receiver", |tx| {
            let ObjectId::Solid(id) = edit::create_block(
                tx,
                DVec3::new(-64.0, -64.0, 16.0),
                DVec3::new(64.0, 64.0, 32.0),
                "dev/dev_measuregeneric01",
            )?
            else {
                unreachable!()
            };
            let solid = tx.solid_mut(id)?;
            for side in &mut solid.sides {
                side.lightmap_scale = 64;
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
            Ok(())
        })
        .unwrap();
}

fn terrain_samples(bsp: &Bsp, kind: LightingKind) -> Vec<RgbExp32> {
    let terrain = bsp.displacements().unwrap().unwrap();
    assert_eq!(terrain.infos.len(), 1);
    let face = bsp.lighting_faces(kind).unwrap()[terrain.infos[0].face as usize];
    assert_eq!(face.lightmap_size, [3, 3]);
    let info = bsp.records::<TexInfo>().unwrap()[face.texinfo as usize];
    let lighting = bsp.lighting(kind).unwrap().unwrap();
    let samples = lighting.face(&face, &info).unwrap().unwrap();
    assert_eq!(samples.maps(), 1);
    assert_eq!(samples.styles(), &[0]);
    samples.samples(0, 0).unwrap().to_vec()
}

#[test]
fn clear_world_boundaries_preserve_every_terrain_luxel_in_both_channels() {
    let (_directory, game) = game();
    let mut document = room(false);
    terrain(&mut document);
    light(&mut document, false, 192.0);
    let source = document.vmf();
    let open = compiled(source, &game, LightingMode::DirectLdrHdr, 0);
    let clear = compiled(
        &pane(source, "test/clear", false, 88.0, false),
        &game,
        LightingMode::DirectLdrHdr,
        0,
    );
    let opaque = compiled(
        &pane(source, "test/opaque", false, 88.0, false),
        &game,
        LightingMode::DirectLdrHdr,
        0,
    );
    for kind in [LightingKind::Ldr, LightingKind::Hdr] {
        let expected = terrain_samples(open.bsp.as_ref().unwrap(), kind);
        assert_eq!(expected.len(), 16);
        assert!(expected.iter().all(|sample| sample.rgb != [0; 3]));
        assert_eq!(terrain_samples(clear.bsp.as_ref().unwrap(), kind), expected);
        assert_eq!(
            terrain_samples(opaque.bsp.as_ref().unwrap(), kind),
            vec![RgbExp32::default(); 16]
        );
    }
}

#[test]
fn clear_sources_continue_to_farther_sky_and_lit_surfaces_for_bounces_and_probes() {
    let (_directory, game) = game();
    let mut source = room(false).vmf().clone();
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
    light(&mut document, false, 224.0);
    document
        .edit("blue sky", |tx| {
            edit::create_entity(
                tx,
                "light_environment",
                Some(DVec3::new(0.0, 0.0, 224.0)),
                &[
                    ("_light".into(), "0".into()),
                    ("_ambient".into(), "0 0 255 100".into()),
                    ("pitch".into(), "-90".into()),
                ],
                Vec::new(),
            )
        })
        .unwrap();
    let source = document.vmf();
    let open = compiled(source, &game, LightingMode::DirectLdr, 1);
    let clear = compiled(
        &pane(source, "test/clear", true, 160.0, true),
        &game,
        LightingMode::DirectLdr,
        1,
    );
    let opaque = compiled(
        &pane(source, "test/opaque", true, 160.0, true),
        &game,
        LightingMode::DirectLdr,
        1,
    );
    assert!(clear.stats.bounce_rays > 0 && clear.stats.ambient_samples > 0);
    let open_bsp = open.bsp.as_ref().unwrap();
    let clear_bsp = clear.bsp.as_ref().unwrap();
    let opaque_bsp = opaque.bsp.as_ref().unwrap();
    let expected = floor(open_bsp, LightingKind::Ldr, DVec3::ZERO, 0, 0, 1);
    assert!(expected.rgb[0] > 0 && expected.rgb[2] > 0);
    assert_eq!(
        floor(clear_bsp, LightingKind::Ldr, DVec3::ZERO, 0, 0, 1),
        expected
    );
    assert_ne!(
        floor(opaque_bsp, LightingKind::Ldr, DVec3::ZERO, 0, 0, 1),
        expected
    );
    let expected = open_bsp.leaf_ambient(LightingKind::Ldr).unwrap().unwrap();
    assert_eq!(
        clear_bsp.leaf_ambient(LightingKind::Ldr).unwrap().unwrap(),
        expected
    );
    let LeafAmbient::Indexed { samples, .. } = expected else {
        panic!("Missing indexed probes")
    };
    assert_eq!(samples.len(), 1);
    let upward = samples[0].cube[4].linear();
    assert!(upward[0] > 0.0 && upward[2] > 0.0);
    let LeafAmbient::Indexed { samples, .. } =
        opaque_bsp.leaf_ambient(LightingKind::Ldr).unwrap().unwrap()
    else {
        panic!("Missing indexed probes")
    };
    assert_eq!(samples.len(), 1);
    assert!(samples[0].cube[4].linear()[0] < upward[0]);
}
