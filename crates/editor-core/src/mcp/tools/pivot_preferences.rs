use super::super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PivotDefaultsAction {
    Save,
    Apply,
    Reload,
    Reset,
}

impl PivotDefaultsAction {
    fn name(&self) -> &'static str {
        match self {
            Self::Save => "save",
            Self::Apply => "apply",
            Self::Reload => "reload",
            Self::Reset => "reset",
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PivotDefaultsArgs {
    action: PivotDefaultsAction,
    snapshot: String,
}

#[tool_router(router=pivot_preferences_tools,vis="pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect current document transform pivot, saved startup pivot, factory selection-center default, storage path, readiness/errors, finite coordinate requirement/file bound and an action snapshot. Shared with Transform pivot defaults… in the UI. Modes are selection_center, world_origin, or custom with a fixed world point. Ordinary pivot edits and surface picks never persist automatically. Malformed/unsupported files remain preserved and block Save/Apply until Reload succeeds or explicit Reset repairs them.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_transform_pivot_defaults(&self) -> ToolResult {
        ok(self.session().pivot_preferences_state())
    }

    #[tool(
        description = "Manage transform pivot defaults with the latest get_transform_pivot_defaults.snapshot. save persists only this document's current pivot; apply changes only its pivot while preserving transform mode, axes, texture policies and angle snap; reload retries storage without applying; reset explicitly writes factory selection-center defaults without applying. New document Sessions apply ready saved defaults once at startup; New/Open within the same Session retain the current pivot. Apply rejects active tools/previews/builds/launches. Save/Apply/Reset recheck the inspected source/directory; external changes require Reload. Only save/reset write, through coordinated atomic publication. write_committed=true with ready=false means publication completed but verification failed; inspect error. Fixed custom coordinates must be finite f64 values; individual transform operations retain their existing geometry/deformation limits. Surface attachment, picked-target provenance, selection, cameras, map edits and Undo history are never saved. UI: Transform pivot defaults… → Save current as defaults / Apply saved defaults / Reload defaults / Reset saved defaults."
    )]
    pub(in crate::mcp) fn set_transform_pivot_defaults(
        &self,
        Parameters(args): Parameters<PivotDefaultsArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().change_pivot_preferences(
                Actor::Agent,
                args.action.name(),
                &args.snapshot,
            )
        })
    }
}
