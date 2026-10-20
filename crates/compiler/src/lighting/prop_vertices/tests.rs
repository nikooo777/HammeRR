use super::*;
use crate::{LightingMode, Options, SkyQuality};
use map_core::{Document, edit};
use source_formats::{
    bsp,
    gamefs::{Mount, MountedPath},
    vmt::Material,
};

mod fixtures;
mod source_identity;

fn options(dual: bool) -> Options {
    Options {
        lighting: if dual {
            LightingMode::DirectLdrHdr
        } else {
            LightingMode::DirectLdr
        },
        bake_static_prop_vertices: true,
        sky_quality: SkyQuality::Fast,
        ..Default::default()
    }
}

fn compile(vmf: &Vmf, game: &GameFs, dual: bool) -> crate::Output {
    let output = crate::compile_with_options(vmf, game, options(dual));
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.bsp.is_some());
    output
}

fn file(bsp: &Bsp, name: &str) -> vhv::File {
    let archive = bsp.pak_data(bsp::PakLimits::default()).unwrap();
    let entry = archive
        .entries
        .iter()
        .find(|entry| entry.name == name)
        .unwrap();
    vhv::File::parse(&entry.data, vhv::Limits::default()).unwrap()
}

#[test]
fn native_color_units_gamma_quantization_bgra_and_clipping_have_literal_oracles() {
    for (value, byte) in [
        (0.0, 0),
        (0.01, 0),
        (1.0, 10),
        (4.0, 19),
        (16.0, 36),
        (63.75, 68),
        (255.0, 128),
        (1024.0, 239),
        (1e8, 239),
    ] {
        assert_eq!(
            color::encode(DVec3::splat(value)).unwrap(),
            vhv::Bgra8 {
                blue: byte,
                green: byte,
                red: byte,
                alpha: 255,
            },
            "linear source value {value}"
        );
    }
    assert_eq!(
        color::encode(DVec3::new(256.0, 64.0, 16.0)).unwrap(),
        vhv::Bgra8 {
            blue: 36,
            green: 68,
            red: 128,
            alpha: 255
        }
    );
    assert!(color::encode(DVec3::new(-1.0, 0.0, 0.0)).is_err());
    assert!(color::encode(DVec3::splat(f64::NAN)).is_err());
    assert!(color::encode(DVec3::splat(1e12)).is_err());
}

#[test]
fn model_transform_rotates_normals_and_keeps_the_world_sample_at_the_vertex() {
    let vertex = Vertex {
        position: [1.0, 0.0, 0.0],
        normal: [2.0, 0.0, 0.0],
        uv: [0.0; 2],
    };
    let prop = StaticProp {
        origin: [4.0, 5.0, 6.0],
        angles: [0.0, 90.0, 0.0],
        ..Default::default()
    };
    let sample = sample(&vertex, &Placement::new(&prop)).unwrap();
    assert!(sample.point.abs_diff_eq(DVec3::new(4.0, 6.0, 6.0), 1e-12));
    assert!(sample.normal.abs_diff_eq(DVec3::Y, 1e-12));
    assert!(sample.origin.abs_diff_eq(DVec3::new(4.0, 6.0, 6.0), 1e-12));
}

#[test]
fn dual_channels_preserve_emitted_ordinals_duplicates_and_unused_group_vertices() {
    let (_directory, game) = fixtures::game();
    let vmf = fixtures::scene(true);
    let before = vmf.clone();
    let output = compile(&vmf, &game, true);
    let stats = output.stats.prop_vertex_lighting.as_ref().unwrap();
    assert_eq!(
        (stats.props, stats.models, stats.meshes, stats.vertices),
        (2, 1, 2, 12)
    );
    assert!(stats.ldr_bytes > 0 && stats.hdr_bytes > 0 && stats.work > 0);
    let bsp = output.bsp.as_ref().unwrap();
    assert_eq!(bsp.static_props().unwrap().unwrap().props.len(), 3);
    let manifest =
        crate::prop_lighting_assets::read_generated_prop_lighting(bsp, &Control::default())
            .unwrap()
            .unwrap();
    assert_eq!(
        manifest
            .receivers
            .iter()
            .map(|receiver| receiver.ordinal)
            .collect::<Vec<_>>(),
        [0, 2]
    );
    assert_eq!(manifest.assets.len(), 4);
    assert!(manifest.assets.iter().all(|asset| asset.ordinal != 1));
    let ldr = file(bsp, "sp_0.vhv");
    let hdr = file(bsp, "sp_hdr_0.vhv");
    assert_eq!(ldr.meshes[0].colors.len(), 6);
    let expected = [127, 128, 127, 127, 127, 127];
    for (index, (&byte, (ldr, hdr))) in expected
        .iter()
        .zip(ldr.meshes[0].colors.iter().zip(&hdr.meshes[0].colors))
        .enumerate()
    {
        assert_eq!(
            *ldr,
            vhv::Bgra8 {
                blue: 0,
                green: 0,
                red: byte,
                alpha: 255
            },
            "LDR vertex {index}"
        );
        assert_eq!(
            *hdr,
            vhv::Bgra8 {
                blue: byte,
                green: 0,
                red: 0,
                alpha: 255
            },
            "HDR vertex {index}"
        );
    }
    assert_eq!(
        *bsp.lumps[bsp::lump::MAP_FLAGS].data.first().unwrap() & 3,
        3
    );
    assert!(
        manifest
            .sources
            .iter()
            .any(|source| source.path == "materials/shared/base.vmt")
    );
    assert!(
        manifest
            .sources
            .iter()
            .any(|source| source.path == "materials/models/test/two.vmt")
    );
    assert!(
        manifest
            .missing_sources
            .contains(&"materials/models/test/one.vmt".to_owned())
    );
    assert_eq!(vmf, before);
}

