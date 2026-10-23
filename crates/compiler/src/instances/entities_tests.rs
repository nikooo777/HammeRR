use source_formats::bsp::{LightingKind, lump, world_light_kind};

use super::*;

fn static_light(id: u32, class: &str, origin: &str) -> Entity {
    let mut entity = Entity::new(id, class);
    for (key, value) in [
        ("_constant_attn", "0"),
        ("_distance", "0"),
        ("_fifty_percent_distance", "0"),
        ("_hardfalloff", "0"),
        ("_light", "255 96 32 300"),
        ("_lightHDR", "32 96 255 450"),
        ("_lightscaleHDR", "1"),
        ("_linear_attn", "0"),
        ("_quadratic_attn", "1"),
        ("_zero_percent_distance", "0"),
        ("pattern", ""),
        ("spawnflags", "0"),
        ("style", "0"),
        ("origin", origin),
    ] {
        entity.properties.set(key, value);
    }
    if class == "light_spot" {
        for (key, value) in [
            ("angles", "0 0 0"),
            ("pitch", "-90"),
            ("_inner_cone", "30"),
            ("_cone", "45"),
            ("_exponent", "1"),
        ] {
            entity.properties.set(key, value);
        }
    }
    entity
}

fn emitter_fixture(lights: Vec<Entity>) -> (tempfile::TempDir, Vmf, Vmf, GameFs) {
    let (directory, mut parent, _, game) = fixture();
    parent
        .entities
        .last_mut()
        .unwrap()
        .properties
        .set("origin", "16 -8 4");
    let child = helper_file(lights);
    write_child(directory.path(), "child.vmf", &child);
    (directory, parent, child, game)
}

fn independent_beam(entity: &Entity) -> DVec3 {
    let pitch = entity
        .properties
        .get("pitch")
        .unwrap()
        .parse::<f64>()
        .unwrap()
        .to_radians();
    let yaw = vector(entity, "angles").unwrap().y.to_radians();
    DVec3::new(
        pitch.cos() * yaw.cos(),
        pitch.cos() * yaw.sin(),
        pitch.sin(),
    )
}

fn flattened_emitters(parent: &Vmf, child: &Vmf) -> Vmf {
    let mut document = Document::new(parent.clone());
    document
        .edit("independent emitter control", |tx| {
            for helper in parent.entities.iter().filter(|entity| {
                entity
                    .classname()
                    .is_some_and(|class| class.eq_ignore_ascii_case("func_instance"))
            }) {
                let origin = vector(helper, "origin").unwrap();
                let angles = vector(helper, "angles").unwrap();
                let matrix = source_matrix(angles);
                for original in child.world.solids.iter().filter(|solid| !solid.hidden) {
                    let mut solid = manual_solid(original, origin, angles);
                    solid.id = tx.new_object_id()?;
                    for side in &mut solid.sides {
                        side.id = tx.new_side_id()?;
                        if let Some(disp) = &mut side.displacement {
                            assert_eq!(angles, DVec3::ZERO);
                            disp.start_position += origin;
                        }
                    }
                    tx.add_solid(solid)?;
                }
                for original in child.entities.iter().filter(|entity| !entity.hidden) {
                    let mut entity = original.clone();
                    entity.id = tx.new_object_id()?;
                    entity.editor = None;
                    let position = matrix * original.origin().unwrap() + origin;
                    entity.properties.set("origin", format_vec3(position));
                    if angles != DVec3::ZERO {
                        if original.classname() == Some("light_spot") {
                            let direction = matrix * independent_beam(original);
                            let pitch = direction.z.clamp(-1.0, 1.0).asin().to_degrees();
                            let yaw = direction.y.atan2(direction.x).to_degrees();
                            entity
                                .properties
                                .set("angles", format_vec3(DVec3::new(-pitch, yaw, 0.0)));
                            entity
                                .properties
                                .set("pitch", source_formats::vmf::format_number(pitch));
                        } else {
                            assert!(original.properties.get("angles").is_none());
                            entity.properties.set("angles", format_vec3(angles));
                        }
                        entity.properties.remove("angle");
                    }
                    tx.add_entity(entity)?;
                }
            }
            for helper in parent.entities.iter().filter(|entity| {
                entity
                    .classname()
                    .is_some_and(|class| class.eq_ignore_ascii_case("func_instance"))
            }) {
                tx.remove(ObjectId::Entity(helper.id))?;
            }
            Ok(())
        })
        .unwrap();
    document.into_vmf()
}

fn emitter_output(
    parent: &Vmf,
    game: &GameFs,
    source: Option<&SourceContext>,
    lighting: crate::LightingMode,
) -> crate::Output {
    let output = compile_with_source_controlled(
        parent,
        game,
        Options {
            lighting,
            ..Options::default()
        },
        None,
        source,
        &Control::default(),
    );
    assert!(!output.report.has_errors(), "{:?}", output.report);
    assert!(output.bsp.is_some());
    output
}

#[test]
fn fgd_neutral_defaults_and_hdr_fallback_are_admitted_with_exact_static_identity() {
    for class in ["light", "light_spot"] {
        let mut light = static_light(2, class, "0 0 96");
        for (key, value) in [
            ("_lightHDR", "-1 -1 -1 1"),
            ("defaultstyle", "0"),
            ("targetname", ""),
            ("target", ""),
            ("parentname", ""),
            ("name", ""),
            ("globalname", ""),
            ("killtarget", ""),
            ("lightingorigin", ""),
        ] {
            light.properties.set(key, value);
        }
        entities::admit(&light, entities::kind(&light).unwrap()).unwrap();
        light.properties.set("targetname", " ");
        assert!(
            entities::admit(&light, entities::kind(&light).unwrap())
                .unwrap_err()
                .contains("empty targetname")
        );
    }
}

#[test]
fn child_appearances_preserve_offered_ids_scalar_case_and_neutral_spellings() {
    for class in ["light", "light_spot"] {
        for value in (0..=11)
            .map(|value| value.to_string())
            .chain(["+1", "01", " 11 "].map(str::to_owned))
        {
            for key in ["style", "Style", "STYLE"] {
                let mut light = static_light(20, class, "0 0 96");
                light.properties.remove("style");
                light.properties.set(key, value.clone());
                let before = light.clone();
                let result = entities::admit(&light, entities::kind(&light).unwrap());
                assert!(result.is_ok(), "{class} {key}={value:?}: {result:?}");
                assert_eq!(light, before);
            }
        }
        let mut absent = static_light(20, class, "0 0 96");
        absent.properties.remove("style");
        let before = absent.clone();
        entities::admit(&absent, entities::kind(&absent).unwrap()).unwrap();
        assert_eq!(absent, before);
        for key in ["style", "DefaultStyle", "SpawnFlags"] {
            for value in ["0", "+0", "00", "-0", "0.0", "0e0", " -0.0 ", "1e-9999"] {
                let mut light = static_light(20, class, "0 0 96");
                light.properties.set("style", "11");
                light.properties.remove(key);
                light.properties.set(key, value);
                let before = light.clone();
                let result = entities::admit(&light, entities::kind(&light).unwrap());
                assert!(result.is_ok(), "{class} {key}={value:?}: {result:?}");
                assert_eq!(light, before);
            }
        }
    }
}

