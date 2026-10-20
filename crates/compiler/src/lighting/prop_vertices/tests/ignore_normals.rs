use super::super::super::{
    PointLight, ReceiverAngularMode,
    bake::Emitter,
    props::PropTriangle,
    shadows::{ShadowIndex, Triangle},
};
use super::*;

fn prop(vmf: &mut Vmf, id: u32) -> &mut Entity {
    vmf.entities
        .iter_mut()
        .find(|entity| entity.id == id)
        .unwrap()
}

fn flag(vmf: &mut Vmf) {
    prop(vmf, 9101).properties.set("ignorenormals", "1");
}

fn payload(bsp: &Bsp, name: &str) -> Vec<u8> {
    bsp.pak_data(bsp::PakLimits::default())
        .unwrap()
        .entries
        .into_iter()
        .find(|entry| entry.name == name)
        .unwrap()
        .data
}

fn manifest(bsp: &Bsp) -> crate::prop_lighting_assets::PropLightingManifest {
    crate::prop_lighting_assets::read_generated_prop_lighting(bsp, &Control::default())
        .unwrap()
        .unwrap()
}

fn colors(bsp: &Bsp, name: &str, expected: [u8; 3]) {
    let [red, green, blue] = expected;
    for mesh in file(bsp, name).meshes {
        assert!(!mesh.colors.is_empty());
        assert!(
            mesh.colors.iter().all(|color| *color
                == vhv::Bgra8 {
                    red,
                    green,
                    blue,
                    alpha: 255
                }),
            "{name}: {:?}",
            mesh.colors
        );
    }
}

fn prop_triangle(points: [DVec3; 3], owner: Option<u32>) -> PropTriangle {
    let mut triangle = PropTriangle::new(points).unwrap();
    if let Some(owner) = owner {
        triangle.set_owner(owner);
    }
    triangle
}

#[test]
fn direct_point_keeps_all_lods_unused_duplicate_rows_channels_and_default_bytes() {
    let (directory, game) = fixtures::game();
    fixtures::add_lod(directory.path());
    let mut vmf = fixtures::scene(true);
    prop(&mut vmf, 9001).properties.set("_light", "255 0 0 100");
    prop(&mut vmf, 9001)
        .properties
        .set("_lightHDR", "0 0 255 200");
    let default = compile(&vmf, &game, true);
    prop(&mut vmf, 9101).properties.set("ignorenormals", "0");
    let explicit_zero = compile(&vmf, &game, true);
    assert_eq!(
        default.bsp.as_ref().unwrap().to_bytes(),
        explicit_zero.bsp.as_ref().unwrap().to_bytes()
    );
    drop(explicit_zero);
    flag(&mut vmf);
    let before = vmf.clone();
    for normal in [
        [1.0, 0.0, 0.0],
        [-1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, -1.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.0, 0.0, -1.0],
    ] {
        fixtures::normals(directory.path(), normal);
        let output = compile(&vmf, &game, true);
        let bsp = output.bsp.as_ref().unwrap();
        colors(bsp, "sp_0.vhv", [52, 0, 0]);
        colors(bsp, "sp_hdr_0.vhv", [0, 0, 71]);
        let native = bsp.static_props().unwrap().unwrap();
        assert_eq!(
            native
                .props
                .iter()
                .map(|prop| prop.flags & (IGNORE_NORMALS | DISABLED))
                .collect::<Vec<_>>(),
            [IGNORE_NORMALS, DISABLED, 0]
        );
        let manifest = manifest(bsp);
        assert_eq!(
            manifest
                .receivers
                .iter()
                .map(|receiver| receiver.ordinal)
                .collect::<Vec<_>>(),
            [0, 2]
        );
        assert_eq!(
            manifest.receivers[0].meshes,
            [
                crate::prop_lighting_assets::MeshLayout {
                    lod: 0,
                    vertex_count: 6
                },
                crate::prop_lighting_assets::MeshLayout {
                    lod: 1,
                    vertex_count: 4
                }
            ]
        );
        assert!(
            !manifest
                .sources
                .iter()
                .any(|source| source.path.ends_with(".vtf"))
        );
        assert_eq!(
            output.stats.prop_vertex_lighting.as_ref().unwrap().vertices,
            20
        );
        if normal == [0.0, 0.0, 1.0] {
            let default = default.bsp.as_ref().unwrap();
            assert_eq!(payload(bsp, "sp_2.vhv"), payload(default, "sp_2.vhv"));
            assert_eq!(
                payload(bsp, "sp_hdr_2.vhv"),
                payload(default, "sp_hdr_2.vhv")
            );
            let original = self::manifest(default);
            assert_eq!(manifest.sources, original.sources);
            assert_eq!(manifest.missing_sources, original.missing_sources);
            for (index, (left, right)) in bsp.lumps.iter().zip(&default.lumps).enumerate() {
                if index != bsp::lump::GAME_LUMP && index != bsp::lump::PAKFILE {
                    assert_eq!(left, right, "native lump {index}");
                }
            }
        }
    }
    assert_eq!(vmf, before);
}

