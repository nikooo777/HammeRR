use glam::DVec3;
use map_core::{Document, ObjectId, Plane, edit};
use native_runtime::CaptureReceipt;
use source_formats::bsp::{self, Bsp, PakEntry, PakLimits};
use source_formats::gamefs::{GameFs, Mount, MountedPath};
use source_formats::vmf::Vmf;

use super::*;
use crate::{Control, LightingMode, Options, Output, compile_with_context_controlled};

#[path = "rotation_tests.rs"]
mod rotation_tests;

pub(super) fn game() -> (tempfile::TempDir, GameFs) {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(directory.path().join("materials/dev")).unwrap();
    std::fs::create_dir_all(directory.path().join("materials/skybox")).unwrap();
    for face in ["rt", "lf", "bk", "ft", "up", "dn"] {
        std::fs::write(
            directory
                .path()
                .join(format!("materials/skybox/sky_day01_01{face}.vmt")),
            "UnlitGeneric { $unknown sky_receipt }",
        )
        .unwrap();
    }
    std::fs::write(directory.path().join("materials/dev/dev_measuregeneric01.vmt"), "LightmappedGeneric { $envmap env_cubemap $surfaceprop concrete Proxies { TextureScroll { textureScrollVar $basetexturetransform textureScrollRate 0.2 } } >=dx90 { $envmap env_cubemap $unknown keeps_this } }").unwrap();
    let mut game = crate::test_common::empty_game();
    game.mounts.push(MountedPath {
        ids: vec!["game".into()],
        mount: Mount::Directory(directory.path().to_owned()),
    });
    (directory, game)
}

pub(super) fn source() -> (Vmf, [u32; 2]) {
    let mut document = Document::new(crate::test_common::room());
    let ids = document
        .edit("reflection fixture", |tx| {
            let mut ids = [0; 2];
            for (slot, origin) in ids.iter_mut().zip([[-96.0, 0.0, 64.0], [96.0, 0.0, 64.0]]) {
                let ObjectId::Entity(id) = edit::create_entity(
                    tx,
                    "env_cubemap",
                    Some(DVec3::from_array(origin)),
                    &[("cubemapsize".into(), "6".into())],
                    Vec::new(),
                )?
                else {
                    unreachable!()
                };
                *slot = id;
            }
            edit::create_entity(
                tx,
                "light",
                Some(DVec3::new(0.0, 0.0, 180.0)),
                &[("_light".into(), "255 255 255 200".into())],
                Vec::new(),
            )?;
            Ok(ids)
        })
        .unwrap();
    (document.vmf().clone(), ids)
}

fn pending(vmf: &Vmf, game: &GameFs) -> Output {
    let output = compile_with_context_controlled(
        vmf,
        game,
        Options {
            lighting: LightingMode::DirectLdr,
            generate_reflections: true,
            ..Default::default()
        },
        Some(&CompileContext::new("reflections").unwrap()),
        &Control::default(),
    );
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.bsp.is_some());
    assert_eq!(
        output.cubemap_assets.as_ref().unwrap().phase,
        CubemapAssetPhase::Pending
    );
    output
}

fn capture_fixture(output: &Output) -> (CubemapAssetManifest, Vec<u8>, CaptureReceipt) {
    let manifest = output.cubemap_assets.as_ref().unwrap();
    let (rebased, private_manifest) = manifest::rebase(
        &output.bsp.as_ref().unwrap().to_bytes(),
        manifest,
        &CompileContext::new("private_capture").unwrap(),
        &Control::default(),
    )
    .unwrap();
    let staged = native_runtime::capture_staging_bsp(&rebased).unwrap();
    let mut captured = Bsp::parse(&staged).unwrap();
    let mut archive = captured.pak_data(PakLimits::default()).unwrap();
    for sample in &private_manifest.samples {
        let entry = archive
            .entries
            .iter_mut()
            .find(|entry| entry.name == sample.texture)
            .unwrap();
        entry.data = native_black_cube(sample.edge);
    }
    captured
        .set_pak_data(&archive, PakLimits::default())
        .unwrap();
    let receipt = CaptureReceipt {
        channel: native_runtime::CaptureChannel::Ldr,
        bsp: captured.to_bytes(),
        staged_bsp: staged.clone(),
        artifacts: "/tmp/fake-native-reflection-evidence".into(),
        elapsed_ms: 1,
        verified_controls: true,
        preservation_passed: true,
    };
    (private_manifest, staged, receipt)
}

