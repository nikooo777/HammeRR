[← Manual](README.md)

# Linked paths

Create an ordered chain of `path_corner` or `path_track` entities, or reorder
and reposition an existing chain while retaining its node identities. Review
the complete chain before applying changes to the map.

## Create a path

1. In the main side panel, click **Create linked path…**, below **Draw polygon…**.
2. Choose **Class**: **path_corner** or **path_track**. Enter a **Name prefix**;
   names follow `prefix_001`, `prefix_002` and so on.
3. Enter each point's world coordinates under **Points**, using **X**, **Y** and
   **Z**. **Add point** appends a node; **Remove** deletes a row. Use **Move up**
   and **Move down** to set the link order.
4. Enable **Closed loop** to link the last node to the first. Leave it off for
   an open chain whose last node has no next target.
5. Click **Preview** and inspect the proposed names, positions and links.
   Changing a field requires another **Preview** before **Apply**. If another
   actor owns the current draft, use **Review preview** to take ownership.
6. Click **Apply** to create and select the complete chain in one Undo step,
   or **Cancel** to discard the draft. **Undo** removes that creation; **Redo**
   restores the same node IDs and links.

![Linked path creation and ordered node review](images/linked-path.png)

Limits: 2–64 distinct points, with finite coordinates within ±65536 per axis.
Prefixes use 1–96 ASCII letters, digits, underscores or dashes and must start
with a letter or underscore. Occupied names, including hidden entities and
case variants, receive a numeric suffix. A name that would acquire an existing
reference is rejected; choose another prefix. Do not repeat the first point
to close the chain; use **Closed loop**.

Loaded FGD defaults apply, while generated names and next links determine the
chain. Reloading definitions, editing the map, changing selection or replacing
the geometry preview invalidates the draft. A replaced token cannot apply or
cancel another draft. Creation does not attach a train or other moving entity,
or validate path behavior in the game.

## Edit an existing path

Existing-path editing keeps every node's ID, name, class, flags, custom values,
I/O connections and references from other entities. It changes only the edited
positions and next-node links. Untouched raw values remain as authored; new FGD
defaults do not replace existing values.

1. Select one existing `path_corner` or `path_track` node. Any node in the chain
   works; an open chain loads from its first node, while a closed loop starts
   at the selected node.
2. Open **Create linked path…** and click **Load selected path**. The point
   table shows the complete chain with each node's name and ID. **Class**,
   **Name prefix** and **Closed loop** are locked to retain its identity and
   open or closed topology.
3. Use **Move up** and **Move down** to change node order. Edit **X**, **Y** and
   **Z** to move that named node. **Add point** and **Remove** are unavailable.
4. Click **Preview**. Inspect the names, positions and next links in the table
   and the viewport. Changed fields require another **Preview**. Use
   **Review preview** to load and take ownership of a shared agent draft.
5. Click **Apply** to update and select the existing nodes in one Undo step,
   or **Cancel** to leave the map unchanged. **Undo** and **Redo** restore the
   same nodes and their links. Applying an unchanged path adds no history step.
6. Use **New path** to return the table to creation mode.

![Reordering existing named nodes without replacing their identities](images/linked-path-existing.png)

Limits: editing requires one complete, visible, unparented simple chain or
closed loop of 2–64 distinct points, all of the same point class. Every name
must be unique across the map, including hidden entities and case variants.
Missing targets, links to another class, duplicate IDs or structural keys,
wildcard or special path targets, alternate branches and several path nodes
targeting one node are rejected. Existing external entity references and I/O
remain unchanged; they do not become new links in the chain.

Changing the map or entity definitions after loading a chain requires
**Load selected path** again. Invalid requests retain the last valid preview.
Insertion, removal, renaming, open/closed conversion, train setup and runtime
movement validation remain outside this workflow.