#[test]
fn child_appearances_reject_unoffered_values_and_non_neutral_runtime_fields() {
    for class in ["light", "light_spot"] {
        for value in [
            "", " ", "12", "31", "32", "255", "256", "-1", "1.0", "1e0", "NaN", "inf",
        ] {
            let mut light = static_light(20, class, "0 0 96");
            light.properties.set("style", value);
            let before = light.clone();
            let error = entities::admit(&light, entities::kind(&light).unwrap()).unwrap_err();
            assert!(error.contains("style"), "{class} style={value:?}: {error}");
            assert_eq!(light, before);
        }
        for (key, value) in [
            ("defaultstyle", ""),
            ("defaultstyle", "1"),
            ("spawnflags", " "),
            ("spawnflags", "1"),
            ("pattern", "az"),
            ("targetname", "lamp"),
            ("pattern", " "),
            ("targetname", " "),
        ] {
            let mut light = static_light(20, class, "0 0 96");
            light.properties.set("style", "11");
            light.properties.set(key, value);
            assert!(
                entities::admit(&light, entities::kind(&light).unwrap()).is_err(),
                "{class} {key}={value:?}"
            );
        }
        for key in ["STYLE", "DefaultStyle", "SpawnFlags"] {
            let mut light = static_light(20, class, "0 0 96");
            light.properties.set("style", "11");
            light.properties.set("defaultstyle", "0");
            light.properties.0.push((key.into(), "0".into()));
            assert!(
                entities::admit(&light, entities::kind(&light).unwrap())
                    .unwrap_err()
                    .contains("repeats property"),
                "{class} {key}"
            );
        }
    }
}

#[test]
fn translated_child_emitters_and_terrain_match_literal_fullbright_ldr_and_hdr_bsp() {
    let (directory, parent, child, game) = emitter_fixture(vec![
        static_light(20, "light", "-48 16 128"),
        static_light(21, "light_spot", "64 -16 160"),
    ]);
    let mut document = Document::new(child);
    document
        .edit("terrain receiver", |tx| {
            let ObjectId::Solid(id) = edit::create_block(
                tx,
                DVec3::new(-32.0, -32.0, 8.0),
                DVec3::new(32.0, 32.0, 16.0),
                "dev/terrain",
            )?
            else {
                unreachable!()
            };
            let solid = tx.document().solid(id).unwrap();
            let geometry = BrushGeometry::from_solid(solid).unwrap();
            let face = geometry
                .faces
                .iter()
                .find(|face| face.plane.normal.z > 0.99)
                .unwrap();
            let side = solid.sides[face.side].id;
            map_core::displacement::edit::create(
                tx,
                &[map_core::surface::FaceId { solid: id, side }],
                2,
                0,
            )?;
            Ok(())
        })
        .unwrap();
    let child = document.into_vmf();
    write_child(directory.path(), "child.vmf", &child);
    let before = std::fs::read(directory.path().join("child.vmf")).unwrap();
    let context = source(directory.path());
    let flat = flattened_emitters(&parent, &child);
    for lighting in [
        crate::LightingMode::Fullbright,
        crate::LightingMode::DirectLdr,
        crate::LightingMode::DirectLdrHdr,
    ] {
        let actual = emitter_output(&parent, &game, Some(&context), lighting);
        let expected = emitter_output(&flat, &game, None, lighting);
        let stats = actual.stats.instances.as_ref().unwrap();
        assert_eq!(
            (
                stats.point_lights,
                stats.spot_lights,
                stats.terrain_patches,
                stats.terrain_vertices
            ),
            (1, 1, 1, 25)
        );
        let actual = actual.bsp.unwrap();
        assert_eq!(
            actual.to_bytes(),
            expected.bsp.unwrap().to_bytes(),
            "{lighting:?}"
        );
        if lighting != crate::LightingMode::Fullbright {
            let ldr = actual.world_lights(LightingKind::Ldr).unwrap();
            assert_eq!(ldr.len(), 2);
            assert_eq!(
                (ldr[0].kind, ldr[1].kind),
                (world_light_kind::POINT, world_light_kind::SPOT)
            );
            assert!(
                ldr.iter()
                    .all(|light| light.style == 0 && light.owner == -1)
            );
            assert!(
                actual
                    .lighting(LightingKind::Ldr)
                    .unwrap()
                    .unwrap()
                    .samples()
                    .iter()
                    .any(|sample| sample.rgb != [0; 3])
            );
        }
        if lighting == crate::LightingMode::DirectLdrHdr {
            assert_ne!(
                actual.lump(lump::LIGHTING).data,
                actual.lump(lump::LIGHTING_HDR).data
            );
            assert_ne!(
                actual.world_lights(LightingKind::Ldr).unwrap()[0].intensity,
                actual.world_lights(LightingKind::Hdr).unwrap()[0].intensity
            );
        }
    }
    assert_eq!(
        std::fs::read(directory.path().join("child.vmf")).unwrap(),
        before
    );
}

