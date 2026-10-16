[← Manual](README.md)

# Linked paths

Create an ordered chain of `path_corner` or `path_track` entities with named
links and inherited entity defaults. Review the complete chain before adding it
to the map.

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
reorder an existing path, or validate path behavior in the game.
