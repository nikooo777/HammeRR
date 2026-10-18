use super::*;

fn value(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false), "{:?}", result.content);
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

#[test]
fn model_snap_mcp_shares_the_cached_mesh_target_and_owned_apply() {
    let (_directory, session, selected, model, ray) = crate::test_assets::model_snap();
    let ObjectId::Solid(solid) = selected else {
        unreachable!()
    };
    let original = session.document().vmf().clone();
    let shape = map_core::subobject::topology(session.document().solid(solid).unwrap()).unwrap();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let args = json!({"method":"brush_corner","source_corner":{"solid":solid,"sides":shape.vertices[0].sides},"ray_origin":ray.origin.to_array(),"ray_direction":ray.direction.to_array()});
    let report =
        value(server.preview_surface_placement(Parameters(serde_json::from_value(args).unwrap())));
    let target = &report["plan"]["corner"]["target"];
    assert_eq!(target["kind"], "model_vertex");
    assert_eq!(target["entity"], crate::identity::id_string(model));
    assert_eq!(target["model"], "models/test/receiver.mdl");
    assert_eq!(target["provenance"], "cached_decoded_model_mesh");
    assert!(target["solid"].is_null() && target["side"].is_null());
    let token = report["token"].as_u64().unwrap();
    assert_eq!(
        value(
            server.get_surface_placement(Parameters(ModelChoiceQueryArgs { token: Some(token) }))
        ),
        report
    );
    assert_eq!(lock(&shared).document().vmf(), &original);
    value(server.apply_surface_placement(Parameters(ModelChoiceTokenArgs { token })));
    lock(&shared).undo(Actor::Agent).unwrap();
    assert_eq!(lock(&shared).document().vmf(), &original);
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
}
