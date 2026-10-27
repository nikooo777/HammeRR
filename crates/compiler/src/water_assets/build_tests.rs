use std::fs;

use glam::DVec3;
use map_core::{Document, ObjectId, Plane, compile_region, edit};
use source_formats::{bsp, vmf::Vmf};

use super::tests::{
    BOTTOM, CUBE, Fixture, HDR_CUBE, NORMAL, ROOT, SLIME, SLIME_PATCH, SLIME_STATIC_ENVMAP,
    STATIC_LIQUIDS, WATER,
};
use super::*;
use crate::{Control, Options, OutputLease, WriteReservation, test_common};

fn room() -> Vmf {
    let mut document = Document::new(test_common::room());
    document
        .edit("Water pool", |transaction| {
            let ObjectId::Solid(id) = edit::create_block(
                transaction,
                DVec3::new(-128.0, -128.0, 0.0),
                DVec3::new(128.0, 128.0, 64.0),
                "test/nodraw",
            )?
            else {
                unreachable!()
            };
            for side in &mut transaction.solid_mut(id)?.sides {
                if Plane::from_points(side.plane.0).unwrap().normal.z > 0.99 {
                    side.material = "TeSt/RoOt".into();
                }
            }
            Ok(())
        })
        .unwrap();
    document.vmf().clone()
}

fn fixture() -> Fixture {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.directory.path().join("materials/tools")).unwrap();
    fixture.write("materials/tools/toolsblack.vmt", "LightmappedGeneric {}");
    fixture.write(
        "materials/test/nodraw.vmt",
        "UnlitGeneric { %compilenodraw 1 }",
    );
    fixture
}

fn options() -> Options {
    Options {
        pack_water_assets: true,
        ..Default::default()
    }
}

#[test]
fn complete_build_uses_shader_roots_and_changes_only_the_pak() {
    for (material, files) in STATIC_LIQUIDS {
        let fixture = fixture();
        fixture.liquid(material);
        let source = room();
        let original = source.clone();
        let baseline = crate::compile(&source, &fixture.game);
        let output = crate::compile_with_options(&source, &fixture.game, options());
        assert!(!output.report.has_errors(), "{:?}", output.report);
        assert_eq!(output.stats.water_regions, 1);
        let plan = output.water_assets.as_ref().unwrap();
        let roots: &[&str] = if material == WATER {
            &[BOTTOM, ROOT]
        } else {
            &[ROOT]
        };
        assert_eq!(plan.roots(), roots);
        assert!(output.material_sources.is_some());
        let stats = output.stats.water_assets.as_ref().unwrap();
        assert_eq!(
            (stats.roots, stats.channels, stats.files, stats.asset_bytes),
            (roots.len(), "ldr_hdr", files, plan.asset_bytes())
        );
        let bsp = output.bsp.as_ref().unwrap();
        for index in 0..bsp::LUMP_COUNT {
            if index != bsp::lump::PAKFILE {
                assert_eq!(
                    bsp.lumps[index],
                    baseline.bsp.as_ref().unwrap().lumps[index]
                );
            }
        }
        verify_packed_water(bsp, &plan.manifest(), &Control::default()).unwrap();
        assert_eq!(source, original);
        assert!(
            !output
                .report
                .diagnostics
                .iter()
                .any(|item| item.message.contains("remain mounted")
                    || item.message.contains("packing is unavailable"))
        );
    }
}

