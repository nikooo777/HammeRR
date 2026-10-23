[← Manual](README.md)

# Object groups

Select and edit an assembly as one group.

## Create a group

1. Select the objects.
2. Open **Object groups…**.
3. Click **New from selection**.
4. Enter **Group name**.
5. Review **Direct members**.
6. Click **Apply group**.

**Add current selection** adds members.
**Use chosen parent** changes nesting.
Cancel discards the pending changes.

## Select or dissolve a group

1. Choose a group in **Object groups…**.
2. Choose the selection combine mode.
3. Select its members.

Ordinary picking selects the outermost visible group.
Use a member's **Select** button for an explicit member selection.
Enable group bypass to pick individual objects temporarily.
**Include hidden members** includes hidden peers.

To dissolve a group, use **Ungroup…** and review its members.
The member geometry remains in the map.

## Limits

- 8192 group records, 8192 direct members per edit, and 64 nesting levels.
- Group names have at most 128 characters.
- Imported invalid group graphs remain preserved and reported.
- Editing entity-owned child-brush memberships is unavailable.
- Group bypass is session-local. It does not affect individual face selection.
- Explicit transform/placement IDs keep their exact scope; select the complete group first when required.
