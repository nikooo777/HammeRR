mod common;

use compiler::{Control, LightingMode, Options, SkyQuality};
use glam::DVec3;
use map_core::{BrushGeometry, Document, ObjectId, Plane, edit};
use source_formats::bsp::{self, Bsp, LeafAmbient, LightingKind, TexInfo};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::vmf::{DispInfo, Vmf};

const HDR_LUMPS: [usize; 5] = [
    bsp::lump::LIGHTING_HDR,
    bsp::lump::WORLDLIGHTS_HDR,
    bsp::lump::LEAF_AMBIENT_INDEX_HDR,
    bsp::lump::LEAF_AMBIENT_LIGHTING_HDR,
    bsp::lump::FACES_HDR,
];

fn options(mode: LightingMode, bounces: u8) -> Options {
    Options {
        lighting: mode,
        bounces,
        sky_quality: SkyQuality::Fast,
        ..Default::default()
    }
}

fn room() -> Document {
    let mut source = common::room();
    coarsen(&mut source);
    Document::new(source)
}

fn coarsen(source: &mut Vmf) {
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
    {
        side.lightmap_scale = 64;
    }
}

fn light(document: &mut Document, origin: DVec3, ldr: &str, hdr: &str, name: Option<&str>) {
    let mut properties = vec![
        ("_light".into(), ldr.into()),
        ("_lightHDR".into(), hdr.into()),
        ("_constant_attn".into(), "1".into()),
        ("_quadratic_attn".into(), "0".into()),
    ];
    if let Some(name) = name {
        properties.push(("targetname".into(), name.into()));
    }
    document
        .edit("paired light", |tx| {
            edit::create_entity(tx, "light", Some(origin), &properties, Vec::new())
        })
        .unwrap();
}

fn game(bump: bool) -> (tempfile::TempDir, GameFs) {
    let directory = tempfile::tempdir().unwrap();
    let materials = directory.path().join("materials/dev");
    std::fs::create_dir_all(&materials).unwrap();
    std::fs::write(
        materials.join("dev_measuregeneric01.vmt"),
        if bump {
            "LightmappedGeneric { $basetexture dev/reflector $bumpmap dev/normal }"
        } else {
            "LightmappedGeneric { $basetexture dev/reflector }"
        },
    )
    .unwrap();
    let mut texture = vec![0_u8; 80];
    texture[..4].copy_from_slice(b"VTF\0");
    for (offset, value) in [(4, 7_u32), (8, 2), (12, 80)] {
        texture[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
    }
    for offset in [16, 18, 24, 63] {
        texture[offset..offset + 2].copy_from_slice(&1_u16.to_le_bytes());
    }
    texture[56] = 1;
    texture[57..61].copy_from_slice(&(-1_i32).to_le_bytes());
    for axis in 0..3 {
        texture[32 + axis * 4..36 + axis * 4].copy_from_slice(&0.1_f32.to_le_bytes());
    }
    texture.extend_from_slice(&[255; 4]);
    std::fs::write(materials.join("reflector.vtf"), texture).unwrap();
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().to_owned()),
    });
    (directory, game)
}

fn compiled(source: &Vmf, game: &GameFs, mode: LightingMode, bounces: u8) -> compiler::Output {
    let result = compiler::compile_with_options(source, game, options(mode, bounces));
    assert!(!result.report.has_errors(), "{:?}", result.report);
    result
}

fn hdr_reference(source: &Vmf) -> Vmf {
    let mut reference = source.clone();
    for entity in &mut reference.entities {
        if let Some(value) = entity.properties.get("_lightHDR").map(str::to_owned) {
            entity.properties.set("_light", value);
        }
    }
    reference
}