#[test]
fn all_declared_lods_remap_the_same_root_colors_in_distinct_group_order() {
    let (directory, game) = fixtures::game();
    fixtures::add_lod(directory.path());
    let output = compile(&fixtures::scene(false), &game, true);
    let stats = output.stats.prop_vertex_lighting.as_ref().unwrap();
    assert_eq!(
        (stats.props, stats.models, stats.meshes, stats.vertices),
        (1, 1, 2, 10)
    );
    let bsp = output.bsp.as_ref().unwrap();
    for (name, hdr) in [("sp_0.vhv", false), ("sp_hdr_0.vhv", true)] {
        let file = file(bsp, name);
        assert_eq!(
            file.meshes
                .iter()
                .map(|mesh| mesh.layout)
                .collect::<Vec<_>>(),
            [
                vhv::MeshLayout {
                    lod: 0,
                    vertex_count: 6
                },
                vhv::MeshLayout {
                    lod: 1,
                    vertex_count: 4
                },
            ]
        );
        for (mesh, expected) in file
            .meshes
            .iter()
            .zip([vec![127, 128, 127, 127, 127, 127], vec![127, 128, 127, 127]])
        {
            for (color, expected) in mesh.colors.iter().zip(expected) {
                assert_eq!(color.alpha, 255);
                assert_eq!(color.green, 0);
                assert_eq!(
                    (color.red, color.blue),
                    if hdr { (0, expected) } else { (expected, 0) }
                );
            }
        }
    }
}

#[test]
fn native_total_color_mesh_limit_rejects_257_groups_across_lods_before_generation() {
    let (directory, game) = fixtures::game();
    fixtures::groups(directory.path(), 257);
    let output = crate::compile_with_options(&fixtures::scene(false), &game, options(false));
    assert!(output.bsp.is_none());
    assert!(output.stats.prop_vertex_lighting.is_none());
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .any(
                |diagnostic| diagnostic.object.as_deref() == Some("entity:9101")
                    && diagnostic.message.contains("hardware groups")
            ),
        "{:?}",
        output.report
    );
}

#[test]
fn opaque_world_shadow_and_spot_cones_reach_the_vertex_sampler() {
    let (_directory, game) = fixtures::game();
    let vmf = fixtures::scene(false);
    let output = compile(&vmf, &game, false);
    assert_eq!(
        file(output.bsp.as_ref().unwrap(), "sp_0.vhv").meshes[0].colors[1].red,
        128
    );
    let mut document = Document::new(vmf.clone());
    document
        .edit("opaque blocker", |tx| {
            edit::create_block(
                tx,
                DVec3::new(-16.0, -16.0, 70.0),
                DVec3::new(16.0, 16.0, 75.0),
                "dev/dev_measuregeneric01",
            )
        })
        .unwrap();
    let output = compile(document.vmf(), &game, false);
    assert!(
        file(output.bsp.as_ref().unwrap(), "sp_0.vhv").meshes[0]
            .colors
            .iter()
            .all(|color| color.red == 0 && color.green == 0 && color.blue == 0)
    );
    let mut source = vmf;
    let light = source
        .entities
        .iter_mut()
        .find(|entity| entity.classname() == Some("light"))
        .unwrap();
    light.properties.set("classname", "light_spot");
    light.properties.set("pitch", "0");
    light.properties.set("_inner_cone", "10");
    light.properties.set("_cone", "20");
    let output = compile(&source, &game, false);
    assert!(
        file(output.bsp.as_ref().unwrap(), "sp_0.vhv").meshes[0]
            .colors
            .iter()
            .all(|color| color.red == 0)
    );
}

#[test]
fn sky_sun_and_ambient_are_gathered_only_through_visible_sky() {
    let (_directory, game) = fixtures::game();
    for visible in [false, true] {
        let mut vmf = fixtures::scene(false);
        if visible {
            let ceiling = vmf
                .world
                .solids
                .iter_mut()
                .find(|solid| solid.sides.iter().all(|side| side.plane.0[0].z >= 256.0))
                .unwrap();
            let face = ceiling
                .sides
                .iter_mut()
                .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z < -0.99)
                .unwrap();
            face.material = "sky".into();
        }
        let light = vmf
            .entities
            .iter_mut()
            .find(|entity| entity.classname() == Some("light"))
            .unwrap();
        for (key, value) in [
            ("classname", "light_environment"),
            ("pitch", "-90"),
            ("_light", "255 0 0 128"),
            ("_ambient", "0 0 255 64"),
        ] {
            light.properties.set(key, value);
        }
        let output = compile(&vmf, &game, false);
        let colors = file(output.bsp.as_ref().unwrap(), "sp_0.vhv");
        let center = colors.meshes[0].colors[1];
        if visible {
            assert_eq!(center.red, 93);
            assert!(center.blue > 0);
            assert_eq!(center.green, 0);
        } else {
            assert_eq!(
                center,
                vhv::Bgra8 {
                    red: 0,
                    green: 0,
                    blue: 0,
                    alpha: 255
                }
            );
        }
    }
}

