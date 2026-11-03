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

> [!WARNING]
> Hidden members are left out of compiled builds. Show them before you compile.
[Temporary editor visibility](selection.md#temporary-editor-visibility) is separate.

## Automatic categories and saved rules

Use [Working region](working-region.md) for automatic object-type, exact
entity-class and loaded FGD group categories, custom exclusions, and map-local
rule profiles.

1. Open **View → Working region…**.
2. Choose **Categories and exclusions**, find a category, then click
   **Exclude category**. This affects editor display and picking immediately.
3. Choose **Saved rule profiles** to save the applied inclusion filter and
   exclusions with **Save applied as profile**.
4. Use **Clear working region** to restore otherwise-visible objects.

Categories follow authored map content; they do not change named-layer
membership. Saved profiles store rules, not copies of member objects.
Profile metadata is saved in the map; applying it does not change compilation
or saved hidden state.

FGD categories show **FGD · Parent / Name**, their exact-class count, current
whole-owner count and declaration source. One group adds one exclusion, even
when it contains many classes. Empty or unavailable declarations have disabled
category buttons; **Remove exclusion** remains available for existing rules.
FGD groups do not create or edit named layers, and they do not expand class
inheritance or wildcard patterns.

## Limits

- 2048 layers, 64 nesting levels and 32768 distinct memberships.
- Colors identify layer records; they do not recolor geometry.
- Layers contain whole objects, not individual faces or components.
- Imported unsupported layer/group links block authoring and remain preserved.

<!-- page-nav -->

---

[← Groups](groups.md) · [Manual](README.md) · [Map checks, notes and recovery →](map-tools.md)
