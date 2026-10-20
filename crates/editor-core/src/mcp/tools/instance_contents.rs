use super::super::*;

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InspectInstanceContentsArgs {
    offset: Option<usize>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InstanceContentsAction {
    Refresh,
    Cancel,
    Clear,
    Frame,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InstanceContentsArgs {
    action: InstanceContentsAction,
    token: Option<String>,
    ids: Option<Vec<String>>,
    aspect: Option<f32>,
    background: Option<bool>,
}

#[tool_router(router = instance_contents_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect the explicitly refreshed nested func_instance world/detail reference layer through four helper levels without loading child bytes or initializing graphics. Reports disabled/empty/not_loaded/loading/ready/stale/cancelled/failed/context_suppressed, requested/displayed availability, owner-only bounds/transforms, root-relative references, root owner versus leaf source provenance, saved-parent source and transitive child identities, bounded diagnostics and CPU/GPU limits. Owners paginate with offset and limit 1–64. Defaults off; toggling set_view.instance_contents only displays a current snapshot and never loads files. Refresh explicitly with set_instance_contents. No child editing or root face/placement identities.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn inspect_instance_contents(
        &self,
        Parameters(args): Parameters<InspectInstanceContentsArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .inspect_instance_contents(args.offset.unwrap_or(0), args.limit.unwrap_or(64))
        })
    }

    #[tool(
        description = "Manage the same read-only reference layer as Instance contents… in the viewport. action=refresh freezes the in-memory parent and resolves bounded relative .vmf world/func_detail children outside the Session lock; background=true returns the request token immediately, default false waits. Refresh evicts the previous effective layer; any missing/unsupported/stale dependency fails the whole request. action=cancel/clear/frame requires the current inspection token. Cancel requests worker cancellation; Clear frees the ready layer. Frame accepts root instance ids (omitted uses selection, empty selection frames all) and positive finite aspect; it changes only the camera and preserves it on failure. Refresh/cancel/clear do not change the checkbox. Nested references support four helper levels with root-relative filenames and leaf source provenance. No automatic loading, BSP build, child edits, runtime entities, material fidelity or native runtime claim. Visibility/picking are centrally suppressed outside ordinary Objects mode or during authoring tools/drafts; source changes require another explicit Refresh."
    )]
    pub(in crate::mcp) fn set_instance_contents(
        &self,
        Parameters(args): Parameters<InstanceContentsArgs>,
    ) -> ToolResult {
        run(|| match args.action {
            InstanceContentsAction::Refresh => {
                if args.token.is_some() || args.ids.is_some() || args.aspect.is_some() {
                    return Err("Refresh accepts only action and optional background".into());
                }
                let request = self
                    .session()
                    .begin_instance_contents_refresh(Actor::Agent)?;
                let token = request.token();
                if args.background.unwrap_or(false) {
                    let shared = self.session.clone();
                    let spawned = std::thread::Builder::new()
                        .name("instance-contents".into())
                        .spawn(self.worker(move || {
                            let result = request.run();
                            let _ = lock(&shared)
                                .finish_instance_contents_refresh(Actor::Agent, result);
                        }));
                    if let Err(error) = spawned {
                        return self.session().fail_instance_contents_refresh(
                            Actor::Agent,
                            token,
                            format!("Could not start instance refresh: {error}"),
                        );
                    }
                    Ok(json!({"token":token.to_string(),"background":true}))
                } else {
                    let result = request.run();
                    self.session()
                        .finish_instance_contents_refresh(Actor::Agent, result)
                }
            }
            action => {
                if args.background.is_some() {
                    return Err("background is accepted only for Refresh".into());
                }
                let token = parse_token(
                    args.token
                        .as_deref()
                        .ok_or("Supply the current instance contents token")?,
                )?;
                let mut session = self.session();
                match action {
                    InstanceContentsAction::Cancel | InstanceContentsAction::Clear
                        if args.ids.is_some() || args.aspect.is_some() =>
                    {
                        Err("Cancel and Clear accept only action and token".into())
                    }
                    InstanceContentsAction::Cancel => {
                        session.cancel_instance_contents_refresh(Actor::Agent, token)
                    }
                    InstanceContentsAction::Clear => {
                        session.clear_instance_contents(Actor::Agent, token)
                    }
                    InstanceContentsAction::Frame => {
                        let ids = ids_or_selection(&session, args.ids.as_deref())?;
                        session.frame_instance_contents(
                            Actor::Agent,
                            token,
                            &ids,
                            args.aspect.unwrap_or(1.0),
                        )
                    }
                    InstanceContentsAction::Refresh => unreachable!(),
                }
            }
        })
    }
}

#[cfg(test)]
mod tests;
