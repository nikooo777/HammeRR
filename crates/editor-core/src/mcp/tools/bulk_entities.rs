use super::super::*;

#[tool_router(router = bulk_entities_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Preview changing 1–64 captured whole entities to one loaded compatible FGD class. Every retained property is validated against the future class before acceptance; unknown properties and connections remain, no defaults are inserted. Rejects brush/point kind mismatches, child-brush/world selections, duplicate targets, malformed retained fields, and batches above 256 brushes, 8192 sides or the shared 64 MiB snapshot admission. Returns an owned bulk_entities token and exact before/after targets. Map, selection and Undo remain unchanged; invalid requests retain the previous preview. UI: Entity → Change entity classes…."
    )]
    pub(in crate::mcp) fn preview_entity_class_change(
        &self,
        Parameters(args): Parameters<BulkEntityClassArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let ids = BulkEntityTargetsArgs {
                ids: args.ids,
                generation: args.generation,
                revision: args.revision,
                definitions: args.definitions,
            }
            .targets(&session)?;
            session.preview_entity_class_change(&ids, &args.classname)
        })
    }

    #[tool(
        description = "Preview moving every brush from 1–64 captured whole brush entities to the world in one Undo step. Retains authored source geometry, brush/side IDs, UVs and supported group/layer/instance order metadata; runtime poses are not baked. Shows removed classname, origin, angles, all other behavior properties and outgoing connections. Incoming references are not rewritten and the aggregate warning reports when they may become unresolved. Attached entity notes, measurement bindings, origin custody or opaque metadata that cannot survive removal reject the entire request. Rejects points, child brushes and mixed invalid members without skips. Apply or Cancel uses the returned bulk_entities token. UI: Entity → Move to world."
    )]
    pub(in crate::mcp) fn preview_move_entities_to_world(
        &self,
        Parameters(args): Parameters<BulkEntityTargetsArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let ids = args.targets(&session)?;
            session.preview_move_entities_to_world(&ids)
        })
    }

    #[tool(
        description = "Inspect the shared bulk entity class-change or Move to world preview, captured targets, exact removed behavior, aggregate incoming-reference warning, source generation/revision/definitions, token and Apply availability. Selection changes never retarget it; map or FGD changes make it stale.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_bulk_entities(&self) -> ToolResult {
        ok(self.session().bulk_entities_report())
    }

    #[tool(
        description = "Apply the matching current bulk_entities preview atomically as one Undo step and select the resulting entities or world brushes. Rejects replaced tokens or changed map/FGD sources without mutation. UI: entity action preview → Apply."
    )]
    pub(in crate::mcp) fn apply_bulk_entities(
        &self,
        Parameters(args): Parameters<BulkEntityTokenArgs>,
    ) -> ToolResult {
        run(|| self.session().apply_bulk_entities(Actor::Agent, args.token))
    }

    #[tool(
        description = "Cancel the matching bulk_entities preview, including a stale one, without changing the map or Undo. A replaced token cannot cancel the new preview. UI: entity action preview → Cancel."
    )]
    pub(in crate::mcp) fn cancel_bulk_entities(
        &self,
        Parameters(args): Parameters<BulkEntityTokenArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.cancel_bulk_entities(args.token)?;
            Ok(json!({"cancelled":true}))
        })
    }
}
