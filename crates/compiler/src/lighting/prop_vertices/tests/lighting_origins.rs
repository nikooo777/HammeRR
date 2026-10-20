use super::*;

const SOURCE_MATERIAL: &str = "lighting_origins/source";
const SOURCE_TEXTURE: &str = "materials/lighting_origins/source.vtf";
const ORIGIN_NAME: &str = "receiver_origin";

fn fixture(mut vmf: Vmf) -> (tempfile::TempDir, GameFs, Vmf) {
    let (directory, game) = fixtures::game();
    std::fs::create_dir_all(directory.path().join("materials/lighting_origins")).unwrap();
    std::fs::write(
        directory
            .path()
            .join("materials/lighting_origins/source.vmt"),
        "LightmappedGeneric { $basetexture lighting_origins/source }",
    )
    .unwrap();
    let mut texture = fixtures::caster_texture(255);
    for (axis, value) in [0.125_f32, 0.25, 0.75].into_iter().enumerate() {
        texture[32 + axis * 4..36 + axis * 4].copy_from_slice(&value.to_le_bytes());
    }
    assert_eq!(
        source_formats::vtf::Vtf::parse(texture.clone())
            .unwrap()
            .reflectivity,
        [0.125, 0.25, 0.75]
    );
    std::fs::write(directory.path().join(SOURCE_TEXTURE), texture).unwrap();
    for side in vmf
        .world
        .solids
        .iter_mut()
        .flat_map(|solid| &mut solid.sides)
    {
        side.material = SOURCE_MATERIAL.into();
    }
    (directory, game, vmf)
}

fn receiver(vmf: &mut Vmf) -> &mut Entity {
    vmf.entities
        .iter_mut()
        .find(|entity| entity.id == 9101)
        .unwrap()
}

fn helper(id: u32, point: [f32; 3]) -> Entity {
    let mut entity = Entity::new(id, "info_lighting");
    entity.properties.set("targetname", ORIGIN_NAME);
    entity
        .properties
        .set("origin", format!("{} {} {}", point[0], point[1], point[2]));
    entity
}

fn set_origin(vmf: &mut Vmf, point: [f32; 3]) {
    receiver(vmf).properties.set("lightingorigin", ORIGIN_NAME);
    vmf.entities.push(helper(9400, point));
}

fn bake(vmf: &Vmf, game: &GameFs, bounces: u8, dual: bool) -> crate::Output {
    let output = crate::compile_with_options(
        vmf,
        game,
        Options {
            bounces,
            ..options(dual)
        },
    );
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.bsp.is_some());
    assert_eq!(output.stats.bounces, bounces);
    if bounces == 1 {
        assert!(output.stats.bounce_samples > 0);
    }
    output
}

