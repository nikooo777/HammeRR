use std::fs;

use glam::DVec3;
use source_formats::bsp;
use source_formats::vmf::{Entity, Vmf};

use super::tests::{FIRST, FIRST_TEXTURE, Fixture, MODEL, PHY, SECOND, SECOND_TEXTURE, VTX, VVD};
use super::*;
use crate::{Control, LightingMode, Options, OutputLease, WriteReservation};

fn options(lighting: LightingMode) -> Options {
    Options {
        pack_static_prop_assets: true,
        lighting,
        sky_quality: crate::SkyQuality::Fast,
        ..Default::default()
    }
}

fn source(solid: &str) -> Vmf {
    let mut source = crate::test_common::room();
    let id = source.object_ids().chain(source.side_ids()).max().unwrap() + 1;
    let mut prop = Entity::new(id, "prop_static");
    for (key, value) in [("model", MODEL), ("origin", "32 32 8"), ("solid", solid)] {
        prop.properties.set(key, value);
    }
    source.entities.push(prop);
    source
}

#[test]
fn every_lighting_mode_packs_originals_independently_and_only_changes_the_pak() {
    let fixture = Fixture::new();
    let source = source("0");
    for lighting in [
        LightingMode::Fullbright,
        LightingMode::DirectLdr,
        LightingMode::DirectLdrHdr,
    ] {
        let baseline = crate::compile_with_options(
            &source,
            &fixture.game,
            Options {
                lighting,
                sky_quality: crate::SkyQuality::Fast,
                ..Default::default()
            },
        );
        let output = crate::compile_with_options(&source, &fixture.game, options(lighting));
        assert!(!baseline.report.has_errors(), "{:?}", baseline.report);
        assert!(!output.report.has_errors(), "{:?}", output.report);
        let plan = output.static_prop_assets.as_ref().unwrap();
        let stats = output.stats.static_prop_assets.as_ref().unwrap();
        assert_eq!(
            (stats.models, stats.files, stats.asset_bytes),
            (1, 7, plan.asset_bytes())
        );
        assert!(output.prop_lighting_assets.is_none());
        let bsp = output.bsp.as_ref().unwrap();
        let baseline = baseline.bsp.as_ref().unwrap();
        assert_eq!(bsp.game_lumps, baseline.game_lumps);
        for index in 0..bsp::LUMP_COUNT {
            if index != bsp::lump::PAKFILE {
                assert_eq!(bsp.lumps[index], baseline.lumps[index]);
            }
        }
        verify_packed_static_props(bsp, &plan.manifest(), &Control::default()).unwrap();
    }
    let default = crate::compile(&source, &fixture.game);
    let explicit_false = crate::compile_with_options(
        &source,
        &fixture.game,
        Options {
            pack_static_prop_assets: false,
            ..Default::default()
        },
    );
    assert!(default.static_prop_assets.is_none() && explicit_false.static_prop_assets.is_none());
    assert_eq!(
        default.bsp.unwrap().to_bytes(),
        explicit_false.bsp.unwrap().to_bytes()
    );
}

#[test]
fn packing_and_both_vertex_lighting_channels_share_original_sources() {
    let fixture = Fixture::new();
    let output = crate::compile_with_options(
        &source("0"),
        &fixture.game,
        Options {
            bake_static_prop_vertices: true,
            ..options(LightingMode::DirectLdrHdr)
        },
    );
    assert!(!output.report.has_errors(), "{:?}", output.report);
    let plan = output.static_prop_assets.as_ref().unwrap();
    let lighting = output.prop_lighting_assets.as_ref().unwrap();
    let bsp = output.bsp.as_ref().unwrap();
    verify_packed_static_props(bsp, &plan.manifest(), &Control::default()).unwrap();
    crate::prop_lighting_assets::verify_generated_prop_lighting(
        bsp,
        lighting.manifest(),
        &Control::default(),
    )
    .unwrap();
    assert!(plan.entries().iter().any(|entry| entry.name == SECOND));
    let archive = bsp.pak_data(bsp::PakLimits::default()).unwrap();
    assert!(
        archive
            .entries
            .iter()
            .any(|entry| entry.name.ends_with(".vhv"))
    );
    assert!(archive.entries.iter().any(|entry| {
        entry
            .name
            .rsplit('/')
            .next()
            .is_some_and(|name| name.starts_with("sp_hdr_") && name.ends_with(".vhv"))
    }));
}