pub(super) fn native_black_cube(edge: u16) -> Vec<u8> {
    let legacy = manifest::bootstrap_cube(edge).unwrap();
    let mut bytes = legacy[..80].to_vec();
    bytes[8..12].copy_from_slice(&4u32.to_le_bytes());
    bytes[12..16].copy_from_slice(&96u32.to_le_bytes());
    bytes[52..56].copy_from_slice(&3u32.to_le_bytes());
    bytes[68..72].copy_from_slice(&2u32.to_le_bytes());
    bytes.extend_from_slice(&[0x30, 0, 0, 0, 96, 0, 0, 0, b'C', b'R', b'C', 2, 0, 0, 0, 0]);
    bytes.resize(96 + (legacy.len() - 80) / 4 * 3, 0);
    bytes
}

#[test]
fn neutral_bootstraps_add_no_energy_and_keep_opaque_black_capture_freshness() {
    for edge in [1, 32, 128] {
        let bootstrap = manifest::bootstrap_cube(edge).unwrap();
        let initial = source_formats::vtf::Vtf::parse(bootstrap.clone()).unwrap();
        assert_eq!(initial.reflectivity, [0.0; 3]);
        for mip in 0..initial.mip_count {
            for face in 0..initial.faces {
                assert!(
                    initial
                        .decode_rgba8(mip, 0, face)
                        .unwrap()
                        .pixels
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .all(|pixel| *pixel == [0, 0, 0, 0])
                );
            }
        }
        let native = native_black_cube(edge);
        let captured = source_formats::vtf::Vtf::parse(native.clone()).unwrap();
        assert_eq!(captured.format, source_formats::vtf::ImageFormat::Bgr888);
        for mip in 0..captured.mip_count {
            for face in 0..captured.faces {
                assert!(
                    captured
                        .decode_rgba8(mip, 0, face)
                        .unwrap()
                        .pixels
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .all(|pixel| *pixel == [0, 0, 0, 255])
                );
            }
        }
        let sample = CubemapSampleAsset {
            helper_id: 1,
            origin: [0; 3],
            size_enum: 0,
            edge,
            texture: "materials/maps/test/c0_0_0.vtf".into(),
            bootstrap_sha256: hash(&bootstrap),
            sha256: hash(&native),
        };
        manifest::validate_sample(&bootstrap, &sample, CubemapAssetPhase::Pending).unwrap();
        manifest::validate_sample(&native, &sample, CubemapAssetPhase::Verified).unwrap();
        assert!(manifest::validate_sample(&native, &sample, CubemapAssetPhase::Pending).is_err());
        assert!(
            manifest::validate_sample(&bootstrap, &sample, CubemapAssetPhase::Verified).is_err()
        );
        let mut header_only = bootstrap.clone();
        header_only[32..36].copy_from_slice(&0.25f32.to_le_bytes());
        assert!(
            manifest::validate_sample(&header_only, &sample, CubemapAssetPhase::Verified).is_err()
        );
    }
}

fn merge(output: Output, game: &GameFs) -> Output {
    let manifest = output.cubemap_assets.clone().unwrap();
    let (private_manifest, staged, receipt) = capture_fixture(&output);
    finalize::merge_capture(
        output,
        &manifest,
        &private_manifest,
        &staged,
        receipt,
        game,
        &Control::default(),
    )
    .unwrap()
}

#[test]
fn disabled_context_is_byte_identical_and_pending_cannot_replace_a_file() {
    let (_directory, game) = game();
    let (vmf, _) = source();
    let source = vmf.clone();
    let options = Options {
        lighting: LightingMode::DirectLdr,
        ..Default::default()
    };
    let old = crate::compile_with_options(&vmf, &game, options);
    let explicit = compile_with_context_controlled(
        &vmf,
        &game,
        options,
        Some(&CompileContext::new("unused").unwrap()),
        &Control::default(),
    );
    assert_eq!(
        old.bsp.unwrap().to_bytes(),
        explicit.bsp.unwrap().to_bytes()
    );
    let pending = pending(&vmf, &game);
    assert_eq!(
        pending.bsp.as_ref().unwrap().lump(bsp::lump::FACES).version,
        0
    );
    let directory = tempfile::tempdir().unwrap();
    let destination = directory.path().join("reflections.bsp");
    std::fs::write(&destination, b"previous BSP").unwrap();
    assert!(pending.write(&destination).is_err());
    assert_eq!(std::fs::read(&destination).unwrap(), b"previous BSP");
    assert_eq!(vmf, source);
    assert!(
        Options {
            lighting: LightingMode::Fullbright,
            generate_reflections: true,
            ..Default::default()
        }
        .validate()
        .is_err()
    );
}

