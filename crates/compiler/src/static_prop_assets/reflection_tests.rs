use std::fs;

use source_formats::bsp::{self, Bsp, PakEntry, PakLimits};
use source_formats::vmf::Entity;

use super::tests::{FIRST, Fixture, MODEL};
use super::{PACKED_STATIC_PROPS_MANIFEST_PATH, verify_packed_static_props};
use crate::cubemap_assets::{self, CompileContext, CubemapAssetPhase};
use crate::prop_lighting_assets;
use crate::{Control, LightingMode, Options, compile_with_context_controlled};

fn assert_preserved(bsp: &Bsp, expected: &[PakEntry]) {
    let archive = bsp.pak_data(PakLimits::default()).unwrap();
    for original in expected {
        let actual = archive
            .entries
            .iter()
            .find(|entry| entry.name == original.name)
            .unwrap();
        assert_eq!(
            actual, original,
            "Original packed entry changed: {}",
            original.name
        );
    }
}

#[test]
fn pending_reflection_generation_suppresses_prop_capture_guidance() {
    let fixture = Fixture::new();
    for directory in ["materials/dev", "materials/skybox"] {
        fs::create_dir_all(fixture.directory.path().join(directory)).unwrap();
    }
    fixture.write(
        "materials/dev/dev_measuregeneric01.vmt",
        "LightmappedGeneric { $basetexture test/one $envmap env_cubemap }",
    );
    fixture.write(
        FIRST,
        "VertexLitGeneric { $basetexture test/one $envmap env_cubemap }",
    );
    for suffix in ["rt", "bk", "lf", "ft", "up", "dn"] {
        fixture.write(
            &format!("materials/skybox/sky_day01_01{suffix}.vmt"),
            "UnlitGeneric { $basetexture test/one }",
        );
    }
    let (mut source, _) = cubemap_assets::tests::source();
    let id = source.object_ids().chain(source.side_ids()).max().unwrap() + 1;
    let mut prop = Entity::new(id, "prop_static");
    for (key, value) in [("model", MODEL), ("origin", "32 32 8"), ("solid", "0")] {
        prop.properties.set(key, value);
    }
    source.entities.push(prop);
    for lighting in [LightingMode::DirectLdr, LightingMode::DirectLdrHdr] {
        for generate_reflections in [false, true] {
            let pending = compile_with_context_controlled(
                &source,
                &fixture.game,
                Options {
                    generate_reflections,
                    lighting,
                    sky_quality: crate::SkyQuality::Fast,
                    ..Default::default()
                },
                Some(&CompileContext::new("prop_reflection_guidance").unwrap()),
                &Control::default(),
            );
            assert!(!pending.report.has_errors(), "{:?}", pending.report);
            let warned = pending.report.diagnostics.iter().any(|diagnostic| {
                diagnostic.object.as_deref() == Some(format!("entity:{id}").as_str())
                    && diagnostic.message.contains("declares $envmap env_cubemap")
            });
            assert_eq!(warned, !generate_reflections, "{:?}", pending.report);
            if generate_reflections {
                assert_eq!(
                    pending.cubemap_assets.as_ref().unwrap().phase,
                    CubemapAssetPhase::Pending
                );
            }
        }
    }
}