fn sky_scene(visibility: u8) -> Vmf {
    let mut vmf = fixtures::scene(false);
    vmf.world.solids.clear();
    let mut document = Document::new(vmf);
    document
        .edit("split upper and lower sky visibility", |tx| {
            for (mins, maxs) in [
                ([-272.0, -272.0, -16.0], [272.0, 272.0, 0.0]),
                ([-272.0, -272.0, 256.0], [272.0, 272.0, 272.0]),
                ([-272.0, -272.0, 0.0], [-256.0, 272.0, 128.0]),
                ([256.0, -272.0, 0.0], [272.0, 272.0, 128.0]),
                ([-256.0, -272.0, 0.0], [256.0, -256.0, 128.0]),
                ([-256.0, 256.0, 0.0], [256.0, 272.0, 128.0]),
                ([-272.0, -272.0, 128.0], [-256.0, 272.0, 256.0]),
                ([256.0, -272.0, 128.0], [272.0, 272.0, 256.0]),
                ([-256.0, -272.0, 128.0], [256.0, -256.0, 256.0]),
                ([-256.0, 256.0, 128.0], [256.0, 272.0, 256.0]),
            ] {
                let sky = visibility == 2 || (visibility == 1 && mins[2] >= 128.0);
                edit::create_block(
                    tx,
                    DVec3::from_array(mins),
                    DVec3::from_array(maxs),
                    if sky {
                        "sky"
                    } else {
                        "dev/dev_measuregeneric01"
                    },
                )?;
            }
            Ok(())
        })
        .unwrap();
    let mut vmf = document.vmf().clone();
    for side in vmf
        .world
        .solids
        .iter_mut()
        .flat_map(|solid| &mut solid.sides)
    {
        side.lightmap_scale = 128;
    }
    prop(&mut vmf, 9101).properties.set("origin", "0 0 128");
    flag(&mut vmf);
    for (key, value) in [
        ("classname", "light_environment"),
        ("pitch", "-90"),
        ("_light", "0"),
        ("_ambient", "255 0 0 40"),
    ] {
        prop(&mut vmf, 9001).properties.set(key, value);
    }
    vmf
}

#[test]
fn compiler_sky_ambient_uses_full_sphere_visibility_for_reversed_and_slanted_normals() {
    let (directory, game) = fixtures::game();
    for normal in [[0.0, 0.0, 1.0], [0.0, 0.0, -1.0], [1.0, 2.0, -3.0]] {
        fixtures::normals(directory.path(), normal);
        for (visibility, expected) in [(0, 0), (1, 40), (2, 55)] {
            let output = compile(&sky_scene(visibility), &game, false);
            colors(output.bsp.as_ref().unwrap(), "sp_0.vhv", [expected, 0, 0]);
        }
    }
    let mut vmf = sky_scene(2);
    prop(&mut vmf, 9001).properties.set("_light", "255 0 0 100");
    prop(&mut vmf, 9001)
        .properties
        .set("_lightHDR", "0 0 255 200");
    prop(&mut vmf, 9001).properties.set("_ambient", "0");
    prop(&mut vmf, 9001).properties.set("_ambientHDR", "0");
    fixtures::normals(directory.path(), [0.0, 0.0, -1.0]);
    let output = compile(&vmf, &game, true);
    colors(output.bsp.as_ref().unwrap(), "sp_0.vhv", [52, 0, 0]);
    colors(output.bsp.as_ref().unwrap(), "sp_hdr_0.vhv", [0, 0, 71]);
}

