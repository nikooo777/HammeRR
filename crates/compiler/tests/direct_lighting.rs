mod common;

use compiler::lighting::LuxelFrame;
use compiler::{LightingMode, Options};
use glam::DVec3;
use map_core::{Document, Plane, edit};
use source_formats::bsp::{self, Bsp, Face, LightingKind, TexInfo};

fn options() -> Options {
    Options {
        lighting: LightingMode::DirectLdr,
        ..Options::default()
    }
}

fn room(origin: DVec3, properties: &[(&str, &str)]) -> Document {
    let mut document = Document::new(common::room());
    let properties = properties
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect::<Vec<_>>();
    document
        .edit("point light", |tx| {
            edit::create_entity(tx, "light", Some(origin), &properties, Vec::new())
        })
        .unwrap();
    document
}

fn floor_sample(bsp: &Bsp, point: DVec3) -> DVec3 {
    let faces = bsp.records::<Face>().unwrap();
    let infos = bsp.records::<TexInfo>().unwrap();
    let planes = bsp.records::<bsp::Plane>().unwrap();
    let lighting = bsp.lighting(LightingKind::Ldr).unwrap().unwrap();
    for face in &faces {
        let plane = planes[face.plane as usize];
        if plane.normal[2] < 0.99 || plane.dist.abs() > 0.01 {
            continue;
        }
        let info = &infos[face.texinfo as usize];
        let frame = LuxelFrame::new(
            Plane {
                normal: DVec3::from_array(plane.normal.map(f64::from)),
                dist: f64::from(plane.dist),
            },
            info,
            face.lightmap_mins,
            face.lightmap_size,
        )
        .unwrap();
        let samples = lighting.face(face, info).unwrap().unwrap();
        let [width, height] = frame.dimensions();
        for t in 0..height {
            for s in 0..width {
                if frame.position(s, t).unwrap().distance(point) < 0.001 {
                    return DVec3::from_array(
                        samples.samples(0, 0).unwrap()[t * width + s].linear(),
                    );
                }
            }
        }
    }
    panic!("No baked floor sample at {point}")
}

#[test]
fn direct_ldr_emits_checked_colored_samples_averages_and_world_lights() {
    let document = room(
        DVec3::new(0.0, 0.0, 100.0),
        &[("_light", "255 0 0 200"), ("_quadratic_attn", "1")],
    );
    let before = document.vmf().write(&Default::default());
    let output = compiler::compile_with_options(document.vmf(), &common::empty_game(), options());
    assert!(!output.report.has_errors(), "{:?}", output.report);
    let stats = &output.stats;
    assert_eq!(stats.lighting_mode, LightingMode::DirectLdr);
    assert_eq!(stats.world_lights, 1);
    assert!(stats.lighting_faces > 0 && stats.lightmap_samples > 0);
    assert_eq!(
        stats.lighting_bytes,
        (stats.lightmap_samples + stats.lighting_faces) * 4
    );
    let bsp = output.bsp.unwrap();
    assert_eq!(floor_sample(&bsp, DVec3::ZERO), DVec3::new(200.0, 0.0, 0.0));
    assert_eq!(bsp.world_lights(LightingKind::Ldr).unwrap().len(), 1);
    assert!(bsp.lighting(LightingKind::Hdr).unwrap().is_none());
    let lighting = bsp.lighting(LightingKind::Ldr).unwrap().unwrap();
    let infos = bsp.records::<TexInfo>().unwrap();
    for face in bsp.records::<Face>().unwrap() {
        if let Some(samples) = lighting.face(&face, &infos[face.texinfo as usize]).unwrap() {
            assert_eq!(samples.styles(), [0]);
            assert!(samples.average(0).is_some());
            assert!(
                samples
                    .samples(0, 0)
                    .unwrap()
                    .iter()
                    .all(|value| value.rgb[1] == 0 && value.rgb[2] == 0)
            );
        }
    }
    assert_eq!(document.vmf().write(&Default::default()), before);
    let again = compiler::compile_with_options(document.vmf(), &common::empty_game(), options())
        .bsp
        .unwrap();
    assert_eq!(bsp.to_bytes(), again.to_bytes());
}

