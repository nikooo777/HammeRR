use super::super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NavigationArgs {
    fly_speed: f32,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NavigationDefaultsAction {
    Save,
    Apply,
    Reload,
    Reset,
}

impl NavigationDefaultsAction {
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
pub struct NavigationDefaultsArgs {
    action: NavigationDefaultsAction,
    snapshot: String,
}

#[tool_router(router = navigation_preferences_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect shared fly speed and navigation startup defaults. Returns current/saved/factory fly_speed in Hammer units per second, range 25..20000 (factory 600), Shift multiplier 4, storage errors and an action snapshot. Same Session state as the visible Fly speed input and perspective right-button wheel. Camera positions, look sensitivity, planar zoom and dolly sensitivity are separate and unchanged. No graphics or asset reads.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_navigation_settings(&self) -> ToolResult {
        ok(self.session().navigation_preferences_state())
    }

    #[tool(
        description = "Set the shared temporary fly_speed, a finite 25..20000 Hammer units per second. Same Session operation as the Fly speed input; right-button wheel scales this same value. Does not save automatically, move the camera, edit the map/history or invalidate active owned previews/picks. Use set_navigation_defaults to explicitly persist startup speed. Shift retains its 4x movement multiplier."
    )]
    pub(in crate::mcp) fn set_navigation_settings(
        &self,
        Parameters(args): Parameters<NavigationArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.set_fly_speed(args.fly_speed)?;
            Ok(session.navigation_preferences_state())
        })
    }

    #[tool(
        description = "Manage navigation defaults using the latest get_navigation_settings.snapshot, shared with Navigation defaults…. save writes current fly_speed; apply restores saved (or factory if no file) speed into this session; reload reads storage without applying; reset writes factory speed 600 without applying. Only save/reset write bounded atomic navigation-defaults.json. Save/Apply/Reset recheck inspected file identity and bytes; outside writes reject stale actions. Malformed/unsupported files remain intact until explicit Reset. No action changes camera/selection/source/history or cancels active owned map previews/picks. No navigation sensitivity or camera positions are stored. write_committed=true means the write completed even if later verification failed; inspect ready/error. Save/Reset coordinate participating Linux writers through a nonblocking lock and retain the original directory binding. Changed directories require Reload; programs ignoring the lock remain outside that guarantee. UI labels: Save current as defaults / Apply saved defaults / Reload defaults / Reset saved defaults."
    )]
    pub(in crate::mcp) fn set_navigation_defaults(
        &self,
        Parameters(args): Parameters<NavigationDefaultsArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().change_navigation_preferences(
                Actor::Agent,
                args.action.name(),
                &args.snapshot,
            )
        })
    }
}