#[test]
fn default_off_and_explicit_opt_out_produce_no_vhv_or_prop_map_flags() {
    let (_directory, game) = fixtures::game();
    let vmf = fixtures::scene(false);
    let mut disabled = options(false);
    disabled.bake_static_prop_vertices = false;
    let off = crate::compile_with_options(&vmf, &game, disabled);
    assert!(!off.report.has_errors(), "{:?}", off.report);
    assert!(off.stats.prop_vertex_lighting.is_none());
    let off_bsp = off.bsp.unwrap();
    assert!(
        crate::prop_lighting_assets::read_generated_prop_lighting(&off_bsp, &Control::default())
            .unwrap()
            .is_none()
    );
    let mut source = vmf;
    source
        .entities
        .iter_mut()
        .find(|entity| entity.classname() == Some("prop_static"))
        .unwrap()
        .properties
        .set("disablevertexlighting", "1");
    let opted_out = compile(&source, &game, false);
    assert!(opted_out.stats.prop_vertex_lighting.is_none());
    assert!(
        crate::prop_lighting_assets::read_generated_prop_lighting(
            opted_out.bsp.as_ref().unwrap(),
            &Control::default()
        )
        .unwrap()
        .is_none()
    );
    let repeated = crate::compile_with_options(&fixtures::scene(false), &game, disabled);
    assert_eq!(off_bsp.to_bytes(), repeated.bsp.unwrap().to_bytes());
}

#[test]
fn unsupported_flags_styles_and_selected_skin_fail_with_source_diagnostics() {
    let (directory, game) = fixtures::game();
    for (key, value) in [("ignorenormals", "1"), ("mindxlevel", "80")] {
        let mut vmf = fixtures::scene(false);
        vmf.entities
            .iter_mut()
            .find(|entity| entity.classname() == Some("prop_static"))
            .unwrap()
            .properties
            .set(key, value);
        let output = crate::compile_with_options(&vmf, &game, options(false));
        assert!(output.bsp.is_none(), "{key}");
        assert!(
            output
                .report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.object.as_deref() == Some("entity:9101")),
            "{key}: {:?}",
            output.report
        );
    }
    let mut styled = fixtures::scene(false);
    styled
        .entities
        .iter_mut()
        .find(|entity| entity.classname() == Some("light"))
        .unwrap()
        .properties
        .set("style", "1");
    let output = crate::compile_with_options(&styled, &game, options(false));
    assert!(output.bsp.is_none());
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("style-0"))
    );
    std::fs::write(
        directory.path().join("materials/models/test/two.vmt"),
        "VertexLitGeneric { $basetexture test $bumpmap normal }",
    )
    .unwrap();
    let mut vmf = fixtures::scene(false);
    vmf.entities
        .iter_mut()
        .find(|entity| entity.classname() == Some("prop_static"))
        .unwrap()
        .properties
        .set("skin", "1");
    let output = crate::compile_with_options(&vmf, &game, options(false));
    assert!(output.bsp.is_none());
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("$bumpmap"))
    );
    for text in [
        "UnlitGeneric {}",
        "VertexLitGeneric { $translucent 1 }",
        "VertexLitGeneric { $halflambert 1 }",
        "VertexLitGeneric { Proxies {} }",
    ] {
        assert!(assets::validate_material(&Material::parse(text).unwrap()).is_err());
    }
    assert!(
        assets::validate_material(&Material::parse("VertexLitGeneric { $nocull 1 }").unwrap())
            .is_ok()
    );
    let path = directory.path().join("models/props/test.mdl");
    let mut mdl = std::fs::read(&path).unwrap();
    mdl[377] = 255;
    std::fs::write(path, mdl).unwrap();
    let output = crate::compile_with_options(&fixtures::scene(false), &game, options(false));
    assert!(output.bsp.is_none());
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("authored MDL root LOD 0"))
    );
}

#[test]
fn prepared_write_rejects_changed_patch_or_new_higher_priority_candidate_after_output_drop() {
    for change_patch in [true, false] {
        let (directory, game) = fixtures::game();
        let output_directory = tempfile::tempdir_in(directory.path().parent().unwrap()).unwrap();
        let destination = output_directory.path().join("previous.bsp");
        std::fs::write(&destination, b"previous complete output").unwrap();
        let output = compile(&fixtures::scene(false), &game, false);
        let control = Control::default();
        let prepared = output
            .prepare_write_controlled(&destination, &control)
            .unwrap()
            .unwrap();
        if change_patch {
            std::fs::write(
                directory.path().join("materials/shared/base.vmt"),
                "VertexLitGeneric { $basetexture replacement }",
            )
            .unwrap();
        } else {
            std::fs::write(
                directory.path().join("materials/models/test/one.vmt"),
                "VertexLitGeneric { $basetexture appeared }",
            )
            .unwrap();
        }
        drop(output);
        assert!(prepared.commit(&control).is_err());
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            b"previous complete output"
        );
    }
}

