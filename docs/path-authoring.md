[← Manual](README.md)

# Linked paths

Create or edit ordered `path_corner` and `path_track` chains.

## Create a path

1. Open **Create linked path…**.
2. Choose **Class** and **Name prefix**.
3. Enter **Points** with X, Y and Z coordinates.
4. Use **Add point**, **Remove**, **Move up** and **Move down** to set the order.
5. Enable **Closed loop** if required.
6. Click **Preview**.
7. Inspect names, positions and links, then click **Apply** or **Cancel**.

Do not repeat the first point to close a loop.

## Edit a path

1. Select one existing node.
2. Open **Create linked path… → Load selected path**.
3. Edit positions or reorder rows.
4. Use **New node prefix** for inserted nodes.
5. Preview, review removals, then apply.

Use **Pick position…** to place a row on a visible surface.
Escape or **Cancel position pick** leaves the preceding position unchanged.

## Limits

- 2–64 distinct points within ±65536.
- Prefixes use 1–96 ASCII letters, digits, underscores or dashes, starting with a letter or underscore.
- Existing names and node identities remain unless explicitly removed.
- Ambiguous chains, unsafe references and incompatible source changes reject.
- Authoring a chain does not attach a train or validate runtime movement.
