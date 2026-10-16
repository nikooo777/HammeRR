use super::*;

fn value(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false));
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}
#[test]
fn hdr_sky_view_requests_validate_atomically_before_graphics_and_preserve_source() {
    let mut session = Session::new(Err("fixture content unavailable".into()));
    session.begin_property_edit(PropertyTarget::World).unwrap();
    let source = session.document().vmf().clone();
    let history = session.history_snapshot();
    let draft = json!(session.property_draft());
    let camera = session.camera();
    let shared = Arc::new(Mutex::new(session));
    let server = EditorServer::new(shared.clone(), None);
    assert_eq!(
        value(server.get_map_info())["rendering"]["initialized"],
        false
    );
    let default = lock(&shared).view();
    assert_eq!(view_state(default)["sky_channel"], "ldr");
    assert_eq!(view_state(default)["sky_exposure_ev"], 0.0);
    for args in [
        json!({"sky_channel":"auto","surface":"flat"}),
        json!({"sky_channel":"hdr","sky_exposure_ev":8.01,"tool_textures":false}),
        json!({"sky_exposure_ev":-8.01,"selection_outlines":false}),
    ] {
        let changed = server
            .set_view(Parameters(serde_json::from_value(args.clone()).unwrap()))
            .unwrap();
        assert_eq!(changed.is_error, Some(true));
        assert_eq!(lock(&shared).view(), default);
        let rendered = server
            .render_view(Parameters(serde_json::from_value(args).unwrap()))
            .unwrap();
        assert_eq!(rendered.is_error, Some(true));
        assert_eq!(
            value(server.get_map_info())["rendering"]["initialized"],
            false
        );
        assert_eq!(lock(&shared).view(), default);
    }
    for args in [
        json!({"sky_channel":true}),
        json!({"sky_exposure_ev":"NaN"}),
    ] {
        assert!(serde_json::from_value::<SetViewArgs>(args.clone()).is_err());
        assert!(serde_json::from_value::<RenderArgs>(args).is_err());
    }
    let view = value(
        server.set_view(Parameters(
            serde_json::from_value(
                json!({"sky_preview":true,"sky_channel":"hdr","sky_exposure_ev":2}),
            )
            .unwrap(),
        )),
    );
    assert_eq!(view["sky_channel"], "hdr");
    assert_eq!(view["sky_exposure_ev"], 2.0);
    let state = value(server.get_map_info());
    assert_eq!(state["skybox"]["requested_channel"], "hdr");
    assert_eq!(state["skybox"]["tonemap"], "reinhard_per_channel");
    assert_eq!(state["skybox"]["eligible"], false);
    assert_eq!(state["rendering"]["initialized"], false);
    let session = lock(&shared);
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(json!(session.property_draft()), draft);
    assert_eq!(session.camera(), camera);
}
