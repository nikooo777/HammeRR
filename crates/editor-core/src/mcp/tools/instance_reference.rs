use super::super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
enum Action {
    Begin,
    Update,
    Check,
    File,
    Review,
    Apply,
    Cancel,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InstanceReferenceArgs {
    action: Action,
    token: Option<u64>,
    /// Begin only: existing func_instance; omit to create a new reference.
    id: Option<String>,
    /// Update only: explicit relative .vmf path, at most 512 UTF-8 bytes.
    file: Option<String>,
    /// Update only: authored X Y Z vector, finite native-representable values within ±16384.
    origin: Option<String>,
    /// Update only: authored Source Pitch Yaw Roll vector, finite native-representable values.
    angles: Option<String>,
    /// File only: explicit native-picker path, converted only inside the saved parent's referring tree.
    selected_path: Option<String>,
}

#[tool_router(router = instance_reference_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect the shared owned Instance reference… authoring form. Reports raw file/origin/angles, component strings when valid, owner/token, saved parent directory, validity/errors and metadata-only file-presence status. Opening and editing this form never loads child bytes or initializes graphics. contents_validation is always not_checked: explicit Refresh instance contents or Compile checks child syntax, source keys, aggregate budgets and transformed geometry; only Compile checks child brush-model materials and native validity. No child editing or automatic refresh.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_instance_reference(&self) -> ToolResult {
        ok(self.session().get_instance_reference())
    }

    #[tool(
        description = "Share Instance reference… / Edit instance reference… with the human. action=begin accepts only optional id (omit for Create). update requires owned token and optional file/origin/angles strings, retaining invalid raw input for correction without source edits. check performs bounded relative-file presence/metadata checks only. file accepts owned token and selected_path, converts an explicit picker result under the saved referring parent, and checks presence without reading bytes. apply creates or edits in one Undo step after helper/source/path checks; preserves unrelated imported fields and never substitutes child paths or auto-refreshes. cancel clears only an owned matching form, even stale. review transfers only this form to a new token, preserving stale state. Every non-begin action requires token; irrelevant fields reject. Save the parent first; source/definitions/revision changes stale the form. Refresh committed contents separately with set_instance_contents."
    )]
    pub(in crate::mcp) fn set_instance_reference(
        &self,
        Parameters(args): Parameters<InstanceReferenceArgs>,
    ) -> ToolResult {
        run(|| {
            let changes = args.file.is_some() || args.origin.is_some() || args.angles.is_some();
            if matches!(args.action, Action::Begin) {
                if args.token.is_some() || changes || args.selected_path.is_some() {
                    return Err("Begin accepts only action and optional id".into());
                }
                let id = args
                    .id
                    .as_deref()
                    .map(|id| -> Result<u32, String> {
                        match parse_id(id)? {
                            ObjectId::Entity(id) => Ok(id),
                            _ => Err("Select an existing func_instance entity".into()),
                        }
                    })
                    .transpose()?;
                return self.session().begin_instance_reference(Actor::Agent, id);
            }
            if args.id.is_some() {
                return Err("id is accepted only for Begin".into());
            }
            let token = args
                .token
                .ok_or("Supply the current instance reference token")?;
            if !matches!(args.action, Action::Update) && changes {
                return Err("file/origin/angles are accepted only for Update".into());
            }
            if !matches!(args.action, Action::File) && args.selected_path.is_some() {
                return Err("selected_path is accepted only for File".into());
            }
            let mut session = self.session();
            match args.action {
                Action::Update => {
                    if !changes {
                        return Err("Update requires file, origin or angles".into());
                    }
                    session.update_instance_reference(
                        Actor::Agent,
                        token,
                        crate::session::instance_reference::Changes {
                            file: args.file,
                            origin: args.origin,
                            angles: args.angles,
                        },
                    )
                }
                Action::Check => session.check_instance_reference(Actor::Agent, token),
                Action::File => session.instance_reference_file(
                    Actor::Agent,
                    token,
                    Path::new(
                        args.selected_path
                            .as_deref()
                            .ok_or("File requires selected_path")?,
                    ),
                ),
                Action::Review => session.review_instance_reference(Actor::Agent, token),
                Action::Apply => session.apply_instance_reference(Actor::Agent, token),
                Action::Cancel => session.cancel_instance_reference(Actor::Agent, token),
                Action::Begin => unreachable!(),
            }
        })
    }
}

#[cfg(test)]
mod tests;
