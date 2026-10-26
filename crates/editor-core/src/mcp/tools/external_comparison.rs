use super::super::*;
use crate::session::external_comparison::{Action, Filter, Query};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(extend("type"="object"))]
pub enum ExternalComparisonArgs {
    Compare {
        file: String,
        background: Option<bool>,
    },
    Page {
        token: u64,
        status: Option<String>,
        search: Option<String>,
        offset: Option<usize>,
        limit: Option<usize>,
        background: Option<bool>,
    },
    Select {
        token: u64,
        status: Option<String>,
        search: Option<String>,
        mode: Option<String>,
        background: Option<bool>,
    },
    Frame {
        token: u64,
        status: Option<String>,
        search: Option<String>,
        aspect: Option<f32>,
        background: Option<bool>,
    },
    Review {
        token: u64,
    },
    Cancel {
        token: u64,
    },
}

fn filter(status: Option<String>, search: Option<String>) -> Result<Filter, String> {
    let status=match status.as_deref(){None|Some("all")=>None,Some("identical")=>Some(map_core::comparison::Status::Identical),
        Some("changed")=>Some(map_core::comparison::Status::Changed),Some("current_only")=>Some(map_core::comparison::Status::CurrentOnly),
        Some("external_only")=>Some(map_core::comparison::Status::ExternalOnly),Some("ambiguous")=>Some(map_core::comparison::Status::Ambiguous),
        _=>return Err("Comparison status must be all, identical, changed, current_only, external_only or ambiguous".into())};
    Ok(Filter {
        status,
        search: search.unwrap_or_default(),
    })
}