#[test]
fn manual_bindings_override_nearest_and_invalid_owners_fail() {
    let (_directory, game) = game();
    let (mut vmf, helpers) = source();
    let floor = vmf.world.solids[0]
        .sides
        .iter()
        .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z > 0.9)
        .unwrap()
        .id;
    let baseline = inspect_cubemap_bindings(
        &vmf,
        &game,
        &CubemapBindingQuery {
            limit: 1024,
            ..Default::default()
        },
        &Control::default(),
    )
    .unwrap();
    assert_eq!(
        baseline
            .assignments
            .iter()
            .find(|assignment| assignment.face_id == floor)
            .unwrap()
            .helper_id,
        helpers[0]
    );
    vmf.entities
        .iter_mut()
        .find(|entity| entity.id == helpers[1])
        .unwrap()
        .properties
        .set("sides", floor.to_string());
    let manual = inspect_cubemap_bindings(
        &vmf,
        &game,
        &CubemapBindingQuery {
            helper_id: Some(helpers[1]),
            limit: 1024,
            ..Default::default()
        },
        &Control::default(),
    )
    .unwrap();
    assert!(
        manual
            .assignments
            .iter()
            .any(|assignment| assignment.face_id == floor && assignment.manual)
    );
    vmf.entities
        .iter_mut()
        .find(|entity| entity.id == helpers[0])
        .unwrap()
        .properties
        .set("sides", floor.to_string());
    assert!(
        inspect_cubemap_bindings(&vmf, &game, &Default::default(), &Control::default())
            .unwrap_err()
            .contains("duplicate")
    );
    vmf.entities
        .iter_mut()
        .find(|entity| entity.id == helpers[0])
        .unwrap()
        .properties
        .set("sides", "4294967295");
    assert!(
        inspect_cubemap_bindings(&vmf, &game, &Default::default(), &Control::default())
            .unwrap_err()
            .contains("missing")
    );
}

#[test]
fn model_pivot_assignment_uses_world_winding_center() {
    let (_directory, game) = game();
    let (vmf, helpers) = source();
    let mut document = Document::new(vmf);
    document
        .edit("model brush", |tx| {
            edit::create_block(
                tx,
                DVec3::new(128.0, -32.0, 16.0),
                DVec3::new(192.0, 32.0, 48.0),
                "dev/dev_measuregeneric01",
            )?;
            Ok(())
        })
        .unwrap();
    let mut vmf = document.vmf().clone();
    let solid = vmf.world.solids.pop().unwrap();
    let top = solid
        .sides
        .iter()
        .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z > 0.9)
        .unwrap()
        .id;
    let mut entity = vmf.entities[0].clone();
    entity.id = 999_000;
    entity.properties.set("classname", "func_brush");
    entity.properties.set("origin", "160 0 32");
    entity.solids = vec![solid];
    vmf.entities.push(entity);
    let report = inspect_cubemap_bindings(
        &vmf,
        &game,
        &CubemapBindingQuery {
            limit: 1024,
            ..Default::default()
        },
        &Control::default(),
    )
    .unwrap();
    assert_eq!(
        report
            .assignments
            .iter()
            .find(|assignment| assignment.face_id == top)
            .unwrap()
            .helper_id,
        helpers[1]
    );
}

fn model_source() -> (Vmf, [u32; 2], u32) {
    let (vmf, helpers) = source();
    let mut document = Document::new(vmf);
    let model = document
        .edit("reflection model", |tx| {
            let ObjectId::Solid(solid) = edit::create_block(
                tx,
                DVec3::new(128.0, -32.0, 16.0),
                DVec3::new(192.0, 32.0, 48.0),
                "dev/dev_measuregeneric01",
            )?
            else {
                unreachable!()
            };
            let ObjectId::Entity(model) = edit::tie_to_entity(
                tx,
                &[solid],
                "func_brush",
                &[("origin".into(), "160 0 32".into())],
            )?
            else {
                unreachable!()
            };
            Ok(model)
        })
        .unwrap();
    (document.vmf().clone(), helpers, model)
}

