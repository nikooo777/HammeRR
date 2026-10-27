use super::*;
use compiler::cubemap_assets::MANIFEST_PATH;
use compiler::{CompileContext, CubemapAssetManifest, CubemapAssetPhase, LightingMode, Options};
use glam::{DMat3, DQuat, DVec3, EulerRot};
use map_core::edit;
use source_formats::bsp::{Cubemap, PakLimits, lump};
use source_formats::vmf::{Properties, Vmf};

fn fixture(dx_levels: [&str; 2]) -> (tempfile::TempDir, Session, PathBuf, PathBuf, PathBuf) {
    let (directory, game, _) = super::super::baked_prop_vertices::test_support::fixture();
    std::fs::copy(
        directory.path().join("materials/models/test/one.vmt"),
        directory.path().join("materials/models/test/two.vmt"),
    )
    .unwrap();
    let mut physics = [16_i32, 0, 1, 7].map(i32::to_le_bytes).concat();
    physics.extend_from_slice(&32_i32.to_le_bytes());
    physics.extend_from_slice(b"VPHY");
    physics.extend_from_slice(&0x100_u16.to_le_bytes());
    physics.extend_from_slice(&0_u16.to_le_bytes());
    physics.extend_from_slice(&4_u32.to_le_bytes());
    physics.extend_from_slice(&[0; 20]);
    physics.extend_from_slice(b"solid {}\0");
    std::fs::write(directory.path().join("models/test/receiver.phy"), physics).unwrap();
    std::fs::write(
        directory.path().join("materials/test/reflective.vmt"),
        "LightmappedGeneric { $basetexture test/checker $envmap env_cubemap }",
    )
    .unwrap();
    std::fs::create_dir_all(directory.path().join("materials/skybox")).unwrap();
    for face in ["rt", "lf", "bk", "ft", "up", "dn"] {
        std::fs::write(
            directory
                .path()
                .join(format!("materials/skybox/sky_day01_01{face}.vmt")),
            "UnlitGeneric { }",
        )
        .unwrap();
    }
    let mut leaf = Document::empty();
    leaf.edit("reflective child", |tx| {
        edit::create_block(
            tx,
            DVec3::new(-40., -24., 16.),
            DVec3::new(-8., 24., 48.),
            "test/reflective",
        )?;
        edit::create_block(
            tx,
            DVec3::new(8., -16., 16.),
            DVec3::new(40., 16., 32.),
            "test/reflective",
        )
    })
    .unwrap();
    let mut leaf = leaf.into_vmf();
    let mut detail = Entity::new(100, "func_detail");
    detail.solids.push(leaf.world.solids.pop().unwrap());
    let sides = format!(
        "+{} 0{}",
        leaf.world.solids[0].sides[0].id, detail.solids[0].sides[0].id
    );
    let editor = Properties(vec![
        ("color".into(), "50 110 220".into()),
        ("logicalpos".into(), "[5 8]".into()),
    ]);
    let mut far = Entity::new(200, "env_cubemap");
    far.properties.set("origin", "0 0 -96");
    far.properties.set("cubemapsize", "1");
    far.properties.set("sides", sides);
    far.editor = Some(editor.clone());
    let mut near = Entity::new(201, "ENV_CUBEMAP");
    near.properties.set("origin", "0 0 64");
    near.properties.set("cubemapsize", "1");
    near.editor = Some(editor);
    let mut light = Entity::new(202, "light");
    light.properties.set("origin", "0 0 80");
    light.properties.set("_light", "255 128 64 200");
    light.properties.set("_lightHDR", "64 128 255 300");
    leaf.entities = vec![far, detail, light, near];
    for (id, origin, skin, solid) in [(203, "80 8 32", "1", "2"), (204, "-80 8 32", "0", "6")] {
        let mut prop = Entity::new(id, "prop_static");
        for (key, value) in [
            ("origin", origin),
            ("angles", "0 0 0"),
            ("model", "models/test/receiver.mdl"),
            ("skin", skin),
            ("solid", solid),
            ("lightingorigin", ""),
            ("ignorenormals", "1"),
            ("disableselfshadowing", "1"),
            ("fademindist", "128"),
            ("fademaxdist", "1024"),
            ("fadescale", "0.5"),
            ("mindxlevel", dx_levels[0]),
            ("maxdxlevel", dx_levels[1]),
        ] {
            prop.properties.set(key, value);
        }
        prop.editor = leaf.entities[0].editor.clone();
        leaf.entities.push(prop);
    }
    let child = directory.path().join("leaf.vmf");
    std::fs::write(&child, leaf.write(&Default::default())).unwrap();
    let mut middle = Document::empty().into_vmf();
    for (id, origin) in [(10, "-64 0 0"), (11, "64 0 0")] {
        let mut helper = Entity::new(id, "func_instance");
        helper.properties.set("file", "leaf.vmf");
        helper.properties.set("origin", origin);
        helper.properties.set("angles", "90 0 0");
        middle.entities.push(helper);
    }
    let middle_path = directory.path().join("middle.vmf");
    std::fs::write(&middle_path, middle.write(&Default::default())).unwrap();
    let mut parent = crate::test_assets::room();
    for side in parent
        .world
        .solids
        .iter_mut()
        .flat_map(|solid| &mut solid.sides)
    {
        side.material = "test/checker".into();
        side.lightmap_scale = 128;
    }
    let mut root = Entity::new(500, "func_instance");
    root.properties.set("file", "middle.vmf");
    root.properties.set("origin", "0 0 128");
    root.properties.set("angles", "0 90 0");
    parent.entities.push(root);
    let parent_path = directory.path().join("parent.vmf");
    std::fs::write(&parent_path, parent.write(&Default::default())).unwrap();
    let mut session = Session::new(Ok(game));
    session.open(Actor::User, &parent_path, true).unwrap();
    session
        .edit(Actor::User, "reserve authored IDs", |tx| {
            for _ in 0..5 {
                tx.new_object_id()?;
                tx.new_side_id()?;
            }
            Ok(())
        })
        .unwrap();
    (directory, session, parent_path, child, middle_path)
}

