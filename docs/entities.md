[← Manual](README.md)

# Entities and logic

## Place an entity

Click **Entity…**, search the entity classes, and place one. It appears at the camera or on the current planar workplane, following the editor's snapping.

Classes come from the Counter-Strike: Source FGD. **Load FGD…** in the class picker loads other definition files. Custom classes and unknown properties stay editable.

## Edit properties

Select an entity to see its properties in the inspector. Common settings come first, with controls that fit each type: position and rotation fields, color pickers, brightness sliders, number fields, material previews, choice lists and spawn flags.

- **?** next to a property explains it.
- **Advanced properties** shows the raw engine keys and lets you add custom ones.
- Invalid values are refused and the map stays unchanged.
- Direction properties such as Move direction accept pitch, yaw and roll.

Every entity dialog has a Cancel button and closes with Escape.

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