#[test]
fn quarter_turn_emitters_match_independent_beams_world_lights_and_lightmaps() {
    let mut spot = static_light(21, "light_spot", "64 -16 160");
    spot.properties.set("pitch", "-45");
    let (directory, mut parent, child, game) =
        emitter_fixture(vec![static_light(20, "light", "-48 16 128"), spot]);
    parent
        .entities
        .last_mut()
        .unwrap()
        .properties
        .set("angles", "0 90 0");
    let context = source(directory.path());
    let flat = flattened_emitters(&parent, &child);
    let actual = emitter_output(
        &parent,
        &game,
        Some(&context),
        crate::LightingMode::DirectLdrHdr,
    )
    .bsp
    .unwrap();
    let expected = emitter_output(&flat, &game, None, crate::LightingMode::DirectLdrHdr)
        .bsp
        .unwrap();
    for index in 0..actual.lumps.len() {
        if index != lump::ENTITIES && index != lump::WORLDLIGHTS && index != lump::WORLDLIGHTS_HDR {
            assert_eq!(actual.lump(index), expected.lump(index), "lump {index}");
        }
    }
    for channel in [LightingKind::Ldr, LightingKind::Hdr] {
        let actual = actual.world_lights(channel).unwrap();
        let expected = expected.world_lights(channel).unwrap();
        assert_eq!(actual.len(), 2);
        for (a, b) in actual.iter().zip(expected) {
            assert!(
                glam::Vec3::from_array(a.origin)
                    .abs_diff_eq(glam::Vec3::from_array(b.origin), 1e-6)
            );
            assert_eq!(
                (a.kind, a.style, a.intensity),
                (b.kind, b.style, b.intensity)
            );
            assert!(
                glam::Vec3::from_array(a.normal)
                    .abs_diff_eq(glam::Vec3::from_array(b.normal), 1e-6)
            );
        }
    }
    assert!(
        actual
            .entities()
            .unwrap()
            .contains("\"classname\" \"light_spot\"")
    );
    assert!(actual.entities().unwrap().contains("\"hammerid\""));
}

#[test]
fn nested_spotlight_beams_follow_independent_noncommuting_matrices_at_every_depth() {
    for poses in [
        vec![DVec3::new(90.0, 0.0, 0.0)],
        vec![DVec3::new(-90.0, 0.0, 0.0), DVec3::new(0.0, 90.0, 0.0)],
        vec![
            DVec3::new(20.0, 35.0, 10.0),
            DVec3::new(-15.0, 70.0, 25.0),
            DVec3::new(30.0, -20.0, 45.0),
            DVec3::new(-10.0, 25.0, -35.0),
        ],
    ] {
        let lights = [
            (0.0, 0.0, 0.0),
            (-90.0, 0.0, 0.0),
            (90.0, 50.0, 0.0),
            (-25.0, 35.0, 70.0),
        ]
        .into_iter()
        .enumerate()
        .map(|(index, (pitch, yaw, roll))| {
            let mut light = static_light(20 + index as u32, "light_spot", "8 -4 96");
            light.properties.set("pitch", pitch.to_string());
            light.properties.set("angles", format!("0 {yaw} {roll}"));
            light
        })
        .collect::<Vec<_>>();
        let (directory, mut parent, child, _) = emitter_fixture(lights);
        let mut file = "child.vmf".to_owned();
        for (level, angles) in poses.iter().enumerate().skip(1) {
            let name = format!("level{level}.vmf");
            write_child(
                directory.path(),
                &name,
                &helper_file(vec![nested_helper(
                    100,
                    &file,
                    "3 -5 7",
                    &format_vec3(*angles),
                )]),
            );
            file = name;
        }
        let root = parent.entities.last_mut().unwrap();
        root.properties.set("file", &file);
        root.properties.set("origin", "3 -5 7");
        root.properties.set("angles", format_vec3(poses[0]));
        let owner = root.id;
        let context = source(directory.path());
        let prepared = prepare(
            &parent,
            Some(&context),
            &Control::default(),
            &mut Report::default(),
        )
        .unwrap();
        let stats = prepared.stats().unwrap();
        assert_eq!(
            (
                stats.spot_lights,
                stats.point_lights,
                stats.max_expansion_depth
            ),
            (4, 0, poses.len())
        );
        let generated = prepared
            .vmf()
            .entities
            .iter()
            .filter(|entity| entity.classname() == Some("light_spot"))
            .collect::<Vec<_>>();
        for (actual, original) in generated.into_iter().zip(&child.entities) {
            let mut direction = independent_beam(original);
            let mut position = original.origin().unwrap();
            for angles in poses.iter().skip(1).chain(poses.first()) {
                direction = source_matrix(*angles) * direction;
                position = source_matrix(*angles) * position + DVec3::new(3.0, -5.0, 7.0);
            }
            let mapped = map_core::light::direction(
                actual.properties.get("angles"),
                actual.properties.get("angle"),
                actual.properties.get("pitch"),
                actual.origin().unwrap(),
                None,
            )
            .unwrap();
            assert!(
                mapped.abs_diff_eq(direction, 1e-9),
                "{poses:?}: {mapped:?} != {direction:?}"
            );
            assert!(actual.origin().unwrap().abs_diff_eq(position, 1e-9));
            assert_eq!(prepared.root_entity(actual.id), Some(owner));
            let mut report = Report::default();
            report.diagnostics.push(Diagnostic::warning(
                "leaf warning",
                Some(format!("entity:{}", actual.id)),
                actual.origin(),
            ));
            prepared.annotate(&mut report);
            assert_eq!(
                report.diagnostics[0].object,
                Some(format!("entity:{owner}"))
            );
            assert!(
                report.diagnostics[0]
                    .message
                    .contains(&format!("instance child.vmf, child object {}", original.id))
            );
        }
    }
}

#[test]
fn repeated_diamond_lights_share_first_dependencies_and_keep_root_leaf_provenance() {
    let (directory, mut parent, _, _) = emitter_fixture(vec![
        static_light(100, "light", "0 0 96"),
        static_light(101, "light_spot", "32 0 128"),
    ]);
    write_child(
        directory.path(),
        "middle.vmf",
        &helper_file(vec![
            nested_helper(100, "child.vmf", "-32 0 0", "0 0 0"),
            nested_helper(101, "child.vmf", "32 0 0", "0 90 0"),
        ]),
    );
    let root = parent.entities.last_mut().unwrap();
    root.properties.set("file", "middle.vmf");
    let first = root.id;
    let second = first + 1;
    parent
        .entities
        .push(nested_helper(second, "middle.vmf", "-80 0 0", "0 0 0"));
    let before = parent.clone();
    let context = source(directory.path());
    let prepared = prepare(
        &parent,
        Some(&context),
        &Control::default(),
        &mut Report::default(),
    )
    .unwrap();
    let stats = prepared.stats().unwrap();
    assert_eq!(
        (
            stats.instances,
            stats.files,
            stats.point_lights,
            stats.spot_lights
        ),
        (6, 2, 4, 4)
    );
    for (owner, count) in [(first, 4), (second, 4)] {
        assert_eq!(
            prepared
                .vmf()
                .entities
                .iter()
                .filter(|entity| prepared.root_entity(entity.id) == Some(owner))
                .count(),
            count
        );
    }
    assert_eq!(parent, before);
    prepared
        .dependencies()
        .unwrap()
        .verify(&Control::default())
        .unwrap();
    let mut changed = std::fs::read(directory.path().join("child.vmf")).unwrap();
    changed.extend_from_slice(b"\n");
    std::fs::write(directory.path().join("child.vmf"), changed).unwrap();
    assert!(
        prepared
            .dependencies()
            .unwrap()
            .verify(&Control::default())
            .is_err()
    );
}

