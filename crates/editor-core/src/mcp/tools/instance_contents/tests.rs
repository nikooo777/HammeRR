use super::*;

fn value(result: ToolResult) -> Value {
    let result = result.unwrap();
    assert!(!result.is_error.unwrap_or(false), "{:?}", result.content);
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

fn fixture() -> (tempfile::TempDir, EditorServer, ObjectId, PathBuf) {
    let (directory, game, parent, child) = crate::test_assets::instance_map(false);
    let mut session = Session::new(Ok(game));
    session.open(Actor::User, &parent, true).unwrap();
    let solids = session
        .document()
        .vmf()
        .world
        .solids
        .iter()
        .map(|solid| ObjectId::Solid(solid.id))
        .collect::<Vec<_>>();
    session
        .edit(Actor::User, "isolated reference parent", |tx| {
            for &solid in &solids {
                tx.remove(solid)?;
            }
            Ok(())
        })
        .unwrap();
    session.save(Actor::User, None).unwrap();
    let owner = ObjectId::Entity(session.document().vmf().entities.last().unwrap().id);
    session.set_view(ViewOptions {
        grid_visible: false,
        ..session.view()
    });
    let server = EditorServer::new(Arc::new(Mutex::new(session)), None);
    (directory, server, owner, child)
}

#[test]
fn instance_tools_share_lazy_refresh_owner_picks_framing_and_atomic_guards() {
    let (_directory, server, owner, child) = fixture();
    let source = server.session().document().vmf().clone();
    let view = server.session().view();
    let history = server.session().document().undo_labels().count();
    let camera = server.session().camera();
    let serials = {
        let session = server.session();
        (session.scene_serial(), session.view_serial())
    };
    for args in [json!({"limit":0}), json!({"limit":65})] {
        assert!(
            server
                .inspect_instance_contents(Parameters(serde_json::from_value(args).unwrap()))
                .unwrap()
                .is_error
                .unwrap_or(false)
        );
    }
    let before =
        value(server.inspect_instance_contents(Parameters(InspectInstanceContentsArgs::default())));
    assert_eq!(before["state"], "not_loaded");
    assert_eq!(
        value(server.get_map_info())["rendering"]["initialized"],
        false
    );
    let ready = value(server.set_instance_contents(Parameters(
        serde_json::from_value(json!({"action":"refresh"})).unwrap(),
    )));
    assert_eq!(ready["state"], "ready");
    assert_eq!(ready["displayed"], false);
    assert_eq!(ready["stats"]["world_brushes"], 1);
    assert_eq!(ready["stats"]["detail_brushes"], 1);
    for enabled in [true, false] {
        let toggled = value(server.set_view(Parameters(
            serde_json::from_value(json!({"instance_contents":enabled})).unwrap(),
        )));
        assert_eq!(toggled["instance_contents"], enabled);
        let session = server.session();
        assert_eq!((session.scene_serial(), session.view_serial()), serials);
    }
    assert_eq!(
        value(server.get_map_info())["rendering"]["initialized"],
        false
    );
    let args = json!({"x":32,"y":32,"width":65,"height":65,"camera":{"position":[32,8,96],"look_at":[32,8,48]},"instance_contents":true});
    let hit =
        value(server.pick_at_pixel(Parameters(serde_json::from_value(args.clone()).unwrap())));
    assert_eq!(hit["object"], id_string(owner));
    assert!(hit["solid"].is_null() && hit["side"].is_null());
    let mut hidden_args = args;
    hidden_args["instance_contents"] = json!(false);
    assert_eq!(
        value(server.pick_at_pixel(Parameters(serde_json::from_value(hidden_args).unwrap()))),
        json!({"hit":null})
    );
    let token = ready["token"].as_u64().unwrap().to_string();
    let framed = value(
        server.set_instance_contents(Parameters(
            serde_json::from_value(
                json!({"action":"frame","token":token,"ids":[id_string(owner)],"aspect":1.5}),
            )
            .unwrap(),
        )),
    );
    assert!(framed.is_object());
    assert_ne!(server.session().camera(), camera);
    server.session().set_camera(camera);
    for args in [
        json!({"action":"clear","token":token,"background":true}),
        json!({"action":"refresh","token":token}),
        json!({"action":"frame","token":token,"aspect":0}),
        json!({"action":"clear","token":"18446744073709551615"}),
    ] {
        assert!(
            server
                .set_instance_contents(Parameters(serde_json::from_value(args).unwrap()))
                .unwrap()
                .is_error
                .unwrap_or(false)
        );
        assert_eq!(server.session().camera(), camera);
    }
    assert!(
        serde_json::from_value::<InstanceContentsArgs>(json!({"action":"refresh","typo":true}))
            .is_err()
    );
    assert!(serde_json::from_value::<RenderArgs>(json!({"instance_contents":"true"})).is_err());
    let child_bytes = std::fs::read(&child).unwrap();
    std::fs::write(
        child,
        [child_bytes.as_slice(), b"\n// changed dependency\n"].concat(),
    )
    .unwrap();
    let stale =
        value(server.inspect_instance_contents(Parameters(InspectInstanceContentsArgs::default())));
    assert_eq!(stale["state"], "stale");
    assert_eq!(stale["available"], false);
    let session = server.session();
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.document().undo_labels().count(), history);
    assert_eq!(session.view(), view);
    assert_eq!(session.camera(), camera);
    assert_eq!((session.scene_serial(), session.view_serial()), serials);
}