#[test]
fn reflection_merge_and_run_rebase_preserve_original_props_and_vertex_lighting() {
    let fixture = Fixture::new();
    for directory in ["materials/dev", "materials/skybox"] {
        fs::create_dir_all(fixture.directory.path().join(directory)).unwrap();
    }
    fixture.write(
        "materials/dev/dev_measuregeneric01.vmt",
        "LightmappedGeneric { $basetexture test/one $envmap env_cubemap }",
    );
    for suffix in ["rt", "bk", "lf", "ft", "up", "dn"] {
        fixture.write(
            &format!("materials/skybox/sky_day01_01{suffix}.vmt"),
            "UnlitGeneric { $basetexture test/one }",
        );
    }
    let (mut source, _) = cubemap_assets::tests::source();
    let id = source.object_ids().chain(source.side_ids()).max().unwrap() + 1;
    let mut prop = Entity::new(id, "prop_static");
    for (key, value) in [("model", MODEL), ("origin", "32 32 8"), ("solid", "0")] {
        prop.properties.set(key, value);
    }
    source.entities.push(prop);
    let control = Control::default();
    let pending = compile_with_context_controlled(
        &source,
        &fixture.game,
        Options {
            pack_static_prop_assets: true,
            bake_static_prop_vertices: true,
            generate_reflections: true,
            lighting: LightingMode::DirectLdr,
            sky_quality: crate::SkyQuality::Fast,
            ..Default::default()
        },
        Some(&CompileContext::new("packed_prop_reflections").unwrap()),
        &control,
    );
    assert!(!pending.report.has_errors(), "{:?}", pending.report);
    let plan = pending.static_prop_assets.as_ref().unwrap();
    assert_eq!(plan.roots(), &[MODEL]);
    assert_eq!(plan.entries().len(), 7);
    let original_manifest = plan.manifest();
    let lighting_manifest = pending
        .prop_lighting_assets
        .as_ref()
        .unwrap()
        .manifest()
        .clone();
    assert!(!lighting_manifest.assets.is_empty());
    let before = pending.bsp.as_ref().unwrap().clone();
    let archive = before.pak_data(PakLimits::default()).unwrap();
    assert_preserved(&before, plan.entries());
    let mut preserved = plan.entries().to_vec();
    for path in [
        PACKED_STATIC_PROPS_MANIFEST_PATH,
        prop_lighting_assets::MANIFEST_PATH,
    ]
    .into_iter()
    .chain(
        lighting_manifest
            .assets
            .iter()
            .map(|asset| asset.path.as_str()),
    ) {
        preserved.push(
            archive
                .entries
                .iter()
                .find(|entry| entry.name == path)
                .unwrap()
                .clone(),
        );
    }
    assert_eq!(
        pending.cubemap_assets.as_ref().unwrap().phase,
        CubemapAssetPhase::Pending
    );
    let merged = cubemap_assets::tests::merge(pending, &fixture.game);
    assert!(!merged.report.has_errors(), "{:?}", merged.report);
    let merged_bsp = merged.bsp.as_ref().unwrap();
    assert_preserved(merged_bsp, &preserved);
    assert_eq!(merged_bsp.game_lumps, before.game_lumps);
    for index in 0..bsp::LUMP_COUNT {
        if index != bsp::lump::PAKFILE {
            assert_eq!(merged_bsp.lumps[index], before.lumps[index]);
        }
    }
    verify_packed_static_props(merged_bsp, &original_manifest, &control).unwrap();
    prop_lighting_assets::verify_generated_prop_lighting(merged_bsp, &lighting_manifest, &control)
        .unwrap();
    assert_eq!(
        merged.static_prop_assets.as_ref().unwrap().manifest(),
        original_manifest
    );
    assert_eq!(
        merged.prop_lighting_assets.as_ref().unwrap().manifest(),
        &lighting_manifest
    );
    let reflections = merged.cubemap_assets.as_ref().unwrap();
    assert_eq!(reflections.phase, CubemapAssetPhase::Verified);
    assert!(!reflections.samples.is_empty() && !reflections.aliases.is_empty());
    cubemap_assets::verify_generated_cubemaps(merged_bsp, reflections, &control).unwrap();
    fs::remove_dir_all(fixture.directory.path().join("models")).unwrap();
    fs::remove_dir_all(fixture.directory.path().join("materials")).unwrap();
    let rebased = cubemap_assets::rebase_generated_cubemaps(
        &merged_bsp.to_bytes(),
        "packed_prop_private_run",
        &control,
    )
    .unwrap();
    let rebased = Bsp::parse(&rebased).unwrap();
    assert_preserved(&rebased, &preserved);
    assert_eq!(rebased.game_lumps, merged_bsp.game_lumps);
    verify_packed_static_props(&rebased, &original_manifest, &control).unwrap();
    prop_lighting_assets::verify_generated_prop_lighting(&rebased, &lighting_manifest, &control)
        .unwrap();
    let rebased_reflections = cubemap_assets::read_generated_cubemaps(&rebased, &control)
        .unwrap()
        .unwrap();
    assert_eq!(rebased_reflections.phase, CubemapAssetPhase::Verified);
    assert_eq!(rebased_reflections.namespace, "packed_prop_private_run");
    assert_eq!(
        rebased_reflections.logical_namespace,
        "packed_prop_reflections"
    );
    assert_eq!(rebased_reflections.sources, reflections.sources);
    assert_eq!(rebased_reflections.samples.len(), reflections.samples.len());
    for (actual, original) in rebased_reflections.samples.iter().zip(&reflections.samples) {
        assert_eq!(actual.sha256, original.sha256);
        assert_ne!(actual.texture, original.texture);
    }
    cubemap_assets::verify_generated_cubemaps(&rebased, &rebased_reflections, &control).unwrap();
}
