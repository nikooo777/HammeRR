[← Manual](README.md)

# Object groups

Keep world brushes and whole entities together in native VMF groups. Groups can
contain other groups, so an assembly retains its smaller sets of objects.

## Create and edit groups

1. Select objects, then open **Object groups…** in the inspector.
2. Choose **New from selection** and enter a **Group name**. Completely selected
   existing groups become nested members.
3. Review **Direct members**. **Add current selection** adds the current objects;
   choose a group on the left and use **Add chosen group** to nest it.
4. To change nesting, choose a group on the left and click **Use chosen parent**,
   or choose **Make root**. **Remove** returns a member to the edited group's
   previous parent when applied.
5. Choose **Apply group** to record one Undo step. **Cancel**, Escape or **Close**
   discards the draft. Save the VMF to keep applied changes.

Choose an existing group and **Edit group** to change its name, parent or direct
members. Invalid cycles and missing members appear in the draft and disable
**Apply group**. A map edit or map replacement makes an older draft unusable.

![Object group hierarchy, membership and ungroup review](images/object-groups.png)

## Select and ungroup

1. Ordinary viewport picks and MCP object selection select the outermost group.
   Ctrl+click toggles that group as one unit. Area, volume and paint selection
   use the same groups; **Fully inside** requires the complete visible group.
2. Choose a group in **Object groups…**, choose **Replace selection**,
   **Add to selection**, **Remove from selection** or **Toggle selection**, then
   choose **Select group**. This selects an inner group without its ancestors.
3. Use a member's **Select** button to select that member explicitly. Enable
   **Bypass groups** to pick individual objects in the viewport. This setting
   changes selection behavior without changing the saved groups.
4. Hidden peers are excluded by default. **Include hidden members** allows
   explicit group or member selection to include them.
5. To dissolve a group, choose **Ungroup…**, review its direct members, then
   choose **Ungroup**. Members move to its parent; nested groups remain intact.
   Undo restores the group and all memberships together.

## Limits

- Grouping edits world brushes and whole entities. Imported membership links on
  brushes inside an entity are preserved and reported; those child links are
  not editable here. Ungroup is blocked while those brushes reference the group.
- Malformed imported group graphs are preserved and reported. Repair is outside
  this workflow; ordinary object selection remains available.
- Groups support 8,192 records, 8,192 direct members per edit, 64 nested levels
  and names of up to 128 characters. Group and member lists are paginated.
- Group bypass is a session setting. Changing it cancels pending area, volume
  and paint selection previews. Faces retain their individual selection rules.
- Explicit object IDs passed to transform or placement operations retain their
  exact scope. Select a group first to operate on its complete selection.