#[test]
fn percentage_light_bakes_half_intensity_and_hard_cutoff_without_source_edits() {
    let document = room(
        DVec3::new(0.0, 0.0, 100.0),
        &[
            ("_light", "255 0 0 200"),
            ("_fifty_percent_distance", "100"),
            ("_zero_percent_distance", "200"),
            ("_hardfalloff", "1"),
        ],
    );
    let before = document.vmf().clone();
    let output = compiler::compile_with_options(document.vmf(), &common::empty_game(), options());
    assert!(!output.report.has_errors(), "{:?}", output.report);
    let bsp = output.bsp.unwrap();
    assert_eq!(floor_sample(&bsp, DVec3::ZERO), DVec3::new(100.0, 0.0, 0.0));
    assert_eq!(floor_sample(&bsp, DVec3::new(176.0, 0.0, 0.0)), DVec3::ZERO);
    assert_eq!(*document.vmf(), before);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("previous.bsp");
    std::fs::write(&path, b"previous BSP").unwrap();
    let invalid = room(
        DVec3::new(0.0, 0.0, 100.0),
        &[
            ("_light", "255 0 0 200"),
            ("_fifty_percent_distance", "100"),
            ("_zero_percent_distance", "50"),
        ],
    );
    let output = compiler::compile_with_options(invalid.vmf(), &common::empty_game(), options());
    assert!(output.report.has_errors());
    assert!(!output.write(&path).unwrap());
    assert_eq!(std::fs::read(path).unwrap(), b"previous BSP");
}

fn sky_fixture(
    material: bool,
    shade: bool,
) -> (tempfile::TempDir, source_formats::gamefs::GameFs, Document) {
    use source_formats::gamefs::{Mount, MountedPath};
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("materials")).unwrap();
    std::fs::write(
        directory.path().join("materials/sky.vmt"),
        "UnlitGeneric { %compilesky 1 }",
    )
    .unwrap();
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().to_owned()),
    });
    let mut vmf = common::room();
    if material {
        let ceiling = vmf
            .world
            .solids
            .iter_mut()
            .find(|solid| solid.sides.iter().all(|side| side.plane.0[0].z >= 256.0))
            .unwrap();
        let side = ceiling
            .sides
            .iter_mut()
            .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z < -0.99)
            .unwrap();
        side.material = "sky".into();
    }
    let mut document = Document::new(vmf);
    document
        .edit("sun", |tx| {
            edit::create_entity(
                tx,
                "light_environment",
                Some(DVec3::new(0.0, 0.0, 128.0)),
                &[
                    ("_light".into(), "255 0 0 200".into()),
                    ("_ambient".into(), "0".into()),
                    ("pitch".into(), "-90".into()),
                ],
                Vec::new(),
            )
        })
        .unwrap();
    if shade {
        document
            .edit("shade", |tx| {
                edit::create_block(
                    tx,
                    DVec3::new(-16.0, -16.0, 32.0),
                    DVec3::new(16.0, 16.0, 48.0),
                    "dev/dev_measuregeneric01",
                )
            })
            .unwrap();
    }
    (directory, game, document)
}