#[test]
fn unsupported_automatic_model_poses_fail_in_compile_and_inspection() {
    for dependency_only in [false, true] {
        let (directory, game) = game();
        if dependency_only {
            std::fs::write(
                directory
                    .path()
                    .join("materials/dev/dev_measuregeneric01.vmt"),
                "LightmappedGeneric { $fallbackmaterial dev/reflection_dependency }",
            )
            .unwrap();
            std::fs::write(
                directory
                    .path()
                    .join("materials/dev/reflection_dependency.vmt"),
                "LightmappedGeneric { $envmap env_cubemap }",
            )
            .unwrap();
        }
        for fields in [
            vec![("angles", "bad")],
            vec![("angles", "0 0 0"), ("AnGlEs", "0 90 0")],
        ] {
            let (mut vmf, _, model) = model_source();
            let owner = vmf
                .entities
                .iter_mut()
                .find(|entity| entity.id == model)
                .unwrap();
            owner.properties.0.extend(
                fields
                    .iter()
                    .map(|&(key, value)| (key.into(), value.into())),
            );
            let sides: Vec<_> = owner.solids[0].sides.iter().map(|side| side.id).collect();
            let original = vmf.clone();
            let error =
                inspect_cubemap_bindings(&vmf, &game, &Default::default(), &Control::default())
                    .unwrap_err();
            assert!(error.contains("automatic cubemap assignment"), "{error}");
            assert!(error.contains(&format!("entity:{model}")), "{error}");
            assert!(
                sides
                    .iter()
                    .any(|side| error.contains(&format!("side:{side}"))),
                "{error}"
            );
            for lighting in [LightingMode::DirectLdr, LightingMode::DirectLdrHdr] {
                let output = compile_with_context_controlled(
                    &vmf,
                    &game,
                    Options {
                        lighting,
                        generate_reflections: true,
                        ..Default::default()
                    },
                    Some(&CompileContext::new("reflections").unwrap()),
                    &Control::default(),
                );
                assert!(output.bsp.is_none());
                assert!(
                    output.report.diagnostics.iter().any(|diagnostic| {
                        diagnostic.level == crate::Level::Error
                            && diagnostic.message.contains("automatic cubemap assignment")
                            && diagnostic.message.contains(&format!("entity:{model}"))
                            && sides
                                .iter()
                                .any(|side| diagnostic.message.contains(&format!("side:{side}")))
                    }),
                    "{lighting:?} {fields:?}: {:?}",
                    output.report
                );
            }
            assert_eq!(vmf, original);
        }
    }
}

#[test]
fn rotated_models_keep_manual_and_nonreflective_binding_semantics() {
    for manual in [false, true] {
        let (directory, game) = game();
        let (mut vmf, helpers, model) = model_source();
        let owner = vmf
            .entities
            .iter_mut()
            .find(|entity| entity.id == model)
            .unwrap();
        owner.properties.set("angles", "20 35 10");
        let sides: Vec<_> = owner.solids[0].sides.iter().map(|side| side.id).collect();
        if !manual {
            std::fs::write(
                directory.path().join("materials/dev/nonreflective.vmt"),
                "LightmappedGeneric { $surfaceprop concrete }",
            )
            .unwrap();
            for side in &mut owner.solids[0].sides {
                side.material = "dev/nonreflective".into();
            }
        }
        if manual {
            vmf.entities
                .iter_mut()
                .find(|entity| entity.id == helpers[0])
                .unwrap()
                .properties
                .set(
                    "sides",
                    sides
                        .iter()
                        .map(u32::to_string)
                        .collect::<Vec<_>>()
                        .join(" "),
                );
        }
        let original = vmf.clone();
        let report = inspect_cubemap_bindings(
            &vmf,
            &game,
            &CubemapBindingQuery {
                limit: 1024,
                ..Default::default()
            },
            &Control::default(),
        )
        .unwrap();
        for side in &sides {
            let assignment = report
                .assignments
                .iter()
                .find(|assignment| assignment.face_id == *side);
            if manual {
                let assignment = assignment.unwrap();
                assert!(assignment.manual);
                assert_eq!(assignment.helper_id, helpers[0]);
            } else {
                assert!(assignment.is_none());
            }
        }
        let output = compile_with_context_controlled(
            &vmf,
            &game,
            Options {
                lighting: LightingMode::DirectLdr,
                generate_reflections: true,
                ..Default::default()
            },
            Some(&CompileContext::new("reflections").unwrap()),
            &Control::default(),
        );
        assert!(!output.report.has_errors(), "{:?}", output.report);
        assert!(output.bsp.is_some());
        assert_eq!(vmf, original);
    }
}

