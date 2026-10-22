use super::super::*;

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InspectInstanceContentsArgs {
    offset: Option<usize>,
    limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InstanceContentsAction {
    Refresh,
    Cancel,
    Clear,
    Frame,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InstanceContentsArgs {
    action: InstanceContentsAction,
    token: Option<String>,
    ids: Option<Vec<String>>,
    aspect: Option<f32>,
    background: Option<bool>,
}

#[tool_router(router = instance_contents_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect the explicitly refreshed nested func_instance world/detail reference layer, including real displaced power 2–4 terrain and counts for strictly admitted unnamed static point/spot lights, through four helper levels without loading child bytes or initializing graphics. Reports disabled/empty/not_loaded/loading/ready/stale/cancelled/failed/context_suppressed, requested/displayed availability, owner-only bounds/transforms, root-relative references, root owner versus leaf source provenance, saved-parent source and transitive child identities, bounded diagnostics and CPU/GPU limits. Terrain bounds and root-helper picking use the displaced mesh. Terrain face_provenance rows add terrain:true, power, vertices and displayed triangles beside root owner, true leaf file and original brush/side IDs. Optional stats.terrain_patches and terrain_vertices omit zero counts and describe generated patches and authored grid vertices before neighbor LOD adjustments. Optional stats.point_lights and spot_lights omit zero values and count expansion before compile-region filtering. Optional stats.static_props uses the same pre-region count policy. Strict child prop_static supports ordinary solid0/2/6 and absent/default6, but static_prop_preview reports unavailable with its shared reason; child models add no geometry, content bounds, picking or framing. A model-only ready assembly retains identities with available:false and null owner bounds. Refresh validates ordinary header-independent fade/DX options with the same compiler parser, but never reads child MDL/PHY assets or certifies runtime visibility. Optional stats.player_spawns counts expanded canonical info_player_terrorist/info_player_counterterrorist children before compile-region filtering and omits zero. player_spawn_preview returns available:false with an explicit reason: no child spawn models/icons, bounds, picking or framing are available. Spawn-only references can be ready with checked identities/counts, available:false, zero geometry and null owner bounds. No player model assets are loaded. Child func_viscluster helpers add optional omit-zero stats.visclusters, counting visible expanded helpers before compile-region filtering; hidden helpers and all-hidden helper brushes are omitted. viscluster_preview returns available:false and its shared reason: no helper reference geometry, icons, content bounds, picking or framing is available. Helpers never add physical detail brushes or substitute reference geometry. Helper-only references remain ready with source/dependency identities, zero mesh and null owner bounds. Refresh checks bounded source and transformed hulls; dry-leaf membership, liquid, area, ambiguity and regional checks remain compiler-owned. Scoped child info_lighting helpers add omit-zero stats.lighting_origins before regions (including unreferenced helpers) and lighting_origin_preview:{available:false,reason}; no helper geometry, icons, bounds, picking or framing is shown. limits.lighting_origin_name_bytes is 127. Helper-only assemblies can be ready with identities/counts, available:false, zero geometry and null owner bounds. Child prop lightingorigin binds only within its immediate file occurrence under explicit targetname prefixes on every ancestor, fixup_style:0 or absent/default0, with ASCII letter/digit/underscore components and composed names up to 127 bytes. Postfix/None/automatic names/global escapes reject on these branches. Missing/hidden local bindings are checked only for retained props during compilation; ready reference resolution does not prove them usable. Outside-region helpers remain available; root props keep exact final-name lookup. Child lights add no icons, illumination, pick surfaces or content bounds to the reference or authored-lighting preview. A light-only assembly can report ready with available:false, zero geometry counts, no face provenance and null owner bounds, retaining checked dependency identities but no reference geometry to display, pick or frame. limits reports terrain_source_vertices:65536, terrain_expanded_vertices:262144, terrain_grid_work:2097152, terrain_context_vertices:262144 and terrain_parent_lod:must_match_ordinary_context. Owners paginate with offset and limit 1–64. Defaults off; toggling set_view.instance_contents only displays a current snapshot and never loads files. Refresh explicitly with set_instance_contents. No child editing or root face/placement identities. Refresh does not capture reflections; the existing compile operations can capture supported assembled geometry with root-map env_cubemap helpers. Child cubemap helpers remain unsupported and authored cubemap binding inspection/drafts are unavailable with visible references.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn inspect_instance_contents(
        &self,
        Parameters(args): Parameters<InspectInstanceContentsArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .inspect_instance_contents(args.offset.unwrap_or(0), args.limit.unwrap_or(64))
        })
    }

    #[tool(
        description = "Manage the same read-only reference layer as Instance contents… in the viewport. action=refresh freezes the in-memory parent and resolves bounded relative .vmf world/func_detail children outside the Session lock; background=true returns the request token immediately, default false waits. Refresh evicts the previous effective layer; any missing/unsupported/stale dependency fails the whole request. action=cancel/clear/frame requires the current inspection token. Cancel requests worker cancellation; Clear frees the ready layer. Frame accepts root instance ids (omitted uses selection, empty selection frames all) and positive finite aspect; it changes only the camera and preserves it on failure. Refresh/cancel/clear do not change the checkbox. Nested references support four helper levels with root-relative filenames and leaf source provenance. Unnamed static light/light_spot children pass the same strict compiler admission, including absent-or-zero static style/defaultstyle/spawnflags and legacy angle, empty runtime names/targets and explicit spotlight pitch in [-90,90]; zero pitch requires the first angles component zero, and the -90 downward default is supported. Optional point_lights/spot_lights counts precede compile-region filtering and omit zeros. Child player spawns support only canonical lowercase info_player_terrorist/info_player_counterterrorist with an explicit finite origin and finite source/intermediate poses. Optional angles defaults to zero; legacy angle must be absent/zero. spawnflags must be absent or integer0; names/targets/parents must be absent/empty. Brushes, I/O, opaque blocks, duplicate/case aliases and extra keys reject; no enabled/team fields are supported. Optional stats.player_spawns counts expansion before origin-based region filtering and omits zero. Spawn models/icons/reference bounds/picking/framing are unavailable; no native spawn-selection, team-balancing or player-clearance claim. Strict child prop_static supports unit-scale/body0 normal-white scenery with ordinary solid0/2/6 and absent/default6. Skin is a nonnegative integer with absent/default0; retained model headers validate the actual family range after region selection. ignorenormals/disableshadows/disablevertexlighting/disableselfshadowing accept integer0/1 (absent0), reusing ordinary lighting consumers. Vertex-bake opt-out excludes only that receiver and never bypasses required MDL/PHY. fademindist/fademaxdist/fadescale accept finite native-f32 values with defaults -1/0/1; end and scale must be nonnegative. A negative start inherits a positive end; end0 emits start0 and disables the authored fade interval. screenspacefade accepts integer0/1 (default0), interpreting start/end as distance or screen pixel width; no start-end ordering is imposed. mindxlevel/maxdxlevel accept u16 integers (default0); minimum must not exceed a nonzero maximum. Zero DX bounds use the ordinary unrestricted defaults. Shared header-independent visibility validation runs during reference refresh without model reads. Other non-neutral flags/body/scale remain unsupported; refresh does not certify model families or runtime fade/DX appearance. The opt-in vertex bake retains its ordinary unrestricted-DX receiver requirement. Optional static_props counts also precede region filtering and omit zero. Model files are not read during refresh; static_prop_preview explicitly reports unavailable, with no model bounds, pick surfaces or framing. Compilation uses the ordinary post-region MDL/required-PHY consumer. Optional stats.player_spawns counts expanded canonical info_player_terrorist/info_player_counterterrorist children before compile-region filtering and omits zero. player_spawn_preview returns available:false with an explicit reason: no child spawn models/icons, bounds, picking or framing are available. Spawn-only references can be ready with checked identities/counts, available:false, zero geometry and null owner bounds. No player model assets are loaded. Child func_viscluster helpers add optional omit-zero stats.visclusters, counting visible expanded helpers before compile-region filtering; hidden helpers and all-hidden helper brushes are omitted. viscluster_preview returns available:false and its shared reason: no helper reference geometry, icons, content bounds, picking or framing is available. Helpers never add physical detail brushes or substitute reference geometry. Helper-only references remain ready with source/dependency identities, zero mesh and null owner bounds. Refresh checks bounded source and transformed hulls; dry-leaf membership, liquid, area, ambiguity and regional checks remain compiler-owned. Scoped child info_lighting helpers add omit-zero stats.lighting_origins before regions (including unreferenced helpers) and lighting_origin_preview:{available:false,reason}; no helper geometry, icons, bounds, picking or framing is shown. limits.lighting_origin_name_bytes is 127. Helper-only assemblies can be ready with identities/counts, available:false, zero geometry and null owner bounds. Child prop lightingorigin binds only within its immediate file occurrence under explicit targetname prefixes on every ancestor, fixup_style:0 or absent/default0, with ASCII letter/digit/underscore components and composed names up to 127 bytes. Postfix/None/automatic names/global escapes reject on these branches. Missing/hidden local bindings are checked only for retained props during compilation; ready reference resolution does not prove them usable. Outside-region helpers remain available; root props keep exact final-name lookup. Lights add no icons, illumination, content bounds or pick surfaces in reference or authored-lighting previews; a light-only refresh is ready with available:false and zero geometry, so Frame has no reference geometry to frame. Power 2–4 world/detail terrain uses actual displaced triangles for flat-color rendering, bounds and root-helper picking; no new switch is needed. Child terrain is bounded to 65,536 source-grid vertices, 262,144 expanded grid vertices and 2,097,152 grid-work units; repeated occurrences and intermediate transforms count separately. The complete parent/child terrain context is capped at 262,144 vertices. Invalid terrain fails the entire refresh without cage substitution. Refresh also fails if adding child contents would change ordinary parent terrain vertices or triangle indices relative to the ordinary parent context; preexisting parent-to-parent LOD alone does not block preview, and compilation remains available. No automatic loading, BSP build, child edits, other runtime entities, material fidelity or native runtime claim. Visibility/picking are centrally suppressed outside ordinary Objects mode or during authoring tools/drafts; source changes require another explicit Refresh."
    )]
    pub(in crate::mcp) fn set_instance_contents(
        &self,
        Parameters(args): Parameters<InstanceContentsArgs>,
    ) -> ToolResult {
        run(|| match args.action {
            InstanceContentsAction::Refresh => {
                if args.token.is_some() || args.ids.is_some() || args.aspect.is_some() {
                    return Err("Refresh accepts only action and optional background".into());
                }
                let request = self
                    .session()
                    .begin_instance_contents_refresh(Actor::Agent)?;
                let token = request.token();
                if args.background.unwrap_or(false) {
                    let shared = self.session.clone();
                    let spawned = std::thread::Builder::new()
                        .name("instance-contents".into())
                        .spawn(self.worker(move || {
                            let result = request.run();
                            let _ = lock(&shared)
                                .finish_instance_contents_refresh(Actor::Agent, result);
                        }));
                    if let Err(error) = spawned {
                        return self.session().fail_instance_contents_refresh(
                            Actor::Agent,
                            token,
                            format!("Could not start instance refresh: {error}"),
                        );
                    }
                    Ok(json!({"token":token.to_string(),"background":true}))
                } else {
                    let result = request.run();
                    self.session()
                        .finish_instance_contents_refresh(Actor::Agent, result)
                }
            }
            action => {
                if args.background.is_some() {
                    return Err("background is accepted only for Refresh".into());
                }
                let token = parse_token(
                    args.token
                        .as_deref()
                        .ok_or("Supply the current instance contents token")?,
                )?;
                let mut session = self.session();
                match action {
                    InstanceContentsAction::Cancel | InstanceContentsAction::Clear
                        if args.ids.is_some() || args.aspect.is_some() =>
                    {
                        Err("Cancel and Clear accept only action and token".into())
                    }
                    InstanceContentsAction::Cancel => {
                        session.cancel_instance_contents_refresh(Actor::Agent, token)
                    }
                    InstanceContentsAction::Clear => {
                        session.clear_instance_contents(Actor::Agent, token)
                    }
                    InstanceContentsAction::Frame => {
                        let ids = ids_or_selection(&session, args.ids.as_deref())?;
                        session.frame_instance_contents(
                            Actor::Agent,
                            token,
                            &ids,
                            args.aspect.unwrap_or(1.0),
                        )
                    }
                    InstanceContentsAction::Refresh => unreachable!(),
                }
            }
        })
    }
}

#[cfg(test)]
mod tests;