fn overwrite(path: &std::path::Path, offset: u64, data: &[u8]) {
    use std::io::{Seek, SeekFrom, Write};

    let mut file = std::fs::OpenOptions::new().write(true).open(path).unwrap();
    file.seek(SeekFrom::Start(offset)).unwrap();
    file.write_all(data).unwrap();
    file.sync_all().unwrap();
}

#[test]
fn disabled_receiver_caster_companions_patch_and_alpha_are_guarded_after_output_drop() {
    for change_texture in [false, true] {
        let (directory, game) = fixtures::game();
        fixtures::caster(directory.path(), 0);
        let vmf = fixtures::caster_scene();
        let transparent = compile(&vmf, &game, false);
        assert_eq!(
            file(transparent.bsp.as_ref().unwrap(), "sp_0.vhv").meshes[0].colors[1].red,
            128
        );
        drop(transparent);
        std::fs::write(
            directory.path().join("materials/casters/mask.vtf"),
            fixtures::caster_texture(255),
        )
        .unwrap();
        let output_directory = tempfile::tempdir_in(directory.path().parent().unwrap()).unwrap();
        let destination = output_directory.path().join("previous.bsp");
        std::fs::write(&destination, b"previous complete output").unwrap();
        let output = compile(&vmf, &game, false);
        let bsp = output.bsp.as_ref().unwrap();
        assert!(
            file(bsp, "sp_0.vhv").meshes[0]
                .colors
                .iter()
                .all(|color| color.red == 0)
        );
        let manifest =
            crate::prop_lighting_assets::read_generated_prop_lighting(bsp, &Control::default())
                .unwrap()
                .unwrap();
        assert_eq!(manifest.receivers.len(), 1);
        assert_eq!(manifest.receivers[0].ordinal, 0);
        assert!(manifest.assets.iter().all(|asset| asset.ordinal == 0));
        for path in [
            "models/casters/test.mdl",
            "models/casters/test.vvd",
            "models/casters/test.dx90.vtx",
            "materials/casters/cast.vmt",
            "materials/casters/base.vmt",
            "materials/casters/mask.vtf",
        ] {
            assert!(
                manifest.sources.iter().any(|source| source.path == path),
                "{path}"
            );
        }
        let control = Control::default();
        let prepared = output
            .prepare_write_controlled(&destination, &control)
            .unwrap()
            .unwrap();
        if change_texture {
            overwrite(
                &directory.path().join("materials/casters/mask.vtf"),
                0,
                &fixtures::caster_texture(0),
            );
        } else {
            overwrite(
                &directory.path().join("models/casters/test.vvd"),
                80,
                &(-8.0_f32).to_le_bytes(),
            );
        }
        drop(output);
        assert!(prepared.commit(&control).is_err());
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            b"previous complete output"
        );
    }
}

#[test]
fn prepared_write_guards_original_world_patch_and_missing_material_reads() {
    for present in [true, false] {
        let (directory, game) = fixtures::game();
        std::fs::create_dir_all(directory.path().join("materials/world_guard")).unwrap();
        if present {
            std::fs::write(
                directory.path().join("materials/world_guard/surface.vmt"),
                "Patch { include \"materials/world_guard/base.vmt\" }",
            )
            .unwrap();
            std::fs::write(
                directory.path().join("materials/world_guard/base.vmt"),
                "LightmappedGeneric { %compilesky 0 }",
            )
            .unwrap();
        }
        let mut vmf = fixtures::scene(false);
        for side in vmf
            .world
            .solids
            .iter_mut()
            .flat_map(|solid| &mut solid.sides)
        {
            side.material = "world_guard/surface".into();
        }
        let output_directory = tempfile::tempdir_in(directory.path().parent().unwrap()).unwrap();
        let destination = output_directory.path().join("previous.bsp");
        std::fs::write(&destination, b"previous complete output").unwrap();
        let output = compile(&vmf, &game, false);
        let manifest = crate::prop_lighting_assets::read_generated_prop_lighting(
            output.bsp.as_ref().unwrap(),
            &Control::default(),
        )
        .unwrap()
        .unwrap();
        if present {
            for path in [
                "materials/world_guard/surface.vmt",
                "materials/world_guard/base.vmt",
            ] {
                assert!(
                    manifest.sources.iter().any(|source| source.path == path),
                    "{path}"
                );
            }
        } else {
            assert!(
                manifest
                    .missing_sources
                    .iter()
                    .any(|path| path == "materials/world_guard/surface.vmt")
            );
        }
        let control = Control::default();
        let prepared = output
            .prepare_write_controlled(&destination, &control)
            .unwrap()
            .unwrap();
        if present {
            overwrite(
                &directory.path().join("materials/world_guard/base.vmt"),
                0,
                b"LightmappedGeneric { %compilesky 1 }",
            );
        } else {
            std::fs::write(
                directory.path().join("materials/world_guard/surface.vmt"),
                "LightmappedGeneric { %compilesky 1 }",
            )
            .unwrap();
        }
        drop(output);
        assert!(prepared.commit(&control).is_err());
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            b"previous complete output"
        );
    }
}

