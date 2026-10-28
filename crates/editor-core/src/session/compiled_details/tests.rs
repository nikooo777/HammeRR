use std::path::PathBuf;

use map_core::{ObjectId, query::Ray};
use source_formats::vmf::Entity;

use super::*;
use crate::session::{SelectMode, compilation::BuildSettingsUpdate};

const ATLAS: &str = "UnlitGeneric { $basetexture test/atlas $translucent 1 $nocull 1 $vertexcolor 1 $vertexalpha 1 }";

fn fixture() -> (tempfile::TempDir, Session, PathBuf) {
    fixture_with_atlas(Some("test/atlas"))
}

fn fixture_with_atlas(declaration: Option<&str>) -> (tempfile::TempDir, Session, PathBuf) {
    let (directory, game) = crate::test_assets::model_lighting::game();
    std::fs::create_dir_all(directory.path().join("maps")).unwrap();
    for name in ["one", "two"] {
        std::fs::write(
            directory
                .path()
                .join(format!("materials/models/test/{name}.vmt")),
            "UnlitGeneric { $basetexture test/checker }",
        )
        .unwrap();
    }
    std::fs::write(directory.path().join("materials/test/atlas.vmt"), ATLAS).unwrap();
    let mut atlas = std::fs::read(directory.path().join("materials/test/checker.vtf")).unwrap();
    atlas[16..18].copy_from_slice(&32u16.to_le_bytes());
    atlas.truncate(80 + 32 * 32 * 4);
    std::fs::create_dir_all(directory.path().join("materials/detail")).unwrap();
    std::fs::write(
        directory.path().join("materials/detail/detailsprites.vmt"),
        ATLAS.replace("test/atlas", "detail/detailsprites"),
    )
    .unwrap();
    std::fs::write(
        directory.path().join("materials/detail/detailsprites.vtf"),
        &atlas,
    )
    .unwrap();
    std::fs::write(directory.path().join("materials/test/atlas.vtf"), atlas).unwrap();
    std::fs::write(
        directory.path().join("materials/test/ground.vmt"),
        "UnlitGeneric { %detailtype tufts }",
    )
    .unwrap();
    std::fs::write(directory.path().join("detail.vbsp"),"detail { tufts { density 128 group { alpha 1 item { sprite \"0 0 4 4 8\" spritesize \"0.25 0.5 16 32\" detailOrientation 2 upright 1 sway 0.5 } } } }").unwrap();
    let mut vmf = crate::test_assets::room();
    for side in vmf
        .world
        .solids
        .iter_mut()
        .flat_map(|solid| &mut solid.sides)
    {
        side.material = "test/ground".into();
    }
    match declaration {
        Some(name) => vmf.world.properties.set("detailmaterial", name),
        None => {
            vmf.world.properties.remove("detailmaterial");
        }
    }
    for (id, kind, origin) in [
        (9000, "", "0 0 64"),
        (9001, "cross", "64 0 64"),
        (9002, "tri", "-64 0 64"),
    ] {
        let mut entity = Entity::new(id, "prop_detail_sprite");
        for (key, value) in [
            ("origin", origin),
            ("position_ul", "-4 16"),
            ("position_lr", "12 -16"),
            ("tex_ul", "0 0"),
            ("tex_size", "4 4"),
            ("tex_total_size", "8"),
            ("scale", "2"),
            ("detailOrientation", "0"),
        ] {
            entity.properties.set(key, value);
        }
        if !kind.is_empty() {
            entity.properties.set("sprite_shape", kind);
        }
        vmf.entities.push(entity);
    }
    let mut model = Entity::new(9003, "prop_detail");
    for (key, value) in [
        ("origin", "0 64 64"),
        ("angles", "0 90 0"),
        ("model", "models/test/receiver.mdl"),
    ] {
        model.properties.set(key, value);
    }
    vmf.entities.push(model);
    let source = directory.path().join("maps/details.vmf");
    std::fs::write(&source, vmf.write(&Default::default())).unwrap();
    let mut session = Session::new(Ok(game));
    session.open(Actor::User, &source, false).unwrap();
    session
        .update_build_settings(
            Actor::User,
            BuildSettingsUpdate {
                lighting: Some("fullbright"),
                physics: Some("none"),
                generate_reflections: Some(false),
                pack_sky_assets: Some(false),
                pack_water_assets: Some(false),
                pack_static_prop_assets: Some(false),
                bake_static_prop_vertices: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
    let completed = session.begin_compile(Actor::User, None).unwrap().run();
    session.finish_compile(Actor::User, &completed);
    assert!(
        completed.as_ref().unwrap().published(),
        "{:?}",
        session.compile_status()
    );
    assert_eq!(
        baked_build::build_state_controlled(&session, SOURCE_MODE, &Control::default()).unwrap()["status"],
        "current"
    );
    (directory, session, source)
}

fn prepare(session: &mut Session) -> u64 {
    let request = session.begin_compiled_details(Actor::User).unwrap();
    let token = request.token();
    let report = session
        .finish_compiled_details(Actor::User, request.run())
        .unwrap();
    assert_eq!(report["status"], "prepared", "{report}");
    token
}

fn retain_native_fixture(session: &mut Session, bsp: &Bsp) {
    let path = session.compiled.as_ref().unwrap().path.clone();
    bsp.write(&path).unwrap();
    let identity = FileIdentity::capture(&path, &Control::default()).unwrap();
    let build = session.compiled.as_mut().unwrap();
    build.output_identity = Ok(identity.clone());
    build.lightmap_identity.output = Ok(identity);
}

fn radiance_fixture(
    dual: bool,
    styled: bool,
    independent_static: bool,
) -> (tempfile::TempDir, Session, PathBuf) {
    let (directory, mut session, source) = fixture();
    std::fs::write(
        directory.path().join("materials/test/ground.vmt"),
        "UnlitGeneric { }",
    )
    .unwrap();
    let mut vmf = session.document().vmf().clone();
    let card = vmf
        .entities
        .iter()
        .find(|entity| entity.classname() == Some("prop_detail_sprite"))
        .unwrap()
        .clone();
    for orientation in [1, 2] {
        let mut card = card.clone();
        card.id = 9003 + orientation;
        card.properties
            .set("detailOrientation", orientation.to_string());
        card.properties
            .set("origin", format!("{} 0 64", orientation * 32));
        vmf.entities.push(card);
    }
    for (id, style, ldr, hdr) in [
        (9100, "0", "0 255 0 32", "0 255 0 64"),
        (9101, "1", "255 0 0 120", "0 0 0 0"),
        (9102, "2", "0 0 0 0", "0 0 255 80"),
    ] {
        if style != "0" && !styled {
            continue;
        }
        let mut light = Entity::new(id, "light");
        for (key, value) in [
            ("origin", "0 0 224"),
            ("style", style),
            ("_light", ldr),
            (
                "_lightHDR",
                if style == "0" && !independent_static {
                    ldr
                } else {
                    hdr
                },
            ),
            ("_constant_attn", "1"),
            ("_linear_attn", "0"),
            ("_quadratic_attn", "0"),
        ] {
            light.properties.set(key, value);
        }
        vmf.entities.push(light);
    }
    std::fs::write(&source, vmf.write(&Default::default())).unwrap();
    session.open(Actor::User, &source, false).unwrap();
    session
        .update_build_settings(
            Actor::User,
            BuildSettingsUpdate {
                lighting: Some(if dual { "direct_ldr_hdr" } else { "direct_ldr" }),
                sky_quality: Some("fast"),
                bounces: Some(0),
                refine_ambient_probes: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
    let completed = session.begin_compile(Actor::User, None).unwrap().run();
    session.finish_compile(Actor::User, &completed);
    assert!(
        completed.as_ref().unwrap().published(),
        "{:?}",
        session.compile_status()
    );
    (directory, session, source)
}

fn radiance_settings(channel: LightingChannel, exposure_ev: f64) -> LightingSettings {
    LightingSettings {
        mode: Lighting::BakedRadiance,
        channel,
        exposure_ev,
        style_gains: vec![
            render::compiled_details::StyleGain {
                style: 1,
                gain: 1.0,
            },
            render::compiled_details::StyleGain {
                style: 2,
                gain: 0.5,
            },
        ],
    }
}

#[test]
fn compiled_detail_radiance_consumes_actual_channel_union_for_every_receiver_and_lifecycle() {
    let (_directory, mut session, source) = radiance_fixture(true, true, true);
    let before = (
        session.document().vmf().clone(),
        session.camera(),
        session.history_snapshot(),
        std::fs::read(&source).unwrap(),
    );
    let bsp = Bsp::read(&session.compiled.as_ref().unwrap().path).unwrap();
    let native = bsp.detail_props().unwrap().unwrap();
    let ldr = bsp.detail_prop_lighting(false).unwrap().unwrap();
    let hdr = bsp.detail_prop_lighting(true).unwrap().unwrap();
    assert!(
        native
            .props
            .iter()
            .all(|prop| prop.light_style_count == 3 && prop.lighting.rgb == [0; 3])
    );
    assert!(
        ldr.as_chunks::<3>()
            .0
            .iter()
            .all(|entries| entries[0].style == 0
                && entries[0].lighting.rgb[1] > 0
                && entries[1].style == 1
                && entries[1].lighting.rgb[0] > 0
                && entries[2].style == 2
                && entries[2].lighting.rgb == [0; 3])
    );
    assert!(
        hdr.as_chunks::<3>()
            .0
            .iter()
            .zip(ldr.as_chunks::<3>().0)
            .all(|(hdr, ldr)| hdr[0].style == 0
                && hdr[0].lighting.linear()[1] > ldr[0].lighting.linear()[1]
                && hdr[1].lighting.rgb == [0; 3]
                && hdr[2].lighting.rgb[2] > 0)
    );
    let token = prepare(&mut session);
    let settings = radiance_settings(LightingChannel::Hdr, 1.0);
    let old_serial = session.scene_serial();
    let report = session
        .set_compiled_details_lighting_options(Actor::User, token, settings.clone())
        .unwrap();
    assert!(session.scene_serial() > old_serial);
    assert_eq!(report["complete"], false);
    assert!(report["lighting"]["base_source_channel"].is_null());
    assert_eq!(
        report["lighting"]["base_policy"],
        "per_record_static_contributions"
    );
    assert_eq!(report["lighting"]["available_styles"], json!([1, 2]));
    assert_eq!(
        report["lighting"]["style_gains"],
        json!([{"style":1,"gain":1.0},{"style":2,"gain":0.5}])
    );
    session
        .set_compiled_details_visible(Actor::User, token, true)
        .unwrap();
    for channel in [LightingChannel::Hdr, LightingChannel::Ldr] {
        let settings = radiance_settings(channel, 1.0);
        let inspection = session
            .set_compiled_details_lighting_options(Actor::Agent, token, settings)
            .unwrap();
        let selected = if channel == LightingChannel::Hdr {
            &hdr
        } else {
            &ldr
        };
        for (ordinal, prop) in native.props.iter().enumerate() {
            let lighting = &inspection["page"]["rows"][ordinal]["lighting"];
            assert_eq!(
                lighting["static_contribution"]["source"],
                "selected_channel_style_zero"
            );
            assert_eq!(
                lighting["static_contribution"]["selected_channel"],
                channel.name()
            );
            assert!(lighting["static_contribution"]["shared_base_source_channel"].is_null());
            assert_eq!(lighting["static_contribution"]["style_zero"]["weight"], 1.0);
            assert_eq!(
                lighting["static_contribution"]["style_zero_policy"],
                "editor_unity"
            );
            let extras = inspection["page"]["rows"][ordinal]["lighting"]["style_extras"]
                .as_array()
                .unwrap();
            assert_eq!(extras.len(), 3);
            for (value, entry) in extras.iter().zip(
                &selected[prop.light_styles as usize
                    ..prop.light_styles as usize + usize::from(prop.light_style_count)],
            ) {
                assert_eq!(value["style"], entry.style);
                assert_eq!(value["rgb"], json!(entry.lighting.rgb));
                assert_eq!(value["exponent"], entry.lighting.exponent);
                assert_eq!(value["gain"], if entry.style == 2 { 0.5 } else { 1.0 });
                assert_eq!(value["effective_weight"], value["gain"]);
                assert_eq!(
                    value["linear"],
                    json!(entry.lighting.rgb.map(|channel| f64::from(channel)
                        * 2.0f64.powi(i32::from(entry.lighting.exponent))
                        / 255.0))
                );
            }
        }
        let mesh = session
            .compiled_details
            .ready
            .as_ref()
            .unwrap()
            .geometry
            .as_ref()
            .unwrap();
        assert_eq!(mesh.detail_parts.len(), 6);
        for part in &mesh.detail_parts {
            let prop = native.props[part.ordinal];
            let base_scale = 2.0f64.powi(i32::from(prop.lighting.exponent)) / 255.0;
            let table = if channel == LightingChannel::Hdr {
                &hdr
            } else {
                &ldr
            };
            let mut expected = prop.lighting.rgb.map(|value| f64::from(value) * base_scale);
            for entry in &table[prop.light_styles as usize
                ..prop.light_styles as usize + usize::from(prop.light_style_count)]
            {
                let weight = if entry.style == 2 { 0.5 } else { 1.0 };
                let scale = 2.0f64.powi(i32::from(entry.lighting.exponent)) * weight / 255.0;
                for (total, value) in expected.iter_mut().zip(entry.lighting.rgb) {
                    *total += f64::from(value) * scale;
                }
            }
            let expected = expected.map(|value| (value * 2.0) as f32);
            let exposed: [f64; 3] = serde_json::from_value(
                inspection["page"]["rows"][part.ordinal]["lighting"]["exposed_linear"].clone(),
            )
            .unwrap();
            assert_eq!(exposed.map(|value| value as f32), expected);
            assert!(!part.vertices.is_empty());
            assert!(mesh.vertices[part.vertices.clone()].iter().all(
                |vertex| vertex.baked_irradiance == expected
                    && vertex.lighting_receiver
                        == render::compiled_details::BAKED_RADIANCE_RECEIVER
                    && vertex.color[..3] == [255; 3]
            ));
        }
    }
    session
        .set_compiled_details_visible(Actor::User, token, false)
        .unwrap();
    let settings = radiance_settings(LightingChannel::Hdr, -1.0);
    session
        .set_compiled_details_lighting_options(Actor::User, token, settings.clone())
        .unwrap();
    session
        .set_compiled_details_visible(Actor::User, token, true)
        .unwrap();
    assert_eq!(session.compiled_details.lighting, settings);
    let token = prepare(&mut session);
    assert_eq!(
        session.compiled_details.lighting,
        LightingSettings::default()
    );
    session
        .set_compiled_details_lighting_options(
            Actor::User,
            token,
            radiance_settings(LightingChannel::Hdr, 0.0),
        )
        .unwrap();
    session.clear_compiled_details(Actor::User, token).unwrap();
    assert_eq!(
        session.compiled_details.lighting,
        LightingSettings::default()
    );
    let mut bsp = Bsp::read(&session.compiled.as_ref().unwrap().path).unwrap();
    let mut complete = native.clone();
    let mut complete_ldr = Vec::new();
    let mut complete_hdr = Vec::new();
    for prop in &mut complete.props {
        let old_offset = prop.light_styles as usize;
        prop.light_styles = complete_ldr.len() as u32;
        prop.light_style_count = 64;
        for (output, input) in [(&mut complete_ldr, &ldr), (&mut complete_hdr, &hdr)] {
            output.extend_from_slice(&input[old_offset..old_offset + 3]);
            output.extend(
                (3..=63).map(|style| source_formats::bsp::DetailPropLighting {
                    style,
                    lighting: Default::default(),
                }),
            );
        }
    }
    bsp.set_detail_props_with_lighting(&complete, &complete_ldr, Some(&complete_hdr))
        .unwrap();
    retain_native_fixture(&mut session, &bsp);
    let token = prepare(&mut session);
    let mut settings = radiance_settings(LightingChannel::Ldr, 0.0);
    settings
        .style_gains
        .push(render::compiled_details::StyleGain {
            style: 63,
            gain: 2.0,
        });
    let report = session
        .set_compiled_details_lighting_options(Actor::User, token, settings)
        .unwrap();
    assert_eq!(
        report["lighting"]["available_styles"],
        json!((1..=63).collect::<Vec<_>>())
    );
    assert_eq!(report["page"]["rows"][0]["light_style_count"], 64);
    assert_eq!(
        report["page"]["rows"][0]["lighting"]["style_extras"]
            .as_array()
            .unwrap()
            .len(),
        64
    );
    assert_eq!(
        report["page"]["rows"][0]["lighting"]["style_extras"][0]["effective_weight"],
        1.0
    );
    session
        .set_compiled_details_visible(Actor::User, token, true)
        .unwrap();
    session.clear_compiled_details(Actor::User, token).unwrap();
    assert_eq!(
        (
            session.document().vmf().clone(),
            session.camera(),
            session.history_snapshot(),
            std::fs::read(&source).unwrap()
        ),
        before
    );
}

#[test]
fn radiance_channel_availability_distinguishes_zero_extras_missing_tables_and_fullbright() {
    for (dual, independent_static) in [(false, false), (true, false), (true, true)] {
        let (_directory, mut session, _source) = radiance_fixture(dual, false, independent_static);
        let token = prepare(&mut session);
        let baked = &session.compiled_details.ready.as_ref().unwrap().baked;
        assert_eq!(
            baked.ldr.is_some() && baked.hdr.is_some(),
            independent_static
        );
        for channel in [LightingChannel::Ldr, LightingChannel::Hdr] {
            let settings = LightingSettings {
                mode: Lighting::BakedRadiance,
                channel,
                ..Default::default()
            };
            let before = session.compiled_details_observation();
            let result =
                session.set_compiled_details_lighting_options(Actor::User, token, settings);
            assert_eq!(result.is_ok(), dual || channel == LightingChannel::Ldr);
            if result.is_err() {
                assert_eq!(session.compiled_details_observation(), before);
            }
        }
        if independent_static {
            let report = session
                .set_compiled_details_lighting(Actor::User, token, "baked_ldr")
                .unwrap();
            assert_eq!(report["lighting"]["baked_ldr_available"], true);
            assert_eq!(report["lighting"]["available_styles"], json!([]));
            let ldr = Bsp::read(&session.compiled.as_ref().unwrap().path)
                .unwrap()
                .detail_prop_lighting(false)
                .unwrap()
                .unwrap();
            session
                .set_compiled_details_visible(Actor::User, token, true)
                .unwrap();
            let ready = session.compiled_details.ready.as_ref().unwrap();
            for part in &ready.geometry.as_ref().unwrap().detail_parts {
                let prop = ready.records.props[part.ordinal];
                let color = ldr[prop.light_styles as usize].lighting;
                let expected = color.rgb.map(|value| {
                    (f64::from(value) * 2.0f64.powi(i32::from(color.exponent)))
                        .clamp(0.0, 255.0)
                        .round() as u8
                });
                assert_ne!(expected, [0; 3]);
                assert!(
                    ready.geometry.as_ref().unwrap().vertices[part.vertices.clone()]
                        .iter()
                        .all(|vertex| vertex.color[..3] == expected)
                );
            }
        }
    }
    let (_directory, mut session, _source) = radiance_fixture(true, true, true);
    let mut bsp = Bsp::read(&session.compiled.as_ref().unwrap().path).unwrap();
    let hdr_lump = bsp
        .game_lumps
        .iter()
        .find(|lump| lump.id == *b"dplh")
        .unwrap()
        .clone();
    bsp.game_lumps.retain(|lump| lump.id != *b"dplh");
    retain_native_fixture(&mut session, &bsp);
    let token = prepare(&mut session);
    session
        .set_compiled_details_lighting_options(
            Actor::User,
            token,
            radiance_settings(LightingChannel::Ldr, 0.0),
        )
        .unwrap();
    let before = session.compiled_details_observation();
    assert!(
        session
            .set_compiled_details_lighting_options(
                Actor::User,
                token,
                radiance_settings(LightingChannel::Hdr, 0.0)
            )
            .unwrap_err()
            .contains("absent")
    );
    assert_eq!(session.compiled_details_observation(), before);
    assert!(
        session
            .set_compiled_details_lighting(Actor::User, token, "baked_ldr")
            .is_err()
    );
    let mut bsp = Bsp::read(&session.compiled.as_ref().unwrap().path).unwrap();
    bsp.game_lumps.push(hdr_lump);
    bsp.game_lumps.retain(|lump| lump.id != *b"dplt");
    retain_native_fixture(&mut session, &bsp);
    let token = prepare(&mut session);
    let inspected = session.inspect_compiled_details(Some(token), 0, 1).unwrap();
    assert!(
        inspected["page"]["rows"][0]["lighting"]["selected_table_unavailable_reason"]
            .as_str()
            .unwrap()
            .contains("absent")
    );
    let before = session.compiled_details_observation();
    assert!(
        session
            .set_compiled_details_lighting_options(
                Actor::User,
                token,
                radiance_settings(LightingChannel::Ldr, 0.0)
            )
            .unwrap_err()
            .contains("absent")
    );
    assert!(
        session
            .set_compiled_details_lighting(Actor::User, token, "baked_ldr")
            .unwrap_err()
            .contains("absent")
    );
    assert_eq!(session.compiled_details_observation(), before);
    let (_directory, mut session, _source) = fixture();
    let token = prepare(&mut session);
    assert!(
        session
            .set_compiled_details_lighting_options(
                Actor::User,
                token,
                LightingSettings {
                    mode: Lighting::BakedRadiance,
                    ..Default::default()
                }
            )
            .unwrap_err()
            .contains("Fullbright")
    );
}

#[test]
fn radiance_settings_refuse_invalid_or_stale_updates_without_mesh_or_options_changes() {
    let (_directory, mut session, _source) = radiance_fixture(true, true, true);
    let token = prepare(&mut session);
    session
        .set_compiled_details_visible(Actor::User, token, true)
        .unwrap();
    let valid = radiance_settings(LightingChannel::Hdr, 0.0);
    session
        .set_compiled_details_lighting_options(Actor::User, token, valid.clone())
        .unwrap();
    let original = session.compiled_details_observation();
    let vertices = session
        .compiled_details
        .ready
        .as_ref()
        .unwrap()
        .geometry
        .as_ref()
        .unwrap()
        .vertices
        .clone();
    let mut bad = Vec::new();
    for ev in [f64::NAN, f64::INFINITY, -9.0, 9.0] {
        let mut value = valid.clone();
        value.exposure_ev = ev;
        bad.push(value);
    }
    for (style, gain) in [
        (0, 1.0),
        (64, 1.0),
        (3, 1.0),
        (1, -0.1),
        (1, 2.1),
        (1, f64::NAN),
    ] {
        let mut value = valid.clone();
        value.style_gains = vec![render::compiled_details::StyleGain { style, gain }];
        bad.push(value);
    }
    let mut duplicate = valid.clone();
    duplicate.style_gains.push(duplicate.style_gains[0]);
    bad.push(duplicate);
    for value in bad {
        assert!(
            session
                .set_compiled_details_lighting_options(Actor::User, token, value)
                .is_err()
        );
        assert_eq!(session.compiled_details_observation(), original);
        assert_eq!(
            session
                .compiled_details
                .ready
                .as_ref()
                .unwrap()
                .geometry
                .as_ref()
                .unwrap()
                .vertices,
            vertices
        );
    }
    assert!(
        session
            .set_compiled_details_lighting_options(Actor::User, token + 1, valid.clone())
            .is_err()
    );
    assert_eq!(session.compiled_details_observation(), original);
    session
        .edit(Actor::User, "change source", |tx| {
            tx.world_properties_mut()
                .set("detailmaterial", "test/changed");
            Ok(())
        })
        .unwrap();
    assert!(
        session
            .set_compiled_details_lighting_options(Actor::User, token, valid)
            .is_err()
    );
}

#[test]
fn malformed_present_hdr_extras_fail_preparation_even_when_no_prop_uses_them() {
    let (_directory, mut session, _source) = radiance_fixture(true, false, false);
    let mut bsp = Bsp::read(&session.compiled.as_ref().unwrap().path).unwrap();
    assert!(
        bsp.detail_props()
            .unwrap()
            .unwrap()
            .props
            .iter()
            .all(|prop| prop.light_style_count == 0)
    );
    bsp.game_lumps.push(source_formats::bsp::GameLump {
        id: *b"dplh",
        version: 0,
        flags: 0,
        data: vec![1, 0, 0, 0],
    });
    retain_native_fixture(&mut session, &bsp);
    let request = session.begin_compiled_details(Actor::Agent).unwrap();
    let report = session
        .finish_compiled_details(Actor::Agent, request.run())
        .unwrap();
    assert_eq!(report["status"], "unavailable");
    assert_eq!(report["complete"], false);
    assert!(report["error"].as_str().unwrap().contains("truncated"));
}

#[test]
fn baked_ldr_modes_use_owned_literal_model_and_card_colors_and_preserve_state_on_refusal() {
    let (_directory, mut session, source) = fixture();
    let before = (
        session.document.vmf().clone(),
        session.camera(),
        session.history_snapshot(),
        std::fs::read(&source).unwrap(),
    );
    let mut bsp = Bsp::read(&session.compiled.as_ref().unwrap().path).unwrap();
    let mut native = bsp.detail_props().unwrap().unwrap();
    assert!(native.props.iter().any(|prop| prop.kind == 0));
    assert!(native.props.iter().any(|prop| prop.kind != 0));
    for prop in &mut native.props {
        prop.lighting = source_formats::bsp::RgbExp32 {
            rgb: if prop.kind == 0 {
                [64, 0, 128]
            } else {
                [128, 64, 0]
            },
            exponent: 0,
        };
    }
    bsp.set_detail_props(&native).unwrap();
    retain_native_fixture(&mut session, &bsp);
    let token = prepare(&mut session);
    let report = session
        .inspect_compiled_details(Some(token), 0, 64)
        .unwrap();
    assert_eq!(report["lighting"]["mode"], "editor_shading");
    assert_eq!(report["lighting"]["baked_ldr_available"], true);
    let untouched = session.compiled_details_observation();
    for (requested, mode) in [(token + 1, "baked_ldr"), (token, "hdr")] {
        assert!(
            session
                .set_compiled_details_lighting(Actor::User, requested, mode)
                .is_err()
        );
        assert_eq!(session.compiled_details_observation(), untouched);
    }
    let changed = session
        .set_compiled_details_lighting(Actor::User, token, "baked_ldr")
        .unwrap();
    assert_eq!(changed["lighting"]["mode"], "baked_ldr");
    assert_eq!(changed["displayed"], false);
    assert_eq!(changed["complete"], false);
    session
        .set_compiled_details_visible(Actor::Agent, token, true)
        .unwrap();
    let geometry = session
        .compiled_details
        .ready
        .as_ref()
        .unwrap()
        .geometry
        .as_ref()
        .unwrap();
    let baked = geometry.vertices.clone();
    let bounds = geometry
        .detail_parts
        .iter()
        .map(|part| part.bounds)
        .collect::<Vec<_>>();
    for part in &geometry.detail_parts {
        let expected = if native.props[part.ordinal].kind == 0 {
            [64, 0, 128]
        } else {
            [128, 64, 0]
        };
        assert!(!part.vertices.is_empty());
        for vertex in &geometry.vertices[part.vertices.clone()] {
            assert_eq!(vertex.color[..3], expected);
            assert_eq!(
                vertex.lighting_receiver,
                render::compiled_details::BAKED_LIGHTING_RECEIVER
            );
        }
    }
    session
        .set_compiled_details_lighting(Actor::User, token, "editor_shading")
        .unwrap();
    let geometry = session
        .compiled_details
        .ready
        .as_ref()
        .unwrap()
        .geometry
        .as_ref()
        .unwrap();
    assert!(
        geometry
            .vertices
            .iter()
            .all(|vertex| vertex.color[..3] == [255; 3] && vertex.lighting_receiver == 0)
    );
    assert_eq!(
        geometry
            .detail_parts
            .iter()
            .map(|part| part.bounds)
            .collect::<Vec<_>>(),
        bounds
    );
    session
        .set_compiled_details_lighting(Actor::Agent, token, "baked_ldr")
        .unwrap();
    assert_eq!(
        session
            .compiled_details
            .ready
            .as_ref()
            .unwrap()
            .geometry
            .as_ref()
            .unwrap()
            .vertices,
        baked
    );
    session
        .set_compiled_details_visible(Actor::User, token, false)
        .unwrap();
    assert_eq!(
        session.inspect_compiled_details(Some(token), 0, 1).unwrap()["lighting"]["mode"],
        "baked_ldr"
    );

    let shared_native = native.clone();
    let mut static_entries = Vec::new();
    for (ordinal, prop) in native.props.iter_mut().enumerate() {
        if ordinal.is_multiple_of(3) {
            continue;
        }
        let mut extra = prop.lighting;
        if ordinal % 3 == 1 {
            prop.lighting = source_formats::bsp::RgbExp32::default();
        } else {
            prop.lighting.rgb = prop.lighting.rgb.map(|value| value / 2);
            for (value, shared) in extra.rgb.iter_mut().zip(prop.lighting.rgb) {
                *value -= shared;
            }
        }
        prop.light_styles = static_entries.len() as u32;
        prop.light_style_count = 1;
        static_entries.push(source_formats::bsp::DetailPropLighting {
            lighting: extra,
            style: 0,
        });
    }
    bsp.set_detail_props_with_lighting(&native, &static_entries, None)
        .unwrap();
    retain_native_fixture(&mut session, &bsp);
    let token = prepare(&mut session);
    let report = session
        .set_compiled_details_lighting(Actor::User, token, "baked_ldr")
        .unwrap();
    assert!(report["lighting"]["base_source_channel"].is_null());
    assert_eq!(
        report["lighting"]["base_policy"],
        "per_record_static_contributions"
    );
    for (ordinal, row) in report["page"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
    {
        assert_eq!(
            row["lighting"]["static_contribution"]["source"],
            match ordinal % 3 {
                0 => "shared_native_base",
                1 => "selected_channel_style_zero",
                _ => "shared_native_base_and_selected_channel_style_zero",
            }
        );
    }
    session
        .set_compiled_details_visible(Actor::User, token, true)
        .unwrap();
    assert_eq!(
        session
            .compiled_details
            .ready
            .as_ref()
            .unwrap()
            .geometry
            .as_ref()
            .unwrap()
            .vertices,
        baked
    );
    native = shared_native;
    native.props.last_mut().unwrap().light_style_count = 1;
    bsp.set_detail_props_with_lighting(
        &native,
        &[source_formats::bsp::DetailPropLighting {
            lighting: source_formats::bsp::RgbExp32 {
                rgb: [240, 80, 20],
                exponent: -1,
            },
            style: 32,
        }],
        None,
    )
    .unwrap();
    retain_native_fixture(&mut session, &bsp);
    let token = prepare(&mut session);
    let report = session.inspect_compiled_details(Some(token), 0, 1).unwrap();
    assert_eq!(report["lighting"]["mode"], "editor_shading");
    assert_eq!(report["lighting"]["baked_ldr_available"], false);
    assert_eq!(report["lighting"]["ldr_style_records"], 1);
    assert!(
        report["lighting"]["unavailable_reason"]
            .as_str()
            .unwrap()
            .contains(&format!("Detail record {}", native.props.len() - 1))
    );
    session
        .set_compiled_details_visible(Actor::User, token, true)
        .unwrap();
    let untouched = session.compiled_details_observation();
    assert!(
        session
            .set_compiled_details_lighting(Actor::Agent, token, "baked_ldr")
            .unwrap_err()
            .contains("runtime light styles")
    );
    assert_eq!(session.compiled_details_observation(), untouched);
    assert!(
        session
            .compiled_details
            .ready
            .as_ref()
            .unwrap()
            .geometry
            .as_ref()
            .unwrap()
            .vertices
            .iter()
            .all(|vertex| vertex.color[..3] == [255; 3] && vertex.lighting_receiver == 0)
    );
    assert_eq!(
        (
            session.document.vmf().clone(),
            session.camera(),
            session.history_snapshot(),
            std::fs::read(source).unwrap()
        ),
        before
    );
}

#[test]
fn malformed_unused_ldr_style_table_is_not_silently_replaced_with_white() {
    let (_directory, mut session, _source) = fixture();
    let mut bsp = Bsp::read(&session.compiled.as_ref().unwrap().path).unwrap();
    let native = bsp.detail_props().unwrap().unwrap();
    assert!(native.props.iter().all(|prop| prop.light_style_count == 0));
    bsp.set_detail_props_with_lighting(&native, &[], None)
        .unwrap();
    bsp.game_lumps
        .iter_mut()
        .find(|lump| lump.id == *b"dplt")
        .unwrap()
        .data = vec![1, 0, 0, 0];
    retain_native_fixture(&mut session, &bsp);
    let request = session.begin_compiled_details(Actor::Agent).unwrap();
    let report = session
        .finish_compiled_details(Actor::Agent, request.run())
        .unwrap();
    assert_eq!(report["status"], "unavailable");
    assert_eq!(report["complete"], false);
    assert!(report["page"]["total"].is_null());
    assert!(
        report["error"]
            .as_str()
            .unwrap()
            .contains("truncated detail-prop records")
    );
}

#[test]
fn final_record_layer_preserves_authored_selection_picking_and_outline_controls() {
    let (_directory, mut session, _source) = fixture();
    let bsp = Bsp::read(&session.compiled.as_ref().unwrap().path).unwrap();
    let native = bsp.detail_props().unwrap().unwrap();
    assert!(
        native.props.len() > 4,
        "fixture must include actual generated records"
    );
    let token = prepare(&mut session);
    let report = session
        .inspect_compiled_details(Some(token), 0, 64)
        .unwrap();
    assert_eq!(report["page"]["total"], native.props.len());
    assert!(
        report["page"]["rows"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["source_association"].is_null())
    );
    for (row, prop) in report["page"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .zip(&native.props)
    {
        assert_eq!(row["origin"], json!(prop.origin));
        assert_eq!(row["leaf"], prop.leaf);
        assert_eq!(row["kind"], prop.kind);
    }
    assert!(session.inspect_compiled_details(None, 1, 1).is_err());
    assert!(
        session
            .inspect_compiled_details(Some(token + 1), 0, 1)
            .is_err()
    );
    assert!(
        session
            .inspect_compiled_details(Some(token), native.props.len() + 1, 1)
            .is_err()
    );
    assert_eq!(
        session
            .inspect_compiled_details(Some(token), native.props.len(), 1)
            .unwrap()["page"]["returned"],
        0
    );
    let selected = ObjectId::Solid(session.document.vmf().world.solids[0].id);
    session.select(&[selected], SelectMode::Replace).unwrap();
    let source_bytes = session.document.vmf().write(&Default::default());
    let revision = session.document.revision();
    let camera = session.camera();
    let ray = Ray {
        origin: glam::DVec3::new(0.0, 0.0, 128.0),
        direction: glam::DVec3::NEG_Z,
    };
    let hit = session.pick(&ray).map(|hit| hit.object);
    let ordinary = session.build_scene();
    let outline = ordinary.selection_outline_ranges.clone();
    let ordinary_vertices = ordinary.vertices.clone();
    let shown = session
        .set_compiled_details_visible(Actor::User, token, true)
        .unwrap();
    assert_eq!(shown["complete"], true);
    assert_eq!(shown["displayed"], true);
    let displayed = session.build_scene();
    assert!(displayed.vertices.len() > ordinary_vertices.len());
    assert_eq!(
        &displayed.vertices[..ordinary_vertices.len()],
        ordinary_vertices.as_slice()
    );
    assert_eq!(displayed.selection_outline_ranges, outline);
    assert_eq!(session.pick(&ray).map(|hit| hit.object), hit);
    assert_eq!(
        session.selection().iter().copied().collect::<Vec<_>>(),
        [selected]
    );
    assert_eq!(
        session.document.vmf().write(&Default::default()),
        source_bytes
    );
    assert_eq!(session.document.revision(), revision);
    assert_eq!(session.camera(), camera);
    let stamp = session.working_region_stamp();
    session
        .set_working_region(
            Actor::User,
            crate::session::working_region::Action::Set {
                enabled: true,
                bounds: [glam::DVec3::splat(10_000.0), glam::DVec3::splat(10_016.0)],
            },
            &stamp,
        )
        .unwrap();
    assert!(session.build_scene().indices.is_empty());
    let report = session.inspect_compiled_details(Some(token), 0, 1).unwrap();
    assert_eq!(report["displayed"], true);
    assert_eq!(report["view_policy"]["region_records"], 0);
    let stamp = session.working_region_stamp();
    session
        .set_working_region(
            Actor::User,
            crate::session::working_region::Action::Set {
                enabled: true,
                bounds: [
                    glam::DVec3::new(-2.0, -2.0, 63.0),
                    glam::DVec3::new(2.0, 2.0, 65.0),
                ],
            },
            &stamp,
        )
        .unwrap();
    assert!(!session.build_scene().indices.is_empty());
    let report = session.inspect_compiled_details(Some(token), 0, 1).unwrap();
    assert!(report["view_policy"]["region_records"].as_u64().unwrap() > 0);
    assert!(report["view_policy"]["region_records"].as_u64().unwrap() < native.props.len() as u64);
    let stamp = session.working_region_stamp();
    session
        .set_working_region(
            Actor::User,
            crate::session::working_region::Action::Clear,
            &stamp,
        )
        .unwrap();
    let stamp = session.working_region_stamp();
    session
        .set_editor_visibility(
            Actor::User,
            crate::session::editor_visibility::Action::Hide(vec![selected]),
            &stamp,
        )
        .unwrap();
    session.preflight_compiled_details();
    assert_eq!(
        session.inspect_compiled_details(Some(token), 0, 1).unwrap()["displayed"],
        true
    );
    let stamp = session.working_region_stamp();
    session
        .set_editor_visibility(
            Actor::User,
            crate::session::editor_visibility::Action::ShowAll,
            &stamp,
        )
        .unwrap();
    assert_eq!(session.document.revision(), revision);
    session.set_face_mode(true);
    session.preflight_compiled_details();
    assert_eq!(
        session.inspect_compiled_details(Some(token), 0, 1).unwrap()["status"],
        "context_suppressed"
    );
    session.set_face_mode(false);
    session.preflight_compiled_details();
    assert_eq!(
        session.inspect_compiled_details(Some(token), 0, 1).unwrap()["displayed"],
        true
    );
    session
        .set_compiled_details_visible(Actor::User, token, false)
        .unwrap();
    assert_eq!(
        session.build_scene().vertices.len(),
        ordinary_vertices.len()
    );
}

#[test]
fn cancelled_expired_and_stale_requests_cannot_restore_a_complete_layer() {
    let (_directory, mut session, _source) = fixture();
    let request = session.begin_compiled_details(Actor::Agent).unwrap();
    let token = request.token();
    assert!(session.begin_compiled_details(Actor::User).is_err());
    session
        .cancel_compiled_details(Actor::Agent, token)
        .unwrap();
    let cancelled = session
        .finish_compiled_details(Actor::Agent, request.run())
        .unwrap();
    assert_eq!(cancelled["status"], "cancelled");
    assert_eq!(cancelled["complete"], false);
    let request = session.begin_compiled_details(Actor::Agent).unwrap();
    session
        .clear_compiled_details(Actor::User, request.token())
        .unwrap();
    assert!(
        session
            .finish_compiled_details(Actor::Agent, request.run())
            .is_err()
    );
    let token = prepare(&mut session);
    session
        .set_compiled_details_visible(Actor::User, token, true)
        .unwrap();
    session
        .edit(Actor::User, "Change committed detail source", |tx| {
            tx.world_properties_mut()
                .set("detailmaterial", "test/other");
            Ok(())
        })
        .unwrap();
    let stale = session
        .inspect_compiled_details(Some(token), 0, 64)
        .unwrap();
    assert_eq!(stale["status"], "stale");
    assert_eq!(stale["displayed"], false);
    assert!(stale["page"]["total"].as_u64().unwrap() > 0);
    assert!(
        session
            .set_compiled_details_visible(Actor::Agent, token, true)
            .is_err()
    );
    assert!(session.begin_compiled_details(Actor::Agent).is_err());
    assert!(
        session
            .set_compiled_details_lighting(Actor::Agent, token, "baked_ldr")
            .is_err()
    );
}

#[test]
fn preview_texture_observation_is_separate_and_output_replacement_retires_rows() {
    let (directory, mut session, _source) = fixture();
    let token = prepare(&mut session);
    session
        .set_compiled_details_visible(Actor::User, token, true)
        .unwrap();
    let texture = directory.path().join("materials/test/checker.vtf");
    let mut bytes = std::fs::read(&texture).unwrap();
    bytes[80] ^= 0xff;
    std::fs::write(&texture, bytes).unwrap();
    let build =
        baked_build::build_state_controlled(&session, SOURCE_MODE, &Control::default()).unwrap();
    assert_eq!(
        build["current"], true,
        "Model preview textures are not compiler-consumed in this fixture: {build}"
    );
    let stale = session.inspect_compiled_details(Some(token), 0, 1).unwrap();
    assert_eq!(stale["status"], "stale");
    assert_eq!(stale["displayed"], false);
    assert_eq!(stale["build"]["current"], true);
    assert_eq!(stale["preview_assets"]["status"], "stale");
    assert!(
        stale["preview_assets"]["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .any(|diagnostic| diagnostic
                .as_str()
                .unwrap()
                .contains("Preview asset observations"))
    );
    let path = session.compiled.as_ref().unwrap().path.clone();
    std::fs::write(path, b"replacement output").unwrap();
    let report = session.inspect_compiled_details(Some(token), 0, 1).unwrap();
    assert_eq!(report["status"], "unavailable");
    assert!(report["page"]["total"].is_null());
    assert_eq!(report["complete"], false);
}

#[test]
fn unreadable_preview_only_texture_refuses_the_whole_geometry_set() {
    let (directory, mut session, _source) = fixture();
    let token = prepare(&mut session);
    let source = session.document.vmf().write(&Default::default());
    let ordinary = session.build_scene().vertices.len();
    std::fs::write(
        directory.path().join("materials/test/checker.vtf"),
        b"unsupported preview texture",
    )
    .unwrap();
    assert_eq!(
        baked_build::build_state_controlled(&session, SOURCE_MODE, &Control::default()).unwrap()["current"],
        true
    );
    assert!(
        session
            .set_compiled_details_visible(Actor::Agent, token, true)
            .is_err()
    );
    let report = session.inspect_compiled_details(Some(token), 0, 1).unwrap();
    assert_eq!(report["status"], "unavailable");
    assert_eq!(report["complete"], false);
    assert_eq!(report["displayed"], false);
    assert!(report["error"].as_str().unwrap().contains("VTF"));
    assert_eq!(session.build_scene().vertices.len(), ordinary);
    assert_eq!(session.document.vmf().write(&Default::default()), source);
}

#[test]
fn absent_and_empty_native_atlas_declarations_compile_prepare_and_show_default_assets() {
    assert_eq!(DetailProps::sprite_material_name(Some(" ")), " ");
    assert_eq!(
        DetailProps::sprite_material_name(Some(" test/atlas ")),
        " test/atlas "
    );
    for declaration in [None, Some("")] {
        let (_directory, mut session, source) = fixture_with_atlas(declaration);
        let original = std::fs::read(&source).unwrap();
        let bsp = Bsp::read(&session.compiled.as_ref().unwrap().path).unwrap();
        let blocks = source_formats::keyvalues::parse_flat_blocks_bounded(
            bsp.entities().unwrap(),
            FlatBlockLimits {
                max_bytes: 1024 * 1024,
                max_blocks: 8192,
                max_pairs: 65_536,
            },
        )
        .unwrap();
        let world = blocks
            .iter()
            .find(|block| {
                block
                    .iter()
                    .any(|(key, value)| key == "classname" && value == "worldspawn")
            })
            .unwrap();
        let emitted = world
            .iter()
            .find(|(key, _)| key == "detailmaterial")
            .map(|(_, value)| value.as_str());
        assert_eq!(emitted, declaration);
        let native = bsp.detail_props().unwrap().unwrap();
        assert!(native.props.len() > 4);
        let token = prepare(&mut session);
        let report = session
            .inspect_compiled_details(Some(token), 0, 64)
            .unwrap();
        assert_eq!(report["sprite_material"], "detail/detailsprites");
        assert_eq!(report["page"]["total"], native.props.len());
        let shown = session
            .set_compiled_details_visible(Actor::User, token, true)
            .unwrap();
        assert_eq!(shown["complete"], true, "{shown}");
        assert_eq!(shown["displayed"], true, "{shown}");
        let atlas = session.materials.find("detail/detailsprites").unwrap();
        assert_eq!(
            atlas.display_texture.as_deref(),
            Some("materials/detail/detailsprites.vtf")
        );
        assert_eq!(atlas.base.as_ref().unwrap().width, 32);
        assert!(!session.build_scene().indices.is_empty());
        assert_eq!(std::fs::read(source).unwrap(), original);
    }
}
