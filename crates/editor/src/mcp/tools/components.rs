use super::super::*;

#[tool_router(router = components_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "List committed source brushes belonging to the selected owners, including children of large brush entities, without the 16-brush topology inspection limit. Returns numeric solid IDs, owner IDs, material, bounds, total, active_solids, inspection_limit and the latest component snapshot. offset defaults to 0; limit defaults to 100 and must be 1–256. Sorted, bounded pages do not alter selection, picking, map, history or previews. Source choices stay based on committed geometry during a draft. Capture an explicit 1–16-brush scope through set_component_picking using the returned snapshot, then get_brush_components uses that scope. Mirrors the component dialog's brush picker and page controls.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_component_sources(
        &self,
        Parameters(args): Parameters<ComponentSourcesArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().component_sources(
                args.offset.unwrap_or(0),
                args.limit
                    .unwrap_or(crate::session::components::SOURCE_PAGE_SIZE),
            )
        })
    }

    #[tool(
        description = "Replace, add or remove one source brush in the shared component picking scope using the latest snapshot from get_component_sources or get_brush_components. solid must belong to the selected owners; action is replace, add or remove; kind is vertex, edge or face. Captured scopes contain 1–16 brushes and can span source pages. Adding an existing member with the same kind is a no-op that preserves drafts; invalid, stale, out-of-source and over-limit choices preserve state. Removing the final member stops picking and clears component markers/drafts without map/history edits. Returns component state and bounded captured geometry, or empty brushes after stopping. Mirrors the Brush scope action selector in Reshape brushes."
    )]
    pub(in crate::mcp) fn update_component_scope(
        &self,
        Parameters(args): Parameters<ComponentScopeArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.update_component_scope(args.kind, args.solid, args.action, &args.snapshot)?;
            let ids = session
                .component_picking()
                .map_or_else(Vec::new, |picking| picking.solids.iter().copied().collect());
            session.component_report(&ids)
        })
    }

    #[tool(
        description = "Inspect precise convex brush topology and the shared vertex/edge selection. Returns vertices with incident face IDs, edges with endpoint positions and vertex indices, face rings, document markers and a snapshot guard. Numeric solid IDs may address entity brushes; defaults to selected objects. At most 16 brushes, 64 sides each. Uses the currently rendered geometry preview when present, including generated hollow/carve brushes; applying or selecting draft-only components still requires committing that geometry first. Component identity is solid + incident face IDs; array indices are for this response only. Selected quad faces expose crease_choices with opposing neighboring-side IDs and original midpoints. Selected convex faces with 3–8 corners expose every distinct boundary edge pair in partition_choices; quads retain two opposing choices followed by four adjacent pairs; source choices remain committed during topology previews. Texture projections default to world anchoring; texture_mode describes the current component preview.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_brush_components(
        &self,
        Parameters(args): Parameters<ComponentQueryArgs>,
    ) -> ToolResult {
        run(|| {
            let session = self.session();
            session.component_report(&args.solids.unwrap_or_else(|| session.component_solids()))
        })
    }

    #[tool(
        description = "Select shared brush faces, vertices or edges using solid + sides from get_brush_components. One side identifies a whole face; two incident side IDs identify an edge; three or more identify a vertex. Requires the latest snapshot. Supports replace/add/remove/toggle; empty replace clears components. Selecting components cancels other geometry drafts, highlights vertices/edges in the viewport and selects their owning objects. At most 512 components in 16 brushes."
    )]
    pub(in crate::mcp) fn select_brush_components(
        &self,
        Parameters(args): Parameters<ComponentSelectionArgs>,
    ) -> ToolResult {
        run(|| {
            let mode = parse_select_mode(args.mode.as_deref().unwrap_or("replace"))?;
            let components = args
                .components
                .into_iter()
                .map(|component| map_core::subobject::Component {
                    solid: component.solid,
                    sides: component.sides,
                })
                .collect::<Vec<_>>();
            let mut session = self.session();
            session.select_components(&components, mode, &args.snapshot)?;
            session.component_report(
                &session
                    .component_selection()
                    .iter()
                    .map(|component| component.solid)
                    .collect::<Vec<_>>(),
            )
        })
    }

    #[tool(
        description = "Enable or stop viewport component picking using the latest component snapshot. When enabled, kind is vertex, edge or face; solids defaults to the current captured scope or selected brushes (1–16). Captures a fixed brush scope that survives component selection and displays its guides before the first click. Clears competing tools and any movement draft. Invalid settings preserve the previous state. enabled=false clears picking, component selection and its draft without map edits. External object selection, source edits and competing tools exit this mode. Returns state including picking; get_brush_components also reports it."
    )]
    pub(in crate::mcp) fn set_component_picking(
        &self,
        Parameters(args): Parameters<ComponentPickingArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let kind = if args.enabled {
                Some(args.kind.ok_or("Choose vertex, edge or face")?)
            } else {
                None
            };
            let solids = args.solids.unwrap_or_else(|| session.component_solids());
            session.set_component_picking(kind, &solids, &args.snapshot)?;
            let report_solids = kind.map_or_else(Vec::new, |_| session.component_solids());
            session.component_report(&report_solids)
        })
    }

    #[tool(
        description = "Read component candidates at viewport coordinates using the same query as a UI click. x/y are continuous pixels from top-left (pixel center is column+0.5,row+0.5); use the camera and size of render_view. kind is vertex, edge or face; solids defaults to captured picking scope or selected brushes. Radius defaults to 10 pixels, allowed 1–32. Uses current component preview geometry and shared tool visibility; finish other geometry drafts or enter component picking first. Corners and edges are pickable through surfaces, matching their guides; faces use the nearest visible brush surface and are occluded by other brushes. Hidden brushes are excluded. Edges are clipped at the camera near plane. Returns at most 64 candidates sorted by screen distance, depth and stable solid/sides IDs, plus total/truncated, camera, size and snapshot. Read-only; pass chosen component and returned snapshot to select_brush_components. A normal UI click selects the first candidate; Ctrl toggles it; an empty normal click clears components while retaining scope.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn pick_brush_components(
        &self,
        Parameters(args): Parameters<ComponentPixelArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let size = render_size(args.width, args.height);
            let camera = args
                .camera
                .unwrap_or_default()
                .resolve(&session, size.0 as f32 / size.1 as f32)?;
            let solids = args.solids.unwrap_or_else(|| session.component_solids());
            let radius = args
                .radius
                .unwrap_or(crate::session::component_picking::PICK_RADIUS);
            let picks = session.pick_components(
                camera,
                crate::session::Pointer {
                    position: DVec2::new(args.x, args.y),
                    size,
                },
                args.kind,
                &solids,
                radius,
            )?;
            Ok(json!({
                "snapshot":session.component_stamp(), "kind":args.kind, "solids":solids,
                "camera":camera_json(&camera), "width":size.0, "height":size.1, "radius":radius,
                "total":picks.total, "truncated":picks.total > picks.candidates.len(),
                "candidates":picks.candidates.iter().map(|candidate|json!({
                    "component":crate::session::components::component_json(&candidate.component),
                    "position":candidate.position.to_array(), "screen":candidate.screen.to_array(),
                    "distance":candidate.distance, "depth":candidate.depth,
                })).collect::<Vec<_>>(),
            }))
        })
    }

    #[tool(
        description = "Preview moving selected brush faces/vertices/edges by a total world-space delta from committed geometry. Requires the latest component snapshot; optional snap honors the shared grid. Preserves face/solid IDs, ownership, material and metadata. Textures stay world-anchored by default. texture_lock=true attaches UVs to every corresponding corner; affine stretches and rotations work, incompatible face deformations reject and preserve the previous draft. Rejects nonplanar faces, collapsed edges, concavity, changed topology, displacements or coordinates outside ±65536. Invalid updates preserve the last valid draft. render_view and get_brush_components show the draft; Apply is one undo step. Source edits or other geometry tools invalidate the draft."
    )]
    pub(in crate::mcp) fn preview_component_move(
        &self,
        Parameters(args): Parameters<ComponentMoveArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.preview_component_move(
                vec3(args.delta),
                args.snap,
                args.texture_lock,
                &args.snapshot,
            )?;
            session.component_report(
                &session
                    .component_selection()
                    .iter()
                    .map(|component| component.solid)
                    .collect::<Vec<_>>(),
            )
        })
    }

    #[tool(
        description = "Apply the exact component preview token as one shared undo step. Selection remains available for another move; stored geometry is identical to the preview. Stale tokens reject without editing."
    )]
    pub(in crate::mcp) fn commit_component_move(
        &self,
        Parameters(args): Parameters<VolumeTokenArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.commit_component_move(
                Actor::Agent,
                args.token
                    .parse()
                    .map_err(|_| "Invalid component preview token")?,
            )?;
            session.component_report(
                &session
                    .component_selection()
                    .iter()
                    .map(|component| component.solid)
                    .collect::<Vec<_>>(),
            )
        })
    }

    #[tool(
        description = "Preview welding selected corners to their average position independently within each brush, or to target world coordinates or target_corner (solid/sides) from the edited brush scope. target and target_corner are mutually exclusive. get_brush_components.weld_targets lists existing source corners even during a draft. Fixed targets share one position across all edited brushes; preview.weld_target reports the mode. Target coordinates must be finite and within ±65536. Requires the latest component snapshot and at least two distinct vertices per brush; edges/faces are rejected. Surviving solid/side IDs, ownership, materials, lighting and unknown metadata remain. texture_lock defaults false (world UV projection); true preserves surviving corner UVs and averages merged contributors separately per source face. Reports texture_mode=weld_attached when enabled. Incompatible projections reject the whole batch. Collapsed faces are removed and their side IDs reported in preview.weld; redundant, self-touching, nonplanar, concave, empty or displacement results reject atomically and preserve the preceding draft. Limits: 512 selected corners, 16 brushes, 64 sides per brush. Topology and guides show the merged selection, while source_selection retains the original corners. Apply or Cancel before selecting preview components or dragging. No map/history changes until commit_component_edit; cancel_component_edit discards the draft."
    )]
    pub(in crate::mcp) fn preview_component_weld(
        &self,
        Parameters(args): Parameters<ComponentWeldArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let target = match (args.target, args.target_corner) {
                (None, None) => map_core::subobject::WeldTarget::Mean,
                (Some(position), None) => map_core::subobject::WeldTarget::Position(vec3(position)),
                (None, Some(corner)) => {
                    map_core::subobject::WeldTarget::Corner(map_core::subobject::Component {
                        solid: corner.solid,
                        sides: corner.sides,
                    })
                }
                (Some(_), Some(_)) => {
                    return Err("Choose either target or target_corner, not both".into());
                }
            };
            if let Some(lock) = args.texture_lock {
                session.preview_component_weld_with_texture(target, lock, &args.snapshot)?;
            } else if target == map_core::subobject::WeldTarget::Mean {
                session.preview_component_weld(&args.snapshot)?;
            } else {
                session.preview_component_weld_target(target, &args.snapshot)?;
            }
            selected_component_report(&session)
        })
    }

    #[tool(
        description = "Preview splitting one selected face or edge per brush by inserting and raising its center/midpoint outward. rise is 0.125–4096 world units, normal to the face or along the normalized sum of the two edge face normals. Requires the latest component snapshot. Replaces affected faces with convex triangle fans; keeps each source side ID on its first triangle and allocates IDs for additional faces. New faces inherit their source material, lighting and unknown metadata. texture_lock defaults false (world UV projection); true preserves every fan triangle corner UV, including the original face center/edge midpoint. Existing corner positions, solid IDs and entity ownership remain. preview.split reports rise, inserted positions, source/new side IDs and face inheritance. Rejects corners, multiple components per brush, neighboring-face crossings, invalid/nonconvex geometry, displacements, 64-face overflow or coordinates outside ±65536 atomically. Drafts show the new corner selection; source_selection retains inputs. Apply/cancel with commit_component_edit/cancel_component_edit. Apply verifies allocated IDs and is one undo step; selecting or dragging draft topology is blocked."
    )]
    pub(in crate::mcp) fn preview_component_split(
        &self,
        Parameters(args): Parameters<ComponentSplitArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            if let Some(lock) = args.texture_lock {
                session.preview_component_split_with_texture(args.rise, lock, &args.snapshot)?;
            } else {
                session.preview_component_split(args.rise, &args.snapshot)?;
            }
            selected_component_report(&session)
        })
    }

    #[tool(
        description = "Preview a roof/ridge crease on one selected quad face per brush, across 1–16 brushes. First select faces and inspect get_brush_components.crease_choices: each pair identifies the two neighboring face IDs at opposing source edges and their midpoints. pairs supplies {solid, edges:[neighbor_side,neighbor_side]} for every selected face; order is irrelevant. Raise both edge midpoints by rise (0.125–4096 units) along the source face normal, making two flat roof planes. Preserve all old corners, original side/solid IDs, entity ownership, lighting and unknown metadata; the additional roof side inherits the source face. roof_texture_lock defaults false (world UV projection); true preserves corresponding corner/midpoint UVs on both roof faces only. Neighboring projections remain exact. Reject nonquad, nonplanar, concave, neighboring-plane crossing, displacement, 64-face overflow and out-of-bounds results atomically before allocating IDs. Latest snapshot required. preview.crease reports new side IDs, ridge positions and the two resulting corner selections; source_selection and crease_choices remain original. Invalid updates retain the preceding draft. Apply/cancel with commit_component_edit/cancel_component_edit. Apply verifies generated IDs and records one undo step. Picking and movement of draft topology are blocked."
    )]
    pub(in crate::mcp) fn preview_component_crease(
        &self,
        Parameters(args): Parameters<ComponentCreaseArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            if let Some(lock) = args.roof_texture_lock {
                session.preview_component_crease_with_texture(
                    &args.pairs,
                    args.rise,
                    lock,
                    &args.snapshot,
                )?;
            } else {
                session.preview_component_crease(&args.pairs, args.rise, &args.snapshot)?;
            }
            selected_component_report(&session)
        })
    }

    #[tool(
        description = "Preview scaling selected faces, edges or corners with positive finite X/Y/Z factors. Default pivot is the selected component bounding-box center, reported by get_brush_components.component_center; optional pivot uses world coordinates within ±65536. Unselected corners remain fixed. Existing solid/side IDs, ownership, lighting and metadata stay. texture_lock=false keeps world projections; true preserves UVs at every corresponding corner and rejects incompatible projections. Invalid/nonplanar/concave/collapsed/displacement results, identity transforms and coordinates outside ±65536 reject atomically, retaining the preceding valid preview. Latest snapshot required; 512 components/16 brushes/64 sides. preview.transform reports input and resolved pivot. Move handles pause during this preview. Apply/cancel through commit_component_edit/cancel_component_edit; one undo step."
    )]
    pub(in crate::mcp) fn preview_component_scale(
        &self,
        Parameters(args): Parameters<ComponentScaleArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.preview_component_transform(
                map_core::subobject::ComponentTransform::Scale(vec3(args.factors)),
                args.pivot.map(vec3),
                args.texture_lock.unwrap_or(false),
                &args.snapshot,
            )?;
            selected_component_report(&session)
        })
    }

    #[tool(
        description = "Preview rotating selected faces, edges or corners by X/Y/Z angles in degrees about the selected bounding-box center or optional world pivot. Uses right-handed world axes in X then Y then Z order. get_brush_components.component_center and preview.transform expose default/resolved pivots. All values must be finite; pivot/result coordinates within ±65536. Unselected corners stay fixed. Retains solid/side IDs, owners, materials, lighting and metadata. World UVs are default; texture_lock=true must preserve every corresponding corner UV or reject. Nonplanar/concave/collapsed/identity/displacement results reject atomically while retaining the prior valid draft. Latest snapshot required; 512 components/16 brushes/64 sides. Move handles pause until Apply/Discard. Use commit_component_edit/cancel_component_edit; one undo step."
    )]
    pub(in crate::mcp) fn preview_component_rotate(
        &self,
        Parameters(args): Parameters<ComponentRotateArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.preview_component_transform(
                map_core::subobject::ComponentTransform::Rotate(vec3(args.degrees)),
                args.pivot.map(vec3),
                args.texture_lock.unwrap_or(false),
                &args.snapshot,
            )?;
            selected_component_report(&session)
        })
    }

    #[tool(
        description = "Apply the exact shared movement, scaling, rotation, welding or splitting preview token as one undo step. Stored geometry matches the draft; splitting verifies generated face IDs before accepting. Retains resulting component selection and picking scope; welding/splitting select each resulting corner using its new incidence. Splitting changes active viewport picking to corners. Stale tokens, changed allocations and invalid handle updates reject without editing."
    )]
    pub(in crate::mcp) fn commit_component_edit(
        &self,
        Parameters(args): Parameters<VolumeTokenArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.commit_component_edit(Actor::Agent, parse_token(&args.token)?)?;
            selected_component_report(&session)
        })
    }

    #[tool(
        description = "Discard the exact movement, scale, rotate, weld, split, ridge or extrusion preview without changing map, face allocation or history. Restores original component selection and retains viewport picking scope/kind. Stale tokens cannot discard a newer draft. Equivalent to Discard preview; stopping component picking clears the whole tool."
    )]
    pub(in crate::mcp) fn cancel_component_edit(
        &self,
        Parameters(args): Parameters<VolumeTokenArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.cancel_component_move(parse_token(&args.token)?)?;
            selected_component_report(&session)
        })
    }

    #[tool(
        description = "Cancel the exact component preview token without changing the map, history or component selection. Stale tokens cannot cancel a replacement preview. Clear selected markers with select_brush_components using an empty replacement selection; exit picking and clear all guides with set_component_picking enabled=false."
    )]
    pub(in crate::mcp) fn cancel_component_move(
        &self,
        Parameters(args): Parameters<VolumeTokenArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.cancel_component_move(
                args.token
                    .parse()
                    .map_err(|_| "Invalid component preview token")?,
            )?;
            session.component_report(
                &session
                    .component_selection()
                    .iter()
                    .map(|component| component.solid)
                    .collect::<Vec<_>>(),
            )
        })
    }

    #[tool(
        description = "Edit one position, rotation, color or brightness component while preserving the others. Uses the same operation as the GUI component inputs."
    )]
    pub(in crate::mcp) fn set_entity_component(
        &self,
        Parameters(args): Parameters<EntityComponentArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let id = single_entity(&session, args.id.as_deref())?;
            session.set_entity_component(Actor::Agent, id, &args.key, args.index, &args.value)?;
            detail(session.document(), ObjectId::Entity(id))
        })
    }

    #[tool(
        description = "Inspect component move settings, current drag and visible axis/plane/view handle paths. Uses the same guides and hit testing as the viewport. Optional camera/point inspect without changing the user's view. Select components first; handles follow the last valid movement preview.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_component_handles(
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
            let guide = session.component_guide();
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
                json!({"state":session.component_report(&[])?, "camera":camera_json(&camera), "width":size.0,"height":size.1,
                "picked":picked, "origin":guide.map(|g|g.origin.to_array()),
                "handles":shapes.iter().map(|shape|json!({"handle":shape.handle.name(), "color":shape.color,
                    "segments":shape.segments.iter().map(|points|points.map(|p|p.to_array())).collect::<Vec<_>>(),
                    "pixel_segments":shape.segments.iter().filter_map(|points|Some([render::handles::project(&camera,points[0],size)?.to_array(),render::handles::project(&camera,points[1],size)?.to_array()])).collect::<Vec<_>>()
                })).collect::<Vec<_>>() }),
            )
        })
    }

    #[tool(
        description = "Set shared component move-handle visibility, grid snapping and corner texture attachment. Requires the latest component snapshot; omitted settings remain unchanged. A change cancels an active handle drag. Equivalent to the Reshape brushes controls."
    )]
    pub(in crate::mcp) fn set_component_move_options(
        &self,
        Parameters(args): Parameters<ComponentMoveOptionsArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let mut options = session.component_move_options();
            options.enabled = args.enabled.unwrap_or(options.enabled);
            options.snap = args.snap.unwrap_or(options.snap);
            options.texture_lock = args.texture_lock.unwrap_or(options.texture_lock);
            session.set_component_move_options(options, &args.snapshot)?;
            session.component_report(&[])
        })
    }

    #[tool(
        description = "Begin a guarded component move drag for selected faces/edges/corners. handle is x/y/z, xy/xz/yz or view. Optional pointer starts viewport dragging; otherwise use numeric offsets. Captures camera and starting preview offset; retains fixed picking scope. Returns drag.token; geometry remains unchanged until commit."
    )]
    pub(in crate::mcp) fn begin_component_drag(
        &self,
        Parameters(args): Parameters<BeginComponentDragArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let handle =
                render::handles::Handle::parse(&args.handle).ok_or("Unknown move handle")?;
            session.begin_component_drag(
                handle,
                args.pointer.map(PointerArgs::resolve).transpose()?,
                &args.snapshot,
            )?;
            session.component_report(&[])
        })
    }

    #[tool(
        description = "Update the exact component drag token using either pointer or offset, a total pair of world-unit amounts since begin. Axis handles require second amount zero; plane handles use world axes and view uses captured screen right/up. Shared snap and texture attachment apply. Invalid geometry preserves the last valid draft and blocks commit until a valid update. Returning to zero creates no edit."
    )]
    pub(in crate::mcp) fn update_component_drag(
        &self,
        Parameters(args): Parameters<UpdateComponentDragArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.update_component_drag(
                parse_token(&args.token)?,
                args.pointer.map(PointerArgs::resolve).transpose()?,
                args.offset.map(glam::DVec2::from_array),
            )?;
            session.component_report(&[])
        })
    }

    #[tool(
        description = "Apply the exact component drag token as one undo step using the last valid preview. Rejects an invalid last update or replaced drag. Keeps component selection and viewport picking scope for the next move; a zero-offset drag changes nothing."
    )]
    pub(in crate::mcp) fn commit_component_drag(
        &self,
        Parameters(args): Parameters<VolumeTokenArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.commit_component_drag(Actor::Agent, parse_token(&args.token)?)?;
            session.component_report(&[])
        })
    }

    #[tool(
        description = "Cancel the exact component drag token and its preview without changing map or history. Keeps selection and picking scope. Camera, source, selection or tool changes also invalidate the gesture; stale tokens cannot cancel newer work."
    )]
    pub(in crate::mcp) fn cancel_component_drag(
        &self,
        Parameters(args): Parameters<VolumeTokenArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.cancel_component_drag(parse_token(&args.token)?)?;
            session.component_report(&[])
        })
    }
}
