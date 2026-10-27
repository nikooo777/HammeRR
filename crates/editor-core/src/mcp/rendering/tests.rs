use super::*;
use crate::session::{ViewUpdate, working_region};
use std::collections::BTreeMap;

fn fixture() -> (tempfile::TempDir, Session, ObjectId) {
    let (directory, game) = crate::test_assets::game();
    let sky = directory.path().join("materials/skybox");
    std::fs::create_dir_all(&sky).unwrap();
    for suffix in render::sky::SUFFIXES {
        std::fs::write(
            sky.join(format!("frame_before{suffix}.vmt")),
            "UnlitGeneric { $basetexture test/checker }",
        )
        .unwrap();
    }
    let mut session = Session::new(Ok(game));
    session
        .edit(Actor::Agent, "set fixture sky", |tx| {
            tx.world_properties_mut().set("skyname", "frame_before");
            Ok(())
        })
        .unwrap();
    session
        .create_entity(
            Actor::Agent,
            "keyframe_rope",
            Some(DVec3::X * 64.0),
            BTreeMap::from([("targetname".into(), "end".into())]),
        )
        .unwrap();
    let source = session
        .create_entity(
            Actor::Agent,
            "move_rope",
            Some(DVec3::NEG_X * 64.0),
            BTreeMap::from([
                ("NextKey".into(), "end".into()),
                ("Slack".into(), "0".into()),
                ("Width".into(), "12".into()),
            ]),
        )
        .unwrap();
    session.select(&[], SelectMode::Replace).unwrap();
    session
        .set_spatial(
            Actor::Agent,
            crate::spatial::Update {
                grid_visible: Some(false),
                ..Default::default()
            },
        )
        .unwrap();
    let snapshot = session.working_region_stamp();
    session
        .set_working_region(
            Actor::Agent,
            working_region::Action::Set {
                bounds: [DVec3::splat(-80.0), DVec3::splat(80.0)],
                enabled: true,
            },
            &snapshot,
        )
        .unwrap();
    session.reload_material_assets(Actor::Agent);
    (directory, session, source)
}

fn camera() -> Camera {
    Camera::looking_at(Vec3::new(0.0, -192.0, 96.0), Vec3::ZERO)
}

fn view(session: &Session) -> ViewOptions {
    session
        .resolve_view_update(ViewUpdate {
            rope_preview: Some(true),
            sky_preview: Some(true),
            sky_exposure_ev: Some(2.0),
            selection_outlines: Some(false),
            ..Default::default()
        })
        .unwrap()
}

fn replace_context(session: &mut Session, source: ObjectId) {
    session.delete_objects(Actor::Agent, &[source]).unwrap();
    session
        .edit(Actor::Agent, "change fixture sky", |tx| {
            tx.world_properties_mut().set("skyname", "frame_after");
            Ok(())
        })
        .unwrap();
    let snapshot = session.working_region_stamp();
    session
        .set_working_region(Actor::Agent, working_region::Action::Clear, &snapshot)
        .unwrap();
    session.reload_material_assets(Actor::Agent);
}

fn report(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false), "{:?}", result.content);
    serde_json::from_str(&result.content[1].as_text().unwrap().text).unwrap()
}