#[test]
fn native_direct_sun_requires_sky_and_preserves_opaque_shadows() {
    for (sky, shade, expected) in [(true, false, 200.0), (true, true, 0.0), (false, false, 0.0)] {
        let (_directory, game, document) = sky_fixture(sky, shade);
        let source = document.vmf().clone();
        let output = compiler::compile_with_options(document.vmf(), &game, options());
        assert!(!output.report.has_errors(), "{:?}", output.report);
        assert_eq!(output.stats.world_lights, 1);
        let bsp = output.bsp.unwrap();
        assert_eq!(
            floor_sample(&bsp, DVec3::ZERO),
            DVec3::new(expected, 0.0, 0.0)
        );
        assert_eq!(
            floor_sample(&bsp, DVec3::new(128.0, 0.0, 0.0)),
            DVec3::new(if sky { 200.0 } else { 0.0 }, 0.0, 0.0)
        );
        let light = bsp.world_lights(LightingKind::Ldr).unwrap()[0];
        assert_eq!(light.kind, bsp::world_light_kind::SKY);
        assert!((DVec3::from_array(light.normal.map(f64::from)) + DVec3::Z).length() < 1e-6);
        assert!((light.intensity[0] as f64 - 200.0 / 255.0).abs() < 1e-7);
        assert_eq!(light.cluster, -1);
        assert_eq!(*document.vmf(), source);
    }
}

#[test]
fn rejected_sky_parameters_and_duplicate_emitters_preserve_previous_output() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("old.bsp");
    std::fs::write(&path, b"previous BSP").unwrap();
    for (field, value) in [
        ("_ambient", "NaN"),
        ("SunSpreadAngle", "91"),
        ("SunSpreadAngle", "-0.1"),
        ("SunSpreadAngle", "NaN"),
        ("SunSpreadAngle", "inf"),
        ("duplicate", "1"),
    ] {
        let (_directory, game, mut document) = sky_fixture(true, false);
        document
            .edit("invalid sun", |tx| {
                if field == "duplicate" {
                    edit::create_entity(
                        tx,
                        "light_environment",
                        Some(DVec3::new(0.0, 0.0, 100.0)),
                        &[("_light".into(), "255".into())],
                        Vec::new(),
                    )?;
                } else {
                    let id = tx
                        .vmf()
                        .entities
                        .iter()
                        .find(|entity| entity.classname() == Some("light_environment"))
                        .unwrap()
                        .id;
                    tx.entity_mut(id)?.properties.set(field, value);
                }
                Ok(())
            })
            .unwrap();
        let output = compiler::compile_with_options(document.vmf(), &game, options());
        assert!(output.report.has_errors(), "{field}: {:?}", output.report);
        assert!(!output.write(&path).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), b"previous BSP");
    }
}

#[test]
fn soft_sun_bakes_a_penumbra_without_changing_native_light_direction() {
    let (_directory, game, mut document) = sky_fixture(true, false);
    document
        .edit("high shadow caster", |tx| {
            edit::create_block(
                tx,
                DVec3::new(-32.0, -128.0, 96.0),
                DVec3::new(32.0, 128.0, 112.0),
                "dev/dev_measuregeneric01",
            )
        })
        .unwrap();
    let hard = compiler::compile_with_options(document.vmf(), &game, options());
    assert!(!hard.report.has_errors(), "{:?}", hard.report);
    let hard = hard.bsp.unwrap();
    assert_eq!(floor_sample(&hard, DVec3::ZERO), DVec3::ZERO);
    let id = document
        .vmf()
        .entities
        .iter()
        .find(|entity| entity.classname() == Some("light_environment"))
        .unwrap()
        .id;
    document
        .edit("soft sun", |tx| {
            tx.entity_mut(id)?.properties.set("SunSpreadAngle", "20");
            Ok(())
        })
        .unwrap();
    let source = document.vmf().clone();
    let mut previous_work = 0;
    for quality in [compiler::SkyQuality::Fast, compiler::SkyQuality::Detailed] {
        let soft = compiler::compile_with_options(
            document.vmf(),
            &game,
            Options {
                sky_quality: quality,
                ..options()
            },
        );
        assert!(!soft.report.has_errors(), "{:?}", soft.report);
        assert!(soft.stats.lighting_work > previous_work);
        previous_work = soft.stats.lighting_work;
        let soft = soft.bsp.unwrap();
        let center = floor_sample(&soft, DVec3::ZERO);
        assert!(center.x > 0.0 && center.x < 200.0, "{quality:?}: {center}");
        assert_eq!(
            floor_sample(&soft, DVec3::new(128.0, 0.0, 0.0)),
            DVec3::new(200.0, 0.0, 0.0)
        );
        assert_eq!(
            soft.world_lights(LightingKind::Ldr).unwrap(),
            hard.world_lights(LightingKind::Ldr).unwrap()
        );
        assert_eq!(*document.vmf(), source);
    }
    document
        .edit("hard sun", |tx| {
            tx.entity_mut(id)?.properties.set("SunSpreadAngle", "0");
            Ok(())
        })
        .unwrap();
    let zero = compiler::compile_with_options(document.vmf(), &game, options())
        .bsp
        .unwrap();
    for (index, lump) in hard.lumps.iter().enumerate() {
        if index != bsp::lump::ENTITIES {
            assert_eq!(zero.lumps[index].data, lump.data, "lump {index}");
        }
    }
}