fn preview(session: &mut Session) -> Value {
    let request = session
        .begin_instance_collapse(Actor::User, &session.instance_collapse_stamp())
        .unwrap();
    session
        .finish_instance_collapse(Actor::User, request.run())
        .unwrap()
}

fn id_map(rows: &Value) -> BTreeMap<u32, u32> {
    rows.as_array()
        .unwrap()
        .iter()
        .map(|row| {
            let ObjectId::Entity(expanded) =
                crate::identity::parse_id(row["expanded"].as_str().unwrap()).unwrap()
            else {
                panic!("expanded entity ID");
            };
            let ObjectId::Entity(authored) =
                crate::identity::parse_id(row["authored"].as_str().unwrap()).unwrap()
            else {
                panic!("authored entity ID");
            };
            (expanded, authored)
        })
        .collect()
}

fn remap_manifest(
    mut manifest: CubemapAssetManifest,
    helpers: &BTreeMap<u32, u32>,
    sides: &BTreeMap<u32, u32>,
) -> CubemapAssetManifest {
    for sample in manifest.samples.iter_mut().chain(&mut manifest.hdr_samples) {
        sample.helper_id = helpers[&sample.helper_id];
    }
    for assignment in &mut manifest.assignments {
        assignment.face_id = sides[&assignment.face_id];
        assignment.helper_id = helpers[&assignment.helper_id];
    }
    manifest
}

