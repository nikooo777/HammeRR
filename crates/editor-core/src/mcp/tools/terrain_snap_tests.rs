use super::*;

fn value(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false), "{:?}", result.content);
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[test]
fn terrain_snap_tools_share_exact_target_review_cancel_and_undo() {
    let (mut session, selected, face, ray) = crate::test_assets::terrain_snap();
    let ObjectId::Solid(solid) = selected else {
        unreachable!()
    };
    session
        .transform_objects(
            Actor::Agent,
            &[selected],
            map_core::edit::Transform {
                translation: DVec3::new(0.125, 0.0625, 0.03125),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    let ids = crate::test_assets::placement_moving_roots(&mut session, selected);
    let source = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let shape = map_core::subobject::topology(session.document().solid(solid).unwrap()).unwrap();
    let args = json!({"ids":ids.iter().copied().map(crate::identity::id_string).collect::<Vec<_>>(),"method":"brush_corner", "source_corner":{"solid":solid,"sides":shape.vertices[0].sides}, "ray_origin":ray.origin.to_array(), "ray_direction":ray.direction.to_array()});
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let sources = value(
        server.get_surface_placement_sources(Parameters(SurfacePlacementSourcesArgs { ids: None })),
    );
    let expected = lock(&shared)
        .surface_placement_source_corners(&ids)
        .unwrap();
    assert_eq!(
        sources["sources"],
        json!(expected.iter().map(|source|json!({
        "owner":crate::identity::id_string(source.owner), "solid":source.component.solid,
        "sides":source.component.sides, "position":source.position.to_array(),
    })).collect::<Vec<_>>())
    );
    assert_eq!(
        sources["ids"],
        json!(
            ids.iter()
                .copied()
                .map(crate::identity::id_string)
                .collect::<Vec<_>>()
        )
    );
    assert_eq!(sources["provenance"], "committed_geometry");
    assert_eq!(
        sources["limits"],
        json!({"source_brushes":256,"source_corners":4096})
    );
    assert!(sources["sources"].as_array().unwrap().iter().any(|source| {
        source["solid"] == solid
            && source["position"]
                .as_array()
                .unwrap()
                .iter()
                .any(|value| value.as_f64().unwrap().fract() != 0.0)
    }));
    let mut explicit = ids
        .iter()
        .copied()
        .map(crate::identity::id_string)
        .rev()
        .collect::<Vec<_>>();
    explicit.push(explicit[0].clone());
    assert_eq!(
        value(
            server.get_surface_placement_sources(Parameters(SurfacePlacementSourcesArgs {
                ids: Some(explicit)
            }))
        ),
        sources
    );
    for invalid in [
        json!({"ids":[],"unknown":true}),
        json!({"ids":"solid:1"}),
        json!({"ids":[7]}),
    ] {
        assert!(serde_json::from_value::<SurfacePlacementSourcesArgs>(invalid).is_err());
    }
    let schema = server
        .get_tool("get_surface_placement_sources")
        .unwrap()
        .input_schema;
    assert_eq!(schema.get("additionalProperties"), Some(&json!(false)));
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert_eq!(lock(&shared).document().undo_labels().count(), history);
    let report = value(
        server.preview_surface_placement(Parameters(serde_json::from_value(args.clone()).unwrap())),
    );
    let target = &report["plan"]["corner"]["target"];
    assert_eq!(target["kind"], "terrain_vertex");
    assert_eq!(target["solid"], face.solid);
    assert_eq!(target["side"], face.side);
    assert_eq!(target["provenance"], "prepared_surface");
    assert!(target["sides"].is_null());
    assert_eq!(
        report["plan"]["corner"]["position"],
        json!([128.0, 16.0, 17.0])
    );
    let token = report["token"].as_u64().unwrap();
    assert_eq!(
        value(
            server.get_surface_placement_sources(Parameters(SurfacePlacementSourcesArgs {
                ids: None
            }))
        ),
        sources
    );
    assert_eq!(
        value(
            server.get_surface_placement(Parameters(ModelChoiceQueryArgs { token: Some(token) }))
        ),
        report
    );
    assert_eq!(lock(&shared).document().vmf(), &source);
    value(server.cancel_surface_placement(Parameters(ModelChoiceTokenArgs { token })));
    assert_eq!(lock(&shared).document().undo_labels().count(), history);
    let mut begin = args;
    begin.as_object_mut().unwrap().remove("ray_origin");
    begin.as_object_mut().unwrap().remove("ray_direction");
    let picking = value(
        server.begin_surface_placement_pick(Parameters(serde_json::from_value(begin).unwrap())),
    );
    let token = picking["token"].as_u64().unwrap();
    let picked = value(
        server.pick_surface_placement(Parameters(SurfacePlacementPickArgs {
            token,
            origin: ray.origin.to_array(),
            direction: ray.direction.to_array(),
        })),
    );
    assert_eq!(picked["plan"], report["plan"]);
    value(server.apply_surface_placement(Parameters(ModelChoiceTokenArgs { token })));
    assert_eq!(lock(&shared).document().undo_labels().count(), history + 1);
    lock(&shared).undo(Actor::Agent).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &source);
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
}