#[test]
fn parent_region_selects_transformed_light_origins_including_its_boundary() {
    let (directory, mut parent, child, game) = emitter_fixture(vec![
        static_light(20, "light", "-512 0 96"),
        static_light(21, "light_spot", "-384 0 128"),
    ]);
    let first = parent.entities.last_mut().unwrap();
    first.properties.set("origin", "512 0 0");
    let second = first.id + 1;
    parent
        .entities
        .push(nested_helper(second, "child.vmf", "0 0 0", "0 0 0"));
    parent.cordon = Some(Node::block(
        "cordon",
        vec![
            Node::string("active", "1"),
            Node::string("mins", "-128 -128 0"),
            Node::string("maxs", "128 128 256"),
        ],
    ));
    let context = source(directory.path());
    let flat = flattened_emitters(&parent, &child);
    let actual = emitter_output(
        &parent,
        &game,
        Some(&context),
        crate::LightingMode::DirectLdrHdr,
    );
    let expected = emitter_output(&flat, &game, None, crate::LightingMode::DirectLdrHdr);
    assert!(actual.stats.compile_region.is_some());
    let stats = actual.stats.instances.as_ref().unwrap();
    assert_eq!((stats.point_lights, stats.spot_lights), (2, 2));
    let actual = actual.bsp.unwrap();
    assert_eq!(actual.to_bytes(), expected.bsp.unwrap().to_bytes());
    assert!(
        actual
            .entities()
            .unwrap()
            .contains("\"origin\" \"128 0 128\"")
    );
    for channel in [LightingKind::Ldr, LightingKind::Hdr] {
        let lights = actual.world_lights(channel).unwrap();
        assert!(lights.iter().any(|light| light.origin == [0.0, 0.0, 96.0]));
        assert!(
            lights
                .iter()
                .all(|light| (0.0..=128.0).contains(&light.origin[0]))
        );
    }
}

#[test]
fn child_light_refusals_are_mode_independent_and_preserve_previous_files() {
    let (directory, parent, _, game) = emitter_fixture(Vec::new());
    let owner = parent.entities.last().unwrap().id;
    let context = source(directory.path());
    let destination = directory.path().join("previous.bsp");
    std::fs::write(&destination, b"previous BSP").unwrap();
    std::fs::write(destination.with_extension("lin"), b"previous LIN").unwrap();
    for issue in [
        "targetname",
        "whitespace target",
        "target",
        "parentname",
        "style",
        "reserved style",
        "defaultstyle",
        "pattern",
        "spawnflags",
        "unknown",
        "duplicate",
        "canonical origin",
        "canonical class",
        "opaque",
        "connections",
        "brush",
        "missing origin",
        "origin bounds",
        "origin NaN",
        "origin f32",
        "angles",
        "brightness",
        "hdr",
        "hdr scale",
        "attenuation",
        "missing pitch",
        "pitch range",
        "pitch NaN",
        "zero pitch fallback",
        "legacy angle",
        "cone",
        "focus",
        "dynamic",
        "sun",
        "wrong class case",
    ] {
        let mut light = static_light(20, "light_spot", "0 0 96");
        match issue {
            "targetname" => light.properties.set("targetname", "switchable"),
            "whitespace target" => light.properties.set("target", " "),
            "target" => light.properties.set("target", "aim"),
            "parentname" => light.properties.set("parentname", "mover"),
            "style" => light.properties.set("style", "12"),
            "reserved style" => light.properties.set("style", "32"),
            "defaultstyle" => light.properties.set("defaultstyle", "2"),
            "pattern" => light.properties.set("pattern", "az"),
            "spawnflags" => light.properties.set("spawnflags", "1"),
            "unknown" => light.properties.set("StartDisabled", "0"),
            "duplicate" => light
                .properties
                .0
                .push(("_LIGHT".into(), "255 255 255 100".into())),
            "canonical origin" => {
                light
                    .properties
                    .0
                    .iter_mut()
                    .find(|(key, _)| key == "origin")
                    .unwrap()
                    .0 = "Origin".into()
            }
            "canonical class" => light.properties.0[0].0 = "ClassName".into(),
            "opaque" => light.extra.push(Node::block("unknown", Vec::new())),
            "connections" => {
                light.connections = Some(vec![source_formats::vmf::Connection {
                    output: "OnUser1".into(),
                    value: "target,TurnOn,,0,-1".into(),
                }])
            }
            "brush" => light
                .solids
                .push(test_common::room().world.solids.remove(0)),
            "missing origin" => {
                light.properties.remove("origin");
            }
            "origin bounds" => light.properties.set("origin", "16385 0 96"),
            "origin NaN" => light.properties.set("origin", "NaN 0 96"),
            "origin f32" => light.properties.set("origin", "1e50 0 96"),
            "angles" => light.properties.set("angles", "0 inf 0"),
            "brightness" => light.properties.set("_light", "255 255 255 -1"),
            "hdr" => light.properties.set("_lightHDR", "not a light"),
            "hdr scale" => light.properties.set("_lightscaleHDR", "-1"),
            "attenuation" => light.properties.set("_quadratic_attn", "NaN"),
            "missing pitch" => {
                light.properties.remove("pitch");
            }
            "pitch range" => light.properties.set("pitch", "91"),
            "pitch NaN" => light.properties.set("pitch", "NaN"),
            "zero pitch fallback" => {
                light.properties.set("pitch", "0");
                light.properties.set("angles", "30 0 0");
            }
            "legacy angle" => light.properties.set("angle", "-1"),
            "cone" => light.properties.set("_cone", "15"),
            "focus" => light.properties.set("_exponent", "-1"),
            "dynamic" => light.properties.set("classname", "light_dynamic"),
            "sun" => light.properties.set("classname", "light_environment"),
            "wrong class case" => light.properties.set("classname", "Light_Spot"),
            _ => unreachable!(),
        }
        write_child(directory.path(), "child.vmf", &helper_file(vec![light]));
        for lighting in [
            crate::LightingMode::Fullbright,
            crate::LightingMode::DirectLdr,
            crate::LightingMode::DirectLdrHdr,
        ] {
            let output = compile_with_source_controlled(
                &parent,
                &game,
                Options {
                    lighting,
                    ..Options::default()
                },
                None,
                Some(&context),
                &Control::default(),
            );
            assert!(output.report.has_errors(), "{issue}, {lighting:?}");
            assert!(output.bsp.is_none() && output.leak.is_none(), "{issue}");
            assert!(
                output
                    .report
                    .diagnostics
                    .iter()
                    .any(|diagnostic| diagnostic.object == Some(format!("entity:{owner}"))),
                "{issue}"
            );
            assert_eq!(std::fs::read(&destination).unwrap(), b"previous BSP");
            assert_eq!(
                std::fs::read(destination.with_extension("lin")).unwrap(),
                b"previous LIN"
            );
        }
    }
}