#[test]
fn render_response_assembly_preserves_captured_metadata_across_later_edits() {
    let (_directory, mut session, source) = fixture();
    let broken = session
        .edit(Actor::User, "import invalid brush", |tx| {
            let id =
                map_core::edit::create_block(tx, DVec3::ZERO, DVec3::splat(64.0), "test/checker")?;
            let ObjectId::Solid(solid) = id else {
                unreachable!()
            };
            tx.solid_mut(solid)?.sides[0].plane.0 = [DVec3::ZERO, DVec3::X, DVec3::X * 2.0];
            Ok(id)
        })
        .unwrap();
    let view = view(&session);
    let mesh = session.build_scene_for_view(view);
    assert!(mesh.batches.iter().any(|batch| batch.rope));
    let metadata = FrameMetadata::capture(&mut session, view, camera());
    let generation = metadata.material_reload["generation"].clone();
    let region = metadata.working_region.clone();
    let rope = metadata.rope_preview.clone();
    assert_eq!(metadata.skybox["name"], "frame_before");
    assert_eq!(metadata.skybox["ready"], true);
    assert_eq!(metadata.skybox["eligible"], true);
    assert_eq!(region["enabled"], true);
    assert_eq!(rope["applied"], true);
    assert_eq!(rope["links"].as_array().unwrap().len(), 1);
    let rendered = RenderedView {
        metadata,
        instance_contents: json!({"retained":"instance"}),
        sky3d_preview: json!({"retained":"sky3d"}),
        normal_maps: Default::default(),
        lighting: Default::default(),
        sprite_guides: json!({"retained":"sprite"}),
        png: b"captured frame".to_vec(),
        issues: mesh.issues.clone(),
    };
    replace_context(&mut session, source);
    session.delete_objects(Actor::User, &[broken]).unwrap();
    let current = FrameMetadata::capture(&mut session, view, camera());
    assert_eq!(current.skybox["name"], "frame_after");
    assert_eq!(current.skybox["ready"], false);
    assert_eq!(current.working_region["enabled"], false);
    assert_eq!(current.rope_preview["applied"], false);
    assert_ne!(current.material_reload["generation"], generation);
    let result = report(rendered.into_tool_result(camera(), (65, 65), view, json!({}), true));
    let ObjectId::Solid(broken) = broken else {
        unreachable!()
    };
    assert_eq!(
        result["preview_issues"],
        json!([{"brush":broken,"face":null,"message":"side 0 has collinear plane points"}])
    );
    assert_eq!(result["preview_issues_omitted"], 0);
    assert_eq!(result["skybox"]["name"], "frame_before");
    assert_eq!(result["skybox"]["ready"], true);
    assert_eq!(result["skybox"]["exposure_ev"], 2.0);
    assert_eq!(result["working_region"], region);
    assert_eq!(result["rope_preview"], rope);
    assert_eq!(result["material_reload"]["generation"], generation);
    assert_eq!(result["instance_contents"]["retained"], "instance");
    assert_eq!(result["sky3d_preview"]["retained"], "sky3d");
    assert_eq!(result["sprite_guides"]["retained"], "sprite");
    assert_eq!(result["view"]["rope_preview"], true);
    let mut concise = result.clone();
    omit_render_catalogues(&mut concise);
    assert!(result["rope_preview"].get("limits").is_some());
    assert!(concise["rope_preview"].get("limits").is_none());
    assert_eq!(
        concise["rope_preview"]["diagnostics"],
        result["rope_preview"]["diagnostics"]
    );
    assert_eq!(concise["skybox"]["ready"], true);
    assert_eq!(concise["skybox"]["suppression_reason"], Value::Null);
    assert!(
        serde_json::to_vec(&concise).unwrap().len() < serde_json::to_vec(&result).unwrap().len()
    );
}

#[test]
fn frame_metadata_tracks_uniform_overrides_without_scene_key_changes_or_shared_edits() {
    let (_directory, mut session, _) = fixture();
    let shared_view = session.view();
    let source = session.document().vmf().clone();
    let history = session.history_snapshot();
    let shared_camera = session.camera();
    session.begin_property_edit(PropertyTarget::World).unwrap();
    let draft = json!(session.property_draft());
    let enabled = view(&session);
    let key = session.render_scene_key(enabled);
    for (rope, sky, tools, suppression) in [
        (true, true, true, Value::Null),
        (false, true, true, Value::Null),
        (true, false, true, json!("disabled")),
        (true, true, false, Value::Null),
    ] {
        let temporary = ViewOptions {
            rope_preview: rope,
            sky_preview: sky,
            tool_textures: tools,
            sky_exposure_ev: -3.0,
            ..enabled
        };
        assert_eq!(session.render_scene_key(temporary), key);
        let metadata = FrameMetadata::capture(&mut session, temporary, camera());
        assert_eq!(metadata.rope_preview["enabled"], rope);
        assert_eq!(metadata.rope_preview["applied"], rope);
        assert_eq!(metadata.rope_preview["links"].as_array().unwrap().len(), 1);
        assert_eq!(metadata.skybox["eligible"], sky);
        assert_eq!(metadata.skybox["suppression_reason"], suppression);
        assert_eq!(metadata.skybox["exposure_ev"], -3.0);
    }
    assert_eq!(session.view(), shared_view);
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(session.camera(), shared_camera);
    assert_eq!(json!(session.property_draft()), draft);
}