#[test]
fn local_model_ambient_quality_and_refinement_preserve_lightmaps_anchors_and_sources() {
    use compiler::lighting::{LightingChannel, LightingPhase};
    use source_formats::bsp::{Leaf, LeafAmbient};
    use source_formats::gamefs::{Mount, MountedPath};

    let document = room(
        DVec3::new(0.0, 0.0, 100.0),
        &[
            ("_light", "255 0 0 200"),
            ("_lightHDR", "0 0 255 400"),
            ("_quadratic_attn", "1"),
        ],
    );
    let source = document.vmf().clone();
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/validation/777");
    std::fs::create_dir_all(&root).unwrap();
    let directory = tempfile::tempdir_in(root).unwrap();
    let game_root = directory.path().join("game");
    let material = game_root.join("materials/dev/dev_measuregeneric01.vmt");
    std::fs::create_dir_all(material.parent().unwrap()).unwrap();
    std::fs::write(&material, "LightmappedGeneric {}").unwrap();
    let mut game = common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(game_root),
    });
    let mut outputs = Vec::new();
    for (quality, refine) in [
        (compiler::SkyQuality::Fast, false),
        (compiler::SkyQuality::Detailed, false),
        (compiler::SkyQuality::Detailed, true),
    ] {
        let output = compiler::compile_with_options(
            document.vmf(),
            &game,
            Options {
                lighting: LightingMode::DirectLdrHdr,
                sky_quality: quality,
                brush_texture_shadows: true,
                refine_ambient_probes: refine,
                ..options()
            },
        );
        assert!(!output.report.has_errors(), "{:?}", output.report);
        assert!(output.stats.ambient_samples > 0);
        assert!(output.material_sources.is_some());
        outputs.push(output);
    }
    let maps: Vec<_> = outputs
        .iter()
        .map(|output| output.bsp.as_ref().unwrap())
        .collect();
    for (index, lump) in maps[0].lumps.iter().enumerate() {
        if !matches!(
            index,
            bsp::lump::LEAF_AMBIENT_LIGHTING
                | bsp::lump::LEAF_AMBIENT_LIGHTING_HDR
                | bsp::lump::LEAF_AMBIENT_INDEX
                | bsp::lump::LEAF_AMBIENT_INDEX_HDR
        ) {
            assert_eq!(lump, &maps[1].lumps[index], "quality lump {index}");
        }
    }
    assert_ne!(
        maps[0].lumps[bsp::lump::LEAF_AMBIENT_LIGHTING],
        maps[1].lumps[bsp::lump::LEAF_AMBIENT_LIGHTING]
    );
    for (index, lump) in maps[1].lumps.iter().enumerate() {
        if !matches!(
            index,
            bsp::lump::LEAF_AMBIENT_LIGHTING | bsp::lump::LEAF_AMBIENT_LIGHTING_HDR
        ) {
            assert_eq!(lump, &maps[2].lumps[index], "refinement lump {index}");
        }
    }
    let leaves = maps[2].records::<Leaf>().unwrap();
    for (kind, channel) in [
        (LightingKind::Ldr, LightingChannel::Ldr),
        (LightingKind::Hdr, LightingChannel::Hdr),
    ] {
        let LeafAmbient::Indexed {
            indices: original_indices,
            samples: original,
        } = maps[1].leaf_ambient(kind).unwrap().unwrap()
        else {
            unreachable!()
        };
        let LeafAmbient::Indexed { indices, samples } =
            maps[2].leaf_ambient(kind).unwrap().unwrap()
        else {
            unreachable!()
        };
        assert_eq!(indices, original_indices);
        assert_eq!(samples.len(), original.len());
        assert!(indices.iter().any(|index| index.sample_count == 8));
        for (leaf, index) in leaves
            .iter()
            .zip(&indices)
            .filter(|(_, index)| index.sample_count != 0)
        {
            let first = usize::from(index.first_sample);
            let count = usize::from(index.sample_count);
            assert!(count <= 8);
            assert_eq!(
                &samples[first..first + count - 1],
                &original[first..first + count - 1]
            );
            let selected = &samples[first..first + count];
            let native_point = |sample: &bsp::LeafAmbientSample| {
                DVec3::from_array(std::array::from_fn(|axis| {
                    let center = (f32::from(leaf.mins[axis]) + f32::from(leaf.maxs[axis])) * 0.5;
                    let half = f32::from(leaf.maxs[axis]) - center;
                    f64::from(
                        (f32::from(sample.position[axis]) * (f32::from_bits(0x3c008081) * half)
                            - half)
                            + center,
                    )
                }))
            };
            for (slot, sample) in selected.iter().enumerate() {
                let point = native_point(sample);
                assert!(
                    point
                        .cmpge(DVec3::from_array(leaf.mins.map(f64::from)))
                        .all()
                );
                assert!(
                    point
                        .cmple(DVec3::from_array(leaf.maxs.map(f64::from)))
                        .all()
                );
                for earlier in &selected[..slot] {
                    assert!(point.distance(native_point(earlier)) > 128.0);
                }
            }
        }
        let progress = outputs[2].stats.lighting_progress.as_ref().unwrap();
        let phase = progress
            .phases
            .iter()
            .find(|phase| phase.channel == channel && phase.phase == LightingPhase::AmbientProbes)
            .unwrap();
        assert!(phase.samples_planned_complete);
        assert_eq!(phase.samples_planned, phase.samples_completed);
        assert!(phase.samples_completed > samples.len() as u64);
        assert!(phase.samples_completed <= (samples.len() * 2) as u64);
    }
    assert!(outputs[2].report.diagnostics.iter().any(|diagnostic| {
        diagnostic
            .message
            .contains("refinement evaluates up to 8/16")
    }));
    assert_eq!(*document.vmf(), source);
    let path = directory.path().join("published.bsp");
    assert!(outputs[2].write(&path).unwrap());
    let published = std::fs::read(&path).unwrap();
    std::fs::write(
        &material,
        "LightmappedGeneric { $reflectivity \"[0 0 0]\" }",
    )
    .unwrap();
    assert!(outputs[2].write(&path).is_err());
    assert_eq!(std::fs::read(path).unwrap(), published);
}