#[test]
fn intermediate_child_light_bounds_fail_before_an_outer_pose_returns_inside() {
    let (directory, mut parent, _, _) = emitter_fixture(vec![static_light(20, "light", "8 0 96")]);
    write_child(
        directory.path(),
        "middle.vmf",
        &helper_file(vec![nested_helper(100, "child.vmf", "16380 0 0", "0 0 0")]),
    );
    let root = parent.entities.last_mut().unwrap();
    root.properties.set("file", "middle.vmf");
    root.properties.set("origin", "-16380 0 0");
    let owner = root.id;
    let context = source(directory.path());
    let error = prepare(
        &parent,
        Some(&context),
        &Control::default(),
        &mut Report::default(),
    )
    .err()
    .unwrap();
    assert!(
        error
            .message
            .contains("light origin exceeds finite Source compile bounds"),
        "{error:?}"
    );
    assert_eq!(error.object, Some(format!("entity:{owner}")));
    assert!(error.message.contains("child.vmf"));
}

#[test]
fn hidden_invalid_lights_are_ignored_and_editor_metadata_is_not_cloned() {
    let mut hidden = static_light(20, "light_dynamic", "NaN 0 96");
    hidden.hidden = true;
    let (directory, mut parent, _, _) = fixture();
    let mut valid = static_light(21, "light", "0 0 96");
    valid.editor = parent.entities[0].editor.clone();
    assert!(valid.editor.is_some());
    write_child(
        directory.path(),
        "child.vmf",
        &helper_file(vec![hidden, valid]),
    );
    parent
        .entities
        .last_mut()
        .unwrap()
        .properties
        .set("origin", "0 0 0");
    let context = source(directory.path());
    let prepared = prepare(
        &parent,
        Some(&context),
        &Control::default(),
        &mut Report::default(),
    )
    .unwrap();
    let lights = prepared
        .vmf()
        .entities
        .iter()
        .filter(|entity| entity.classname() == Some("light"))
        .collect::<Vec<_>>();
    assert_eq!(lights.len(), 1);
    assert!(lights[0].editor.is_none());
    assert_eq!(prepared.stats().unwrap().point_lights, 1);
    assert!(
        !prepared
            .vmf()
            .entities
            .iter()
            .any(|entity| entity.classname() == Some("light_dynamic"))
    );
}

#[test]
fn child_light_objects_and_repeated_property_work_are_bounded_before_cloning() {
    let (directory, mut parent, _, _) = emitter_fixture(vec![static_light(20, "light", "0 0 96")]);
    let id = parent.object_ids().max().unwrap() + 1;
    for offset in 0..MAX_OBJECTS - parent.object_ids().count() {
        parent
            .entities
            .push(Entity::new(id + offset as u32, "info_target"));
    }
    let context = source(directory.path());
    let error = prepare(
        &parent,
        Some(&context),
        &Control::default(),
        &mut Report::default(),
    )
    .err()
    .unwrap();
    assert!(error.message.contains("combined objects"), "{error:?}");

    let (directory, mut parent, _, _) = emitter_fixture(Vec::new());
    let mut light = static_light(20, "light", "0 0 96");
    light.properties.set("angles", "0 0 0");
    light.properties.set("defaultstyle", "0");
    for (key, value) in &mut light.properties.0 {
        if !matches!(key.as_str(), "classname" | "pattern") {
            value.push_str(&" ".repeat(MAX_FIELD_BYTES - value.len()));
        }
    }
    let lights = (0..128)
        .map(|index| {
            let mut light = light.clone();
            light.id = 20 + index;
            light
        })
        .collect();
    write_child(directory.path(), "child.vmf", &helper_file(lights));
    let mut file = "child.vmf".to_owned();
    for level in 1..4 {
        let name = format!("level{level}.vmf");
        write_child(
            directory.path(),
            &name,
            &helper_file(vec![nested_helper(500, &file, "0 0 0", "0 0 0")]),
        );
        file = name;
    }
    let root = parent.entities.last_mut().unwrap();
    root.properties.set("file", &file);
    root.properties.set("origin", "0 0 0");
    let first = root.clone();
    for index in 1..16 {
        let mut root = first.clone();
        root.id += index;
        parent.entities.push(root);
    }
    let context = source(directory.path());
    let error = prepare(
        &parent,
        Some(&context),
        &Control::default(),
        &mut Report::default(),
    )
    .err()
    .unwrap();
    assert!(error.message.contains("geometry-work"), "{error:?}");
}

#[test]
fn cancellation_after_a_real_child_light_rolls_back_the_detached_transaction() {
    let (directory, mut parent, _, _) = emitter_fixture(vec![static_light(20, "light", "0 0 96")]);
    let first = parent.entities.last().unwrap().clone();
    let mut second = first.clone();
    second.id += 1;
    parent.entities.push(second.clone());
    let before = parent.clone();
    let context = source(directory.path());
    let control = Control::default();
    let mut report = Report::default();
    let mut expansion = expand::Expansion::new(
        &parent,
        &context,
        &control,
        &mut report,
        PreparationPurpose::Reference,
    );
    let mut document = Document::new(parent);
    let error = document
        .edit("cancel generated static light", |tx| {
            expansion.helper(
                tx,
                &first,
                &first,
                &mut Vec::new(),
                &mut vec![context.root.clone()],
            )?;
            assert_eq!(
                tx.vmf()
                    .entities
                    .iter()
                    .filter(|entity| entity.classname() == Some("light"))
                    .count(),
                1
            );
            control.cancel().unwrap();
            expansion.helper(
                tx,
                &second,
                &second,
                &mut Vec::new(),
                &mut vec![context.root.clone()],
            )
        })
        .unwrap_err();
    assert!(error.to_string().contains("cancelled"));
    assert_eq!(document.vmf(), &before);
    assert_eq!(expansion.finish().stats.point_lights, 1);
}

