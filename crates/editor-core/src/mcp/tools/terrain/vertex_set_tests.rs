use super::*;
use map_core::surface::FaceId;
use rmcp::model::{CallToolRequestParams, CallToolResponse, NumberOrString};
use rmcp::service::{RequestContext, serve_directly};
use std::time::Duration;

fn state(session: &Session) -> Value {
    json!({
        "source":session.document().vmf().write(&Default::default()),
        "draft":session.render_document().vmf().write(&Default::default()),
        "preview":session.face_preview_token(),
        "highlight":session.terrain_triangle_highlight_json(),
        "history":session.history_snapshot(),
    })
}

#[tokio::test]
async fn absolute_vertex_tool_dispatch_uses_strict_explicit_values_without_highlight_changes() {
    let mut session = Session::new(Err("fixture content unavailable".into()));
    let ObjectId::Solid(solid) = session
        .create_block(Actor::User, DVec3::ZERO, DVec3::splat(64.0), "test", false)
        .unwrap()
    else {
        unreachable!()
    };
    let side = session
        .document()
        .solid(solid)
        .unwrap()
        .sides
        .iter()
        .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
        .unwrap()
        .id;
    let face = FaceId { solid, side };
    session.create_terrain(Actor::User, &[face], 2, 2).unwrap();
    session
        .highlight_terrain_vertices(Actor::User, &[(face, vec![12])])
        .unwrap();
    let source_map = session.document().vmf().clone();
    let source = session.terrain_vertices_json(&[face], false).unwrap()["source"].clone();
    let guides = session.terrain_triangle_highlight_json();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    let (transport, _client) = tokio::io::duplex(4096);
    let service = serve_directly::<rmcp::RoleServer, _, _, _, _>(server.clone(), transport, None);
    let valid = json!({"source":source,"targets":[{"face":{"solid":solid,"side":side},"vertices":[{"vertex":7,"z":-8191.25},{"vertex":0,"position":[0.125,1.25,8191.5]}]}]});
    for (arguments, success) in [
        (valid.clone(), true),
        (
            json!({"source":source,"targets":[{"face":{"solid":solid,"side":side},"vertices":[{"vertex":0,"z":42},{"vertex":25,"z":1}]}]}),
            false,
        ),
        (
            json!({"source":source,"targets":[{"face":{"solid":solid,"side":side},"vertices":[{"vertex":0,"z":42},{"vertex":0,"z":1}]}]}),
            false,
        ),
        (
            json!({"source":source,"targets":[{"face":{"solid":solid,"side":side},"vertices":[{"vertex":0,"z":42,"position":[0,0,0]}]}]}),
            false,
        ),
        (
            json!({"source":source,"targets":[{"face":{"solid":solid,"side":side},"vertices":[{"vertex":0}]}]}),
            false,
        ),
        (
            json!({"source":source,"targets":[{"face":{"solid":solid,"side":side},"vertices":[{"vertex":0,"z":65537}]}]}),
            false,
        ),
        (
            json!({"source":{"generation":"999","revision":"999"},"targets":valid["targets"]}),
            false,
        ),
    ] {
        let before = state(&lock(&shared));
        let request = CallToolRequestParams::new("preview_terrain_vertex_set")
            .with_arguments(arguments.as_object().unwrap().clone());
        let context = RequestContext::new(NumberOrString::Number(1), service.peer().clone());
        match tokio::time::timeout(Duration::from_secs(5), server.call_tool(request, context))
            .await
            .unwrap()
        {
            Ok(CallToolResponse::Complete(result)) => {
                assert_eq!(!result.is_error.unwrap_or(false), success, "{result:?}")
            }
            Err(error) => assert!(!success, "{error:?}"),
            _ => panic!("unexpected response"),
        }
        let session = lock(&shared);
        assert_eq!(session.document().vmf(), &source_map);
        assert_eq!(session.terrain_triangle_highlight_json(), guides);
        if !success {
            assert_eq!(state(&session), before);
        }
        let draft = session.terrain_grids(&[face], true).unwrap();
        assert_eq!(draft[0].mesh.positions[7].z, -8191.25);
        assert_eq!(draft[0].mesh.positions[0], DVec3::new(0.125, 1.25, 8191.5));
    }
    tokio::time::timeout(Duration::from_secs(5), service.cancel())
        .await
        .unwrap()
        .unwrap();
}