#[test]
fn verified_black_captures_merge_only_pak_and_rebase_only_registered_paths() {
    let (_directory, game) = game();
    let (vmf, _) = source();
    let output = pending(&vmf, &game);
    let original = output.bsp.as_ref().unwrap().clone();
    let mut verified = merge(output, &game);
    assert_eq!(
        verified.cubemap_assets.as_ref().unwrap().phase,
        CubemapAssetPhase::Verified
    );
    let bsp = verified.bsp.as_mut().unwrap();
    assert_eq!(bsp.game_lumps, original.game_lumps);
    for index in 0..bsp::LUMP_COUNT {
        if index != bsp::lump::PAKFILE {
            assert_eq!(bsp.lump(index), original.lump(index), "lump {index}");
        }
    }
    let unknown_material = "maps/reflections/c-96_0_64";
    let unknown_default = "materials/maps/reflections/cubemapdefault.vtf";
    let mut names = bsp.texdata_strings().unwrap();
    names.push(unknown_material.into());
    bsp.set_texdata_strings(&names);
    let mut archive = bsp.pak_data(PakLimits::default()).unwrap();
    archive.entries.push(PakEntry {
        name: unknown_default.into(),
        data: b"unregistered asset".to_vec(),
    });
    archive.entries.push(PakEntry {
        name: "hammerr/unknown.bin".into(),
        data: vec![0, 255, 3, 17],
    });
    bsp.set_pak_data(&archive, PakLimits::default()).unwrap();
    let before = bsp.to_bytes();
    let after =
        rebase_generated_cubemaps(&before, "hammerr_random_run", &Control::default()).unwrap();
    let rebased = Bsp::parse(&after).unwrap();
    let manifest = read_generated_cubemaps(&rebased, &Control::default())
        .unwrap()
        .unwrap();
    assert_eq!(manifest.namespace, "hammerr_random_run");
    assert_eq!(manifest.logical_namespace, "reflections");
    assert!(
        rebased
            .texdata_strings()
            .unwrap()
            .iter()
            .any(|name| name == unknown_material)
    );
    let pak = rebased.pak_data(PakLimits::default()).unwrap();
    assert!(
        pak.entries
            .iter()
            .any(|entry| entry.name == unknown_default && entry.data == b"unregistered asset")
    );
    assert!(
        pak.entries
            .iter()
            .any(|entry| entry.name == "hammerr/unknown.bin" && entry.data == [0, 255, 3, 17])
    );
    for index in 0..bsp::LUMP_COUNT {
        if ![
            bsp::lump::PAKFILE,
            bsp::lump::TEXDATA_STRING_DATA,
            bsp::lump::TEXDATA_STRING_TABLE,
        ]
        .contains(&index)
        {
            assert_eq!(
                &before[8 + index * 16..24 + index * 16],
                &after[8 + index * 16..24 + index * 16]
            );
        }
    }
    let first_alias = &manifest.aliases[0];
    let alias = pak
        .entries
        .iter()
        .find(|entry| entry.name == source_formats::vmt::material_path(&first_alias.alias))
        .unwrap();
    let text = std::str::from_utf8(&alias.data).unwrap();
    assert!(text.contains("TextureScroll") && text.contains("keeps_this"));
    assert!(!text.contains("\"env_cubemap\""));
}

#[test]
fn missing_capture_changed_geometry_and_failed_preservation_are_rejected() {
    let (_directory, game) = game();
    let (vmf, _) = source();
    for failure in 0..3 {
        let output = pending(&vmf, &game);
        let manifest = output.cubemap_assets.clone().unwrap();
        let (private, staged, mut receipt) = capture_fixture(&output);
        if failure == 0 {
            let mut captured = Bsp::parse(&receipt.bsp).unwrap();
            let mut archive = captured.pak_data(PakLimits::default()).unwrap();
            archive
                .entries
                .retain(|entry| entry.name != private.samples[0].texture);
            captured
                .set_pak_data(&archive, PakLimits::default())
                .unwrap();
            receipt.bsp = captured.to_bytes();
        } else if failure == 1 {
            let mut captured = Bsp::parse(&receipt.bsp).unwrap();
            captured.lumps[bsp::lump::VERTEXES].data[0] ^= 1;
            receipt.bsp = captured.to_bytes();
        } else {
            receipt.preservation_passed = false;
        }
        assert!(
            finalize::merge_capture(
                output,
                &manifest,
                &private,
                &staged,
                receipt,
                &game,
                &Control::default()
            )
            .is_err()
        );
    }
}

#[test]
fn script_and_size_preflight_fail_before_native_launch() {
    let (_directory, game) = game();
    let (mut vmf, helpers) = source();
    vmf.entities
        .iter_mut()
        .find(|entity| entity.id == helpers[0])
        .unwrap()
        .properties
        .set("cubemapsize", "9");
    assert!(
        inspect_cubemap_bindings(&vmf, &game, &Default::default(), &Control::default())
            .unwrap_err()
            .contains("larger verified viewport")
    );
    vmf.entities[0].properties.set("vscripts", "unsafe.nut");
    let output = compile_with_context_controlled(
        &vmf,
        &game,
        Options {
            lighting: LightingMode::DirectLdr,
            generate_reflections: true,
            ..Default::default()
        },
        Some(&CompileContext::new("reflections").unwrap()),
        &Control::default(),
    );
    assert!(output.bsp.is_none());
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("script"))
    );
}

