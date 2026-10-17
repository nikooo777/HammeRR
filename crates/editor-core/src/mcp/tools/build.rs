use super::super::*;

#[cfg(test)]
mod instance_dependency_tests;
#[cfg(test)]
mod instance_rejection_tests;
#[cfg(test)]
mod reflection_settings_tests;
#[cfg(test)]
mod sky_packing_settings_tests;
#[cfg(test)]
mod viscluster_tests;

#[tool_router(router = build_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Check applied map data for geometry/ID problems, invalid UVs, entity properties/connections, missing material/model paths, visible team spawns and standing-hull clearance against visible world/func_detail brushes and terrain sheets with known player-blocking materials, plus native-compiler gaps. Does not change map data; replaces the shared issue report and repair preview. Hidden content is diagnosed but excluded from spawn collision. Reports explain unchecked collision and runtime limits. This is not a leak or playability test."
    )]
    pub(in crate::mcp) fn run_map_checks(&self) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.run_map_checks(Actor::Agent);
            session.map_checks_json(None, "", 0, 50)
        })
    }

    #[tool(
        description = "Inspect shared build settings and limitations. Defaults are fullbright, no physics bodies, pack_sky_assets:false and generate_reflections:false. Sky packing retains the exact authored family's complete supported material/Patch/LDR/HDR closure; incomplete or unsupported families fail without automatic fallback. It is independent of baked lighting and does not select the game's rendering channel. Native reflection generation requires direct_ldr or direct_ldr_hdr and a verified capture runtime; direct_ldr_hdr requires complete LDR and HDR captured samples before publishing. direct_ldr_hdr separately bakes LDR/HDR channels with channel-specific authored inputs; stats.hdr reports HDR counters and native map-data eligibility, lighting_total_work is shared aggregate, and flat lighting counters remain LDR. Runtime HDR selection still requires game/hardware support. direct_ldr bakes static/switchable/animated point/spot and static direct-sky and sampled sky ambient on world/brush-entity faces and power-2/3/4 terrain, including legacy/percentage falloff, shadows and smoothed terrain normals. Engine physics builds checked brush/fluid/terrain bodies using an isolated verified CS:S Linux64 worker. Settings leave VMF/undo unchanged."
    )]
    pub(in crate::mcp) fn get_compile_settings(&self) -> ToolResult {
        ok(self.session().compile_settings())
    }

    #[tool(
        description = "Inspect the native reflection capture runtime: resolved engine_root/steam_root, explicit configured_engine_root/configured_steam_root overrides, supported_channels ldr/hdr, selected channel ldr|ldr_hdr from shared lighting, readiness and an actionable reason. Readiness validates the environment; the current map still needs complete HDR arrays for HDR capture. The mounted GameFs installation is the default only if it verifies as supported CS:S; custom source assets can use an explicit installation override. Does not launch a process or change settings."
    )]
    pub(in crate::mcp) fn get_native_capture_runtime(&self) -> ToolResult {
        ok(self.session().native_capture_runtime())
    }

    #[tool(
        description = "Replace both machine-local native reflection runtime overrides through the same Session operation as Build settings. engine_root is a verified CS:S installation; steam_root is the existing Steam client root. Omitted or null values clear that override to its checked default. Both resulting paths validate before either changes; failure preserves the prior pair. Active builds freeze runtime settings. Does not launch a process, persist roots into recipes/recent maps, or edit VMF/Undo."
    )]
    pub(in crate::mcp) fn set_native_capture_runtime(
        &self,
        Parameters(args): Parameters<NativeCaptureRuntimeArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().configure_native_capture_runtime(
                Actor::Agent,
                args.engine_root.as_deref().map(Path::new),
                args.steam_root.as_deref().map(Path::new),
            )
        })
    }

    #[tool(
        description = "Inspect built-in Fast preview/Final lighting and up to 32 custom build recipes, their exact supported compiler settings, active recipe identity, registry revision, storage errors and frozen-job rules. Does not build or change the map."
    )]
    pub(in crate::mcp) fn get_build_recipes(&self) -> ToolResult {
        ok(self.session().build_recipes())
    }

    #[tool(
        description = "Reload the complete saved build recipe list through the same Session operation as Reload recipes in Build settings/Build details. Failed bounded read or validation preserves the valid list and active identity, exposes an error and blocks custom writes until repair and successful Reload. Optional revision rejects stale requests; active builds and closing block Reload. Current compiler options, map/history/camera, pending reviews and captured jobs are unchanged. Active identity is retained with its saved name only if the same recipe still matches current options. Does not apply settings, initialize graphics, coordinate writers or share recipe state across Sessions."
    )]
    pub(in crate::mcp) fn reload_build_recipes(
        &self,
        Parameters(args): Parameters<ReloadBuildRecipesArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .reload_build_recipes(Actor::Agent, args.revision)
        })
    }

    #[tool(
        description = "Save current shared compiler settings as a named custom recipe, or replace an existing custom recipe with replace_id. Names are unique ignoring case and at most 80 UTF-8 bytes. Built-ins are immutable. Atomic bounded persistence leaves preferences unchanged on failure. Active builds block edits; VMF and undo remain unchanged."
    )]
    pub(in crate::mcp) fn save_build_recipe(
        &self,
        Parameters(args): Parameters<SaveBuildRecipeArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().save_build_recipe(
                Actor::Agent,
                &args.name,
                args.replace_id.as_deref(),
                args.revision,
            )
        })
    }

    #[tool(
        description = "Rename an existing custom build recipe without changing its settings. Built-ins are immutable; active builds block edits. Existing jobs retain their captured name. Optional revision rejects stale edits; VMF and undo remain unchanged."
    )]
    pub(in crate::mcp) fn rename_build_recipe(
        &self,
        Parameters(args): Parameters<RenameBuildRecipeArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .rename_build_recipe(Actor::Agent, &args.id, &args.name, args.revision)
        })
    }

    #[tool(
        description = "Delete an existing custom build recipe. Current compiler settings and completed job provenance remain unchanged; deleting the active recipe clears its current label. Built-ins cannot be deleted; active builds block edits. Optional revision rejects stale edits; VMF and undo remain unchanged."
    )]
    pub(in crate::mcp) fn delete_build_recipe(
        &self,
        Parameters(args): Parameters<BuildRecipeIdArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .delete_build_recipe(Actor::Agent, &args.id, args.revision)
        })
    }

    #[tool(
        description = "Apply all settings from a built-in or custom recipe through shared Session build settings. Does not start a build. Active builds block applying; different values stale prior output. Final lighting selects Direct LDR, engine physics, Detailed visibility/sampling, four bounces and 45-degree smoothing; it retains the experimental compiler's limits. Optional revision rejects stale edits; VMF and undo remain unchanged."
    )]
    pub(in crate::mcp) fn apply_build_recipe(
        &self,
        Parameters(args): Parameters<BuildRecipeIdArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .apply_build_recipe(Actor::Agent, &args.id, args.revision)
        })
    }

    #[tool(
        description = "Update shared build lighting (fullbright/direct_ldr/direct_ldr_hdr), physics (none/engine) sky_quality (fast/balanced/detailed: 32/128/512 sky/model ambient and bounce directions) visibility (fast/detailed: conservative flood/portal clipping) bounces (0..4: direct only/up to four diffuse world-terrain-source bounces into world/model receivers) smoothing_angle (whole degrees 0–90; zero disables automatic unassigned brush smoothing), pack_sky_assets (boolean, default false; pack the exact complete authored sky material/Patch/LDR/HDR closure), or generate_reflections (boolean, default false; native one-bounce capture per selected lighting channel and verified packing). Omitted fields retain their values and all fields validate first; unknown arguments reject. Enabling sky packing starts no process or asset lookup. Compilation fails on incomplete/unsupported sky families without automatic family fallback; a declared HDR channel must be complete even with an LDR bake. Sky packing supports every lighting mode and does not require a native capture runtime or select the game's channel. Reflection generation requires direct_ldr or direct_ldr_hdr. The HDR mode captures both channels and publishes only after both verify; Fullbright reflection generation rejects atomically. get_native_capture_runtime inspects readiness; set_native_capture_runtime configures machine-local roots. Engine physics needs hammerr-physics beside the editor and verified Linux64 vphysics.so. Builds world/clip/brush-model bodies, horizontal world-fluid hulls and terrain meshes; static props use model assets. World/terrain triangles retain VMT surface properties through a checked 127-entry native table; detail/model fluids, brush-model surfaces, density and compiler-derived masses remain unfinished. Active builds freeze settings; differing settings stale previous output. VMF/undo/drafts remain unchanged."
    )]
    pub(in crate::mcp) fn set_compile_settings(
        &self,
        Parameters(args): Parameters<CompileSettingsArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().update_build_settings(
                Actor::Agent,
                crate::session::compilation::BuildSettingsUpdate {
                    pack_sky_assets: args.pack_sky_assets,
                    lighting: args.lighting.as_deref(),
                    physics: args.physics.as_deref(),
                    sky_quality: args.sky_quality.as_deref(),
                    visibility: args.visibility.as_deref(),
                    bounces: args.bounces,
                    smoothing_angle: args.smoothing_angle,
                    generate_reflections: args.generate_reflections,
                },
            )
        })
    }

    #[tool(
        description = "Start a background compile of the current map snapshot and return its job token immediately. Poll get_compile_status, or cancel_compile with that token. Does not launch the game or save the VMF. Saved maps support bounded single-level func_instance world/func_detail brushes from checked relative VMFs inside the saved parent directory. Explicit default-off viewport reference contents use inspect_instance_contents/set_instance_contents; compilation does not refresh that layer. No recursive/runtime-entity expansion; func_instance_parms, unsupported helpers and Generate reflections with instances reject. Instance builds retain exact source/dependency guards for atomic BSP/LIN publication, stale status, baked inspection and background launch verification. Stats.instances reports bounded files/counts and hashes. Failed, changed or cancelled builds preserve previous outputs."
    )]
    pub(in crate::mcp) fn start_compile(
        &self,
        Parameters(args): Parameters<CompileArgs>,
    ) -> ToolResult {
        run(|| {
            start_compile(
                &self.session,
                Actor::Agent,
                args.path.as_deref().map(std::path::Path::new),
                false,
            )
        })
    }

    #[tool(
        description = "Inspect the latest compile job: token, state, current stage, elapsed time, output path, snapshot/dependency freshness and terminal diagnostics/statistics. Instance jobs report bounded stats.instances counts/files/hashes and instance_dependencies.current/error from cheap metadata/alias checks; launch verifies exact dependencies in its background worker. Job diagnostics include the first 100 messages and the total count. Shared with the editor's build status."
    )]
    pub(in crate::mcp) fn get_compile_status(&self) -> ToolResult {
        ok(self.session().compile_status())
    }

    #[tool(
        description = "Request cancellation of the exact active compile job. Poll get_compile_status until cancelled; another build remains blocked until the worker stops. Cancellation is rejected once output publication has completed. Previous BSP and pointfile remain unchanged when cancellation is accepted."
    )]
    pub(in crate::mcp) fn cancel_compile(
        &self,
        Parameters(args): Parameters<CompileTokenArgs>,
    ) -> ToolResult {
        run(|| {
            let token = args
                .token
                .parse()
                .map_err(|_| "Invalid compile job token")?;
            let mut session = self.session();
            session.cancel_compile(Actor::Agent, token)?;
            Ok(session.compile_status())
        })
    }

    #[tool(
        description = "Compile the current snapshot using shared lighting/physics settings. direct_ldr_hdr emits separate LDR/HDR face lightmaps, world lights and ambient probes under one shared work budget; stats.hdr reports HDR counters and map_data_present (nonempty HDR light samples/world lights/ambient). Empty HDR data cannot activate native HDR; map-data presence alone does not verify runtime HDR. Flat lighting counters remain LDR and lighting_total_work is aggregate. direct_ldr bakes static/switchable/animated point/spot lights with legacy/percentage falloff and directional sun and sampled sky ambient through verified sky surfaces, with material-driven normal-map channels, world/terrain shadows and model-local self shadows. Brush entities receive pivot-aware direct and optional style-preserving bounced maps (up to four bounces) that move with the model; reflected model sources, dynamic relighting, persistent model world shadows and prop lightmaps remain unfinished. SunSpreadAngle supports 0–90 degrees with deterministic soft shadows; sky quality selects 32/128/512 directions for sun and ambient. Model ambient gathers distance-filtered local world/terrain lightmaps and sky into up to 1/4/8 separated cubes per open world leaf at Fast/Balanced/Detailed quality, with shared work bounds and counters. Adaptive refinement and baked prop lighting remain unfinished. World power-2/3/4 terrain supports smooth normals sewn across matching equal-power full-edge neighbors; diagnostics and statistics describe limits and deferred edges. Convex func_areaportal volumes now separate visibility areas with checked reciprocal clip geometry and runtime portalnumber keys; leaked, overlapping or multi-area boundaries fail. func_areaportalwindow shares those boundaries with checked visible brush-model references and finite fade settings. Window portals emit version-1 bindings and retain distance/input controls; missing, ambiguous or undersized rendered models fail before publication. Authored func_viscluster supports one convex brush per helper in full-map dry builds: strict greater-than-10% leaf intersection, ambiguous claims and numerical uncertainty reject. It conservatively unions solved leaf visibility into fewer emitted clusters and recomputes PAS; no compile-time, compressed-size or native float parity benefit is promised. Shared stats distinguish visibility_solved_clusters from final clusters. Fullbright is default. Returns structured diagnostics/leak paths and preserves previous output on failure. Does not save the VMF; leaks write a .lin file. Brush entities use explicit origins or one visible origin volume, with checked local geometry and compensated UVs."
    )]
    pub(in crate::mcp) async fn compile_map(
        &self,
        Parameters(args): Parameters<CompileArgs>,
    ) -> ToolResult {
        self.compile_snapshot(args, false).await
    }

    #[tool(
        description = "Compile the current unsaved map snapshot and launch it in the installed Linux CS:S client. Returns compile diagnostics, leak paths, and game process details. Does not save the VMF."
    )]
    pub(in crate::mcp) async fn run_map(
        &self,
        Parameters(args): Parameters<CompileArgs>,
    ) -> ToolResult {
        self.compile_snapshot(args, true).await
    }

    #[tool(
        description = "Launch the latest successful compile of the current map in Linux CS:S. Rejects outdated compiles and duplicate launches. Use run_map to compile and launch in one call."
    )]
    pub(in crate::mcp) async fn launch_game(&self) -> ToolResult {
        let session = self.session.clone();
        let result = tokio::task::spawn_blocking(move || launch(&session, Actor::Agent)).await;
        match result {
            Ok(Ok(log)) => ok(game_log_json(&log)),
            Ok(Err(error)) => fail(error),
            Err(error) => {
                let error = error.to_string();
                let _ = self
                    .session()
                    .finish_launch(Actor::Agent, Err(error.clone()));
                fail(error)
            }
        }
    }

    #[tool(
        description = "Read bounded tails of the most recent CS:S run's engine console and launcher output, including process status and exit code. Logs remain available after the game exits.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn game_log(&self, Parameters(args): Parameters<GameLogArgs>) -> ToolResult {
        run(|| {
            self.session()
                .game_log(args.lines.unwrap_or(game::DEFAULT_LOG_LINES))
                .map(|log| game_log_json(&log))
        })
    }

    #[tool(
        description = "Stop only the CS:S process launched by this HammeRR session. The game log remains readable afterward."
    )]
    pub(in crate::mcp) async fn stop_game(&self) -> ToolResult {
        let session = self.session.clone();
        match tokio::task::spawn_blocking(move || lock(&session).stop_game(Actor::Agent)).await {
            Ok(Ok(())) => ok(json!({"stopped": true})),
            Ok(Err(error)) => fail(error),
            Err(error) => fail(error.to_string()),
        }
    }
}