#[test]
fn ignore_normals_retains_own_point_and_spot_shadows_until_separately_disabled() {
    let (directory, game) = fixtures::game();
    fixtures::self_shadow_model(directory.path());
    for spot in [false, true] {
        let mut vmf = fixtures::scene(false);
        flag(&mut vmf);
        if spot {
            for (key, value) in [
                ("classname", "light_spot"),
                ("pitch", "-90"),
                ("_inner_cone", "30"),
                ("_cone", "45"),
                ("_exponent", "2"),
            ] {
                prop(&mut vmf, 9001).properties.set(key, value);
            }
        }
        let shadowed = compile(&vmf, &game, !spot);
        assert_eq!(
            file(shadowed.bsp.as_ref().unwrap(), "sp_0.vhv").meshes[0].colors[1].red,
            0
        );
        self_shadow_flag(&mut vmf, "1");
        let open = compile(&vmf, &game, !spot);
        let bsp = open.bsp.as_ref().unwrap();
        assert_eq!(
            bsp.static_props().unwrap().unwrap().props[0].flags & (IGNORE_NORMALS | NO_SELF_SHADOW),
            IGNORE_NORMALS | NO_SELF_SHADOW
        );
        assert_eq!(file(bsp, "sp_0.vhv").meshes[0].colors[1].red, 79);
        if !spot {
            assert_eq!(file(bsp, "sp_hdr_0.vhv").meshes[0].colors[1].blue, 79);
        }
    }
}

fn measured_game(vmf: &mut Vmf) -> (tempfile::TempDir, GameFs) {
    let (directory, game) = fixtures::game();
    std::fs::write(
        directory.path().join("materials/measured.vmt"),
        "LightmappedGeneric { $basetexture measured }",
    )
    .unwrap();
    let mut texture = fixtures::caster_texture(255);
    for axis in 0..3 {
        texture[32 + axis * 4..36 + axis * 4].copy_from_slice(&0.5_f32.to_le_bytes());
    }
    std::fs::write(directory.path().join("materials/measured.vtf"), texture).unwrap();
    for side in vmf
        .world
        .solids
        .iter_mut()
        .flat_map(|solid| &mut solid.sides)
    {
        side.material = "measured".into();
    }
    (directory, game)
}

#[test]
fn absent_and_explicit_zero_keep_exact_dual_channel_bytes_at_every_supported_depth() {
    let mut vmf = fixtures::scene(false);
    let (_directory, game) = measured_game(&mut vmf);
    for bounces in 0..=4 {
        prop(&mut vmf, 9101).properties.remove("ignorenormals");
        let absent = crate::compile_with_options(
            &vmf,
            &game,
            Options {
                bounces,
                ..options(true)
            },
        );
        assert!(!absent.report.has_errors(), "{:?}", absent.report);
        prop(&mut vmf, 9101).properties.set("ignorenormals", "0");
        let zero = crate::compile_with_options(
            &vmf,
            &game,
            Options {
                bounces,
                ..options(true)
            },
        );
        assert!(!zero.report.has_errors(), "{:?}", zero.report);
        assert_eq!(
            absent.bsp.as_ref().unwrap().to_bytes(),
            zero.bsp.as_ref().unwrap().to_bytes(),
            "depth {bounces}"
        );
    }
}

