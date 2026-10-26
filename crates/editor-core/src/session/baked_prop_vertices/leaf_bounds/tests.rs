use super::*;
use crate::session::{PropertyTarget, compilation::BuildSettingsUpdate};

fn compiled() -> (tempfile::TempDir, Session, std::path::PathBuf) {
    let (directory, game, vmf) = super::super::test_support::leaf_membership_fixture();
    let source = directory.path().join("maps/leaf-membership.vmf");
    std::fs::write(&source, vmf.write(&Default::default())).unwrap();
    let mut session = Session::new(Ok(game));
    session.open(Actor::User, &source, false).unwrap();
    session
        .update_build_settings(
            Actor::User,
            BuildSettingsUpdate {
                lighting: Some("fullbright"),
                physics: Some("none"),
                bake_static_prop_vertices: Some(false),
                pack_static_prop_assets: Some(false),
                pack_sky_assets: Some(false),
                generate_reflections: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
    let result = session.begin_compile(Actor::User, None).unwrap().run();
    session.finish_compile(Actor::User, &result);
    assert!(
        result.as_ref().unwrap().published(),
        "{}",
        session.compile_status()
    );
    (directory, session, source)
}

fn token(session: &Session) -> String {
    session
        .inspect_baked_prop_vertices(&Query::default())
        .unwrap()
        .report["build"]["token"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn consumed_world_material_stales_unbaked_guides_inspection_and_launch() {
    let (directory, mut session, source) = compiled();
    let token = token(&session);
    session
        .set_compiled_prop_leaf_bounds(Actor::Agent, Some(token), Some(1))
        .unwrap();
    assert!(session.compiled_launch_readiness().is_ok());
    let origin = crate::session::LaunchOrigin::new(session.document_generation);
    let materials = session.compiled.as_ref().unwrap().material_sources.clone();
    assert!(materials.is_some());
    origin
        .verify_compiled_sources(None, materials.as_ref())
        .unwrap();
    let bytes = std::fs::read(source.with_extension("bsp")).unwrap();
    let material = directory.path().join("materials/test/checker.vmt");
    let mut changed = std::fs::read(&material).unwrap();
    changed.extend_from_slice(b"\n// neutral source edit\n");
    std::fs::write(material, changed).unwrap();
    session
        .compiled
        .as_ref()
        .unwrap()
        .static_prop_sources
        .as_ref()
        .unwrap()
        .verify(&Control::default())
        .unwrap();
    assert!(
        origin
            .verify_compiled_sources(None, materials.as_ref())
            .is_err()
    );
    assert!(session.compiled_launch_readiness().is_err());
    assert_eq!(session.baked_lightmap_build_state()["current"], false);
    let status = session.compile_status();
    assert_eq!(status["job"]["stale"], true);
    assert!(status["job"]["material_source_issue"].is_string());
    assert!(session.compile_label().contains("earlier material sources"));
    let report = session
        .inspect_baked_prop_vertices(&Query::default())
        .unwrap()
        .report;
    assert_eq!(report["build"]["current"], false);
    assert_eq!(session.compiled_prop_leaf_bounds()["displayed"], false);
    let serial = session.scene_serial();
    session.preflight_compiled_prop_leaf_bounds();
    assert!(session.scene_serial() > serial);
    assert_eq!(std::fs::read(source.with_extension("bsp")).unwrap(), bytes);
}

fn replace_owned_bsp(session: &mut Session, bsp: &Bsp) {
    let build = session.compiled.as_mut().unwrap();
    std::fs::write(&build.path, bsp.to_bytes()).unwrap();
    build.lightmap_identity.output = baked_build::capture_output_identity(
        &build.path,
        Some(bsp),
        baked_build::MAX_BSP_BYTES,
        "prop leaf fixture",
    );
    build.static_prop_origins = Err("Fixture replaced native prop metadata".into());
}

#[test]
fn unbaked_prop_guides_use_complete_native_membership_and_coexist_with_visibility() {
    let (_directory, mut session, source) = compiled();
    let token = token(&session);
    let inspection = session
        .inspect_baked_prop_vertices(&Query::default())
        .unwrap()
        .report;
    assert_eq!(inspection["bake"]["status"], "not_baked");
    assert_eq!(inspection["props"]["rows"][0]["status"], "disabled_by_prop");
    assert_eq!(inspection["props"]["rows"][1]["status"], "not_baked");
    assert_eq!(inspection["vertices"]["returned"], 0);
    let bsp = Bsp::read(source.with_extension("bsp")).unwrap();
    let native = bsp.static_props().unwrap().unwrap();
    let prop = &native.props[1];
    let membership = &native.leaves
        [usize::from(prop.first_leaf)..usize::from(prop.first_leaf) + usize::from(prop.leaf_count)];
    assert!(
        membership.len() > 1,
        "A real prop must join multiple native leaves"
    );
    let leaves = bsp.records::<Leaf>().unwrap();
    session.begin_property_edit(PropertyTarget::World).unwrap();
    let before = (
        session.document().vmf().clone(),
        session.camera(),
        session.selected_ids(),
        session.history_snapshot(),
        serde_json::to_value(session.property_draft()).unwrap(),
        session.compile_settings(),
        std::fs::read(&source).unwrap(),
        std::fs::read(source.with_extension("bsp")).unwrap(),
    );
    let key = session.render_scene_key(session.view());
    let state = session
        .set_compiled_prop_leaf_bounds(Actor::User, Some(token.clone()), Some(1))
        .unwrap();
    assert_ne!(session.render_scene_key(session.view()), key);
    assert_eq!(state["requested"], true);
    assert_eq!(state["displayed"], true);
    assert_eq!(state["first_leaf"], prop.first_leaf);
    assert_eq!(state["leaf_count"], membership.len());
    for (row, &ordinal) in state["native_leaves"]
        .as_array()
        .unwrap()
        .iter()
        .zip(membership)
    {
        let leaf = &leaves[usize::from(ordinal)];
        assert_eq!(row["ordinal"], ordinal);
        assert_eq!(
            row["bounds"],
            json!([leaf.mins.map(f64::from), leaf.maxs.map(f64::from)])
        );
    }
    let mesh = session.build_scene();
    let lines = mesh
        .lines
        .iter()
        .filter(|line| line.color == [220, 140, 255, 255])
        .collect::<Vec<_>>();
    assert_eq!(lines.len(), membership.len() * LINE_VERTICES_PER_BOX);
    for (vertices, &ordinal) in lines
        .as_chunks::<LINE_VERTICES_PER_BOX>()
        .0
        .iter()
        .zip(membership)
    {
        let leaf = &leaves[usize::from(ordinal)];
        for axis in 0..3 {
            let low = vertices
                .iter()
                .map(|vertex| vertex.position[axis])
                .fold(f32::INFINITY, f32::min);
            let high = vertices
                .iter()
                .map(|vertex| vertex.position[axis])
                .fold(f32::NEG_INFINITY, f32::max);
            assert_eq!(low, f32::from(leaf.mins[axis]));
            assert_eq!(high, f32::from(leaf.maxs[axis]));
        }
    }
    session
        .set_compiled_visibility_overlay(
            Actor::Agent,
            Some(token.clone()),
            Some(compiled_visibility::Target::Leaf {
                ordinal: usize::from(membership[0]),
            }),
            compiled_visibility::GuideMode::default(),
        )
        .unwrap();
    let visibility = session.compiled_visibility_overlay();
    session
        .set_compiled_prop_leaf_bounds(Actor::Agent, None, None)
        .unwrap();
    assert_eq!(session.compiled_prop_leaf_bounds()["requested"], false);
    assert_eq!(session.compiled_visibility_overlay(), visibility);
    let mesh = session.build_scene();
    assert_eq!(
        mesh.lines
            .iter()
            .filter(|line| line.color == [70, 220, 255, 255])
            .count(),
        24
    );
    assert!(
        mesh.lines
            .iter()
            .all(|line| line.color != [220, 140, 255, 255])
    );
    session.compiled.as_mut().unwrap().static_prop_origins = Err("No captured associations".into());
    let disabled = session
        .set_compiled_prop_leaf_bounds(Actor::Agent, Some(token.clone()), Some(0))
        .unwrap();
    assert_eq!(disabled["prop"], 0);
    assert_eq!(disabled["displayed"], true);
    assert!(disabled["leaf_count"].as_u64().unwrap() > 0);
    assert_eq!(
        before,
        (
            session.document().vmf().clone(),
            session.camera(),
            session.selected_ids(),
            session.history_snapshot(),
            serde_json::to_value(session.property_draft()).unwrap(),
            session.compile_settings(),
            std::fs::read(&source).unwrap(),
            std::fs::read(source.with_extension("bsp")).unwrap(),
        )
    );
}

#[test]
fn invalid_over_budget_cancelled_and_stale_requests_preserve_the_previous_guide() {
    let (_directory, mut session, source) = compiled();
    let token = token(&session);
    let previous = session
        .set_compiled_prop_leaf_bounds(Actor::User, Some(token.clone()), Some(1))
        .unwrap();
    let original = Bsp::read(source.with_extension("bsp")).unwrap();
    let serial = session.scene_serial();
    for (requested, prop) in [
        (None, Some(1)),
        (Some("wrong".into()), Some(1)),
        (Some(token.clone()), Some(u32::MAX)),
        (Some(token.clone()), None),
    ] {
        assert!(
            session
                .set_compiled_prop_leaf_bounds(Actor::Agent, requested, prop)
                .is_err()
        );
        assert_eq!(session.compiled_prop_leaf_bounds(), previous);
        assert_eq!(session.scene_serial(), serial);
    }
    let control = Control::default();
    control.cancel().unwrap();
    assert!(
        session
            .set_compiled_prop_leaf_bounds_controlled(
                Actor::Agent,
                Some(token.clone()),
                Some(0),
                &control
            )
            .unwrap_err()
            .contains("cancelled")
    );
    assert_eq!(session.compiled_prop_leaf_bounds(), previous);
    session
        .preview_block(DVec3::ZERO, DVec3::splat(32.))
        .unwrap();
    let draft = session.render_document().vmf().clone();
    assert!(
        session
            .set_compiled_prop_leaf_bounds(Actor::User, Some(token.clone()), Some(0))
            .is_err()
    );
    assert_eq!(session.render_document().vmf(), &draft);
    assert_eq!(session.compiled_prop_leaf_bounds(), previous);
    session.cancel_block_preview();
    let mut oversized = original.clone();
    let mut props = oversized.static_props().unwrap().unwrap();
    let leaf = props.leaves[usize::from(props.props[1].first_leaf)];
    props.leaves = vec![leaf; MAX_GUIDE_LEAVES + 1];
    props.props[1].first_leaf = 0;
    props.props[1].leaf_count = (MAX_GUIDE_LEAVES + 1) as u16;
    for index in [0, 2] {
        props.props[index].first_leaf = 0;
        props.props[index].leaf_count = 1;
    }
    oversized.set_static_props(&props).unwrap();
    replace_owned_bsp(&mut session, &oversized);
    let error = session
        .set_compiled_prop_leaf_bounds(Actor::Agent, Some(token.clone()), Some(1))
        .unwrap_err();
    assert!(
        error.contains("513 native leaf memberships") && error.contains("cap is 512"),
        "{error}"
    );
    assert_eq!(session.compiled_prop_leaf_bounds(), previous);
    let mut malformed = original.clone();
    let lump = malformed
        .game_lumps
        .iter_mut()
        .find(|lump| lump.id == *b"sprp")
        .unwrap();
    let models = i32::from_le_bytes(lump.data[..4].try_into().unwrap()) as usize;
    let leaf_start = 4 + models * source_formats::bsp::STATIC_PROP_NAME_BYTES;
    let leaf_count =
        i32::from_le_bytes(lump.data[leaf_start..leaf_start + 4].try_into().unwrap()) as usize;
    let records = leaf_start + 4 + leaf_count * 2 + 4;
    lump.data[records + 26..records + 28].copy_from_slice(&u16::MAX.to_le_bytes());
    replace_owned_bsp(&mut session, &malformed);
    assert!(
        session
            .set_compiled_prop_leaf_bounds(Actor::Agent, Some(token.clone()), Some(0))
            .is_err()
    );
    assert_eq!(session.compiled_prop_leaf_bounds(), previous);
    let mut invalid_bounds = original.clone();
    let mut leaves = invalid_bounds.records::<Leaf>().unwrap();
    let native = original.static_props().unwrap().unwrap();
    let ordinal = native.leaves[usize::from(native.props[0].first_leaf)];
    leaves[usize::from(ordinal)].mins[0] = i16::MAX;
    leaves[usize::from(ordinal)].maxs[0] = i16::MIN;
    invalid_bounds.set_records(&leaves);
    replace_owned_bsp(&mut session, &invalid_bounds);
    assert!(
        session
            .set_compiled_prop_leaf_bounds(Actor::Agent, Some(token.clone()), Some(0))
            .unwrap_err()
            .contains("bounds are invalid")
    );
    assert_eq!(session.compiled_prop_leaf_bounds(), previous);
    replace_owned_bsp(&mut session, &original);
    session
        .update_build_settings(
            Actor::User,
            BuildSettingsUpdate {
                bounces: Some(1),
                ..Default::default()
            },
        )
        .unwrap();
    let key = session.render_scene_key(session.view());
    session.preflight_compiled_prop_leaf_bounds();
    assert_ne!(session.render_scene_key(session.view()), key);
    let stale = session.compiled_prop_leaf_bounds();
    assert_eq!(stale["requested"], true);
    assert_eq!(stale["displayed"], false);
    assert!(stale["reason"].as_str().unwrap().contains("suppressed"));
    assert!(
        session
            .build_scene()
            .lines
            .iter()
            .all(|line| line.color != [220, 140, 255, 255])
    );
    assert!(
        session
            .set_compiled_prop_leaf_bounds(Actor::User, Some(token.clone()), Some(0))
            .is_err()
    );
    assert_eq!(session.compiled_prop_leaf_bounds(), stale);
    session
        .update_build_settings(
            Actor::User,
            BuildSettingsUpdate {
                bounces: Some(0),
                ..Default::default()
            },
        )
        .unwrap();
    std::fs::write(source.with_extension("bsp"), b"changed output").unwrap();
    session.preflight_compiled_prop_leaf_bounds();
    assert_eq!(session.compiled_prop_leaf_bounds()["displayed"], false);
    assert!(
        session
            .set_compiled_prop_leaf_bounds(Actor::User, Some(token), Some(0))
            .is_err()
    );
    session
        .set_compiled_prop_leaf_bounds(Actor::Agent, None, None)
        .unwrap();
    assert_eq!(session.compiled_prop_leaf_bounds()["requested"], false);
}

#[test]
fn complete_leaf_bounds_validate_native_ranges_bounds_and_storage_before_installation() {
    let mut bsp = Bsp::default();
    bsp.set_records(&[Leaf {
        mins: [-2, -3, -4],
        maxs: [5, 6, 7],
        ..Leaf::default()
    }]);
    let mut props = StaticProps {
        version: 6,
        models: vec![StaticProps::model_record("models/test/receiver.mdl").unwrap()],
        leaves: vec![0; MAX_GUIDE_LEAVES],
        props: vec![source_formats::bsp::StaticProp {
            leaf_count: MAX_GUIDE_LEAVES as u16,
            ..Default::default()
        }],
    };
    let control = Control::default();
    let guide = prepare(&bsp, Some(&props), 1, 0, &control).unwrap();
    assert_eq!(guide.boxes.len(), MAX_GUIDE_LEAVES);
    assert_eq!(
        guide.boxes.last().unwrap().bounds,
        [[-2., -3., -4.], [5., 6., 7.]]
    );
    assert!(
        guide.retained_bytes + guide.line_vertices * size_of::<render::scene::LineVertex>()
            <= MAX_GUIDE_BYTES
    );
    assert!(storage(usize::MAX).is_err());
    assert!(storage(MAX_GUIDE_LEAVES * 2).is_err());
    props.props[0].first_leaf = u16::MAX;
    assert!(prepare(&bsp, Some(&props), 1, 0, &control).is_err());
    props.props[0].first_leaf = 0;
    props.leaves[0] = 1;
    assert!(prepare(&bsp, Some(&props), 1, 0, &control).is_err());
    props.leaves[0] = 0;
    bsp.set_records(&[Leaf {
        mins: [2; 3],
        maxs: [1; 3],
        ..Leaf::default()
    }]);
    assert!(prepare(&bsp, Some(&props), 1, 0, &control).is_err());
    assert!(prepare(&bsp, None, 1, 0, &control).is_err());
    assert!(prepare(&bsp, Some(&props), 1, 1, &control).is_err());
}

#[test]
fn successful_build_and_document_replacement_release_prop_leaf_guides() {
    let (_directory, mut session, _source) = compiled();
    session
        .set_compiled_prop_leaf_bounds(Actor::User, Some(token(&session)), Some(1))
        .unwrap();
    let result = session.begin_compile(Actor::User, None).unwrap().run();
    session.finish_compile(Actor::User, &result);
    assert!(result.as_ref().unwrap().published());
    assert_eq!(session.compiled_prop_leaf_bounds()["requested"], false);
    session
        .set_compiled_prop_leaf_bounds(Actor::User, Some(token(&session)), Some(0))
        .unwrap();
    session.new_map(Actor::Agent, false).unwrap();
    assert_eq!(session.compiled_prop_leaf_bounds()["requested"], false);
}