#[test]
fn collapsed_props_and_cubemaps_preserve_native_lighting_collision_and_separate_source_maps() {
    for (bake_static_prop_vertices, dx_levels) in [(false, ["80", "95"]), (true, ["0", "0"])] {
        let (_directory, mut session, parent, child, middle) = fixture(dx_levels);
        let source = session.document.vmf().clone();
        let history = session.document.undo_labels().count();
        let disk = [&parent, &child, &middle].map(|path| std::fs::read(path).unwrap());
        let ready = preview(&mut session);
        assert_eq!(ready["stats"]["cubemaps"], 4);
        let helpers = id_map(&ready["helper_id_map"]);
        let emitted = id_map(&ready["entity_id_map"]);
        let props = id_map(&ready["static_prop_id_map"]);
        assert_eq!((helpers.len(), emitted.len()), (4, 2));
        assert_eq!(props.len(), 4);
        assert!(
            props
                .iter()
                .any(|(expanded, authored)| expanded != authored)
        );
        assert_eq!(ready["stats"]["static_props"], 4);
        assert!(helpers.keys().all(|id| !emitted.contains_key(id)));
        assert!(
            props
                .keys()
                .all(|id| !emitted.contains_key(id) && !helpers.contains_key(id))
        );
        assert_eq!(
            emitted
                .values()
                .chain(helpers.values())
                .chain(props.values())
                .collect::<BTreeSet<_>>()
                .len(),
            emitted.len() + helpers.len() + props.len()
        );
        assert_eq!(
            ready["entity_provenance"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|row| row["classname"]
                    .as_str()
                    .is_some_and(|class| class.eq_ignore_ascii_case("env_cubemap"))
                    && row["emitted"] == false)
                .count(),
            4
        );
        let shown = session.render_document().vmf().clone();
        let prop_origins = ready["entity_provenance"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["classname"] == "prop_static")
            .collect::<Vec<_>>();
        assert_eq!(prop_origins.len(), 4);
        let rotation = DMat3::from_cols(-DVec3::Z, -DVec3::X, DVec3::Y);
        for (row, expected) in prop_origins.iter().zip([
            [-8., -32., 48.],
            [-8., -32., 208.],
            [-8., 96., 48.],
            [-8., 96., 208.],
        ]) {
            let ObjectId::Entity(id) =
                crate::identity::parse_id(row["entity"].as_str().unwrap()).unwrap()
            else {
                panic!("authored prop ID");
            };
            let entity = shown
                .entities
                .iter()
                .find(|entity| entity.id == id)
                .unwrap();
            assert!(
                entity
                    .origin()
                    .unwrap()
                    .abs_diff_eq(DVec3::from_array(expected), 1e-8)
            );
            let angles = source_formats::vmf::parse_vec3(entity.properties.get("angles").unwrap())
                .unwrap()
                .map(f64::to_radians);
            let actual = DMat3::from_quat(DQuat::from_euler(
                EulerRot::ZYX,
                angles.y,
                angles.x,
                angles.z,
            ));
            assert!(actual.abs_diff_eq(rotation, 1e-8));
            assert_eq!(
                entity.editor.as_ref().unwrap().get("color"),
                Some("50 110 220")
            );
            assert_eq!(row["emitted"], false);
            assert_eq!(row["compiler_handled"], true);
            assert_eq!(
                row["occurrence"],
                json!([
                    {"helper":500,"file":"middle.vmf"},
                    {"helper":if expected[1] < 0. {10} else {11},"file":"leaf.vmf"}
                ])
            );
            assert_eq!(
                row["original_entity"],
                if expected[2] == 48. { 203 } else { 204 }
            );
        }
        let control = Control::default();
        let context = instances::SourceContext::capture(&parent, &control).unwrap();
        let prepared = instances::resolve_for_collapse(
            &source,
            Some(&context),
            &control,
            &mut compiler::Report::default(),
        )
        .unwrap();
        let mut sides = BTreeMap::new();
        assert_eq!(prepared.vmf().solids().count(), shown.solids().count());
        for (expanded, authored) in prepared.vmf().solids().zip(shown.solids()) {
            assert_eq!(expanded.sides.len(), authored.sides.len());
            for (before, after) in expanded.sides.iter().zip(&authored.sides) {
                assert_eq!(before.plane, after.plane);
                assert_eq!(before.material, after.material);
                assert!(sides.insert(before.id, after.id).is_none());
            }
        }
        assert!(
            sides
                .values()
                .zip(sides.values().skip(1))
                .all(|(before, after)| before < after)
        );
        let mut manual = BTreeSet::new();
        for before in prepared.vmf().entities.iter().filter(|entity| {
            entity
                .classname()
                .is_some_and(|class| class.eq_ignore_ascii_case("env_cubemap"))
        }) {
            let after = shown
                .entities
                .iter()
                .find(|entity| entity.id == helpers[&before.id])
                .unwrap();
            assert_eq!(before.origin(), after.origin());
            assert_eq!(before.editor, after.editor);
            if let Some(text) = before.properties.get("sides") {
                let expected = text
                    .split_whitespace()
                    .map(|value| sides[&value.parse::<u32>().unwrap()])
                    .collect::<Vec<_>>();
                let actual = after
                    .properties
                    .get("sides")
                    .unwrap()
                    .split_whitespace()
                    .map(|value| value.parse::<u32>().unwrap())
                    .collect::<Vec<_>>();
                assert_eq!(actual, expected);
                manual.extend(actual.into_iter().map(|side| (side, after.id)));
                assert_ne!(
                    before.properties.get("sides"),
                    after.properties.get("sides")
                );
            } else {
                assert_eq!(after.properties.get("sides"), None);
            }
        }
        assert_eq!(manual.len(), 4);
        let options = Options {
            lighting: LightingMode::DirectLdrHdr,
            generate_reflections: true,
            bake_static_prop_vertices,
            ..Default::default()
        };
        let output_context = CompileContext::new("collapsed_cubes").unwrap();
        let expanded = compiler::compile_with_source_controlled(
            &source,
            &session.game,
            options,
            Some(&output_context),
            Some(&context),
            &control,
        );
        let authored = compiler::compile_with_context_controlled(
            &shown,
            &session.game,
            options,
            Some(&output_context),
            &control,
        );
        assert!(!expanded.report.has_errors(), "{:?}", expanded.report);
        assert!(!authored.report.has_errors(), "{:?}", authored.report);
        if bake_static_prop_vertices {
            assert_eq!(
                expanded.stats.prop_vertex_lighting.as_ref().unwrap().props,
                4
            );
            assert!(
                expanded
                    .stats
                    .prop_vertex_lighting
                    .as_ref()
                    .unwrap()
                    .hdr_bytes
                    > 0
            );
        } else {
            assert!(expanded.stats.prop_vertex_lighting.is_none());
        }
        let expanded_bsp = expanded.bsp.as_ref().unwrap();
        let authored_bsp = authored.bsp.as_ref().unwrap();
        let native = authored_bsp.static_props().unwrap().unwrap();
        assert_eq!(
            native
                .props
                .iter()
                .map(|prop| (prop.skin, prop.solid))
                .collect::<Vec<_>>(),
            [(1, 2), (0, 6), (1, 2), (0, 6)]
        );
        assert!(native.props.iter().all(|prop| prop.fade_min == 128.
            && prop.fade_max == 1024.
            && prop.fade_scale == 0.5
            && prop.dx_levels
                == if bake_static_prop_vertices {
                    [0, 0]
                } else {
                    [80, 95]
                }
            && prop.flags & (8 | 128) == (8 | 128)));
        let before = expanded
            .static_prop_origins
            .as_ref()
            .unwrap()
            .associations(expanded_bsp, 4)
            .unwrap();
        let after = authored
            .static_prop_origins
            .as_ref()
            .unwrap()
            .associations(authored_bsp, 4)
            .unwrap();
        for (before, after) in before.iter().zip(after) {
            let compiler::entity_origins::Origin::Instance {
                instance,
                file,
                entity,
                expanded_entity,
                occurrence,
            } = before
            else {
                panic!("instance prop source");
            };
            assert_eq!(*instance, 500);
            assert_eq!(file, "leaf.vmf");
            assert!(matches!(entity, 203 | 204));
            assert_eq!(
                serde_json::to_value(occurrence.as_ref()).unwrap(),
                prop_origins
                    .iter()
                    .find(|row| row["expanded"]
                        == crate::identity::id_string(ObjectId::Entity(*expanded_entity)))
                    .unwrap()["occurrence"]
            );
            assert_eq!(
                *after,
                compiler::entity_origins::Origin::Root {
                    entity: props[expanded_entity]
                }
            );
        }
        let original_manifest = expanded.cubemap_assets.as_ref().unwrap();
        let manifest = authored.cubemap_assets.as_ref().unwrap();
        assert_eq!(manifest.phase, CubemapAssetPhase::Pending);
        assert_eq!((manifest.samples.len(), manifest.hdr_samples.len()), (4, 4));
        assert!(
            !manifest.aliases.is_empty()
                && !manifest.texinfo_bindings.is_empty()
                && !manifest.face_bindings.is_empty()
        );
        assert_eq!(
            remap_manifest(original_manifest.clone(), &helpers, &sides),
            *manifest
        );
        assert_eq!(
            manifest
                .assignments
                .iter()
                .filter(|assignment| assignment.manual)
                .map(|assignment| (assignment.face_id, assignment.helper_id))
                .collect::<BTreeSet<_>>(),
            manual
        );
        let mut automatic = shown.clone();
        for entity in &mut automatic.entities {
            if entity
                .classname()
                .is_some_and(|class| class.eq_ignore_ascii_case("env_cubemap"))
            {
                entity.properties.remove("sides");
            }
        }
        let automatic = compiler::compile_with_context_controlled(
            &automatic,
            &session.game,
            options,
            Some(&output_context),
            &control,
        );
        assert!(!automatic.report.has_errors(), "{:?}", automatic.report);
        for (side, helper) in &manual {
            assert_ne!(
                automatic
                    .cubemap_assets
                    .as_ref()
                    .unwrap()
                    .assignments
                    .iter()
                    .find(|assignment| assignment.face_id == *side)
                    .unwrap()
                    .helper_id,
                *helper
            );
        }
        let expanded = expanded.bsp.unwrap();
        let authored = authored.bsp.unwrap();
        assert_eq!(
            authored
                .records::<Cubemap>()
                .unwrap()
                .iter()
                .map(|sample| sample.origin)
                .collect::<Vec<_>>(),
            [[0, -160, 128], [0, 0, 128], [0, -32, 128], [0, 128, 128]]
        );
        assert_eq!(
            (expanded.version, expanded.map_revision),
            (authored.version, authored.map_revision)
        );
        assert_eq!(expanded.game_lumps, authored.game_lumps);
        for (index, expected) in authored.lumps.iter().enumerate() {
            if index != lump::PAKFILE && index != lump::ENTITIES {
                assert_eq!(&expanded.lumps[index], expected, "lump {index}");
            }
        }
        let replacements = emitted
            .iter()
            .map(|(before, after)| {
                (
                    format!("\"hammerid\" \"{before}\"\n"),
                    format!("\"hammerid\" \"{after}\"\n"),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let entity_text = expanded
            .entities()
            .unwrap()
            .split_inclusive('\n')
            .map(|line| replacements.get(line).map_or(line, String::as_str))
            .collect::<String>();
        assert_eq!(entity_text, authored.entities().unwrap());
        assert!(!entity_text.to_ascii_lowercase().contains("env_cubemap"));
        assert!(!entity_text.contains("prop_static"));
        let source_pak = expanded.pak_data(PakLimits::default()).unwrap();
        let authored_pak = authored.pak_data(PakLimits::default()).unwrap();
        let actual = source_pak
            .entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry.data.as_slice()))
            .collect::<BTreeMap<_, _>>();
        let expected = authored_pak
            .entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry.data.as_slice()))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(actual.len(), source_pak.entries.len());
        assert_eq!(expected.len(), authored_pak.entries.len());
        assert_eq!(
            actual.keys().collect::<Vec<_>>(),
            expected.keys().collect::<Vec<_>>()
        );
        for (name, data) in actual {
            if name == MANIFEST_PATH {
                assert_eq!(
                    remap_manifest(serde_json::from_slice(data).unwrap(), &helpers, &sides),
                    serde_json::from_slice::<CubemapAssetManifest>(expected[name]).unwrap()
                );
            } else {
                assert_eq!(data, expected[name], "packed asset {name}");
            }
        }
        session
            .cancel_instance_collapse(Actor::User, ready["token"].as_u64().unwrap())
            .unwrap();
        assert_eq!(session.render_document().vmf(), &source);
        let ready = preview(&mut session);
        let request = session
            .apply_instance_collapse(Actor::User, ready["token"].as_u64().unwrap())
            .unwrap();
        let applied = session
            .finish_instance_collapse(Actor::User, request.run())
            .unwrap();
        assert_eq!(
            applied["captured_preview"]["helper_id_map"],
            ready["helper_id_map"]
        );
        assert_eq!(
            applied["captured_preview"]["static_prop_id_map"],
            ready["static_prop_id_map"]
        );
        assert_eq!(session.document.vmf(), &shown);
        session.undo(Actor::User).unwrap();
        assert_eq!(session.document.vmf(), &source);
        assert_eq!(session.document.undo_labels().count(), history);
        for (path, bytes) in [&parent, &child, &middle].into_iter().zip(disk) {
            assert_eq!(std::fs::read(path).unwrap(), bytes);
        }
    }
}