fn assert_channel_reference(actual: &Bsp, kind: LightingKind, reference: &Bsp) {
    let lumps = if kind == LightingKind::Hdr {
        HDR_LUMPS
    } else {
        [
            bsp::lump::LIGHTING,
            bsp::lump::WORLDLIGHTS,
            bsp::lump::LEAF_AMBIENT_INDEX,
            bsp::lump::LEAF_AMBIENT_LIGHTING,
            bsp::lump::FACES,
        ]
    };
    for (actual_lump, reference_lump) in lumps.into_iter().zip([
        bsp::lump::LIGHTING,
        bsp::lump::WORLDLIGHTS,
        bsp::lump::LEAF_AMBIENT_INDEX,
        bsp::lump::LEAF_AMBIENT_LIGHTING,
        bsp::lump::FACES,
    ]) {
        assert_eq!(
            actual.lumps[actual_lump].data, reference.lumps[reference_lump].data,
            "channel {kind:?} lump {actual_lump}"
        );
    }
}

fn floor(bsp: &Bsp, kind: LightingKind, style: usize, direction: usize) -> DVec3 {
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
                if frame.position(s, t).unwrap().length() < 1e-6 {
                    return DVec3::from_array(
                        lighting
                            .face(&face, info)
                            .unwrap()
                            .unwrap()
                            .samples(style, direction)
                            .unwrap()[t * width + s]
                            .linear(),
                    );
                }
            }
        }
    }
    panic!("Missing floor receiver");
}

#[test]
fn dual_channel_radiance_matches_independent_numeric_references_and_legacy_ldr() {
    let mut document = room();
    light(
        &mut document,
        DVec3::new(0.0, 0.0, 128.0),
        "255 0 0 100",
        "0 0 255 1600",
        None,
    );
    let source = document.vmf().clone();
    let game = common::empty_game();
    let ldr = compiled(&source, &game, LightingMode::DirectLdr, 0);
    let hdr = compiled(&hdr_reference(&source), &game, LightingMode::DirectLdr, 0);
    let dual = compiled(&source, &game, LightingMode::DirectLdrHdr, 0);
    let hdr_stats = dual.stats.hdr.as_ref().unwrap();
    assert!(hdr_stats.map_data_present);
    assert_eq!(
        dual.stats.lighting_total_work,
        dual.stats.lighting_work + hdr_stats.lighting_work
    );
    assert_eq!(hdr_stats.world_lights, 1);
    let bsp = dual.bsp.unwrap();
    assert_channel_reference(&bsp, LightingKind::Ldr, ldr.bsp.as_ref().unwrap());
    assert_channel_reference(&bsp, LightingKind::Hdr, hdr.bsp.as_ref().unwrap());
    assert_eq!(
        floor(&bsp, LightingKind::Ldr, 0, 0),
        DVec3::new(100.0, 0.0, 0.0)
    );
    assert_eq!(
        floor(&bsp, LightingKind::Hdr, 0, 0),
        DVec3::new(0.0, 0.0, 1600.0)
    );
    assert_eq!(
        bsp.world_lights(LightingKind::Ldr).unwrap()[0].intensity,
        [(100.0_f64 / 255.0) as f32, 0.0, 0.0]
    );
    assert_eq!(
        bsp.world_lights(LightingKind::Hdr).unwrap()[0].intensity,
        [0.0, 0.0, (1600.0_f64 / 255.0) as f32]
    );
    for (index, lump) in ldr.bsp.unwrap().lumps.into_iter().enumerate() {
        if !HDR_LUMPS.contains(&index) {
            assert_eq!(lump, bsp.lumps[index], "LDR lump {index} changed");
        }
    }
    assert_eq!(*document.vmf(), source);
}

