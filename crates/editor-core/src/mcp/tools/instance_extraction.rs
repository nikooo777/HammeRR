use super::super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum Action {
    Prepare,
    Extract,
    Review,
    Cancel,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InstanceExtractionArgs {
    action: Action,
    token: Option<u64>,
    /// Prepare only: a new explicit relative .vmf under an existing parent directory.
    file: Option<String>,
    /// Prepare/Extract only; default false waits for bounded off-lock validation.
    background: Option<bool>,
}

#[tool_router(router = instance_extraction_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect the owned Extract selection to instance… review, source/selection staleness, final relative path, exact serialized/compiler-resolved validation, world/detail/face counts and child byte hash. No reads, graphics or edits beyond cheap identity metadata checks. Undo restores only the parent; an exported child remains a file artifact.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_instance_extraction(&self) -> ToolResult {
        ok(self.session().get_instance_extraction())
    }

    #[tool(
        description = "Extract selected visible world-brush suffix and whole ordinary func_detail suffix into one new relative VMF, then replace them with an identity/world-origin func_instance in exactly one Undo step. prepare accepts only file and optional background; freezes source/selection and validates private serialized child through the compiler off-lock, without publishing. extract requires owned token and optional background; exact rehashes off-lock then new-file-only publication follows all fallible parent cleanup. review transfers a ready/failed review to a fresh token. cancel removes only owned staging. Rejects existing visible references, groups/layers/notes/partitions/inbound side bindings, terrain/runtime/nested content and unsupported metadata rather than stripping it. No overwrite, directory creation, auto-refresh, game launch or child editing. Undo/Redo only change the parent; the child persists. Stale/cancel/error before publication preserves parent/history and existing files; filesystem races retain standard no-clobber guard limits."
    )]
    pub(in crate::mcp) fn set_instance_extraction(
        &self,
        Parameters(args): Parameters<InstanceExtractionArgs>,
    ) -> ToolResult {
        run(|| {
            let request = match args.action {
                Action::Prepare => {
                    if args.token.is_some() {
                        return Err("Prepare accepts only file and optional background".into());
                    }
                    self.session().begin_instance_extraction(
                        Actor::Agent,
                        args.file.as_deref().ok_or("Prepare requires file")?,
                    )?
                }
                Action::Extract => {
                    if args.file.is_some() {
                        return Err("file is accepted only for Prepare".into());
                    }
                    self.session().publish_instance_extraction(
                        Actor::Agent,
                        args.token.ok_or("Extract requires token")?,
                    )?
                }
                action => {
                    if args.file.is_some() || args.background.is_some() {
                        return Err("Review/Cancel accept only token".into());
                    }
                    let token = args.token.ok_or("Supply the current extraction token")?;
                    return match action {
                        Action::Review => self
                            .session()
                            .review_instance_extraction(Actor::Agent, token),
                        Action::Cancel => self
                            .session()
                            .cancel_instance_extraction(Actor::Agent, token),
                        _ => unreachable!(),
                    };
                }
            };
            if args.background.unwrap_or(false) {
                let token = request.token();
                let shared = self.session.clone();
                if let Err(error) = std::thread::Builder::new()
                    .name("instance-extraction".into())
                    .spawn(move || {
                        let completed = request.run();
                        let _ = lock(&shared).finish_instance_extraction(Actor::Agent, completed);
                    })
                {
                    let _ = self
                        .session()
                        .cancel_instance_extraction(Actor::Agent, token);
                    return Err(format!("Could not start extraction worker: {error}"));
                }
                Ok(json!({"token":token,"background":true}))
            } else {
                let completed = request.run();
                self.session()
                    .finish_instance_extraction(Actor::Agent, completed)
            }
        })
    }
}

#[cfg(test)]
mod tests;