#[test]
fn sparse_ambient_leaves_keep_exact_bytes_without_fabricated_candidates() {
    use source_formats::bsp::LeafAmbient;

    let mut source = common::room();
    for side in source
        .world
        .solids
        .iter_mut()
        .flat_map(|solid| &mut solid.sides)
    {
        for point in &mut side.plane.0 {
            *point *= 0.125;
        }
    }
    source.entities.clear();
    let mut document = Document::new(source);
    document
        .edit("small ambient room", |tx| {
            edit::create_entity(
                tx,
                "info_player_terrorist",
                Some(DVec3::new(0.0, 0.0, 8.0)),
                &[],
                Vec::new(),
            )?;
            edit::create_entity(
                tx,
                "light",
                Some(DVec3::new(0.0, 0.0, 24.0)),
                &[("_light".into(), "255 160 80 200".into())],
                Vec::new(),
            )?;
            Ok(())
        })
        .unwrap();
    let source = document.vmf().clone();
    for quality in [
        compiler::SkyQuality::Balanced,
        compiler::SkyQuality::Detailed,
    ] {
        let mut maps = Vec::new();
        for refine in [false, true] {
            let output = compiler::compile_with_options(
                document.vmf(),
                &common::empty_game(),
                Options {
                    sky_quality: quality,
                    refine_ambient_probes: refine,
                    ..options()
                },
            );
            assert!(!output.report.has_errors(), "{:?}", output.report);
            let map = output.bsp.unwrap();
            let LeafAmbient::Indexed { indices, samples } =
                map.leaf_ambient(LightingKind::Ldr).unwrap().unwrap()
            else {
                unreachable!()
            };
            assert_eq!(samples.len(), 1);
            assert_eq!(
                indices
                    .iter()
                    .filter(|index| index.sample_count == 1)
                    .count(),
                1
            );
            assert!(indices.iter().all(|index| index.sample_count <= 1));
            maps.push(map);
        }
        assert_eq!(maps[0].to_bytes(), maps[1].to_bytes());
    }
    assert_eq!(*document.vmf(), source);
}