#[test]
fn positive_depth_mixed_receivers_fail_atomically_but_opt_out_retains_ordinal_hole() {
    let mut vmf = fixtures::scene(true);
    let (directory, game) = measured_game(&mut vmf);
    prop(&mut vmf, 9103).properties.set("ignorenormals", "1");
    let before = vmf.clone();
    let output_directory = tempfile::tempdir_in(directory.path().parent().unwrap()).unwrap();
    let destination = output_directory.path().join("previous.bsp");
    let pointfile = destination.with_extension("lin");
    std::fs::write(&destination, b"previous complete output").unwrap();
    std::fs::write(&pointfile, b"previous pointfile").unwrap();
    for bounces in 1..=4 {
        let failed = crate::compile_with_options(
            &vmf,
            &game,
            Options {
                bounces,
                ..options(true)
            },
        );
        assert!(failed.bsp.is_none() && failed.prop_lighting_assets.is_none());
        assert!(
            failed
                .report
                .diagnostics
                .iter()
                .any(
                    |diagnostic| diagnostic.object.as_deref() == Some("entity:9103")
                        && diagnostic
                            .message
                            .contains("ignorenormals require Direct only")
                ),
            "{:?}",
            failed.report
        );
        assert!(!failed.write(&destination).unwrap());
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            b"previous complete output"
        );
        assert_eq!(std::fs::read(&pointfile).unwrap(), b"previous pointfile");
        assert_eq!(vmf, before);
        let mut disabled = vmf.clone();
        prop(&mut disabled, 9103)
            .properties
            .set("disablevertexlighting", "1");
        let output = crate::compile_with_options(
            &disabled,
            &game,
            Options {
                bounces,
                ..options(true)
            },
        );
        assert!(!output.report.has_errors(), "{:?}", output.report);
        let bsp = output.bsp.as_ref().unwrap();
        assert_eq!(
            manifest(bsp)
                .receivers
                .iter()
                .map(|receiver| receiver.ordinal)
                .collect::<Vec<_>>(),
            [0]
        );
        assert_eq!(
            bsp.static_props().unwrap().unwrap().props[2].flags & (IGNORE_NORMALS | DISABLED),
            IGNORE_NORMALS | DISABLED
        );
        assert!(
            !bsp.pak_data(bsp::PakLimits::default())
                .unwrap()
                .entries
                .iter()
                .any(|entry| entry.name == "sp_2.vhv" || entry.name == "sp_hdr_2.vhv")
        );
    }
}

#[test]
fn positive_depth_preflight_precedes_all_receiver_companion_and_material_reads() {
    let (directory, game) = fixtures::game();
    let mut vmf = fixtures::scene(true);
    prop(&mut vmf, 9103).properties.set("ignorenormals", "1");
    let mut bsp = crate::compile_with_options(&vmf, &game, Options::default())
        .bsp
        .unwrap();
    let control = Control::default();
    let mut report = Report::default();
    let mut map = Map::load_with_prop_sources_controlled(
        &vmf,
        &game,
        crate::materials::PropSourceMode::Direct,
        &mut report,
        &control,
    )
    .unwrap();
    let props = crate::static_props::bake(
        &vmf,
        &game,
        &mut bsp,
        &mut report,
        &control,
        map.materials.prop_lighting_sources_mut(),
    )
    .unwrap();
    let sources = map.materials.take_prop_lighting_sources().unwrap();
    std::fs::write(
        directory.path().join("models/props/test.vvd"),
        b"unavailable receiver companion",
    )
    .unwrap();
    std::fs::write(
        directory.path().join("materials/shared/base.vmt"),
        "UnlitGeneric {}",
    )
    .unwrap();
    let mut budget = Budget::new(&control);
    let tracker =
        super::super::super::asset_tracking::Tracker::new(&game, sources, &mut budget).unwrap();
    assert!(
        prepare(
            &map,
            Settings {
                vmf: &vmf,
                prop_sources: &props.sources,
                bounces: 1
            },
            tracker,
            &bsp,
            &mut report,
            &mut budget
        )
        .is_err()
    );
    assert!(
        report
            .diagnostics
            .iter()
            .any(
                |diagnostic| diagnostic.object.as_deref() == Some("entity:9103")
                    && diagnostic
                        .message
                        .contains("ignorenormals require Direct only")
            ),
        "{report:?}"
    );
}

