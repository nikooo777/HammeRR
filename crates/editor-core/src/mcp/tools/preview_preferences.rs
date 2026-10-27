use super::super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PreviewDefaultsAction {
    Save,
    Apply,
    Reload,
    Reset,
}

impl PreviewDefaultsAction {
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
pub struct PreviewDefaultsArgs {
    action: PreviewDefaultsAction,
    snapshot: String,
}

#[tool_router(router = preview_preferences_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect current and saved startup defaults for selection_outlines, selection_tint, entity_images, entity_model_guides, rope_preview and sky_preview. Shared with Preview defaults… in the UI. Returns the six exact booleans, factory defaults, storage path, bounded file limit, load/write errors and an action snapshot. Ordinary set_view and temporary render_view calls never persist automatically. No renderer or asset reads. Malformed/unsupported files are preserved and block Save/Apply until Reload succeeds or Reset is explicitly requested.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_preview_defaults(&self) -> ToolResult {
        ok(self.session().preview_preferences_state())
    }

    #[tool(
        description = "Manage the six viewport preview/guide startup booleans using the latest get_preview_defaults.snapshot and the same Session operations as Preview defaults…. save explicitly persists the current booleans; apply restores saved defaults into the current session without writing; reload reads storage without applying; reset explicitly writes factory booleans without applying them. Only save/reset write, using bounded atomic storage with visible failures. Save/Apply/Reset recheck inspected file identity and bytes; outside changes reject stale actions. Apply preserves other view settings, cameras, selection, map and Undo history and rejects active geometry/measurement previews, viewport tools, builds and launches. Save/Reload/Reset do not change the view or invalidate viewport picking. Sky channel/exposure, fog, rendering mode, tool filters, grids and active tools are not stored. write_committed=true reports a completed write even if later verification failed; inspect ready/error. Save/Reset coordinate participating Linux writers through a nonblocking lock and retain the original directory binding. Changed directories require Reload; programs ignoring the lock remain outside that guarantee. UI actions: Save current as defaults / Apply saved defaults / Reload defaults / Reset saved defaults."
    )]
    pub(in crate::mcp) fn set_preview_defaults(
        &self,
        Parameters(args): Parameters<PreviewDefaultsArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().change_preview_preferences(
                Actor::Agent,
                args.action.name(),
                &args.snapshot,
            )
        })
    }
}

#[cfg(test)]
mod tests;
