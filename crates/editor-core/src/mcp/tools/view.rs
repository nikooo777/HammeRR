use super::super::*;

#[tool_router(router = view_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Select the captured authored source of one native static prop from the current successful owned build. Require build_token and zero-based prop ordinal from inspect_baked_prop_vertices. Rechecks owned native output, current sources and sealed available association. Replaces selection with exactly one root entity or the captured outer containing reference in Objects mode, without group expansion, unhiding or opening children. Disabled/non-baked props can select when their mapping is available. Stale/missing/invalid output or metadata, geometry drafts and active viewport work refuse atomically. Preserves camera, map/history/settings and ordinary property drafts. Returns target, source_kind, mode and selected_count. UI: Baked prop vertices → Select source / Select containing reference."
    )]
    pub(in crate::mcp) fn select_compiled_prop_source(
        &self,
        Parameters(args): Parameters<SelectCompiledPropSourceArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .select_compiled_prop_source(Actor::Agent, args.build_token, args.prop)
        })
    }

    #[tool(
        description = "Select the captured authored source of one native light from the current successful owned build. Require build_token, explicit channel ldr/hdr and zero-based ordinal from inspect_compiled_world_lights. Rechecks owned native output, current sources and available channel-sealed association. Point/spot and known sun/sky entity sources select one root entity or the captured outer containing reference in Objects mode. A generated surface source selects its exact root face in Faces mode, or its outer containing reference in Objects mode. Root-face results use target:{solid,side}, source_kind:root_face and mode:faces; existing entity results keep their target string, source_kind and mode:objects. Native owner, position and names are not authored source IDs. Missing metadata, ambiguous contributors, generated/unresolved face origins, stale/invalid output, geometry drafts and active viewport work refuse atomically. Preserves camera, map/history/settings and ordinary property drafts, without group expansion, unhiding or child-file opening. Returns target, source_kind, mode and selected_count. UI: Compiled lights → Select source / Select source face / Select containing reference."
    )]
    pub(in crate::mcp) fn select_compiled_world_light_source(
        &self,
        Parameters(args): Parameters<SelectCompiledWorldLightSourceArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().select_compiled_world_light_source(
                Actor::Agent,
                args.build_token,
                crate::session::compiled_world_lights::Channel::parse(&args.channel)?,
                args.ordinal,
            )
        })
    }

    #[tool(
        description = "Inspect compiler-owned visibility from this Session's last successfully published BSP. UI: Build details… → Inspect visibility…. No path argument. Start without build_token for summary; explicit targets {kind:leaf|cluster|portal,ordinal:N} and retained pages require its opaque token. Default target {kind:summary}. Native light target {kind:world_light,channel:ldr|hdr,ordinal:N} uses the record's stored source cluster from the same owned BSP, without position lookup. Supports POINT/SPOT/SURFACE; directional sky, unknown kinds, unbaked channels, invalid ordinals and invalid source clusters reject. The requested target stays intact; world_light reports channel/ordinal/cluster and selection describes that cluster. UI: Compiled lights → Inspect source visibility. PVS/PAS is potential visibility/audibility, not native light winners, falloff, ray occlusion or exact finite-area visibility. Position target {kind:position,source:{kind:position,position:[x,y,z]}} or {kind:position,source:{kind:camera}} resolves through native Model0 planes/nodes on that same owned BSP read. Camera source requires Perspective and samples the shared camera once; explicit world positions work in every view. Returned position data includes requested/effective f32 point, native leaf/cluster and declared ordered f32 arithmetic/tie policy without engine bit-parity claims. Solid/exterior cluster -1 has no PVS/PAS row; reserved leaf0 has no invented bounds or guide. Use the returned resolved Leaf target for subsequent pages, framing or showing a guide. kind pvs (default) or pas decodes separate exact native rows. offset and visibility_offset page independently; limit 1–64. Summary pages world leaves, leaf pages incident portals, cluster pages member leaves, portal pages its two native endpoints and returns its complete bounded ring. Leaf boxes are native bounds, not convex cells. Original portal slots and final clusters survive authored coarsening, including internal same-cluster portals. Historical intact output remains explicitly stale; changed/corrupt/missing output clears rows/totals. Snapshot/map/settings/content/retained-source guards apply, not every world VMT. Drafts are not compiled. Read-only CPU inspection, no camera/selection/history changes.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn inspect_compiled_visibility(
        &self,
        Parameters(args): Parameters<InspectCompiledVisibilityArgs>,
    ) -> ToolResult {
        use crate::session::compiled_visibility::Query;
        run(|| {
            let defaults = Query::default();
            let query = Query {
                build_token: args.build_token,
                target: args.target.unwrap_or(defaults.target),
                kind: args.kind.unwrap_or(defaults.kind),
                offset: args.offset.unwrap_or(0),
                visibility_offset: args.visibility_offset.unwrap_or(0),
                limit: args.limit.unwrap_or(defaults.limit),
            };
            let report = self.session().inspect_compiled_visibility(&query)?;
            serde_json::to_value(report).map_err(|error| error.to_string())
        })
    }

    #[tool(
        description = "Frame one compiler-owned world-leaf bounds box or portal ring from inspect_compiled_visibility. Explicit build_token and target {kind:leaf|portal,ordinal:N} required; aspect defaults to1 (0.01–100). Rechecks owned BSP/source/snapshot before camera movement. Stale/missing/replaced/unavailable data, targets other than leaf/portal and active viewport gestures refuse. Preserves camera projection, selection, history and ordinary drafts. UI: Visibility inspection → Frame selection."
    )]
    pub(in crate::mcp) fn frame_compiled_visibility(
        &self,
        Parameters(args): Parameters<FrameCompiledVisibilityArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let inspection = session.frame_compiled_visibility(
                Actor::Agent,
                args.build_token,
                args.target,
                args.aspect.unwrap_or(1.0),
            )?;
            Ok(json!({"inspection":inspection,"camera":camera_json(&session.camera())}))
        })
    }

    #[tool(
        description = "Show one owned compiler visibility guide configuration. Default mode {kind:selected} shows target {kind:leaf|portal,ordinal:N}. Mode {kind:destinations,visibility:pvs|pas} accepts a clustered leaf, cluster, or target {kind:world_light,channel:ldr|hdr,ordinal:N} and shows ALL native world-leaf bounds joined to the complete chosen PVS/PAS destination row, independent of inspection pages. Native light targets use the stored source cluster from the same owned BSP; directional sky, unknown kinds, unbaked channels, invalid ordinals and invalid source clusters reject. The guide retains the light target and reports world_light channel/ordinal/cluster. These sets do not show native light winners, falloff, ray occlusion or exact finite-area visibility. Resolve camera/position through inspect_compiled_visibility first and use its returned Leaf target. Explicit build_token required when showing. Complete sets above 512 leaves or the 256 KiB guide/renderer budget refuse with counts/cap and preserve the previous guide; ordinary inspection stays available. No silent truncation. State reports source cluster, destination cluster count, leaf count, exact bounded native leaf ordinals and resource limits. Omit/null target to clear. Current owned output/source/snapshot required; stale/missing/replaced guides are suppressed. Potential visibility/audibility bounds are not convex cells, render faces or current game occlusion. Preserves map/history/selection/ordinary drafts; active viewport gestures refuse showing. CPU state operation; no game or arbitrary file. UI: Visibility inspection → Show selected guide / Show PVS bounds / Show PAS bounds / Clear guide."
    )]
    pub(in crate::mcp) fn set_compiled_visibility_overlay(
        &self,
        Parameters(args): Parameters<SetCompiledVisibilityOverlayArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().set_compiled_visibility_overlay(
                Actor::Agent,
                args.build_token,
                args.target,
                args.mode,
            )
        })
    }

    #[tool(
        description = "Frame one native POINT/SPOT origin or finite SURFACE emission centroid from the current successful owned build. Pass explicit build_token, channel ldr/hdr and zero-based native ordinal from inspect_compiled_world_lights; optional aspect defaults to 1 and must be 0.01–100. Rechecks the same bounded owned output and source guards before moving the camera. Returns build provenance, exact record, framing bounds and camera. Surface records are emitting-triangle centroid point approximations, not finite-area visibility or runtime-selection evidence. Stale, replaced, missing, unbaked, out-of-range, directional sky, unknown and out-of-world records refuse. A known sun/sky source can instead be selected through select_compiled_world_light_source and framed through frame_selection. Active viewport gestures and block previews refuse without cancellation. Preserves projection, map, selection, history, settings and ordinary drafts. Missing source associations do not prevent valid native centroid framing. No arbitrary path, source selection, guide or lighting preview. UI: Compiled lights → select point/spot/surface record → Frame light."
    )]
    pub(in crate::mcp) fn frame_compiled_world_light(
        &self,
        Parameters(args): Parameters<FrameCompiledWorldLightArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let aspect = args.aspect.unwrap_or(1.0);
            let frame = session.frame_compiled_world_light(
                Actor::Agent,
                args.build_token,
                crate::session::compiled_world_lights::Channel::parse(&args.channel)?,
                args.ordinal,
                aspect,
            )?;
            Ok(json!({
                "build":frame.inspection.build,"channel":frame.inspection.channel,
                "record":frame.inspection.page.rows[0],"framed_bounds":frame.bounds,"aspect":aspect,
                "camera":camera_json(&frame.camera),"notes":frame.inspection.notes,
            }))
        })
    }

    #[tool(
        description = "Frame one emitted native static-prop origin from the current successful owned build. Require build_token and zero-based prop ordinal from inspect_baked_prop_vertices; optional aspect defaults to 1 and must be finite 0.01–100. Reuses bounded owned BSP/native-table inspection and current source/output guards. Disabled/non-baked receivers can frame; unavailable source association does not remove a valid native origin. Uses a fixed 16-unit half-extent around the point, not model/mesh/collision bounds or current source-object resolution. Preserves projection, selection, map/history/settings and ordinary drafts. Stale/missing/replaced/out-of-range data or active viewport work refuses before camera mutation. UI: Baked prop vertices → select prop → Frame prop origin."
    )]
    pub(in crate::mcp) fn frame_compiled_prop_origin(
        &self,
        Parameters(args): Parameters<FrameCompiledPropOriginArgs>,
    ) -> ToolResult {
        run(|| {
            let aspect = args.aspect.unwrap_or(1.0);
            let frame = self.session().frame_compiled_prop_origin(
                Actor::Agent,
                args.build_token,
                args.prop,
                aspect,
            )?;
            Ok(
                json!({"build":frame.inspection.report["build"],"record":frame.inspection.report["props"]["rows"][0],"framed_bounds":frame.bounds,"point_half_extent":crate::session::baked_prop_vertices::FRAME_HALF_EXTENT,"aspect":aspect,"camera":camera_json(&frame.camera)}),
            )
        })
    }

    #[tool(
        description = "Inspect exact native world-light records from this Session's last successful owned BSP, shared with Build settings… / Build details… → Inspect compiled lights…. CPU-only and read-only, with no path, exposure, camera or source-selection argument. Returns current/stale/unavailable build provenance, independent LDR/HDR table status and pages of at most 64 native records with ordinal, kind/code, origin, intensity, normal, cluster, style, cone dots, exponent, radius, attenuation, signed flags, texinfo and owner. Values are stored native fields, not screen brightness. Point/spot and known sun/sky source_association values preserve status:available plus the existing entity origin: root entity, or outer helper, leaf file, original/expanded entity and complete ordered occurrence route. Generated surface associations add face (compiled ordinal), material (original canonical name) and a typed root or exact-instance face origin with solid/side and occurrence. No authored light entity is invented. Associations are sealed to each exact native channel count/digest and retain historical source identity. Repeated uses of one leaf source keep distinct helper routes. Missing/inconsistent metadata, ambiguous face contributors, generated/unresolved face origins and unsupported kinds report unavailable with a reason while native rows remain readable. Pass a surface face address to inspect_baked_lightmaps source, including its full instance occurrence for exact matches; frame_compiled_face frames returned face geometry. Surface records remain centroid point approximations. Use inspect_compiled_visibility target {kind:world_light,channel:ldr|hdr,ordinal:N} with the same build_token to inspect the stored source cluster; the human action is Inspect source visibility. PVS/PAS does not show native light winners, falloff, ray occlusion or exact finite-area visibility. Start without build_token only at offset0; retain its opaque token for channel/page requests. Wrong tokens, malformed arguments and invalid ranges reject. Fullbright and unbaked HDR are explicit; a valid channel can contain zero records without implying runtime HDR readiness. Intact stale output stays historical; changed/corrupt output clears rows and totals. Pending drafts survive and are not baked. Freshness covers committed map/settings and retained source guards, not every world texture; asset-directory changes conservatively mark source evidence stale. Page copies and routes remain bounded by returned limits. No source-file opening, native game or viewport shading.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn inspect_compiled_world_lights(
        &self,
        Parameters(args): Parameters<InspectCompiledWorldLightsArgs>,
    ) -> ToolResult {
        use crate::session::compiled_world_lights::{Channel, Query};
        run(|| {
            let defaults = Query::default();
            let query = Query {
                build_token: args.build_token,
                channel: Channel::parse(args.channel.as_deref().unwrap_or("ldr"))?,
                offset: args.offset.unwrap_or(defaults.offset),
                limit: args.limit.unwrap_or(defaults.limit),
            };
            let inspection = self.session().inspect_compiled_world_lights(&query)?;
            serde_json::to_value(inspection).map_err(|error| error.to_string())
        })
    }

    #[tool(
        description = "Inspect actual baked static-prop VHV colors from this Session's successful owned BSP, shared with Build settings… / Build details… → Inspect baked prop vertices…. CPU-only and read-only; no graphics, game, model loading, arbitrary paths or map/history/selection/camera/settings changes. Returns current/stale/unavailable build provenance, native sprp ordinals/model/origin/flags, captured root or full nested authored entity source addresses, independent LDR/HDR availability and packed-file identity, ordered all-LOD hardware sections and exact stored BGRA8/RGBA8 rows. These are encoded 0–255 vertex-color bytes, not linear irradiance or native screen brightness. Pages contain at most 32 props, 32 sections and 256 vertices, with offset/limit/returned/total/next_offset. Omit build_token only for a fresh observation; explicit prop/section and nonzero offsets require the returned opaque token. Changed tokens/ranges reject without silently reusing indices. Disabled props retain ordinal holes and have no color rows; missing HDR has no fallback. Intact historical output remains explicitly stale after source/assets/settings changes; replaced/corrupt output clears all rows. Pending displayed drafts are explicitly not baked. props.rows and selected_prop expose source_association status, original Origin JSON, readable label/description; selected_prop also covers disabled/non-baked props. source_associations reports capture/seal failures without hiding valid native/color data. Captured routes remain historical and never resolve or select current objects. Source copies admit at most 128 KiB per call; reduce prop_limit after a source-page budget refusal. No root-model vertex/position mapping or viewport shading.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn inspect_baked_prop_vertices(
        &self,
        Parameters(args): Parameters<InspectBakedPropVerticesArgs>,
    ) -> ToolResult {
        use crate::session::baked_prop_vertices::{Channel, Query};
        run(|| {
            let defaults = Query::default();
            let query = Query {
                build_token: args.build_token,
                channel: Channel::parse(args.channel.as_deref().unwrap_or("ldr"))?,
                prop: args.prop,
                section: args.section,
                prop_offset: args.prop_offset.unwrap_or(defaults.prop_offset),
                prop_limit: args.prop_limit.unwrap_or(defaults.prop_limit),
                section_offset: args.section_offset.unwrap_or(defaults.section_offset),
                section_limit: args.section_limit.unwrap_or(defaults.section_limit),
                vertex_offset: args.vertex_offset.unwrap_or(defaults.vertex_offset),
                vertex_limit: args.vertex_limit.unwrap_or(defaults.vertex_limit),
            };
            self.session()
                .inspect_baked_prop_vertices(&query)
                .map(|inspection| inspection.report)
        })
    }

    #[tool(
        description = "Inspect the default-off unshadowed authored LDR or HDR point/spot/direct-sun lighting preview without graphics initialization. Reports admitted/excluded lights, flat brush receiver polygons, separate world/func_detail terrain surfaces/drawn triangles and authored MDL instance/selected-part/drawn-triangle coverage, current render-document/working-region/material snapshot, bounded diagnostics and editor limits. Static unnamed style-0 lights only, maximum 32 including one visible light_environment; conflicting visible suns are all excluded; overflow disables preview rather than choosing a prefix. Zero admitted lights make supported receivers black. Set shared lighting_preview, lighting_channel ldr/hdr (default ldr), lighting_quality fast/detail, lighting_exposure_ev and default-off lighting_sky_ambient with set_view, or temporary render_view overrides. Optional sky ambient uses the selected channel's compiler ambient or half-sun default without sky visibility/occlusion, shares its slot and summed radiance budget, and rejects the whole sun for malformed requested ambient. Fast interpolates vertex lighting and rejects enabled normal maps; Detail composes with supported flat-brush normal maps. Terrain uses checked sewn LOD normals and opaque single-base materials; declared bump/blend/detail or unsafe normals reject receiver admission. Authored MDL meshes use selected body/skin, positive uniform scale and world normals; whole-instance opaque single-base VertexLitGeneric and Normal render mode only. Model bump/normal/proxy/effect, unsupported declarations, opacity and unsafe triangles keep ordinary display with diagnostics. Direct sun has no distance attenuation and uses the same visible working-region source filter; unlike compiler cordons it does not retain sources outside that region. HDR uses compiler overrides, gains and valid absent/negative-RGB inheritance; malformed requested HDR excludes the source without LDR substitution. Lighting and skybox channels/exposures are independent. Sky visibility, shadows, penumbrae, bounce, baked lighting/material emission, animation and native brightness/tone-map parity are excluded. Disabled sky ambient ignores literal ambient fields. Whole-value unresolved template color or HDR gain tokens exclude that light with an explicit reason even in LDR preview and with sky ambient disabled; other valid lamps remain admitted. Sky-camera miniatures keep separate fixed shading. Source/history/camera and pending geometry reviews remain unchanged.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_lighting_preview(&self) -> ToolResult {
        ok(self.session().lighting_preview_state())
    }

    #[tool(
        description = "Inspect shared normal_maps preview admission through the same CPU scene as the viewport. Defaults off; enable with set_view.normal_maps or override render_view.normal_maps. Reports admitted materials, mounted VMT/VTF provenance, exact unsupported-state errors, applied/rejected flat brush polygons and resource budgets. LightmappedGeneric and ordinary static RGB tangent-space normals only. Terrain, models, sprites/icons/helpers and non-Textured views are excluded. Fixed editor directional shading or enabled Detail unshadowed authored lights; no native game parity. Reload material assets clears cached successes/failures. No graphics initialization or source/history/camera mutation.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_normal_map_preview(&self) -> ToolResult {
        ok(self.session().normal_map_preview_state())
    }

    #[tool(
        description = "Inspect baked BSP face lightmaps from this session's last successful owned build, shared with Build status → Inspect baked lightmaps…. Read-only CPU operation; no GPU, game, map/history/selection/camera changes. Optional channel ldr/hdr (default ldr), exact zero-based compiled face, style_index and map_index (0 base, 1–3 directional bump maps). Omitted face selects the first valid lit face. Paginated face summaries use offset and limit (1–32). Returns explicit current/stale/unavailable build provenance, LDR/HDR availability, dimensions/styles/bytes, decoded linear RGBExp32 minimum/mean/maximum and stored base average, diagnostics and a PNG no larger than 128 pixels per side. Display conversion is documented separately from linear statistics and is not native tone mapping. Grid dimensions and preview pixels describe native rectangular lightmap storage, not actual bake positions. Planar boundary kernels can use covered-footprint centroids; their actual positions are not retained in the BSP or reconstructed by inspection. Compiler-retained many-to-many authored contributors are paginated by contributor_offset; generated/unresolved faces are explicit. Optional typed source (kind=root with solid/side, or kind=instance with instance/file/solid/side) or selected_authored_face=true finds compiled matches paginated by offset/limit; these selectors are mutually exclusive with face. Instance contributors include captured occurrence helper/file routes. Optional source.occurrence selects one complete route; omission preserves grouped matches across repeated leaf faces. References match retained origins only and never authorize arbitrary file reads. Selecting inspection rows does not change editor selection. Pending displayed drafts are explicitly not baked. Changed/missing BSP files cannot be inspected; stale map/source/settings builds remain explicitly stale. No path argument.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn inspect_baked_lightmaps(
        &self,
        Parameters(args): Parameters<InspectBakedLightmapsArgs>,
    ) -> ToolResult {
        use crate::session::baked_lightmaps::{Channel, Query};
        let result = (|| {
            let query = Query {
                channel: Channel::parse(args.channel.as_deref().unwrap_or("ldr"))?,
                face: args.face,
                source: args.source,
                selected_authored_face: args.selected_authored_face,
                contributor_offset: args.contributor_offset.unwrap_or_default(),
                offset: args.offset.unwrap_or_default(),
                limit: args
                    .limit
                    .unwrap_or(crate::session::baked_lightmaps::MAX_PAGE_FACES),
                style_index: args.style_index.unwrap_or_default(),
                map_index: args.map_index.unwrap_or_default(),
            };
            self.session().inspect_baked_lightmaps(&query)
        })();
        let inspection = match result {
            Ok(inspection) => inspection,
            Err(error) => return fail(error),
        };
        let mut content = vec![ContentBlock::text(inspection.report.to_string())];
        if let Some(image) = inspection.image {
            let mut bytes = Vec::new();
            let mut encoder = png::Encoder::new(Cursor::new(&mut bytes), image.width, image.height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let result = encoder
                .write_header()
                .and_then(|mut writer| writer.write_image_data(&image.pixels));
            if let Err(error) = result {
                return fail(error.to_string());
            }
            content.push(ContentBlock::image(
                base64::engine::general_purpose::STANDARD.encode(bytes),
                "image/png",
            ));
        }
        Ok(CallToolResult::success(content))
    }

    #[tool(
        description = "Inspect the exact applied world skyname through the same read-only Session operation as World settings → Inspect applied sky…. Returns six material names, directory/VPK and Patch provenance, authored LDR/HDR references, VTF dimensions/formats, bounded read counts and actionable issues. ldr_ready/hdr_ready describe supported material/VTF structure only; they do not establish editor preview readiness, HDR radiance, actual game channel selection or portable sky packing. No family substitution, document/history/camera/draft changes or native processes. No arguments; refresh by calling again.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn inspect_sky_assets(
        &self,
        Parameters(_args): Parameters<InspectSkyAssetsArgs>,
    ) -> ToolResult {
        ok(json!(self.session().inspect_sky_assets()))
    }

    #[tool(
        description = "Inspect the shared Rope preview toggle, checked authored move_rope/keyframe_rope NextKey spans, static curve points, Slack/Width, bounded limits and link diagnostics. Enable through set_view.rope_preview or override one render_view. Neutral helper shading; no RopeMaterial fidelity, physics, collision, wind, runtime I/O, parenting or attachment binding. Clicking a span selects its source entity; endpoint helpers, area selection and framing remain available.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_rope_preview(&self) -> ToolResult {
        ok(self.session().rope_preview_state())
    }

    #[tool(
        description = "Inspect shared Fog preview settings and all authored env_fog_controller choices, parsed parameters and visible errors. Defaults off; no automatic controller selection. Snapshot guards reviewed settings. Static initial view-depth range fog in perspective Textured views only; sky/helpers unchanged. No native parity, radial/directional blend, far-Z clipping, HDR/zoom scaling, water, fog-volume switching, runtime I/O or transitions.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_fog_preview(&self) -> ToolResult {
        ok(self.session().fog_preview_state())
    }

    #[tool(
        description = "Apply Fog preview using the same Session operation as the Fog preview… dialog. Pass enabled, explicit env_fog_controller entity:<id> when enabling, and latest get_fog_preview.snapshot. Invalid/unsupported controllers and stale snapshots leave settings unchanged. Disabling always works with a fresh snapshot. Preview controls leave map/history unchanged. render_view accepts temporary fog_preview/fog_controller overrides."
    )]
    pub(in crate::mcp) fn set_fog_preview(
        &self,
        Parameters(args): Parameters<FogPreviewArgs>,
    ) -> ToolResult {
        run(|| {
            let controller = fog_controller_id(args.controller.as_deref())?;
            self.session()
                .set_fog_preview(args.enabled, controller, &args.snapshot)
        })
    }

    #[tool(
        description = "List named camera views stored in this VMF, including projection, position, orientation, field of view and planar zoom. Also returns generation/revision guards. Up to 64 views; names up to 80 characters. Imported Hammer cameras remain preserved separately."
    )]
    pub(in crate::mcp) fn get_camera_views(&self) -> ToolResult {
        run(|| self.session().camera_views_json())
    }

    #[tool(
        description = "Save the current shared camera as a named view in the map. Optional replace_id replaces that view's camera and name. Adds one undo step; save the VMF to persist. Duplicate names are rejected. Optional generation/revision guards prevent stale edits."
    )]
    pub(in crate::mcp) fn save_camera_view(
        &self,
        Parameters(args): Parameters<CameraViewSaveArgs>,
    ) -> ToolResult {
        run(|| {
            let guard = camera_view_guard(args.generation, args.revision)?;
            let mut session = self.session();
            let id = session.save_camera_view(Actor::Agent, &args.name, args.replace_id, guard)?;
            Ok(json!({"id":id,"state":session.camera_views_json()?}))
        })
    }

    #[tool(
        description = "Rename a saved camera view without changing its camera. One undoable map edit; optional generation/revision guards from get_camera_views."
    )]
    pub(in crate::mcp) fn rename_camera_view(
        &self,
        Parameters(args): Parameters<CameraViewRenameArgs>,
    ) -> ToolResult {
        run(|| {
            let guard = camera_view_guard(args.generation, args.revision)?;
            let mut session = self.session();
            session.rename_camera_view(Actor::Agent, args.id, &args.name, guard)?;
            session.camera_views_json()
        })
    }

    #[tool(
        description = "Restore a named camera view into the shared viewport without changing map data, selection or undo history. Restores perspective or planar projection and zoom. Optional generation/revision guards reject stale view IDs."
    )]
    pub(in crate::mcp) fn restore_camera_view(
        &self,
        Parameters(args): Parameters<CameraViewIdArgs>,
    ) -> ToolResult {
        run(|| {
            let guard = camera_view_guard(args.generation, args.revision)?;
            let mut session = self.session();
            session.restore_camera_view(Actor::Agent, args.id, guard)?;
            Ok(camera_json(&session.camera()))
        })
    }

    #[tool(
        description = "Delete a named camera view in one undoable map edit. Optional generation/revision guards reject stale IDs. Does not move the current camera."
    )]
    pub(in crate::mcp) fn delete_camera_view(
        &self,
        Parameters(args): Parameters<CameraViewIdArgs>,
    ) -> ToolResult {
        run(|| {
            let guard = camera_view_guard(args.generation, args.revision)?;
            let mut session = self.session();
            session.delete_camera_view(Actor::Agent, args.id, guard)?;
            session.camera_views_json()
        })
    }

    #[tool(
        description = "Load an external .lin/.pts pointfile into the shared leak-path renderer. Requires a regular UTF-8 file ≤2 MiB containing 2–65536 lines of XYZ coordinates within ±1000000; blank lines allowed. Validates completely before replacing a path. Enables leak visibility without moving the camera or editing map/history. The path becomes stale after map edits and clears on map replacement or a current compile result."
    )]
    pub(in crate::mcp) fn load_pointfile(
        &self,
        Parameters(args): Parameters<PointfileLoadArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.load_pointfile(Actor::Agent, std::path::Path::new(&args.path))?;
            serde_json::to_value(session.pointfile_report(0, 50)?)
                .map_err(|error| error.to_string())
        })
    }

    #[tool(
        description = "Inspect the shared external or current compiled leak path: source, stale/current state, token, current point, bounds, length and paginated XYZ points. Indices/offsets start at zero; limit defaults to 50, maximum 200. Stale external paths remain inspectable but cannot render or navigate. Shared with the Pointfile window.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_pointfile(
        &self,
        Parameters(args): Parameters<PointfileQueryArgs>,
    ) -> ToolResult {
        run(|| {
            serde_json::to_value(
                self.session()
                    .pointfile_report(args.offset, args.limit.unwrap_or(50))?,
            )
            .map_err(|error| error.to_string())
        })
    }

    #[tool(
        description = "Unload the shared external or compiled leak path without deleting any file or editing the map. Optional token rejects another client's replacement path."
    )]
    pub(in crate::mcp) fn clear_pointfile(
        &self,
        Parameters(args): Parameters<PointfileTokenArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.clear_pointfile(args.token.as_deref())?;
            serde_json::to_value(session.pointfile_report(0, 1)?).map_err(|error| error.to_string())
        })
    }

    #[tool(
        description = "Move the shared camera to a zero-based point index on the current path. Perspective aims along the next nonzero segment, or continues the last segment at the end; planar views center on the point and retain zoom. Optional token guards replacements; stale paths reject navigation. Does not edit map/history or selection."
    )]
    pub(in crate::mcp) fn navigate_pointfile(
        &self,
        Parameters(args): Parameters<PointfileNavigateArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.navigate_pointfile(args.index, args.token.as_deref())?;
            Ok(
                json!({"camera":camera_json(&session.camera()), "pointfile":session.pointfile_report(0, 1)?}),
            )
        })
    }

    #[tool(
        description = "Frame the entire current pointfile in the shared camera. Aspect is viewport width/height (default 1, range 0.01–100); optional token guards replacement. Stale paths and paths exceeding planar zoom limits reject framing without moving the camera. Shared with Frame path in the Pointfile window."
    )]
    pub(in crate::mcp) fn frame_pointfile(
        &self,
        Parameters(args): Parameters<PointfileActionArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.frame_pointfile(args.aspect.unwrap_or(1.0), args.token.as_deref())?;
            Ok(camera_json(&session.camera()))
        })
    }

    #[tool(
        description = "Inspect the current shared viewport options, including transient lighting and sky channels/exposures, without initializing graphics.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_view(&self) -> ToolResult {
        ok(view_state(self.session().view()))
    }

    #[tool(
        description = "Set shared viewport visibility and mode in any projection: instance_contents displays an explicitly refreshed read-only brush and terrain reference layer; it never loads files. inspect_instance_contents reports status and set_instance_contents refreshes/cancels/clears/frames it. lighting_preview enables bounded unshadowed static point/spot and direct sun diffuse lighting; lighting_channel ldr/hdr selects authored inputs independently of sky_channel (default ldr), lighting_quality fast/detail and lighting_exposure_ev -8..8 control its editor display. Inspect get_lighting_preview for admitted/excluded coverage and limits. One visible light_environment uses a shared light slot and headroom budget; duplicate suns are all excluded. lighting_sky_ambient optionally includes the eligible sun's unshadowed compiler ambient or half-sun default for the selected lighting channel; it defaults off and malformed requested ambient excludes that sun. HDR uses compiler overrides, gains and valid absent/negative-RGB inheritance; malformed requested HDR excludes the source without LDR substitution. No baked lighting/material emission, bounce, shadows, sky visibility, occlusion or native-brightness parity. Modes: textured, flat, wireframe, lightmap_density or smoothing_groups. surface_shading defaults off, preserving ordinary weak fixed shading; enable stronger fixed ambient/directional relief on Textured brush, terrain and authored model surfaces. Tool/sky surfaces, editor helpers and reference guides remain unchanged. Admitted authored-light receivers retain their lighting override. This editor direction has no shadows or compile/native-lighting effect. normal_maps defaults off and previews admitted flat-brush normal maps under fixed editor directional shading in Textured views; inspect get_normal_map_preview for diagnostics and limits. Terrain/models/resource guides are excluded; Detail unshadowed authored lighting composes with admitted normal maps. Flat shading ignores brush textures/transparency; wireframe shows geometry through solids. selection_tint defaults on; turn it off to show selected surfaces without their highlight while keeping selection and picking. selection_outlines defaults on and controls selected object/face edges, marker outlines, model brackets and rope centerlines while preserving surfaces, selection tint, picking and diagnostic/tool guides. authored_models, sprite_resources and entity_helpers independently control authored MDL surfaces, supported static Sprite resource images, and ordinary point/light/selected FGD geometric helpers. Hidden authored models retain exact wire bounds; selected helpers retain outlines and relevant cached failures retain amber diagnostics. entity_images controls literal FGD icons and updates sprite_resources only when that option is omitted for compatibility; entity_model_guides controls literal studio/studioprop meshes. Disabled resource categories defer new asset reads. selection_bounds adds aggregate selected bounds/center/XYZ extent guides; get_selection_presentation reports complete counts and dimensions. These display changes preserve picking, bounds, source, ownership and pending reviews. Helpers, selection and tool filters remain active. rope_preview shows bounded static authored NextKey rope spans with Slack/Width; inspect get_rope_preview for diagnostics and limits. No rope physics, parenting or attachment binding. sky_preview resolves world skyname as six mounted skybox faces selected by sky_channel (ldr default or explicit hdr), with sky_exposure_ev -8..8 applying only to HDR under the documented linear/Reinhard editor policy. This does not select game or build channels. The preview draws on sky surfaces in perspective Textured views independently of Tool textures; unavailable faces retain the tool surface. The independent sky3d_preview flag displays only an explicitly selected and prepared 3D sky region; inspect get_3d_sky_preview and prepare with set_3d_sky_preview. No sky fog or material animation; unavailable HDR never falls back to LDR. Choose a smoothing group 1–32; groups 25–32 are hard-edge memberships. Diagnostics inspect authored metadata, not baked lighting or final engine smoothing."
    )]
    pub(in crate::mcp) fn set_view(&self, Parameters(args): Parameters<SetViewArgs>) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let view = session.resolve_view_update(args.view_update())?;
            session.set_view(view);
            Ok(view_state(view))
        })
    }

    #[tool(
        description = "Inspect complete displayed selection counts and aggregate bounds, center and XYZ dimensions through the same Session report used by the Inspector and Selection bounds overlay. Includes current draft/marquee/volume/paint selection, exact child-brush and owner counts, source-brush/face/component counts and working-region/saved/editor visibility counts. Bounds are explicitly unavailable if any selected unit cannot be bounded; no prefix or partial totals. Resource display switches do not change identities or bounds. Source/history/camera are unchanged.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_selection_presentation(&self) -> ToolResult {
        let session = self.session();
        ok(json!(session.selection_presentation(session.view())))
    }

    #[tool(
        description = "Inspect shared authored-model, static Sprite resource, general-helper, FGD icon/model-guide display switches and bounded visual-resource diagnostics. Same settings as Overlays and set_view/render_view. These switches preserve geometric picking, exact bounds and selection ownership; hidden authored meshes retain wire bounds and known errors retain amber markers. Sprite resources preview only supported static editor images, without runtime animation/FX. Reported error counts cover the stated admitted-resource scope, not full map validation.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_helper_presentation(&self) -> ToolResult {
        let session = self.session();
        ok(session.helper_presentation(session.view()))
    }

    #[tool(
        description = "The user's current viewport camera.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_camera(&self) -> ToolResult {
        ok(camera_json(&self.session().camera()))
    }

    #[tool(
        description = "Move the user's viewport camera or switch perspective/top/front/side. Perspective supports position + look_at; planar views support center + ortho_height. Pass focus to frame object IDs."
    )]
    pub(in crate::mcp) fn set_camera(
        &self,
        Parameters(spec): Parameters<CameraSpec>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.preflight_instance_contents();
            let camera = spec.resolve(&session, 1.0)?;
            session.set_camera(camera);
            session.log(Actor::Agent, "moved the camera");
            Ok(camera_json(&camera))
        })
    }

    #[tool(
        description = "Render the map to a PNG with the editor's renderer. Textured, flat, wireframe and diagnostic modes work in perspective or planar views, with entity helpers, grid and shared previews. Selection tint and outlines are independent controls. Static policy/limit catalogues are omitted unless include_details=true; availability states, diagnostics and frame identity remain. Optional instance_contents, surface, surface_shading and camera overrides leave shared settings unchanged; surface_shading selects stronger fixed ambient/directional relief for Textured brush/terrain/authored-model surfaces, preserving ordinary weak shading when false and admitted authored-light overrides; tools/helpers stay unchanged. The returned view reports the actual override; the retained instance_contents report describes the exact uploaded reference snapshot; pass camera.focus to frame objects. additive_sprite_guides is an optional authored mode5 LDR editor-image approximation with constrained white/255/FX0 appearance. It requires sprite_resources, preserves literal icons and reports raw/additive policy and rejected appearance in sprite_guides; no native runtime/color-space parity.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn render_view(
        &self,
        Parameters(args): Parameters<RenderArgs>,
    ) -> ToolResult {
        let size = render_size(args.width, args.height);
        let (camera, mut view) = {
            let mut session = self.session();
            session.preflight_instance_contents();
            let camera = match args
                .camera
                .as_ref()
                .unwrap_or(&CameraSpec::default())
                .resolve_with_instance_contents(
                    &session,
                    size.0 as f32 / size.1 as f32,
                    args.instance_contents,
                    args.rope_preview,
                ) {
                Ok(camera) => camera,
                Err(error) => return fail(error),
            };
            let view = match session.resolve_view_update(args.view_update()) {
                Ok(view) => view,
                Err(error) => return fail(error),
            };
            (camera, view)
        };
        let fog_preview = {
            let session = self.session();
            let controller = match fog_controller_id(args.fog_controller.as_deref()) {
                Ok(controller) => controller,
                Err(error) => return fail(error),
            };
            match session.fog_render_state(args.fog_preview, controller) {
                Ok((fog, report)) => {
                    view.fog = fog;
                    report
                }
                Err(error) => return fail(error),
            }
        };
        let rendered = match self.render_png(&camera, size, view) {
            Ok(rendered) => rendered,
            Err(error) => return fail(error),
        };
        rendered.into_tool_result(camera, size, view, fog_preview, args.include_details)
    }

    #[tool(
        description = "Identify the object and face at a pixel of an image rendered by render_view with the same camera and size.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn pick_at_pixel(
        &self,
        Parameters(args): Parameters<PickArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.preflight_instance_contents();
            let (w, h) = render_size(args.width, args.height);
            let camera = args
                .camera
                .unwrap_or_default()
                .resolve_with_instance_contents(
                    &session,
                    w as f32 / h as f32,
                    args.instance_contents,
                    args.rope_preview,
                )?;
            let ndc_x = 2.0 * (args.x + 0.5) / w as f32 - 1.0;
            let ndc_y = 1.0 - 2.0 * (args.y + 0.5) / h as f32;
            let (origin, direction) = camera.ray(ndc_x, ndc_y, w as f32 / h as f32);
            let ray = Ray {
                origin: origin.as_dvec3(),
                direction: direction.as_dvec3(),
            };
            let hit = session.pick_with_previews(&ray, args.rope_preview, args.instance_contents);
            Ok(hit.map_or(json!({ "hit": null }), |hit| {
                hit_json(session.render_document(), &hit)
            }))
        })
    }

    #[tool(
        description = "Cast a ray and report the first visible object and face hit, e.g. to find the floor below a point. Visible referenced contents select their root instance owner with null solid/side; child geometry is never an editable root face.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn raycast(&self, Parameters(args): Parameters<RaycastArgs>) -> ToolResult {
        run(|| {
            if vec3(args.direction).length_squared() == 0.0 {
                return Err("direction must not be zero".into());
            }
            let mut session = self.session();
            let ray = Ray {
                origin: vec3(args.origin),
                direction: vec3(args.direction),
            };
            let hit = session.pick(&ray);
            Ok(hit.map_or(json!({ "hit": null }), |hit| {
                hit_json(session.render_document(), &hit)
            }))
        })
    }

    #[tool(
        description = "Inspect depth-ordered object hits along an explicit world ray without changing selection/history. Returns at most 64 candidates, exact total/truncation, hit object/brush/side/point/distance, brush owner and selection_unit. Shared mode deduplicates to outermost visible nested groups, whole objects or individual solids before the cap; Solids keeps individual brush-entity children. Type/hidden/tool filters match ordinary selection. Usable model triangles replace helper boxes. Stable IDs break equal-depth ties. Requires viewport Objects mode; specialized tools and geometry previews reject.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_object_hit_candidates(
        &self,
        Parameters(args): Parameters<ObjectHitArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().object_hit_candidates(&Ray {
                origin: vec3(args.origin),
                direction: vec3(args.direction),
            })
        })
    }

    #[tool(
        description = "Cycle object selection along an explicit world ray through the shared Alt-click operation. Replace-only and stateless: if the current selection interpreted in the active groups/objects/solids mode equals a candidate unit, advance and wrap; otherwise select the nearest. Empty rays preserve selection. Recomputes visible brush/model geometry, type filters and selection rules each call; returns chosen zero-based index plus candidates/total/truncation. Solids mode distinguishes brush-entity children and reports their owner. Limited to the nearest 64 deduplicated units. No source/Undo edit; incompatible tools/previews reject."
    )]
    pub(in crate::mcp) fn cycle_object_hit(
        &self,
        Parameters(args): Parameters<ObjectHitArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().cycle_object_hit(
                Actor::Agent,
                &Ray {
                    origin: vec3(args.origin),
                    direction: vec3(args.direction),
                },
            )
        })
    }
}
