use super::super::*;

fn parse_terrain_snapshot_token(token: &str) -> Result<u64, String> {
    if token.is_empty() || token.len() > 20 || !token.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err("Native terrain snapshot token must be a bounded decimal integer".into());
    }
    token
        .parse()
        .map_err(|_| "Invalid native terrain snapshot token".into())
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct PrepareTerrainSnapshot {
    snapshot: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct CompareTerrainBasis {
    token: String,
    #[serde(default)]
    options: crate::session::compiled_terrain_snapshot::ComparisonOptions,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct InspectTerrainSnapshot {
    token: String,
    displacement: Option<u16>,
    offset: Option<usize>,
    limit: Option<usize>,
    source_offset: Option<usize>,
    source_limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(in crate::mcp) enum TerrainSnapshotAction {
    Cancel,
    Clear,
    Show,
    Hide,
    Guide,
    ClearGuide,
    CancelComparison,
    ClearComparison,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct SetTerrainSnapshot {
    action: TerrainSnapshotAction,
    token: String,
    displacement: Option<u16>,
    vertex: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub(in crate::mcp) struct FrameTerrainSample {
    token: String,
    displacement: u16,
    vertex: usize,
    aspect: f32,
}

#[tool_router(router = compiled_terrain_snapshot_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect preparation readiness, source snapshot, owned token, worker/cleanup state and the complete native terrain observation receipt. comparison reports the explicit-profile complete offline prediction status, CPU/FP receipt and per-field equality/error summaries without changing observed fields. Preparation observes final position/normal/tangent-S/tangent-T fields in one private supported Linux CS:S client. Existing rows can be historical and report current:false with a reason. Does not start a game or compute substitute vectors. UI: Native terrain….",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_compiled_terrain_snapshot(&self) -> ToolResult {
        run(|| Ok(self.session().compiled_terrain_snapshot()))
    }

    #[tool(
        description = "Prepare a complete observed native terrain snapshot through the shared Session worker. Requires the latest get_compiled_terrain_snapshot.snapshot, a current saved successful owned build and the supported Linux CS:S/Steam runtime. Returns accepted worker status/token; poll get_compiled_terrain_snapshot for completion. Uses the exact complete emitted BSP and verified final client arrays, with -insecure and sv_lan1 in an isolated private game. Display filters do not limit observation. An accepted attempt retires previous rows and the vector guide. Native errors, cancellation and unconfirmed cleanup remain explicit and cannot return previous successful rows. Admission errors preserve the previous result. No authored map, selection, history or camera changes. UI: Native terrain… → Prepare."
    )]
    pub(in crate::mcp) fn prepare_compiled_terrain_snapshot(
        &self,
        Parameters(args): Parameters<PrepareTerrainSnapshot>,
    ) -> ToolResult {
        run(|| {
            self.document_access()
                .prepare_compiled_terrain_snapshot(Actor::Agent, args.snapshot)
        })
    }

    #[tool(
        description = "Compute the complete offline native terrain position/normal/tangent-S/tangent-T pipeline from the exact retained compiled BSP alone and compare every raw field against the matching independent observed snapshot token. Requires a current complete observed snapshot; no game starts. Uses the existing bounded cancellable terrain worker, so poll get_compiled_terrain_snapshot.comparison. options contains independent position_rounding and rounding (nearest_even, toward_negative, toward_positive, toward_zero), flush_to_zero and denormals_are_zero; defaults nearest_even/nearest_even/false/false. The worker binds the actual current x86 CPU and RSQRTSS estimate probe; settings describe the predictive arithmetic profile, never an inferred historical native MXCSR. Successful complete comparisons retain mismatches as results, per-field exact-bit/ULP/absolute-error summaries and paged predicted values; observed rows, guides and framing remain independent. Accepted recomputation retires only previous comparison rows; admission failures preserve them. No preview normals, observed input fitting, shader pixels, VRAD or later LOD-frame parity. UI: Native terrain… → Offline comparison → Compare all."
    )]
    pub(in crate::mcp) fn compare_compiled_terrain_basis(
        &self,
        Parameters(args): Parameters<CompareTerrainBasis>,
    ) -> ToolResult {
        run(|| {
            self.document_access().compare_compiled_terrain_basis(
                Actor::Agent,
                parse_terrain_snapshot_token(&args.token)?,
                args.options,
            )
        })
    }

    #[tool(
        description = "Page one complete observed native terrain snapshot. token is required. Omit displacement for emitted native catalog rows; provide its exact ordinal for native row-major sample rows. offset defaults0 and limit64; limits1–64, response<=256KiB. Samples report exact build/profile/displacement/FACE/grid identity, raw position/N/S/T float components and hex bits, vector states and magnitudes. Nonfinite components are null with preserved bits; zero vectors stay zero. Selected-displacement pages include exact compiled source contributors with source_offset(default0)/source_limit(default1), at most64 and128KiB. Instance occurrence paths label provenance only, with no authored vertex/triangle correspondence. Historical rows remain explicit; frame and guides require a current build/runtime and no displayed geometry draft. When a complete offline basis comparison exists, catalog rows include its displacement summary and sample rows include comparison.predicted plus per-field exact bit equality, component_equal, absolute_error, ulp_distance and maxima. Nonfinite numerical errors are null; signed-zero differences remain bit mismatches. No native pixel, VRAD or runtime LOD-frame agreement is claimed.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn inspect_compiled_terrain_snapshot(
        &self,
        Parameters(args): Parameters<InspectTerrainSnapshot>,
    ) -> ToolResult {
        run(|| {
            self.session().inspect_compiled_terrain_snapshot(
                crate::session::compiled_terrain_snapshot::Query {
                    token: parse_terrain_snapshot_token(&args.token)?,
                    displacement: args.displacement,
                    offset: args.offset.unwrap_or(0),
                    limit: args.limit.unwrap_or(64),
                    source_offset: args.source_offset.unwrap_or(0),
                    source_limit: args.source_limit.unwrap_or(1),
                },
            )
        })
    }

    #[tool(
        description = "Manage the matching native terrain snapshot token. cancel requests pending worker cancellation; clear also drops retained rows and guide and prevents late publication. Both accept the exact owned token after source staleness. cancel also cancels a running offline comparison. clear retires observed and predicted rows together. cancel_comparison cancels only the offline worker; clear_comparison drops only its result, cancels any pending comparison and prevents late publication while preserving observed rows/guide. hide retains the guide request and rows; show rechecks current build/runtime/source and viewport context. guide requires one displacement and native row-major vertex, replaces the single guide and shows it. clear_guide drops only the guide. Only guide accepts displacement/vertex. The guide is one point cross plus available N/S/T directions normalized to a fixed display length; reported native magnitudes/bits stay unchanged. Zero/nonfinite vectors have no substitute. Working region and active drafts can suppress presentation after complete observation. Stale/foreign tokens and invalid requests preserve state. UI: Native terrain… → Cancel / Clear / Show / Hide / Show vectors / Clear guide."
    )]
    pub(in crate::mcp) fn set_compiled_terrain_snapshot(
        &self,
        Parameters(args): Parameters<SetTerrainSnapshot>,
    ) -> ToolResult {
        run(|| {
            if !matches!(args.action, TerrainSnapshotAction::Guide)
                && (args.displacement.is_some() || args.vertex.is_some())
            {
                return Err("Only Guide accepts displacement and vertex".into());
            }
            let token = parse_terrain_snapshot_token(&args.token)?;
            let mut session = self.session();
            match args.action {
                TerrainSnapshotAction::Cancel => {
                    session.cancel_compiled_terrain_snapshot(Actor::Agent, token)
                }
                TerrainSnapshotAction::Clear => {
                    session.clear_compiled_terrain_snapshot(Actor::Agent, token)
                }
                TerrainSnapshotAction::Show => {
                    session.set_compiled_terrain_snapshot_visible(Actor::Agent, token, true)
                }
                TerrainSnapshotAction::Hide => {
                    session.set_compiled_terrain_snapshot_visible(Actor::Agent, token, false)
                }
                TerrainSnapshotAction::Guide => session.set_compiled_terrain_sample_guide(
                    Actor::Agent,
                    token,
                    args.displacement
                        .ok_or("Guide requires a native displacement ordinal")?,
                    args.vertex
                        .ok_or("Guide requires a native row-major vertex index")?,
                ),
                TerrainSnapshotAction::ClearGuide => {
                    session.clear_compiled_terrain_sample_guide(Actor::Agent, token)
                }
                TerrainSnapshotAction::CancelComparison => {
                    session.cancel_compiled_terrain_basis_comparison(Actor::Agent, token)
                }
                TerrainSnapshotAction::ClearComparison => {
                    session.clear_compiled_terrain_basis_comparison(Actor::Agent, token)
                }
            }
        })
    }

    #[tool(
        description = "Frame one exact observed native terrain sample through the shared Session camera operation. Requires the current owned snapshot token, native displacement ordinal, native row-major vertex index and finite aspect0.01–100. Rejects historical results, changed runtime/source/output, active viewport gestures, displayed geometry drafts and nonfinite positions. Degenerate or nonfinite N/S/T fields remain frameable at their observed position. Frames a fixed point box; no authored vertex mapping or source selection is inferred. Does not change map or history. UI: Native terrain… → Frame sample."
    )]
    pub(in crate::mcp) fn frame_compiled_terrain_sample(
        &self,
        Parameters(args): Parameters<FrameTerrainSample>,
    ) -> ToolResult {
        run(|| {
            self.session().frame_compiled_terrain_sample(
                Actor::Agent,
                parse_terrain_snapshot_token(&args.token)?,
                args.displacement,
                args.vertex,
                args.aspect,
            )
        })
    }
}