#[test]
fn unsupported_liquid_packing_preserves_prior_outputs_and_source() {
    for (material, reason) in [
        (
            "LightmappedGeneric { %compilewater 1 $basetexture test/color }",
            "supported Water-shader or static opaque SLIME",
        ),
        (
            "LightmappedGeneric { %compileslime 1 $bottommaterial test/root $basetexture test/color $envmap env_cubemap }",
            "runtime env_cubemap",
        ),
        (
            "LightmappedGeneric { %compileslime 1 $bottommaterial test/root $basetexture test/color $detail test/color }",
            "$detail",
        ),
        (
            "LightmappedGeneric { %compileslime 1 $bottommaterial test/root $basetexture test/color $bumpmap test/cube }",
            "cubemap layout",
        ),
        (
            "LightmappedGeneric { %compileslime 1 $bottommaterial test/root $basetexture test/color $bumpmap test/missing }",
            "Missing Water source",
        ),
    ] {
        let fixture = fixture();
        fixture.write(ROOT, material);
        let path = fixture.directory.path().join("previous.bsp");
        fs::write(&path, b"previous BSP").unwrap();
        fs::write(path.with_extension("lin"), b"previous LIN").unwrap();
        let control = Control::default();
        let _lease = OutputLease::acquire(&path, &control).unwrap();
        let reservation = WriteReservation::new(&path, &control).unwrap();
        let source = room();
        let original = source.clone();
        assert!(
            crate::compile(&source, &fixture.game).bsp.is_some(),
            "{material}"
        );
        let output = crate::compile_with_options(&source, &fixture.game, options());
        assert!(
            output.bsp.is_none() && output.report.has_errors(),
            "{material}"
        );
        assert!(
            output
                .report
                .diagnostics
                .iter()
                .any(|item| item.message.contains(reason)),
            "{:?}",
            output.report
        );
        assert!(output.stats.water_assets.is_none());
        match output.prepare_write_reserved(reservation, &control) {
            Err(_) | Ok(None) => {}
            Ok(Some(_)) => panic!("Unsupported liquid packing was prepared for publication"),
        }
        assert_eq!(source, original);
        assert_eq!(fs::read(&path).unwrap(), b"previous BSP");
        assert_eq!(
            fs::read(path.with_extension("lin")).unwrap(),
            b"previous LIN"
        );
    }
}

#[test]
fn water_publication_keeps_original_sources_through_preparation_and_commit() {
    for material in [WATER, SLIME_PATCH, SLIME_STATIC_ENVMAP] {
        for after_preparation in [false, true] {
            let fixture = fixture();
            fixture.liquid(material);
            let path = fixture.directory.path().join("previous.bsp");
            fs::write(&path, b"previous BSP").unwrap();
            fs::write(path.with_extension("lin"), b"previous LIN").unwrap();
            let control = Control::default();
            let lease = OutputLease::acquire(&path, &control).unwrap();
            let reservation = WriteReservation::new(&path, &control).unwrap();
            let output = crate::compile_with_options(&room(), &fixture.game, options());
            assert!(!output.report.has_errors(), "{:?}", output.report);
            if after_preparation {
                let prepared = output
                    .prepare_write_reserved(reservation, &control)
                    .unwrap()
                    .unwrap();
                fixture.write(
                    if material == WATER {
                        NORMAL
                    } else if material == SLIME_STATIC_ENVMAP {
                        CUBE
                    } else {
                        BOTTOM
                    },
                    b"changed original bytes",
                );
                assert!(prepared.commit_checked(&lease, &control).is_err());
            } else {
                fixture.write(
                    if material == WATER {
                        NORMAL
                    } else if material == SLIME_STATIC_ENVMAP {
                        HDR_CUBE
                    } else {
                        "materials/test/overridden.vtf"
                    },
                    b"changed original bytes",
                );
                assert!(
                    output
                        .prepare_write_reserved(reservation, &control)
                        .is_err()
                );
            }
            assert_eq!(fs::read(&path).unwrap(), b"previous BSP");
            assert_eq!(
                fs::read(path.with_extension("lin")).unwrap(),
                b"previous LIN"
            );
        }
    }
}