fn rendered(server: &EditorServer, args: Value) -> (Vec<u8>, Value) {
    let result = server
        .render_view(Parameters(serde_json::from_value(args).unwrap()))
        .unwrap();
    assert!(!result.is_error.unwrap_or(false), "{:?}", result.content);
    (
        base64::engine::general_purpose::STANDARD
            .decode(&result.content[0].as_image().unwrap().data)
            .unwrap(),
        serde_json::from_str(&result.content[1].as_text().unwrap().text).unwrap(),
    )
}

#[test]
#[ignore = "requires offscreen GPU; root runs explicitly with --ignored"]
fn instance_mcp_retains_uploaded_report_and_preflights_stale_geometry_before_cache_reuse() {
    let (_directory, server, owner, child) = fixture();
    let source = server.session().document().vmf().clone();
    let history = server.session().document().undo_labels().count();
    let view = server.session().view();
    let camera = server.session().camera();
    let args = json!({"width":65,"height":65,"camera":{"position":[32,8,96],"look_at":[32,8,48]},"selection_outlines":false,"instance_contents":false});
    let off = rendered(&server, args.clone());
    value(server.set_instance_contents(Parameters(
        serde_json::from_value(json!({"action":"refresh"})).unwrap(),
    )));
    let mut on_args = args.clone();
    on_args["instance_contents"] = json!(true);
    let on = rendered(&server, on_args.clone());
    assert_ne!(off.0, on.0);
    assert_eq!(on.1["instance_contents"]["displayed"], true);
    assert_eq!(off.0, rendered(&server, args.clone()).0);
    assert_eq!(on.0, rendered(&server, on_args.clone()).0);
    value(server.set_selection_mode(Parameters(
        serde_json::from_value(json!({"mode":"faces"})).unwrap(),
    )));
    let suppressed = rendered(&server, on_args.clone());
    assert_eq!(suppressed.0, off.0);
    assert_eq!(
        suppressed.1["instance_contents"]["status"],
        "context_suppressed"
    );
    value(server.set_selection_mode(Parameters(
        serde_json::from_value(json!({"mode":"objects"})).unwrap(),
    )));
    assert_eq!(rendered(&server, on_args.clone()).0, on.0);
    std::fs::write(
        &child,
        [
            std::fs::read(&child).unwrap(),
            b"\n// changed dependency\n".to_vec(),
        ]
        .concat(),
    )
    .unwrap();
    let stale = rendered(&server, on_args);
    assert_eq!(stale.0, off.0);
    assert_eq!(stale.1["instance_contents"]["state"], "stale");
    assert_eq!(stale.1["instance_contents"]["displayed"], false);
    assert_eq!(on.1["instance_contents"]["displayed"], true);
    assert_eq!(
        on.1["instance_contents"]["owners"][0]["owner"],
        id_string(owner)
    );
    let session = server.session();
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.document().undo_labels().count(), history);
    assert_eq!(session.view(), view);
    assert_eq!(session.camera(), camera);
}

#[test]
fn reference_hits_deduplicate_to_grouped_owner_and_cycle_by_true_surface_distance() {
    let (_directory, server, owner, _) = fixture();
    let target;
    let behind;
    {
        let mut session = server.session();
        behind = session
            .create_block(
                Actor::User,
                DVec3::new(28.0, 4.0, 0.0),
                DVec3::new(36.0, 12.0, 8.0),
                "root",
                false,
            )
            .unwrap();
        target = session
            .create_entity(
                Actor::User,
                "info_target",
                Some(DVec3::new(128.0, 0.0, 0.0)),
                BTreeMap::new(),
            )
            .unwrap();
        let token = session.begin_group_edit(None, false).unwrap()["token"]
            .as_u64()
            .unwrap();
        session
            .update_group_edit(
                token,
                Some("Reference owner group".into()),
                None,
                Some(vec![
                    map_core::groups::Member::Object(owner),
                    map_core::groups::Member::Object(target),
                ]),
            )
            .unwrap();
        session.apply_group_edit(Actor::User, token).unwrap();
        session.select(&[], SelectMode::Replace).unwrap();
        let view = session.view();
        session.set_view(ViewOptions {
            instance_contents: true,
            ..view
        });
        session.save(Actor::User, None).unwrap();
    }
    value(server.set_instance_contents(Parameters(
        serde_json::from_value(json!({"action":"refresh"})).unwrap(),
    )));
    let mut session = server.session();
    let source = session.document().vmf().clone();
    let history = session.document().undo_labels().count();
    let ray = Ray {
        origin: DVec3::new(32.0, 8.0, 96.0),
        direction: DVec3::NEG_Z * 3.0,
    };
    let candidates = session.object_hit_candidates(&ray).unwrap();
    assert_eq!(candidates["total"], 2);
    assert_eq!(candidates["candidates"][0]["object"], id_string(owner));
    assert_eq!(candidates["candidates"][1]["object"], id_string(behind));
    assert!(
        candidates["candidates"][0]["solid"].is_null()
            && candidates["candidates"][0]["side"].is_null()
    );
    let mut group = vec![owner, target];
    group.sort();
    session.cycle_object_hit(Actor::User, &ray).unwrap();
    assert_eq!(session.selected_ids(), group);
    session.cycle_object_hit(Actor::User, &ray).unwrap();
    assert_eq!(session.selected_ids(), vec![behind]);
    assert_eq!(session.document().vmf(), &source);
    assert_eq!(session.document().undo_labels().count(), history);
}
