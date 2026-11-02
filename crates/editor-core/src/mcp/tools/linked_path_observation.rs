use super::super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct ObservationQuery {
    #[serde(default)]
    include_samples: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct ObservationStart {
    train: String,
    snapshot: String,
    #[serde(default)]
    stationary: bool,
    sample_interval_ms: Option<u16>,
    samples_per_phase: Option<u16>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct ObservationCancel {
    token: String,
}

#[tool_router(router = linked_path_observation_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect native train movement observation readiness, snapshot, running token, explicit errors and last receipt. include_samples=true includes at most256 actual absolute-origin/game-time samples; omitted false returns the retained sample count. A historical receipt is marked current:false. Does not start a game or change the map.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_linked_path_observation(
        &self,
        Parameters(args): Parameters<ObservationQuery>,
    ) -> ToolResult {
        run(|| Ok(self.session().linked_path_observation(args.include_samples)))
    }

    #[tool(
        description = "Start a bounded private CS:S movement observation through the shared Session operation. Requires train entity ID, latest get_linked_path_observation.snapshot and a current saved successful BSP. Linux/Steam and the verified native server build are required. stationary:true observes Stop only; false applies Stop/Start/Stop/Start (StartForward for tracktrains). Existing authored BSP/entity identity is retained. Defaults:200ms interval,8 samples per phase; intervals100–1000ms,3–64 samples per phase,total<=256 and requested duration<=60s. Returns accepted running status/token, not a movement result. Poll get_linked_path_observation; any native/script/source/cleanup failure remains explicit. Uses -insecure and sv_lan1 in an isolated game; no desktop input or map/history mutation."
    )]
    pub(in crate::mcp) fn start_linked_path_observation(
        &self,
        Parameters(args): Parameters<ObservationStart>,
    ) -> ToolResult {
        run(|| {
            let ObjectId::Entity(train) = parse_id(&args.train)? else {
                return Err("Native observation needs an entity:<id> train".into());
            };
            self.document_access().start_linked_path_observation(
                Actor::Agent,
                crate::session::linked_path_observation::Options {
                    train,
                    snapshot: args.snapshot,
                    stationary: args.stationary,
                    sample_interval_ms: args.sample_interval_ms.unwrap_or(200),
                    samples_per_phase: args.samples_per_phase.unwrap_or(8),
                },
            )
        })
    }

    #[tool(
        description = "Cancel the matching native movement observation token and retain its explicit completion/cleanup outcome. Stale or foreign tokens reject; source map, selection, history and camera stay unchanged."
    )]
    pub(in crate::mcp) fn cancel_linked_path_observation(
        &self,
        Parameters(args): Parameters<ObservationCancel>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .cancel_linked_path_observation(Actor::Agent, &args.token)
        })
    }
}
