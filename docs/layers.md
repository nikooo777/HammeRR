[← Manual](README.md)

# Layers

Organize brushes and entities into named, colored native VMF layers. An object
can belong to several layers, and layers can contain child layers.

## Create and edit a layer

1. Select the objects to organize, then open **Layers…** in the inspector.
2. Click **New layer**, enter a **Layer name**, choose **Layer color · RGB**, and
   choose a **Parent layer** or **Root (no parent)**.
3. Review **Direct members**. **Use current selection** replaces that set;
   **Add current selection**, **Remove current selection** and each row's
   **Remove** adjust it. Memberships in other layers remain.
4. Click **Review changes**, inspect any error, then **Apply changes**. Each
   application records one Undo step. Save the VMF to keep the layer.
5. To revise a layer, choose it on the left and click **Edit layer**. **Review pending edit** opens the exact existing human or MCP draft for inspection and continued editing. **Cancel**,
   Escape and **Close** discard only that draft. A map edit makes it stale;
   reopen the draft before applying.

![Nested layers, colors and membership review](images/layers.png)

To assign individual brushes inside a brush entity, use **Selection →
Faces · materials & textures** and select faces on those brushes before adding
or removing the current selection. Membership is assigned to each brush,
regardless of how many of its faces are selected.

## Select, hide and delete

1. Search by name or native ID in **Find a layer by name or ID**. Use
   **Previous** and **Next** to page through layers.
2. Choose a layer. **Include child layers** controls recursive actions.
3. Choose a selection mode and click **Select members**. **Select hidden
   members too** includes hidden objects. Explicit layer selection uses the
   layer's objects without expanding unrelated physical groups.
4. Use **Hide members** or **Show members** to change ordinary map visibility
   in one Undo step. Hidden members are also omitted from builds. A matching
   entity-owned brush selects or hides its whole entity.
5. Choose **Delete layer…**, review the explanation, then **Delete layer**.
   Objects remain, children move to the deleted layer's parent, and only that
   layer's memberships are removed. **Cancel** keeps the layer.

Clipboard transfers carry the required layer ancestry and memberships, including
entity-owned brush links. Pasting allocates fresh layer IDs and keeps destination
layers separate, even when names or source numbers match.

## Limits

- Up to 2,048 layers, 64 nesting levels, 32,768 distinct memberships and 8,192
  direct members in one edit. New names use 1–128 characters without surrounding
  whitespace, controls or literal quotes.
- Layer IDs have an independent namespace. Imported zero and full unsigned IDs
  can be inspected and edited; new IDs increase monotonically through Undo.
- Unknown layer metadata is preserved, with bounds of 8 MiB, 131,072 nodes and
  64 levels. Malformed graphs, missing references, exhausted IDs and direct
  layer links on physical group records prevent authoring; assign member
  objects instead.
- Layer colors identify records in this window. They do not recolor geometry.
  Separate editor-only visibility filters, visibility explanations and automatic
  categories are unavailable. Hide/show uses saved map visibility.
- Layers contain whole brushes or entities, rather than individual faces or
  components. Named partition intents and private Hammer clipboard formats
  remain outside clipboard transfer.
