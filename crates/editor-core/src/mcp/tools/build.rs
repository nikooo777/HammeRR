use super::super::*;

#[cfg(test)]
mod instance_dependency_tests;
#[cfg(test)]
mod instance_rejection_tests;
#[cfg(test)]
mod prop_vertex_lighting_settings_tests;
#[cfg(test)]
mod recipe_authority_tests;
#[cfg(test)]
mod reflection_settings_tests;
#[cfg(test)]
mod sky_packing_settings_tests;
#[cfg(test)]
mod submission_tests;
#[cfg(test)]
mod viscluster_tests;
#[cfg(test)]
mod water_packing_settings_tests;

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
        description = "Inspect shared build settings and limitations. Defaults are fullbright, no physics bodies, bake_static_prop_vertices:false, pack_sky_assets:false, pack_water_assets:false and generate_reflections:false. prop_vertex_lighting reports current settings availability, an unavailable reason and receiver limits; it does not inspect assets. Static-prop vertex lighting requires lit mode and bounces:0 through 4, supports independently evaluated LDR or LDR+HDR, and uses CPU sampling without native capture. Indirect reception uses the preceding world/terrain lighting pass and requires texture-header or authored material reflectivity for every lit source. Dimensions independently use diffuse or tool VTF headers, or the SDK no-preview 128x128 convention, with each complete VTF actually read for metrics hashed; authored $reflectivity accepts one ASCII [r g b] triple of finite linear values from 0 to 1, at most 128 bytes. Only trimmed %notooltexture 0 or 1 selects that convention before dimension reads; empty or other values and failed required providers reject without fallback. Props and brush models are not reflected sources. Unsupported requested receivers reject the build; disablevertexlighting=1 is the explicit per-prop opt-out. disableselfshadowing=1 omits only the receiver ordinal from point/spot, sun, sky-ambient and reflected receiver rays; world, terrain and other props still block. limits.receiver.ignore_normals reports ignorenormals=1 support at depths 0 through 4: constant 0.35 point/spot/sun reception, full-sphere sky ambient and spherical reflected reception with SDK angular normalization, retaining every real shadow. limits.receiver.lighting_origin reports that lightingorigin preserves a unique visible info_lighting point for native runtime lighting while baked samples remain at actual vertices, without embedded recovery. Water packing retains original files for complete supported static Water material/Patch/texture closures. Every reachable Water material requires an explicit static cubemap with original LDR/HDR siblings; runtime/default env_cubemap, missing assets and unsupported graphs stop publication. It supports every lighting mode without native capture. Sky packing retains the exact authored family's complete supported material/Patch/LDR/HDR closure; incomplete or unsupported families fail without automatic fallback. It is independent of baked lighting and does not select the game's rendering channel. Native reflection generation requires direct_ldr or direct_ldr_hdr and a verified capture runtime; direct_ldr_hdr requires complete LDR and HDR captured samples before publishing. direct_ldr_hdr separately bakes LDR/HDR channels with channel-specific authored inputs; stats.hdr reports HDR counters and native map-data eligibility, lighting_total_work is shared aggregate, and flat lighting counters remain LDR. Runtime HDR selection still requires game/hardware support. direct_ldr bakes static/switchable/animated point/spot and static direct-sky and sampled sky ambient on world/brush-entity faces and power-2/3/4 terrain, including legacy/percentage falloff, shadows and smoothed terrain normals. Engine physics builds checked brush/fluid/terrain bodies using an isolated verified CS:S Linux64 worker. Settings leave VMF/undo unchanged."
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
        description = "Inspect built-in Fast preview/Final lighting and up to 32 custom build recipes, their exact supported compiler settings, active recipe identity, per-Session action revision, storage errors and frozen-job rules. Does not build or change the map."
    )]
    pub(in crate::mcp) fn get_build_recipes(&self) -> ToolResult {
        ok(self.session().build_recipes())
    }

    #[tool(
        description = "Reload the complete saved build recipe list through the same Session operation as Reload recipes in Build settings/Build details. Failed bounded read or validation preserves the valid list and active identity, exposes an error and blocks custom writes until repair and successful Reload. Optional revision rejects stale requests; an active build in the originating document and closing block Reload. Current compiler options, map/history/camera, pending reviews and captured jobs are unchanged. Active identity is retained with its saved name only if the same recipe still matches current options. Attached Sessions share the saved list; current options and jobs stay document-local. Does not apply settings, initialize graphics or coordinate separate processes."
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
        description = "Save current document compiler settings as a named custom recipe, or replace an existing custom recipe with replace_id. Names are unique ignoring case and at most 80 UTF-8 bytes. Built-ins are immutable. Atomic bounded persistence leaves preferences unchanged on failure. Active builds in the originating document block edits; optional action revision rejects changed recipes or local settings; VMF and undo remain unchanged."
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
        description = "Rename an existing custom build recipe without changing its settings. Built-ins are immutable; active builds in the originating document block edits. Existing jobs retain their captured name. Optional action revision rejects changed saved recipes or local settings; VMF and undo remain unchanged."
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
        description = "Delete an existing custom build recipe. Current compiler settings and completed job provenance remain unchanged; deleting the active recipe clears its current label. Built-ins cannot be deleted; active builds in the originating document block edits. Optional action revision rejects changed saved recipes or local settings; VMF and undo remain unchanged."
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
        description = "Apply all settings from a built-in or custom recipe through shared Session build settings. Does not start a build. Active builds in the originating document block applying; different values stale prior output. Final lighting selects Direct LDR, engine physics, Detailed visibility/sampling, four bounces and 45-degree smoothing; it retains the experimental compiler's limits. Optional action revision rejects changed saved recipes or local settings; VMF and undo remain unchanged."
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
        description = "Update shared build lighting (fullbright/direct_ldr/direct_ldr_hdr), physics (none/engine) sky_quality (fast/balanced/detailed: 32/128/512 sky/model ambient and bounce directions) visibility (fast/detailed: conservative flood/portal clipping) bounces (0..4: direct only/up to four diffuse world-terrain-source bounces into world/model or opted-in prop receivers) smoothing_angle (whole degrees 0–90; zero disables automatic unassigned brush smoothing), pack_sky_assets (boolean, default false; pack the exact complete authored sky material/Patch/LDR/HDR closure), pack_water_assets (boolean, default false; pack original files for complete supported static Water material/Patch/texture closures), generate_reflections (boolean, default false; native one-bounce capture per selected lighting channel and verified packing), or bake_static_prop_vertices (boolean, default false; direct static-prop vertex lighting packed into the BSP). Static-prop vertex lighting requires direct_ldr or direct_ldr_hdr with bounces:0 through 4; invalid depths reject atomically. At depth N above zero, prop reception uses world/terrain lightmaps from depth N-1 and adds direct light once. Indirect reception requires texture-header or authored material reflectivity for every lit source material. Dimensions independently use diffuse or tool VTF headers, or the SDK no-preview 128x128 convention; every complete VTF actually read for metrics is hashed. Only trimmed %notooltexture 0 or 1 selects that convention before dimension reads; empty or other values and failed required providers reject without fallback. Props and brush models do not supply reflected light. Incompatible updates reject atomically without changing lighting or bounces. It uses CPU sampling and requires no native capture runtime. Receivers require body0/root0, unit scale, one static bodypart/model, all 1–8 draw LODs with at most 256 hardware sections total, and plain opaque selected VertexLitGeneric materials. lightingorigin preserves a unique visible info_lighting point for native runtime lighting; baked samples, normals and PVS stay at actual vertices, with no embedded-vertex recovery. ignorenormals=1 supports bounces:0 through 4, using constant 0.35 point/spot/sun reception, full-sphere sky ambient and spherical reflected reception with SDK angular normalization while retaining every real shadow. Styled lights and specialized materials fail requested compilation unless the prop explicitly sets disablevertexlighting=1. LDR+HDR publishes only complete independently evaluated channels. Omitted fields retain their values and all fields validate first; unknown arguments reject. Enabling sky or water packing starts no process or asset lookup. Water packing requires explicit static cubemaps with original LDR/HDR siblings throughout every reachable Water material; runtime/default env_cubemap, missing assets and unsupported graphs stop publication. Water packing supports every lighting mode without native capture. Compilation fails on incomplete/unsupported sky families without automatic family fallback; a declared HDR channel must be complete even with an LDR bake. Sky packing supports every lighting mode and does not require a native capture runtime or select the game's channel. Reflection generation requires direct_ldr or direct_ldr_hdr. The HDR mode captures both channels and publishes only after both verify; Fullbright reflection generation rejects atomically. get_native_capture_runtime inspects readiness; set_native_capture_runtime configures machine-local roots. Engine physics needs hammerr-physics beside the editor and verified Linux64 vphysics.so. Builds world/clip/brush-model bodies, horizontal world-fluid hulls and terrain meshes; static props use model assets. World, terrain and brush-model triangles retain per-face surface properties through a checked table of at most 127 names including default. Brush-model base masses use the dominant emitted surface with collision volume and density, or shell area, thickness and density, capped at 50,000 kg; engine mass scale and overrides remain effective. Custom density controls and detail/model fluids remain unavailable. Active builds freeze settings; differing settings stale previous output. VMF/undo/drafts remain unchanged."
    )]
    pub(in crate::mcp) fn set_compile_settings(
        &self,
        Parameters(args): Parameters<CompileSettingsArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().update_build_settings(
                Actor::Agent,
                crate::session::compilation::BuildSettingsUpdate {
                    bake_static_prop_vertices: args.bake_static_prop_vertices,
                    pack_sky_assets: args.pack_sky_assets,
                    pack_water_assets: args.pack_water_assets,
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
        description = "Start a background compile of the current map snapshot and return its job token immediately. Poll get_compile_status, or cancel_compile with that token. Does not launch the game or save the VMF. Saved maps support nested func_instance world/func_detail brushes, power 2–4 terrain, strictly admitted unnamed static point/spot lights and static props through four helper levels, with every root-relative filename resolved inside the saved parent directory. No extra terrain switch is required. Child terrain allows 65,536 source-grid vertices, 262,144 expanded grid vertices and 2,097,152 grid-work units, charging repeated occurrences and intermediate transforms separately; invalid or unsupported terrain rejects without cage substitution. Explicit default-off viewport reference contents use inspect_instance_contents/set_instance_contents; compilation does not refresh that layer. Other runtime-entity expansion, func_instance_parms, unsupported helpers and Generate reflections with instances reject. Child lights require exact light/light_spot classes, canonical classname and explicit finite origin, valid brightness/attenuation/HDR/cone fields, absent-or-zero style/defaultstyle/spawnflags and legacy angle, and empty names/targets/parents/patterns/runtime references. Unknown properties, blocks, connections and duplicate case aliases reject. A child spotlight requires explicit pitch in [-90,90], including the supported -90 downward default; zero pitch requires the first angles component zero. Optional angles defaults to zero. Both lighting-channel definitions validate even in Fullbright. Strict unnamed child prop_static supports unit-scale/body0 normal-white scenery. Skin is a nonnegative integer with absent/default0; retained model headers validate the actual family range after region selection. ignorenormals/disableshadows/disablevertexlighting/disableselfshadowing accept integer0/1 (absent0), reusing ordinary lighting consumers. Vertex-bake opt-out excludes only that receiver and never bypasses required MDL/PHY. Other non-neutral flags/body/fades/scale remain unsupported; reference refresh does not validate skin families or preview these options. Collision supports solid0/2/6 and absent/default6; mode6 requires its original matching PHY even in Fullbright with physics:none. Optional stats.instances.static_props counts expansion before regions and omits zero. Source/intermediate model boxes and original MDL/required-PHY guards use the ordinary consumer; child model reference preview remains unavailable. Exact child info_lighting requires canonical lowercase unique classname/origin/targetname, finite source/intermediate points and no behavior fields; optional finite angles/angle are irrelevant. Helper and prop lightingorigin support immediate-occurrence local bindings under explicit targetname prefixes on every containing instance with fixup_style:0 or absent/default0. ASCII letter/digit/underscore components compose outer-inner-local names up to 127 bytes; other naming styles/automatic names/global escapes reject on those branches. Missing/hidden local helpers fail only retained props; parent/sibling helpers cannot substitute, while region-exterior helpers remain available. Root props retain exact lookup of explicitly spelled final names. Generated exact/case-fold collisions reject. Optional stats.instances.lighting_origins counts visible expanded helpers including unreferenced ones before regions and omits zero; helper reference geometry/icons/bounds/picking/framing are unavailable. Instance builds retain exact source/dependency guards for atomic BSP/LIN publication, stale status, baked inspection and background launch verification. stats.instances reports global counts, max_expansion_depth and transitive dependency hashes, plus terrain_patches and terrain_vertices only when nonzero. Terrain counts describe generated patches and authored grid vertices before neighbor LOD adjustments. point_lights and spot_lights omit zero values and count expanded child lights before compile-region filtering, which selects by transformed origin; instance inspection retains root owner and leaf source provenance. Light counts add no child icons or illumination to reference or authored-lighting previews. Window controllers and both rendered-model references require absent or exact-zero initial angle fields in every lighting mode, including unlit models; malformed or ambiguous fields fail. Failed, changed or cancelled builds preserve previous outputs. Mounted detail dictionary entries accept _detail_lighting_tint as three integer linear RGB channels from 0 to 255, applied once with World generated tint; malformed selected entries fail. Existing shared material/world/build operations consume external dictionaries; entry editing is not provided. Compiler material source reads are bounded per Materials collection: VMT: 1 MiB per file, 16 MiB total and 4096 lookups; metric VTF: 256 MiB per file, 512 MiB total and 4096 lookups. Geometry-only and tracked prop-source limits remain stricter."
    )]
    pub(in crate::mcp) fn start_compile(
        &self,
        Parameters(args): Parameters<CompileArgs>,
    ) -> ToolResult {
        run(|| {
            let access = self.document_access();
            if access.managed() {
                access.start_compile(
                    Actor::Agent,
                    args.path.as_deref().map(std::path::Path::new),
                    false,
                    args.expected_generation,
                )
            } else {
                crate::runtime::start_compile_with_operation(
                    &self.session,
                    Actor::Agent,
                    args.path.as_deref().map(std::path::Path::new),
                    false,
                    args.expected_generation,
                    self.operation.clone(),
                )
            }
        })
    }

    #[tool(
        description = "Inspect the latest compile job: token, queued/running/terminal state, queue position and capacity, current stage, elapsed time, output path, snapshot/dependency freshness and terminal diagnostics/statistics. Instance jobs report bounded stats.instances counts/files/hashes and instance_dependencies.current/error from cheap metadata/alias checks; launch verifies exact dependencies in its background worker. stats.instances.terrain_patches and terrain_vertices appear only when nonzero and count generated patches and authored grid vertices before neighbor LOD adjustments. Optional point_lights and spot_lights also omit zero values and count expansion before compile-region filtering, rather than emitted region lights. Job diagnostics include the first 100 messages and the total count. Published successful static-prop vertex bakes expose stats.prop_vertex_lighting with actual props/models/meshes/vertices, separate ldr_bytes/hdr_bytes, work and asset_bytes. Shared with the editor's build status."
    )]
    pub(in crate::mcp) fn get_compile_status(&self) -> ToolResult {
        ok(self.session().compile_status())
    }

    #[tool(
        description = "Request cancellation of the exact queued or running compile job. Queued work retires promptly; running work remains busy until its owned cleanup stops. Poll get_compile_status until cancelled. Cancellation is rejected once output publication has completed. Previous BSP and pointfile remain unchanged when cancellation is accepted."
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
            self.document_access().cancel_compile(Actor::Agent, token)
        })
    }

    #[tool(
        description = "Compile the current snapshot using shared lighting/physics settings. direct_ldr_hdr emits separate LDR/HDR face lightmaps, world lights and ambient probes under one shared work budget; stats.hdr reports HDR counters and map_data_present (nonempty HDR light samples/world lights/ambient). Empty HDR data cannot activate native HDR; map-data presence alone does not verify runtime HDR. Flat lighting counters remain LDR and lighting_total_work is aggregate. direct_ldr bakes static/switchable/animated point/spot lights with legacy/percentage falloff and directional sun and sampled sky ambient through verified sky surfaces, with material-driven normal-map channels, world/terrain shadows and model-local self shadows. Brush entities receive pivot-aware direct and optional style-preserving bounced maps (up to four bounces) that move with the model; reflected model sources, dynamic relighting, persistent model world shadows and prop lightmaps remain unfinished. SunSpreadAngle supports 0–90 degrees with deterministic soft shadows; sky quality selects 32/128/512 directions for sun and ambient. Model ambient gathers distance-filtered local world/terrain lightmaps and sky into up to 1/4/8 separated cubes per open world leaf at Fast/Balanced/Detailed quality, with shared work bounds and counters. Adaptive refinement, per-texel prop lightmaps and reflected prop sources remain unfinished. Opt-in bake_static_prop_vertices supports direct static style-0 lighting for admitted simple opaque model receivers at bounces:0 through 4, receiving the preceding world/terrain lighting pass at each positive depth with texture-header or authored material reflectivity, dimensions independently from diffuse or tool VTF headers or the SDK no-preview 128x128 convention, and whole-VTF source guards. Only trimmed %notooltexture 0 or 1 selects that convention before dimension reads; empty or other values and failed required providers reject without fallback; unsupported requests fail rather than omit a receiver. Published results expose stats.prop_vertex_lighting with actual props, models, hardware meshes/vertices, separate ldr_bytes/hdr_bytes, work and asset_bytes. World power-2/3/4 terrain supports smooth normals sewn across matching equal-power full-edge neighbors; diagnostics and statistics describe limits and deferred edges. Convex func_areaportal volumes now separate visibility areas with checked reciprocal clip geometry and runtime portalnumber keys; leaked, overlapping or multi-area boundaries fail. func_areaportalwindow shares those boundaries with checked visible brush-model references and finite fade settings. Window portals emit version-1 bindings and retain distance/input controls; missing, ambiguous or undersized rendered models fail before publication. Window controllers and both rendered-model references require absent or exact-zero initial angle fields in every lighting mode, including unlit models; malformed or ambiguous fields fail. Authored func_viscluster supports one convex brush per helper in full-map dry builds: strict greater-than-10% leaf intersection, ambiguous claims and numerical uncertainty reject. It conservatively unions solved leaf visibility into fewer emitted clusters and recomputes PAS; no compile-time, compressed-size or native float parity benefit is promised. Shared stats distinguish visibility_solved_clusters from final clusters. Fullbright is default. Returns structured diagnostics/leak paths and preserves previous output on failure. Does not save the VMF; leaks write a .lin file. Brush entities use explicit origins or one visible origin volume, with checked local geometry and compensated UVs. Saved maps support nested world/func_detail instances, including power 2–4 terrain, strictly admitted unnamed static point/spot lights and static props, through four helper levels with root-relative filenames, global limits, leaf source provenance and guarded transitive dependencies. No extra terrain switch is required. Child terrain allows 65,536 source-grid vertices, 262,144 expanded grid vertices and 2,097,152 grid-work units, charging repeated occurrences and intermediate transforms separately. Invalid or unsupported grids reject without cage substitution. stats.instances.terrain_patches and terrain_vertices appear only when nonzero and count generated patches and authored grid vertices before neighbor LOD adjustments. Child lights require exact light/light_spot classes, canonical classname and explicit finite origin, valid ordinary brightness/attenuation/HDR/cone fields, absent-or-zero style/defaultstyle/spawnflags and legacy angle, and empty names/targets/parents/patterns/runtime references. Unknown properties, blocks, connections and duplicate case aliases reject. Child spotlights require explicit pitch in [-90,90], including the supported -90 downward default; zero pitch requires the first angles component zero, with optional angles defaulting to zero. Both LDR/HDR definitions validate even in Fullbright. Strict unnamed child prop_static supports unit-scale/body0 normal-white scenery. Skin is a nonnegative integer with absent/default0; retained model headers validate the actual family range after region selection. ignorenormals/disableshadows/disablevertexlighting/disableselfshadowing accept integer0/1 (absent0), reusing ordinary lighting consumers. Vertex-bake opt-out excludes only that receiver and never bypasses required MDL/PHY. Other non-neutral flags/body/fades/scale remain unsupported; reference refresh does not validate skin families or preview these options. Collision supports solid0/2/6 and absent/default6; mode6 requires its original matching PHY even in Fullbright with physics:none. Optional stats.instances.static_props counts expansion before regions and omits zero. Source/intermediate model boxes and original MDL/required-PHY guards use the ordinary consumer; child model reference preview remains unavailable. Exact child info_lighting requires canonical lowercase unique classname/origin/targetname, finite source/intermediate points and no behavior fields; optional finite angles/angle are irrelevant. Helper and prop lightingorigin support immediate-occurrence local bindings under explicit targetname prefixes on every containing instance with fixup_style:0 or absent/default0. ASCII letter/digit/underscore components compose outer-inner-local names up to 127 bytes; other naming styles/automatic names/global escapes reject on those branches. Missing/hidden local helpers fail only retained props; parent/sibling helpers cannot substitute, while region-exterior helpers remain available. Root props retain exact lookup of explicitly spelled final names. Generated exact/case-fold collisions reject. Optional stats.instances.lighting_origins counts visible expanded helpers including unreferenced ones before regions and omits zero; helper reference geometry/icons/bounds/picking/framing are unavailable. Optional stats.instances.point_lights and spot_lights omit zero values and count expansion before compile-region filtering, which selects by transformed origin. No new switch is required; child illumination/icons remain absent from reference and authored-lighting previews. Other runtime entities, replacements and Generate reflections with instances still reject. Mounted detail dictionary entries accept _detail_lighting_tint as three integer linear RGB channels from 0 to 255, applied once with World generated tint; malformed selected entries fail. Existing shared material/world/build operations consume external dictionaries; entry editing is not provided. Compiler material source reads are bounded per Materials collection: VMT: 1 MiB per file, 16 MiB total and 4096 lookups; metric VTF: 256 MiB per file, 512 MiB total and 4096 lookups. Geometry-only and tracked prop-source limits remain stricter."
    )]
    pub(in crate::mcp) async fn compile_map(
        &self,
        Parameters(args): Parameters<CompileArgs>,
    ) -> ToolResult {
        self.compile_snapshot(args, false).await
    }

    #[tool(
        description = "Compile the current unsaved map snapshot and launch it in the installed Linux CS:S client. Returns compile diagnostics, leak paths, and game process details. Does not save the VMF. Compiler material source reads are bounded per Materials collection: VMT: 1 MiB per file, 16 MiB total and 4096 lookups; metric VTF: 256 MiB per file, 512 MiB total and 4096 lookups. Geometry-only and tracked prop-source limits remain stricter."
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
        let submission = match LaunchSubmission::begin(&self.session, Actor::Agent) {
            Ok(submission) => submission,
            Err(error) => {
                self.session()
                    .log(Actor::Agent, format!("game launch failed: {error}"));
                return fail(error);
            }
        };
        let mut cancellation = submission.cancellation();
        let result = tokio::task::spawn_blocking(self.worker(move || submission.run())).await;
        cancellation.disarm();
        match result {
            Ok(Ok(log)) => ok(game_log_json(&log)),
            Ok(Err(error)) => fail(error),
            Err(error) => {
                let error = error.to_string();
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
        match tokio::task::spawn_blocking(
            self.worker(move || lock(&session).stop_game(Actor::Agent)),
        )
        .await
        {
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

#[cfg(test)]
mod launch_submission_tests;