#[test]
fn changing_a_nonreflective_scene_texture_rejects_capture_publication() {
    let (directory, game) = game();
    std::fs::create_dir_all(directory.path().join("materials/ordinary")).unwrap();
    std::fs::write(
        directory.path().join("materials/ordinary/wall.vmt"),
        "UnlitGeneric { $basetexture ordinary/wall }",
    )
    .unwrap();
    let texture = directory.path().join("materials/ordinary/wall.vtf");
    std::fs::write(&texture, manifest::bootstrap_cube(1).unwrap()).unwrap();
    let (mut vmf, _) = source();
    for side in &mut vmf.world.solids[1].sides {
        side.material = "ordinary/wall".into();
    }
    let output = pending(&vmf, &game);
    let manifest = output.cubemap_assets.clone().unwrap();
    assert!(
        manifest
            .sources
            .iter()
            .any(|source| source.path == "materials/ordinary/wall.vtf")
    );
    let (private, staged, receipt) = capture_fixture(&output);
    let mut changed = std::fs::read(&texture).unwrap();
    changed[80] ^= 1;
    std::fs::write(&texture, changed).unwrap();
    let error = finalize::merge_capture(
        output,
        &manifest,
        &private,
        &staged,
        receipt,
        &game,
        &Control::default(),
    )
    .unwrap_err();
    assert!(error.to_string().contains("source changed"));
}

#[test]
fn native_cube_validation_rejects_normalized_and_ambiguous_headers() {
    let legacy = manifest::bootstrap_cube(32).unwrap();
    manifest::validate_cube(&legacy, 32).unwrap();
    for offset in [24, 63] {
        let mut malformed = legacy.clone();
        malformed[offset..offset + 2].fill(0);
        assert!(manifest::validate_cube(&malformed, 32).is_err());
    }
    let mut native = legacy[..80].to_vec();
    native[8..12].copy_from_slice(&4u32.to_le_bytes());
    native[12..16].copy_from_slice(&96u32.to_le_bytes());
    native[68..72].copy_from_slice(&2u32.to_le_bytes());
    native.extend_from_slice(&[0x30, 0, 0, 0, 96, 0, 0, 0, b'C', b'R', b'C', 2, 1, 0, 0, 0]);
    native.extend_from_slice(&legacy[80..]);
    manifest::validate_cube(&native, 32).unwrap();
    let mut inline = native.clone();
    inline[83] = 2;
    assert!(manifest::validate_cube(&inline, 32).is_err());
    let mut duplicate = native.clone();
    duplicate[88..96].copy_from_slice(&native[80..88]);
    assert!(manifest::validate_cube(&duplicate, 32).is_err());
    let mut trailing = native.clone();
    trailing.push(0);
    assert!(manifest::validate_cube(&trailing, 32).is_err());
    let mut header_only = legacy.clone();
    header_only[32..36].copy_from_slice(&0.25f32.to_le_bytes());
    let sample = CubemapSampleAsset {
        helper_id: 1,
        origin: [0, 0, 0],
        size_enum: 6,
        edge: 32,
        texture: "materials/maps/test/c0_0_0.vtf".into(),
        bootstrap_sha256: hash(&legacy),
        sha256: hash(&header_only),
    };
    assert!(manifest::validate_sample(&header_only, &sample, CubemapAssetPhase::Verified).is_err());
    assert!(
        material::bound_text(&("Shader{key\"}}\" child{".to_owned() + &"{".repeat(33))).is_err()
    );
}

#[test]
fn explicit_named_dependencies_survive_registered_rebase_and_face_tampering_fails() {
    let (directory, game) = game();
    for path in ["materials/maps/reflections", "materials/test"] {
        std::fs::create_dir_all(directory.path().join(path)).unwrap();
    }
    std::fs::write(
        directory
            .path()
            .join("materials/maps/reflections/ordinary.vmt"),
        "UnlitGeneric { $envmap maps/reflections/custom }",
    )
    .unwrap();
    std::fs::write(
        directory
            .path()
            .join("materials/maps/reflections/custom.vtf"),
        manifest::bootstrap_cube(1).unwrap(),
    )
    .unwrap();
    std::fs::write(
        directory.path().join("materials/test/glass.vmt"),
        "LightmappedGeneric { $envmap env_cubemap $unknown child_metadata }",
    )
    .unwrap();
    std::fs::write(directory.path().join("materials/dev/dev_measuregeneric01.vmt"), "LightmappedGeneric { $envmap env_cubemap $bottommaterial maps/reflections/ordinary $fallbackmaterial test/glass >=dx90 { $envmap maps/reflections/custom } }").unwrap();
    let (vmf, _) = source();
    let output = merge(pending(&vmf, &game), &game);
    let bsp = output.bsp.unwrap();
    let bytes =
        rebase_generated_cubemaps(&bsp.to_bytes(), "zzzz_private", &Control::default()).unwrap();
    let rebased = Bsp::parse(&bytes).unwrap();
    let manifest = read_generated_cubemaps(&rebased, &Control::default())
        .unwrap()
        .unwrap();
    assert!(manifest.aliases.iter().any(|alias| {
        alias
            .envmaps
            .iter()
            .any(|name| name == "maps/reflections/custom")
    }));
    assert!(manifest.aliases.iter().any(|alias| {
        alias
            .dependencies
            .iter()
            .any(|name| name == "maps/reflections/ordinary")
    }));
    let mut corrupted = rebased.clone();
    let binding = manifest
        .face_bindings
        .iter()
        .find(|binding| binding.lump == bsp::lump::FACES)
        .unwrap();
    let mut faces = corrupted.records::<bsp::Face>().unwrap();
    faces[binding.index].texinfo = 0;
    corrupted.set_records(&faces);
    assert!(read_generated_cubemaps(&corrupted, &Control::default()).is_err());
    let mut corrupted = rebased;
    let binding = &manifest.texinfo_bindings[0];
    let mut infos = corrupted.records::<bsp::TexInfo>().unwrap();
    infos[binding.index].texture_vecs[0][3] += 1.0;
    corrupted.set_records(&infos);
    assert!(read_generated_cubemaps(&corrupted, &Control::default()).is_err());
}

