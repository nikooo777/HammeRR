use super::super::*;

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InspectCompiledDetailsArgs {
    token: Option<String>,
    offset: Option<usize>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Prepare,
    Cancel,
    Clear,
    Show,
    Hide,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetCompiledDetailsArgs {
    action: Action,
    token: Option<String>,
    background: Option<bool>,
}

#[tool_router(router=compiled_details_tools,vis="pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect the explicitly prepared final native detail-prop records from the retained successful BSP without loading assets or initializing graphics. Returns preparation token, native leaf-ordered ordinal/kind/dictionary/model/origin/angles/scale/orientation/sway/shape and base lighting fields, complete layer coverage, separate build freshness and preview observations, explicit loading/prepared/ready/stale/cancelled/unavailable/context_suppressed states and 1–64-row pages. No authored source identity is inferred. Nonzero pages require the returned preparation token. Output replacement invalidates retained records. Does not select, pick, edit or frame authored objects."
    )]
    pub(in crate::mcp) fn inspect_compiled_details(
        &self,
        Parameters(args): Parameters<InspectCompiledDetailsArgs>,
    ) -> ToolResult {
        run(|| {
            let token = args.token.as_deref().map(parse_detail_token).transpose()?;
            self.session().inspect_compiled_details(
                token,
                args.offset.unwrap_or(0),
                args.limit.unwrap_or(64),
            )
        })
    }

    #[tool(
        description = "Manage the read-only Compiled details… editor layer through the same Session operations. prepare requires a current successful owned build, retires the previous layer and reads exact owned BSP bytes on one cancellable worker; background defaults false. cancel/clear/show/hide require the returned preparation token and reject background. clear cancels pending work and releases retained geometry; hide preserves preparation. show validates original compiler/output identities and separately observes verified shared model/material assets before atomically enabling the complete set. Missing assets, unsupported material effects and resource limits refuse the whole layer. Preview supports final models and atlas cards/cross/tri shapes as white static geometry, with full/vertical cards facing the actual render camera. Pole/coincident views use the Source VectorAngles singular basis. It does not simulate sway, avoidance, runtime random card mirroring, baked lighting/styles/HDR, runtime fading or native appearance parity. Layer display is suppressed during editing tools/drafts and outside Objects mode. Working region intersects whole compiled-prop geometry bounds, with conservative rotation envelopes for camera-facing cards. Temporary authored-ID hiding is independent of this compiler-owned layer; no authored owner is guessed. No authored geometry, selection, picking, camera or Undo changes."
    )]
    pub(in crate::mcp) fn set_compiled_details(
        &self,
        Parameters(args): Parameters<SetCompiledDetailsArgs>,
    ) -> ToolResult {
        run(|| match args.action {
            Action::Prepare => {
                if args.token.is_some() {
                    return Err("Prepare accepts only action and optional background".into());
                }
                let request = self.session().begin_compiled_details(Actor::Agent)?;
                let token = request.token();
                if args.background.unwrap_or(false) {
                    let shared = self.session.clone();
                    let spawned = std::thread::Builder::new()
                        .name("compiled-details".into())
                        .spawn(self.worker(move || {
                            let prepared = request.run();
                            let _ = lock(&shared).finish_compiled_details(Actor::Agent, prepared);
                        }));
                    if let Err(error) = spawned {
                        return self.session().fail_compiled_details(
                            Actor::Agent,
                            token,
                            format!("Could not start compiled detail preparation: {error}"),
                        );
                    }
                    Ok(json!({"status":"loading","token":token.to_string()}))
                } else {
                    let prepared = request.run();
                    self.session()
                        .finish_compiled_details(Actor::Agent, prepared)
                }
            }
            action => {
                if args.background.is_some() {
                    return Err("Only Prepare accepts background".into());
                }
                let token = parse_detail_token(
                    args.token
                        .as_deref()
                        .ok_or("Use the returned compiled detail token")?,
                )?;
                let mut session = self.session();
                match action {
                    Action::Cancel => session.cancel_compiled_details(Actor::Agent, token),
                    Action::Clear => session.clear_compiled_details(Actor::Agent, token),
                    Action::Show => session.set_compiled_details_visible(Actor::Agent, token, true),
                    Action::Hide => {
                        session.set_compiled_details_visible(Actor::Agent, token, false)
                    }
                    Action::Prepare => unreachable!(),
                }
            }
        })
    }
}

fn parse_detail_token(raw: &str) -> Result<u64, String> {
    if raw.is_empty()
        || raw.len() > 20
        || raw.starts_with('0')
        || !raw.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err("Detail token must be the returned opaque decimal string".into());
    }
    raw.parse::<u64>()
        .map_err(|_| "Detail token must be the returned opaque decimal string".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(result: ToolResult) -> Value {
        let result = result.unwrap();
        assert!(!result.is_error.unwrap_or(false), "{:?}", result.content);
        serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
    }

    #[test]
    fn compiled_detail_mcp_uses_owned_empty_preparation_and_guards_tokens_without_graphics() {
        let (_directory, session, _source) =
            crate::session::compiled_world_lights::test_support::compiled(false);
        let before = (
            session.document().vmf().clone(),
            session.camera(),
            session.history_snapshot(),
        );
        let server = EditorServer::new(Arc::new(Mutex::new(session)), None);
        let report = value(
            server.inspect_compiled_details(Parameters(InspectCompiledDetailsArgs::default())),
        );
        assert_eq!(report["status"], "not_loaded");
        let prepared = value(server.set_compiled_details(Parameters(
            serde_json::from_value(json!({"action":"prepare"})).unwrap(),
        )));
        assert_eq!(prepared["status"], "prepared");
        assert_eq!(prepared["page"]["total"], 0);
        let token = prepared["token"].as_str().unwrap();
        let shown = value(server.set_compiled_details(Parameters(
            serde_json::from_value(json!({"action":"show","token":token})).unwrap(),
        )));
        assert_eq!(shown["complete"], true);
        assert_eq!(shown["displayed"], true);
        assert_eq!(
            value(server.get_map_info())["rendering"]["initialized"],
            false
        );
        for args in [
            json!({"action":"show","token":"0"}),
            json!({"action":"prepare","token":token}),
            json!({"action":"hide","token":token,"background":false}),
        ] {
            assert!(
                server
                    .set_compiled_details(Parameters(serde_json::from_value(args).unwrap()))
                    .unwrap()
                    .is_error
                    .unwrap_or(false)
            );
        }
        let cleared = value(server.set_compiled_details(Parameters(
            serde_json::from_value(json!({"action":"clear","token":token})).unwrap(),
        )));
        assert_eq!(cleared["status"], "not_loaded");
        assert!(cleared["token"].is_null());
        assert!(
            server
                .inspect_compiled_details(Parameters(
                    serde_json::from_value(json!({"token":token})).unwrap()
                ))
                .unwrap()
                .is_error
                .unwrap_or(false)
        );
        let session = server.session();
        assert_eq!(
            (
                session.document().vmf().clone(),
                session.camera(),
                session.history_snapshot()
            ),
            before
        );
    }
}