#[test]
fn work_exhaustion_and_cancellation_precede_source_reads() {
    let (_directory, game) = fixtures::game();
    let control = Control::default();
    let mut budget = Budget::new(&control);
    let tracker = super::super::asset_tracking::Tracker::new(
        &game,
        super::super::asset_tracking::SourceReads::default(),
        &mut budget,
    )
    .unwrap();
    let mut assets = assets::Assets::new(tracker);
    budget.consume(100_000_000).unwrap();
    assert!(
        assets
            .required("models/props/test.mdl", &mut budget)
            .is_err()
    );
    let (sources, missing, _) = assets.finish();
    assert!(sources.is_empty() && missing.is_empty());
    let control = Control::default();
    let mut budget = Budget::new(&control);
    let tracker = super::super::asset_tracking::Tracker::new(
        &game,
        super::super::asset_tracking::SourceReads::default(),
        &mut budget,
    )
    .unwrap();
    let mut assets = assets::Assets::new(tracker);
    control.cancel().unwrap();
    assert!(
        assets
            .required("models/props/test.mdl", &mut budget)
            .is_err()
    );
    let (sources, missing, _) = assets.finish();
    assert!(sources.is_empty() && missing.is_empty());
}

#[test]
fn grounded_slanted_and_downward_normals_use_actual_brush_contact() {
    let (directory, game) = fixtures::game();
    let vmf = fixtures::grounded(0.0);
    fixtures::normals(directory.path(), [3.0_f32.sqrt() * 0.5, 0.0, -0.5]);
    let lit = compile(&vmf, &game, false);
    let colors = file(lit.bsp.as_ref().unwrap(), "sp_0.vhv")
        .meshes
        .remove(0)
        .colors;
    assert!(colors.iter().all(|color| color.red > 0 && color.red < 128));
    assert_eq!(colors[0], colors[2]);
    drop(lit);
    fixtures::normals(directory.path(), [0.0, 0.0, -1.0]);
    let dark = compile(&vmf, &game, false);
    assert!(
        file(dark.bsp.as_ref().unwrap(), "sp_0.vhv").meshes[0]
            .colors
            .iter()
            .all(|color| color.red == 0 && color.green == 0 && color.blue == 0)
    );
}

#[test]
fn original_segment_sees_thin_blocker_and_original_burial_is_rejected() {
    let (_directory, game) = fixtures::game();
    let mut document = Document::new(fixtures::scene(false));
    document
        .edit("thin blocker before the former normal bias", |tx| {
            edit::create_block(
                tx,
                DVec3::new(-16.0, -16.0, 50.125),
                DVec3::new(16.0, 16.0, 50.375),
                "dev/dev_measuregeneric01",
            )
        })
        .unwrap();
    let blocked = compile(document.vmf(), &game, false);
    assert!(
        file(blocked.bsp.as_ref().unwrap(), "sp_0.vhv").meshes[0]
            .colors
            .iter()
            .all(|color| color.red == 0)
    );
    let buried = crate::compile_with_options(&fixtures::grounded(-0.25), &game, options(false));
    assert!(buried.bsp.is_none());
    assert!(
        buried.report.diagnostics.iter().any(|diagnostic| {
            diagnostic.object.as_deref() == Some("entity:9101")
                && diagnostic.message.contains("buried")
                && diagnostic.message.contains("-0.250000")
        }),
        "{:?}",
        buried.report
    );
}

#[test]
fn overlapping_other_prop_support_is_rejected_for_both_emitted_owner_orders() {
    for support_first in [false, true] {
        let (directory, game) = fixtures::game();
        fixtures::caster(directory.path(), 255);
        let mut vmf = fixtures::caster_scene();
        self_shadow_flag(&mut vmf, "1");
        vmf.entities
            .iter_mut()
            .find(|entity| entity.id == 9200)
            .unwrap()
            .properties
            .set("origin", "0 0 50");
        if support_first {
            let receiver = vmf
                .entities
                .iter()
                .position(|entity| entity.id == 9101)
                .unwrap();
            let support = vmf
                .entities
                .iter()
                .position(|entity| entity.id == 9200)
                .unwrap();
            vmf.entities.swap(receiver, support);
        }
        let output = crate::compile_with_options(&vmf, &game, options(false));
        assert!(output.bsp.is_none());
        assert!(
            output.report.diagnostics.iter().any(|diagnostic| {
                diagnostic.object.as_deref() == Some("entity:9101")
                    && diagnostic.message.contains("another prop triangle")
                    && diagnostic.message.contains("50.000000")
            }),
            "{:?}",
            output.report
        );
    }
}

fn floor() -> super::super::Occluder {
    hull(DVec3::new(-16.0, -16.0, -8.0), DVec3::new(16.0, 16.0, 0.0))
}

fn hull(mins: DVec3, maxs: DVec3) -> super::super::Occluder {
    let planes = (0..3)
        .flat_map(|axis| {
            let mut normal = DVec3::ZERO;
            normal[axis] = 1.0;
            [
                map_core::Plane {
                    normal,
                    dist: maxs[axis],
                },
                map_core::Plane {
                    normal: -normal,
                    dist: -mins[axis],
                },
            ]
        })
        .collect();
    super::super::Occluder::new(planes, mins, maxs).unwrap()
}

