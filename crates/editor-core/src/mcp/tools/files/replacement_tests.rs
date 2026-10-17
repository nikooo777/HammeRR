use super::*;

fn value(result: ToolResult, error: bool) -> Value {
    let result = result.unwrap();
    assert_eq!(result.is_error.unwrap_or(false), error, "{result:?}");
    serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
}

fn recovery_state(session: &Session) -> Value {
    let mut state = session.recovery_status();
    state
        .as_object_mut()
        .unwrap()
        .remove("next_autosave_seconds");
    state
}

#[test]
fn mcp_replacement_tokens_reject_peer_edits_and_saves_for_every_replacement() {
    for action in ["new", "open", "recent", "recovery"] {
        for save in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let mut state = Session::new(Err("no assets needed".into()));
            state.configure_recent_maps(Ok(directory.path().join("preferences")));
            state.configure_recovery(Ok(directory.path().join("recovery")));
            let path = directory.path().join("source.vmf");
            std::fs::write(&path, "world { id 1 classname worldspawn }").unwrap();
            state.open(Actor::User, &path, false).unwrap();
            let request = state
                .begin_recovery_snapshot(crate::recovery::Kind::Manual)
                .unwrap();
            let entry = state
                .finish_recovery_snapshot(Actor::User, &request, request.run())
                .unwrap();
            let shared = Arc::new(Mutex::new(state));
            let server = EditorServer::new(shared.clone(), None);
            let inspected = value(server.get_map_info(), false);
            let token = inspected["replacement_review"].as_str().unwrap();
            assert_eq!(inspected["generation"], lock(&shared).document_generation());
            if save {
                lock(&shared)
                    .save(Actor::User, Some(&directory.path().join("copy.vmf")))
                    .unwrap();
            } else {
                lock(&shared)
                    .create_block(
                        Actor::User,
                        DVec3::ZERO,
                        DVec3::splat(16.0),
                        "brick/test",
                        false,
                    )
                    .unwrap();
            }
            let source = lock(&shared).document().vmf().clone();
            let history = lock(&shared).history_snapshot();
            let recent = lock(&shared).recent_maps();
            let recovery = recovery_state(&lock(&shared));
            let result = match action {
                "new" => server.new_map(Parameters(
                    serde_json::from_value(json!({"discard_changes":true,"expected_review":token}))
                        .unwrap(),
                )),
                "open" => server.open_map(Parameters(
                    serde_json::from_value(
                        json!({"path":path,"discard_changes":true,"expected_review":token}),
                    )
                    .unwrap(),
                )),
                "recent" => server.open_recent_map(Parameters(
                    serde_json::from_value(
                        json!({"path":path,"discard_changes":true,"expected_review":token}),
                    )
                    .unwrap(),
                )),
                "recovery" => server.restore_recovery_snapshot(Parameters(
                    serde_json::from_value(
                        json!({"id":entry.id,"discard_changes":true,"expected_review":token}),
                    )
                    .unwrap(),
                )),
                _ => unreachable!(),
            };
            let failed = result.unwrap();
            assert!(failed.is_error.unwrap_or(false), "{failed:?}");
            assert!(
                failed.content[0]
                    .as_text()
                    .unwrap()
                    .text
                    .contains("Map changed")
            );
            let session = lock(&shared);
            assert_eq!(session.document().vmf(), &source);
            assert_eq!(session.history_snapshot(), history);
            assert_eq!(session.recent_maps(), recent);
            assert_eq!(recovery_state(&session), recovery);
            drop(session);
            assert!(server.offscreen.lock().unwrap().renderer.is_none());
        }
    }
}

#[test]
fn matching_mcp_reviews_keep_discard_opt_in_and_legacy_arguments_work() {
    let shared = Arc::new(Mutex::new(Session::new(Err("no assets needed".into()))));
    let server = EditorServer::new(shared.clone(), None);
    lock(&shared)
        .create_block(
            Actor::User,
            DVec3::ZERO,
            DVec3::splat(16.0),
            "brick/test",
            false,
        )
        .unwrap();
    let info = value(server.get_map_info(), false);
    let token = info["replacement_review"].as_str().unwrap();
    let failed = server
        .new_map(Parameters(
            serde_json::from_value(json!({"expected_review":token})).unwrap(),
        ))
        .unwrap();
    assert!(failed.is_error.unwrap_or(false));
    value(
        server.new_map(Parameters(
            serde_json::from_value(json!({"expected_review":token,"discard_changes":true}))
                .unwrap(),
        )),
        false,
    );
    value(
        server.new_map(Parameters(serde_json::from_value(json!({})).unwrap())),
        false,
    );
}

#[tokio::test]
async fn mcp_generation_guards_preserve_outputs_and_do_not_start_compile_jobs() {
    let directory = tempfile::tempdir().unwrap();
    let shared = Arc::new(Mutex::new(Session::new(Err("no assets needed".into()))));
    let server = EditorServer::new(shared.clone(), None);
    let generation = value(server.get_map_info(), false)["generation"].clone();
    lock(&shared).new_map(Actor::User, true).unwrap();
    let saved = directory.path().join("chosen.vmf");
    let output = directory.path().join("chosen.bsp");
    std::fs::write(&saved, b"keep vmf").unwrap();
    std::fs::write(&output, b"keep bsp").unwrap();
    let args = json!({"path":output,"expected_generation":generation});
    let results = [
        server.save_map(Parameters(
            serde_json::from_value(json!({"path":saved,"expected_generation":generation})).unwrap(),
        )),
        server.start_compile(Parameters(serde_json::from_value(args.clone()).unwrap())),
        server
            .compile_map(Parameters(serde_json::from_value(args.clone()).unwrap()))
            .await,
        server
            .run_map(Parameters(serde_json::from_value(args).unwrap()))
            .await,
    ];
    for result in results {
        let failed = result.unwrap();
        assert!(failed.is_error.unwrap_or(false), "{failed:?}");
        assert!(
            failed.content[0]
                .as_text()
                .unwrap()
                .text
                .contains("replaced")
        );
    }
    assert!(!lock(&shared).compiling());
    assert!(lock(&shared).compile_token().is_none());
    assert!(lock(&shared).path().is_none());
    assert_eq!(std::fs::read(saved).unwrap(), b"keep vmf");
    assert_eq!(std::fs::read(output).unwrap(), b"keep bsp");
    assert!(server.offscreen.lock().unwrap().renderer.is_none());
}
