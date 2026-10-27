use super::super::*;

#[tool_router(router = checks_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Read the shared map-check report with severity/search filtering and pagination. Stale reports remain readable; rerun checks before selecting or repairing an issue. Includes explicit repair preview state and check limitations.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_map_checks(
        &self,
        Parameters(args): Parameters<MapChecksArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().map_checks_json(
                args.severity.as_deref(),
                args.query.as_deref().unwrap_or_default(),
                args.offset.unwrap_or_default(),
                args.limit.unwrap_or(50),
            )
        })
    }

    #[tool(
        description = "Select the object or face of a current map issue, optionally framing it in the viewport. Does not reveal hidden content. Rejects map-wide issues, ambiguous IDs, stale reports and replaced report tokens."
    )]
    pub(in crate::mcp) fn select_map_issue(
        &self,
        Parameters(args): Parameters<MapIssueArgs>,
    ) -> ToolResult {
        run(|| {
            let report = args.report.parse().map_err(|_| "Invalid report token")?;
            let issue = args.issue.parse().map_err(|_| "Invalid issue ID")?;
            self.session().select_map_issue(
                Actor::Agent,
                report,
                issue,
                args.frame,
                args.aspect.unwrap_or(1.0),
            )?;
            Ok(json!({"selected":args.issue,"framed":args.frame}))
        })
    }

    #[tool(
        description = "Review explicit repairs for selected issues in the current report. Returns before/after values without changing the map or undo history. Currently repairs invalid texture projections and zero lightmap scales. Use apply_map_fixes or cancel_map_fixes with the returned preview token."
    )]
    pub(in crate::mcp) fn preview_map_fixes(
        &self,
        Parameters(args): Parameters<MapFixesArgs>,
    ) -> ToolResult {
        run(|| {
            let report = args.report.parse().map_err(|_| "Invalid report token")?;
            let issues = args
                .issues
                .iter()
                .map(|id| id.parse().map_err(|_| "Invalid issue ID"))
                .collect::<Result<Vec<_>, _>>()?;
            let mut session = self.session();
            session.preview_map_fixes(report, &issues)?;
            Ok(session.map_fix_preview_json())
        })
    }

    #[tool(
        description = "Apply the exact reviewed map-repair preview as one undoable edit, then rerun checks. Rejects stale maps, changed definitions and replaced previews."
    )]
    pub(in crate::mcp) fn apply_map_fixes(
        &self,
        Parameters(args): Parameters<MapFixTokenArgs>,
    ) -> ToolResult {
        run(|| {
            let token = args.token.parse().map_err(|_| "Invalid preview token")?;
            let mut session = self.session();
            let repaired = session.apply_map_fixes(Actor::Agent, token)?;
            Ok(json!({"repaired":repaired,"checks":session.map_checks_json(None,"",0,50)?}))
        })
    }

    #[tool(
        description = "Discard the exact map-repair preview without changing map data or undo history. Does not clear the issue report."
    )]
    pub(in crate::mcp) fn cancel_map_fixes(
        &self,
        Parameters(args): Parameters<MapFixTokenArgs>,
    ) -> ToolResult {
        run(|| {
            let token = args.token.parse().map_err(|_| "Invalid preview token")?;
            self.session().cancel_map_fixes(token)?;
            Ok(json!({"cancelled":true}))
        })
    }

    #[tool(
        description = "Searchable object report with IDs, type, class, name, materials, visibility and bounds. Same filters as the human Object report. Class, name and material filters are case-insensitive substrings; exact_class=true requires the complete class name. Default page 100, maximum 500.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_object_report(
        &self,
        Parameters(args): Parameters<ReportArgs>,
    ) -> ToolResult {
        run(|| {
            let session = self.session();
            let rows = session.object_report(&args.filter)?;
            let offset = args.offset.unwrap_or(0);
            Ok(
                json!({"total":rows.len(), "offset":offset, "objects":rows.iter().skip(offset).take(args.limit.unwrap_or(100).min(500)).collect::<Vec<_>>(), "visibility":session.visibility_state(), "generation":session.document_generation(), "revision":session.document().revision()}),
            )
        })
    }

    #[tool(
        description = "Select ALL matching report objects, including results beyond a page. Optionally frame the resulting selection. Uses shared report filters (classname is a case-insensitive substring unless exact_class=true) and replace/add/remove/toggle selection."
    )]
    pub(in crate::mcp) fn select_object_report(
        &self,
        Parameters(args): Parameters<SelectReportArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let count = session.select_report(
                Actor::Agent,
                &args.filter,
                parse_select_mode(args.mode.as_deref().unwrap_or("replace"))?,
                args.frame,
                args.aspect.unwrap_or(1.6),
            )?;
            Ok(
                json!({"matched":count, "selection":session.selected_ids().into_iter().map(id_string).collect::<Vec<_>>()}),
            )
        })
    }

    #[tool(
        description = "Hide, show, isolate, show all or restore previous visibility, in one undo step. Saved VMF hidden state includes nested brushes. Hidden objects are omitted by the compiler. Restore keeps newly created objects and ignores deleted ones."
    )]
    pub(in crate::mcp) fn set_object_visibility(
        &self,
        Parameters(args): Parameters<VisibilityArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let ids = if ["show_all", "restore"].contains(&args.action.as_str()) {
                args.ids
                    .as_deref()
                    .map(parse_ids)
                    .transpose()?
                    .unwrap_or_default()
            } else {
                ids_or_selection(&session, args.ids.as_deref())?
            };
            session.set_visibility(Actor::Agent, &args.action, &ids)?;
            Ok(session.visibility_state())
        })
    }
}
