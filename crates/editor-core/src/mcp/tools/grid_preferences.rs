use super::super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GridDefaultsAction {
    Save,
    Apply,
    Reload,
    Reset,
}

impl GridDefaultsAction {
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
pub struct GridDefaultsArgs {
    action: GridDefaultsAction,
    snapshot: String,
}

#[tool_router(router=grid_preferences_tools,vis="pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect current grid size/snapping/visibility, saved user defaults, factory defaults, storage path, load/write errors, file bounds and an action snapshot. Grid defaults are explicit user preferences shared with Grid defaults… in the UI. Temporary set_spatial_settings calls never persist automatically. Malformed/unsupported files are preserved and block Save/Apply until Reload succeeds or Reset is explicitly requested.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_grid_defaults(&self) -> ToolResult {
        ok(self.session().grid_preferences_state())
    }

    #[tool(
        description = "Manage grid defaults with the latest get_grid_defaults.snapshot. save explicitly persists the current grid size, snap and grid_visible values; apply loads saved defaults into this session without writing; reload retries storage without changing the current grid; reset explicitly replaces saved defaults with factory values (16 units, snapping/grid visible) without applying them. Save/Apply/Reset recheck the bounded inspected file identity and bytes; external changes reject stale actions. Apply preserves workplane elevation, brush depth and draw-tool state and rejects active previews/builds. No action edits the map or Undo history. Only save/reset write, using atomic storage with visible failures and no session-only fallback. write_committed=true reports a completed write even if its later reload failed; inspect ready/error. Independent writers do not lock the file. UI: Grid defaults… → Save current as defaults / Apply saved defaults / Reload defaults / Reset saved defaults."
    )]
    pub(in crate::mcp) fn set_grid_defaults(
        &self,
        Parameters(args): Parameters<GridDefaultsArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .change_grid_preferences(Actor::Agent, args.action.name(), &args.snapshot)
        })
    }
}

#[cfg(test)]
mod tests;
