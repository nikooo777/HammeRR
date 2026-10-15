use super::super::*;

#[tool_router(router = files_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Authored map statistics shared with Map statistics in the editor. Counts committed data including hidden objects and nested entity brushes; previews are excluded. Objects count world brushes plus entities, brush_sides count authored planes (not rendered or compiled faces). Lists materials by side usage, entity classes by entity count, external model paths by entity references, lightmap scales and displacement powers by side count. Material paths normalize case, slashes, materials/ and .vmt. Hidden brushes inherit entity visibility. Counts are not compiled BSP budgets or asset validation. Rows sort by descending usage then name. Default 50 rows, maximum 200; follow next_offset for all entries.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_map_statistics(
        &self,
        Parameters(args): Parameters<StatisticsArgs>,
    ) -> ToolResult {
        run(|| {
            let report = self.session().map_statistics(
                args.section,
                args.offset,
                args.limit.unwrap_or(crate::session::statistics::PAGE_SIZE),
            )?;
            serde_json::to_value(report).map_err(|error| error.to_string())
        })
    }

    #[tool(
        description = "Summary of the open map: file, unsaved state, object counts, bounds, selection, undo history, camera, the last material_reload report and recent activity by the user and agents.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_map_info(&self) -> ToolResult {
        let rendering = self.rendering_status();
        let mut session = self.session();
        session.poll_game();
        let skybox = skybox_state(&mut session);
        let doc = session.document();
        let vmf = doc.vmf();
        let point_entities = vmf.entities.iter().filter(|e| e.solids.is_empty()).count();
        let bounds = query::union(doc.objects().filter_map(|id| query::bounds(doc, id)));
        let activity: Vec<String> = session
            .recent_log()
            .take(10)
            .map(|e| format!("{}: {}", e.actor.label(), e.text))
            .collect();
        let mut result = json!({
            "rendering": rendering,
            "material_reload": session.material_reload_report(),
            "compile_region": session.compile_region_build_scope(),
            "context": session.context_state(),
            "primitive": session.primitive_state(),
            "repeat_array": session.repeat_array(),
            "clipboard": session.clipboard_report(),
            "map_checks": {"report":session.map_check_report().map(|report|report.token.to_string()),"stale":session.map_checks_stale()},
            "path": session.path().map(|p| p.display().to_string()),
            "title": session.title(),
            "unsaved_changes": doc.is_dirty(),
            "game": session.game().game_dir.display().to_string(),
            "game_error": session.game_error(),
            "entity_definitions": definitions_json(&session),
            "compiling": session.compiling(),
            "launching": session.launching(),
            "game_running": session.game_running(),
            "pointfile": session.pointfile_report(0, 1).ok(),
            "components": session.component_report(&[]).ok(),
            "hollow": session.hollow_state(),
            "leak": session.leak().map(|leak| json!({"entity": format!("entity:{}", leak.entity), "points": leak.points.iter().copied().map(point).collect::<Vec<_>>()})),
            "view": view_state(session.view()),
            "skybox": skybox,
            "spatial": session.spatial(),
            "block_preview": bounds_json(session.block_preview()),
            "clipping": clip_state(&session),
            "transform": transform_state(&session),
            "area_selection": area_state(&session),
            "volume_selection": {"preview":session.volume_preview().map(|preview|preview.token.to_string())},
            "paint_selection": session.paint_selection_json(0, 100),
            "connection_draft": connection_draft_json(&session),
            "rename_draft": rename_draft_json(&session),
            "property_draft": session.property_draft(),
            "visibility": session.visibility_state(),
        });
        result.as_object_mut().unwrap().extend([
            ("recovery".into(), session.recovery_status()),
            ("face_selection".into(), face_state(&session)),
            ("world_brushes".into(), json!(vmf.world.solids.len())),
            ("point_entities".into(), json!(point_entities)),
            (
                "brush_entities".into(),
                json!(vmf.entities.len() - point_entities),
            ),
            (
                "worldspawn".into(),
                json!(
                    vmf.world
                        .properties
                        .iter()
                        .map(|(k, v)| json!([k, v]))
                        .collect::<Vec<_>>()
                ),
            ),
            ("bounds".into(), bounds_json(bounds)),
            (
                "selection".into(),
                json!(
                    session
                        .selection()
                        .iter()
                        .map(|&id| id_string(id))
                        .collect::<Vec<_>>()
                ),
            ),
            (
                "undo".into(),
                json!(doc.undo_labels().take(10).collect::<Vec<_>>()),
            ),
            (
                "redo".into(),
                json!(doc.redo_labels().take(10).collect::<Vec<_>>()),
            ),
            ("camera".into(), camera_json(&session.camera())),
            ("recent_activity".into(), json!(activity)),
        ]);
        ok(result)
    }

    #[tool(description = "Open a .vmf map file in the editor.")]
    pub(in crate::mcp) fn open_map(&self, Parameters(args): Parameters<OpenMapArgs>) -> ToolResult {
        run(|| {
            let message = self.session().open(
                Actor::Agent,
                &PathBuf::from(&args.path),
                args.discard_changes,
            )?;
            Ok(json!({ "message": message }))
        })
    }

    #[tool(
        description = "List up to 20 recent maps, most recent first, with file availability, retained game directory and build settings, context compatibility, persistence status and storage errors. Read only; missing files remain in the list until explicitly removed.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_recent_maps(&self) -> ToolResult {
        ok(self.session().recent_maps())
    }

    #[tool(
        description = "Reopen an exact path from get_recent_maps through the normal map validation and dirty/discard guards. Restores retained build settings only for matching mounted game content and while no compile is active. Does not switch game content. Failed opens preserve the map, history and recent list."
    )]
    pub(in crate::mcp) fn open_recent_map(
        &self,
        Parameters(args): Parameters<OpenMapArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let message = session.open_recent_map(
                Actor::Agent,
                Path::new(&args.path),
                args.discard_changes,
            )?;
            Ok(
                json!({"message": message, "recent_maps": session.recent_maps(), "build_settings": session.compile_settings()}),
            )
        })
    }

    #[tool(
        description = "Remove one exact path returned by get_recent_maps from the retained list. Does not delete or edit the map file, current map or undo history. Failed storage writes preserve the list."
    )]
    pub(in crate::mcp) fn remove_recent_map(
        &self,
        Parameters(args): Parameters<RecentMapPathArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.remove_recent_map(Path::new(&args.path))?;
            Ok(session.recent_maps())
        })
    }

    #[tool(
        description = "Clear the retained recent-map list and its saved per-map build context. Does not delete map files or edit the current map. Failed storage writes preserve the previous list."
    )]
    pub(in crate::mcp) fn clear_recent_maps(&self) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.clear_recent_maps()?;
            Ok(session.recent_maps())
        })
    }

    #[tool(description = "Start a new empty map.")]
    pub(in crate::mcp) fn new_map(&self, Parameters(args): Parameters<NewMapArgs>) -> ToolResult {
        run(|| {
            self.session().new_map(Actor::Agent, args.discard_changes)?;
            Ok(json!({ "message": "new map" }))
        })
    }

    #[tool(description = "Save the map as .vmf, to its current file or a new path.")]
    pub(in crate::mcp) fn save_map(&self, Parameters(args): Parameters<SaveMapArgs>) -> ToolResult {
        run(|| {
            let path = self.session().save(
                Actor::Agent,
                args.path.as_ref().map(PathBuf::from).as_deref(),
            )?;
            Ok(json!({ "saved": path.display().to_string() }))
        })
    }

    #[tool(
        description = "Find objects by classname, targetname, material, kind and/or a bounding box. Returns summaries with ids and bounds.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn find_objects(
        &self,
        Parameters(args): Parameters<FindArgs>,
    ) -> ToolResult {
        run(|| {
            let session = self.session();
            let found = session.object_report(&ObjectFilter {
                query: args.query,
                hidden: args.hidden,
                classname: args.classname,
                targetname: args.targetname,
                material: args.material,
                kind: args.kind,
                within: args.within,
            })?;
            let offset = args.offset.unwrap_or(0);
            let objects = found
                .iter()
                .skip(offset)
                .take(args.limit.unwrap_or(50).min(500))
                .map(|row| {
                    let mut value = summary(session.document(), row.object);
                    value["bounds"] =
                        bounds_json(row.bounds.map(|[low, high]| (vec3(low), vec3(high))));
                    value["hidden"] = json!(row.hidden);
                    value["hidden_brushes"] = json!(row.hidden_brushes);
                    value
                })
                .collect::<Vec<_>>();
            Ok(json!({"total":found.len(), "offset":offset, "objects":objects}))
        })
    }

    #[tool(
        description = "Full details of objects: entity key/values, outputs and brushes; brush faces with side ids, materials, normals, planes and texture axes. Defaults to the selection.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_objects(&self, Parameters(args): Parameters<IdsArgs>) -> ToolResult {
        run(|| {
            let session = self.session();
            let ids = ids_or_selection(&session, args.ids.as_deref())?;
            let model_bounds = session.model_bounds_in(session.document());
            let objects = ids
                .iter()
                .map(|&id| {
                    let mut value = detail(session.document(), id)?;
                    if let Some(&bounds) = model_bounds.get(&id) {
                        value["bounds"] = bounds_json(Some(bounds));
                    }
                    let context = session.object_context_summary(id);
                    if !context.is_null() {
                        value["context"] = context;
                    }
                    Ok::<_, String>(value)
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(json!({ "objects": objects }))
        })
    }

    #[tool(description = "Undo the last steps, whoever made them.")]
    pub(in crate::mcp) fn undo(&self, Parameters(args): Parameters<StepsArgs>) -> ToolResult {
        let mut session = self.session();
        let undone: Vec<String> = (0..args.steps.unwrap_or(1))
            .map_while(|_| session.undo(Actor::Agent))
            .collect();
        ok(json!({ "undone": undone }))
    }

    #[tool(description = "Redo undone steps.")]
    pub(in crate::mcp) fn redo(&self, Parameters(args): Parameters<StepsArgs>) -> ToolResult {
        let mut session = self.session();
        let redone: Vec<String> = (0..args.steps.unwrap_or(1))
            .map_while(|_| session.redo(Actor::Agent))
            .collect();
        ok(json!({ "redone": redone }))
    }
}