#[test]
fn reserved_publication_succeeds_without_rebaselining_mount_directories() {
    for material in [WATER, SLIME] {
        let fixture = fixture();
        fixture.liquid(material);
        let path = fixture.directory.path().join("portable.bsp");
        let control = Control::default();
        let lease = OutputLease::acquire(&path, &control).unwrap();
        let reservation = WriteReservation::new(&path, &control).unwrap();
        let output = crate::compile_with_options(&room(), &fixture.game, options());
        assert!(!output.report.has_errors(), "{:?}", output.report);
        let prepared = output
            .prepare_write_reserved(reservation, &control)
            .unwrap()
            .unwrap();
        let sources = crate::asset_sources::capture_output_asset_source_guard(
            &output,
            &fixture.game,
            &control,
        )
        .unwrap();
        prepared
            .commit_checked_with(&lease, &control, || sources.verify_metadata())
            .unwrap();
        let bsp = bsp::Bsp::parse(&fs::read(path).unwrap()).unwrap();
        assert!(read_packed_water(&bsp, &control).unwrap().is_some());
    }
}

#[test]
fn first_cordon_observation_is_reused_by_main_map_loading() {
    let fixture = fixture();
    let mut document = Document::new(room());
    document
        .edit("region", |transaction| {
            compile_region::apply(
                transaction,
                Some(compile_region::Definition {
                    enabled: true,
                    bounds: compile_region::Bounds {
                        mins: DVec3::new(-192.0, -192.0, -16.0),
                        maxs: DVec3::new(192.0, 192.0, 272.0),
                    },
                }),
            )
        })
        .unwrap();
    let control = Control::default();
    let mut report = crate::Report::default();
    let prepared = crate::cordons::prepare_with_material_sources(
        document.vmf(),
        &fixture.game,
        &mut report,
        &control,
        Some(&fixture.sources),
    )
    .unwrap();
    assert!(fixture.sources.is_observed(ROOT).unwrap());
    fixture.write(ROOT, "Water { $envmap test/changed }");
    let error = crate::map::Map::load_with_material_sources_controlled(
        prepared.vmf.as_ref(),
        &fixture.game,
        &mut report,
        &control,
        None,
        &fixture.sources,
    )
    .err()
    .unwrap();
    assert!(error.contains("changed") && error.contains(ROOT), "{error}");
}

#[test]
fn packing_rejects_wider_authored_surface_names_without_repairing_them() {
    let fixture = fixture();
    for name in ["materials/test/root", "test/root.vmt", "test\\root"] {
        let mut source = room();
        for side in source
            .world
            .solids
            .iter_mut()
            .flat_map(|solid| &mut solid.sides)
        {
            if side.material == "TeSt/RoOt" {
                side.material = name.into();
            }
        }
        assert!(
            crate::compile(&source, &fixture.game).bsp.is_some(),
            "{name}"
        );
        let output = crate::compile_with_options(&source, &fixture.game, options());
        assert!(output.bsp.is_none() && output.report.has_errors(), "{name}");
        assert!(
            output
                .report
                .diagnostics
                .iter()
                .any(|item| item.message.contains("Pack liquid assets")
                    && item.message.contains("paths")),
            "{name}: {:?}",
            output.report
        );
    }
}

#[test]
fn dry_build_reports_zero_applicability_without_inventing_assets() {
    let output =
        crate::compile_with_options(&test_common::room(), &test_common::empty_game(), options());
    assert!(!output.report.has_errors(), "{:?}", output.report);
    let plan = output.water_assets.as_ref().unwrap();
    assert!(plan.roots().is_empty() && plan.entries().is_empty());
    let stats = output.stats.water_assets.as_ref().unwrap();
    assert_eq!((stats.roots, stats.files, stats.asset_bytes), (0, 0, 0));
    assert_eq!(
        read_packed_water(output.bsp.as_ref().unwrap(), &Control::default()).unwrap(),
        Some(plan.manifest())
    );
}

#[test]
fn generated_reflection_rebasing_preserves_original_liquid_bundles_without_sources() {
    for material in [WATER, SLIME_PATCH] {
        generated_reflection_rebasing(material);
    }
}

