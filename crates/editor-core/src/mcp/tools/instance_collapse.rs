use super::super::*;
use crate::session::instance_collapse::Scope;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(extend("type" = "object"))]
pub enum InstanceCollapseArgs {
    Prepare {
        snapshot: String,
        scope: Option<Scope>,
        background: Option<bool>,
    },
    Apply {
        token: u64,
        background: Option<bool>,
    },
    Review {
        token: u64,
    },
    Cancel {
        token: u64,
    },
}

#[tool_router(router = instance_collapse_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect the owned instance collapse preview, snapshot, token, source/material/dependency staleness, shared family statistics, fresh object IDs and bounded face/entity provenance. visible_root_order lists visible root IDs in effective expansion order; malformed ordering reports root_order_error without substituting parent-array order. Scope defaults to all_visible_references; selected_references captures any admitted nonempty subset of complete visible root helpers, in effective order. selected_leading_references remains a compatibility spelling for the same widened selection scope. Runtime entity_id_map, cubemap helper_id_map and static_prop_id_map retain their meanings; compiler_helper_id_map also reports other consumed helper/detail owners. Unselected and hidden root references and child files stay intact. No source or history edit.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_instance_collapse(&self) -> ToolResult {
        ok(self.session().get_instance_collapse())
    }

    #[tool(
        description = "Preview or apply materialization of all visible instance references or any selected subset through shared compiler preparation. Admitted families include world/detail brushes and checked terrain, checked static props and lighting-origin bindings, static lights/targets/environment/fog/spawns, supported stationary func_brush/func_wall models, occluders, visclusters, overlays and cubemaps. Unsupported fields or contexts refuse the entire batch. prepare requires current snapshot; scope defaults to all_visible_references or accepts selected_references. Legacy selected_leading_references is a compatibility spelling with the same arbitrary-subset behavior. Selected scope requires only complete visible root helpers; empty, mixed, hidden or duplicate roots reject before replacing a review. Selection, scope, relevant material policy and original source identity are frozen. Shared ordering metadata preserves effective expansion order around retained references; no unselected root is added or loaded implicitly. apply requires owned token, verifies original parent/child/material/model sources off lock and commits exactly the reviewed plan in one Undo edit. background defaults false; true starts the same owned worker. review transfers an idle shared preview to a fresh token; cancel discards only matching owned work. Materialized multi-solid and empty func_detail owners retain their properties/editor metadata. Manual cubemap and overlay side bindings remap through fresh side IDs; absence remains absence. Runtime entity_id_map records fresh hammerid changes; helper_id_map remains cubemap-only, compiler_helper_id_map covers consumed helpers/detail owners, and static_prop_id_map covers props without runtime hammerids. Unselected roots may receive different build-local IDs on later compilation. Hidden child content, nondefault child world settings and unpreservable metadata still refuse. No child-file writes, silent substitutes or automatic source refresh. UI: Instance reference… → Collapse scope → Preview collapse references… → Apply collapse / Cancel collapse."
    )]
    pub(in crate::mcp) fn set_instance_collapse(
        &self,
        Parameters(args): Parameters<InstanceCollapseArgs>,
    ) -> ToolResult {
        run(|| {
            let (request, background) = match args {
                InstanceCollapseArgs::Prepare {
                    snapshot,
                    scope,
                    background,
                } => (
                    self.session().begin_instance_collapse_scoped(
                        Actor::Agent,
                        &snapshot,
                        scope.unwrap_or_default(),
                    )?,
                    background.unwrap_or(false),
                ),
                InstanceCollapseArgs::Apply { token, background } => (
                    self.session()
                        .apply_instance_collapse(Actor::Agent, token)?,
                    background.unwrap_or(false),
                ),
                InstanceCollapseArgs::Review { token } => {
                    return self.session().review_instance_collapse(Actor::Agent, token);
                }
                InstanceCollapseArgs::Cancel { token } => {
                    return self.session().cancel_instance_collapse(Actor::Agent, token);
                }
            };
            if !background {
                let completed = request.run();
                return self
                    .session()
                    .finish_instance_collapse(Actor::Agent, completed);
            }
            let token = request.token();
            let shared = self.session.clone();
            let spawned = std::thread::Builder::new()
                .name("instance-collapse".into())
                .spawn(self.worker(move || {
                    let completed = request.run();
                    let _ = lock(&shared).finish_instance_collapse(Actor::Agent, completed);
                }));
            if let Err(error) = spawned {
                let _ = self.session().cancel_instance_collapse(Actor::Agent, token);
                return Err(format!("Could not start instance collapse worker: {error}"));
            }
            Ok(json!({"token":token,"background":true}))
        })
    }
}