#[test]
fn ambiguous_native_patch_edits_fail_without_synthesizing_reflectivity() {
    let (directory, game) = game();
    std::fs::create_dir_all(directory.path().join("materials/patches")).unwrap();
    std::fs::write(
        directory.path().join("materials/patches/base.vmt"),
        "LightmappedGeneric { $surfaceprop concrete }",
    )
    .unwrap();
    std::fs::write(
        directory.path().join("materials/patches/bad.vmt"),
        "Patch { include materials/patches/base.vmt replace { $envmap env_cubemap } }",
    )
    .unwrap();
    let error = material::Graph::new(&game, &Control::default())
        .needs_alias("patches/bad")
        .unwrap_err();
    assert!(error.contains("ambiguous Patch"));
    std::fs::write(
        directory.path().join("materials/patches/base.vmt"),
        "LightmappedGeneric { $envmap env_cubemap }",
    )
    .unwrap();
    std::fs::write(
        directory.path().join("materials/patches/bad.vmt"),
        "Patch { include materials/patches/base.vmt replace { Proxies { X { value 1 } } } }",
    )
    .unwrap();
    assert!(
        material::Graph::new(&game, &Control::default())
            .needs_alias("patches/bad")
            .unwrap_err()
            .contains("nested Patch")
    );
}

#[test]
fn prepared_output_in_a_content_mount_does_not_invalidate_its_final_source_guard() {
    let (directory, game) = game();
    let (vmf, _) = source();
    let output = merge(pending(&vmf, &game), &game);
    let path = directory.path().join("reflections.bsp");
    let control = Control::default();
    let lease = crate::OutputLease::acquire(&path, &control).unwrap();
    let prepared = output
        .prepare_write_controlled(&path, &control)
        .unwrap()
        .unwrap();
    let guard =
        capture_reflection_source_guard(output.cubemap_assets.as_ref().unwrap(), &game, &control)
            .unwrap();
    assert!(
        prepared
            .commit_checked_with(&lease, &control, || guard.verify_metadata())
            .unwrap()
    );
    assert_eq!(&std::fs::read(&path).unwrap()[..4], b"VBSP");
}

#[test]
fn final_source_guard_detects_shadow_additions_after_output_preparation() {
    let (_directory, mut game) = game();
    let (vmf, _) = source();
    let output = merge(pending(&vmf, &game), &game);
    let first = tempfile::tempdir().unwrap();
    game.mounts.insert(
        0,
        MountedPath {
            ids: vec!["game".into()],
            mount: Mount::Directory(first.path().to_owned()),
        },
    );
    let destination = tempfile::tempdir().unwrap();
    let path = destination.path().join("reflections.bsp");
    std::fs::write(&path, b"previous output").unwrap();
    let control = Control::default();
    let lease = crate::OutputLease::acquire(&path, &control).unwrap();
    let prepared = output
        .prepare_write_controlled(&path, &control)
        .unwrap()
        .unwrap();
    let guard =
        capture_reflection_source_guard(output.cubemap_assets.as_ref().unwrap(), &game, &control)
            .unwrap();
    std::fs::create_dir_all(first.path().join("Materials/Dev")).unwrap();
    std::fs::write(
        first.path().join("Materials/Dev/DEV_MEASUREGENERIC01.vmt"),
        "UnlitGeneric { $color [1 0 0] }",
    )
    .unwrap();
    assert!(
        prepared
            .commit_checked_with(&lease, &control, || guard.verify_metadata())
            .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"previous output");
}