#[test]
fn collapsed_assemblies_refuse_changed_models_and_nonlocal_bindings_without_partial_preview() {
    let (_directory, mut session, _parent, child, _middle) = fixture(["80", "95"]);
    let source = session.document.vmf().clone();
    let ready = preview(&mut session);
    let shown = session.render_document().vmf().clone();
    let history = session.history_snapshot();
    let model = session.game.game_dir.join("models/test/receiver.mdl");
    let original_model = std::fs::read(&model).unwrap();
    let mut changed_model = original_model.clone();
    changed_model[116..120].copy_from_slice(&25f32.to_le_bytes());
    std::fs::write(&model, &changed_model).unwrap();
    assert_eq!(session.get_instance_collapse()["ready"], false);
    let request = session
        .apply_instance_collapse(Actor::User, ready["token"].as_u64().unwrap())
        .unwrap();
    assert!(
        session
            .finish_instance_collapse(Actor::User, request.run())
            .is_err()
    );
    assert_eq!(session.document.vmf(), &source);
    assert_eq!(session.render_document().vmf(), &shown);
    assert_eq!(session.history_snapshot(), history);
    std::fs::write(&model, &original_model).unwrap();
    let ready = preview(&mut session);
    let request = session
        .apply_instance_collapse(Actor::User, ready["token"].as_u64().unwrap())
        .unwrap();
    let completed = request.run();
    std::fs::write(&model, changed_model).unwrap();
    assert!(
        session
            .finish_instance_collapse(Actor::User, completed)
            .is_err()
    );
    assert_eq!(session.document.vmf(), &source);
    assert_eq!(session.render_document().vmf(), &shown);
    assert_eq!(session.history_snapshot(), history);
    std::fs::write(&model, original_model).unwrap();
    let ready = preview(&mut session);
    let bytes = std::fs::read(&child).unwrap();
    let mut invalid = Vmf::parse(std::str::from_utf8(&bytes).unwrap()).unwrap();
    invalid
        .entities
        .iter_mut()
        .find(|entity| entity.properties.get("sides").is_some())
        .unwrap()
        .properties
        .set("sides", "4294967295");
    std::fs::write(&child, invalid.write(&Default::default())).unwrap();
    let request = session
        .apply_instance_collapse(Actor::User, ready["token"].as_u64().unwrap())
        .unwrap();
    assert!(
        session
            .finish_instance_collapse(Actor::User, request.run())
            .is_err()
    );
    let request = session
        .begin_instance_collapse(Actor::User, &session.instance_collapse_stamp())
        .unwrap();
    let error = session
        .finish_instance_collapse(Actor::User, request.run())
        .unwrap_err();
    assert!(error.contains("same child"), "{error}");
    assert_eq!(session.document.vmf(), &source);
    assert_eq!(session.render_document().vmf(), &shown);
    assert_eq!(session.history_snapshot(), history);
    std::fs::write(child, bytes).unwrap();
    let token = session.get_instance_collapse()["token"].as_u64().unwrap();
    session
        .cancel_instance_collapse(Actor::User, token)
        .unwrap();
    assert_eq!(session.render_document().vmf(), &source);
}