#[test]
fn flagged_source_guards_survive_output_drop_for_receiver_and_disabled_caster_assets() {
    for changed in [
        "models/props/test.vvd",
        "models/casters/test.dx90.vtx",
        "materials/casters/base.vmt",
        "materials/casters/mask.vtf",
        "materials/models/test/one.vmt",
    ] {
        let (directory, game) = fixtures::game();
        fixtures::caster(directory.path(), 0);
        let mut vmf = fixtures::caster_scene();
        flag(&mut vmf);
        let output_directory = tempfile::tempdir_in(directory.path().parent().unwrap()).unwrap();
        let destination = output_directory.path().join("previous.bsp");
        std::fs::write(&destination, b"previous complete output").unwrap();
        let output = compile(&vmf, &game, true);
        colors(output.bsp.as_ref().unwrap(), "sp_0.vhv", [79, 0, 0]);
        let control = Control::default();
        let prepared = output
            .prepare_write_controlled(&destination, &control)
            .unwrap()
            .unwrap();
        if changed.ends_with(".vtf") {
            overwrite(
                &directory.path().join(changed),
                0,
                &fixtures::caster_texture(255),
            );
        } else if changed.ends_with(".vvd") {
            overwrite(
                &directory.path().join(changed),
                80,
                &(-8.0_f32).to_le_bytes(),
            );
        } else if changed.ends_with(".vtx") {
            overwrite(&directory.path().join(changed), 260, &1_u16.to_le_bytes());
        } else {
            std::fs::write(
                directory.path().join(changed),
                "VertexLitGeneric { $basetexture appeared }",
            )
            .unwrap();
        }
        drop(output);
        assert!(prepared.commit(&control).is_err(), "{changed}");
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            b"previous complete output"
        );
    }
}

#[test]
fn flagged_vertices_keep_original_point_burial_normal_and_vtx_morph_admission() {
    let (directory, game) = fixtures::game();
    let mut vmf = fixtures::scene(false);
    flag(&mut vmf);
    fixtures::normals(directory.path(), [0.0, 0.0, -1.0]);
    let mut grounded = fixtures::grounded(0.0);
    flag(&mut grounded);
    let contact = compile(&grounded, &game, false);
    colors(contact.bsp.as_ref().unwrap(), "sp_0.vhv", [79, 0, 0]);
    let mut document = Document::new(vmf.clone());
    document
        .edit("thin blocker above actual receiver", |tx| {
            edit::create_block(
                tx,
                DVec3::new(-16.0, -16.0, 50.125),
                DVec3::new(16.0, 16.0, 50.375),
                "dev/dev_measuregeneric01",
            )
        })
        .unwrap();
    let blocked = compile(document.vmf(), &game, false);
    colors(blocked.bsp.as_ref().unwrap(), "sp_0.vhv", [0, 0, 0]);
    prop(&mut vmf, 9101).properties.set("origin", "0 0 -0.25");
    let buried = crate::compile_with_options(&vmf, &game, options(false));
    assert!(buried.bsp.is_none());
    assert!(
        buried
            .report
            .diagnostics
            .iter()
            .any(
                |diagnostic| diagnostic.object.as_deref() == Some("entity:9101")
                    && diagnostic.message.contains("buried")
                    && diagnostic.message.contains("-0.250000")
            ),
        "{:?}",
        buried.report
    );
    prop(&mut vmf, 9101).properties.set("origin", "0 0 50");
    fixtures::normals(directory.path(), [0.0; 3]);
    let invalid = crate::compile_with_options(&vmf, &game, options(false));
    assert!(invalid.bsp.is_none());
    assert!(
        invalid
            .report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("normal")),
        "{:?}",
        invalid.report
    );
    fixtures::normals(directory.path(), [0.0, 0.0, -1.0]);
    overwrite(
        &directory.path().join("models/props/test.dx90.vtx"),
        97,
        &[2 | 8],
    );
    let morph = crate::compile_with_options(&vmf, &game, options(false));
    assert!(morph.bsp.is_none());
    assert!(
        morph
            .report
            .diagnostics
            .iter()
            .any(
                |diagnostic| diagnostic.object.as_deref() == Some("entity:9101")
                    && diagnostic.message.contains("ordinary hardware-skinned")
            ),
        "{:?}",
        morph.report
    );
}

