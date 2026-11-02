use super::super::*;

#[tool_router(router = working_region_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect session-local working-region viewport filters: snapshot, enabled saved mins/maxs, active state, options (draw_distance, follow_camera, filter), effective_mins/effective_maxs, camera_position, exact total/excluded/selected_excluded counts and paginated object/child-brush eligibility, reasons and bounds source. offset defaults 0; limit defaults 50 (1–200). Box, camera distance and object-report query rules combine with AND at whole-owner scope; exclusions report outside_working_region, outside_draw_distance or object_filter. Camera-following preserves saved box dimensions; distance measures the closest owner-bound point. Models use asset bounds with explicit helper fallback; unresolved bounds stay visible to spatial filters. Saved Hide, editor-hidden exclusions and tool textures apply independently. This display filter never edits saved source, selection, history or compile scope.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_working_region(
        &self,
        Parameters(args): Parameters<WorkingRegionInspectArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .get_working_region(args.offset, args.limit.unwrap_or(50))
        })
    }

    #[tool(
        description = "Change shared editor-only working-region filters using the latest get_working_region.snapshot. action=set requires finite ordered mins/maxs within editor bounds; enabled defaults true; an options object atomically replaces all options, while omitted/null options preserves them. action=selection_bounds captures current object/face selection (empty rejects), accepts enabled only and preserves options. action=configure requires an options object only and preserves saved box/enabled. options contains optional draw_distance (positive finite camera distance), follow_camera (defaults false; centers enabled saved box dimensions on camera) and filter (same query/classname/exact_class/targetname/material/kind/hidden/within rules as get_object_report). Omitted options fields reset to defaults. Box, distance and query rules combine with AND for whole owners. action=clear accepts snapshot only and resets box and options. Unknown, contradictory, stale or invalid requests preserve settings and drafts. Changes refresh scenes/picking and cancel active spatial gestures while preserving source, selection, Undo and owned geometry/overlay drafts. UI: Working region… / Clear working region."
    )]
    pub(in crate::mcp) fn set_working_region(
        &self,
        Parameters(args): Parameters<WorkingRegionSetArgs>,
    ) -> ToolResult {
        run(|| {
            use crate::session::working_region::Action;
            let (action, snapshot) = match args {
                WorkingRegionSetArgs::Set {
                    snapshot,
                    mins,
                    maxs,
                    enabled,
                    options,
                } => {
                    let bounds = [DVec3::from_array(mins), DVec3::from_array(maxs)];
                    let enabled = enabled.unwrap_or(true);
                    let action = match options {
                        Some(options) => Action::SetWithOptions {
                            bounds,
                            enabled,
                            options,
                        },
                        None => Action::Set { bounds, enabled },
                    };
                    (action, snapshot)
                }
                WorkingRegionSetArgs::SelectionBounds { snapshot, enabled } => (
                    Action::SelectionBounds {
                        enabled: enabled.unwrap_or(true),
                    },
                    snapshot,
                ),
                WorkingRegionSetArgs::Configure { snapshot, options } => {
                    (Action::Configure { options }, snapshot)
                }
                WorkingRegionSetArgs::Clear { snapshot } => (Action::Clear, snapshot),
            };
            self.session()
                .set_working_region(Actor::Agent, action, &snapshot)
        })
    }
    #[tool(
        description = "Inspect temporary editor-hidden committed root IDs and a current snapshot. offset defaults 0; limit defaults 50 (1–200). This filter affects viewport rendering and spatial hits independently of saved Hide, object-selection type flags, tool textures and working region. Explicit-ID reports and edits remain available. New/Open resets it; removed source IDs are pruned after edits/Undo/Redo.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_editor_visibility(
        &self,
        Parameters(args): Parameters<WorkingRegionInspectArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .get_editor_visibility(args.offset, args.limit.unwrap_or(50))
        })
    }

    #[tool(
        description = "Change temporary editor visibility using the latest get_editor_visibility snapshot. action=hide accepts optional root ids (defaults current selection), validates the entire batch, and adds exclusions without further group expansion or changing selection/source/history/builds. Apply or cancel displayed geometry previews before Hide. action=show_all accepts snapshot only, clears only temporary exclusions, and remains available during previews, Faces mode, empty selection, disabled pick types and empty working regions. Real changes cancel active spatial gestures while preserving inactive owned reviews; stale/invalid/no-op requests leave drafts unchanged. UI: Selection → Hide selected in editor / Show all editor-hidden."
    )]
    pub(in crate::mcp) fn set_editor_visibility(
        &self,
        Parameters(args): Parameters<EditorVisibilitySetArgs>,
    ) -> ToolResult {
        run(|| {
            use crate::session::editor_visibility::Action;
            let mut session = self.session();
            let (action, snapshot) = match args {
                EditorVisibilitySetArgs::Hide { snapshot, ids } => (
                    Action::Hide(ids_or_selection(&session, ids.as_deref())?),
                    snapshot,
                ),
                EditorVisibilitySetArgs::ShowAll { snapshot } => (Action::ShowAll, snapshot),
            };
            session.set_editor_visibility(Actor::Agent, action, &snapshot)
        })
    }
}
