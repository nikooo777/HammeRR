use super::super::*;

#[tool_router(router = visibility_rules_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect automatic editor-only visibility categories and the current visibility-rules snapshot. Optional query defaults empty and matches id/label by ASCII-case-insensitive substring (at most 256 bytes); offset defaults 0; limit defaults 50 (1–200). Rows report id, label, filter, total, excluded, source, available, unavailable_reason and optional membership with parent/name, class_count, numeric matching_classes and declaration sources. Counts include saved-hidden authored root owners. Empty authored categories are omitted; FGD AutoVisGroup rows remain present with zero matches. FGD groups use one entity-only exact class-set exclusion, up to 256 literal classnames; no wildcard or inheritance expansion. The excluded flag means the exact canonical rule is present, even when overlapping rules also hide objects. Failed/unloaded definitions make FGD rows unavailable; authored categories remain usable. Treat FGD IDs as opaque stable IDs. At most 4096 authored classes, each at most 256 bytes. Exclusions affect rendering and spatial picking independently of native layers/Hide/Show compilation state. Use the returned snapshot for changes; map, query-rule or definitions changes invalidate it.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_visibility_categories(
        &self,
        Parameters(args): Parameters<VisibilityCategoriesArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .visibility_categories(&args.query, args.offset, args.limit.unwrap_or(50))
        })
    }

    #[tool(
        description = "Change one available automatic editor-only category with its exact id and the latest get_visibility_categories snapshot. excluded=true adds that category's canonical exclusion; excluded=false removes only that exact rule. An FGD group consumes one exclusion and captures its current literal class membership. Preserves inclusion, overlapping exclusions and box/distance/follow-camera settings, so removing the category rule may leave objects excluded by another rule. No source, selection, Undo or compile-scope change. Unknown/unavailable categories, reloaded definitions and other stale/invalid requests reject without changes."
    )]
    pub(in crate::mcp) fn set_visibility_category(
        &self,
        Parameters(args): Parameters<VisibilityCategorySetArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().set_visibility_category(
                Actor::Agent,
                &args.id,
                args.excluded,
                &args.snapshot,
            )
        })
    }

    #[tool(
        description = "Exclude an object-report filter using the latest get_visibility_categories snapshot. Accepts query, classname, classnames, exact_class, targetname, material, kind, hidden and within with shared ObjectFilter semantics. classnames is an optional OR set of 1–256 distinct literal entity classnames, each 1–256 UTF-8 bytes without control characters or surrounding whitespace; matching is complete ASCII-case-insensitive equality, with no wildcard/inheritance expansion, and combines with all other fields using AND. World solids never match classnames. Adds the exact exclusion once and clears active inclusion; preserves other exclusions and box/distance/follow-camera settings. At most 32 exclusion rules; a whole root owner is hidden when any exclusion matches. Editor-only: source, selection, Undo and compilation remain unchanged. Invalid/stale requests reject atomically."
    )]
    pub(in crate::mcp) fn exclude_visibility_filter(
        &self,
        Parameters(args): Parameters<VisibilityFilterExcludeArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .exclude_visibility_filter(Actor::Agent, args.filter, &args.snapshot)
        })
    }

    #[tool(
        description = "Remove one editor-only exclusion by its zero-based index using the latest get_visibility_categories snapshot. Preserves inclusion, other exclusions and box/distance/follow-camera settings. Overlapping rules or other display filters may still hide matching owners. Stale snapshots and out-of-range indices reject without changes. Use set_working_region action=clear to reset the box and all options when recovering an empty view."
    )]
    pub(in crate::mcp) fn remove_visibility_exclusion(
        &self,
        Parameters(args): Parameters<VisibilityExclusionRemoveArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .remove_visibility_exclusion(Actor::Agent, args.index, &args.snapshot)
        })
    }

    #[tool(
        description = "Inspect map-local named visibility profiles, their snapshot, entries {name,rules} and limits. Each strict rules object stores an optional inclusion filter and up to 32 exclusion filters with shared ObjectFilter semantics. Exact class sets, including saved FGD categories, retain literal membership across definition changes; applying a profile does not reinterpret group names. At most 64 profiles; names at most 64 characters/256 UTF-8 bytes with Unicode-lowercase uniqueness and lookup; aggregate encoded metadata at most 256 KiB. Malformed or unsupported metadata stays preserved and reports unavailable. Profiles are separate from native layer/Hide/Show compilation state.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_visibility_profiles(
        &self,
        Parameters(_args): Parameters<VisibilityProfilesArgs>,
    ) -> ToolResult {
        run(|| self.session().visibility_profiles())
    }

    #[tool(
        description = "Change map-local visibility profiles using the latest get_visibility_profiles snapshot. action=save requires name and optional replace (default false), captures current inclusion/exclusions and requires replace=true to overwrite an existing profile. action=rename requires name and new_name; delete/apply require name only. Save/Rename/Delete are Undoable map metadata edits, persisted by Save and restored on reopen. Apply changes only session inclusion/exclusions, preserving box/distance/follow-camera settings and source/history. Unknown/incompatible fields, malformed metadata and stale/invalid requests reject without changes."
    )]
    pub(in crate::mcp) fn set_visibility_profile(
        &self,
        Parameters(args): Parameters<VisibilityProfileSetArgs>,
    ) -> ToolResult {
        run(|| {
            use crate::session::visibility_profiles::Action;
            let (action, snapshot) = match args {
                VisibilityProfileSetArgs::Save {
                    snapshot,
                    name,
                    replace,
                } => (Action::Save { name, replace }, snapshot),
                VisibilityProfileSetArgs::Rename {
                    snapshot,
                    name,
                    new_name,
                } => (Action::Rename { name, new_name }, snapshot),
                VisibilityProfileSetArgs::Delete { snapshot, name } => {
                    (Action::Delete { name }, snapshot)
                }
                VisibilityProfileSetArgs::Apply { snapshot, name } => {
                    (Action::Apply { name }, snapshot)
                }
            };
            self.session()
                .change_visibility_profile(Actor::Agent, action, &snapshot)
        })
    }
}