#[test]
fn model_ambient_probes_reflect_world_light_and_avoid_detail_solids() {
    use source_formats::bsp::{Leaf, LeafAmbient, Model, Node};
    let mut document = room(
        DVec3::new(0.0, 0.0, 220.0),
        &[("_light", "255 0 0 200"), ("_constant_attn", "1")],
    );
    document
        .edit("central detail solid", |tx| {
            let map_core::ObjectId::Solid(id) = edit::create_block(
                tx,
                DVec3::new(-32.0, -32.0, 64.0),
                DVec3::new(32.0, 32.0, 192.0),
                "dev/dev_measuregeneric01",
            )?
            else {
                unreachable!()
            };
            edit::tie_to_entity(tx, &[id], "func_detail", &[])?;
            Ok(())
        })
        .unwrap();
    let source = document.vmf().clone();
    let output = compiler::compile_with_options(
        document.vmf(),
        &common::empty_game(),
        Options {
            sky_quality: compiler::SkyQuality::Detailed,
            ..options()
        },
    );
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.stats.ambient_samples > 0);
    let bsp = output.bsp.unwrap();
    let LeafAmbient::Indexed { indices, samples } =
        bsp.leaf_ambient(LightingKind::Ldr).unwrap().unwrap()
    else {
        unreachable!()
    };
    let leaves = bsp.records::<Leaf>().unwrap();
    let nodes = bsp.records::<Node>().unwrap();
    let planes = bsp.records::<bsp::Plane>().unwrap();
    let model = bsp.records::<Model>().unwrap()[0];
    assert_eq!(samples.len(), output.stats.ambient_samples);
    assert_eq!(
        output.stats.ambient_bytes,
        indices.len() * 4 + samples.len() * 28
    );
    for (owner, index) in indices
        .iter()
        .enumerate()
        .filter(|(_, index)| index.sample_count != 0)
    {
        assert!(index.sample_count <= 8);
        let first = usize::from(index.first_sample);
        let selected = &samples[first..first + usize::from(index.sample_count)];
        for (sample_index, sample) in selected.iter().enumerate() {
            let point = DVec3::from_array(sample.world_position(&leaves[owner]).unwrap());
            assert!(
                !point.cmpge(DVec3::new(-32.0, -32.0, 64.0)).all()
                    || !point.cmple(DVec3::new(32.0, 32.0, 192.0)).all()
            );
            for earlier in &selected[..sample_index] {
                let earlier = DVec3::from_array(earlier.world_position(&leaves[owner]).unwrap());
                assert!(point.distance(earlier) > 128.0);
            }
            let mut cursor = model.head_node;
            while cursor >= 0 {
                let node = nodes[cursor as usize];
                let plane = planes[node.plane as usize];
                let distance = DVec3::from_array(plane.normal.map(f64::from)).dot(point)
                    - f64::from(plane.dist);
                cursor = node.children[usize::from(distance < 0.0)];
            }
            assert_eq!((-1 - cursor) as usize, owner);
            assert!(sample.cube.iter().any(|color| color.linear()[0] > 0.0));
            assert!(
                sample
                    .cube
                    .iter()
                    .all(|color| color.linear()[1..] == [0.0, 0.0])
            );
        }
    }
    assert!(indices.iter().any(|index| index.sample_count > 1));
    assert_eq!(*document.vmf(), source);
    let fullbright =
        compiler::compile_with_options(document.vmf(), &common::empty_game(), Options::default());
    assert_eq!(fullbright.stats.ambient_samples, 0);
    assert!(
        fullbright.bsp.unwrap().lumps[bsp::lump::LEAF_AMBIENT_LIGHTING]
            .data
            .is_empty()
    );
}