#[test]
fn combined_root_and_child_lights_keep_the_existing_lit_world_light_limit() {
    let lights = (0..source_formats::bsp::MAX_WORLD_LIGHTS / 2)
        .map(|index| static_light(20 + index as u32, "light", "0 0 96"))
        .collect();
    let (directory, mut parent, _, game) = emitter_fixture(lights);
    let root = parent.entities.last().unwrap().clone();
    let mut sibling = root.clone();
    sibling.id += 1;
    parent.entities.push(sibling);
    parent
        .entities
        .push(static_light(root.id + 2, "light", "0 0 128"));
    let context = source(directory.path());
    let output = compile_with_source_controlled(
        &parent,
        &game,
        Options {
            lighting: crate::LightingMode::DirectLdrHdr,
            ..Options::default()
        },
        None,
        Some(&context),
        &Control::default(),
    );
    assert!(output.bsp.is_none());
    assert!(
        output
            .report
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.message.contains("8,192 world-light limit")),
        "{:?}",
        output.report
    );
    assert_eq!(
        output.stats.instances.as_ref().unwrap().point_lights,
        source_formats::bsp::MAX_WORLD_LIGHTS
    );
}

#[test]
fn nested_child_light_leak_seed_and_solid_warning_select_the_editable_root() {
    for solid in [false, true] {
        let (directory, mut parent, _, game) = emitter_fixture(vec![static_light(
            20,
            "light",
            if solid { "0 0 -8" } else { "0 0 96" },
        )]);
        write_child(
            directory.path(),
            "middle.vmf",
            &helper_file(vec![nested_helper(100, "child.vmf", "16 -8 0", "0 0 0")]),
        );
        let root = parent.entities.last_mut().unwrap();
        root.properties.set("file", "middle.vmf");
        root.properties.set("origin", "0 0 0");
        let owner = root.id;
        if !solid {
            parent.world.solids.remove(0);
            parent.entities.retain(|entity| entity.id == owner);
        }
        let context = source(directory.path());
        let prepared = prepare(
            &parent,
            Some(&context),
            &Control::default(),
            &mut Report::default(),
        )
        .unwrap();
        let generated = prepared
            .vmf()
            .entities
            .iter()
            .find(|entity| entity.classname() == Some("light"))
            .unwrap();
        assert_eq!(prepared.root_entity(generated.id), Some(owner));
        assert_eq!(prepared.root_entity(owner), None);
        let output = compiled(&parent, &game, &context);
        let diagnostic = output
            .report
            .diagnostics
            .iter()
            .find(|diagnostic| {
                diagnostic.message.contains(if solid {
                    "inside solid space"
                } else {
                    "map leaks"
                })
            })
            .unwrap();
        assert_eq!(diagnostic.object, Some(format!("entity:{owner}")));
        assert!(
            diagnostic
                .message
                .contains("instance child.vmf, child object 20"),
            "{diagnostic:?}"
        );
        assert_eq!(diagnostic.position, generated.origin());
        if solid {
            assert!(output.bsp.is_some(), "{:?}", output.report);
        } else {
            let leak = output.leak.unwrap();
            assert_eq!(leak.entity, owner);
            assert_eq!(leak.points.first().copied(), generated.origin());
            assert!(leak.points.len() > 1);
        }
    }
}

#[test]
fn legacy_flat_fullbright_light_behavior_and_zero_stat_shape_are_preserved() {
    let mut parent = test_common::room();
    let mut light = static_light(
        parent.object_ids().max().unwrap() + 1,
        "light_spot",
        "0 0 96",
    );
    light.properties.remove("pitch");
    light
        .properties
        .set("_light", "malformed legacy root input");
    parent.entities.push(light);
    let prepared = prepare(&parent, None, &Control::default(), &mut Report::default()).unwrap();
    assert!(matches!(&prepared.vmf, Cow::Borrowed(_)));
    assert!(prepared.stats().is_none());
    let output = crate::compile(&parent, &test_common::empty_game());
    assert!(output.bsp.is_some(), "{:?}", output.report);
    assert!(output.stats.instances.is_none());
    let stats = serde_json::to_value(InstanceStats::default()).unwrap();
    assert!(stats.get("point_lights").is_none());
    assert!(stats.get("spot_lights").is_none());
}

fn parameter_light(id: u32, class: &str, origin: &str) -> Entity {
    let mut light = static_light(id, class, origin);
    light.properties.set("_light", "$lamp");
    light.properties.set("_lightHDR", "$hdr");
    light
}

fn light_bindings(helper: &mut Entity, ldr: &str, hdr: &str) {
    helper.properties.set("replace01", format!("$LaMp {ldr}"));
    helper.properties.set("replace10", format!("$HDR {hdr}"));
}

