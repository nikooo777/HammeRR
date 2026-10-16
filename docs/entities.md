[← Manual](README.md)

# Entities and logic

## Place an entity

Click **Entity…**, search the entity classes, and place one. It appears at the camera or on the current planar workplane, following the editor's snapping.

Classes come from the Counter-Strike: Source FGD. **Load FGD…** in the class picker loads other definition files. Custom classes and unknown properties stay editable.

## Load extended entity definitions

FGD extensions can add or replace property controls, defaults, choices, spawn flags and I/O signatures for existing classes. The inspector, connection suggestions and agent class catalog use the same loaded definitions.

1. Create a root FGD that includes the game's FGD and any extension files.
2. Click **Entity…**, then **Load FGD…** and choose the root FGD.
3. Search for the class and select it to inspect its updated properties. Existing entities show the updated controls when selected.

`@ExtendClass` declarations apply after every included file has loaded, in declaration order. They retain the target's class type, description and inheritance. Unmentioned properties, choices, flags and I/O stay available; a same-name property or I/O declaration replaces the previous definition. Replacing a choices or flags property replaces its entire choice list. Extensions to base classes also reach their descendants. Distinct helpers with the same name remain separate; exact duplicates are removed.

Reloading definitions preserves existing entity values, custom keys, connections and undo history. It does not rewrite stored values to new defaults. A failed load keeps the previous catalog. Reload the root FGD to pick up later file changes.

Limits: FGD inheritance and extension targets match declaration names exactly, including case (`Light` and `light` can be separate classes). An extension whose target is absent is ignored and never creates a class. Extension descriptions are ignored. `base(...)` on `@ExtendClass` is unsupported and reports an error; extend an existing base class or define inheritance on the target class instead. Include and inheritance cycles or excessive nesting reject the load. Other extended directives and helper rendering are not implied by loading their metadata.

## Place on a picked surface

Preview a new point entity against a brush face, terrain sheet or supported model mesh before adding it to the map.

1. Click **Entity…**, then search for and choose a point class. Click **Pick surface…**.
2. Enter **Offset** in map units. Positive values leave a gap; negative values embed the entity. Contact uses the model mesh when available, otherwise its editor helper bounds.
3. Enable **Align local up to surface** to rotate the entity's existing up direction to the geometric surface normal. Source angles, legacy angle values and light pitch use the same orientation handling as ordinary transforms.
4. Enable **Snap along surface · grid …** to snap the contact anchor along the surface using the current grid. The two world coordinates outside the normal's dominant axis are rounded; the remaining coordinate stays on the plane. A second surface check rejects a snapped anchor outside the picked face.
5. Click **Pick surface**, then click a visible surface in the viewport. A miss keeps picking active. **Escape** or **Cancel picking** discards an owned pick.
6. Review **Origin**, **Bounds minimum**, **Bounds maximum**, the surface normal and contact offset. Cyan lines show the new entity's bounds; the yellow line marks the contact normal. Changed settings require another **Pick surface**.
7. Click **Apply entity** to create and select the entity in one Undo step. **Cancel**, **Escape** or closing the window discards the owned draft without changing the map or history.

![Entity surface placement with reviewed bounds and contact offset](images/entity-placement-controls.png)

A shared agent draft opens with **Review shared placement**. Review transfers ownership and replaces its token before human picking, Apply or cancellation. A stale shared draft can be reviewed for cancellation or another surface pick; Apply still requires a current preview. The UI and MCP use the same Session operations.

Limits:

- One entity is placed at a time. This uses contact against one hit plane; it does not settle the entity, check gameplay clearance or resolve intersections elsewhere.
- Targets follow brush, terrain, model, tool and working-region visibility. Point helper boxes are not targets. Unsupported or unavailable model sources use editor helper bounds, with a visible diagnostic when asset loading fails.
- Snapping affects the contact anchor; the final origin can be off-grid because of normal offset and support bounds. Normals come from actual geometry rather than smoothed lighting normals.
- `info_overlay` uses **Overlay…**. Contextual `infodecal` creation and brush entities are outside this workflow. Explicit-position creation remains available through **Place entity**.
- Offsets and the complete displayed bounds must remain within ±65536 units. A competing edit, map replacement, grid, selection, content or visibility change requires another preview. Old tokens cannot Apply or discard replacement geometry.
- Finish other geometry previews first. Apply or cancel entity placement before saving, compiling or playing.