#[test]
#[ignore = "requires offscreen GPU; root runs explicitly with --ignored"]
fn rendered_metadata_retains_uploaded_frame_through_cache_toggles_and_later_edits() {
    let (_directory, session, source) = fixture();
    let shared_view = session.view();
    let view = view(&session);
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let rendered = server.render_png(&camera(), (65, 65), view).unwrap();
    let png = rendered.png.clone();
    let generation = rendered.metadata.material_reload["generation"].clone();
    let rope = rendered.metadata.rope_preview.clone();
    let region = rendered.metadata.working_region.clone();
    assert_eq!(rope["applied"], true);
    let cached = server.render_png(&camera(), (65, 65), view).unwrap();
    assert_eq!(cached.png, png);
    assert_eq!(cached.metadata.rope_preview, rope);
    let hidden = ViewOptions {
        rope_preview: false,
        ..view
    };
    assert_eq!(hidden.scene_key(0), view.scene_key(0));
    let disabled = server.render_png(&camera(), (65, 65), hidden).unwrap();
    assert_eq!(disabled.metadata.rope_preview["enabled"], false);
    assert_eq!(disabled.metadata.rope_preview["applied"], false);
    assert_ne!(disabled.png, png);
    let restored = server.render_png(&camera(), (65, 65), view).unwrap();
    assert_eq!(restored.png, png);
    assert_eq!(restored.metadata.rope_preview, rope);
    assert_eq!(lock(&shared).view(), shared_view);

    replace_context(&mut lock(&shared), source);
    let retained = report(rendered.into_tool_result(camera(), (65, 65), view, json!({}), true));
    assert_eq!(retained["rope_preview"], rope);
    assert_eq!(retained["skybox"]["name"], "frame_before");
    assert_eq!(retained["skybox"]["ready"], true);
    assert_eq!(retained["working_region"], region);
    assert_eq!(retained["material_reload"]["generation"], generation);
    let changed = server.render_png(&camera(), (65, 65), view).unwrap();
    assert_ne!(changed.png, png);
    assert_eq!(changed.metadata.rope_preview["applied"], false);
    assert_eq!(changed.metadata.rope_preview["links"], json!([]));
    assert_eq!(changed.metadata.skybox["name"], "frame_after");
    assert_eq!(changed.metadata.skybox["ready"], false);
    assert_eq!(changed.metadata.working_region["enabled"], false);
    assert_ne!(changed.metadata.material_reload["generation"], generation);
}

#[test]
fn selection_tint_changes_geometry_but_keeps_selection_outlines_and_source() {
    let (_directory, mut session, _) = fixture();
    let block = session
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(32.0),
            "test/checker",
            false,
        )
        .unwrap();
    session.select(&[block], SelectMode::Replace).unwrap();
    let source = session.document().vmf().clone();
    let history = session.history_snapshot();
    let tinted = session.view();
    let untinted = session
        .resolve_view_update(ViewUpdate {
            selection_tint: Some(false),
            ..Default::default()
        })
        .unwrap();
    assert_ne!(
        session.render_scene_key(tinted),
        session.render_scene_key(untinted)
    );
    let with = session.build_scene_for_view(tinted);
    let without = session.build_scene_for_view(untinted);
    assert_eq!(with.vertices.len(), without.vertices.len());
    assert!(
        with.vertices
            .iter()
            .zip(&without.vertices)
            .any(|(a, b)| a.color != b.color)
    );
    assert_eq!(
        with.selection_outline_ranges,
        without.selection_outline_ranges
    );
    assert_eq!(session.selected_ids(), [block]);
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(session.view(), tinted);
}
