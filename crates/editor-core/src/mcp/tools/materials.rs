use super::super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MagneticSourceArgs {
    solid: u32,
    sides: Vec<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MagneticSettingsArgs {
    enabled: bool,
    radius: f64,
    source_corner: Option<MagneticSourceArgs>,
    snapshot: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MagneticPreviewArgs {
    translation: [f64; 3],
    ray_origin: [f64; 3],
    ray_direction: [f64; 3],
    snapshot: String,
}

#[tool_router(router = materials_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect opt-in magnetic Move snapping, world-unit radius, chosen source corner, source staleness, exact current snapshot and current preview token/result. Captures the nearest real corner of the first visible brush face or prepared terrain/model hit triangle only inside radius from the source after ordinary constrained/grid movement. Reports captured, outside_radius or no_surface with requested/final translation, target identity/provenance and distance. Capturing may leave the handle axis/plane; no through-wall or scene-wide search. Settings remain temporary and do not save automatically.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_magnetic_snapping(&self) -> ToolResult {
        ok(self.session().magnetic_snapping())
    }

    #[tool(
        description = "List the same explicit Source corner choices as Magnetic snapping in the Inspector. Uses current selection, including individual entity children, whole world/brush entities and point/model companions. Only visible flat brushes supply source corners. Reuses checked topology/precise intersections and whole-selection bounds admission. Returns current snapshot and up to 256 source brushes/4096 corners with owner, solid, incident sides and authored position. Does not alter selection/source/history/previews.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_magnetic_snap_sources(&self) -> ToolResult {
        run(|| self.session().magnetic_snap_sources())
    }

    #[tool(
        description = "Apply the shared temporary magnetic settings using latest get_magnetic_snap_sources.snapshot (or get_magnetic_snapping.snapshot for the same current source). Required enabled/radius/snapshot; source_corner={solid,sides} is required when enabling. Radius is 0.125–1024 world units, default 16. All selected roots/individual children translate together; source must belong to them, and mixed owner/child selections reject. Selected hidden roots, invalid source topology/identity/bounds or stale snapshots reject without altering settings or previews. Actual setting changes retire active magnetic/Move gestures; no-op settings preserve them. UI: Magnetic snapping… → Refresh magnetic sources / Source corner / Magnetic snapping / Capture radius / Apply magnetic settings."
    )]
    pub(in crate::mcp) fn set_magnetic_snapping(
        &self,
        Parameters(args): Parameters<MagneticSettingsArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().set_magnetic_snapping(
                Actor::Agent,
                args.enabled,
                args.radius,
                args.source_corner
                    .map(|source| map_core::subobject::Component {
                        solid: source.solid,
                        sides: source.sides,
                    }),
                &args.snapshot,
            )
        })
    }

    #[tool(
        description = "Preview a rigid translation of the current root/child selection using the same magnetic planner as pointer-driven Move handles. Pass translation in world units, explicit target ray and latest get_magnetic_snapping.snapshot. The supplied translation is the ordinary requested result; this operation performs no additional grid rounding. Returns an exact preview token plus captured/outside_radius/no_surface and final offset. Invalid rays/source/target geometry preserve the previous preview. Excludes moving objects/brushes from targets, validates all source/final bounds, and changes no source/history. Apply with commit_magnetic_translation/token (or shared Apply transform); Cancel uses cancel_magnetic_translation/token. Source/selection/visibility/assets/settings changes prevent stale Apply."
    )]
    pub(in crate::mcp) fn preview_magnetic_translation(
        &self,
        Parameters(args): Parameters<MagneticPreviewArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().preview_magnetic_translation(
                Actor::Agent,
                vec3(args.translation),
                Ray {
                    origin: vec3(args.ray_origin),
                    direction: vec3(args.ray_direction),
                },
                &args.snapshot,
            )
        })
    }

    #[tool(
        description = "Apply exactly the current magnetic preview token through the shared transform commit, in one Undo step. Stale/replaced tokens and changed source/target/visibility/settings reject; an identical final pose creates no history. UI: Apply transform or releasing the Move handle."
    )]
    pub(in crate::mcp) fn commit_magnetic_translation(
        &self,
        Parameters(args): Parameters<VolumeTokenArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .commit_magnetic_translation(Actor::Agent, parse_token(&args.token)?)
        })
    }

    #[tool(
        description = "Cancel the exact current magnetic preview token without changing source/history or settings. Stale but still owned previews can be cancelled; an old token cannot cancel replacement work. UI: Cancel transform / Escape."
    )]
    pub(in crate::mcp) fn cancel_magnetic_translation(
        &self,
        Parameters(args): Parameters<VolumeTokenArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .cancel_magnetic_translation(parse_token(&args.token)?)
        })
    }

    #[tool(
        description = "Inspect the same bounded committed visible source-corner choices as Place on surface… → Snap selected corner → Source corner. Optional whole-root ids default to the selection; inspection never changes selection, map, history or active drafts. Returns canonical ids, sources with owner ID, numeric solid, complete incident sides and exact unrounded world position, committed_geometry provenance and limits of 256 visible flat brushes / 4096 checked corners. Source eligibility shares the human chooser's topology, whole-root, identity, visibility and bounds checks. Point/model companions can move but supply no source corner. Invalid roots or exceeded limits return an error without a partial list or draft mutation. Pass one returned solid/sides pair as source_corner to the existing shared placement preview or pick operation.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_surface_placement_sources(
        &self,
        Parameters(args): Parameters<SurfacePlacementSourcesArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let mut ids = ids_or_selection(&session, args.ids.as_deref())?;
            ids.sort_unstable();
            ids.dedup();
            let sources = session.surface_placement_source_corners(&ids)?;
            Ok(json!({
                "ids":ids.iter().copied().map(id_string).collect::<Vec<_>>(),
                "sources":sources.into_iter().map(|source|json!({
                    "owner":id_string(source.owner), "solid":source.component.solid,
                    "sides":source.component.sides, "position":source.position.to_array(),
                })).collect::<Vec<_>>(),
                "provenance":"committed_geometry",
                "limits":{"source_brushes":crate::session::placement::MAX_SOURCE_BRUSHES,
                    "source_corners":crate::session::placement::MAX_SOURCE_CORNERS},
            }))
        })
    }

    #[tool(
        description = "Read the selected material's mounted VMT source and winning directory/file or VPK index/storage paths through the same synchronous Session operation as Browse materials → Source…. Returns up to 64 KiB of raw UTF-8 source with explicit truncation, up to nine source/include lookups, resolved Patch shader declaration and recognized texture parameter values. Ordinary names are declarations, not proof of asset availability or rendering. Resolved Water, Water_DX90 and Water_DX9_HDR also return water_dependencies: a one-material SDK-qualified static inventory with typed roles/defaults/runtime resources, file origins/bytes/SHA256, existing VTF parser header observations, contextual issues and explicit supported_static_inventory, unsupported, unavailable_source or limited state; non-Water is not_applicable. Raw/resolved source and Water inspection use the same original-read capture; supported inventories verify those original mounted source identities before return. This historical observation grants no rendering readiness, map portability, packing or durable source token. Root source failures remain report.error and never trigger a collector repair read; Water-only failures leave ordinary source declarations available. No lower-priority fallback or preview/cache mutation. Each VMT is limited to 1 MiB, aggregate VMT reads to 4 MiB, paths to 512 bytes, nesting to 64 and nodes to 4096. The Water graph allows nine VMTs total, 64 assets, 128 edges, 16 issues, 16 MiB per VTF and 64 MiB total source payload; no pixel decoding or GPU upload occurs. VPK provenance requires the mounted index to remain current. Does not edit the map, selection, history, camera or active drafts.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_material_source(
        &self,
        Parameters(args): Parameters<MaterialSourceArgs>,
    ) -> ToolResult {
        run(|| {
            let report = self.session().inspect_material_source(&args.material)?;
            serde_json::to_value(report).map_err(|error| error.to_string())
        })
    }
    #[tool(
        description = "Reload cached VMT/VTF display assets and sky faces, rescan the material catalog, and re-resolve cached model material paths from existing mounts without replacing decoded geometry. Retains material IDs, source/history/camera and active geometry previews; refreshes desktop/offscreen textures. Returns reloaded, models_refreshed, models_changed, and unavailable-preview failures (first 100) with total failed count. Model path errors replace stale bindings and appear in inspect_entity_model.material_error. Unavailable images replace stale textures; valid untextured/tool materials may have no display texture. Viewport snapshots may require renewed review. Does not remount archives, watch files or reload particle definitions. UI: Browse materials… → Reload material assets."
    )]
    pub(in crate::mcp) fn reload_material_assets(&self) -> ToolResult {
        let report = self.session().reload_material_assets(Actor::Agent);
        ok(json!(report))
    }

    #[tool(
        description = "Preview rigid surface placement of selected or explicit object ids together. method=floor (default) or ceiling casts vertically and preserves XY; method=ray uses ray_origin/ray_direction and centers the selection tangentially at the hit. Optional align rotates the first object's local up to the geometric normal; offset is normal distance, texture_lock defaults to current settings. Supports actual brush/terrain/model geometry and helper bounds for unsupported point assets. Returns owned token and contact/transform details; no source/history edit. method=brush_corner requires selected whole world brushes/entities and one explicit source_corner {solid,sides} from a visible flat brush owned by that selection; every root shares the same translation and all moving owners are excluded as targets; uses the ray to choose the nearest real corner of the first visible flat brush face or nearest of the first prepared terrain/model triangle's three vertices. Terrain reports kind=terrain_vertex, solid/side, triangle, row-major index, grid_size and prepared_surface provenance; brush targets retain solid/sides with kind=brush_corner. Model targets report kind=model_vertex with entity, cached model/checksum, body/skin/scale/pose, global part/vertex indices, part-local triangle and cached_decoded_model_mesh provenance. Translates only, preserving texture policy; align=true/nonzero offset and ambiguous targets reject without replacing a valid plan. Inspect source_position and plan.corner identities/positions before Apply. Finish other geometry previews first. UI: Place on surface… → Snap selected corner."
    )]
    pub(in crate::mcp) fn preview_surface_placement(
        &self,
        Parameters(args): Parameters<SurfacePlacementArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let ids = ids_or_selection(&session, args.ids.as_deref())?;
            let method = crate::session::placement::Method::parse(
                args.method.as_deref().unwrap_or("floor"),
            )?;
            let ray = match (args.ray_origin, args.ray_direction) {
                (Some(origin), Some(direction)) => Some(map_core::query::Ray {
                    origin: DVec3::from_array(origin),
                    direction: DVec3::from_array(direction),
                }),
                (None, None) => None,
                _ => return Err("Supply both ray_origin and ray_direction".into()),
            };
            let options = crate::session::placement::Options {
                source_corner: args
                    .source_corner
                    .map(|corner| map_core::subobject::Component {
                        solid: corner.solid,
                        sides: corner.sides,
                    }),
                align: args.align,
                offset: args.offset.unwrap_or(0.0),
                texture_lock: args
                    .texture_lock
                    .unwrap_or(session.transform_tool().texture_lock),
            };
            session.preview_surface_placement(Actor::Agent, &ids, method, ray, options)
        })
    }

    #[tool(
        description = "Arm shared viewport surface picking for selected/explicit ids, optional align, normal offset and texture_lock. Returns owned token without editing source/history. The next pick_surface_placement supplies the ray; missed/invalid hits keep picking active. method=brush_corner plus explicit source_corner {solid,sides} arms exact corner snapping; ray is the default method. Corner snapping targets visible flat brush corners, prepared terrain-triangle vertices or supported model mesh vertices and needs one chosen source corner in the moving whole-owner selection, zero offset and no alignment. UI: Place on surface… → Pick surface or Snap selected corner → Pick target corner."
    )]
    pub(in crate::mcp) fn begin_surface_placement_pick(
        &self,
        Parameters(args): Parameters<BeginSurfacePlacementPickArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let ids = ids_or_selection(&session, args.ids.as_deref())?;
            let method = args.method.as_deref().unwrap_or("ray");
            if !matches!(method, "ray" | "brush_corner")
                || (method == "brush_corner") != args.source_corner.is_some()
            {
                return Err("Choose ray without source_corner, or brush_corner with an explicit source_corner".into());
            }
            let options = crate::session::placement::Options {
                source_corner: args
                    .source_corner
                    .map(|corner| map_core::subobject::Component {
                        solid: corner.solid,
                        sides: corner.sides,
                    }),
                align: args.align,
                offset: args.offset.unwrap_or(0.0),
                texture_lock: args
                    .texture_lock
                    .unwrap_or(session.transform_tool().texture_lock),
            };
            session.begin_surface_placement_pick(&ids, options)
        })
    }

    #[tool(
        description = "Complete owned surface picking with a world origin/direction ray. Excludes selected objects and hidden/tool-filtered targets, uses geometric brush/terrain/model normals and installs the same transform preview as the viewport click. In brush_corner mode the first visible flat brush face, prepared terrain triangle or supported model mesh triangle supplies its nearest corner. Brush ties use incident-side identity; terrain ties use row-major vertex index and model ties use global vertex index within the hit triangle. Terrain positions match the prepared displayed surface, including mixed-power seams, without changing authored samples. Model vertices use the exact cached selected-body mesh with its existing positive uniform scale and Source pose; no helper or animation fallback. Unsupported hits keep picking active. No source/history edit."
    )]
    pub(in crate::mcp) fn pick_surface_placement(
        &self,
        Parameters(args): Parameters<SurfacePlacementPickArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().pick_surface_placement(
                Actor::Agent,
                args.token,
                map_core::query::Ray {
                    origin: DVec3::from_array(args.origin),
                    direction: DVec3::from_array(args.direction),
                },
            )
        })
    }

    #[tool(
        description = "Inspect the shared surface-placement token, validity, picking state, selection, options and last contact/transform plan. Corner target kind distinguishes brush_corner solid/sides from terrain_vertex solid/side/triangle/index/grid_size with prepared_surface provenance, or model_vertex cached asset/pose and part/triangle/vertex provenance; plan.corner.position is the exact chosen world point. Optional token checks ownership; stale plans remain inspectable/cancellable. UI: placement summary.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_surface_placement(
        &self,
        Parameters(args): Parameters<ModelChoiceQueryArgs>,
    ) -> ToolResult {
        run(|| {
            let report = self.session().surface_placement();
            if args
                .token
                .is_some_and(|token| report["token"].as_u64() != Some(token))
            {
                return Err("Surface placement was closed or replaced".into());
            }
            Ok(report)
        })
    }

    #[tool(
        description = "Apply only the captured valid surface-placement preview in one undoable transform, preserving unrelated data and chosen texture-lock policy. UI: Apply placement."
    )]
    pub(in crate::mcp) fn apply_surface_placement(
        &self,
        Parameters(args): Parameters<ModelChoiceTokenArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .apply_surface_placement(Actor::Agent, args.token)
        })
    }

    #[tool(
        description = "Cancel only the captured surface placement/picking token, including stale cleanup. Clears its owned transform only; a numeric/handle/other preview replacement is preserved. UI: Cancel/Escape."
    )]
    pub(in crate::mcp) fn cancel_surface_placement(
        &self,
        Parameters(args): Parameters<ModelChoiceTokenArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().cancel_surface_placement(args.token)?;
            Ok(json!({"cancelled":true}))
        })
    }

    #[tool(
        description = "Select individual brush faces using solid and side IDs from picking or object inspection. Shared with the user's face selection; replace/add/remove/toggle modes. Validates the whole batch."
    )]
    pub(in crate::mcp) fn select_faces(
        &self,
        Parameters(args): Parameters<SelectFacesArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let mode = match args.mode.as_deref().unwrap_or("replace") {
                "replace" => SelectMode::Replace,
                "add" => SelectMode::Add,
                "remove" => SelectMode::Remove,
                "toggle" => SelectMode::Toggle,
                _ => return Err("mode must be replace, add, remove or toggle".into()),
            };
            session.select_faces(&args.faces.iter().map(face_id).collect::<Vec<_>>(), mode)?;
            Ok(face_state(&session))
        })
    }

    #[tool(
        description = "Inspect shared selected faces, texture parameters, lightmap scales, smoothing groups, owners, active preview token and optional atlas rectangle/snapshot. Face IDs work for world brushes and brushes inside entities.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_face_selection(&self) -> ToolResult {
        ok(face_state(&self.session()))
    }

    #[tool(
        description = "Map one continuous texture across a face group. face fits combined coplanar bounds using a reference face's directions/mirroring. view fits a parallel projection using the current camera right/up axes (no perspective distortion). repeats controls tiles across the whole group. Uses actual material dimensions; mixed materials need an explicit replacement. Rejects edge-on targets atomically. UVs/material change; lighting and geometry remain intact. Preview shares token-guarded commit_face_preview/cancel_face_preview."
    )]
    pub(in crate::mcp) fn project_face_group(
        &self,
        Parameters(args): Parameters<ProjectFaceGroupArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let faces = args.faces.as_ref().map_or_else(
                || session.selected_faces(),
                |faces| faces.iter().map(face_id).collect(),
            );
            let projection = match args.projection.as_deref().unwrap_or("face") {
                "face" => {
                    crate::session::GroupProjection::Face(args.reference.as_ref().map(face_id))
                }
                "view" if args.reference.is_none() => crate::session::GroupProjection::View,
                "view" => return Err("reference is only used for face projection".into()),
                _ => return Err("projection must be face or view".into()),
            };
            let changed = session.project_face_group(
                Actor::Agent,
                &faces,
                projection,
                args.material.as_deref(),
                glam::DVec2::from_array(args.repeats.unwrap_or([1.0, 1.0])),
                args.preview,
            )?;
            Ok(json!({"faces_changed":changed,"state":face_state(&session)}))
        })
    }

    #[tool(
        description = "Find faces with an exact material path in map, selected_objects or selected_faces. Case, backslashes, materials/ prefix and .vmt suffix are normalized; no wildcards. Hidden world brushes, entities and child brushes are excluded unless include_hidden=true. Stores a shared report and returns its token, counts and first 100 faces; no map or selection edit."
    )]
    pub(in crate::mcp) fn find_material_faces(
        &self,
        Parameters(query): Parameters<crate::session::material_search::Query>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.find_material_faces(query)?;
            Ok(session.material_search_json(0, 100))
        })
    }

    #[tool(
        description = "Inspect the shared material search report and stale status. Paginated faces, limit 1–500 (default 100).",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_material_search(
        &self,
        Parameters(args): Parameters<MaterialSearchPageArgs>,
    ) -> ToolResult {
        ok(self
            .session()
            .material_search_json(args.offset.unwrap_or(0), args.limit.unwrap_or(100)))
    }

    #[tool(
        description = "Select all faces in a current material search, across all result pages. Requires report token, supports replace/add/remove/toggle. Hidden faces remain hidden. Selection-scoped results can become stale when selection changes; find again before replacement."
    )]
    pub(in crate::mcp) fn select_material_faces(
        &self,
        Parameters(args): Parameters<SelectMaterialFacesArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let mode = match args.mode.as_deref().unwrap_or("replace") {
                "replace" => SelectMode::Replace,
                "add" => SelectMode::Add,
                "remove" => SelectMode::Remove,
                "toggle" => SelectMode::Toggle,
                _ => return Err("mode must be replace, add, remove or toggle".into()),
            };
            let count = session.select_material_faces(parse_token(&args.token)?, mode)?;
            Ok(json!({"matched":count, "state":face_state(&session)}))
        })
    }

    #[tool(
        description = "Replace every matching face from a current material search in one undo step. Requires report token; rejects changed map/selection or replaced reports. Changes material only, preserving UVs, lighting, displacements and hidden flags. preview=true uses shared face preview with token-guarded commit/cancel. Exact replacement; no substring substitution."
    )]
    pub(in crate::mcp) fn replace_materials(
        &self,
        Parameters(args): Parameters<ReplaceMaterialsArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let count = session.replace_materials(
                Actor::Agent,
                parse_token(&args.token)?,
                &args.material,
                args.preview,
            )?;
            Ok(json!({"faces_changed":count, "state":face_state(&session)}))
        })
    }

    #[tool(
        description = "Add reproducible per-face U/V texel-offset variation. seed is an integer 0–4294967295; max_shift_u/v are finite nonnegative maximum absolute shifts in texels. Omitted faces use shared selection. A stable digest of seed and solid/side IDs gives independent shifts within each signed maximum, added to committed offsets; face order and repeated IDs do not affect results. Same source and seed reproduce the same preview; reapplying after commit accumulates. Preserves axes, skew, scale, rotation, material, lighting, terrain and unknown data. Zero maxima return faces_changed=0 without replacing an existing draft. Invalid batches preserve source/history and any previous preview. preview=true uses the shared owned face draft; finish with token-guarded commit_face_preview/cancel_face_preview. Otherwise applies atomically in one Undo step. UI: Edit face textures… → Vary texture offsets → Preview variation."
    )]
    pub(in crate::mcp) fn vary_face_uv(
        &self,
        Parameters(args): Parameters<VaryFaceUvArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let faces = args.faces.as_ref().map_or_else(
                || session.selected_faces(),
                |faces| faces.iter().map(face_id).collect(),
            );
            let previous = session.face_preview_token();
            let changed = session.vary_face_uv(
                Actor::Agent,
                &faces,
                args.seed,
                args.max_shift_u,
                args.max_shift_v,
                args.preview,
            )?;
            let created = args.preview && session.face_preview_token() != previous;
            Ok(json!({"faces_changed":changed,"preview_created":created,
                "preview_token":created.then(|| session.face_preview_token().unwrap().to_string()),
                "state":face_state(&session)}))
        })
    }

    #[tool(
        description = "Edit materials and texture offsets (texels), scales (world units per texel) and absolute rotation (degrees about the face normal). Lightmap scale is 1–1024 world units per lighting pixel. Smoothing group lists replace/add/remove memberships (1–24 smooth, 25–32 hard-edge); empty replacement clears. Direct u_axis/v_axis world vectors allow skew without normalization; null components preserve each face's component. Vectors cannot combine with alignment or rotation, and must produce a finite nonsingular projection on every target face. Scales and texel offsets remain independent. Omitted values preserve each face's original value. Omitted faces use shared face selection. Fit maps one material tile onto each face's projected bounds using actual texture dimensions. Preview renders a draft without changing the map or undo history; commit or cancel it with the preview tools. Atomic, undoable when applied."
    )]
    pub(in crate::mcp) fn edit_faces(
        &self,
        Parameters(args): Parameters<EditFacesArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let faces = args.faces.as_ref().map_or_else(
                || session.selected_faces(),
                |faces| faces.iter().map(face_id).collect(),
            );
            let changed = session.edit_faces(
                Actor::Agent,
                &faces,
                map_core::surface::SurfaceEdit {
                    material: args.material,
                    shift_u: args.shift_u,
                    shift_v: args.shift_v,
                    scale_u: args.scale_u,
                    scale_v: args.scale_v,
                    rotation: args.rotation,
                    u_axis: args.u_axis,
                    v_axis: args.v_axis,
                    lightmap_scale: args.lightmap_scale,
                    smoothing_groups: args
                        .smoothing_groups
                        .as_deref()
                        .map(map_core::surface::smoothing_mask)
                        .transpose()
                        .map_err(|error| error.to_string())?,
                    smoothing_add: map_core::surface::smoothing_mask(
                        args.smoothing_add.as_deref().unwrap_or_default(),
                    )
                    .map_err(|error| error.to_string())?,
                    smoothing_remove: map_core::surface::smoothing_mask(
                        args.smoothing_remove.as_deref().unwrap_or_default(),
                    )
                    .map_err(|error| error.to_string())?,
                    alignment: match args.alignment.as_deref() {
                        None => None,
                        Some("world") => Some(map_core::surface::Alignment::World),
                        Some("face") => Some(map_core::surface::Alignment::Face),
                        _ => return Err("alignment must be world or face".into()),
                    },
                    justify_u: texture_justification(args.justify_u.as_deref(), false)?,
                    justify_v: texture_justification(args.justify_v.as_deref(), true)?,
                    ..Default::default()
                },
                args.fit.unwrap_or(false),
                args.preview.unwrap_or(false),
            )?;
            Ok(json!({"faces_changed": changed, "state": face_state(&session)}))
        })
    }

    #[tool(
        description = "Sample committed material and UV projection from one face into the shared texture sample. Select a source, sample it, then select targets and call paint_face_texture. The sample is a snapshot, survives selection/source edits, and clears on map replacement. Sampling cancels existing face and texture-stroke previews without map or undo edits."
    )]
    pub(in crate::mcp) fn sample_face_texture(
        &self,
        Parameters(args): Parameters<SampleFaceArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let face = match args.face {
                Some(face) => face_id(&face),
                None => match session.selected_faces().as_slice() {
                    [face] => *face,
                    _ => return Err("select exactly one source face or provide face".into()),
                },
            };
            session.sample_face_texture(face)?;
            Ok(json!({"sample": surface_sample_state(&session)}))
        })
    }

    #[tool(
        description = "Read the shared sampled material/projection, including original geometry and whether its source still matches. Null means no sample. Sampling does not automatically follow later source edits."
    )]
    pub(in crate::mcp) fn get_surface_sample(&self) -> ToolResult {
        ok(json!({"sample": surface_sample_state(&self.session())}))
    }

    #[tool(
        description = "Propagate a sampled texture through up to 256 connected target faces, folding its UV projection at each shared edge. Unlike single-edge wrap, targets can connect through other selected targets. Returns deterministic parent/face links and the shared face-preview token. Rejects disconnected targets, displacements and loops with incompatible seams; select an open strip to leave a seam. Source included in targets is skipped. Preserves geometry, lighting metadata and selection; preview/commit/cancel and one-step undo match Wrap connected faces in the UI. Invalid batches preserve the previous draft."
    )]
    pub(in crate::mcp) fn propagate_face_texture(
        &self,
        Parameters(args): Parameters<TextureChainArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let faces = args
                .faces
                .as_ref()
                .map(|faces| faces.iter().map(face_id).collect::<Vec<_>>())
                .unwrap_or_else(|| session.selected_faces());
            let result = session.propagate_face_texture(Actor::Agent, &faces, args.preview)?;
            let face_json =
                |face: map_core::surface::FaceId| json!({"solid":face.solid,"side":face.side});
            Ok(
                json!({"faces_changed":result.changed,"source_in_targets":result.source_in_targets,"links":result.links.into_iter().map(|link|json!({"parent":face_json(link.parent),"face":face_json(link.face)})).collect::<Vec<_>>(),"max_targets":map_core::texture_chain::MAX_FACES,"state":face_state(&session)}),
            )
        })
    }

    #[tool(
        description = "Paint sampled material onto target faces in one undo step. projection also copies UV vectors/scales/offsets and rejects projections that collapse on the target. wrap rotates around an actual shared edge and preserves its texel coordinates; displacements and unrelated faces are rejected. Omitted targets use selected faces. A failed batch changes nothing; preview shares commit_face_preview/cancel_face_preview and is invalidated by map or selection edits."
    )]
    pub(in crate::mcp) fn paint_face_texture(
        &self,
        Parameters(args): Parameters<PaintFacesArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let faces = args.faces.as_ref().map_or_else(
                || session.selected_faces(),
                |faces| faces.iter().map(face_id).collect(),
            );
            let mode = match args.mode.as_deref().unwrap_or("material") {
                "material" => map_core::surface::PaintMode::Material,
                "projection" => map_core::surface::PaintMode::Projection,
                "wrap" => map_core::surface::PaintMode::Wrap,
                _ => return Err("mode must be material, projection or wrap".into()),
            };
            let changed = session.paint_face_texture(Actor::Agent, &faces, mode, args.preview)?;
            Ok(json!({"faces_changed": changed, "state": face_state(&session)}))
        })
    }

    #[tool(
        description = "Apply the shared face material/lighting/terrain preview in one undo step. Pass its token to reject a replaced preview. Fails if edits, document replacement or selection changes have invalidated it. Atlas drafts require the exact token, originating actor and current material snapshot; rejected Apply preserves the draft."
    )]
    pub(in crate::mcp) fn commit_face_preview(
        &self,
        Parameters(args): Parameters<FacePreviewArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session
                .check_face_preview_token(args.token.as_deref().map(parse_token).transpose()?)?;
            let changed = session.commit_face_preview(Actor::Agent)?;
            Ok(json!({"handle": session.face_preview_handle(), "faces_changed": changed}))
        })
    }

    #[tool(
        description = "Cancel the shared face material/lighting preview without changing the document or undo/redo history. Pass its token to reject a replaced preview. Atlas drafts require their exact token and originating actor; their stale assets do not prevent owned Cancel. Equivalent to the editing dialog's Cancel button."
    )]
    pub(in crate::mcp) fn cancel_face_preview(
        &self,
        Parameters(args): Parameters<FacePreviewArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.cancel_owned_face_preview(
                Actor::Agent,
                args.token.as_deref().map(parse_token).transpose()?,
            )?;
            Ok(json!({"handle": session.face_preview_handle(), "preview": false}))
        })
    }

    #[tool(
        description = "Begin a sampled texture stroke over nearest visible faces. First call sample_face_texture. material preserves target UVs; projection copies the captured material and UV projection and rejects collapsed mappings. Same circular sampler, radius, camera/viewport and limits as paint selection. Returns a shared token and rendered preview; map, selection and undo remain unchanged. Invalid starts preserve the existing preview."
    )]
    pub(in crate::mcp) fn begin_texture_stroke(
        &self,
        Parameters(args): Parameters<BeginTextureStrokeArgs>,
    ) -> ToolResult {
        run(|| {
            let pointer = args.pointer.resolve()?;
            let mut session = self.session();
            let camera = args
                .camera
                .unwrap_or_default()
                .resolve(&session, pointer.size.0 as f32 / pointer.size.1 as f32)?;
            let operation = args
                .mode
                .as_deref()
                .map(crate::session::paint::Operation::parse)
                .transpose()?
                .unwrap_or_else(|| {
                    if session.paint_operation() == crate::session::paint::Operation::Projection {
                        crate::session::paint::Operation::Projection
                    } else {
                        crate::session::paint::Operation::Material
                    }
                });
            let radius = args.radius.unwrap_or(session.paint_radius());
            session.begin_texture_stroke(
                crate::session::paint::Spec {
                    camera,
                    pointer,
                    radius,
                    faces: true,
                    mode: SelectMode::Replace,
                },
                operation,
            )?;
            Ok(session.paint_selection_json(0, 100))
        })
    }

    #[tool(
        description = "Extend the exact sampled texture stroke through ordered pixel points. Frontmost faces are deduplicated; only new faces are painted in the draft. Invalid projection, point or budget updates preserve the previous preview atomically. The stroke retains its captured sample; sampling a different source invalidates it."
    )]
    pub(in crate::mcp) fn update_texture_stroke(
        &self,
        Parameters(args): Parameters<UpdatePaintArgs>,
    ) -> ToolResult {
        run(|| {
            let points = args
                .points
                .into_iter()
                .map(DVec2::from_array)
                .collect::<Vec<_>>();
            let mut session = self.session();
            session.update_texture_stroke(parse_token(&args.token)?, &points)?;
            Ok(session.paint_selection_json(0, 100))
        })
    }

    #[tool(
        description = "Inspect shared brush operation/radius, texture-stroke token, captured source/material, changed-face count, camera, cursor, limits and paginated candidates. operation distinguishes texture and selection strokes. Use render_view for the draft and get_surface_sample for the source projection.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_texture_stroke(
        &self,
        Parameters(args): Parameters<MaterialSearchPageArgs>,
    ) -> ToolResult {
        ok(self
            .session()
            .paint_selection_json(args.offset.unwrap_or(0), args.limit.unwrap_or(100)))
    }

    #[tool(
        description = "Commit the exact texture stroke as one atomic face edit, preserving selection, geometry, unrelated UV/lighting metadata and hidden faces. The applied result matches the preview. Empty or unchanged strokes add no undo step; stale or selection-stroke tokens are rejected. Equivalent to releasing a texture drag or Apply textures."
    )]
    pub(in crate::mcp) fn commit_texture_stroke(
        &self,
        Parameters(args): Parameters<VolumeTokenArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let changed = session.commit_texture_stroke(Actor::Agent, parse_token(&args.token)?)?;
            Ok(json!({"faces_changed":changed,"state":session.paint_selection_json(0,100)}))
        })
    }

    #[tool(
        description = "Discard the exact texture-stroke preview and brush outline without changing map, selection or undo history. A stale token cannot cancel another client's newer stroke."
    )]
    pub(in crate::mcp) fn cancel_texture_stroke(
        &self,
        Parameters(args): Parameters<VolumeTokenArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let token = parse_token(&args.token)?;
            session.cancel_texture_stroke(token)?;
            Ok(session.paint_selection_json(0, 100))
        })
    }

    #[tool(
        description = "Browse the shared material catalog, bounded to 200 entries per page and 256 game mounts. match_mode is substring (default), literal prefix or regex over material names, case-insensitive by default. Regex uses the regex crate syntax, at most 256 pattern bytes, depth 32 and 1 MiB compiler/DFA budgets; invalid/unsupported patterns reject. Optional shader is an exact case-insensitive resolved shader name, including Patch resolution. Each shader query classifies at most 128 materials and reads at most 16 MiB; repeat the same query while shader_scan.complete is false. Counts/pages include confirmed matches so far; shader_scan.matches_complete is true only after the complete scan without unresolved materials. Failures include total counts and the first 16 material/error rows, never silent nonmatches. Classified entries report shader, declaration identity and its captured VMT origin. Shader declarations remain cached until Reload material assets; current source metadata is separate and observed mount changes invalidate classifications. Sources select current winning VMT directory/VPK origins, never shadowed copies or assumed stock content. Optionally exclude all maps/... paths, including authored ones. Groups include all, used, recent, favorites, folder:<path> and category:<label>. UI: Browse materials → Name match / Shader. No map/history/selection/camera/draft changes.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn browse_materials(
        &self,
        Parameters(args): Parameters<BrowseMaterialsArgs>,
    ) -> ToolResult {
        run(|| {
            let catalog = self.session().material_catalog_filtered(
                args.query.as_deref().unwrap_or(""),
                args.group.as_deref().unwrap_or("all"),
                args.offset.unwrap_or(0),
                args.limit.unwrap_or(36),
                &crate::materials::CatalogFilter {
                    source: args.source,
                    exclude_generated_map_materials: args.exclude_generated_map_materials,
                    match_mode: args.match_mode,
                    shader: args.shader,
                },
            )?;
            serde_json::to_value(catalog).map_err(|error| error.to_string())
        })
    }

    #[tool(
        description = "Reload material favorites from the configured saved store after another editor changed it or storage was repaired. Replaces only the valid saved favorites/source and clears its conflict/error. Invalid Reload preserves the previous favorites and blocks persistent writes until repaired Reload. Does not reload material assets, initialize graphics, change map/history/selection/camera/view or apply/cancel an owned draft. Session-only favorites remain editable when no store exists."
    )]
    pub(in crate::mcp) fn reload_material_favorites(&self) -> ToolResult {
        run(|| {
            self.session()
                .reload_material_favorites()
                .map(|status| json!({"favorites":status}))
        })
    }

    #[tool(
        description = "Add or remove a material favorite shared with the GUI catalog. Favorites persist in the configured recovery directory; sessions without a store keep local favorites. Paths are canonicalized and bounded to 256 bytes; at most 512 favorites. Exact-source writes and a stable Linux lock reject stale sessions; use reload_material_favorites before retrying conflicts. Pre-publication failures preserve previous favorites; a written-but-unverified outcome adopts the published set and blocks until Reload. Recent applications remain session-local."
    )]
    pub(in crate::mcp) fn set_material_favorite(
        &self,
        Parameters(args): Parameters<FavoriteMaterialArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.favorite_material(&args.material, args.favorite)?;
            let favorites = session.material_favorites_snapshot();
            Ok(
                json!({"material": crate::material_favorites::canonical_name(&args.material)?,
                "favorite": favorites.contains(&args.material),
                "favorites": favorites.status}),
            )
        })
    }

    #[tool(
        description = "Apply a material to whole objects or to individual faces (defaults to the selection)."
    )]
    pub(in crate::mcp) fn set_material(
        &self,
        Parameters(args): Parameters<SetMaterialArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let mut targets: Vec<FaceTarget> = args
                .faces
                .iter()
                .flatten()
                .map(|f| FaceTarget::Side {
                    solid: f.solid,
                    side: f.side,
                })
                .collect();
            if args.ids.is_some() {
                targets.extend(
                    ids_or_selection(&session, args.ids.as_deref())?
                        .into_iter()
                        .map(FaceTarget::Object),
                );
            }
            if args.ids.is_none() && args.faces.is_none() {
                targets = session.material_targets();
            }
            if targets.is_empty() {
                return Err("select objects or faces to apply a material".into());
            }
            let changed = session.apply_material(Actor::Agent, &targets, &args.material)?;
            Ok(json!({ "faces_changed": changed }))
        })
    }

    #[tool(
        description = "Search the same catalog as Browse materials. match_mode selects substring (default), literal prefix or bounded regex; shader optionally selects an exact resolved shader name, including Patch materials. Returns compatible material names plus provenance/classification entries, sources and shader_scan. Repeat while shader_scan.complete is false: each operation classifies at most 128 candidates / 16 MiB. Before completion counts/pages describe confirmed matches only; failed classifications stay explicit and make matches_complete false. Queries cap at 256 bytes; regex depth and compiler/DFA memory are bounded. Winning-source and maps-path filters compose with both matchers. Cached shader declarations refresh with Reload material assets; current VMT provenance does not describe cached display pixels. No map/history/selection/camera/draft changes.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn search_materials(
        &self,
        Parameters(args): Parameters<SearchMaterialsArgs>,
    ) -> ToolResult {
        run(|| {
            let catalog = self.session().material_catalog_filtered(
                &args.query,
                "all",
                args.offset.unwrap_or(0),
                args.limit.unwrap_or(50),
                &crate::materials::CatalogFilter {
                    source: args.source,
                    exclude_generated_map_materials: args.exclude_generated_map_materials,
                    match_mode: args.match_mode,
                    shader: args.shader,
                },
            )?;
            let names = catalog
                .entries
                .iter()
                .map(|entry| &entry.name)
                .collect::<Vec<_>>();
            Ok(
                json!({ "materials": names, "entries":catalog.entries, "sources":catalog.sources,
                "total":catalog.total,"offset":catalog.offset,"filter":catalog.filter,
                "shader_scan":catalog.shader_scan,
                "provenance_scope":catalog.provenance_scope }),
            )
        })
    }

    #[tool(
        description = "Inspect the material browser's selected image as a CPU PNG. Optional layer=1 (default editor display) or layer=2 (actual WorldVertexTransition $basetexture2). Layer 2 reports its own original dimensions, mip and texture path captured with the cached declaration/image; absent, unsupported or unavailable second layers reject and never substitute layer 1. channel=color (default) returns unchanged RGBA; alpha shows source alpha bytes as opaque grayscale. Reports decoded-mip alpha range and the resolved VMT identity captured with those pixels. Cached display data stays paired until Reload material assets; uncached materials use one current load. No fresh VMT is paired with an old image. This inspects individual image data, not shader opacity, layer mixing, transforms or animation. Missing primary images produce no substitute PNG; read/decode errors fail explicitly. UI: Browse materials → Layer 1 / Layer 2 and Color / Alpha. No graphics startup or map/history/camera/selection/draft changes.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn preview_material(
        &self,
        Parameters(args): Parameters<MaterialPreviewArgs>,
    ) -> ToolResult {
        let layer = match crate::session::MaterialPreviewLayer::from_number(args.layer.unwrap_or(1))
        {
            Ok(layer) => layer,
            Err(error) => return fail(error),
        };
        let preview = match self.session().material_preview_layer_channel(
            &args.material,
            layer,
            args.channel.unwrap_or_default(),
        ) {
            Ok(preview) => preview,
            Err(error) => return fail(error),
        };
        let mut content = vec![ContentBlock::text(json!({"material": preview.name, "width": preview.width, "height": preview.height, "missing": preview.missing, "tool": preview.tool, "channel":preview.channel, "layer":preview.layer.number(), "source":preview.source, "alpha_range":preview.alpha_range,"physical_origin":null,"physical_origin_status":"unavailable: physical paths were not captured with these display pixels"}).to_string())];
        if let Some(image) = preview.image {
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
}