#[test]
fn selected_leading_collapse_preserves_nested_native_order_with_retained_later_reference() {
    let (_directory, mut session, parent, child, middle) = fixture(["80", "95"]);
    let mut leaf = Vmf::parse(&std::fs::read_to_string(&child).unwrap()).unwrap();
    let mut spot = Entity::new(205, "light_spot");
    for (key, value) in [
        ("origin", "48 -16 80"),
        ("target", "aim"),
        ("pitch", "-45"),
        ("_light", "64 255 128 140"),
        ("_lightHDR", "255 64 128 240"),
    ] {
        spot.properties.set(key, value);
    }
    let mut target = Entity::new(206, "info_target");
    target.properties.set("origin", "16 8 32");
    target.properties.set("targetname", "aim");
    leaf.entities.extend([spot, target]);
    std::fs::write(&child, leaf.write(&Default::default())).unwrap();
    let mut nested = Vmf::parse(&std::fs::read_to_string(&middle).unwrap()).unwrap();
    for helper in &mut nested.entities {
        helper
            .properties
            .set("targetname", format!("inner{}", helper.id));
    }
    std::fs::write(&middle, nested.write(&Default::default())).unwrap();
    let roots = session
        .edit(Actor::User, "three ordered roots", |tx| {
            let mut root = tx.entity_mut(500)?.clone();
            root.properties.set("targetname", "first");
            root.properties.set("origin", "-96 0 128");
            *tx.entity_mut(500)? = root.clone();
            root.id = tx.new_object_id()?;
            root.properties.set("targetname", "second");
            root.properties.set("origin", "0 0 128");
            let second = root.id;
            tx.add_entity(root.clone())?;
            root.id = tx.new_object_id()?;
            root.properties.set("targetname", "retained");
            root.properties.set("origin", "96 0 128");
            let retained = root.id;
            tx.add_entity(root)?;
            for _ in 0..5 {
                tx.new_object_id()?;
            }
            Ok([500, second, retained])
        })
        .unwrap();
    session.save(Actor::User, None).unwrap();
    session
        .select(
            &roots[..2]
                .iter()
                .copied()
                .map(ObjectId::Entity)
                .collect::<Vec<_>>(),
            SelectMode::Replace,
        )
        .unwrap();
    let source = session.document.vmf().clone();
    let disk = [&parent, &child, &middle].map(|path| std::fs::read(path).unwrap());
    let request = session
        .begin_instance_collapse_scoped(
            Actor::User,
            &session.instance_collapse_stamp(),
            Scope::SelectedLeadingReferences,
        )
        .unwrap();
    let ready = session
        .finish_instance_collapse(Actor::User, request.run())
        .unwrap();
    assert_eq!(ready["stats"]["instances"], 6);
    for count in [
        "world_brushes",
        "detail_brushes",
        "point_lights",
        "spot_lights",
        "spotlight_targets",
    ] {
        assert_eq!(ready["stats"][count], 4, "{count}");
    }
    assert_eq!(ready["stats"]["static_props"], 8);
    assert_eq!(ready["stats"]["cubemaps"], 8);
    let shown = session.render_document().vmf().clone();
    assert_eq!(
        shown.entities.iter().find(|entity| entity.id == roots[2]),
        source.entities.iter().find(|entity| entity.id == roots[2])
    );
    assert_eq!(session.document.vmf(), &source);
    let control = Control::default();
    let context = instances::SourceContext::capture(&parent, &control).unwrap();
    let before = instances::resolve_for_collapse(
        &source,
        Some(&context),
        &control,
        &mut compiler::Report::default(),
    )
    .unwrap();
    let after = instances::resolve_for_collapse(
        &shown,
        Some(&context),
        &control,
        &mut compiler::Report::default(),
    )
    .unwrap();
    let mut entities = id_map(&ready["entity_id_map"]);
    let mut helpers = id_map(&ready["helper_id_map"]);
    let mut props = id_map(&ready["static_prop_id_map"]);
    for entity in &before.vmf().entities {
        let Some((owner, file, original)) = before.entity_origin(entity.id) else {
            continue;
        };
        if owner != roots[2] {
            continue;
        }
        let route = before.entity_occurrence(entity.id).unwrap();
        let matches = after
            .vmf()
            .entities
            .iter()
            .filter(|candidate| {
                after.entity_origin(candidate.id) == Some((owner, file, original))
                    && after.entity_occurrence(candidate.id) == Some(route)
            })
            .collect::<Vec<_>>();
        assert_eq!(matches.len(), 1);
        let destination = matches[0].id;
        match entity.classname().unwrap() {
            "light" | "light_spot" | "info_target" => {
                assert!(entities.insert(entity.id, destination).is_none());
            }
            "prop_static" => {
                assert!(props.insert(entity.id, destination).is_none());
            }
            class if class.eq_ignore_ascii_case("env_cubemap") => {
                assert!(helpers.insert(entity.id, destination).is_none());
            }
            _ => {}
        }
    }
    assert_eq!((entities.len(), helpers.len(), props.len()), (18, 12, 12));
    assert!(
        before
            .vmf()
            .entities
            .iter()
            .filter(|entity| before.root_entity(entity.id) == Some(roots[2])
                && matches!(
                    entity.classname(),
                    Some("light" | "light_spot" | "info_target")
                ))
            .any(|entity| entities[&entity.id] != entity.id)
    );
    let mut sides = BTreeMap::new();
    assert_eq!(before.vmf().solids().count(), after.vmf().solids().count());
    for (original, actual) in before.vmf().solids().zip(after.vmf().solids()) {
        let mut normalized = actual.clone();
        normalized.id = original.id;
        assert_eq!(original.sides.len(), actual.sides.len());
        for (original, actual) in original.sides.iter().zip(&mut normalized.sides) {
            assert!(sides.insert(original.id, actual.id).is_none());
            actual.id = original.id;
        }
        assert_eq!(*original, normalized);
    }
    assert!(
        sides
            .values()
            .zip(sides.values().skip(1))
            .all(|(before, after)| before < after)
    );
    let options = Options {
        lighting: LightingMode::DirectLdrHdr,
        generate_reflections: true,
        ..Default::default()
    };
    let output_context = CompileContext::new("selected_leading_cubes").unwrap();
    let expanded = compiler::compile_with_source_controlled(
        &source,
        &session.game,
        options,
        Some(&output_context),
        Some(&context),
        &control,
    );
    let collapsed = compiler::compile_with_source_controlled(
        &shown,
        &session.game,
        options,
        Some(&output_context),
        Some(&context),
        &control,
    );
    assert!(!expanded.report.has_errors(), "{:?}", expanded.report);
    assert!(!collapsed.report.has_errors(), "{:?}", collapsed.report);
    assert_eq!(
        remap_manifest(
            expanded.cubemap_assets.as_ref().unwrap().clone(),
            &helpers,
            &sides
        ),
        *collapsed.cubemap_assets.as_ref().unwrap()
    );
    let expanded = expanded.bsp.unwrap();
    let collapsed = collapsed.bsp.unwrap();
    assert_eq!(collapsed.static_props().unwrap().unwrap().props.len(), 12);
    assert_eq!(collapsed.records::<Cubemap>().unwrap().len(), 12);
    for channel in [
        source_formats::bsp::LightingKind::Ldr,
        source_formats::bsp::LightingKind::Hdr,
    ] {
        assert_eq!(collapsed.world_lights(channel).unwrap().len(), 12);
    }
    assert_eq!(
        (expanded.version, expanded.map_revision),
        (collapsed.version, collapsed.map_revision)
    );
    assert_eq!(expanded.game_lumps, collapsed.game_lumps);
    for (index, expected) in collapsed.lumps.iter().enumerate() {
        if index != lump::PAKFILE && index != lump::ENTITIES {
            assert_eq!(&expanded.lumps[index], expected, "native lump {index}");
        }
    }
    let original_text = expanded.entities().unwrap();
    let replacements = entities
        .iter()
        .map(|(before, after)| {
            let original = format!("\"hammerid\" \"{before}\"\n");
            assert_eq!(original_text.matches(&original).count(), 1);
            (original, format!("\"hammerid\" \"{after}\"\n"))
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(
        original_text
            .split_inclusive('\n')
            .map(|line| replacements.get(line).map_or(line, String::as_str))
            .collect::<String>(),
        collapsed.entities().unwrap()
    );
    let before_pak = expanded.pak_data(PakLimits::default()).unwrap();
    let after_pak = collapsed.pak_data(PakLimits::default()).unwrap();
    let before_count = before_pak.entries.len();
    let after_count = after_pak.entries.len();
    let before_pak = before_pak
        .entries
        .iter()
        .map(|entry| (entry.name.as_str(), entry.data.as_slice()))
        .collect::<BTreeMap<_, _>>();
    let after_pak = after_pak
        .entries
        .iter()
        .map(|entry| (entry.name.as_str(), entry.data.as_slice()))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(before_pak.len(), before_count);
    assert_eq!(after_pak.len(), after_count);
    assert_eq!(
        before_pak.keys().collect::<Vec<_>>(),
        after_pak.keys().collect::<Vec<_>>()
    );
    for (name, bytes) in before_pak {
        if name == MANIFEST_PATH {
            assert_eq!(
                remap_manifest(serde_json::from_slice(bytes).unwrap(), &helpers, &sides),
                serde_json::from_slice::<CubemapAssetManifest>(after_pak[name]).unwrap()
            );
        } else {
            assert_eq!(bytes, after_pak[name], "packed asset {name}");
        }
    }
    let token = ready["token"].as_u64().unwrap();
    let request = session.apply_instance_collapse(Actor::User, token).unwrap();
    session
        .finish_instance_collapse(Actor::User, request.run())
        .unwrap();
    assert_eq!(session.document.vmf(), &shown);
    session.undo(Actor::User).unwrap();
    assert_eq!(session.document.vmf(), &source);
    for (path, bytes) in [&parent, &child, &middle].into_iter().zip(disk) {
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
}
