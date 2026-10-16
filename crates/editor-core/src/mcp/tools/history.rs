use super::super::*;

#[tool_router(router = history_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect the current document's retained transactions, newest first. Shares History in the editor. Reports author, label, applied/redo/current positions, real added/modified/removed object counts, up to 16 object IDs and changed map sections. Object-level summaries exclude property diffs and full snapshots. Default 50 entries, maximum 100; follow next_offset. Initial state is target '0'; each transaction state means after that transaction. History is session-local and clears when a document is opened/replaced. Use the returned snapshot and state with navigate_history. Unknown authors are explicit. Navigation is bounded to 256 transactions, blocks during build/launch/shutdown and active previews.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_history(
        &self,
        Parameters(args): Parameters<HistoryArgs>,
    ) -> ToolResult {
        run(|| {
            let report = self.session().inspect_history(
                args.offset,
                args.limit.unwrap_or(crate::session::history::PAGE_SIZE),
            )?;
            serde_json::to_value(report).map_err(|error| error.to_string())
        })
    }

    #[tool(
        description = "Go to the exact retained transaction state returned by get_history, or '0' for the initial state. Requires the current history snapshot; stale, replaced and discarded branch targets reject atomically. Replays existing undo/redo operations, preserving the redo chain and original transaction authors. Does not save or mark the map clean. Returns exact direction, transaction count, source/target states and resulting dirty state. Maximum 256 transactions per call; use intermediate states for longer histories. An already-current target is a no-op."
    )]
    pub(in crate::mcp) fn navigate_history(
        &self,
        Parameters(args): Parameters<HistoryNavigationArgs>,
    ) -> ToolResult {
        run(|| {
            let result =
                self.session()
                    .navigate_history(Actor::Agent, &args.snapshot, &args.target)?;
            serde_json::to_value(result).map_err(|error| error.to_string())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(result: ToolResult) -> Value {
        let result = result.unwrap();
        assert!(!result.is_error.unwrap_or(false));
        serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
    }

    #[test]
    fn history_tools_share_real_transactions_and_reject_stale_or_undeclared_targets() {
        let mut session = Session::new(Err("no assets".into()));
        for (actor, name) in [(Actor::User, "first"), (Actor::Agent, "second")] {
            session
                .edit(actor, name, |tx| {
                    tx.world_properties_mut().set("skyname", name);
                    Ok(())
                })
                .unwrap();
        }
        let shared = Arc::new(Mutex::new(session));
        let server = EditorServer::new(shared.clone(), None);
        let initial = report(server.get_history(Parameters(HistoryArgs {
            offset: 0,
            limit: Some(1),
        })));
        assert_eq!(initial["entries"][0]["actor"], "agent");
        assert_eq!(initial["next_offset"], 1);
        assert_eq!(
            initial["entries"][0]["sections"],
            json!(["world properties"])
        );
        let older = report(server.get_history(Parameters(HistoryArgs {
            offset: 1,
            limit: Some(1),
        })));
        assert_eq!(older["entries"][0]["actor"], "you");
        let target = older["entries"][0]["state"].as_str().unwrap().to_owned();
        let args = HistoryNavigationArgs {
            snapshot: initial["snapshot"].as_str().unwrap().into(),
            target: target.clone(),
        };
        let result = report(server.navigate_history(Parameters(args)));
        assert_eq!(result["plan"]["direction"], "undo");
        assert_eq!(result["plan"]["steps"], 1);
        assert_eq!(result["current_state"], target);
        assert_eq!(
            lock(&shared)
                .document()
                .vmf()
                .world
                .properties
                .get("skyname"),
            Some("first")
        );
        assert!(
            server
                .navigate_history(Parameters(HistoryNavigationArgs {
                    snapshot: initial["snapshot"].as_str().unwrap().into(),
                    target: "0".into()
                }))
                .unwrap()
                .is_error
                .unwrap_or(false)
        );
        assert_eq!(lock(&shared).document().redo_labels().count(), 1);
        for value in [
            json!({"target":"0"}),
            json!({"snapshot":"0:0:0"}),
            json!({"snapshot":"0:0:0","target":0}),
            json!({"snapshot":"0:0:0","target":"0","extra":true}),
        ] {
            assert!(serde_json::from_value::<HistoryNavigationArgs>(value).is_err());
        }
        assert!(serde_json::from_value::<HistoryArgs>(json!({"limit":1,"target":"0"})).is_err());
        assert!(server.offscreen.lock().unwrap().renderer.is_none());
    }
}