#[test]
fn hdr_only_styles_keep_independent_offsets_and_all_four_bump_maps() {
    let (_directory, game) = game(true);
    let mut document = room();
    light(
        &mut document,
        DVec3::new(0.0, 0.0, 128.0),
        "255 0 0 100",
        "0 0 255 1600",
        None,
    );
    light(
        &mut document,
        DVec3::new(0.0, 0.0, 128.0),
        "0 0 0 0",
        "0 255 0 80",
        Some("hdr_switch"),
    );
    let source = document.vmf().clone();
    let result = compiled(&source, &game, LightingMode::DirectLdrHdr, 0);
    let hdr = result.stats.hdr.as_ref().unwrap();
    assert_eq!(result.stats.light_styles, 1);
    assert_eq!(hdr.light_styles, 2);
    assert_eq!(result.stats.styled_faces, 0);
    assert!(hdr.styled_faces > 0 && hdr.bump_lightmaps > result.stats.bump_lightmaps);
    assert_eq!(result.stats.switchable_light_groups, 1);
    let bsp = result.bsp.unwrap();
    let reference = compiled(&hdr_reference(&source), &game, LightingMode::DirectLdr, 0)
        .bsp
        .unwrap();
    assert_channel_reference(&bsp, LightingKind::Hdr, &reference);
    let ldr_faces = bsp.lighting_faces(LightingKind::Ldr).unwrap();
    let hdr_faces = bsp.lighting_faces(LightingKind::Hdr).unwrap();
    assert!(
        ldr_faces
            .iter()
            .zip(&hdr_faces)
            .any(|(ldr, hdr)| ldr.light_offset != hdr.light_offset)
    );
    let infos = bsp.records::<TexInfo>().unwrap();
    for kind in [LightingKind::Ldr, LightingKind::Hdr] {
        let lighting = bsp.lighting(kind).unwrap().unwrap();
        for face in bsp.lighting_faces(kind).unwrap() {
            if let Some(maps) = lighting.face(&face, &infos[face.texinfo as usize]).unwrap() {
                assert_eq!(maps.maps(), 4);
                assert_eq!(
                    maps.styles(),
                    if kind == LightingKind::Ldr {
                        &[0][..]
                    } else {
                        &[0, 32][..]
                    }
                );
            }
        }
    }
    assert_eq!(
        floor(&bsp, LightingKind::Hdr, 1, 0),
        DVec3::new(0.0, 80.0, 0.0)
    );
    for direction in 1..4 {
        let base = floor(&bsp, LightingKind::Hdr, 0, direction);
        let styled = floor(&bsp, LightingKind::Hdr, 1, direction);
        assert!(base.z > 0.0 && base.x == 0.0 && base.y == 0.0);
        assert!(styled.y > 0.0 && styled.x == 0.0 && styled.z == 0.0);
    }
}

fn shadowed_model(coarse: bool) -> Document {
    let mut document = Document::new(common::room());
    light(
        &mut document,
        DVec3::new(-128.0, 0.0, 128.0),
        "255 0 0 200",
        "0 0 255 1600",
        None,
    );
    document
        .edit("shadowed receiver", |tx| {
            edit::create_block(
                tx,
                DVec3::new(-96.0, -96.0, 48.0),
                DVec3::new(96.0, 96.0, 64.0),
                "dev/dev_measuregeneric01",
            )?;
            let ObjectId::Solid(receiver) = edit::create_block(
                tx,
                DVec3::new(-32.0, -32.0, 8.0),
                DVec3::new(32.0, 32.0, 24.0),
                "dev/dev_measuregeneric01",
            )?
            else {
                unreachable!()
            };
            edit::tie_to_entity(
                tx,
                &[receiver],
                "func_brush",
                &[("origin".into(), "101.5 -67.25 32.125".into())],
            )
        })
        .unwrap();
    if coarse {
        let mut source = document.vmf().clone();
        coarsen(&mut source);
        document = Document::new(source);
    }
    document
}

#[test]
fn multiple_bounces_and_model_probes_read_only_the_selected_channel() {
    let (_directory, game) = game(false);
    let document = shadowed_model(true);
    let source = document.vmf().clone();
    for bounces in [1, 2] {
        let ldr = compiled(&source, &game, LightingMode::DirectLdr, bounces);
        let hdr = compiled(
            &hdr_reference(&source),
            &game,
            LightingMode::DirectLdr,
            bounces,
        );
        let dual = compiled(&source, &game, LightingMode::DirectLdrHdr, bounces);
        let hdr_stats = dual.stats.hdr.as_ref().unwrap();
        assert_eq!(dual.stats.models, ldr.stats.models);
        assert_eq!(dual.stats.lit_brush_models, 1);
        assert_eq!(hdr_stats.lit_brush_models, 1);
        assert!(dual.stats.model_bounce_samples > 0 && hdr_stats.model_bounce_samples > 0);
        assert_eq!(dual.stats.bounce_rays, hdr_stats.bounce_rays);
        let bsp = dual.bsp.unwrap();
        assert_channel_reference(&bsp, LightingKind::Ldr, ldr.bsp.as_ref().unwrap());
        assert_channel_reference(&bsp, LightingKind::Hdr, hdr.bsp.as_ref().unwrap());
        for (kind, active) in [(LightingKind::Ldr, 0), (LightingKind::Hdr, 2)] {
            let LeafAmbient::Indexed { samples, .. } = bsp.leaf_ambient(kind).unwrap().unwrap()
            else {
                panic!("Missing indexed probes");
            };
            assert!(!samples.is_empty());
            let mut positive = false;
            for color in samples.iter().flat_map(|sample| &sample.cube) {
                let values = color.linear();
                positive |= values[active] > 0.0;
                assert!(
                    values
                        .into_iter()
                        .enumerate()
                        .all(|(axis, value)| axis == active || value == 0.0)
                );
            }
            assert!(positive, "Channel {kind:?} probes are black");
        }
    }
    assert_eq!(*document.vmf(), source);
}