#[test]
fn sibling_light_bindings_match_independent_literal_dual_bsp_and_preserve_raw_sources() {
    let mut point = parameter_light(20, "light", "-48 16 128");
    point.properties.set("style", "1");
    point.properties.set("_lightscaleHDR", "$gain");
    point.properties.set("_fifty_percent_distance", "$half");
    point.properties.set("_zero_percent_distance", "$end");
    let mut spot = parameter_light(21, "light_spot", "64 -16 160");
    spot.properties.set("style", "11");
    spot.properties.set("_lightscaleHDR", "$gain");
    spot.properties.set("_fifty_percent_distance", "$half");
    spot.properties.set("_zero_percent_distance", "$end");
    let (directory, mut parent, template, game) = emitter_fixture(vec![point, spot]);
    let colors = [
        ("255 96 32 300", "32 96 255 450"),
        ("64 255 160 240", "256 80 40 600"),
    ];
    let gains = ["2", "0.5"];
    let distances = [("64", "160"), ("96", "0")];
    light_bindings(
        parent.entities.last_mut().unwrap(),
        colors[0].0,
        colors[0].1,
    );
    parent
        .entities
        .last_mut()
        .unwrap()
        .properties
        .set("replace09", format!("$GAIN {}", gains[0]));
    let id = parent.object_ids().max().unwrap() + 1;
    let mut sibling = nested_helper(id, "child.vmf", "-24 12 0", "0 0 0");
    light_bindings(&mut sibling, colors[1].0, colors[1].1);
    sibling
        .properties
        .set("replace09", format!("$gain {}", gains[1]));
    parent.entities.push(sibling);
    for (helper, (half, end)) in parent
        .entities
        .iter_mut()
        .filter(|entity| {
            entity
                .classname()
                .is_some_and(|class| class.eq_ignore_ascii_case("func_instance"))
        })
        .zip(distances)
    {
        helper.properties.set("replace07", format!("$half {half}"));
        helper.properties.set("replace08", format!("$end {end}"));
    }
    write_child(directory.path(), "parent.vmf", &parent);
    let raw = std::fs::read(directory.path().join("child.vmf")).unwrap();
    let context = source(directory.path());
    let mut literal = Document::new(parent.clone());
    literal
        .edit("independent sibling colors", |tx| {
            for (((helper, (ldr, hdr)), gain), (half, end)) in parent
                .entities
                .iter()
                .filter(|entity| {
                    entity
                        .classname()
                        .is_some_and(|class| class.eq_ignore_ascii_case("func_instance"))
                })
                .zip(colors)
                .zip(gains)
                .zip(distances)
            {
                for original in &template.entities {
                    let mut light =
                        static_light(original.id, original.classname().unwrap(), "0 0 0");
                    light.id = tx.new_object_id()?;
                    light.properties.set(
                        "origin",
                        format_vec3(original.origin().unwrap() + helper.origin().unwrap()),
                    );
                    light.properties.set("_light", ldr);
                    light.properties.set("_lightHDR", hdr);
                    light.properties.set("_lightscaleHDR", gain);
                    light.properties.set("_fifty_percent_distance", half);
                    light.properties.set("_zero_percent_distance", end);
                    light
                        .properties
                        .set("style", original.properties.get("style").unwrap());
                    tx.add_entity(light)?;
                }
            }
            for helper in parent.entities.iter().filter(|entity| {
                entity
                    .classname()
                    .is_some_and(|class| class.eq_ignore_ascii_case("func_instance"))
            }) {
                tx.remove(ObjectId::Entity(helper.id))?;
            }
            Ok(())
        })
        .unwrap();
    let expected = emitter_output(
        literal.vmf(),
        &game,
        None,
        crate::LightingMode::DirectLdrHdr,
    );
    let actual = emitter_output(
        &parent,
        &game,
        Some(&context),
        crate::LightingMode::DirectLdrHdr,
    );
    let stats = actual.stats.instances.as_ref().unwrap();
    assert_eq!(
        (
            stats.instances,
            stats.files,
            stats.point_lights,
            stats.spot_lights
        ),
        (2, 1, 2, 2)
    );
    assert_eq!(stats.dependencies[0].bytes, raw.len());
    assert_eq!(
        actual.bsp.as_ref().unwrap().to_bytes(),
        expected.bsp.unwrap().to_bytes()
    );
    for channel in [LightingKind::Ldr, LightingKind::Hdr] {
        let lights = actual.bsp.as_ref().unwrap().world_lights(channel).unwrap();
        assert_eq!(
            lights.iter().map(|light| light.style).collect::<Vec<_>>(),
            [1, 11, 1, 11]
        );
        assert_ne!(lights[0].intensity, lights[2].intensity);
    }
    let contributors = &actual
        .stats
        .lighting_progress
        .as_ref()
        .unwrap()
        .contributors;
    assert!(contributors.iter().any(|light| {
        light
            .instance
            .as_ref()
            .is_some_and(|source| source.original_entity_id == 20)
    }));
    assert_eq!(
        std::fs::read(directory.path().join("child.vmf")).unwrap(),
        raw
    );
    actual
        .instance_dependencies
        .as_ref()
        .unwrap()
        .verify(&Control::default())
        .unwrap();
    let destination = directory.path().join("bound.bsp");
    std::fs::write(&destination, b"previous BSP").unwrap();
    let staged = actual
        .prepare_write_controlled(&destination, &Control::default())
        .unwrap()
        .unwrap();
    let changed = String::from_utf8(raw.clone())
        .unwrap()
        .replace("$lamp", "$other");
    std::fs::write(directory.path().join("child.vmf"), changed).unwrap();
    assert!(staged.commit(&Control::default()).is_err());
    assert_eq!(std::fs::read(&destination).unwrap(), b"previous BSP");
    std::fs::write(directory.path().join("child.vmf"), &raw).unwrap();
    let original_parent = std::fs::read(directory.path().join("parent.vmf")).unwrap();
    let context = source(directory.path());
    let output = emitter_output(
        &parent,
        &game,
        Some(&context),
        crate::LightingMode::Fullbright,
    );
    let mut changed = parent.clone();
    changed
        .entities
        .last_mut()
        .unwrap()
        .properties
        .set("replace01", "$lamp 1 2 3 4");
    write_child(directory.path(), "parent.vmf", &changed);
    assert!(
        output
            .instance_dependencies
            .as_ref()
            .unwrap()
            .verify(&Control::default())
            .is_err()
    );
    std::fs::write(directory.path().join("parent.vmf"), original_parent).unwrap();
}

#[test]
fn nested_light_bindings_are_local_even_when_names_repeat() {
    let (directory, mut parent, _, _) =
        emitter_fixture(vec![parameter_light(20, "light_spot", "0 0 96")]);
    let mut nested = nested_helper(100, "child.vmf", "16 0 0", "0 0 0");
    light_bindings(&mut nested, "64 255 32 200", "64 32 512 600");
    write_child(
        directory.path(),
        "middle.vmf",
        &helper_file(vec![parameter_light(200, "light", "-32 0 96"), nested]),
    );
    let root = parent.entities.last_mut().unwrap();
    root.properties.set("file", "middle.vmf");
    light_bindings(root, "255 32 64 300", "32 255 64 450");
    let context = source(directory.path());
    let resolved = resolve(
        &parent,
        Some(&context),
        &Control::default(),
        &mut Report::default(),
    )
    .unwrap();
    let lights = resolved
        .vmf()
        .entities
        .iter()
        .filter(|entity| matches!(entity.classname(), Some("light" | "light_spot")))
        .collect::<Vec<_>>();
    assert_eq!(lights.len(), 2);
    assert_eq!(lights[0].properties.get("_light"), Some("255 32 64 300"));
    assert_eq!(lights[1].properties.get("_light"), Some("64 255 32 200"));
    assert_eq!(lights[1].properties.get("_lightHDR"), Some("64 32 512 600"));
    assert_eq!(resolved.stats().unwrap().max_expansion_depth, 2);
    for light in lights {
        entities::admit(light, entities::kind(light).unwrap()).unwrap();
    }
    parent
        .entities
        .last_mut()
        .unwrap()
        .properties
        .remove("replace01");
    assert!(
        resolve(
            &parent,
            Some(&context),
            &Control::default(),
            &mut Report::default()
        )
        .err()
        .unwrap()
        .message
        .contains("missing immediate binding")
    );
}