## Edit properties

Select an entity to see its properties in the inspector. Common settings come first, with controls that fit each type: position and rotation fields, color pickers, brightness sliders, number fields, material previews, choice lists and spawn flags.

- **?** next to a property explains it.
- **Advanced properties** shows the raw engine keys and lets you add custom ones.
- Invalid values are refused and the map stays unchanged.
- Direction properties such as Move direction accept pitch, yaw and roll.

Every entity dialog has a Cancel button and closes with Escape.

Model entities can display mounted static meshes directly. See [Models](models.md) for **World Model**, **Skin** and **Model details…**.

## Lights and helpers

Point lights are drawn as a light bulb. Spotlights and sunlight show direction helpers, and a selected spotlight shows its cone. These helpers are editor-only: they appear in the viewport and in agent renders, but add nothing to the compiled map.

How lights, spotlights and the sun are baked, including falloff distances and sky ambient, is covered in [Compile and play](compile-and-play.md#lighting).

The bulb is a [CC0 model by Josh Dean / Poly Haven](https://polyhaven.com/a/lightbulb_led); see its [provenance](../crates/render/assets/lightbulb/SOURCE.txt).

## Brush entities

Select world brushes and click **Tie to entity…** to turn them into a brush entity, such as a door. **Move to world** turns a brush entity back into world brushes.

To set a brush entity's pivot (for doors and rotating brushes), see [brush entity pivots](compile-and-play.md#brush-entities-and-pivots).

## Connections (inputs and outputs)

Select an entity and open **Connections…**.

- **Outgoing** lists the entity's outputs.
- **Incoming** lists connections elsewhere in the map that target this entity. You can edit them from here.

To add or change a connection:

1. Click **Add connection** or **Edit connection**.
2. Choose the output, the target and the input. Suggestions come from the map's named entities and the FGD; custom names also work.
3. Set the parameter, the delay in seconds, and how often it fires: **Unlimited**, **Once** or **Specific count**.
4. Click **Apply connection**.

Deleting a connection is undoable.

- Missing targets, undeclared inputs/outputs and parameter type mismatches show warnings.
- Invalid characters, negative delays and invalid counts block Apply.
- Incoming connections are found by exact name, prefix wildcards (`door*`) and `!self`. Other special targets are only resolved in game.
- New connections use CS:S's comma-separated format, so fields can't contain commas, quotes or control characters. Imported connections keep their original format until you edit them.

![Entity connection form with suggestions and Cancel](images/entity-connections.png)

## Rename safely

Renaming an entity can break the connections and properties that refer to it by name. This tool finds those references and updates them with the name.

1. Select the entity and open **References and rename…** (or edit its **Name**).
2. Review what refers to it: incoming connections, entity-reference properties, parent links, and inputs such as `SetParent` whose parameter is a name.
3. Type the new name. **What will change** lists every update.
4. Click **Apply rename**. The name and all exact references change in one undo step.

- When several entities share a name, **Rename all … entities** renames the group. The preview lists every member, including hidden ones.
- A name that's already in use blocks Apply.
- Wildcards, special targets, ambiguous references and free text are never changed automatically. References whose meaning might change appear under **Needs your attention**. Review them, then confirm **I’ve reviewed these effects; leave these fields unchanged**.
- **Show engine details** shows the raw keys, class names and IDs.
- Scripts and connections created at runtime aren't analyzed. Check those by hand.

![Rename preview with exact updates and references to review](images/entity-rename.png)

## World settings and editing many entities

**World settings…** edits map-wide values such as the skybox.

To edit several entities at once, select them and open **Edit selected entity properties…**:

- Properties they share are listed, and mixed values and flags are marked.
- Changing one flag or one color channel keeps each entity's other values.
- **Use default** writes the FGD default value. **Remove override** deletes the stored key. The inspector shows which values are stored and which are defaults.
- Click **Apply properties** to apply everything in one undo step.

Names are changed through **References and rename…**.

![Property batch with mixed values, defaults and Cancel](images/batch-properties.png)