#[test]
fn point_admission_distinguishes_brush_boundary_band_terrain_owners_and_budget() {
    use super::super::{
        props::PropTriangle,
        shadows::{CONTACT_EPSILON, ShadowIndex, Triangle},
    };

    let control = Control::default();
    let mut budget = Budget::new(&control);
    let brushes = ShadowIndex::new(vec![floor()], &mut |_| Ok(())).unwrap();
    let empty_brushes = ShadowIndex::new(Vec::new(), &mut |_| Ok(())).unwrap();
    let empty_terrain = ShadowIndex::new(Vec::<Triangle>::new(), &mut |_| Ok(())).unwrap();
    let empty_props = ShadowIndex::new(Vec::<PropTriangle>::new(), &mut |_| Ok(())).unwrap();
    let mut ctx = receivers::Context {
        lights: &[],
        shadows: &brushes,
        terrain: &empty_terrain,
        props: &empty_props,
        sun_directions: &[],
        ambient_directions: &[],
    };
    for height in [
        -0.5 * CONTACT_EPSILON,
        0.0,
        0.5 * CONTACT_EPSILON,
        2.0 * CONTACT_EPSILON,
    ] {
        validate_point(&ctx, 4, DVec3::Z * height, &mut budget).unwrap();
    }
    assert!(
        validate_point(&ctx, 4, -DVec3::Z * 2.0 * CONTACT_EPSILON, &mut budget)
            .unwrap_err()
            .contains("buried")
    );
    for (point, direction, blocked) in [
        (DVec3::ZERO, DVec3::Z, false),
        (DVec3::ZERO, -DVec3::Z, true),
        (DVec3::ZERO, DVec3::X, true),
        (DVec3::new(16.0, 16.0, 0.0), DVec3::ONE, false),
        (DVec3::new(16.0, 16.0, 0.0), -DVec3::ONE, true),
    ] {
        assert_eq!(
            brushes
                .blocked(point, point + direction, &mut |work| budget.consume(work))
                .unwrap(),
            blocked
        );
    }
    let points = [DVec3::ZERO, DVec3::X * 8.0, DVec3::Y * 8.0];
    let terrain = ShadowIndex::new(vec![Triangle::new(points).unwrap()], &mut |_| Ok(())).unwrap();
    ctx.shadows = &empty_brushes;
    ctx.terrain = &terrain;
    for point in [DVec3::ZERO, DVec3::X * 4.0, DVec3::new(2.0, 2.0, 0.0)] {
        assert!(
            validate_point(&ctx, 4, point, &mut budget)
                .unwrap_err()
                .contains("terrain triangle")
        );
    }
    for height in [-0.5 * CONTACT_EPSILON, 0.5 * CONTACT_EPSILON] {
        assert!(
            validate_point(&ctx, 4, DVec3::new(2.0, 2.0, height), &mut budget)
                .unwrap_err()
                .contains("terrain triangle")
        );
    }
    for height in [-2.0 * CONTACT_EPSILON, 2.0 * CONTACT_EPSILON] {
        validate_point(&ctx, 4, DVec3::new(2.0, 2.0, height), &mut budget).unwrap();
    }
    ctx.terrain = &empty_terrain;
    let mut own = Vec::new();
    for _ in 0..8 {
        let mut triangle = PropTriangle::new(points).unwrap();
        triangle.set_owner(4);
        own.push(triangle);
    }
    let own = ShadowIndex::new(own, &mut |_| Ok(())).unwrap();
    ctx.props = &own;
    validate_point(&ctx, 4, DVec3::ZERO, &mut budget).unwrap();
    assert!(
        validate_point(&ctx, 9, DVec3::ZERO, &mut budget)
            .unwrap_err()
            .contains("another prop triangle")
    );
    for reverse in [false, true] {
        let mut triangles = Vec::new();
        for _ in 0..8 {
            let mut triangle = PropTriangle::new(points).unwrap();
            triangle.set_owner(4);
            triangles.push(triangle);
        }
        let mut support = PropTriangle::new(points).unwrap();
        support.set_owner(9);
        triangles.push(support);
        if reverse {
            triangles.reverse();
        }
        let mixed = ShadowIndex::new(triangles, &mut |_| Ok(())).unwrap();
        let overlap = receivers::Context {
            props: &mixed,
            ..ctx
        };
        assert!(
            validate_point(&overlap, 4, DVec3::ZERO, &mut budget)
                .unwrap_err()
                .contains("another prop triangle")
        );
    }
    budget.consume(100_000_000 - budget.work()).unwrap();
    assert!(validate_point(&ctx, 4, DVec3::ZERO, &mut budget).is_err());
    let control = Control::default();
    let mut budget = Budget::new(&control);
    control.cancel().unwrap();
    assert!(validate_point(&ctx, 4, DVec3::ZERO, &mut budget).is_err());
}