#[tool_router(router = compile_region_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect committed native compile-region bounds/enabled state, metadata errors, suggested selection/map bounds including terrain cages, owned draft token and limits. Region builds use one sealed axis-aligned box and preserve source/history. World/detail terrain needs its full base and displaced grid contained. Water must be fully contained structural world geometry. Crossing terrain/water, detail/model water, slime, overlays, areaportals and occluders require a full-map build; runtime-model terrain remains unsupported. Disable regions for full-map leak checks."
    )]
    pub(in crate::mcp) fn get_compile_region(&self) -> ToolResult {
        ok(self.session().compile_region_state())
    }

    #[tool(
        description = "Preview compile-region metadata through the shared Session form without changing the VMF/history or source geometry. Pass the latest snapshot plus enabled and finite mins/maxs (at least 16 units per axis, within ±16368). Or pass clear:true alone with snapshot to preview removing imported metadata. Orange bounds show the draft; compile jobs still use committed settings. A newer draft replaces ownership. Apply or cancel with its token."
    )]
    pub(in crate::mcp) fn preview_compile_region(
        &self,
        Parameters(args): Parameters<CompileRegionPreviewArgs>,
    ) -> ToolResult {
        run(|| {
            let definition = if args.clear == Some(true) {
                if args.enabled.is_some() || args.mins.is_some() || args.maxs.is_some() {
                    return Err("Clear takes no bounds or enabled state".into());
                }
                None
            } else {
                Some(map_core::compile_region::Definition {
                    enabled: args.enabled.ok_or("enabled is required")?,
                    bounds: map_core::compile_region::Bounds {
                        mins: DVec3::from_array(args.mins.ok_or("mins is required")?),
                        maxs: DVec3::from_array(args.maxs.ok_or("maxs is required")?),
                    },
                })
            };
            self.session()
                .preview_compile_region(Actor::Agent, definition, &args.snapshot)
        })
    }

    #[tool(
        description = "Apply the owned, current compile-region draft as one undoable metadata edit. Bounds, enable/disable and explicit Clear share this operation. Source geometry and selection stay intact; normal revision changes invalidate other edit previews and stale earlier builds. Running jobs retain their original region snapshot."
    )]
    pub(in crate::mcp) fn apply_compile_region(
        &self,
        Parameters(args): Parameters<CompileRegionTokenArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .apply_compile_region(Actor::Agent, args.token)
        })
    }

    #[tool(
        description = "Cancel only the matching compile-region draft token. Leaves native metadata, geometry, selection, Undo and other previews unchanged. Stale/foreign tokens reject without cancelling replacements."
    )]
    pub(in crate::mcp) fn cancel_compile_region(
        &self,
        Parameters(args): Parameters<CompileRegionTokenArgs>,
    ) -> ToolResult {
        run(|| self.session().cancel_compile_region(args.token))
    }

    #[tool(
        description = "Frame the current compile-region draft or committed bounds using the shared camera operation. Optional positive finite aspect defaults to 16:9. Does not enable a region, edit the map, or change selection."
    )]
    pub(in crate::mcp) fn frame_compile_region(
        &self,
        Parameters(args): Parameters<CompileRegionFrameArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.frame_compile_region(Actor::Agent, args.aspect.unwrap_or(16.0 / 9.0))?;
            Ok(session.compile_region_state())
        })
    }
}
