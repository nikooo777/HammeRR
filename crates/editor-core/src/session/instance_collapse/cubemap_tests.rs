use super::*;
use compiler::cubemap_assets::MANIFEST_PATH;
use compiler::{CompileContext, CubemapAssetManifest, CubemapAssetPhase, LightingMode, Options};
use glam::DVec3;
use map_core::edit;
use source_formats::bsp::{Cubemap, PakLimits, lump};
use source_formats::vmf::{Properties, Vmf};

fn fixture() -> (tempfile::TempDir, Session, PathBuf, PathBuf, PathBuf) {
    let (directory, game) = crate::test_assets::game();
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
fn collapsed_cubemaps_remap_direct_bindings_and_preserve_native_and_pending_payloads() {
    let (_directory, mut session, parent, child, middle) = fixture();
    let source = session.document.vmf().clone();
    let history = session.document.undo_labels().count();
    let disk = [&parent, &child, &middle].map(|path| std::fs::read(path).unwrap());
    let ready = preview(&mut session);
    assert_eq!(ready["stats"]["cubemaps"], 4);
    let helpers = id_map(&ready["helper_id_map"]);
    let emitted = id_map(&ready["entity_id_map"]);
    assert_eq!((helpers.len(), emitted.len()), (4, 2));
    assert!(helpers.keys().all(|id| !emitted.contains_key(id)));
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
    assert_eq!(session.document.vmf(), &shown);
    session.undo(Actor::User).unwrap();
    assert_eq!(session.document.vmf(), &source);
    assert_eq!(session.document.undo_labels().count(), history);
    for (path, bytes) in [&parent, &child, &middle].into_iter().zip(disk) {
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
}

#[test]
fn collapsed_cubemaps_refuse_changed_or_nonlocal_bindings_without_partial_preview() {
    let (_directory, mut session, _parent, child, _middle) = fixture();
    let source = session.document.vmf().clone();
    let ready = preview(&mut session);
    let shown = session.render_document().vmf().clone();
    let history = session.history_snapshot();
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
