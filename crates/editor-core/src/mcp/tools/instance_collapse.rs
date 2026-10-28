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
        description = "Inspect the owned instance collapse preview, snapshot, token, source/dependency staleness, counts, fresh object IDs, complete expanded-to-authored emitted-entity, cubemap-helper and static-prop ID maps for the materialized roots and bounded original leaf face/entity provenance. visible_root_order lists up to 256 visible root IDs in actual parent entity order. Scope is all_visible_references by default, or selected_leading_references: only the frozen selection, a nonempty prefix in visible root-reference order, recursively. Unselected and hidden root references and source child files stay intact. No geometry or history edit.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_instance_collapse(&self) -> ToolResult {
        ok(self.session().get_instance_collapse())
    }

    #[tool(
        description = "Preview or apply collapse of all visible instance references or the selected leading references to editable world/ordinary func_detail brushes, checked terrain, supported prop_static entities with empty lightingorigin and static point/spot lights with checked local info_target helpers and cubemap samples through the compiler's exact source resolver and composed transforms. prepare requires current snapshot; optional scope is all_visible_references (default) or selected_leading_references. Selected scope requires only complete visible root func_instance entities forming a nonempty prefix in actual parent visible root-reference order; empty, mixed, hidden or nonprefix selection rejects before replacing the draft. Selection and scope are frozen; no unselected root is added implicitly; apply requires owned token and rechecks original parent/dependency bytes off lock before one atomic Undo edit. background defaults false; true starts the same owned worker. review transfers an idle shared preview to a fresh token; cancel discards only owned matching work. Other child entities, hidden child content, nondefault child world settings and metadata that cannot be preserved reject the whole batch. Standard child version/view settings remain child-local; parent settings stay authoritative. Manual cubemap sides remap through the reviewed fresh side IDs; automatic side-field absence remains intact. Cubemap helpers use helper_id_map; static props use static_prop_id_map. Both have emitted:false and compiler_handled:true; neither gains a runtime hammerid. Static-prop source and intermediate model bounds use the shared compiler check; original model-header sources are retained and rechecked before Apply. Fresh materialized entity IDs intentionally change their compiled hammerid values. Retained roots remain authored unchanged but their later compiler expansion can receive different build-local IDs/hammerids. Light values, scoped targets and entity order retain shared preparation. No arbitrary-subset mode, writes to child files, general runtime-entity conversion, substitutes or automatic source refresh. UI: Instance reference… → Collapse scope → Preview collapse references… → Apply collapse / Cancel collapse."
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
