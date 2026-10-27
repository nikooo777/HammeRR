use std::fs;
use std::io;

use glam::DVec3;
use map_core::{Document, ObjectId, Plane, edit};
use native_runtime::{
    CaptureChannel, CaptureEnvironment, CaptureObserver, CaptureReceipt, CaptureRequest,
};
use source_formats::{bsp, vmf::Vmf};

use super::build_tests::{fixture, room};
use super::tests::{Fixture, ROOT, SLIME_PATCH, texture};
use super::*;
use crate::cubemap_assets::{self, CompileContext, CubemapAssetPhase};
use crate::{Control, LightingMode, Options, Output, OutputLease, WriteReservation};

const RUNTIME_SLIME: &str = "LightmappedGeneric { %compileslime 1 $bottommaterial test/root $basetexture test/color $bumpmap test/normal $envmap env_cubemap }";

fn scene(patched: bool) -> (Fixture, Vmf) {
    let fixture = fixture();
    if patched {
        fixture.liquid(SLIME_PATCH);
        fixture.write(
            ROOT,
            SLIME_PATCH.replace("$envmap test/cube", "$envmap env_cubemap"),
        );
    } else {
        fixture.write(ROOT, RUNTIME_SLIME);
    }
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
    let mut source = room();
    let pool = source
        .world
        .solids
        .iter_mut()
        .find(|solid| solid.sides.iter().any(|side| side.material == "TeSt/RoOt"))
        .unwrap();
    let top = pool
        .sides
        .iter()
        .find(|side| Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
        .unwrap()
        .id;
    for side in &mut pool.sides {
        side.material = "test/root".into();
    }
    let mut document = Document::new(source);
    for (origin, sides) in [
        (DVec3::new(0.0, -192.0, 128.0), top.to_string()),
        (DVec3::new(0.0, 0.0, 32.0), String::new()),
    ] {
        let helper = document
            .edit("reflection sample", |transaction| {
                edit::create_entity(
                    transaction,
                    "env_cubemap",
                    Some(origin),
                    &[("cubemapsize".into(), "1".into()), ("sides".into(), sides)],
                    Vec::new(),
                )
            })
            .unwrap();
        assert!(matches!(helper, ObjectId::Entity(_)));
    }
    document
        .edit("fixture light", |transaction| {
            edit::create_entity(
                transaction,
                "light",
                Some(DVec3::new(0.0, 0.0, 192.0)),
                &[("_light".into(), "255 255 255 200".into())],
                Vec::new(),
            )
        })
        .unwrap();
    (fixture, document.vmf().clone())
}

fn pending(fixture: &Fixture, source: &Vmf, lighting: LightingMode, reflections: bool) -> Output {
    crate::compile_with_context_controlled(
        source,
        &fixture.game,
        Options {
            pack_water_assets: true,
            generate_reflections: reflections,
            lighting,
            ..Default::default()
        },
        Some(&CompileContext::new("runtime_slime").unwrap()),
        &Control::default(),
    )
}

fn capture(
    request: CaptureRequest,
    observer: &mut dyn CaptureObserver,
) -> io::Result<CaptureReceipt> {
    observer.checkpoint()?;
    let staged = native_runtime::capture_staging_bsp_for_channel(&request.bsp, request.channel)?;
    let mut bsp = bsp::Bsp::parse(&staged)?;
    let mut archive = bsp.pak_data(bsp::PakLimits::default())?;
    for sample in &request.samples {
        archive
            .entries
            .iter_mut()
            .find(|entry| entry.name == sample.texture)
            .unwrap()
            .data = texture(true, request.channel == CaptureChannel::Hdr);
    }
    bsp.set_pak_data(&archive, bsp::PakLimits::default())?;
    Ok(CaptureReceipt {
        channel: request.channel,
        staged_bsp: staged,
        bsp: bsp.to_bytes(),
        artifacts: "/unused-fake-slime-capture".into(),
        elapsed_ms: 1,
        verified_controls: true,
        preservation_passed: true,
    })
}

fn finalize(
    output: Output,
    fixture: &Fixture,
    control: &Control,
    capture: impl FnMut(CaptureRequest, &mut dyn CaptureObserver) -> io::Result<CaptureReceipt>,
) -> io::Result<Output> {
    cubemap_assets::finalize_reflections_with_capture(
        output,
        &CompileContext::new("runtime_slime").unwrap(),
        &fixture.game,
        &CaptureEnvironment {
            engine_root: fixture.directory.path().into(),
            steam_root: fixture.directory.path().into(),
            mounts: Vec::new(),
        },
        control,
        capture,
    )
}

#[test]
fn runtime_slime_keeps_original_closure_and_sample_bindings_through_capture_and_rebase() {
    for patched in [false, true] {
        let (mut fixture, source) = scene(patched);
        let output = pending(&fixture, &source, LightingMode::DirectLdrHdr, true);
        assert!(!output.report.has_errors(), "{:?}", output.report);
        let plan = output.water_assets.as_ref().unwrap();
        assert_eq!(plan.entries().len(), if patched { 7 } else { 3 });
        for entry in plan.entries() {
            assert_eq!(
                entry.data,
                fs::read(fixture.directory.path().join(&entry.name)).unwrap()
            );
        }
        let water = plan.manifest();
        let manifest = output.cubemap_assets.as_ref().unwrap();
        assert_eq!(manifest.samples.len(), 2);
        assert_eq!(manifest.hdr_samples.len(), 2);
        assert!(
            manifest
                .assignments
                .iter()
                .any(|assignment| assignment.manual)
        );
        assert!(
            manifest
                .assignments
                .iter()
                .any(|assignment| !assignment.manual)
        );
        assert!(
            manifest
                .aliases
                .iter()
                .all(|alias| alias.material == "test/root")
        );
        assert!(!manifest.texinfo_bindings.is_empty() && !manifest.face_bindings.is_empty());
        let old_lumps = output.bsp.as_ref().unwrap().lumps.clone();
        let published = tempfile::tempdir().unwrap();
        let destination = published.path().join("runtime_slime.bsp");
        fs::write(&destination, b"previous BSP").unwrap();
        fs::write(destination.with_extension("lin"), b"previous LIN").unwrap();
        assert!(
            output
                .prepare_write_controlled(&destination, &Control::default())
                .is_err()
        );
        assert_eq!(fs::read(&destination).unwrap(), b"previous BSP");
        let output = finalize(output, &fixture, &Control::default(), capture).unwrap();
        assert_eq!(
            output.cubemap_assets.as_ref().unwrap().phase,
            CubemapAssetPhase::Verified
        );
        let bsp = output.bsp.as_ref().unwrap();
        for (index, original) in old_lumps.iter().enumerate() {
            if index != bsp::lump::PAKFILE {
                assert_eq!(&bsp.lumps[index], original);
            }
        }
        fs::remove_dir_all(fixture.directory.path().join("materials")).unwrap();
        fixture.game.mounts.clear();
        verify_packed_water(bsp, &water, &Control::default()).unwrap();
        let rebased = cubemap_assets::rebase_generated_cubemaps(
            &bsp.to_bytes(),
            "private_slime",
            &Control::default(),
        )
        .unwrap();
        let rebased = bsp::Bsp::parse(&rebased).unwrap();
        verify_packed_water(&rebased, &water, &Control::default()).unwrap();
        let mut archive = rebased.pak_data(bsp::PakLimits::default()).unwrap();
        let mut missing_root = water.clone();
        missing_root.roots.clear();
        missing_root.assets.retain(|asset| asset.path != ROOT);
        archive.entries.retain(|entry| entry.name != ROOT);
        archive
            .entries
            .iter_mut()
            .find(|entry| entry.name == PACKED_WATER_MANIFEST_PATH)
            .unwrap()
            .data = serde_json::to_vec(&missing_root).unwrap();
        let mut missing = rebased.clone();
        missing
            .set_pak_data(&archive, bsp::PakLimits::default())
            .unwrap();
        cubemap_assets::read_generated_cubemaps(&missing, &Control::default()).unwrap();
        let error = read_packed_water(&missing, &Control::default())
            .unwrap_err()
            .to_string();
        assert!(error.contains("registered original liquid root"), "{error}");

        let mut archive = rebased.pak_data(bsp::PakLimits::default()).unwrap();
        let mut reflections =
            cubemap_assets::read_generated_cubemaps(&rebased, &Control::default())
                .unwrap()
                .unwrap();
        let alias = &mut reflections.aliases[0];
        let path = source_formats::vmt::material_path(&alias.alias);
        let entry = archive
            .entries
            .iter_mut()
            .find(|entry| entry.name == path)
            .unwrap();
        let mut nodes =
            source_formats::keyvalues::parse(std::str::from_utf8(&entry.data).unwrap()).unwrap();
        if let source_formats::keyvalues::Value::Block(parameters) = &mut nodes[0].value {
            parameters.retain(|node| !node.key.eq_ignore_ascii_case("$envmap"));
        } else {
            unreachable!();
        }
        entry.data = source_formats::keyvalues::write(
            &nodes,
            &source_formats::keyvalues::WriteOptions::default(),
        )
        .into_bytes();
        alias.sha256 = crate::asset_sources::hash(&entry.data);
        alias.envmaps.clear();
        archive
            .entries
            .iter_mut()
            .find(|entry| entry.name == cubemap_assets::MANIFEST_PATH)
            .unwrap()
            .data = serde_json::to_vec(&reflections).unwrap();
        let mut omitted = rebased.clone();
        omitted
            .set_pak_data(&archive, bsp::PakLimits::default())
            .unwrap();
        cubemap_assets::read_generated_cubemaps(&omitted, &Control::default()).unwrap();
        let error = read_packed_water(&omitted, &Control::default())
            .unwrap_err()
            .to_string();
        assert!(error.contains("assigned reflection sample"), "{error}");

        let mut archive = rebased.pak_data(bsp::PakLimits::default()).unwrap();
        archive
            .entries
            .retain(|entry| !entry.name.ends_with(".hdr.vtf"));
        let mut missing = rebased.clone();
        missing
            .set_pak_data(&archive, bsp::PakLimits::default())
            .unwrap();
        assert!(read_packed_water(&missing, &Control::default()).is_err());
    }
}

#[test]
fn runtime_slime_requires_dual_generation_and_original_source_custody() {
    for (lighting, reflections) in [
        (LightingMode::DirectLdr, false),
        (LightingMode::DirectLdrHdr, false),
        (LightingMode::DirectLdr, true),
    ] {
        let (fixture, source) = scene(false);
        let output = pending(&fixture, &source, lighting, reflections);
        assert!(output.bsp.is_none() && output.report.has_errors());
        assert!(
            output
                .report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("runtime env_cubemap requires")),
            "{:?}",
            output.report
        );
    }
    for (after_capture, during_capture) in [(false, false), (false, true), (true, false)] {
        let (fixture, source) = scene(true);
        let path = fixture.directory.path().join("runtime_slime.bsp");
        fs::write(&path, b"previous BSP").unwrap();
        fs::write(path.with_extension("lin"), b"previous LIN").unwrap();
        let control = Control::default();
        let lease = OutputLease::acquire(&path, &control).unwrap();
        let reservation = WriteReservation::new(&path, &control).unwrap();
        let output = pending(&fixture, &source, LightingMode::DirectLdrHdr, true);
        assert!(!output.report.has_errors(), "{:?}", output.report);
        if after_capture {
            let output = finalize(output, &fixture, &control, capture).unwrap();
            let prepared = output
                .prepare_write_reserved(reservation, &control)
                .unwrap()
                .unwrap();
            fixture.write(
                "materials/test/overridden.vtf",
                b"changed overridden original texture",
            );
            assert!(prepared.commit_checked(&lease, &control).is_err());
        } else {
            if !during_capture {
                fixture.write(
                    "materials/test/normal.vtf",
                    b"changed secondary original texture",
                );
            }
            let mut calls = 0;
            let result = finalize(output, &fixture, &control, |request, observer| {
                calls += 1;
                let receipt = capture(request, observer)?;
                if during_capture && calls == 1 {
                    fixture.write(
                        "materials/test/overridden.vtf",
                        b"changed overridden original texture during capture",
                    );
                }
                Ok(receipt)
            });
            assert!(result.is_err());
            assert_eq!(calls == 0, !during_capture);
        }
        assert_eq!(fs::read(&path).unwrap(), b"previous BSP");
        assert_eq!(
            fs::read(path.with_extension("lin")).unwrap(),
            b"previous LIN"
        );
    }
}
