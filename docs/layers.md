[← Manual](README.md)

# Layers

Organize whole brushes and entities into named layers.

## Create or edit a layer

1. Select the objects.
2. Open **Layers…**.
3. Click **New layer**.
4. Enter **Layer name**.
5. Choose **Layer color · RGB** and the parent layer.
6. Review **Direct members**.
7. Click **Review changes**.
8. Click **Apply changes**.

**Use current selection** replaces the membership.
Choose an existing layer, then **Edit layer** to revise it.
Cancel discards pending changes.

## Select, hide or delete

1. Find the layer in **Find a layer by name or ID**.
2. Set **Include child layers** as required.
3. Use **Select members**, **Hide members** or **Show members**.

**Delete layer…** removes the layer after review. It keeps the member objects.
Layer visibility is saved in the map.
[Temporary editor visibility](selection.md#temporary-editor-visibility) is separate.

## Limits

- 2048 layers, 64 nesting levels and 32768 distinct memberships.
- Colors identify layer records; they do not recolor geometry.
- Layers contain whole objects, not individual faces or components.
- Imported unsupported layer/group links block authoring and remain preserved.