#[test]
fn unknown_cluster_samples_all_lights_and_floor_intervals_decide_contact_visibility() {
    use super::super::{
        PointLight,
        props::PropTriangle,
        shadows::{ShadowIndex, Triangle},
    };

    let light = |origin, color: &str| {
        super::super::bake::Emitter::Point(
            PointLight::from_entity(
                &crate::map::MapEntity {
                    id: 1,
                    classname: "light".into(),
                    origin: Some(origin),
                    brushes: Vec::new(),
                    properties: vec![
                        ("_light".into(), color.into()),
                        ("_constant_attn".into(), "1".into()),
                    ],
                },
                -1,
            )
            .unwrap(),
        )
    };
    let lights = [
        light(DVec3::new(8.0, 0.0, 8.0), "255 0 0 255"),
        light(DVec3::new(0.0, 0.0, -4.0), "0 0 255 255"),
    ];
    let visibility = Visibility {
        clusters: Vec::new(),
        count: 0,
        portals: 0,
        data: Vec::new(),
        sky: Vec::new(),
        work: 0,
        removed_pairs: 0,
        workers: 0,
    };
    let control = Control::default();
    let mut budget = Budget::new(&control);
    let mut candidates = Candidates::new(&lights, &visibility, &mut budget).unwrap();
    let brushes = ShadowIndex::new(vec![floor()], &mut |_| Ok(())).unwrap();
    let empty = ShadowIndex::new(Vec::new(), &mut |_| Ok(())).unwrap();
    let terrain = ShadowIndex::new(Vec::<Triangle>::new(), &mut |_| Ok(())).unwrap();
    let props = ShadowIndex::new(Vec::<PropTriangle>::new(), &mut |_| Ok(())).unwrap();
    let mut ctx = receivers::Context {
        lights: &lights,
        shadows: &brushes,
        terrain: &terrain,
        props: &props,
        sun_directions: &[],
        ambient_directions: &[],
    };
    for normal in [-DVec3::Z, DVec3::new(3.0_f64.sqrt() * 0.5, 0.0, -0.5)] {
        let contact = receivers::direct_optional(
            &ctx,
            &mut candidates,
            None,
            receivers::Sample {
                point: DVec3::ZERO,
                normal,
                origin: DVec3::ZERO,
            },
            false,
            &mut budget,
        )
        .unwrap()[0];
        assert_eq!(contact.z, 0.0);
        if normal == -DVec3::Z {
            assert_eq!(contact, DVec3::ZERO);
        } else {
            assert!(contact.x > 0.0);
        }
        ctx.shadows = &empty;
        let exposed = receivers::direct_optional(
            &ctx,
            &mut candidates,
            None,
            receivers::Sample {
                point: DVec3::ZERO,
                normal,
                origin: DVec3::ZERO,
            },
            false,
            &mut budget,
        )
        .unwrap()[0];
        assert_eq!(exposed.x, contact.x);
        assert!(exposed.z > 0.0);
        ctx.shadows = &brushes;
    }
}

fn self_shadow_flag(vmf: &mut Vmf, value: &str) {
    vmf.entities
        .iter_mut()
        .find(|entity| entity.id == 9101)
        .unwrap()
        .properties
        .set("disableselfshadowing", value);
}

#[test]
fn authored_self_shadow_disable_opens_only_own_point_and_spot_occlusion() {
    let (directory, game) = fixtures::game();
    fixtures::self_shadow_model(directory.path());
    for spot in [false, true] {
        let dual = !spot;
        let mut vmf = fixtures::scene(false);
        if spot {
            let light = vmf
                .entities
                .iter_mut()
                .find(|entity| entity.classname() == Some("light"))
                .unwrap();
            for (key, value) in [
                ("classname", "light_spot"),
                ("pitch", "-90"),
                ("_inner_cone", "30"),
                ("_cone", "45"),
                ("_exponent", "1"),
            ] {
                light.properties.set(key, value);
            }
        }
        let default = compile(&vmf, &game, dual);
        let default_bsp = default.bsp.as_ref().unwrap();
        assert_eq!(file(default_bsp, "sp_0.vhv").meshes[0].colors[1].red, 0);
        self_shadow_flag(&mut vmf, "0");
        let off = compile(&vmf, &game, dual);
        assert_eq!(default_bsp.to_bytes(), off.bsp.as_ref().unwrap().to_bytes());
        drop(default);
        drop(off);
        self_shadow_flag(&mut vmf, "1");
        let enabled = compile(&vmf, &game, dual);
        let bsp = enabled.bsp.as_ref().unwrap();
        assert_eq!(
            bsp.static_props().unwrap().unwrap().props[0].flags & 0x80,
            0x80
        );
        assert_eq!(file(bsp, "sp_0.vhv").meshes[0].colors[1].red, 128);
        if dual {
            let hdr = file(bsp, "sp_hdr_0.vhv").meshes[0].colors[1];
            assert_eq!((hdr.red, hdr.green, hdr.blue), (0, 0, 128));
        }
        let manifest =
            crate::prop_lighting_assets::read_generated_prop_lighting(bsp, &Control::default())
                .unwrap()
                .unwrap();
        assert!(
            manifest
                .sources
                .iter()
                .any(|source| source.path == "models/props/test.dx90.vtx")
        );
    }
}