#[test]
fn sampled_sky_ambient_matches_open_closed_and_shaded_hemispheres() {
    for (sky, shade, expected) in [
        (true, false, [200.0, 0.0, 60.0]),
        (true, true, [0.0, 0.0, 37.5]),
        (false, false, [0.0; 3]),
    ] {
        let (_directory, game, mut document) = sky_fixture(sky, shade);
        document
            .edit("blue sky ambient", |tx| {
                let id = tx
                    .vmf()
                    .entities
                    .iter()
                    .find(|entity| entity.classname() == Some("light_environment"))
                    .unwrap()
                    .id;
                tx.entity_mut(id)?.properties.set("_ambient", "0 0 255 100");
                Ok(())
            })
            .unwrap();
        let source = document.vmf().clone();
        let output = compiler::compile_with_options(document.vmf(), &game, options());
        assert!(!output.report.has_errors(), "{:?}", output.report);
        assert_eq!(output.stats.world_lights, 2);
        let bsp = output.bsp.unwrap();
        assert_eq!(floor_sample(&bsp, DVec3::ZERO), DVec3::from_array(expected));
        let lights = bsp.world_lights(LightingKind::Ldr).unwrap();
        assert_eq!(lights[1].kind, bsp::world_light_kind::SKY_AMBIENT);
        assert_eq!(lights[1].normal, [0.0; 3]);
        assert!((lights[1].intensity[2] as f64 - 100.0 / 255.0).abs() < 1e-7);
        assert_eq!(*document.vmf(), source);
    }
}

#[test]
fn native_spotlights_bake_inside_the_cone_and_keep_outside_samples_dark() {
    let document = room(
        DVec3::new(0.0, 0.0, 100.0),
        &[
            ("_light", "255 0 0 200"),
            ("_constant_attn", "1"),
            ("pitch", "-90"),
            ("_inner_cone", "20"),
            ("_cone", "45"),
            ("_exponent", "2"),
        ],
    );
    let mut vmf = document.vmf().clone();
    vmf.entities
        .iter_mut()
        .find(|entity| entity.classname() == Some("light"))
        .unwrap()
        .properties
        .set("classname", "light_spot");
    let source = vmf.clone();
    let output = compiler::compile_with_options(&vmf, &common::empty_game(), options());
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert_eq!(output.stats.world_lights, 1);
    let bsp = output.bsp.unwrap();
    assert_eq!(
        bsp.world_lights(LightingKind::Ldr).unwrap()[0].kind,
        bsp::world_light_kind::SPOT
    );
    assert_eq!(floor_sample(&bsp, DVec3::ZERO), DVec3::new(200.0, 0.0, 0.0));
    assert_eq!(floor_sample(&bsp, DVec3::new(128.0, 0.0, 0.0)), DVec3::ZERO);
    assert!(floor_sample(&bsp, DVec3::new(32.0, 0.0, 0.0)).x > 175.0);
    assert_eq!(vmf, source);
}

