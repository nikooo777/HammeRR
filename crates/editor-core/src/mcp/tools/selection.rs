use super::super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SelectionDefaultsAction {
    Save,
    Apply,
    Reload,
    Reset,
}

impl SelectionDefaultsAction {
    fn name(&self) -> &'static str {
        match self {
            Self::Save => "save",
            Self::Apply => "apply",
            Self::Reload => "reload",
            Self::Reset => "reset",
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectionDefaultsArgs {
    action: SelectionDefaultsAction,
    snapshot: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SelectionProfileArgs {
    snapshot: String,
    operation: crate::session::selection_preferences::ProfileAction,
}

#[tool_router(router = selection_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect session-local object-selection seed filters. Startup uses saved selection defaults, or all types enabled when no file exists. Objects remain visible. Filters affect ordinary object clicks, overlap candidates/cycling, object marquee/volume and object paint. Enabled hits seed complete visible groups; fully-contained groups still need all visible members geometrically inside. Faces, components, geometric raycast/pick_at_pixel and explicit IDs/report/group/layer selection bypass. Current selection is retained. No map/history/compiler or preference writes. Inspect get_selection_defaults for saved values, readiness and explicit default actions.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_selection_filters(&self) -> ToolResult {
        ok(self.session().selection_filter_state())
    }

    #[tool(
        description = "Set all three temporary object-selection seed filters through the same Session operation as Selection → Pick object types. world_brushes, brush_entities and point_entities are required booleans; point_entities includes models, helpers, ropes and root instances. All false is allowed. Cancels only object-selection drafts and stale object gestures; preserves existing selection, face/texture/geometry work, visibility, map/history and compiler output. Values remain in this Session across New/Open and never save automatically. Use set_selection_defaults to explicitly persist startup filters. No-op changes leave revisions and drafts unchanged. Inspect get_selection_filters for group and bypass rules."
    )]
    pub(in crate::mcp) fn set_selection_filters(
        &self,
        Parameters(filters): Parameters<crate::session::selection_filters::SelectionFilters>,
    ) -> ToolResult {
        run(|| self.session().set_selection_filters(Actor::Agent, filters))
    }

    #[tool(
        description = "Inspect shared saved selection-filter defaults, the originating Session's current filters, factory defaults, storage readiness/errors and an action snapshot. Same state as Selection → Selection defaults…. Only world_brushes, brush_entities and point_entities are saved; all false is valid. Shared saved changes never replace current filters in other documents. No graphics, source, history or preference writes.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_selection_defaults(&self) -> ToolResult {
        ok(self.session().selection_preferences_state())
    }

    #[tool(
        description = "Manage selection-filter defaults using the latest get_selection_defaults.snapshot. save writes current filters; apply changes only this Session through set_selection_filters, retiring its stale object-selection gestures without pruning selection; reload reads storage without applying; reset writes all-enabled factory defaults without applying. Only save/reset write bounded atomic selection-defaults.json. Save/Apply/Reset recheck exact source/directory observations; conflicts require Reload. Malformed/unsupported files stay intact until explicit Reset. Participating Linux writers coordinate under the existing physical-destination lock; unrelated writers remain outside that guarantee. write_committed=true with ready=false explicitly reports publication followed by verification failure; inspect error before continuing. UI labels: Save current as defaults / Apply saved defaults / Reload defaults / Reset saved defaults. New/Open retain current filters; startup applies defaults once per Session."
    )]
    pub(in crate::mcp) fn set_selection_defaults(
        &self,
        Parameters(args): Parameters<SelectionDefaultsArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().change_selection_preferences(
                Actor::Agent,
                args.action.name(),
                &args.snapshot,
            )
        })
    }

    #[tool(
        description = "Inspect named object-selection filter profiles, their stable IDs, current filters, independent startup defaults, shared storage readiness/errors and an action snapshot. Same catalogue as Selection defaults… → Named profiles. Profiles do not apply automatically or change startup defaults. Up to 32 profiles with case-insensitively unique names of 1–80 UTF-8 bytes share the bounded selection-defaults.json authority. No graphics, source, history or preference writes.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_selection_profiles(&self) -> ToolResult {
        ok(self.session().selection_profiles_state())
    }

    #[tool(
        description = "Manage named selection-filter profiles through the shared Session operation. Requires latest get_selection_profiles.snapshot and operation: {action:save,name} saves current filters as a new profile; {action:replace,id} replaces that profile's filters while retaining its name/ID; {action:rename,id,name} renames; {action:delete,id} removes; {action:apply,id} applies only to this Session through the ordinary filter setter. Only explicit apply changes active filters or retires object-selection gestures. Profile writes never change startup defaults or any other document's filters, selection, source/history or camera. The catalogue shares exact-source/directory guards, participating writer locks, errors and explicit Reload defaults with startup defaults. Stale forms/conflicts reject. write_committed=true with ready=false means the file was written but verification failed. UI: Save current as profile / Replace selected profile / Apply selected profile / Rename profile / Delete profile."
    )]
    pub(in crate::mcp) fn set_selection_profile(
        &self,
        Parameters(args): Parameters<SelectionProfileArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .change_selection_profile(Actor::Agent, args.operation, &args.snapshot)
        })
    }

    #[tool(
        description = "Change the shared highlighted object selection. Native groups expand to their outermost group unless set_group_selection_bypass enables individual selection. Hidden peers stay hidden; explicitly requested hidden objects remain selectable. Use select_object_group for an inner group. Changed selection invalidates affected geometry previews, including hollow, carve, repeat-array and terrain face drafts; cleared_preview_kinds lists affected active handle kinds. Repeating the current object selection preserves these previews. Property and connection drafts retain their captured targets. Inspect a preview again after changing selection before updating or committing it."
    )]
    pub(in crate::mcp) fn select(&self, Parameters(args): Parameters<SelectArgs>) -> ToolResult {
        run(|| {
            let mode = match args.mode.as_deref().unwrap_or("replace") {
                "replace" => SelectMode::Replace,
                "add" => SelectMode::Add,
                "remove" => SelectMode::Remove,
                "toggle" => SelectMode::Toggle,
                other => return Err(format!("unknown mode {other:?}")),
            };
            let mut session = self.session();
            session.select_grouped_report(&parse_ids(&args.ids)?, mode)
        })
    }

    #[tool(
        description = "Selected objects with their summaries and combined presentation bounds. Enabled admitted rope spans enlarge their source helper bounds for this inspection and framing; authored transform pivots/anchors remain unchanged.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_selection(&self) -> ToolResult {
        let session = self.session();
        let ids = session.selected_ids();
        let doc = session.document();
        ok(json!({
            "objects": ids.iter().map(|&id| summary(doc, id)).collect::<Vec<_>>(),
            "face_selection": face_state(&session),
            "bounds": bounds_json(session.presentation_object_bounds_in(doc, &ids, None)),
        }))
    }

    #[tool(
        description = "Delete objects (defaults to the selection).",
        annotations(destructive_hint = true)
    )]
    pub(in crate::mcp) fn delete_objects(
        &self,
        Parameters(args): Parameters<IdsArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let ids = ids_or_selection(&session, args.ids.as_deref())?;
            session.delete_objects(Actor::Agent, &ids)?;
            Ok(json!({ "deleted": ids.iter().map(|&id| id_string(id)).collect::<Vec<_>>() }))
        })
    }

    #[tool(description = "Move objects by an offset (defaults to the selection).")]
    pub(in crate::mcp) fn move_objects(
        &self,
        Parameters(args): Parameters<MoveArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let ids = ids_or_selection(&session, args.ids.as_deref())?;
            let lock = args.texture_lock.unwrap_or(true);
            session.edit(Actor::Agent, &format!("move {} objects", ids.len()), |tx| {
                edit::translate(tx, &ids, vec3(args.offset), lock)
            })?;
            let doc = session.document();
            Ok(json!({ "objects": ids.iter().map(|&id| summary(doc, id)).collect::<Vec<_>>() }))
        })
    }

    #[tool(
        description = "Move, rotate, scale, mirror and shear objects in one atomic undoable edit, or create a shared draft with preview=true. Choose world (default) or local space. Scale about the pivot, rotate X/Y/Z in that frame, then translate in that frame. Defaults to selection and its bounds center. texture_lock follows movement/rotation; texture_scale_lock stretches about the pivot independently. Omitted scale lock inherits texture_lock, retaining the prior complete-lock/world-fixed behavior. Both false preserve original projections exactly. Point entities rotate their Source orientation and move with the group; model size stays unchanged. Positive scale only. reflection chooses axes in that same frame, preserving outward winding and source IDs. Reflection supports world brushes and brush entities within ±65536; point entities and displacement brushes are rejected atomically. shear shifts its axis by factor times its distinct reference coordinate about the pivot in the same frame, after scale/reflection and before rotation. Finite factors -64..64; nonzero shear shares the brush-only scope and bounds. texture_scale_lock also controls reflection/shear attachment. Inspect get_transform_handles for the shared draft; commit_transform_preview or cancel_transform_preview completes it."
    )]
    pub(in crate::mcp) fn transform_objects(
        &self,
        Parameters(args): Parameters<TransformArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let ids = ids_or_selection(&session, args.ids.as_deref())?;
            let local = match args.space.as_deref().unwrap_or("world") {
                "world" => false,
                "local" => true,
                _ => return Err("space must be world or local".into()),
            };
            let basis = session.transform_basis(&ids, local)?;
            let transform = edit::Transform {
                translation: args.translation.map(vec3).unwrap_or(DVec3::ZERO),
                rotation: args.rotation.map(vec3).unwrap_or(DVec3::ZERO),
                scale: args.scale.map(vec3).unwrap_or(DVec3::ONE),
                reflection: args.reflection.unwrap_or([false; 3]),
                shear: args.shear.map(ShearArgs::into_core).transpose()?,
                texture_lock: args.texture_lock.unwrap_or(true),
                texture_scale_lock: args.texture_scale_lock,
                basis,
                ..edit::Transform::default()
            };
            let transform = edit::Transform {
                translation: basis * transform.translation,
                ..transform
            };
            let pivot = if args.preview {
                session.preview_transform(Actor::Agent, &ids, transform, args.pivot.map(vec3))?
            } else {
                session.transform_objects(Actor::Agent, &ids, transform, args.pivot.map(vec3))?
            };
            let document = if args.preview {
                session.render_document()
            } else {
                session.document()
            };
            Ok(json!({
                "pivot": point(pivot),
                "bounds": bounds_json(session.object_bounds_in(document, &ids)),
                "objects": ids.iter().map(|&id| summary(document, id)).collect::<Vec<_>>(),
                "preview": args.preview, "state": transform_state(&session),
            }))
        })
    }

    #[tool(
        description = "Preview grid snapping using the current spacing and transform texture policy, even when Snap is off. mode=selection (default) moves the selection's world bounds minimum by one offset, preserving relative positions and the existing grid_snap result. mode=each_object computes a separate bounds-minimum move per whole object; returns grid_snap.mode, grid and objects with canonical id, bounds_minimum, snapped_bounds_minimum and delta. Group members snap separately; each brush entity moves all its children together. Child-brush IDs and ambiguous, hidden, invalid or out-of-bounds roots reject the independent batch without replacing a valid draft. IDs default to the shared selection; duplicates snap once. Review get_transform_handles/render_view, then commit_transform_preview or cancel_transform_preview. Apply is one Undo step; aligned objects retain exact data and add no history. Reloading model assets cancels snap drafts. UI: Transform selection → Snap: Selection / Each object → Preview snap to grid."
    )]
    pub(in crate::mcp) fn preview_snap_to_grid(
        &self,
        Parameters(args): Parameters<GridSnapArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let ids = ids_or_selection(&session, args.ids.as_deref())?;
            let snap = session.preview_snap_to_grid_mode(Actor::Agent, &ids, args.mode)?;
            Ok(json!({"grid_snap": snap, "state": transform_state(&session)}))
        })
    }

    #[tool(
        description = "Copy objects with an offset and select the copies (defaults to the selection)."
    )]
    pub(in crate::mcp) fn duplicate_objects(
        &self,
        Parameters(args): Parameters<DuplicateArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let ids = ids_or_selection(&session, args.ids.as_deref())?;
            let copies = session.duplicate_objects(Actor::Agent, &ids, vec3(args.offset))?;
            let doc = session.document();
            Ok(json!({ "copies": copies.iter().map(|&id| summary(doc, id)).collect::<Vec<_>>() }))
        })
    }

    #[tool(
        description = "Switch the shared viewport selection mode: objects or faces. Clears the previous selection and cancels obsolete previews."
    )]
    pub(in crate::mcp) fn set_selection_mode(
        &self,
        Parameters(args): Parameters<SelectionModeArgs>,
    ) -> ToolResult {
        run(|| {
            let enabled = match args.mode.as_str() {
                "objects" => false,
                "faces" => true,
                _ => return Err("mode must be objects or faces".into()),
            };
            self.session().set_face_mode(enabled);
            Ok(json!({"mode": args.mode}))
        })
    }

    #[tool(
        description = "Read or set the shared viewport transform tool: select/move/rotate/scale, world/local axes, selection/world/custom pivot, independent texture movement/rotation and scale-stretch locking, and angle snap. Local axes follow the first selected brush's first face or point-entity orientation. Changing controls cancels pending transform/area interactions."
    )]
    pub(in crate::mcp) fn set_transform_tool(
        &self,
        Parameters(args): Parameters<TransformToolArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let mut tool = session.transform_tool();
            if let Some(mode) = args.mode {
                tool.mode = match mode.as_str() {
                    "select" => None,
                    "move" => Some(render::handles::Mode::Move),
                    "rotate" => Some(render::handles::Mode::Rotate),
                    "scale" => Some(render::handles::Mode::Scale),
                    _ => return Err("mode must be select, move, rotate or scale".into()),
                };
            }
            if let Some(space) = args.space {
                tool.local = match space.as_str() {
                    "world" => false,
                    "local" => true,
                    _ => return Err("space must be world or local".into()),
                };
            }
            if let Some(mode) = args.pivot_mode {
                tool.pivot = match mode.as_str() {
                    "selection" => None,
                    "world" => Some(DVec3::ZERO),
                    "custom" => Some(
                        args.pivot
                            .map(vec3)
                            .or(tool.pivot)
                            .ok_or("custom pivot needs coordinates")?,
                    ),
                    _ => return Err("pivot_mode must be selection, world or custom".into()),
                };
            } else if let Some(pivot) = args.pivot {
                tool.pivot = Some(vec3(pivot));
            }
            tool.texture_lock = args.texture_lock.unwrap_or(tool.texture_lock);
            if let Some(scale_lock) = args.texture_scale_lock {
                tool.texture_scale_lock = Some(scale_lock);
            }
            tool.angle_step = args.angle_step.unwrap_or(tool.angle_step);
            session.set_transform_tool(Actor::Agent, tool)?;
            Ok(transform_state(&session))
        })
    }

    #[tool(
        description = "Inspect the shared transform settings, gesture and draft, plus the same visible handle paths used by the renderer, in world coordinates and viewport pixels. Optional point hit-tests those paths. Camera overrides inspect a view without moving the user.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_transform_handles(
        &self,
        Parameters(args): Parameters<HandlesArgs>,
    ) -> ToolResult {
        run(|| {
            let session = self.session();
            let mut report =
                super::handle_report::report(&session, args, session.transform_guide())?;
            report["state"] = transform_state(&session);
            Ok(report)
        })
    }

    #[tool(
        description = "Begin the shared transform-handle interaction for selected objects. Captures the pivot, axes and original geometry. Supply a pointer for viewport dragging, or omit it for numeric amounts; movement supports axis/plane/view handles, rotation axis rings, scaling axis/uniform handles."
    )]
    pub(in crate::mcp) fn begin_transform_drag(
        &self,
        Parameters(args): Parameters<BeginTransformArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let handle =
                render::handles::Handle::parse(&args.handle).ok_or("Unknown transform handle")?;
            session.begin_transform_drag(
                handle,
                args.pointer.map(PointerArgs::resolve).transpose()?,
            )?;
            Ok(transform_state(&session))
        })
    }

    #[tool(
        description = "Update the current shared handle preview by pointer or numeric total amount from the drag start. Axis move uses units, rotation degrees, scale a positive factor; plane_offset supplies both plane/view offsets. Shared grid, Snap and angle step apply. Every update starts from the original document; it does not add undo history."
    )]
    pub(in crate::mcp) fn update_transform_drag(
        &self,
        Parameters(args): Parameters<UpdateTransformArgs>,
    ) -> ToolResult {
        run(|| {
            if args.amount.is_some() && args.plane_offset.is_some() {
                return Err("Provide amount or plane_offset, not both".into());
            }
            let amount = args
                .plane_offset
                .map(glam::DVec2::from_array)
                .or(args.amount.map(|amount| glam::DVec2::new(amount, 0.0)));
            let mut session = self.session();
            session.update_transform_drag(
                args.pointer.map(PointerArgs::resolve).transpose()?,
                amount,
            )?;
            Ok(transform_state(&session))
        })
    }

    #[tool(
        description = "Apply the shared transform preview in one undo step, preserving the selected IDs. Rejects cancelled or invalidated previews. Equivalent to releasing a human handle drag or choosing Apply transform."
    )]
    pub(in crate::mcp) fn commit_transform_preview(&self) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let pivot = session.commit_transform_preview(Actor::Agent)?;
            Ok(json!({"pivot": pivot.to_array(), "state": transform_state(&session)}))
        })
    }

    #[tool(
        description = "Discard the shared transform preview and active human/agent handle interaction without changing the map or undo history."
    )]
    pub(in crate::mcp) fn cancel_transform_preview(&self) -> ToolResult {
        let mut session = self.session();
        session.cancel_transform_preview();
        ok(transform_state(&session))
    }

    #[tool(
        description = "Read or change shared marquee-selection rules: contained/intersecting and replace/add/remove/toggle. Changing options cancels an unfinished area selection."
    )]
    pub(in crate::mcp) fn set_area_selection_options(
        &self,
        Parameters(args): Parameters<AreaOptionsArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let rule = args
                .rule
                .map(|rule| area_rule(&rule))
                .transpose()?
                .unwrap_or(session.area_rule());
            let mode = args
                .mode
                .map(|mode| parse_select_mode(&mode))
                .transpose()?
                .unwrap_or(session.area_mode());
            session.set_area_settings(rule, mode);
            Ok(area_state(&session))
        })
    }

    #[tool(
        description = "Enable/disable the shared circular paint brush, choose selection/material/projection and set a 2–64 screen-pixel radius. Changing settings cancels the active stroke without map or selection edits. Texture strokes use sample_face_texture; inspect get_surface_sample and preview_material for the source. UI equivalents: Paint selection / Paint texture, operation and Radius."
    )]
    pub(in crate::mcp) fn set_paint_tool(
        &self,
        Parameters(args): Parameters<PaintToolArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let enabled = args.enabled.unwrap_or(session.paint_enabled());
            let radius = args.radius.unwrap_or(session.paint_radius());
            let operation = args
                .operation
                .as_deref()
                .map(crate::session::paint::Operation::parse)
                .transpose()?
                .unwrap_or(session.paint_operation());
            if operation == crate::session::paint::Operation::Selection {
                session.set_paint_tool(Actor::Agent, enabled, radius)?;
            } else {
                session.set_paint_operation(Actor::Agent, enabled, radius, operation)?;
            }
            Ok(session.paint_selection_json(0, 100))
        })
    }

    #[tool(
        description = "Begin a paint-selection preview at a viewport point and return its token. Uses a circular brush on a fixed 2-pixel sample lattice, nearest visible object/face hits and a spatial picking index. Does not change selection/history until commit. Optional camera override leaves the user's camera unchanged. Width/height stay fixed during the stroke. Map, camera, selection, visibility or incompatible tool changes invalidate it. Terrain uses actual displaced sheets; face strokes ignore point helpers. Inspect limits before large strokes."
    )]
    pub(in crate::mcp) fn begin_paint_selection(
        &self,
        Parameters(args): Parameters<BeginPaintArgs>,
    ) -> ToolResult {
        run(|| {
            let pointer = args.pointer.resolve()?;
            let mut session = self.session();
            let camera = args
                .camera
                .unwrap_or_default()
                .resolve(&session, pointer.size.0 as f32 / pointer.size.1 as f32)?;
            let faces = match args.selection_mode.as_deref() {
                None => session.face_mode(),
                Some("objects") => false,
                Some("faces") => true,
                _ => return Err("selection_mode must be objects or faces".into()),
            };
            let radius = args.radius.unwrap_or(session.paint_radius());
            let mode = args
                .mode
                .as_deref()
                .map(parse_select_mode)
                .transpose()?
                .unwrap_or(session.area_mode());
            session.begin_paint_selection(crate::session::paint::Spec {
                camera,
                pointer,
                radius,
                mode,
                faces,
            })?;
            Ok(session.paint_selection_json(0, 100))
        })
    }

    #[tool(
        description = "Extend the exact active paint stroke through up to 256 ordered screen-space points. Continuous segments fill gaps between updates. Revisited samples and hits are deduplicated, including toggle selection. Invalid or over-budget updates leave the previous preview intact; apply/cancel and start another stroke if its budget is full."
    )]
    pub(in crate::mcp) fn update_paint_selection(
        &self,
        Parameters(args): Parameters<UpdatePaintArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let points = args
                .points
                .into_iter()
                .map(DVec2::from_array)
                .collect::<Vec<_>>();
            session.update_paint_selection(parse_token(&args.token)?, &points)?;
            Ok(session.paint_selection_json(0, 100))
        })
    }

    #[tool(
        description = "Inspect Paint selection settings, active stroke token/camera/cursor, sampled-pixel count, matches, limits and visibility rules. offset/limit paginate matches, up to 500 per page. Previews are shared with the UI and appear in render_view.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_paint_selection(
        &self,
        Parameters(args): Parameters<MaterialSearchPageArgs>,
    ) -> ToolResult {
        ok(self
            .session()
            .paint_selection_json(args.offset.unwrap_or(0), args.limit.unwrap_or(100)))
    }

    #[tool(
        description = "Commit the exact paint-selection stroke using its replace/add/remove/toggle rule. Each matched object/face is combined once. Changes selection only, without map edits or undo history; equivalent to releasing a human paint drag or Apply selection."
    )]
    pub(in crate::mcp) fn commit_paint_selection(
        &self,
        Parameters(args): Parameters<VolumeTokenArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let matches =
                session.commit_paint_selection(Actor::Agent, parse_token(&args.token)?)?;
            Ok(
                json!({"matched_objects":matches.objects.len(),"matched_faces":matches.faces.len(),"selected_objects":session.selected_ids().len(),"selected_faces":session.selected_faces().len()}),
            )
        })
    }

    #[tool(
        description = "Cancel the exact active paint stroke, discarding its highlights and brush guide while preserving selection and history. Stale tokens cannot cancel another client's stroke."
    )]
    pub(in crate::mcp) fn cancel_paint_selection(
        &self,
        Parameters(args): Parameters<VolumeTokenArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.cancel_paint_selection(parse_token(&args.token)?)?;
            Ok(session.paint_selection_json(0, 100))
        })
    }

    #[tool(
        description = "Select objects or faces through an axis-aligned world-space box. Shared with Select a volume in the UI. Corners must be ordered, finite and within ±65536 units. Geometry-based contained/intersecting rules; replace/add/remove/toggle. Hidden geometry and hidden tool brushes are skipped; terrain uses actual displaced sheets and excludes parent volumes/faces. preview=true shows a blue guide and resulting selection highlights without changing selection/history. It survives camera movement; map, selection or tool-visibility changes invalidate it. Inspect paginated get_volume_selection and use its token to commit/cancel/frame."
    )]
    pub(in crate::mcp) fn select_volume(
        &self,
        Parameters(args): Parameters<VolumeArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let faces = match args.selection_mode.as_deref() {
                None => session.face_mode(),
                Some("objects") => false,
                Some("faces") => true,
                _ => return Err("selection_mode must be objects or faces".into()),
            };
            let rule = match args.rule.as_deref().unwrap_or("intersecting") {
                "intersecting" => map_core::region::Match::Intersecting,
                "contained" => map_core::region::Match::Contained,
                _ => return Err("rule must be intersecting or contained".into()),
            };
            let token = session.preview_volume_selection(crate::session::volume::Spec {
                mins: DVec3::from_array(args.mins),
                maxs: DVec3::from_array(args.maxs),
                rule,
                mode: parse_select_mode(args.mode.as_deref().unwrap_or("replace"))?,
                faces,
            })?;
            if args.preview {
                Ok(session.volume_selection_json(0, 100))
            } else {
                let matches = session.commit_volume_selection(Actor::Agent, token)?;
                Ok(
                    json!({"matched_objects":matches.objects.len(),"matched_faces":matches.faces.len(),"selected_objects":session.selected_ids().len(),"selected_faces":session.selected_faces().len()}),
                )
            }
        })
    }

    #[tool(
        description = "Inspect the shared volume selection, candidate IDs/counts and suggested selection/map bounds. offset/limit paginate candidates (maximum 500). Blue guide and highlights appear in every projection.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_volume_selection(
        &self,
        Parameters(args): Parameters<MaterialSearchPageArgs>,
    ) -> ToolResult {
        ok(self
            .session()
            .volume_selection_json(args.offset.unwrap_or(0), args.limit.unwrap_or(100)))
    }

    #[tool(
        description = "Apply the shared volume selection by token. Changes selection only, never map contents or undo history. Rejects a closed or superseded preview."
    )]
    pub(in crate::mcp) fn commit_volume_selection(
        &self,
        Parameters(args): Parameters<VolumeTokenArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let matches =
                session.commit_volume_selection(Actor::Agent, parse_token(&args.token)?)?;
            Ok(
                json!({"matched_objects":matches.objects.len(),"matched_faces":matches.faces.len(),"selected_objects":session.selected_ids().len(),"selected_faces":session.selected_faces().len()}),
            )
        })
    }

    #[tool(
        description = "Discard a volume selection preview by token, preserving committed selection and map history. Equivalent to Cancel/Escape/closing its human dialog."
    )]
    pub(in crate::mcp) fn cancel_volume_selection(
        &self,
        Parameters(args): Parameters<VolumeTokenArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .cancel_volume_selection(parse_token(&args.token)?)?;
            Ok(json!({"preview":false}))
        })
    }

    #[tool(
        description = "Frame the previewed volume in the current projection without committing selection. Requires the current volume token. aspect defaults to 1.6."
    )]
    pub(in crate::mcp) fn frame_volume_selection(
        &self,
        Parameters(args): Parameters<VolumeFrameArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session
                .frame_volume_selection(parse_token(&args.token)?, args.aspect.unwrap_or(1.6))?;
            Ok(camera_json(&session.camera()))
        })
    }

    #[tool(
        description = "Select objects or faces through a viewport rectangle, matching the human marquee in perspective/top/front/side views. Uses actual brush/face geometry, point-entity bounds, hidden-object and tool-texture filters; includes geometry through depth. Brush entities select as owners. Contained requires the entire visible object/face inside; intersecting accepts overlap. preview=true leaves selection/history unchanged and shows candidate highlights. Camera overrides do not move the user."
    )]
    pub(in crate::mcp) fn select_area(&self, Parameters(args): Parameters<AreaArgs>) -> ToolResult {
        run(|| {
            let pointer = crate::session::Pointer {
                position: glam::DVec2::from_array(args.start),
                size: (args.width, args.height),
            }
            .validate()?;
            let mut session = self.session();
            let camera = args
                .camera
                .unwrap_or_default()
                .resolve(&session, args.width as f32 / args.height as f32)?;
            let rule = args
                .rule
                .map(|rule| area_rule(&rule))
                .transpose()?
                .unwrap_or(session.area_rule());
            let mode = args
                .mode
                .map(|mode| parse_select_mode(&mode))
                .transpose()?
                .unwrap_or(session.area_mode());
            let matches = session.preview_area_selection(
                Actor::Agent,
                camera,
                pointer,
                glam::DVec2::from_array(args.end),
                rule,
                mode,
            )?;
            if !args.preview {
                session.commit_area_selection()?;
            }
            Ok(json!({"matches": area_matches(&matches), "state": area_state(&session)}))
        })
    }

    #[tool(
        description = "Inspect shared area-selection options and draft: viewport rectangle, camera, object/face mode and matching IDs.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_area_selection(&self) -> ToolResult {
        ok(area_state(&self.session()))
    }

    #[tool(
        description = "Apply the shared area-selection draft with its replace/add/remove/toggle rule. Selection changes do not add map undo history."
    )]
    pub(in crate::mcp) fn commit_area_selection(&self) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let matches = session.commit_area_selection()?;
            Ok(json!({"matches": area_matches(&matches), "state": area_state(&session)}))
        })
    }

    #[tool(
        description = "Cancel the shared area-selection draft and invalidate an unfinished human marquee without changing selection or map history."
    )]
    pub(in crate::mcp) fn cancel_area_selection(&self) -> ToolResult {
        let mut session = self.session();
        session.cancel_area_selection();
        ok(area_state(&session))
    }

    #[tool(
        description = "Frame the current selection, or the whole map when nothing is selected, preserving the perspective/top/front/side projection."
    )]
    pub(in crate::mcp) fn frame_selection(
        &self,
        Parameters(args): Parameters<FrameArgs>,
    ) -> ToolResult {
        run(|| {
            let aspect = args.aspect.unwrap_or(1.0);
            if !aspect.is_finite() || aspect <= 0.0 {
                return Err("aspect must be positive and finite".into());
            }
            let mut session = self.session();
            session.frame_selection(Actor::Agent, aspect)?;
            Ok(camera_json(&session.camera()))
        })
    }
}

#[cfg(test)]
#[path = "selection_filter_tests.rs"]
mod selection_filter_tests;
