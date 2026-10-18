use super::*;

fn value(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false), "{:?}", result.content);
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[test]
fn terrain_snap_tools_share_exact_target_review_cancel_and_undo() {
    let (session, selected, face, ray) = crate::test_assets::terrain_snap();
    let ObjectId::Solid(solid) = selected else {
        unreachable!()
    };
    let source = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let shape = map_core::subobject::topology(session.document().solid(solid).unwrap()).unwrap();
    let args = json!({"method":"brush_corner", "source_corner":{"solid":solid,"sides":shape.vertices[0].sides}, "ray_origin":ray.origin.to_array(), "ray_direction":ray.direction.to_array()});
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
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