fn generated_reflection_rebasing(material: &str) {
    use crate::asset_sources::hash;
    use crate::cubemap_assets::{self, CompileContext, CubemapAssetPhase};
    use crate::{LightingMode, compile_with_context_controlled};

    let mut fixture = fixture();
    fixture.liquid(material);
    for directory in ["materials/dev", "materials/skybox"] {
        fs::create_dir_all(fixture.directory.path().join(directory)).unwrap();
    }
    fixture.write(
        "materials/dev/dev_measuregeneric01.vmt",
        "LightmappedGeneric { $basetexture test/color }",
    );
    for suffix in ["rt", "bk", "lf", "ft", "up", "dn"] {
        fixture.write(
            &format!("materials/skybox/sky_day01_01{suffix}.vmt"),
            "UnlitGeneric { $basetexture test/color }",
        );
    }
    fixture.write(
        "materials/test/reflection.vmt",
        "LightmappedGeneric { $basetexture test/color $envmap env_cubemap }",
    );
    let mut source = room();
    let side = source
        .world
        .solids
        .iter_mut()
        .flat_map(|solid| &mut solid.sides)
        .find(|side| {
            let plane = Plane::from_points(side.plane.0).unwrap();
            plane.normal.abs_diff_eq(-DVec3::X, 1e-8)
                && side.plane.0.iter().all(|point| point.x == 256.0)
        })
        .unwrap();
    side.material = "test/reflection".into();
    let face_id = side.id;
    let mut document = Document::new(source);
    let helper = document
        .edit("reflection sample", |transaction| {
            edit::create_entity(
                transaction,
                "env_cubemap",
                Some(DVec3::new(192.0, 0.0, 128.0)),
                &[
                    ("cubemapsize".into(), "1".into()),
                    ("sides".into(), face_id.to_string()),
                ],
                Vec::new(),
            )
        })
        .unwrap();
    let ObjectId::Entity(helper_id) = helper else {
        unreachable!()
    };
    let control = Control::default();
    let output = compile_with_context_controlled(
        document.vmf(),
        &fixture.game,
        Options {
            generate_reflections: true,
            lighting: LightingMode::DirectLdr,
            ..options()
        },
        Some(&CompileContext::new("published_water").unwrap()),
        &control,
    );
    assert!(!output.report.has_errors(), "{:?}", output.report);
    let water = output.water_assets.as_ref().unwrap().manifest();
    let mut bsp = output.bsp.unwrap();
    let mut reflections = output.cubemap_assets.unwrap();
    assert_eq!(reflections.phase, CubemapAssetPhase::Pending);
    assert_eq!(reflections.samples.len(), 1);
    assert_eq!(reflections.assignments.len(), 1);
    let assignment = &reflections.assignments[0];
    assert_eq!(
        (assignment.face_id, assignment.helper_id),
        (face_id, helper_id)
    );
    assert!(assignment.manual);
    assert_eq!(assignment.material, "test/reflection");
    assert!(!reflections.aliases.is_empty());
    assert!(
        reflections
            .aliases
            .iter()
            .all(|alias| alias.material == "test/reflection")
    );
    verify_packed_water(&bsp, &water, &control).unwrap();
    let original = bsp.pak_data(bsp::PakLimits::default()).unwrap();
    let mut archive = original.clone();
    for sample in &mut reflections.samples {
        let entry = archive
            .entries
            .iter_mut()
            .find(|entry| entry.name == sample.texture)
            .unwrap();
        entry.data[80..].fill(1);
        sample.sha256 = hash(&entry.data);
    }
    reflections.phase = CubemapAssetPhase::Verified;
    archive
        .entries
        .iter_mut()
        .find(|entry| entry.name == cubemap_assets::MANIFEST_PATH)
        .unwrap()
        .data = serde_json::to_vec(&reflections).unwrap();
    bsp.set_pak_data(&archive, bsp::PakLimits::default())
        .unwrap();
    cubemap_assets::verify_generated_cubemaps(&bsp, &reflections, &control).unwrap();
    fs::remove_dir_all(fixture.directory.path().join("materials")).unwrap();
    fixture.game.mounts.clear();
    assert!(!fixture.game.contains(ROOT).unwrap());
    verify_packed_water(&bsp, &water, &control).unwrap();
    let rebased =
        cubemap_assets::rebase_generated_cubemaps(&bsp.to_bytes(), "private_water", &control)
            .unwrap();
    let rebased = bsp::Bsp::parse(&rebased).unwrap();
    assert_eq!(
        read_packed_water(&rebased, &control).unwrap(),
        Some(water.clone())
    );
    let rebased_archive = rebased.pak_data(bsp::PakLimits::default()).unwrap();
    for path in water
        .assets
        .iter()
        .map(|asset| asset.path.as_str())
        .chain([PACKED_WATER_MANIFEST_PATH])
    {
        let expected = original
            .entries
            .iter()
            .find(|entry| entry.name == path)
            .unwrap();
        let actual = rebased_archive
            .entries
            .iter()
            .find(|entry| entry.name == path)
            .unwrap();
        assert_eq!(
            actual, expected,
            "Water asset changed during reflection rebasing: {path}"
        );
    }
    let rebased_reflections = cubemap_assets::read_generated_cubemaps(&rebased, &control)
        .unwrap()
        .unwrap();
    assert_eq!(rebased_reflections.namespace, "private_water");
    assert_eq!(rebased_reflections.logical_namespace, "published_water");
    assert_eq!(rebased_reflections.phase, CubemapAssetPhase::Verified);
    cubemap_assets::verify_generated_cubemaps(&rebased, &rebased_reflections, &control).unwrap();
}