#[test]
fn terrain_channel_geometry_and_auxiliary_records_are_shared() {
    let (_directory, game) = game(true);
    let mut document = room();
    for (power, mins, maxs, scale) in [
        (2, [9.0, 17.0, 0.0], [41.0, 65.0, 16.0], 64),
        (4, [73.0, 81.0, 0.0], [201.0, 145.0, 16.0], 32),
        (3, [-191.0, -175.0, 0.0], [-95.0, -47.0, 16.0], 64),
    ] {
        document
            .edit("terrain receiver", |tx| {
                let ObjectId::Solid(id) = edit::create_block(
                    tx,
                    DVec3::from_array(mins),
                    DVec3::from_array(maxs),
                    "dev/dev_measuregeneric01",
                )?
                else {
                    unreachable!()
                };
                let solid = tx.solid_mut(id)?;
                let geometry = BrushGeometry::from_solid(solid).unwrap();
                let top = geometry
                    .faces
                    .iter()
                    .find(|face| face.plane.normal.z > 0.99)
                    .unwrap();
                let size = (1 << power) + 1;
                solid.sides[top.side].lightmap_scale = scale;
                solid.sides[top.side].displacement = Some(DispInfo {
                    power,
                    start_position: top.vertices[0],
                    flags: Some(0),
                    elevation: 0.0,
                    subdivided: false,
                    normals: Some(vec![vec![DVec3::Z; size]; size]),
                    distances: Some(vec![vec![0.0; size]; size]),
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
    light(
        &mut document,
        DVec3::new(0.0, 0.0, 128.0),
        "255 0 0 100",
        "0 0 255 1600",
        None,
    );
    let source = document.vmf();
    let fullbright = compiled(source, &game, LightingMode::Fullbright, 0)
        .bsp
        .unwrap();
    let ldr = compiled(source, &game, LightingMode::DirectLdr, 0)
        .bsp
        .unwrap();
    let reference = compiled(&hdr_reference(source), &game, LightingMode::DirectLdr, 0)
        .bsp
        .unwrap();
    let dual = compiled(source, &game, LightingMode::DirectLdrHdr, 0)
        .bsp
        .unwrap();
    assert_channel_reference(&dual, LightingKind::Ldr, &ldr);
    assert_channel_reference(&dual, LightingKind::Hdr, &reference);
    let dual = Bsp::parse(&dual.to_bytes()).unwrap();
    let data = dual.displacements().unwrap().unwrap();
    assert_eq!(data.infos.len(), 3);
    let ldr_faces = dual.lighting_faces(LightingKind::Ldr).unwrap();
    let faces = dual.lighting_faces(LightingKind::Hdr).unwrap();
    let fullbright_faces = fullbright.lighting_faces(LightingKind::Ldr).unwrap();
    assert!(data.infos.iter().any(|info| {
        let index = usize::from(info.face);
        fullbright_faces[index].lightmap_size != ldr_faces[index].lightmap_size
    }));
    let mut hdr_only = dual.clone();
    hdr_only.lumps[bsp::lump::FACES].data.clear();
    let mut cursor = 0;
    let mut escaped = false;
    for (index, info) in data.infos.iter().enumerate() {
        let face = ldr_faces[usize::from(info.face)];
        let hdr_face = faces[usize::from(info.face)];
        assert_eq!(face.dispinfo, hdr_face.dispinfo);
        assert_eq!(face.texinfo, hdr_face.texinfo);
        assert_eq!(face.lightmap_size, hdr_face.lightmap_size);
        let samples = dual.displacement_lightmap_samples(index).unwrap().unwrap();
        assert_eq!(
            samples,
            ldr.displacement_lightmap_samples(index).unwrap().unwrap()
        );
        assert_eq!(
            samples,
            hdr_only
                .displacement_lightmap_samples(index)
                .unwrap()
                .unwrap()
        );
        let [width, height] = face.lightmap_size.map(|size| size as usize + 1);
        assert_eq!(samples.len(), width * height);
        assert_eq!(
            samples.len(),
            match info.power {
                2 => 4,
                3 => 12,
                4 => 24,
                _ => unreachable!(),
            }
        );
        let positions = data
            .positions(index, dual.displacement_quad(index).unwrap())
            .unwrap();
        let sampler = bsp::DisplacementSurfaceSampler::new(*info, &positions).unwrap();
        for (sample_index, sample) in samples.iter().enumerate() {
            let expected = sampler
                .sample([
                    (sample_index % width) as f64 / (width - 1) as f64,
                    (sample_index / width) as f64 / (height - 1) as f64,
                ])
                .unwrap()
                .lightmap_record()
                .unwrap_or(bsp::DisplacementLightmapSample {
                    triangle: 0,
                    weights: [0; 3],
                });
            assert_eq!(*sample, expected);
            escaped |= sample.triangle >= 255;
        }
        assert_eq!(info.lightmap_sample_start as usize, cursor);
        let encoded = info.encode_lightmap_samples(&samples).unwrap();
        assert_eq!(
            &dual.lumps[bsp::lump::DISP_LIGHTMAP_SAMPLE_POSITIONS].data
                [cursor..cursor + encoded.len()],
            encoded
        );
        cursor += encoded.len();
    }
    assert!(escaped);
    assert_eq!(
        cursor,
        dual.lumps[bsp::lump::DISP_LIGHTMAP_SAMPLE_POSITIONS]
            .data
            .len()
    );
    let mut invalid = dual.clone();
    let info = data.infos.iter().find(|info| info.power == 2).unwrap();
    invalid.lumps[bsp::lump::DISP_LIGHTMAP_SAMPLE_POSITIONS].data
        [info.lightmap_sample_start as usize] = info.triangle_count().unwrap() as u8;
    assert_eq!(
        invalid.displacements().unwrap_err().to_string(),
        "displacement lightmap sample triangle is out of range"
    );
    let face = faces.iter().find(|face| face.dispinfo >= 0).unwrap();
    let infos = dual.records::<TexInfo>().unwrap();
    let lighting = dual.lighting(LightingKind::Hdr).unwrap().unwrap();
    let maps = lighting
        .face(face, &infos[face.texinfo as usize])
        .unwrap()
        .unwrap();
    assert_eq!(maps.maps(), 4);
    assert!(
        maps.samples(0, 0)
            .unwrap()
            .iter()
            .any(|sample| sample.rgb[2] > 0)
    );
    assert!(
        maps.samples(0, 0)
            .unwrap()
            .iter()
            .all(|sample| sample.rgb[..2] == [0, 0])
    );
}

#[test]
fn invalid_or_overlapping_hdr_inputs_preserve_previous_output_after_ldr_success() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("guarded.bsp");
    std::fs::write(&path, b"previous BSP").unwrap();
    for override_value in ["bad HDR", "255 255 255 1e100"] {
        let mut document = room();
        light(
            &mut document,
            DVec3::new(0.0, 0.0, 128.0),
            "255 0 0 100",
            override_value,
            None,
        );
        let ldr = compiled(
            document.vmf(),
            &common::empty_game(),
            LightingMode::DirectLdr,
            0,
        );
        assert!(ldr.bsp.is_some());
        let bad = compiler::compile_with_options(
            document.vmf(),
            &common::empty_game(),
            options(LightingMode::DirectLdrHdr, 0),
        );
        assert!(bad.report.has_errors() && bad.bsp.is_none());
        assert!(!bad.write(&path).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), b"previous BSP");
    }
    let mut document = room();
    for name in ["one", "two", "three", "four"] {
        light(
            &mut document,
            DVec3::new(0.0, 0.0, 128.0),
            "0 0 0 0",
            "0 255 0 80",
            Some(name),
        );
    }
    compiled(
        document.vmf(),
        &common::empty_game(),
        LightingMode::DirectLdr,
        0,
    );
    let bad = compiler::compile_with_options(
        document.vmf(),
        &common::empty_game(),
        options(LightingMode::DirectLdrHdr, 0),
    );
    assert!(bad.report.has_errors() && bad.bsp.is_none());
    assert!(format!("{:?}", bad.report).contains("four light styles"));
    assert!(!bad.write(&path).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), b"previous BSP");
}

#[test]
fn hdr_rgbexp_overflow_preserves_output_after_a_valid_ldr_pass() {
    let mut document = room();
    for _ in 0..512 {
        light(
            &mut document,
            DVec3::new(0.0, 0.0, 128.0),
            "0 0 0 0",
            "0 0 255 3e38",
            None,
        );
    }
    compiled(
        document.vmf(),
        &common::empty_game(),
        LightingMode::DirectLdr,
        0,
    );
    let result = compiler::compile_with_options(
        document.vmf(),
        &common::empty_game(),
        options(LightingMode::DirectLdrHdr, 0),
    );
    assert!(result.report.has_errors() && result.bsp.is_none());
    assert!(format!("{:?}", result.report).contains("RGBExp32"));
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("guarded.bsp");
    std::fs::write(&path, b"previous BSP").unwrap();
    assert!(!result.write(&path).unwrap());
    assert_eq!(std::fs::read(path).unwrap(), b"previous BSP");
}

#[test]
fn both_channels_share_the_work_limit_and_cancelled_publication_is_atomic() {
    let (_directory, game) = game(false);
    let document = shadowed_model(false);
    let settings = Options {
        lighting_work_limit: 100_000_000,
        sky_quality: SkyQuality::Balanced,
        ..options(LightingMode::DirectLdr, 1)
    };
    let ldr = compiler::compile_with_options(document.vmf(), &game, settings);
    assert!(!ldr.report.has_errors(), "{:?}", ldr.report);
    assert!(ldr.stats.lighting_work > 50_000_000);
    let dual = compiler::compile_with_options(
        document.vmf(),
        &game,
        Options {
            lighting: LightingMode::DirectLdrHdr,
            ..settings
        },
    );
    assert!(dual.report.has_errors() && dual.bsp.is_none());
    assert!(format!("{:?}", dual.report).contains("100000000 work-unit"));
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("guarded.bsp");
    std::fs::write(&path, b"previous BSP").unwrap();
    assert!(!dual.write(&path).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), b"previous BSP");
    let coarse = shadowed_model(true);
    let dual = compiled(coarse.vmf(), &game, LightingMode::DirectLdrHdr, 0);
    let control = Control::default();
    control.cancel().unwrap();
    assert!(dual.write_controlled(&path, &control).is_err());
    assert_eq!(std::fs::read(path).unwrap(), b"previous BSP");
}

#[test]
fn empty_hdr_data_is_reported_without_synthetic_world_lights() {
    let document = room();
    let output = compiled(
        document.vmf(),
        &common::empty_game(),
        LightingMode::DirectLdrHdr,
        0,
    );
    let hdr = output.stats.hdr.as_ref().unwrap();
    assert!(!hdr.map_data_present);
    assert!(output.report.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("HDR unavailable: incomplete map data")
            && diagnostic.message.contains("HDR world lights (lump 54)")
    }));
    assert_eq!(hdr.world_lights, 0);
    let bsp = output.bsp.unwrap();
    assert!(bsp.world_lights(LightingKind::Hdr).unwrap().is_empty());
    assert!(bsp.lumps[bsp::lump::WORLDLIGHTS_HDR].data.is_empty());
}
