use super::super::*;

#[tool_router(router = content_refresh_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect the current-content refresh and optional native watch shared with Content refresh…. Reports current content intent, pending changes, watch coverage/errors, reload readiness/blockers, active cancellation token, refresh errors and whether previous content was retained. Does not reload or change map data.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_content_refresh(
        &self,
        Parameters(_args): Parameters<ContentRefreshInspectArgs>,
    ) -> ToolResult {
        ok(self.session().content_refresh_report())
    }

    #[tool(
        description = "Enable or disable native watching of the current mounted content and effective FGD sources. Changes become pending until explicit reload_content; no automatic content replacement or polling fallback. Shared Session operation reports native watch failures and incomplete coverage explicitly. Does not edit map data or save a content profile."
    )]
    pub(in crate::mcp) fn set_content_watch(
        &self,
        Parameters(args): Parameters<ContentWatchArgs>,
    ) -> ToolResult {
        run(|| self.session().set_content_watch(Actor::Agent, args.enabled))
    }

    #[tool(
        description = "Explicitly reload the current content configuration using get_content_refresh.intent. Prepares mounts and FGD includes outside the Session lock, then applies through shared transition/source guards and invalidates dependent asset caches. Failed, cancelled or stale preparation retains previous content with an explicit error. Preserves map data, history, selection, cameras and requested build settings. Running managed games/builds and conflicting work retain existing refusal guards. Does not reload a running game's process-owned caches or save a profile; use cancel_content_refresh with the pending token to request cancellation."
    )]
    pub(in crate::mcp) async fn reload_content(
        &self,
        Parameters(args): Parameters<ReloadContentArgs>,
    ) -> ToolResult {
        let request = match self.session().begin_content_refresh(Some(args.intent)) {
            Ok(request) => request,
            Err(error) => return fail(error),
        };
        let token = request.token();
        let server = self.clone();
        match tokio::task::spawn_blocking(self.worker(move || {
            let completed = request.run();
            server
                .session()
                .finish_content_refresh(Actor::Agent, completed)
        }))
        .await
        {
            Ok(Ok(value)) => ok(value),
            Ok(Err(error)) => fail(error),
            Err(error) => {
                let message = format!("Content refresh worker failed: {error}");
                self.session().fail_content_refresh(token, message.clone());
                fail(message)
            }
        }
    }

    #[tool(
        description = "Request cancellation of the exact pending content reload using get_content_refresh.pending.token. Uses the same Session operation as Cancel or closing Content refresh…. Preparation retires without applying its candidate, retaining previous content; inspect get_content_refresh for completion. Does not disable the optional watcher."
    )]
    pub(in crate::mcp) fn cancel_content_refresh(
        &self,
        Parameters(args): Parameters<CancelContentRefreshArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .cancel_content_refresh(Actor::Agent, parse_token(&args.token)?)
        })
    }
}
