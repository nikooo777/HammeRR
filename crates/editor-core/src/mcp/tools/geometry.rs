use super::super::*;

#[tool_router(router = geometry_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Draw and extrude a 3–64-corner simple polygon into editable convex brushes. outline contains world-plane pairs (YZ/XZ/XY for axis x/y/z); base and positive depth give axial placement. Convex outlines produce one brush; concave outlines decompose without filling their concavity. Repeats, crossings, touching/collinear edges and invalid dimensions fail atomically. snap defaults to shared grid settings. preview=true shares the existing owned primitive preview/inspect/frame/commit/cancel lifecycle; applied edits use one Undo step. UI: Draw polygon…."
    )]
    pub(in crate::mcp) fn create_polygon_brush(
        &self,
        Parameters(args): Parameters<PolygonArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let spec = map_core::primitive::PolygonSpec {
                outline: args
                    .outline
                    .iter()
                    .copied()
                    .map(glam::DVec2::from_array)
                    .collect(),
                axis: map_core::primitive::Axis::parse(args.axis.as_deref().unwrap_or("z"))
                    .map_err(|error| error.to_string())?,
                base: args.base,
                depth: args.depth,
                material: args.material.unwrap_or_else(|| "TOOLS/TOOLSNODRAW".into()),
            };
            let snap = args.snap.unwrap_or(session.spatial().snap);
            let ids = session.create_polygon(Actor::Agent, spec, snap, args.preview)?;
            let document = if args.preview {
                session.render_document()
            } else {
                session.document()
            };
            Ok(
                json!({"preview":args.preview,"objects":ids.iter().map(|&id|creation_summary(document,id)).collect::<Vec<_>>(),"state":session.primitive_state()}),
            )
        })
    }

    #[tool(
        description = "Create a wedge, cylinder, cone, faceted sphere, segmented arch or polygon in one undoable edit, or set preview=true for shared cancellable geometry. Polygon outline gives world-plane pairs and must agree with mins/maxs; create_polygon_brush calculates those bounds automatically. Curves are inscribed in mins/maxs; arch bounds describe the full ellipse and its sweep chooses the built portion. Axis controls extrusion/poles; wedges rise along the first transverse axis. get_primitive_preview returns actual bounds and a guarded token for commit/cancel/framing. Defaults to nodraw material."
    )]
    pub(in crate::mcp) fn create_primitive(
        &self,
        Parameters(args): Parameters<PrimitiveArgs>,
    ) -> ToolResult {
        run(|| {
            let spec = args.spec()?;
            let mut session = self.session();
            let ids = session.create_primitive(Actor::Agent, spec, args.preview)?;
            let doc = if args.preview {
                session.render_document()
            } else {
                session.document()
            };
            Ok(
                json!({"preview":args.preview,"objects":ids.iter().map(|&id|creation_summary(doc,id)).collect::<Vec<_>>(),"state":session.primitive_state()}),
            )
        })
    }

    #[tool(
        description = "Inspect the shared primitive preview, source generation/revision, parameters, temporary brush/face IDs, actual bounds and complexity limits. Preview geometry is used by rendering and ray queries; get_objects and compilation still inspect the applied map.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_primitive_preview(&self) -> ToolResult {
        ok(self.session().primitive_state())
    }

    #[tool(
        description = "Commit the exact primitive preview token as one undoable edit and select the generated brushes. Rejects closed, replaced or stale previews."
    )]
    pub(in crate::mcp) fn commit_primitive_preview(
        &self,
        Parameters(args): Parameters<MapFixTokenArgs>,
    ) -> ToolResult {
        run(|| {
            let token = args
                .token
                .parse()
                .map_err(|_| "Invalid primitive preview token")?;
            let mut session = self.session();
            let ids = session.commit_primitive_preview(Actor::Agent, token)?;
            Ok(
                json!({"objects":ids.iter().map(|&id|creation_summary(session.document(),id)).collect::<Vec<_>>(),"state":session.primitive_state()}),
            )
        })
    }

    #[tool(
        description = "Discard the exact primitive preview without changing applied geometry, selection or history. A replaced preview token cannot cancel another preview."
    )]
    pub(in crate::mcp) fn cancel_primitive_preview(
        &self,
        Parameters(args): Parameters<MapFixTokenArgs>,
    ) -> ToolResult {
        run(|| {
            let token = args
                .token
                .parse()
                .map_err(|_| "Invalid primitive preview token")?;
            let mut session = self.session();
            session.cancel_primitive_preview(token)?;
            Ok(session.primitive_state())
        })
    }

    #[tool(
        description = "Frame the exact primitive preview in the current projection without applying it or changing selection."
    )]
    pub(in crate::mcp) fn frame_primitive_preview(
        &self,
        Parameters(args): Parameters<PrimitiveFrameArgs>,
    ) -> ToolResult {
        run(|| {
            let token = args
                .token
                .parse()
                .map_err(|_| "Invalid primitive preview token")?;
            let mut session = self.session();
            session.frame_primitive_preview(token, args.aspect.unwrap_or(1.0))?;
            Ok(session.primitive_state())
        })
    }

    #[tool(
        description = "Inspect merge brush candidates, owners, bounds and materials, the shared preview token and snapshot stamp, resulting brush ID and per-face inheritance. 2–16 sources must form a convex gap-free union and share world ownership or one brush entity. Displacements reject. Read-only.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_merge_preview(&self) -> ToolResult {
        ok(self.session().merge_report())
    }

    #[tool(
        description = "Preview merging 2–16 explicit convex brush IDs into one brush. Requires latest get_merge_preview.stamp. Validates complete union coverage, rejecting gaps and concavities instead of silently filling a hull. World brushes may merge together; entity children must belong to one owner. Mixed owners and displacements reject. Lowest brush ID retains brush/editor metadata. Outer faces retain original IDs, material, world UV projection, lighting and unknown fields; lowest brush ID then side ID wins on coplanar faces. Internal faces disappear. Up to 64 outer sides and 256 proof fragments. Invalid requests preserve a valid draft. render_view and get_brush_components show the shared draft. Apply or cancel require its exact token."
    )]
    pub(in crate::mcp) fn preview_merge(
        &self,
        Parameters(args): Parameters<MergeArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.preview_merge(&args.sources, &args.stamp)?;
            Ok(session.merge_report())
        })
    }

    #[tool(
        description = "Apply the exact merge preview as one Undo step after rechecking full geometry and face inheritance. Preserves world/entity ownership and unrelated objects; selects the resulting brush or owning entity. Source edits, selection changes and competing geometry previews invalidate the token."
    )]
    pub(in crate::mcp) fn commit_merge(
        &self,
        Parameters(args): Parameters<VolumeTokenArgs>,
    ) -> ToolResult {
        run(|| {
            let result = self.session().commit_merge(
                Actor::Agent,
                args.token.parse().map_err(|_| "Invalid merge token")?,
            )?;
            Ok(crate::session::merging::result_json(&result))
        })
    }

    #[tool(
        description = "Discard the exact merge preview without changing map geometry, IDs or Undo history. Stale tokens cannot discard a replacement draft. Shares Discard preview and Cancel in Merge brushes."
    )]
    pub(in crate::mcp) fn cancel_merge(
        &self,
        Parameters(args): Parameters<VolumeTokenArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.cancel_merge(args.token.parse().map_err(|_| "Invalid merge token")?)?;
            Ok(session.merge_report())
        })
    }

    #[tool(
        description = "Inspect selected brush candidates and the shared carve preview, source owners and actual terrain-surface bounds/materials, separate carrier bounds, snapshot stamp, exact token, target IDs, resulting fragments, removed/unchanged brushes and face lineage. Reports frozen terrain_mode/blend_mode/partition_neighbors, actual terrain faces/powers/patch/sample counts, blend error bounds, side mappings, material/instance/cubemap guards and limits. Terrain targets require exactly one displaced face; cutters are ordinary convex brushes. Up to 8 retained cutters, 16 targets and 256 intermediate/final fragments. World brushes may disappear; empty brush entities reject. handle is null when closed or stale; stamp guards new requests. No map edits.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_carve_preview(&self) -> ToolResult {
        ok(self.session().carve_report())
    }

    #[tool(
        description = "Preview subtracting one retained ordinary convex cutter from explicit ordinary or terrain target brush IDs, including entity children. Requires latest get_carve_preview.stamp. Disjoint/tangent targets stay unchanged; an all-disjoint sheet/volume request rejects. cut_material defaults to cutter, copying new cap/carrier material/world UVs/lighting/metadata; target requires a unique original target face with matching outward normal; material overrides only cutter material. Terrain keeps its original surface settings and open cut border. terrain_mode is grid (default), flat, cells or fragments, using the existing clipping representation and native-field proofs; unsupported geometry/LOD refuses without fallback. blend_mode is exact (default), native_float or nearest_byte; non-exact requires fragments and reports certified error. partition_neighbors defaults false and explicitly authorizes required neighbors among target IDs; Flat refuses true. Material and visible instance sources are captured and frozen through Apply. Source/output terrain faces, real powers/counts, side_mapping and per-face geometry/surface donor lineage are reported. Original first-fragment IDs, owner entities and unrelated objects remain. Empty brush entities, terrain cutters, unsupported terrain profiles, invalid/thin geometry, 64-face overflow and budgets reject atomically while preserving an existing draft. render_view uses the draft; Apply/cancel require its exact token. Changed selection and competing tools invalidate it. Apply rechecks frozen sources and exact geometry as one Undo step."
    )]
    pub(in crate::mcp) fn preview_carve(
        &self,
        Parameters(args): Parameters<CarveArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.preview_carve_request(crate::session::carving::CarveRequest {
                cutters: &[args.cutter],
                targets: &args.targets,
                cut_material: &args.cut_material.into_policy(),
                terrain_mode: args
                    .terrain_mode
                    .map(ClipTerrainMode::mode)
                    .unwrap_or_default(),
                blend_mode: args.blend_mode.map(ClipBlendMode::mode).unwrap_or_default(),
                partition_neighbors: args.partition_neighbors.unwrap_or_default(),
                stamp: &args.stamp,
            })?;
            Ok(session.carve_report())
        })
    }

    #[tool(
        description = "Preview subtracting the combined volume of 1–8 retained ordinary convex cutters from 1–16 explicit ordinary or terrain target brushes. Roles must be disjoint; IDs normalize so cutter order cannot change fragments or lineage. Overlapping cutter volume is removed once. Requires latest get_carve_preview.stamp. Shares preview_carve material policies, grid/flat/cells/fragments terrain_mode, exact/native_float/nearest_byte blend_mode and explicit partition_neighbors permission. Omitted terrain fields mean grid/exact/false. Terrain targets require exactly one displaced face and keep source material/UV/editing fields; cut_material also controls new structural carrier faces, while native terrain borders stay open. A single selected representation applies throughout subtraction; proof/resource errors never choose another mode. Selected required neighbors must be target IDs and report as changes. Reports actual terrain faces, powers/counts, blend bounds, side_mapping, original geometry/surface donors and frozen material/instance/cubemap guards. All cutters and unrelated objects stay unchanged. At most 256 intermediate/final fragments, 64 sides per brush and the shared clipping terrain/native budgets. Empty owners, terrain cutters, unsupported profiles, invalid/thin geometry, all-disjoint results and exceeded budgets reject atomically, preserving map/IDs/history and the previous valid draft. render_view shares the draft; commit_carve/cancel_carve use its token. Apply replays exact geometry and guarded sources as one Undo step."
    )]
    pub(in crate::mcp) fn preview_carve_many(
        &self,
        Parameters(args): Parameters<CarveManyArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.preview_carve_request(crate::session::carving::CarveRequest {
                cutters: &args.cutters,
                targets: &args.targets,
                cut_material: &args.cut_material.into_policy(),
                terrain_mode: args
                    .terrain_mode
                    .map(ClipTerrainMode::mode)
                    .unwrap_or_default(),
                blend_mode: args.blend_mode.map(ClipBlendMode::mode).unwrap_or_default(),
                partition_neighbors: args.partition_neighbors.unwrap_or_default(),
                stamp: &args.stamp,
            })?;
            Ok(session.carve_report())
        })
    }

    #[tool(
        description = "Apply the exact shared carving preview as one Undo step. Rechecks frozen terrain/material/instance/cubemap sources, definitions, options, fragment IDs, complete face lineage and actual ordinary/terrain geometry against the draft. Retains cutters and entity ownership; selects surviving outputs. Source edits, selection/tool changes and other previews invalidate the token. Changed sources or allocations reject without mutation."
    )]
    pub(in crate::mcp) fn commit_carve(
        &self,
        Parameters(args): Parameters<VolumeTokenArgs>,
    ) -> ToolResult {
        run(|| {
            let result = self.session().commit_carve(
                Actor::Agent,
                args.token.parse().map_err(|_| "Invalid carve token")?,
            )?;
            let mut report = crate::session::carving::result_json(&result);
            report["handle"] = Value::Null;
            Ok(report)
        })
    }

    #[tool(
        description = "Discard the exact carving preview token without changing geometry, IDs or undo history. Stale tokens cannot cancel a replacement draft. Shares Discard preview and Cancel in the editor."
    )]
    pub(in crate::mcp) fn cancel_carve(
        &self,
        Parameters(args): Parameters<VolumeTokenArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.cancel_carve(args.token.parse().map_err(|_| "Invalid carve token")?)?;
            Ok(session.carve_report())
        })
    }

    #[tool(
        description = "Preview hollowing selected brushes or brush entities into convex wall pieces. Optional open_faces leaves those source boundaries open and unexpanded; each source must retain at least one wall. Thickness is 0.125–4096 units along face normals. outward=false insets walls inside the source bounds; true expands outside and preserves the original cavity. Requires latest stamp from get_hollow_preview. At most 16 source brushes, 64 sides each and 256 output walls; displacements, ambiguous IDs and invalid/thin inner geometry reject atomically. Existing surfaces retain metadata; generated inner faces inherit their corresponding source face's material, world projection and lighting. render_view uses the draft. Apply/Cancel use its token. Returns handle with kind and decimal-string token, separate from the required stamp. Changed selection and competing geometry previews invalidate it; repeating the current object selection preserves it. Closed or stale reports have handle=null."
    )]
    pub(in crate::mcp) fn preview_hollow(
        &self,
        Parameters(args): Parameters<HollowArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let ids = ids_or_selection(&session, args.ids.as_deref())?;
            session.preview_hollow(
                &ids,
                map_core::hollow::Spec {
                    thickness: args.thickness,
                    outward: args.outward,
                    open_faces: args.open_faces.iter().map(face_id).collect(),
                },
                &args.stamp,
            )?;
            Ok(session.hollow_report())
        })
    }

    #[tool(
        description = "Inspect selected source faces (solid/side IDs, normals, centers, materials), the shared hollow preview, open_faces, stamp, token, source snapshot, parameters, generated brush IDs, cavity centers/bounds and limits. The cavity center is an interior vertex average, suitable for Look inside. No map edits. handle contains the active kind and decimal-string token, or null when closed or stale; stamp remains the source guard.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_hollow_preview(&self) -> ToolResult {
        ok(self.session().hollow_report())
    }

    #[tool(
        description = "Apply the exact hollow preview as one undo step. Selects generated wall objects, preserves entity ownership and verifies generated geometry/IDs match the preview. Source edits, selection changes and other geometry tools invalidate the token."
    )]
    pub(in crate::mcp) fn commit_hollow(
        &self,
        Parameters(args): Parameters<VolumeTokenArgs>,
    ) -> ToolResult {
        run(|| {
            let result = self.session().commit_hollow(
                Actor::Agent,
                args.token.parse().map_err(|_| "Invalid hollow token")?,
            )?;
            let mut report = crate::session::hollowing::result_json(&result);
            report["handle"] = Value::Null;
            Ok(report)
        })
    }

    #[tool(
        description = "Discard the exact hollow preview token without changing map data or history. Stale tokens cannot cancel a replacement draft. Camera movement from Look inside is retained."
    )]
    pub(in crate::mcp) fn cancel_hollow(
        &self,
        Parameters(args): Parameters<VolumeTokenArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.cancel_hollow(args.token.parse().map_err(|_| "Invalid hollow token")?)?;
            Ok(session.hollow_report())
        })
    }

    #[tool(
        description = "Move the shared perspective camera inside a cavity of the exact hollow preview (zero-based index). Shares the editor's Look inside control. Preserves field of view, preview, map and history; invalid indices/tokens leave the camera unchanged."
    )]
    pub(in crate::mcp) fn look_inside_hollow(
        &self,
        Parameters(args): Parameters<HollowInsideArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.look_inside_hollow(
                args.token.parse().map_err(|_| "Invalid hollow token")?,
                args.index,
            )?;
            Ok(camera_json(&session.camera()))
        })
    }

    #[tool(
        description = "Create a box brush given mins+maxs or center+size and select it. Returns the existing object id, bounds and materials plus all face solid/side IDs and outward normals. Optional rotation and texture/lightmap scales use the shared batch rules (bounds within ±65536 and at least 0.125 units thick)."
    )]
    pub(in crate::mcp) fn create_block(
        &self,
        Parameters(args): Parameters<CreateBlockArgs>,
    ) -> ToolResult {
        run(|| {
            if args.rotation.is_some()
                || args.texture_scale.is_some()
                || args.lightmap_scale.is_some()
            {
                let block = crate::session::brush_batch::BlockSpec {
                    mins: args.mins,
                    maxs: args.maxs,
                    center: args.center,
                    size: args.size,
                    material: args.material,
                    rotation: args.rotation,
                    texture_scale: args.texture_scale,
                    lightmap_scale: args.lightmap_scale,
                };
                let mut session = self.session();
                let ids = session.create_blocks(Actor::Agent, &[block], args.snap)?;
                return Ok(creation_summary(session.document(), ids[0]));
            }
            let (mins, maxs) = match (args.mins, args.maxs, args.center, args.size) {
                (Some(lo), Some(hi), None, None) => (vec3(lo), vec3(hi)),
                (None, None, Some(c), Some(s)) => {
                    (vec3(c) - vec3(s) * 0.5, vec3(c) + vec3(s) * 0.5)
                }
                _ => return Err("give either mins and maxs, or center and size".into()),
            };
            let material = args.material.as_deref().unwrap_or(DEFAULT_MATERIAL);
            let mut session = self.session();
            let id = session.create_block(Actor::Agent, mins, maxs, material, args.snap)?;
            Ok(creation_summary(session.document(), id))
        })
    }

    #[tool(
        description = "Create 1–256 heterogeneous blocks as one atomic Undo step and select them. Each typed block gives mins+maxs or center+size, optional world rotation about its own center, material, texture [U,V] scales and lightmap scale. Uses the same Session operation as Import brush batch…. Bounds before and after rotation must stay within ±65536, with initial thickness at least 0.125. snap defaults false and applies before rotation. Returns compact object summaries in input order, including stable object IDs and all face solid/side IDs with outward normals. Any invalid block rolls back the entire batch, allocated IDs, history and selection."
    )]
    pub(in crate::mcp) fn create_blocks(
        &self,
        Parameters(args): Parameters<CreateBlocksArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let ids = session.create_blocks(Actor::Agent, &args.blocks, args.snap)?;
            Ok(
                json!({"count":ids.len(),"objects":ids.iter().map(|&id|creation_summary(session.document(),id)).collect::<Vec<_>>()}),
            )
        })
    }

    #[tool(
        description = "Inspect or change shared grid, snapping, workplane elevation, brush depth and the planar drawing tool for this session. Changes are validated atomically; omitted values are preserved. Changes never persist automatically; use get_grid_defaults / set_grid_defaults to explicitly save startup grid size, snapping and grid visibility."
    )]
    pub(in crate::mcp) fn set_spatial_settings(
        &self,
        Parameters(args): Parameters<crate::spatial::Update>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.set_spatial(Actor::Agent, args)?;
            Ok(json!(session.spatial()))
        })
    }

    #[tool(
        description = "Draw a brush footprint in top/front/side plane coordinates, using shared snapping, elevation and depth. Set preview=true to inspect the result in render_view without changing the document; repeat with preview=false to create it."
    )]
    pub(in crate::mcp) fn draw_block(
        &self,
        Parameters(args): Parameters<DrawBlockArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let projection = args
                .projection
                .as_deref()
                .map(|value| Projection::parse(value).ok_or("Invalid projection"))
                .transpose()?
                .unwrap_or(session.camera().projection);
            let (mins, maxs) = session.drawn_block_bounds(
                projection,
                glam::DVec2::from_array(args.start),
                glam::DVec2::from_array(args.end),
                args.elevation,
                args.depth,
            )?;
            if args.preview {
                session.preview_block(mins, maxs)?;
                Ok(
                    json!({ "preview": true, "bounds": bounds_json(Some((mins, maxs))), "spatial": session.spatial() }),
                )
            } else {
                let id = session.create_block(
                    Actor::Agent,
                    mins,
                    maxs,
                    args.material.as_deref().unwrap_or(DEFAULT_MATERIAL),
                    false,
                )?;
                Ok(summary(session.document(), id))
            }
        })
    }

    #[tool(
        description = "Cancel the current brush preview without changing the map or undo history."
    )]
    pub(in crate::mcp) fn cancel_block_preview(&self) -> ToolResult {
        self.session().cancel_block_preview();
        ok(json!({ "cancelled": true }))
    }

    #[tool(
        description = "Enable or leave the shared human clipping tool. Enabling needs selected brushes, leaves drawing mode and selects Top when in perspective. Disabling cancels the cut without an edit."
    )]
    pub(in crate::mcp) fn set_clip_tool(
        &self,
        Parameters(args): Parameters<ClipToolArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.set_clip_tool(Actor::Agent, args.enabled)?;
            Ok(clip_state(&session))
        })
    }

    #[tool(
        description = "Set the shared Terrain cut choice: grid (Existing grid, the default), flat (Retile flat terrain), cells (Retile grid cells), or fragments (Retile surface triangles). Changing it resets the blend mode to exact, disables neighbor partitioning and cancels the owned cut preview, points and plane drag while preserving map and Undo history. All drawn lines, three-point picks and handles use this same choice. Flat explicitly retiles eligible planar terrain, including joined slopes on a common tilted or world-axis base and exact common coordinate projection. Native arithmetic, orientation and conditioning checks apply; failed grid cuts never switch modes."
    )]
    pub(in crate::mcp) fn set_clip_terrain_mode(
        &self,
        Parameters(args): Parameters<ClipTerrainModeArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.set_clip_terrain_mode(Actor::Agent, args.mode.mode())?;
            Ok(clip_state(&session))
        })
    }

    #[tool(
        description = "Set Partition selected neighbors; default off, available in grid, cells and fragments. Grid refines only required selected noncrossing power-3/4 neighbor leaves to native power2: power3 allows one level and power4 two. Cells/Fragments retile the whole selected noncrossing neighbor required by a retained matched full original edge. Ordinary retiling emits one power2 patch per old cell in Cells or four in Fragments; certified native-window retiling can retain larger mixed-power patches. Corner-only, severed and unrelated neighbors stay unchanged. Complete source/metadata/material/field/native/LOD proofs, final density and shared output caps still apply; partial edges or incompatible fan density can refuse. Missing authorization identifies the required source and refuses atomically. Flat refuses true. Changing the setting cancels the owned cut draft while preserving map and Undo history; invalid requests preserve settings and the old draft."
    )]
    pub(in crate::mcp) fn set_clip_neighbor_partition(
        &self,
        Parameters(args): Parameters<ClipToolArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.set_clip_neighbor_partition(Actor::Agent, args.enabled)?;
            Ok(clip_state(&session))
        })
    }

    #[tool(
        description = "Set the shared terrain blend mode: exact (default), native_float or nearest_byte. Non-exact modes require fragments. Native float stores inserted alpha as float32, with raw error <=2^-17/255 and each packed-profile error <=(1+3*2^-17)/255. Nearest byte uses ties-to-even rounding, with raw error <=0.5/255 and packed error <=1.5/255. Geometry, UVs and spatial labels stay exact. Changing the mode cancels the owned cut draft; invalid requests preserve it."
    )]
    pub(in crate::mcp) fn set_clip_blend_mode(
        &self,
        Parameters(args): Parameters<ClipBlendModeArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.set_clip_blend_mode(Actor::Agent, args.mode.mode())?;
            Ok(clip_state(&session))
        })
    }

    #[tool(
        description = "Compatibility control for the shared terrain blend mode: enabled=true selects nearest_byte, false selects exact. Nearest byte requires fragments. Changing the mode cancels the owned draft; invalid requests preserve it. Use set_clip_blend_mode for native_float."
    )]
    pub(in crate::mcp) fn set_clip_blend_rounding(
        &self,
        Parameters(args): Parameters<ClipToolArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.set_clip_blend_rounding(Actor::Agent, args.enabled)?;
            Ok(clip_state(&session))
        })
    }

    #[tool(
        description = "Start or revise shared three-point clipping. Pass zero to three world points and the latest get_clip_preview snapshot. Zero starts over; removing the last point discards only the preview. Three distinct, noncollinear points create the same guarded cut preview as human viewport picking. Grid snapping is optional; keep/material choices persist. Invalid inputs preserve the last valid draft. Use commit_clip_preview or cancel_clip_preview to finish; set_clip_tool enabled=false exits."
    )]
    pub(in crate::mcp) fn set_clip_points(
        &self,
        Parameters(args): Parameters<ClipPointsArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let previous = session.clip_points();
            let keep = match args.keep.as_deref() {
                Some("front") => map_core::clip::Keep::Front,
                Some("back") => map_core::clip::Keep::Back,
                Some("both") => map_core::clip::Keep::Both,
                None => previous.map_or(map_core::clip::Keep::Both, |d| d.keep),
                _ => return Err("keep must be front, back or both".into()),
            };
            let material = match args.material {
                Some(value) => (!value.trim().is_empty()).then(|| value.trim().to_owned()),
                None => previous.and_then(|d| d.material.clone()),
            };
            let snap = args.snap.unwrap_or(previous.is_none_or(|d| d.snap));
            session.set_clip_points(
                Actor::Agent,
                crate::session::clip_points::Draft {
                    points: args.points.into_iter().map(DVec3::from_array).collect(),
                    keep,
                    material,
                    snap,
                },
                &args.snapshot,
            )?;
            Ok(clip_state(&session))
        })
    }

    #[tool(
        description = "Add the next cut point by picking a visible brush surface, matching a human viewport click. Start with set_clip_points first; accepts at most three points. Ignores point-entity helpers and hidden/tool-filtered brushes, uses the committed source map, and applies the draft's grid snapping. The third point previews the cut. Optional camera override does not move the user. Supply the latest clipping snapshot."
    )]
    pub(in crate::mcp) fn pick_clip_point(
        &self,
        Parameters(args): Parameters<PickClipPointArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let size = render_size(args.width, args.height);
            let camera = args
                .camera
                .unwrap_or_default()
                .resolve(&session, size.0 as f32 / size.1 as f32)?;
            let point = session.pick_clip_point(
                Actor::Agent,
                camera,
                crate::session::Pointer {
                    position: glam::DVec2::new(args.x, args.y),
                    size,
                },
                &args.snapshot,
            )?;
            Ok(json!({"point":point.to_array(),"state":clip_state(&session)}))
        })
    }

    #[tool(
        description = "Clip selected brushes, individual entity-child solids or whole brush entities by normal/distance, three VMF-wound points or a snapped planar line. Keep front (arrow side), back or both. terrain_mode selects grid (Existing grid), flat (Retile flat terrain), cells (Retile grid cells), or fragments (Retile surface triangles). Grid uses native middle/quarter cuts; Flat retiles affine planar surfaces; Cells cuts complete original rows/columns; Fragments cuts original triangles. Cells can use certified rectangular windows and mixed powers when native LOD moves retained samples, after separate authored/native surface proofs. Cells/Fragments support qualified warped, folded and overhanging surfaces through exact BASE coverage, 3D contact and native local/neighbor normal proofs. Authored and runtime geometry, UVs, paint, triangle labels, physical-material regions and editing fields must retain their certified source meaning. Unsupported inputs, self-intersection, extra contacts, changed retained LOD surfaces and exceeded native/resource bounds refuse explicitly. A one-sided source may discard native movement only when every affected triangle is strictly beyond the cut tolerance in every profile. No failed request changes mode or policy. Valid present ten-word VMF allowed_verts caches reset only on new children; absent caches stay absent and malformed caches refuse. Actual runtime masks remain certified. blend_mode selects exact (default), native_float or nearest_byte; non-exact modes require fragments and report raw/packed error bounds. Legacy round_blend maps true to nearest_byte and false to exact; supplying both blend fields refuses. partition_neighbors explicitly authorizes required selected noncrossing neighbors: Grid refines dyadic leaves to native power2; Cells/Fragments retile whole neighbors required by retained matched full edges. Corner-only, severed and unrelated neighbors stay unchanged. Missing selection/authorization, incompatible density or changed unselected native state refuse. partition_neighbors is unavailable in Flat. Omitted mode uses the shared choice; omitted blend/partition use shared settings, or exact/false when changing terrain_mode. Only successful overrides update shared choices. Caps inherit the source material unless material is supplied; terrain has no sealed cut walls. Normals and runtime LOD are regenerated. preview=true leaves map and Undo untouched; inspect get_clip_preview and render_view, then commit_clip_preview or cancel_clip_preview. Failure preserves the previous valid draft."
    )]
    pub(in crate::mcp) fn clip_brushes(
        &self,
        Parameters(args): Parameters<ClipArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let plane = args.plane.resolve(&session)?;
            let ids = object_ids_or_selection(&session, args.ids.as_deref())?;
            let keep = match args.keep.as_deref().unwrap_or("both") {
                "front" => map_core::clip::Keep::Front,
                "back" => map_core::clip::Keep::Back,
                "both" => map_core::clip::Keep::Both,
                _ => return Err("keep must be front, back or both".into()),
            };
            let result = session.clip_brushes_request(
                Actor::Agent,
                crate::session::ClipRequest {
                    ids: &ids,
                    plane,
                    keep,
                    material: args.material,
                    preview: args.preview,
                    terrain_mode: args.terrain_mode.map(ClipTerrainMode::mode),
                    blend_mode: args.blend_mode.map(ClipBlendMode::mode),
                    round_blend: args.round_blend,
                    partition_neighbors: args.partition_neighbors,
                },
            )?;
            Ok(
                json!({"result": clip_result(&result, session.clip_blend_mode(), session.clip_partition_neighbors()), "state": clip_state(&session)}),
            )
        })
    }

    #[tool(
        description = "Inspect the shared clipping tool and cut preview, including shared and frozen terrain_mode and blend_mode choices, plus partition_neighbors and the legacy round_blend flag, normalized plane, keep side, material, result IDs, piece bounds and face projections. Native terrain results identify cut or partition operations, list partitioned_neighbors source faces and include source/output faces, grid powers, verified neighbors and retained links, side mappings, bounded patch/sample work and manual cubemap rebindings. Terrain results with blend certification report blend_error with the certified raw bound, four native packed-profile bounds and their mode-specific limits in /255 units. Retiled previews retain guarded runtime primary material mapping dimensions and optional detail scales and expose their guard_error without accepting fresh assets. Referenced terrain contexts additionally expose instance/dependency counts, source-qualified child faces and guard_error; full dependency bytes are checked during preparation and Apply.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_clip_preview(&self) -> ToolResult {
        ok(clip_state(&self.session()))
    }

    #[tool(
        description = "Apply the current shared cut preview in one undo step. Rejects cancelled or invalidated previews; selects the resulting objects and leaves clipping mode."
    )]
    pub(in crate::mcp) fn commit_clip_preview(&self) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let result = session.commit_clip_preview(Actor::Agent)?;
            Ok(clip_result(
                &result,
                session.clip_blend_mode(),
                session.clip_partition_neighbors(),
            ))
        })
    }

    #[tool(
        description = "Discard the shared cut preview without changing map or undo history. Invalidates an active human cut gesture; clipping mode remains available for another cut."
    )]
    pub(in crate::mcp) fn cancel_clip_preview(&self) -> ToolResult {
        let mut session = self.session();
        session.cancel_clip_preview();
        ok(clip_state(&session))
    }

    #[tool(
        description = "Inspect shared cut-plane move/rotation handles and pixel/world paths. Optional camera/point performs hit testing without changing the view. Preview a cut first. World axes pivot at the selection center projected onto the plane.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_clip_handles(
        &self,
        Parameters(args): Parameters<HandlesArgs>,
    ) -> ToolResult {
        run(|| {
            let session = self.session();
            let mut report = super::handle_report::report(&session, args, session.clip_guide())?;
            report["state"] = clip_state(&session);
            Ok(report)
        })
    }

    #[tool(
        description = "Set cut-plane handle mode (move/rotate/off), grid snapping and rotation step. Requires the latest clipping snapshot. Changing options cancels the active gesture and restores its starting plane; the cut remains a preview."
    )]
    pub(in crate::mcp) fn set_clip_handle_options(
        &self,
        Parameters(args): Parameters<ClipHandleOptionsArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let mut options = session.clip_handle_options();
            if let Some(mode) = args.mode {
                options.mode = match mode.as_str() {
                    "move" => Some(render::handles::Mode::Move),
                    "rotate" => Some(render::handles::Mode::Rotate),
                    "off" => None,
                    _ => return Err("Choose move, rotate or off".into()),
                };
            }
            options.snap = args.snap.unwrap_or(options.snap);
            options.angle_step = args.angle_step.unwrap_or(options.angle_step);
            session.set_clip_handle_options(options, &args.snapshot)?;
            Ok(clip_state(&session))
        })
    }

    #[tool(
        description = "Begin positioning an existing cut preview with world-axis handles. Move: x/y/z, xy/xz/yz or view; rotate: x/y/z. Supply latest clipping snapshot and optional viewport pointer. Returns drag.token. Preview, keep side, cap material and three-point draft are retained; map/history unchanged."
    )]
    pub(in crate::mcp) fn begin_clip_drag(
        &self,
        Parameters(args): Parameters<BeginComponentDragArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let handle =
                render::handles::Handle::parse(&args.handle).ok_or("Unknown cut-plane handle")?;
            session.begin_clip_drag(
                handle,
                args.pointer.map(PointerArgs::resolve).transpose()?,
                &args.snapshot,
            )?;
            Ok(clip_state(&session))
        })
    }

    #[tool(
        description = "Position the exact cut-plane drag token using pointer or total amount pair since begin. Move amounts are world units; rotation is degrees. Snapping uses the shared grid or angle step. Invalid updates preserve the last valid draft and block finish. The original three cut points move rigidly with the plane, without re-snapping them."
    )]
    pub(in crate::mcp) fn update_clip_drag(
        &self,
        Parameters(args): Parameters<UpdateClipDragArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.update_clip_drag(
                parse_token(&args.token)?,
                args.pointer.map(PointerArgs::resolve).transpose()?,
                args.amount.map(glam::DVec2::from_array),
            )?;
            Ok(clip_state(&session))
        })
    }

    #[tool(
        description = "Finish positioning the exact cut-plane drag token. Keeps the cut as a preview with no map/history changes, matching mouse release. Inspect/render it, then explicitly commit_clip_preview to Apply cut, or cancel_clip_preview to abandon the cut."
    )]
    pub(in crate::mcp) fn finish_clip_drag(
        &self,
        Parameters(args): Parameters<VolumeTokenArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.finish_clip_drag(parse_token(&args.token)?)?;
            Ok(clip_state(&session))
        })
    }

    #[tool(
        description = "Cancel the exact cut-plane drag token and restore the plane and three-point draft from before the gesture. Does not abandon the entire cut or change map/history. Stale tokens cannot cancel newer work. Camera, view or handle-option changes also restore an active gesture."
    )]
    pub(in crate::mcp) fn cancel_clip_drag(
        &self,
        Parameters(args): Parameters<VolumeTokenArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.cancel_clip_drag(parse_token(&args.token)?)?;
            Ok(clip_state(&session))
        })
    }
}

fn creation_summary(document: &Document, id: ObjectId) -> Value {
    let mut result = summary(document, id);
    if let ObjectId::Solid(solid) = id
        && let Some(solid) = document.solid(solid)
    {
        result["faces"] = json!(solid.sides.iter().map(|side| {
            json!({"solid":solid.id,"side":side.id,"normal":Plane::from_points(side.plane.0).map(|plane|point(plane.normal))})
        }).collect::<Vec<_>>());
    }
    result
}