#[test]
fn final_source_guard_rejects_texture_replacement_after_output_preparation() {
    let (directory, game) = game();
    let texture = directory.path().join("materials/dev/source.vtf");
    std::fs::write(&texture, manifest::bootstrap_cube(1).unwrap()).unwrap();
    std::fs::write(
        directory
            .path()
            .join("materials/dev/dev_measuregeneric01.vmt"),
        "LightmappedGeneric { $envmap env_cubemap $basetexture dev/source }",
    )
    .unwrap();
    let (vmf, _) = source();
    let output = merge(pending(&vmf, &game), &game);
    let destination = tempfile::tempdir().unwrap();
    let path = destination.path().join("reflections.bsp");
    std::fs::write(&path, b"previous output").unwrap();
    let control = Control::default();
    let lease = crate::OutputLease::acquire(&path, &control).unwrap();
    let prepared = output
        .prepare_write_controlled(&path, &control)
        .unwrap()
        .unwrap();
    let guard =
        capture_reflection_source_guard(output.cubemap_assets.as_ref().unwrap(), &game, &control)
            .unwrap();
    let replacement = directory.path().join("materials/dev/changed.vtf");
    let mut bytes = std::fs::read(&texture).unwrap();
    *bytes.last_mut().unwrap() ^= 255;
    std::fs::write(&replacement, bytes).unwrap();
    std::fs::rename(&replacement, &texture).unwrap();
    assert!(
        prepared
            .commit_checked_with(&lease, &control, || guard.verify_metadata())
            .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"previous output");
}

#[test]
fn explicit_default_envmap_remains_inspectable_but_cannot_be_shadowed_by_generation() {
    let (directory, game) = game();
    let folder = directory.path().join("materials/maps/reflections");
    std::fs::create_dir_all(&folder).unwrap();
    let texture = manifest::bootstrap_cube(1).unwrap();
    std::fs::write(folder.join("cubemapdefault.vtf"), &texture).unwrap();
    std::fs::write(
        directory.path().join("materials/dev/dev_measuregeneric01.vmt"),
        "LightmappedGeneric { $envmap env_cubemap >=dx90 { $envmap maps/reflections/cubemapdefault } }",
    )
    .unwrap();
    let (vmf, _) = source();
    let before = vmf.clone();
    let query = CubemapBindingQuery {
        namespace: Some("reflections".into()),
        ..Default::default()
    };
    assert!(inspect_cubemap_bindings(&vmf, &game, &query, &Control::default()).is_ok());
    let output = compile_with_context_controlled(
        &vmf,
        &game,
        Options {
            lighting: LightingMode::DirectLdr,
            generate_reflections: true,
            ..Default::default()
        },
        Some(&CompileContext::new("reflections").unwrap()),
        &Control::default(),
    );
    assert!(output.bsp.is_none() && output.report.has_errors());
    assert!(output.report.diagnostics.iter().any(|diagnostic| {
        diagnostic.message.contains("default cubemap")
            && diagnostic
                .message
                .contains("materials/maps/reflections/cubemapdefault.vtf")
    }));
    assert_eq!(vmf, before);
    assert_eq!(
        std::fs::read(folder.join("cubemapdefault.vtf")).unwrap(),
        texture
    );
}

#[test]
fn native_default_merge_rejects_mounted_logical_and_private_name_collisions() {
    for namespace in ["reflections", "private_capture"] {
        let (directory, game) = game();
        let (vmf, _) = source();
        let output = pending(&vmf, &game);
        let manifest = output.cubemap_assets.clone().unwrap();
        let (private, staged, mut receipt) = capture_fixture(&output);
        let mut captured = Bsp::parse(&receipt.bsp).unwrap();
        let mut archive = captured.pak_data(PakLimits::default()).unwrap();
        let data = archive
            .entries
            .iter()
            .find(|entry| entry.name == private.samples[0].texture)
            .unwrap()
            .data
            .clone();
        archive.entries.push(PakEntry {
            name: "materials/maps/private_capture/cubemapdefault.vtf".into(),
            data,
        });
        captured
            .set_pak_data(&archive, PakLimits::default())
            .unwrap();
        receipt.bsp = captured.to_bytes();
        let folder = directory.path().join(format!("materials/maps/{namespace}"));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("cubemapdefault.vtf");
        std::fs::write(&path, b"authored default texture").unwrap();
        let error = finalize::merge_capture(
            output,
            &manifest,
            &private,
            &staged,
            receipt,
            &game,
            &Control::default(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("default cubemap"), "{error}");
        assert_eq!(std::fs::read(&path).unwrap(), b"authored default texture");
    }
}
