use super::super::*;

#[tool_router(router = files_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Close this fixed document and quit its owning editor process. Requires the exact review_token from get_map_info.document_close; discard_changes explicitly permits that reviewed unsaved map/draft loss. Stale/busy/foreign or missing-consent requests leave the document open. Accepted Close is irreversible and survives client cancellation: it stops owned game/audio, cancels and physically drains owned work/recovery, and retires the actual native UI. The result reports completed document/UI cleanup; the host then verifies local response output and exits. Output/transport/process-exit failures remain host errors and cannot be attested by their own response. No document selector or multiple-document support."
    )]
    pub(in crate::mcp) async fn close_document(
        &self,
        Parameters(args): Parameters<CloseDocumentArgs>,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> ToolResult {
        let Some(terminal) = &self.terminal else {
            return fail("An owning terminal host is required for Close");
        };
        let response =
            match terminal.commit_mcp(self, &args.review_token, args.discard_changes, &context) {
                Ok(response) => response,
                Err(error) => return fail(error),
            };
        match response.await {
            Ok(result) => Ok(result),
            Err(_) => {
                fail("The accepted Close result owner was lost; terminal cleanup remains required")
            }
        }
    }

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
        description = "Summary of the open map: file, unsaved state, object counts, bounds, selection, undo history, camera, the last material_reload report and recent activity by the user and agents. Includes generation, an opaque Session-scoped replacement_review for guarded New/Open/Recent/Recovery calls, and document_close inspection of the same reviewed terminal Close state used by the native window. Inspection does not close the document.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_map_info(&self) -> ToolResult {
        let rendering = self.rendering_status();
        let mut session = self.session();
        session.poll_game();
        let skybox = skybox_state(&mut session);
        let sky3d_preview = session.sky3d_preview_state();
        let working_region = session.working_region_summary();
        let document_close = self._document.as_ref().map_or_else(
            || session.document_close_review().inspection(),
            |document| {
                document
                    .close_review_excluding(&session, self.operation.as_ref())
                    .inspection()
            },
        );
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
            "working_region": working_region,
            "material_reload": session.material_reload_report(),
            "compile_region": session.compile_region_build_scope(),
            "context": session.context_state(),
            "primitive": session.primitive_state(),
            "repeat_array": session.repeat_array(),
            "clipboard": session.clipboard_report(),
            "map_checks": {"report":session.map_check_report().map(|report|report.token.to_string()),"stale":session.map_checks_stale()},
            "path": session.path().map(|p| p.display().to_string()),
            "title": session.title(),
            "generation": session.document_generation(),
            "replacement_review": session.buffer_replacement_review().token(),
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
            "sky3d_preview": sky3d_preview,
            "fog_preview": session.fog_preview_state(),
            "rope_preview": session.rope_preview_state(),
            "spatial": session.spatial(),
            "grid_defaults": session.grid_preferences_state(),
            "navigation": session.navigation_preferences_state(),
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
            ("document_close".into(), document_close),
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
                    session
                        .world_properties()
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

    #[tool(
        description = "Open a .vmf map file in the editor. Optional expected_review is the opaque replacement_review from get_map_info; a stale review rejects without replacing the map. discard_changes remains required for unsaved edits."
    )]
    pub(in crate::mcp) fn open_map(&self, Parameters(args): Parameters<OpenMapArgs>) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let review = match args.expected_review.as_deref() {
                Some(token) => session.buffer_replacement_review_from_token(token)?,
                None => session.buffer_replacement_review(),
            };
            let message = session.open_reviewed(
                Actor::Agent,
                &PathBuf::from(&args.path),
                args.discard_changes,
                &review,
            )?;
            Ok(json!({ "message": message }))
        })
    }

    #[tool(
        description = "List up to 20 recent maps with exact paths, file availability, frozen gameinfo/FGD/engine/Steam paths and build settings, context compatibility, list serial, worker status and storage errors. Legacy entries report their compatible-build restore behavior. Missing files remain until explicitly removed or relocated. Does not reload storage, open a map or switch content.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_recent_maps(
        &self,
        Parameters(_args): Parameters<RecentMapsInspectArgs>,
    ) -> ToolResult {
        ok(self.session().recent_maps())
    }

    #[tool(
        description = "Reload the saved Recent maps list through the same Session operation as Reload saved list in the editor. Clears a stale-writer conflict only after a successful bounded read and validation. Failed Reload preserves the previous valid list and exposes the storage error. Does not open/save a map, restore build settings, change selection/camera/history, or apply/cancel drafts. get_recent_maps only inspects the current list."
    )]
    pub(in crate::mcp) fn reload_recent_maps(
        &self,
        Parameters(_args): Parameters<RecentMapsInspectArgs>,
    ) -> ToolResult {
        run(|| self.session().reload_recent_maps())
    }

    #[tool(
        description = "Reopen an exact listed path using get_recent_maps.serial and optional get_map_info.replacement_review. Prepares the map and remembered game/FGD/build/launch context in the background, then adopts them together after buffer review, discard consent, source and list freshness checks. Legacy entries retain compatible idle build-setting restoration. A failed selected context restore refuses without fallback and preserves current map/content. Does not launch a game."
    )]
    pub(in crate::mcp) async fn open_recent_map(
        &self,
        Parameters(args): Parameters<OpenRecentMapArgs>,
    ) -> ToolResult {
        let request = {
            let mut session = self.session();
            let review = match args.expected_review.as_deref() {
                Some(token) => match session.buffer_replacement_review_from_token(token) {
                    Ok(review) => review,
                    Err(error) => return fail(error),
                },
                None => session.buffer_replacement_review(),
            };
            match session.begin_open_recent_map(
                Path::new(&args.path),
                args.discard_changes,
                &review,
                Some(args.serial),
            ) {
                Ok(request) => request,
                Err(error) => return fail(error),
            }
        };
        let token = request.token();
        let server = self.clone();
        match tokio::task::spawn_blocking(self.worker(move || {
            let completed = request.run();
            server.session().finish_recent_map(Actor::Agent, completed)
        }))
        .await
        {
            Ok(Ok(value)) => ok(value),
            Ok(Err(error)) => fail(error),
            Err(error) => {
                let message = format!("Recent map open worker failed: {error}");
                self.session().fail_recent_map(token, message.clone());
                fail(message)
            }
        }
    }

    #[tool(
        description = "Relocate one exact Recent maps record to an explicit existing VMF path using get_recent_maps.serial. The shared Session operation validates the destination and preserves the record's remembered content context. Changed list/source records, duplicate destinations and invalid targets refuse explicitly. Does not move map/assets, guess a path, open the destination or change the current map."
    )]
    pub(in crate::mcp) async fn relocate_recent_map(
        &self,
        Parameters(args): Parameters<RelocateRecentMapArgs>,
    ) -> ToolResult {
        let request = match self.session().begin_relocate_recent_map(
            Path::new(&args.old_path),
            Path::new(&args.new_path),
            Some(args.serial),
        ) {
            Ok(request) => request,
            Err(error) => return fail(error),
        };
        let token = request.token();
        let server = self.clone();
        match tokio::task::spawn_blocking(self.worker(move || {
            let completed = request.run();
            server.session().finish_recent_map(Actor::Agent, completed)
        }))
        .await
        {
            Ok(Ok(value)) => ok(value),
            Ok(Err(error)) => fail(error),
            Err(error) => {
                let message = format!("Recent map relocation worker failed: {error}");
                self.session().fail_recent_map(token, message.clone());
                fail(message)
            }
        }
    }

    #[tool(
        description = "Cancel the exact recent-map open or relocation worker identified by get_recent_maps.worker_token. Uses the same shared cancellation as Recent maps…. Validation drains before another request can start; cancelled work does not replace the current map/content or relocate a record."
    )]
    pub(in crate::mcp) fn cancel_recent_map(
        &self,
        Parameters(args): Parameters<CancelRecentMapArgs>,
    ) -> ToolResult {
        run(|| self.session().cancel_recent_map(Actor::Agent, args.token))
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
    pub(in crate::mcp) fn clear_recent_maps(
        &self,
        Parameters(_args): Parameters<RecentMapsInspectArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.clear_recent_maps()?;
            Ok(session.recent_maps())
        })
    }

    #[tool(
        description = "Start a new empty map. Optional expected_review is the opaque replacement_review from get_map_info; a stale review rejects without replacing the map. discard_changes remains required for unsaved edits."
    )]
    pub(in crate::mcp) fn new_map(&self, Parameters(args): Parameters<NewMapArgs>) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let review = match args.expected_review.as_deref() {
                Some(token) => session.buffer_replacement_review_from_token(token)?,
                None => session.buffer_replacement_review(),
            };
            session.new_map_reviewed(Actor::Agent, args.discard_changes, &review)?;
            Ok(json!({ "message": "new map" }))
        })
    }

    #[tool(
        description = "Save the map as .vmf. Omit path to use the retained current source binding; supply path for an explicit Save as choice that captures its current destination, even for the same filename. Changed implicit bindings and owned source/output conflicts reject before publication."
    )]
    pub(in crate::mcp) fn save_map(&self, Parameters(args): Parameters<SaveMapArgs>) -> ToolResult {
        run(|| {
            let path = self.session().save_for_generation(
                Actor::Agent,
                args.path.as_ref().map(PathBuf::from).as_deref(),
                args.expected_generation,
            )?;
            Ok(json!({ "saved": path.display().to_string() }))
        })
    }

    #[tool(
        description = "Find objects by classname, targetname, material, kind and/or a bounding box through the shared object filter. classname matches a case-insensitive substring unless exact_class=true. Optional classnames is a bounded nonempty list of exact literal entity classnames, matched as OR and combined with all other fields as AND; it never matches world solids or expands inheritance/wildcards. Invalid or unknown filter arguments reject. Returns summaries with ids and bounds.",
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
                classnames: args.classnames,
                exact_class: args.exact_class,
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
        description = "Full details of objects: entity key/values, outputs and brushes; brush faces with side ids, materials, normals, planes and texture axes. Defaults to the selection. Reported presentation bounds include each enabled admitted rope's outgoing span and source helper; transform pivots/anchors and raw map statistics remain authored bounds.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_objects(&self, Parameters(args): Parameters<IdsArgs>) -> ToolResult {
        run(|| {
            let session = self.session();
            let ids = match args.ids.as_deref() {
                Some([]) => {
                    return Err(
                        "explicit ids must not be empty; omit ids to use the selection".into(),
                    );
                }
                Some(ids) => parse_ids(ids)?,
                None => ids_or_selection(&session, None)?,
            };
            let model_bounds = session.presentation_bounds_in(session.document(), None);
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