#[test]
fn flagged_unknown_cluster_keeps_world_terrain_other_unknown_and_tied_prop_blockers() {
    let light = Emitter::Point(
        PointLight::from_entity(
            &crate::map::MapEntity {
                id: 1,
                classname: "light".into(),
                origin: Some(DVec3::Z * 32.0),
                brushes: Vec::new(),
                properties: vec![
                    ("_light".into(), "255 255 255 100".into()),
                    ("_constant_attn".into(), "1".into()),
                ],
            },
            -1,
        )
        .unwrap(),
    );
    let lights = [light];
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
    let points = [
        DVec3::new(-16.0, -16.0, 8.0),
        DVec3::new(16.0, -16.0, 8.0),
        DVec3::new(0.0, 16.0, 8.0),
    ];
    let empty_world = ShadowIndex::new(Vec::new(), &mut |_| Ok(())).unwrap();
    let world = ShadowIndex::new(
        vec![hull(
            DVec3::new(-16.0, -16.0, 0.125),
            DVec3::new(16.0, 16.0, 0.375),
        )],
        &mut |_| Ok(()),
    )
    .unwrap();
    let empty_terrain = ShadowIndex::new(Vec::new(), &mut |_| Ok(())).unwrap();
    let terrain = ShadowIndex::new(vec![Triangle::new(points).unwrap()], &mut |_| Ok(())).unwrap();
    for (brushes, terrain, triangles, skip_owner, expected) in [
        (&empty_world, &empty_terrain, Vec::new(), None, 35.0),
        (&world, &empty_terrain, Vec::new(), Some(7), 0.0),
        (&empty_world, &terrain, Vec::new(), Some(7), 0.0),
        (
            &empty_world,
            &empty_terrain,
            vec![prop_triangle(points, Some(7))],
            None,
            0.0,
        ),
        (
            &empty_world,
            &empty_terrain,
            vec![prop_triangle(points, Some(7))],
            Some(7),
            35.0,
        ),
        (
            &empty_world,
            &empty_terrain,
            vec![prop_triangle(points, None)],
            Some(7),
            0.0,
        ),
        (
            &empty_world,
            &empty_terrain,
            vec![
                prop_triangle(points, Some(7)),
                prop_triangle(points, Some(9)),
            ],
            Some(7),
            0.0,
        ),
        (
            &empty_world,
            &empty_terrain,
            vec![
                prop_triangle(points, Some(9)),
                prop_triangle(points, Some(7)),
            ],
            Some(7),
            0.0,
        ),
    ] {
        let props = ShadowIndex::new(triangles, &mut |_| Ok(())).unwrap();
        let ctx = receivers::Context {
            lights: &lights,
            shadows: brushes,
            terrain,
            props: &props,
            sun_directions: &[],
            ambient_directions: &[],
        };
        let control = Control::default();
        let mut budget = Budget::new(&control);
        let mut candidates = Candidates::new(&lights, &visibility, &mut budget).unwrap();
        let value = receivers::direct_with_reception(
            &ctx,
            &mut candidates,
            None,
            receivers::Sample {
                point: DVec3::ZERO,
                normal: -DVec3::Z,
                origin: DVec3::ZERO,
            },
            false,
            receivers::Reception {
                angular: ReceiverAngularMode::IgnoreNormals,
                skip_owner,
            },
            &mut budget,
        )
        .unwrap();
        assert_eq!(value[0], DVec3::splat(expected));
        for canceled in [false, true] {
            let control = Control::default();
            let mut budget = Budget::new(&control);
            if canceled {
                control.cancel().unwrap();
            } else {
                budget.consume(100_000_000).unwrap();
            }
            assert!(
                receivers::direct_with_reception(
                    &ctx,
                    &mut candidates,
                    None,
                    receivers::Sample {
                        point: DVec3::ZERO,
                        normal: -DVec3::Z,
                        origin: DVec3::ZERO
                    },
                    false,
                    receivers::Reception {
                        angular: ReceiverAngularMode::IgnoreNormals,
                        skip_owner
                    },
                    &mut budget
                )
                .is_err()
            );
        }
    }
}
