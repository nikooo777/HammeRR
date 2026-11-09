use super::super::*;

fn parse_player_support_token(token: &str) -> Result<u64, String> {
    if token.is_empty() || token.len() > 20 || !token.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("Player support token must be a bounded decimal integer".into());
    }
    token
        .parse()
        .map_err(|_| "Invalid player support observation token".into())
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct StartPlayerSupportObservation {
    snapshot: String,
    duration_ms: Option<u64>,
    interval_ms: Option<u64>,
    max_samples: Option<usize>,
    team: Option<u8>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct InspectPlayerSupportObservation {
    token: String,
    offset: Option<usize>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct PlayerSupportToken {
    token: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct FramePlayerSupportSample {
    token: String,
    index: usize,
    aspect: f32,
}

#[tool_router(router = player_support_observation_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect readiness, source snapshot, token, worker, native activity/cleanup state and receipt for passive player support observation. Existing samples may be historical and report current:false with a reason. Does not launch a game. UI: Build → Player support….",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_player_support_observation(&self) -> ToolResult {
        run(|| Ok(self.session().player_support_observation()))
    }

    #[tool(
        description = "Start a bounded passive player support observation through the shared Session worker using the latest get_player_support_observation.snapshot and the exact current saved retained compiled BSP. Fresh private owned supported Linux CS:S listen client uses -insecure and sv_lan1. Returns accepted status/token; poll get_player_support_observation. duration_ms defaults5000, range100–30000; interval_ms defaults50, range15–1000; max_samples defaults128, range3–1024; Duration must span at least two intervals; ceil(duration_ms/interval_ms)+1 must fit max_samples. Duration and interval use native simulation time. team defaults3(Counter-Terrorist), accepts2(Terrorist) or3; the exact BSP must contain a matching team spawn and the engine chooses the actual spawn. Captures one actual freshly spawned idle player under native publication guards: identity, origin, velocities, native movement hull/collision bounds, tick/time, serial-resolved ground and checked supporting surface name. Airborne material fields are explicitly cached previous state. Standing, natural falling and sliding qualify only that observed route. No input, swept queries, contact plane/triangle or active seam crossing. Admission failures preserve prior rows; accepted attempts retire them. Results publish only after resume and confirmed supervisor cleanup/preservation. UI: Player support… → Start."
    )]
    pub(in crate::mcp) fn start_player_support_observation(
        &self,
        Parameters(args): Parameters<StartPlayerSupportObservation>,
    ) -> ToolResult {
        run(|| {
            let defaults = crate::session::player_support_observation::Options::default();
            let options = crate::session::player_support_observation::Options {
                duration_ms: args.duration_ms.unwrap_or(defaults.duration_ms),
                interval_ms: args.interval_ms.unwrap_or(defaults.interval_ms),
                max_samples: args.max_samples.unwrap_or(defaults.max_samples),
                team: args.team.unwrap_or(defaults.team),
            };
            self.document_access().start_player_support_observation(
                Actor::Agent,
                args.snapshot,
                options,
            )
        })
    }

    #[tool(
        description = "Page the matching retained passive player support observation. token required; offset defaults0 and limit64, range1–64, response<=256KiB. Rows retain actual player identity, origin/velocities, collision and movement hull bounds, flags/duck state, health/team/movement mode/buttons, native tick/time, serial-resolved ground entity and checked native surface names. surface_is_cached marks previous airborne material state; it does not prove current support. Ordinal is the global sample index for framing. Historical samples remain inspectable with current:false; framing is separately guarded. No authored face or native contact triangle identity is inferred. UI: Player support… → Previous / Next.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn inspect_player_support_observation(
        &self,
        Parameters(args): Parameters<InspectPlayerSupportObservation>,
    ) -> ToolResult {
        run(|| {
            self.session().inspect_player_support_observation(
                crate::session::player_support_observation::Query {
                    token: parse_player_support_token(&args.token)?,
                    offset: args.offset.unwrap_or(0),
                    limit: args.limit.unwrap_or(64),
                },
            )
        })
    }

    #[tool(
        description = "Request cancellation of the running matching passive player support observation. Exact owned token remains accepted after source staleness. Cleanup continues to own the native activity reservation until its worker drains; cancellation never publishes previous successful rows. UI: Player support… → Cancel."
    )]
    pub(in crate::mcp) fn cancel_player_support_observation(
        &self,
        Parameters(args): Parameters<PlayerSupportToken>,
    ) -> ToolResult {
        run(|| {
            self.session().cancel_player_support_observation(
                Actor::Agent,
                parse_player_support_token(&args.token)?,
            )
        })
    }

    #[tool(
        description = "Clear the matching passive player support observation and release retained rows. Cancels any pending worker and prevents late publication; native activity remains occupied until confirmed cleanup/drain. Exact token may be historical. Stale or foreign tokens preserve state. UI: Player support… → Clear."
    )]
    pub(in crate::mcp) fn clear_player_support_observation(
        &self,
        Parameters(args): Parameters<PlayerSupportToken>,
    ) -> ToolResult {
        run(|| {
            self.session().clear_player_support_observation(
                Actor::Agent,
                parse_player_support_token(&args.token)?,
            )
        })
    }

    #[tool(
        description = "Frame one retained actual native player movement hull at its observed origin using the shared Session camera operation. Requires the current token/build/runtime, global sample index and finite aspect0.01–100. Rejects historical rows, changed source/output/runtime, displayed geometry drafts and active viewport gestures. No map/history/selection change, support contact geometry or authored face inference. UI: Player support… → Frame sample."
    )]
    pub(in crate::mcp) fn frame_player_support_sample(
        &self,
        Parameters(args): Parameters<FramePlayerSupportSample>,
    ) -> ToolResult {
        run(|| {
            self.session().frame_player_support_sample(
                Actor::Agent,
                parse_player_support_token(&args.token)?,
                args.index,
                args.aspect,
            )
        })
    }
}
