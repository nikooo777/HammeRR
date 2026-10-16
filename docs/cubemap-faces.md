[← Manual](README.md)

# Cubemap faces

Choose which `env_cubemap` sample a reflective face uses. The inspector shows
the same manual and automatic assignments used by the compiler, including
the chosen sample position, original material and generated material alias.

1. Select one visible `env_cubemap` entity and choose **Cubemap faces…**.
2. Keep the window open and select faces in the map. **Select face** in the
   assignment list toggles a face in the shared selection. **Select bound
   faces** selects this helper's complete manual binding list.
3. Choose **Bind selected faces** to preview assigning those faces to this
   helper. Existing manual ownership by another helper must be removed first.
4. Review the draft's helper, face count and exact sample choices, then choose
   **Apply**. The change updates the standard `sides` property in one Undo step.
   **Cancel** or Escape discards only this window's owned draft.
5. Use **Remove selected bindings** to remove selected faces from this helper.
   **Clear all manual bindings** previews removal of its complete inspected
   list. Removed faces use automatic selection, which may choose another
   active helper. Review the new choices before **Apply**.
6. Choose **Refresh** to update assignments after map, material or output-name
   changes. Enter a material or face ID in **Filter material or face ID**, then
   choose **Refresh**. **Previous** and **Next** page through this helper's
   assignments.

![Reviewed manual binding removal and automatic sample choices](images/cubemap-faces.png)

The draft label identifies its current owner. **Review current draft** takes
ownership and issues a new token; the previous owner cannot apply or cancel
it. Changes to the map, face selection, mounted content or output namespace
make a draft stale. Preview the intended operation again before applying it.

The same operations are available through `get_cubemap_bindings` and
`set_cubemap_bindings`. Inspection and previews do not capture reflections or
require reflection generation to be enabled. Build and capture settings are
described in [Generated reflections](reflections.md).

## Limits

- Automatic assignment uses source polygons, world-space brush-model pivots
  and the compiler's front-side preference. It does not test visibility or
  occlusion between a face and its sample.
- Manual targets must be unique, visible authored faces that survive the
  committed compile region. Missing faces, ambiguous IDs, malformed lists,
  duplicate owners and non-emitting targets reject with a diagnostic.
- Another authored helper's manual ownership also blocks reassignment when
  that helper is hidden or excluded by the compile region. Unhide it or
  restore its region, remove the old binding explicitly, then bind the face
  to its new helper. The compiler's active sample choices are unchanged.
- One helper can have up to 1,024 manual faces in this workflow; one preview
  accepts up to 1,024 explicit faces. MCP inspection pages contain up to 256
  assignments. The window shows 20 assignments per page.
- Assignment inspection uses the current supported reflection planner and
  material assets. It may report capture-planner limits, invalid helper
  positions or unsupported materials before a binding can be committed.
- Unnamed maps show provisional `untitled` aliases. Saving or changing the
  output basename recomputes aliases; **Refresh** shows the current names.
- Routine window updates use cached inspection rows. **Refresh**, paging,
  preview and Apply explicitly invoke the authoritative planner.
- Raw entity property editing remains available. Dedicated binding previews
  validate the resulting compiler plan before Apply.
