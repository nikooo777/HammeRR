use super::super::*;

#[tool_router(router = content_profiles_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect named content profiles, current effective content paths and build settings, active profile and whether it still matches, shared revision, current intent, source/persistence errors and Apply readiness. Shared with Content profiles…. Does not reload storage or apply a profile.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_content_profiles(
        &self,
        Parameters(_args): Parameters<ContentProfilesInspectArgs>,
    ) -> ToolResult {
        ok(self.session().content_profiles_snapshot())
    }

    #[tool(
        description = "Save a named content profile using get_content_profiles.revision. Omit config to capture current content paths and build settings, or supply gameinfo, optional fgd/engine_root/steam_root and build settings. Omit replace_id to create; supply a saved profile ID to replace it. Shared Session validation and persistence errors refuse the save. Does not apply the profile or launch a game."
    )]
    pub(in crate::mcp) async fn save_content_profile(
        &self,
        Parameters(args): Parameters<SaveContentProfileArgs>,
    ) -> ToolResult {
        let request = match self.session().begin_content_profile_save(
            &args.name,
            args.config,
            args.replace_id.as_deref(),
            Some(args.revision),
        ) {
            Ok(request) => request,
            Err(error) => return fail(error),
        };
        let token = request.token();
        let server = self.clone();
        match tokio::task::spawn_blocking(self.worker(move || {
            let completed = request.run();
            server
                .session()
                .finish_content_profile(Actor::Agent, completed)
        }))
        .await
        {
            Ok(Ok(value)) => ok(value),
            Ok(Err(error)) => fail(error),
            Err(error) => {
                let message = format!("Content profile save worker failed: {error}");
                self.session().fail_content_profile(token, message.clone());
                fail(message)
            }
        }
    }

    #[tool(
        description = "Apply a saved content profile to the selected document using get_content_profiles.revision and intent. Shared Session validation prepares mounts, entity definitions, runtime paths and build settings before replacing current content. Changed revisions, invalid sources or conflicting work refuse while preserving the previous content. Does not launch a game or edit map data."
    )]
    pub(in crate::mcp) async fn apply_content_profile(
        &self,
        Parameters(args): Parameters<ApplyContentProfileArgs>,
    ) -> ToolResult {
        let request = match self.session().begin_content_profile_apply(
            &args.id,
            Some(args.revision),
            Some(args.intent),
        ) {
            Ok(request) => request,
            Err(error) => return fail(error),
        };
        let token = request.token();
        let server = self.clone();
        match tokio::task::spawn_blocking(self.worker(move || {
            let completed = request.run();
            server
                .session()
                .finish_content_profile(Actor::Agent, completed)
        }))
        .await
        {
            Ok(Ok(value)) => ok(value),
            Ok(Err(error)) => fail(error),
            Err(error) => {
                let message = format!("Content profile apply worker failed: {error}");
                self.session().fail_content_profile(token, message.clone());
                fail(message)
            }
        }
    }

    #[tool(
        description = "Delete a saved content profile using get_content_profiles.revision. Changes the saved list through the shared Session operation; current content paths and build settings remain applied. Stale revisions and persistence/source errors refuse explicitly."
    )]
    pub(in crate::mcp) fn remove_content_profile(
        &self,
        Parameters(args): Parameters<RemoveContentProfileArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .remove_content_profile(Actor::Agent, &args.id, Some(args.revision))
        })
    }

    #[tool(
        description = "Explicitly reload saved content profiles using get_content_profiles.revision. Reports storage, source and validation errors through the shared Session operation. Does not apply a profile or change current content paths and build settings."
    )]
    pub(in crate::mcp) fn reload_content_profiles(
        &self,
        Parameters(args): Parameters<ReloadContentProfilesArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .reload_content_profiles(Actor::Agent, Some(args.revision))
        })
    }
}