#[test]
fn sky_and_water_shared_original_texture_collision_preserves_reserved_outputs() {
    let fixture = fixture();
    fs::create_dir_all(fixture.directory.path().join("materials/skybox")).unwrap();
    for suffix in ["rt", "bk", "lf", "ft", "up", "dn"] {
        fixture.write(
            &format!("materials/skybox/collision{suffix}.vmt"),
            "UnlitGeneric { $basetexture test/normal }",
        );
    }
    let mut source = room();
    source.world.properties.set("skyname", "collision");
    let water = crate::compile_with_options(&source, &fixture.game, options());
    assert!(!water.report.has_errors(), "{:?}", water.report);
    assert!(
        water
            .water_assets
            .as_ref()
            .unwrap()
            .entries()
            .iter()
            .any(|entry| entry.name == NORMAL)
    );
    let sky = crate::compile_with_options(
        &source,
        &fixture.game,
        Options {
            pack_sky_assets: true,
            pack_water_assets: false,
            ..options()
        },
    );
    assert!(!sky.report.has_errors(), "{:?}", sky.report);
    assert!(
        sky.sky_assets
            .as_ref()
            .unwrap()
            .entries()
            .iter()
            .any(|entry| entry.name == NORMAL)
    );
    let path = fixture.directory.path().join("previous.bsp");
    fs::write(&path, b"previous BSP").unwrap();
    fs::write(path.with_extension("lin"), b"previous LIN").unwrap();
    let control = Control::default();
    let _lease = OutputLease::acquire(&path, &control).unwrap();
    let reservation = WriteReservation::new(&path, &control).unwrap();
    let output = crate::compile_with_options_controlled(
        &source,
        &fixture.game,
        Options {
            pack_sky_assets: true,
            ..options()
        },
        &control,
    );
    assert!(output.bsp.is_none() && output.report.has_errors());
    assert!(output.stats.sky_assets.is_some());
    assert!(output.stats.water_assets.is_none());
    assert!(
        output.report.diagnostics.iter().any(|item| {
            item.level == crate::Level::Error
                && item.message.contains("Water")
                && item.message.contains("collide")
        }),
        "{:?}",
        output.report
    );
    match output.prepare_write_reserved(reservation, &control) {
        Err(_) | Ok(None) => {}
        Ok(Some(_)) => panic!("Colliding Water/Sky output was prepared for publication"),
    }
    assert_eq!(fs::read(&path).unwrap(), b"previous BSP");
    assert_eq!(
        fs::read(path.with_extension("lin")).unwrap(),
        b"previous LIN"
    );
}