#[test]
fn bound_templates_reject_grouped_semantic_and_context_failures_before_fullbright() {
    let (directory, baseline, template, game) =
        emitter_fixture(vec![parameter_light(20, "light_spot", "0 0 96")]);
    let context = source(directory.path());
    for issue in [
        "missing",
        "unused",
        "duplicate",
        "recursive",
        "noncanonical",
        "embedded",
        "hdr scale",
        "missing distance",
        "unused distance",
        "negative distance",
        "nonfinite distance",
        "distance ordering",
        "distance outside region",
        "distance native overflow",
        "coefficient token",
        "bad ldr",
        "bad hdr",
        "bad hdr outside region",
        "cone",
        "hidden",
        "world context",
        "material context",
        "forwarding",
    ] {
        let mut parent = baseline.clone();
        let mut child = template.clone();
        if matches!(issue, "bad hdr outside region" | "distance outside region") {
            parent.cordon = Some(Node::block(
                "cordon",
                vec![
                    Node::string("active", "1"),
                    Node::string("mins", "-128 -128 0"),
                    Node::string("maxs", "128 128 256"),
                ],
            ));
            child.entities[0].properties.set("origin", "1024 0 96");
        }
        let helper = parent.entities.last_mut().unwrap();
        light_bindings(helper, "255 96 32 300", "32 96 255 450");
        match issue {
            "missing" => {
                helper.properties.remove("replace01");
            }
            "unused" => helper.properties.set("replace02", "$unused 1 2 3 4"),
            "duplicate" => helper.properties.set("replace02", "$LAMP 1 2 3 4"),
            "recursive" => helper.properties.set("replace01", "$lamp $hdr"),
            "noncanonical" => {
                helper.properties.remove("replace01");
                helper.properties.set("Replace01", "$lamp 255 96 32 300");
            }
            "embedded" => child.entities[0]
                .properties
                .set("_light", "255 $lamp 32 300"),
            "hdr scale" => child.entities[0].properties.set("_lightscaleHDR", "$lamp"),
            "missing distance" => child.entities[0]
                .properties
                .set("_fifty_percent_distance", "$half"),
            "unused distance" => helper.properties.set("replace07", "$half 64"),
            "negative distance"
            | "nonfinite distance"
            | "distance ordering"
            | "distance outside region"
            | "distance native overflow" => {
                child.entities[0]
                    .properties
                    .set("_fifty_percent_distance", "$half");
                child.entities[0]
                    .properties
                    .set("_zero_percent_distance", "$end");
                helper.properties.set(
                    "replace07",
                    match issue {
                        "negative distance" => "$half -1",
                        "nonfinite distance" => "$half NaN",
                        "distance native overflow" => "$half 1e40",
                        _ => "$half 64",
                    },
                );
                helper.properties.set("replace08", "$end 32");
            }
            "coefficient token" => child.entities[0].properties.set("_quadratic_attn", "$lamp"),
            "bad ldr" => helper.properties.set("replace01", "$lamp 300 96 32 300"),
            "bad hdr" | "bad hdr outside region" => {
                helper.properties.set("replace10", "$hdr 1 2 3 -1")
            }
            "cone" => child.entities[0].properties.set("_cone", "15"),
            "hidden" => child.entities[0].hidden = true,
            "world context" => child.world.properties.set("skyname", "$lamp"),
            "material context" => {
                child
                    .world
                    .solids
                    .push(test_common::room().world.solids.remove(0));
                child.world.solids[0].sides[0].material = "dev/$lamp".into();
            }
            "forwarding" => {
                let mut nested = nested_helper(100, "child.vmf", "0 0 0", "0 0 0");
                nested.properties.set("replace01", "$local $lamp");
                child.entities.push(nested);
            }
            _ => unreachable!(),
        }
        let owner = helper.id;
        write_child(directory.path(), "child.vmf", &child);
        let raw = std::fs::read(directory.path().join("child.vmf")).unwrap();
        let output = compile_with_source_controlled(
            &parent,
            &game,
            Options {
                lighting: crate::LightingMode::Fullbright,
                ..Options::default()
            },
            None,
            Some(&context),
            &Control::default(),
        );
        assert!(
            output.report.has_errors() && output.bsp.is_none(),
            "{issue}: {:?}",
            output.report
        );
        assert!(
            output
                .report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.object == Some(format!("entity:{owner}"))),
            "{issue}: {:?}",
            output.report
        );
        assert_eq!(
            std::fs::read(directory.path().join("child.vmf")).unwrap(),
            raw
        );
    }
}

#[test]
fn unresolved_standalone_light_templates_fail_in_every_compile_mode() {
    for (class, key) in [
        ("light", "_light"),
        ("light_spot", "_lightHDR"),
        ("light_environment", "_light"),
        ("light_environment", "_lightHDR"),
        ("light_environment", "_ambient"),
        ("light_environment", "_ambientHDR"),
        ("light", "_lightscaleHDR"),
        ("light_spot", "_lightscaleHDR"),
        ("light_environment", "_lightscaleHDR"),
        ("light_environment", "_AmbientScaleHDR"),
        ("light_environment", "SunSpreadAngle"),
        ("light", "_fifty_percent_distance"),
        ("light_spot", "_zero_percent_distance"),
    ] {
        let mut vmf = test_common::room();
        let mut light = static_light(vmf.object_ids().max().unwrap() + 1, class, "0 0 96");
        light.properties.set(key, "$lamp");
        let id = light.id;
        vmf.entities.push(light);
        for value in ["$lamp", "$lamp suffix", "255 $lamp 1 2", "$bad-name"] {
            vmf.entities.last_mut().unwrap().properties.set(key, value);
            for lighting in [
                crate::LightingMode::Fullbright,
                crate::LightingMode::DirectLdr,
                crate::LightingMode::DirectLdrHdr,
            ] {
                let output = compile_with_source_controlled(
                    &vmf,
                    &test_common::empty_game(),
                    Options {
                        lighting,
                        ..Options::default()
                    },
                    None,
                    None,
                    &Control::default(),
                );
                assert!(output.bsp.is_none());
                assert!(
                    output
                        .report
                        .diagnostics
                        .iter()
                        .any(
                            |diagnostic| diagnostic.object == Some(format!("entity:{id}"))
                                && diagnostic
                                    .message
                                    .contains("Unresolved light-value parameter")
                        ),
                    "{class}, {key}, {lighting:?}: {:?}",
                    output.report
                );
            }
        }
    }
}
