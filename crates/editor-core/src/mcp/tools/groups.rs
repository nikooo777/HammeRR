use super::super::*;

#[tool_router(router = groups_tools, vis = "pub(in crate::mcp)")]
impl EditorServer {
    #[tool(
        description = "Inspect native VMF object groups, nested parents, direct membership and shared edit draft. Default page 100, maximum 200. group adds a paginated direct-member detail; counts include hidden members. Malformed imported group graphs remain preserved and reject grouping authoring.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_object_groups(
        &self,
        Parameters(args): Parameters<ObjectGroupsArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().object_groups(
                args.group,
                args.offset.unwrap_or(0),
                args.limit.unwrap_or(100),
            )
        })
    }

    #[tool(
        description = "Begin a shared cancellable native object-group draft. Omit group to create from the selection; completely selected existing groups become nested members. group edits its direct membership/name/parent; ungroup=true previews deletion of that group and promotion of direct members to its parent. Source and Undo history stay unchanged. UI: Object groups… → New from selection / Edit group / Ungroup…."
    )]
    pub(in crate::mcp) fn begin_object_group_edit(
        &self,
        Parameters(args): Parameters<BeginObjectGroupArgs>,
    ) -> ToolResult {
        run(|| self.session().begin_group_edit(args.group, args.ungroup))
    }

    #[tool(
        description = "Update the captured object-group draft with a name, parent or full direct-member list. clear_parent=true moves the group to the root. Removed members return to the group's previous parent; added members leave their previous direct group. Nested groups stay intact. Cycles/missing IDs are inspectable invalid drafts and cannot apply. UI: Group name / Parent group / Remove / Add group."
    )]
    pub(in crate::mcp) fn update_object_group_edit(
        &self,
        Parameters(args): Parameters<UpdateObjectGroupArgs>,
    ) -> ToolResult {
        run(|| {
            if args.clear_parent && args.parent.is_some() {
                return Err("Choose parent or clear_parent, not both".into());
            }
            let parent = if args.clear_parent {
                Some(None)
            } else {
                args.parent.map(Some)
            };
            let members = args
                .members
                .as_ref()
                .map(|members| {
                    members
                        .iter()
                        .map(|member| crate::session::groups::parse_member(member))
                        .collect::<Result<Vec<_>, _>>()
                })
                .transpose()?;
            self.session()
                .update_group_edit(args.token, args.name, parent, members)
        })
    }

    #[tool(
        description = "Add the current object selection to the captured shared group draft. Completely selected existing groups become nested members; group bypass permits selecting individual members. Does not edit source/history. UI: Add current selection."
    )]
    pub(in crate::mcp) fn add_object_group_selection(
        &self,
        Parameters(args): Parameters<ObjectGroupTokenArgs>,
    ) -> ToolResult {
        run(|| self.session().add_selection_to_group_edit(args.token))
    }

    #[tool(
        description = "Apply the captured valid object-group draft as one undoable edit, preserving unknown native metadata and nested membership. Rejects replaced tokens, changed map revisions and map replacement. UI: Apply group / Ungroup."
    )]
    pub(in crate::mcp) fn apply_object_group_edit(
        &self,
        Parameters(args): Parameters<ObjectGroupTokenArgs>,
    ) -> ToolResult {
        run(|| self.session().apply_group_edit(Actor::Agent, args.token))
    }

    #[tool(
        description = "Cancel only the captured object-group draft, including a stale draft. Foreign or replaced tokens reject cancellation; source and Undo history stay unchanged. UI: Cancel / Escape / close."
    )]
    pub(in crate::mcp) fn cancel_object_group_edit(
        &self,
        Parameters(args): Parameters<ObjectGroupTokenArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().cancel_group_edit(args.token)?;
            Ok(json!({"cancelled":true}))
        })
    }

    #[tool(
        description = "Explicitly select the descendant objects of a native object group, including an inner group without selecting its ancestors. mode=replace/add/remove/toggle; include_hidden defaults false. Selection changes no source/history. UI: Object groups… → Select group."
    )]
    pub(in crate::mcp) fn select_object_group(
        &self,
        Parameters(args): Parameters<SelectObjectGroupArgs>,
    ) -> ToolResult {
        run(|| {
            let ids = self.session().select_object_group(
                args.group,
                parse_select_mode(args.mode.as_deref().unwrap_or("replace"))?,
                args.include_hidden,
            )?;
            Ok(json!({"members":ids.into_iter().map(id_string).collect::<Vec<_>>()}))
        })
    }

    #[tool(
        description = "Select one direct object or nested group explicitly, bypassing ancestor expansion. member is solid:<id>, entity:<id> or group:<id>. mode=replace/add/remove/toggle; include_hidden defaults false. UI: member row → Select."
    )]
    pub(in crate::mcp) fn select_object_group_member(
        &self,
        Parameters(args): Parameters<SelectObjectGroupMemberArgs>,
    ) -> ToolResult {
        run(|| {
            let ids = self.session().select_group_member(
                crate::session::groups::parse_member(&args.member)?,
                parse_select_mode(args.mode.as_deref().unwrap_or("replace"))?,
                args.include_hidden,
            )?;
            Ok(json!({"members":ids.into_iter().map(id_string).collect::<Vec<_>>()}))
        })
    }

    #[tool(
        description = "Compatibility setter for object selection rules: true chooses Objects (whole world brushes/entities without groups), false chooses Groups (outermost native groups). To select individual entity brushes choose Solids through set_object_selection_mode. Shared Session setting; current selection and source/history are preserved, pending object gestures retire. Explicit group/member selection works in every mode. UI: Object groups… → Bypass groups."
    )]
    pub(in crate::mcp) fn set_group_selection_bypass(
        &self,
        Parameters(args): Parameters<GroupSelectionBypassArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .set_group_selection_bypass(Actor::Agent, args.bypass)?;
            Ok(json!({"bypass":args.bypass}))
        })
    }

    #[tool(
        description = "Inspect native map layers (VMF visgroups), hierarchy, colors, direct memberships, whole-object selection counts and the current versioned draft. Query/offset/limit page records (default64/max128); selected and member_offset inspect direct members, including owned-brush IDs and owners. parent_choices includes the bounded full parent set. Read-only, no map changes. Layer IDs have an independent namespace. UI: Layers…. Automatic categories and separate editor-only filters are unavailable.",
        annotations(read_only_hint = true)
    )]
    pub(in crate::mcp) fn get_layers(
        &self,
        Parameters(args): Parameters<LayersArgs>,
    ) -> ToolResult {
        run(|| {
            Ok(self.session().layer_report(
                args.query.as_deref().unwrap_or_default(),
                args.offset.unwrap_or(0),
                args.limit.unwrap_or(64),
                args.selected,
                args.member_offset.unwrap_or(0),
            ))
        })
    }

    #[tool(
        description = "Begin a versioned owned layer edit from get_layers.snapshot. Omit id to create with current selection as members; id edits existing native name/color/parent/direct members. delete:true stages deletion, preserving objects and promoting child layers. No map changes until Apply. UI: Layers… → New layer / Edit layer / Delete layer…."
    )]
    pub(in crate::mcp) fn begin_layer_edit(
        &self,
        Parameters(args): Parameters<BeginLayerArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .begin_layer_edit(args.id, args.delete.unwrap_or(false), &args.snapshot)
        })
    }

    #[tool(
        description = "Replace the complete layer draft fields using its latest token and version: name, RGB color, optional parent (omit/null for root), and full direct member IDs. Whole objects and entity-owned solid IDs are supported; Faces-mode UI selection targets owning brushes. Memberships in other layers remain. Returns incremented version and valid/error. Invalid fields do not mutate the map. UI: Layers… → Review changes."
    )]
    pub(in crate::mcp) fn update_layer_edit(
        &self,
        Parameters(args): Parameters<UpdateLayerArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().update_layer_edit(
                args.token,
                args.version,
                args.name,
                args.color,
                args.parent,
                parse_ids(&args.members)?,
            )
        })
    }

    #[tool(
        description = "Apply the latest owned layer token/version as one Undo step, checking map generation/revision. Records and memberships change atomically, with independent monotonic native IDs and preserved unknown metadata. Deletion removes only that layer's memberships and promotes children. Stale/replaced drafts reject. UI: Layers… → Apply changes / Delete layer."
    )]
    pub(in crate::mcp) fn apply_layer_edit(
        &self,
        Parameters(args): Parameters<LayerTokenArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .apply_layer_edit(Actor::Agent, args.token, args.version)
        })
    }

    #[tool(
        description = "Cancel the exact owned layer token/version. A stale window cannot discard a replacement or newer version. No map/history changes. UI: Layers… → Cancel, Escape or close."
    )]
    pub(in crate::mcp) fn cancel_layer_edit(
        &self,
        Parameters(args): Parameters<LayerTokenArgs>,
    ) -> ToolResult {
        run(|| self.session().cancel_layer_edit(args.token, args.version))
    }

    #[tool(
        description = "Select exact layer members through shared Session selection, with get_layers.snapshot. recursive defaults true, include_hidden false, mode replace/add/remove/toggle. Owned-brush hits select their whole entity; this explicit scope does not expand physical groups. UI: Layers… → Select members."
    )]
    pub(in crate::mcp) fn select_layer_members(
        &self,
        Parameters(args): Parameters<LayerSelectionArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().select_layer_members(
                args.id,
                args.recursive.unwrap_or(true),
                args.include_hidden.unwrap_or(false),
                parse_select_mode(args.mode.as_deref().unwrap_or("replace"))?,
                &args.snapshot,
            )
        })
    }

    #[tool(
        description = "Hide or show layer members through the same undoable object-visibility operation as other tools. Uses get_layers.snapshot; action hide/show and recursive (defaulttrue). Hiding also omits objects from compilation. Owned-brush matches act on their whole entity. This is not an independent editor-only layer filter. UI: Layers… → Hide members / Show members."
    )]
    pub(in crate::mcp) fn set_layer_visibility(
        &self,
        Parameters(args): Parameters<LayerVisibilityArgs>,
    ) -> ToolResult {
        run(|| {
            self.session().set_layer_visibility(
                Actor::Agent,
                args.id,
                args.recursive.unwrap_or(true),
                &args.action,
                &args.snapshot,
            )
        })
    }
}
