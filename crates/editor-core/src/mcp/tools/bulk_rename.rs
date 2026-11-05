use super::super::*;

#[tool_router(router = bulk_rename_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Capture 1–256 complete entities for simultaneous distinct-name rename. Requires current generation/revision and uses explicit IDs or the complete entity selection. Captured membership remains fixed when selection changes. Replaces the shared single/group/batch rename draft without editing the map. Returns every original/proposed name and affected reference. Swaps and chains resolve from the original graph; ambiguous duplicate-name groups, unsafe wildcard/class changes and unresolved dynamic/scoped bindings block Apply with evidence."
    )]
    pub(in crate::mcp) fn begin_entity_rename_batch(
        &self,
        Parameters(args): Parameters<BeginRenameBatchArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            if args.generation != session.document_generation()
                || args.revision != session.document().revision()
            {
                return Err("Map changed; inspect the entity set again".into());
            }
            let ids = ids_or_selection(&session, args.ids.as_deref())?
                .into_iter()
                .map(|id| match id {
                    ObjectId::Entity(id) => Ok(id),
                    _ => Err("Bulk rename requires complete entities".to_owned()),
                })
                .collect::<Result<Vec<_>, _>>()?;
            let token = session.begin_entity_rename_batch(&ids)?;
            rename_batch_json(&session, token)
        })
    }

    #[tool(
        description = "Replace all proposed names in the captured rename set using explicit {entity,new_name} entries. Supply every member exactly once. No templates or implicit suffixes. Returns the complete simultaneous reference preview. A changed proposal returns a new token, invalidating earlier tokens. No map mutation; stale/replaced map or definitions refuses."
    )]
    pub(in crate::mcp) fn update_entity_rename_batch(
        &self,
        Parameters(args): Parameters<RenameBatchNamesArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let names = args
                .names
                .into_iter()
                .map(|name| match parse_id(&name.entity)? {
                    ObjectId::Entity(id) => Ok((id, name.new_name)),
                    _ => Err("Bulk rename requires entity IDs".to_owned()),
                })
                .collect::<Result<Vec<_>, String>>()?;
            let token = session.update_entity_rename_batch(args.token, &names)?;
            rename_batch_json(&session, token)
        })
    }

    #[tool(
        description = "Read the complete captured simultaneous name/reference preview. Requires its current token; map replacement, edits, definition changes or another rename proposal refuse.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_entity_rename_batch(
        &self,
        Parameters(args): Parameters<RenameBatchTokenArgs>,
    ) -> ToolResult {
        run(|| rename_batch_json(&self.session(), args.token))
    }

    #[tool(
        description = "Apply the reviewed simultaneous entity names and all exact reference rewrites as one atomic undoable edit. Swaps/chains use original identities. A blocked, unchanged, stale or replaced proposal refuses without partial writes. Use the latest token returned by update_entity_rename_batch."
    )]
    pub(in crate::mcp) fn commit_entity_rename_batch(
        &self,
        Parameters(args): Parameters<RenameBatchTokenArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let plan = session.commit_entity_rename_batch(Actor::Agent, args.token)?;
            Ok(
                json!({"applied":true, "token":null, "entities":plan.names.len(),
                "reference_fields":plan.references.iter().filter(|change| change.changed).count()}),
            )
        })
    }

    #[tool(
        description = "Cancel the exact shared bulk rename draft without editing the map/history. A stale map draft can be cancelled; a replaced draft token cannot cancel its replacement."
    )]
    pub(in crate::mcp) fn cancel_entity_rename_batch(
        &self,
        Parameters(args): Parameters<RenameBatchTokenArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().cancel_entity_rename_batch(args.token)?;
            Ok(json!({"cancelled":true,"token":null}))
        })
    }
}
