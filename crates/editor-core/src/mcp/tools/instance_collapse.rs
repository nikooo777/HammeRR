use super::super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(extend("type" = "object"))]
pub enum InstanceCollapseArgs {
    Prepare {
        snapshot: String,
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
        description = "Inspect the owned Collapse all references preview, snapshot, token, source/dependency staleness, counts, fresh object IDs, the complete expanded-to-authored runtime entity ID map and bounded original leaf face/entity provenance. Scope is every visible root func_instance recursively; hidden root references and source child files stay intact. No geometry or history edit.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_instance_collapse(&self) -> ToolResult {
        ok(self.session().get_instance_collapse())
    }

    #[tool(
        description = "Preview or apply collapse of ALL visible instance references to editable world/ordinary func_detail brushes, checked terrain and static point/spot lights with checked local info_target helpers through the compiler's exact source resolver and composed transforms. prepare requires current snapshot; apply requires owned token and rechecks original parent/dependency bytes off lock before one atomic Undo edit. background defaults false; true starts the same owned worker. review transfers an idle shared preview to a fresh token; cancel discards only owned matching work. Other child entities, hidden child content, nondefault child world settings and metadata that cannot be preserved reject the whole batch. Standard child version/view settings remain child-local; parent settings stay authoritative. Fresh entity IDs intentionally change only the corresponding compiled hammerid values; light values, scoped targets and entity order retain shared preparation. No selected-only mode, writes to child files, general runtime-entity conversion, substitutes or automatic source refresh. UI: Instance reference… → Preview collapse all references… → Apply collapse / Cancel collapse."
    )]
    pub(in crate::mcp) fn set_instance_collapse(
        &self,
        Parameters(args): Parameters<InstanceCollapseArgs>,
    ) -> ToolResult {
        run(|| {
            let (request, background) = match args {
                InstanceCollapseArgs::Prepare {
                    snapshot,
                    background,
                } => (
                    self.session()
                        .begin_instance_collapse(Actor::Agent, &snapshot)?,
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

#[cfg(test)]
mod tests;