#[tool_router(router=external_comparison_tools,vis="pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect the shared external VMF comparison owner, token, captured page, work/error state and explicit authored-object scope. This cheap status call does not revalidate external file bytes; page/select/frame requests always recheck the originally captured FileIdentity off lock. No editing or merging.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_external_map_comparison(&self) -> ToolResult {
        ok(self.session().external_comparison_state())
    }

    #[tool(
        description = "Compare committed root-world brushes and whole root entities with one external VMF, matched by scoped authored IDs. Canonical typed stored data includes IDs, editor/hidden state, ordered fields/connections/child geometry and opaque blocks; ignores root table order and parser/writer-normalized lexical syntax. No map-settings, recursive-instance, geometric-equivalence or merge claim. Results are identical/changed/current_only/external_only/ambiguous; duplicate scoped IDs have no actions. Compare captures an owned bounded off-lock read; page/select/frame recheck original source bytes without silently accepting a replacement. Search covers displayed current/external labels and IDs only. Search/status filters define complete current actions across all pages; pages1–64 rows. Selection/framing exclude external-only/ambiguous rows and refuse stale ownership/documents/drafts; framing refuses any missing/invalid member bounds. review transfers idle ownership to a new token; cancel drops only owned matching work. background defaults false. UI: Compare external map… → Choose VMF… → Compare; filter/pages, Select current matches, Frame current matches, Cancel / Close."
    )]
    pub(in crate::mcp) fn external_map_comparison(
        &self,
        Parameters(args): Parameters<ExternalComparisonArgs>,
    ) -> ToolResult {
        run(|| {
            let (request, background) = match args {
                ExternalComparisonArgs::Compare { file, background } => (
                    self.session()
                        .begin_external_comparison(Actor::Agent, Path::new(&file))?,
                    background.unwrap_or(false),
                ),
                ExternalComparisonArgs::Page {
                    token,
                    status,
                    search,
                    offset,
                    limit,
                    background,
                } => (
                    self.session().query_external_comparison(
                        Actor::Agent,
                        token,
                        Query {
                            filter: filter(status, search)?,
                            offset: offset.unwrap_or(0),
                            limit: limit.unwrap_or(64),
                            action: Action::Page,
                        },
                    )?,
                    background.unwrap_or(false),
                ),
                ExternalComparisonArgs::Select {
                    token,
                    status,
                    search,
                    mode,
                    background,
                } => (
                    self.session().query_external_comparison(
                        Actor::Agent,
                        token,
                        Query {
                            filter: filter(status, search)?,
                            offset: 0,
                            limit: 64,
                            action: Action::Select(parse_select_mode(
                                mode.as_deref().unwrap_or("replace"),
                            )?),
                        },
                    )?,
                    background.unwrap_or(false),
                ),
                ExternalComparisonArgs::Frame {
                    token,
                    status,
                    search,
                    aspect,
                    background,
                } => (
                    self.session().query_external_comparison(
                        Actor::Agent,
                        token,
                        Query {
                            filter: filter(status, search)?,
                            offset: 0,
                            limit: 64,
                            action: Action::Frame(aspect.unwrap_or(1.0)),
                        },
                    )?,
                    background.unwrap_or(false),
                ),
                ExternalComparisonArgs::Review { token } => {
                    return self
                        .session()
                        .review_external_comparison(Actor::Agent, token);
                }
                ExternalComparisonArgs::Cancel { token } => {
                    return self
                        .session()
                        .cancel_external_comparison(Actor::Agent, token);
                }
            };
            if !background {
                let completed = request.run();
                return self
                    .session()
                    .finish_external_comparison(Actor::Agent, completed);
            }
            let token = request.token();
            let shared = self.session.clone();
            let spawned = std::thread::Builder::new()
                .name("external-map-comparison".into())
                .spawn(self.worker(move || {
                    let completed = request.run();
                    let _ = lock(&shared).finish_external_comparison(Actor::Agent, completed);
                }));
            if let Err(error) = spawned {
                let _ = self
                    .session()
                    .cancel_external_comparison(Actor::Agent, token);
                return Err(format!("Could not start comparison worker: {error}"));
            }
            Ok(json!({"token":token,"background":true}))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(result: ToolResult) -> Value {
        let result = result.unwrap();
        assert!(!result.is_error.unwrap_or(false), "{result:?}");
        serde_json::from_str(&result.content[0].as_text().unwrap().text).unwrap()
    }
    #[test]
    fn comparison_mcp_shares_owned_filtered_pages_and_full_current_selection() {
        let directory = tempfile::tempdir().unwrap();
        let mut session = Session::new(Err("No game required".into()));
        session
            .edit(Actor::User, "comparison point", |tx| {
                let mut entity = source_formats::vmf::Entity::new(10, "logic_relay");
                entity.properties.set("targetname", "Review relay");
                entity.properties.set("origin", "0 0 64");
                tx.add_entity(entity)
            })
            .unwrap();
        let path = directory.path().join("external.vmf");
        std::fs::write(&path, session.document().vmf().write(&Default::default())).unwrap();
        let original = session.document().vmf().clone();
        let shared = Arc::new(Mutex::new(session));
        let server = EditorServer::new(shared.clone(), None);
        let call = |args| {
            server.external_map_comparison(Parameters(serde_json::from_value(args).unwrap()))
        };
        let page = value(call(json!({"action":"compare","file":path})));
        assert_eq!(page["rows"][0]["label"], "Review relay · logic_relay");
        assert_eq!(page["rows"][0]["status"], "identical");
        let token = page["token"].as_u64().unwrap();
        let selected = value(call(
            json!({"action":"select","token":token,"status":"identical","search":"Review"}),
        ));
        assert_eq!(selected["current_matches"], 1);
        assert_eq!(
            lock(&shared).selected_ids(),
            [map_core::ObjectId::Entity(10)]
        );
        assert!(
            lock(&shared)
                .cancel_external_comparison(Actor::User, token)
                .is_err()
        );
        value(call(json!({"action":"cancel","token":token})));
        assert!(value(server.get_external_map_comparison())["token"].is_null());
        assert_eq!(lock(&shared).document().vmf(), &original);
        assert!(server.offscreen.lock().unwrap().renderer.is_none());
        for args in [
            json!({"action":"compare","file":"x.vmf","token":token}),
            json!({"action":"cancel","token":token,"search":"x"}),
            json!({"action":"frame","token":token,"command":"merge"}),
        ] {
            assert!(serde_json::from_value::<ExternalComparisonArgs>(args).is_err());
        }
    }
}
