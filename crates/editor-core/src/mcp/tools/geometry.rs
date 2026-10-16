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
                json!({"preview":args.preview,"objects":ids.iter().map(|&id|summary(document,id)).collect::<Vec<_>>(),"state":session.primitive_state()}),
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
                json!({"preview":args.preview,"objects":ids.iter().map(|&id|summary(doc,id)).collect::<Vec<_>>(),"state":session.primitive_state()}),
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
                json!({"objects":ids.iter().map(|&id|summary(session.document(),id)).collect::<Vec<_>>(),"state":session.primitive_state()}),
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
        description = "Inspect selected brush candidates and the shared carve preview, source owners/bounds/materials, snapshot stamp, exact token, target brush IDs, resulting fragments, removed/unchanged brushes, per-face inheritance and limits. Up to 8 convex cutters are retained; up to 16 targets and 256 intermediate/final fragments. World brushes may be removed completely; removing every brush from an entity rejects. No map edits.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_carve_preview(&self) -> ToolResult {
        ok(self.session().carve_report())
    }

    #[tool(
        description = "Preview subtracting one convex cutter brush from explicit target brush IDs, including children of brush entities. Requires the latest stamp from get_carve_preview. The cutter stays unchanged. Disjoint/tangent targets remain unchanged; an entirely disjoint batch rejects. Generated cut faces inherit the corresponding cutter face's material, world UV projection, lighting and unknown metadata. Surviving original faces retain metadata; original IDs remain on the first surviving fragment and later fragments get unique IDs. Preserves owner entities and unrelated objects. Whole world brushes can be removed; empty brush entities, displacements, invalid/thin geometry, 64-face overflow and budgets reject atomically. render_view uses the draft. Apply/cancel require the exact token."
    )]
    pub(in crate::mcp) fn preview_carve(
        &self,
        Parameters(args): Parameters<CarveArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.preview_carve(args.cutter, &args.targets, &args.stamp)?;
            Ok(session.carve_report())
        })
    }

    #[tool(
        description = "Preview subtracting the combined volume of 1–8 distinct retained convex cutter brushes from 1–16 target brushes. Cutters and targets must be disjoint roles. Inputs normalize to increasing brush IDs, so caller order cannot change fragments or coincident-face inheritance. Requires the latest get_carve_preview.stamp. All cutters stay unchanged; overlapping cutter volume is removed once. preview.result.cutters lists normalized IDs; each generated face reports its actual source_solid/source_side and inherits that cutter's material, world UVs, lighting and unknown metadata. Original faces and owner entities remain. Intermediate and final fragments are bounded at 256 with 64 sides each. Empty brush entities, invalid/displacement/thin geometry, bounds, all-disjoint inputs and budgets reject atomically while preserving the valid draft. render_view shows the same draft. commit_carve/cancel_carve use its token; Apply verifies allocations and records one undo step."
    )]
    pub(in crate::mcp) fn preview_carve_many(
        &self,
        Parameters(args): Parameters<CarveManyArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.preview_carve_many(&args.cutters, &args.targets, &args.stamp)?;
            Ok(session.carve_report())
        })
    }

    #[tool(
        description = "Apply the exact shared carving preview as one undo step. Rechecks fragment IDs, face inheritance and geometry against the draft. Retains all cutters and entity ownership; selects surviving carved objects. Source edits, selection/tool changes and other previews invalidate the token. Allocation changes reject without mutation."
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
            Ok(crate::session::carving::result_json(&result))
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
        description = "Preview hollowing selected brushes or brush entities into convex wall pieces. Optional open_faces leaves those source boundaries open and unexpanded; each source must retain at least one wall. Thickness is 0.125–4096 units along face normals. outward=false insets walls inside the source bounds; true expands outside and preserves the original cavity. Requires latest stamp from get_hollow_preview. At most 16 source brushes, 64 sides each and 256 output walls; displacements, ambiguous IDs and invalid/thin inner geometry reject atomically. Existing surfaces retain metadata; generated inner faces inherit their corresponding source face's material, world projection and lighting. render_view uses the draft. Apply/Cancel use its token."
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
        description = "Inspect selected source faces (solid/side IDs, normals, centers, materials), the shared hollow preview, open_faces, stamp, token, source snapshot, parameters, generated brush IDs, cavity centers/bounds and limits. The cavity center is an interior vertex average, suitable for Look inside. No map edits.",
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
            Ok(crate::session::hollowing::result_json(&result))
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
        description = "Create an axis-aligned box brush in the world, given mins+maxs or center+size, and select it."
    )]
    pub(in crate::mcp) fn create_block(
        &self,
        Parameters(args): Parameters<CreateBlockArgs>,
    ) -> ToolResult {
        run(|| {
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
            Ok(summary(session.document(), id))
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
        description = "Clip selected world brushes or brush entities by normal/distance, three VMF-wound points, or a snapped planar line. Keep front (positive-normal arrow side), back or both. Non-crossing brushes stay unchanged; point entities and intersected displacements are rejected atomically. Existing UVs/metadata survive; caps use material or inherit the first source material. preview=true leaves document and undo untouched; inspect with get_clip_preview and render_view, then commit or cancel."
    )]
    pub(in crate::mcp) fn clip_brushes(
        &self,
        Parameters(args): Parameters<ClipArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let plane = args.plane.resolve(&session)?;
            let ids = ids_or_selection(&session, args.ids.as_deref())?;
            let keep = match args.keep.as_deref().unwrap_or("both") {
                "front" => map_core::clip::Keep::Front,
                "back" => map_core::clip::Keep::Back,
                "both" => map_core::clip::Keep::Both,
                _ => return Err("keep must be front, back or both".into()),
            };
            let result = session.clip_brushes(
                Actor::Agent,
                &ids,
                plane,
                keep,
                args.material,
                args.preview,
            )?;
            Ok(json!({"result": clip_result(&result), "state": clip_state(&session)}))
        })
    }

    #[tool(
        description = "Inspect the shared clipping tool and cut preview, including normalized plane, keep side, material, result IDs, piece bounds and face projections.",
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
            self.session()
                .commit_clip_preview(Actor::Agent)
                .map(|result| clip_result(&result))
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
            let size = render_size(args.width, args.height);
            let camera = args
                .camera
                .unwrap_or_default()
                .resolve(&session, size.0 as f32 / size.1 as f32)?;
            let guide = session.clip_guide();
            let shapes = guide.map_or_else(Vec::new, |guide| {
                render::handles::shapes(&camera, size, &guide)
            });
            let picked = if let (Some(guide), Some(point)) = (guide, args.point) {
                let point = glam::Vec2::new(point[0] as f32, point[1] as f32);
                if !point.is_finite() {
                    return Err("Handle point must be finite".into());
                }
                render::handles::pick(&camera, size, &guide, point)
                    .map(render::handles::Handle::name)
            } else {
                None
            };
            Ok(
                json!({"state":clip_state(&session), "camera":camera_json(&camera), "width":size.0,"height":size.1,
                "picked":picked, "origin":guide.map(|g|g.origin.to_array()),
                "handles":shapes.iter().map(|shape|json!({"handle":shape.handle.name(), "color":shape.color,
                    "segments":shape.segments.iter().map(|points|points.map(|p|p.to_array())).collect::<Vec<_>>(),
                    "pixel_segments":shape.segments.iter().filter_map(|points|Some([render::handles::project(&camera,points[0],size)?.to_array(),render::handles::project(&camera,points[1],size)?.to_array()])).collect::<Vec<_>>()
                })).collect::<Vec<_>>() }),
            )
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