fn manifest(bsp: &Bsp) -> crate::prop_lighting_assets::PropLightingManifest {
    crate::prop_lighting_assets::read_generated_prop_lighting(bsp, &Control::default())
        .unwrap()
        .unwrap()
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

fn leaf(bsp: &Bsp, point: DVec3) -> bsp::Leaf {
    let planes = bsp.records::<bsp::Plane>().unwrap();
    let nodes = bsp.records::<bsp::Node>().unwrap();
    let mut node = bsp.records::<bsp::Model>().unwrap()[0].head_node;
    while node >= 0 {
        let branch = &nodes[node as usize];
        let plane = &planes[branch.plane as usize];
        let normal = DVec3::from_array(plane.normal.map(f64::from));
        node = branch.children[usize::from(normal.dot(point) < f64::from(plane.dist))];
    }
    bsp.records::<bsp::Leaf>().unwrap()[(-1 - node) as usize]
}

fn isolated_rooms() -> Vmf {
    let mut document = Document::new(fixtures::scene(false));
    document
        .edit("partition lighting-origin PVS controls", |tx| {
            edit::create_block(
                tx,
                DVec3::new(-8.0, -256.0, 0.0),
                DVec3::new(8.0, 256.0, 256.0),
                SOURCE_MATERIAL,
            )
        })
        .unwrap();
    let mut vmf = document.vmf().clone();
    receiver(&mut vmf).properties.set("origin", "-128 0 50");
    vmf.entities
        .iter_mut()
        .find(|entity| entity.classname() == Some("light"))
        .unwrap()
        .properties
        .set("origin", "-128 0 100");
    let mut seed = Entity::new(9300, "info_player_counterterrorist");
    seed.properties.set("origin", "128 0 8");
    vmf.entities.push(seed);
    vmf
}

#[test]
fn helper_movement_preserves_both_vhv_channels_across_distinct_pvs_and_solid_regions() {
    let (_directory, game, vmf) = fixture(isolated_rooms());
    for bounces in [0, 1] {
        let output = bake(&vmf, &game, bounces, true);
        let bsp = output.bsp.as_ref().unwrap();
        let baseline = [payload(bsp, "sp_0.vhv"), payload(bsp, "sp_hdr_0.vhv")];
        let original = manifest(bsp);
        assert_eq!(bsp.static_props().unwrap().unwrap().props[0].flags & 2, 0);
        assert!(file(bsp, "sp_0.vhv").meshes[0].colors[1].red > 0);
        assert!(file(bsp, "sp_hdr_0.vhv").meshes[0].colors[1].blue > 0);
        let bright = leaf(bsp, DVec3::new(-128.0, 0.0, 80.0)).cluster;
        let dark = leaf(bsp, DVec3::new(128.0, 0.0, 80.0)).cluster;
        assert!(bright >= 0 && dark >= 0 && bright != dark);
        let visibility = bsp.visibility().unwrap().unwrap();
        for (from, to) in [(bright, dark), (dark, bright)] {
            let row = visibility
                .row(from as usize, bsp::VisibilityKind::Visible)
                .unwrap();
            assert_eq!(row[to as usize / 8] & (1 << (to as usize % 8)), 0);
        }
        assert_ne!(
            leaf(bsp, DVec3::new(0.0, 0.0, 80.0)).contents & bsp::contents::SOLID,
            0
        );
        if bounces == 1 {
            assert!(
                original
                    .sources
                    .iter()
                    .any(|source| source.path == SOURCE_TEXTURE)
            );
        }
        drop(output);

        if bounces == 0 {
            let mut dark_receiver = vmf.clone();
            receiver(&mut dark_receiver)
                .properties
                .set("origin", "128 0 50");
            let output = bake(&dark_receiver, &game, bounces, true);
            for name in ["sp_0.vhv", "sp_hdr_0.vhv"] {
                assert!(
                    file(output.bsp.as_ref().unwrap(), name).meshes[0]
                        .colors
                        .iter()
                        .all(|color| color.red == 0 && color.green == 0 && color.blue == 0)
                );
            }
        }

        let mut previous_digest = original.static_props_sha256;
        for point in [[-128.0, 0.0, 80.0], [128.0, 0.0, 80.0], [0.0, 0.0, 80.0]] {
            let mut source = vmf.clone();
            set_origin(&mut source, point);
            let output = bake(&source, &game, bounces, true);
            let bsp = output.bsp.as_ref().unwrap();
            let props = bsp.static_props().unwrap().unwrap();
            let prop = &props.props[0];
            assert_ne!(prop.flags & 2, 0);
            assert_eq!(prop.lighting_origin, point);
            assert_eq!(
                [payload(bsp, "sp_0.vhv"), payload(bsp, "sp_hdr_0.vhv")],
                baseline
            );
            let current = manifest(bsp);
            assert_ne!(current.static_props_sha256, previous_digest);
            let raw = &bsp
                .game_lumps
                .iter()
                .find(|entry| entry.id == *b"sprp")
                .unwrap()
                .data;
            assert_eq!(current.static_props_sha256, crate::asset_sources::hash(raw));
            assert!(!bsp.entities().unwrap().contains("info_lighting"));
            previous_digest = current.static_props_sha256;
        }
    }
}

#[test]
fn translated_and_rotated_receivers_follow_their_vertices_with_one_fixed_helper() {
    let (_directory, game, vmf) = fixture(fixtures::scene(false));
    for bounces in [0, 1] {
        let mut reference = None;
        for (position, angles, directly_lit) in [
            ("0 0 50", "0 0 0", true),
            ("0 0 150", "0 0 0", false),
            ("0 0 50", "180 0 0", false),
        ] {
            let mut source = vmf.clone();
            receiver(&mut source).properties.set("origin", position);
            receiver(&mut source).properties.set("angles", angles);
            let baseline = bake(&source, &game, bounces, false);
            let expected = payload(baseline.bsp.as_ref().unwrap(), "sp_0.vhv");
            drop(baseline);
            set_origin(&mut source, [64.0, 32.0, 64.0]);
            let output = bake(&source, &game, bounces, false);
            let bsp = output.bsp.as_ref().unwrap();
            assert_eq!(payload(bsp, "sp_0.vhv"), expected);
            assert_eq!(
                bsp.static_props().unwrap().unwrap().props[0].lighting_origin,
                [64.0, 32.0, 64.0]
            );
            let red = file(bsp, "sp_0.vhv").meshes[0].colors[1].red;
            if bounces == 0 {
                assert_eq!(red, if directly_lit { 128 } else { 0 });
            }
            if directly_lit {
                assert!(red > 0);
                reference = Some(red);
            } else {
                assert!(
                    red < reference.unwrap(),
                    "position={position}, angles={angles}, bounces={bounces}"
                );
            }
        }
    }
}

#[test]
fn authored_origins_preserve_slanted_and_downward_brush_contact_results() {
    let (directory, game, vmf) = fixture(fixtures::grounded(0.0));
    for normal in [[3.0_f32.sqrt() * 0.5, 0.0, -0.5], [0.0, 0.0, -1.0]] {
        fixtures::normals(directory.path(), normal);
        for bounces in [0, 1] {
            let baseline = bake(&vmf, &game, bounces, false);
            let expected = payload(baseline.bsp.as_ref().unwrap(), "sp_0.vhv");
            drop(baseline);
            let mut source = vmf.clone();
            set_origin(&mut source, [64.0, 0.0, 64.0]);
            let output = bake(&source, &game, bounces, false);
            let bsp = output.bsp.as_ref().unwrap();
            assert_eq!(payload(bsp, "sp_0.vhv"), expected);
            let colors = file(bsp, "sp_0.vhv").meshes.remove(0).colors;
            assert!(
                colors
                    .iter()
                    .all(|color| (color.red > 0) == (normal[0] > 0.0))
            );
            assert!(
                colors
                    .iter()
                    .all(|color| color.green == 0 && color.blue == 0)
            );
        }
    }
}

#[test]
fn buried_vertices_still_fail_with_open_or_solid_authored_origins() {
    let (_directory, game, vmf) = fixture(fixtures::grounded(-0.25));
    for point in [[0.0, 0.0, 64.0], [0.0, 0.0, -8.0]] {
        for bounces in [0, 1] {
            let mut source = vmf.clone();
            set_origin(&mut source, point);
            let output = crate::compile_with_options(
                &source,
                &game,
                Options {
                    bounces,
                    ..options(false)
                },
            );
            assert!(output.bsp.is_none() && output.prop_lighting_assets.is_none());
            assert!(
                output.report.diagnostics.iter().any(|diagnostic| {
                    diagnostic.object.as_deref() == Some("entity:9101")
                        && diagnostic.message.contains("buried")
                        && diagnostic.message.contains("-0.250000")
                        && diagnostic
                            .message
                            .contains("embedded-vertex recovery is unsupported")
                }),
                "{:?}",
                output.report
            );
        }
    }
}

#[test]
fn invalid_helpers_precede_opt_out_and_ignore_normals_preserves_native_origins() {
    let (_directory, game, vmf) = fixture(fixtures::scene(false));
    for bounces in [0, 1] {
        for disabled in [false, true] {
            for duplicate in [false, true] {
                let mut source = vmf.clone();
                receiver(&mut source)
                    .properties
                    .set("lightingorigin", ORIGIN_NAME);
                if disabled {
                    receiver(&mut source)
                        .properties
                        .set("disablevertexlighting", "1");
                }
                if duplicate {
                    source.entities.extend([
                        helper(9400, [0.0, 0.0, 64.0]),
                        helper(9401, [64.0, 0.0, 64.0]),
                    ]);
                }
                let output = crate::compile_with_options(
                    &source,
                    &game,
                    Options {
                        bounces,
                        ..options(false)
                    },
                );
                assert!(output.bsp.is_none() && output.prop_lighting_assets.is_none());
                let message = if duplicate {
                    "multiple info_lighting helpers"
                } else {
                    "must name one visible info_lighting"
                };
                assert!(
                    output.report.diagnostics.iter().any(|diagnostic| {
                        diagnostic.object.as_deref() == Some("entity:9101")
                            && diagnostic.message.contains(message)
                    }),
                    "disabled={disabled}, bounces={bounces}: {:?}",
                    output.report
                );
            }
        }
        let mut source = vmf.clone();
        set_origin(&mut source, [64.0, 0.0, 64.0]);
        receiver(&mut source).properties.set("ignorenormals", "1");
        let result = crate::compile_with_options(
            &source,
            &game,
            Options {
                bounces,
                ..options(false)
            },
        );
        if bounces == 0 {
            assert!(!result.report.has_errors(), "{:?}", result.report);
            let bsp = result.bsp.as_ref().unwrap();
            let prop = &bsp.static_props().unwrap().unwrap().props[0];
            assert_eq!(prop.flags & (2 | 8 | 64), 2 | 8);
            assert_eq!(prop.lighting_origin, [64.0, 0.0, 64.0]);
            let payloads = [payload(bsp, "sp_0.vhv")];
            receiver(&mut source).properties.remove("lightingorigin");
            source
                .entities
                .retain(|entity| entity.classname() != Some("info_lighting"));
            let ordinary = bake(&source, &game, bounces, false);
            assert_eq!(
                payloads,
                [payload(ordinary.bsp.as_ref().unwrap(), "sp_0.vhv")]
            );
            set_origin(&mut source, [64.0, 0.0, 64.0]);
        } else {
            assert!(result.bsp.is_none());
            assert!(
                result.report.diagnostics.iter().any(|diagnostic| {
                    diagnostic.object.as_deref() == Some("entity:9101")
                        && diagnostic
                            .message
                            .contains("ignorenormals require Direct only")
                        && !diagnostic.message.contains("lightingorigin")
                }),
                "{:?}",
                result.report
            );
        }
        receiver(&mut source)
            .properties
            .set("disablevertexlighting", "1");
        let disabled = bake(&source, &game, bounces, false);
        assert!(disabled.stats.prop_vertex_lighting.is_none());
        let bsp = disabled.bsp.as_ref().unwrap();
        let props = bsp.static_props().unwrap().unwrap();
        let prop = &props.props[0];
        assert_eq!(prop.flags & (2 | 8 | 64), 2 | 8 | 64);
        assert_eq!(prop.lighting_origin, [64.0, 0.0, 64.0]);
        assert!(
            crate::prop_lighting_assets::read_generated_prop_lighting(bsp, &Control::default())
                .unwrap()
                .is_none()
        );
        assert!(
            !bsp.pak_data(bsp::PakLimits::default())
                .unwrap()
                .entries
                .iter()
                .any(|entry| entry.name.ends_with(".vhv"))
        );
    }
}