#[test]
fn opaque_world_hulls_cast_shadows_and_near_far_light_matches_reference() {
    let mut document = room(
        DVec3::new(0.0, 0.0, 100.0),
        &[("_light", "255 255 255 200"), ("_quadratic_attn", "1")],
    );
    let clear = compiler::compile_with_options(document.vmf(), &common::empty_game(), options())
        .bsp
        .unwrap();
    document
        .edit("shadow caster", |tx| {
            edit::create_block(
                tx,
                DVec3::new(-16.0, -16.0, 32.0),
                DVec3::new(16.0, 16.0, 48.0),
                "dev/dev_measuregeneric01",
            )
        })
        .unwrap();
    let shadow = compiler::compile_with_options(document.vmf(), &common::empty_game(), options());
    assert!(!shadow.report.has_errors(), "{:?}", shadow.report);
    assert_eq!(floor_sample(&shadow.bsp.unwrap(), DVec3::ZERO), DVec3::ZERO);
    assert_eq!(floor_sample(&clear, DVec3::ZERO), DVec3::splat(200.0));
    let distant = room(
        DVec3::new(0.0, 0.0, 200.0),
        &[("_light", "255 255 255 200"), ("_quadratic_attn", "1")],
    );
    let distant = compiler::compile_with_options(distant.vmf(), &common::empty_game(), options())
        .bsp
        .unwrap();
    assert_eq!(floor_sample(&distant, DVec3::ZERO), DVec3::splat(50.0));
}

#[test]
fn rejected_lights_and_cancelled_bakes_preserve_existing_output() {
    let document = room(
        DVec3::new(0.0, 0.0, 100.0),
        &[("_light", "255 255 255 NaN")],
    );
    let previous = compiler::compile(document.vmf(), &common::empty_game())
        .bsp
        .unwrap()
        .to_bytes();
    let output = compiler::compile_with_options(document.vmf(), &common::empty_game(), options());
    assert!(output.report.has_errors() && output.bsp.is_none());
    assert!(output.report.diagnostics.iter().any(|diagnostic| {
        diagnostic.level == compiler::Level::Error
            && diagnostic
                .object
                .as_deref()
                .is_some_and(|object| object.starts_with("entity:"))
    }));
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("previous.bsp");
    std::fs::write(&path, &previous).unwrap();
    assert!(!output.write(&path).unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), previous);
    let control = compiler::Control::default();
    control.cancel().unwrap();
    let output = compiler::compile_with_options_controlled(
        document.vmf(),
        &common::empty_game(),
        options(),
        &control,
    );
    assert!(output.bsp.is_none());
    assert!(output.write_controlled(&path, &control).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), previous);
}

#[test]
fn fullbright_remains_default_and_zero_lights_still_produce_dark_lightmaps() {
    let document = common::room();
    let default = compiler::compile(&document, &common::empty_game());
    assert_eq!(default.stats.lighting_mode, LightingMode::Fullbright);
    assert!(
        default
            .bsp
            .as_ref()
            .unwrap()
            .lump(bsp::lump::LIGHTING)
            .data
            .is_empty()
    );
    let explicit =
        compiler::compile_with_options(&document, &common::empty_game(), Options::default());
    assert_eq!(
        default.bsp.unwrap().to_bytes(),
        explicit.bsp.unwrap().to_bytes()
    );
    let dark = compiler::compile_with_options(&document, &common::empty_game(), options());
    assert!(!dark.report.has_errors(), "{:?}", dark.report);
    assert_eq!(dark.stats.world_lights, 0);
    assert!(dark.stats.lighting_faces > 0);
    assert_eq!(floor_sample(&dark.bsp.unwrap(), DVec3::ZERO), DVec3::ZERO);
}