#[test]
fn self_shadow_disable_applies_to_both_sun_and_sky_ambient_visibility() {
    let (directory, game) = fixtures::game();
    fixtures::self_shadow_model(directory.path());
    let mut vmf = fixtures::scene(false);
    let ceiling = vmf
        .world
        .solids
        .iter_mut()
        .find(|solid| solid.sides.iter().all(|side| side.plane.0[0].z >= 256.0))
        .unwrap();
    ceiling
        .sides
        .iter_mut()
        .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z < -0.99)
        .unwrap()
        .material = "sky".into();
    let light = vmf
        .entities
        .iter_mut()
        .find(|entity| entity.classname() == Some("light"))
        .unwrap();
    for (key, value) in [
        ("classname", "light_environment"),
        ("pitch", "-90"),
        ("_light", "255 0 0 128"),
        ("_ambient", "0 0 255 64"),
    ] {
        light.properties.set(key, value);
    }
    let default = compile(&vmf, &game, false);
    let shadowed = file(default.bsp.as_ref().unwrap(), "sp_0.vhv").meshes[0].colors[1];
    assert_eq!(shadowed.red, 0);
    drop(default);
    self_shadow_flag(&mut vmf, "1");
    let enabled = compile(&vmf, &game, false);
    let open = file(enabled.bsp.as_ref().unwrap(), "sp_0.vhv").meshes[0].colors[1];
    assert_eq!(open.red, 93);
    assert!(open.blue > shadowed.blue);
}

#[test]
fn owner_skips_preserve_coincident_other_props_world_terrain_and_budget_errors() {
    use super::super::{
        props::PropTriangle,
        shadows::{ShadowIndex, Triangle},
    };

    let points = [
        DVec3::new(-16.0, -16.0, 8.0),
        DVec3::new(16.0, -16.0, 8.0),
        DVec3::new(0.0, 16.0, 8.0),
    ];
    let make_prop = |owner| {
        let mut triangle = PropTriangle::new(points).unwrap();
        triangle.set_owner(owner);
        triangle
    };
    let own = ShadowIndex::new(vec![make_prop(7)], &mut |_| Ok(())).unwrap();
    let empty = ShadowIndex::new(Vec::<Triangle>::new(), &mut |_| Ok(())).unwrap();
    let sky = hull(DVec3::new(-64.0, -64.0, 64.0), DVec3::new(64.0, 64.0, 72.0))
        .with_sky(vec![false, false, false, false, false, true])
        .unwrap();
    let world = ShadowIndex::new(vec![sky.clone()], &mut |_| Ok(())).unwrap();
    let opaque = ShadowIndex::new(
        vec![
            sky,
            hull(DVec3::new(-16.0, -16.0, 16.0), DVec3::new(16.0, 16.0, 24.0)),
        ],
        &mut |_| Ok(()),
    )
    .unwrap();
    let terrain = ShadowIndex::new(vec![Triangle::new(points).unwrap()], &mut |_| Ok(())).unwrap();
    let control = Control::default();
    let mut budget = Budget::new(&control);
    let start = DVec3::ZERO;
    let end = DVec3::Z * 32.0;
    assert!(
        own.blocked_skipping_owner(start, end, None, &mut |work| budget.consume(work))
            .unwrap()
    );
    assert!(
        !own.blocked_skipping_owner(start, end, Some(7), &mut |work| budget.consume(work))
            .unwrap()
    );
    assert!(
        own.blocked_skipping_owner(start, end, Some(9), &mut |work| budget.consume(work))
            .unwrap()
    );
    assert!(
        !super::super::bake::sky_visible(DVec3::Z, start, &world, &empty, &own, &None, &mut budget)
            .unwrap()
    );
    assert!(
        super::super::sky_visible_skipping_owner(
            DVec3::Z,
            start,
            &world,
            &empty,
            &own,
            &None,
            Some(7),
            &mut budget
        )
        .unwrap()
    );
    assert!(
        !super::super::sky_visible_skipping_owner(
            DVec3::Z,
            start,
            &opaque,
            &empty,
            &own,
            &None,
            Some(7),
            &mut budget
        )
        .unwrap()
    );
    assert!(
        !super::super::sky_visible_skipping_owner(
            DVec3::Z,
            start,
            &world,
            &terrain,
            &own,
            &None,
            Some(7),
            &mut budget
        )
        .unwrap()
    );
    for reverse in [false, true] {
        let mut triangles = vec![make_prop(7), make_prop(9)];
        if reverse {
            triangles.reverse();
        }
        let overlap = ShadowIndex::new(triangles, &mut |_| Ok(())).unwrap();
        assert!(
            overlap
                .blocked_skipping_owner(start, end, Some(7), &mut |work| budget.consume(work))
                .unwrap()
        );
        assert!(
            !super::super::sky_visible_skipping_owner(
                DVec3::Z,
                start,
                &world,
                &empty,
                &overlap,
                &None,
                Some(7),
                &mut budget
            )
            .unwrap()
        );
    }
    for canceled in [false, true] {
        let control = Control::default();
        let mut budget = Budget::new(&control);
        if canceled {
            control.cancel().unwrap();
        } else {
            budget.consume(100_000_000).unwrap();
        }
        assert!(
            own.blocked_skipping_owner(start, end, Some(7), &mut |work| budget.consume(work))
                .is_err()
        );
        assert!(
            super::super::sky_visible_skipping_owner(
                DVec3::Z,
                start,
                &world,
                &empty,
                &own,
                &None,
                Some(7),
                &mut budget
            )
            .is_err()
        );
    }
}