#[test]
fn publication_refuses_drift_of_every_original_before_or_after_preparation() {
    for after_preparation in [false, true] {
        for path in [
            MODEL,
            VVD,
            VTX,
            FIRST,
            SECOND,
            FIRST_TEXTURE,
            SECOND_TEXTURE,
            PHY,
        ] {
            let fixture = Fixture::new();
            let destination = fixture.directory.path().join("published/previous.bsp");
            fs::write(&destination, b"previous BSP").unwrap();
            fs::write(destination.with_extension("lin"), b"previous LIN").unwrap();
            let control = Control::default();
            let lease = OutputLease::acquire(&destination, &control).unwrap();
            let reservation = WriteReservation::new(&destination, &control).unwrap();
            let output = crate::compile_with_options(
                &source("6"),
                &fixture.game,
                options(LightingMode::Fullbright),
            );
            assert!(!output.report.has_errors(), "{:?}", output.report);
            if after_preparation {
                let prepared = output
                    .prepare_write_reserved(reservation, &control)
                    .unwrap()
                    .unwrap();
                fixture.mutate(path);
                assert!(
                    prepared.commit_checked(&lease, &control).is_err(),
                    "published changed {path}"
                );
            } else {
                fixture.mutate(path);
                assert!(
                    output
                        .prepare_write_reserved(reservation, &control)
                        .is_err(),
                    "prepared changed {path}"
                );
            }
            assert_eq!(fs::read(&destination).unwrap(), b"previous BSP");
            assert_eq!(
                fs::read(destination.with_extension("lin")).unwrap(),
                b"previous LIN"
            );
        }
    }
}

#[test]
fn hidden_and_region_omitted_props_never_become_packing_roots() {
    for region in [false, true] {
        let fixture = Fixture::new();
        let mut source = source("0");
        let mut omitted = source.entities.last().unwrap().clone();
        omitted.id += 1;
        omitted.properties.set("model", "models/props/unread.mdl");
        omitted.properties.set("solid", "6");
        if region {
            omitted.properties.set("origin", "192 32 8");
            fs::create_dir_all(fixture.directory.path().join("materials/tools")).unwrap();
            fixture.write("materials/tools/toolsblack.vmt", "LightmappedGeneric {}");
        } else {
            omitted.hidden = true;
            omitted.properties.set("origin", "NaN 0 0");
        }
        source.entities.push(omitted);
        fixture.write("models/props/unread.mdl", b"invalid omitted model");
        fixture.write("models/props/unread.phy", b"invalid omitted physics");
        if region {
            let mut document = map_core::Document::new(source);
            document
                .edit("region", |transaction| {
                    map_core::compile_region::apply(
                        transaction,
                        Some(map_core::compile_region::Definition {
                            enabled: true,
                            bounds: map_core::compile_region::Bounds {
                                mins: DVec3::new(-128.0, -128.0, 0.0),
                                maxs: DVec3::new(128.0, 128.0, 256.0),
                            },
                        }),
                    )
                })
                .unwrap();
            source = document.into_vmf();
        }
        let output =
            crate::compile_with_options(&source, &fixture.game, options(LightingMode::Fullbright));
        assert!(!output.report.has_errors(), "{:?}", output.report);
        let plan = output.static_prop_assets.as_ref().unwrap();
        assert_eq!(plan.roots(), &[MODEL]);
        assert_eq!(plan.uses().len(), 1);
        fixture.write("models/props/unread.mdl", b"changed unread model");
        fixture.write("models/props/unread.phy", b"changed unread physics");
        plan.verify_sources(&fixture.game, &Control::default())
            .unwrap();
    }
}

#[test]
fn zero_emitted_props_still_produce_a_valid_empty_plan() {
    let fixture = Fixture::new();
    let output = crate::compile_with_options(
        &crate::test_common::room(),
        &fixture.game,
        options(LightingMode::Fullbright),
    );
    assert!(!output.report.has_errors(), "{:?}", output.report);
    let plan = output.static_prop_assets.as_ref().unwrap();
    assert!(plan.roots().is_empty() && plan.entries().is_empty());
    let stats = output.stats.static_prop_assets.as_ref().unwrap();
    assert_eq!((stats.models, stats.files, stats.asset_bytes), (0, 0, 0));
    verify_packed_static_props(
        output.bsp.as_ref().unwrap(),
        &plan.manifest(),
        &Control::default(),
    )
    .unwrap();
}
