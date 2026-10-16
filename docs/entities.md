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

## Inspect FGD map size

An FGD's `@mapsize(min, max)` records its authored map-size preference. The loaded catalog retains the authored integer pair, the sorted minimum and maximum, and the declaration's source file and line.

1. Click **Entity…**, then **Load FGD…** and choose the root FGD.
2. Read **Loaded FGD catalog** below the search field. **FGD map size** shows the bounds, authored values and source location. **No effective @mapsize declaration** means no non-equal pair was loaded.
3. Reload the root FGD after changing the declaration or an included file.

![Loaded catalog with authored map-size bounds and source location](images/entity-catalog-mapsize.png)

Declarations follow include order. The last non-equal pair wins; equal endpoints leave the previous preference intact. Reversed endpoints are sorted for the displayed bounds. Repeated includes retain the catalog's existing rule: each source file is loaded once. Invalid syntax or values outside signed 32-bit integers reject the load and retain the previous catalog.

This metadata is informational. It does not change editor placement limits, compiler limits, engine limits or existing map coordinates. The agent can read the same report with `get_entity_definitions`; loading and searching definitions also return the metadata.

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

FGD `size()` declarations draw point-entity boxes with the declared dimensions and origin offset. Both centered `size(width height depth)` and `size(mins, maxs)` forms are supported after includes, inheritance and extensions resolve. A class `color()` sets the generic box color. Loaded model meshes and specialized lights keep their existing geometry.

1. Click **Entity…**, then **Load FGD…** to load the definitions.
2. Place or select a point entity. The inspector shows **FGD size**, dimensions and the world-aligned editor-box policy.
3. Pick the box in any viewport, frame the selection, or use area/volume selection and **Working region…**. These operations use the same displayed bounds, including asymmetric origin offsets.
4. Reload the FGD after changing `size()` or `color()`. Existing values and Undo history stay unchanged.

![Inherited FGD size and class color on a generic point entity](images/class-color-size-box.png)

Malformed or ambiguous definitions show **Invalid FGD size; generic editor marker** and the error in the inspector; the unselected marker is amber. Working-region inspection reports `invalid_fgd_size`, an explicit bounds error and unresolved bounds, retaining the marker for correction. Agents read the same structured report through `get_entity_inspector.helpers`.

Limits: These are world-aligned editor boxes in Source units, independent of `angles` and `modelscale`; they do not describe engine collision hulls. Arguments must be native integers; boxes need positive volume and local coordinates within ±65536. Class colors take precedence over the generic box's stored editor color; selection keeps its usual red highlight and **Selection outlines** behavior. Existing authored-model, light and rope previews remain independent. The report lists the loaded catalog sources; individual inherited helpers do not retain file/line provenance.

### Inspect a property radius

FGD `sphere(property)` declarations add three world-axis guide rings around a
selected generic point entity, including brushless `@FilterClass` definitions.
Each ring uses the named property's authored numeric value, or its FGD default
when no value is stored. Multiple property radii remain separate guides. During
a transform preview, the rings follow the displayed origin.

Zero-argument `sphere()` binds the class's declared numeric `radius` property.
For `ambient_generic`, edit **Max Audible Distance**; for `env_shake`, edit
**Effect Radius**; for `env_soundscape`, edit **Radius**. The helper summary
identifies this implicit binding, and the shared report marks `implicit_property`.
Equivalent `sphere()` and `sphere(radius)` declarations share one guide, retaining
the first declaration's binding provenance. The property must still be declared
as a supported numeric type; a missing or incompatible declaration is diagnostic.

1. Click **Entity…**, choose `env_firesensor`, then **Place entity**. Its shipped
   FGD declares `sphere(fireradius)` and labels the property **Radius**.
2. Select the entity and enter a positive **Radius**, such as `128`. Read the
   inspector's helper summary and inspect the rings in any projection.
3. Change **Radius** to inspect a different distance. Zero and negative values
   draw no guide. The viewport and agent's `render_view` use the same rings.
4. Select another object, hide the entity, or exclude its ordinary box with
   **Working region…** to suppress the rings. Restore selection and visibility
   to display them again.

The shipped `filter_enemy` definition uses this same point-guide path for
**Radius** (`filter_radius`) and **Outer Radius** (`filter_outer_radius`), both
defaulting to `0`.

![Selected point entity with distinct property-radius guides](images/fgd-spheres.png)

These are editor distance guides. They do not establish an engine influence
range, collision volume or runtime fallback for nonpositive values. They do not
enlarge the entity's box, picking target, area/volume selection, working-region
bounds, placement support or framing. **Selection outlines** can hide object
edges while these guides remain visible.

Only selected, visible generic point entities display these rings; loaded model
meshes, brush entities and specialized light helpers keep their existing
previews. Model/studio classes and entities with a model path suppress these
guides even if the asset fails to load, because fade distances can use screen
pixels. Numeric `float`, `integer` and `string` properties are supported.
Malformed, missing, duplicate or unsupported values retain their source text and
show diagnostics without substitute rings. The shared inspector report separates
intrinsic radius validity from selection/visibility suppression and reports
potential guide bounds separately from object bounds. At most sixteen distinct
sphere declarations per entity and 128 guides per frame are accepted, with 32
segments per ring. Scalar values must be finite and within ±65536; native
floating-point conversion must retain a finite, noncollapsed guide. Diagnostic
previews retain at most 256 bytes of raw value and 128 bytes of property name,
with explicit truncation flags. `iconsprite`, `sprite`, `studio`,
frustum and other helper declarations remain outside this preview scope.

FGD `line(R G B, targetname, property)` draws a selected generic point entity's
origin to the point named by that property. A second `targetname, property` pair
draws between two named points instead. Repeated declarations retain their own
literal FGD colors; `rendercolor` does not recolor these editor links.

1. Place an `info_target`, then set its **Name** to `beam_end`.
2. Place an `env_laser` and set **Target of Laser** to `beam_end`.
3. Select the laser to show its origin-to-target line. Read the helper summary
   for missing, ambiguous or suppressed endpoints. Change **Target of Laser**
   through the property panel or **Advanced properties** to update the same guide.
4. For `env_beam`, set **Start Entity** and **Ending Entity**, then select the beam
   to see the line between those named origins. Transform previews move the
   displayed endpoints without committing an edit.

![Selected point entity with separate origin-to-target and two-target line guides](images/fgd-lines.png)

These links are editor guides; they do not simulate beams, parenting, attachments
or entity I/O. Only unique, exact-case literal target names are displayed.
Case-insensitive name collisions are ambiguous, and case-only matches report an
unsupported state rather than guessing native matching behavior. Wildcards,
`!tokens`, `nodeid` lookup and brush endpoints are unavailable. Reference values
use authored properties before labeled FGD defaults. Targets need one authored
`targetname` and one finite authored origin within ±65536 units. Missing,
duplicate, invalid and coincident endpoints add no replacement geometry.

Generic visible point sources retain the same specialized-helper exclusions as
spheres. Endpoints may be model or light points when their origins are valid.
Both source and endpoints must be visible and included in **Working region…**;
hidden or excluded duplicate names still count toward ambiguity. Lines never
change object bounds, picking, framing, placement or volume selection. The shared
inspector and `get_entity_inspector.helpers.lines` report resolution separately
from selection/visibility suppression. Limits are sixteen declarations per
entity, 128 displayed lines, 65536 lookup entities/name values, and 128/256 bytes
for property/value previews. Oversized inputs are rejected before clipped text
can be resolved.

How lights, spotlights and the sun are baked, including falloff distances and sky ambient, is covered in [Compile and play](compile-and-play.md#lighting).

### Inspect a Source projector guide

Selected `env_projectedtexture` entities show an authored square projector guide
with eight corners and twelve edges. The viewport and agent renders use the
same displayed origin and **Pitch Yaw Roll (Y Z X)**, including transform previews.
Zero angles face +X; positive pitch points downward. This independent guide
does not evaluate FGD `frustum()` or its fifth `pitch_scale` argument. The
inspector states this distinction and agents read the declared argument in
`get_entity_inspector.helpers.source_projector`.

1. Load the game's FGD through **Entity…** → **Load FGD…**, then place and select
   `env_projectedtexture`.
2. Set **FOV**, **NearZ** and **FarZ**. **FOV** applies equally to horizontal and
   vertical projection; **NearZ** and **FarZ** measure along the forward axis.
3. Edit **Pitch Yaw Roll (Y Z X)** to orient the guide. Change **Light Color** to
   recolor its edges. The fourth component remains intensity; it does not change
   guide opacity or exposure.
4. Read **Source projector guide** in the helper summary for unavailable values
   or suppressed visibility. Select another object, hide the source or exclude
   its ordinary box with **Working region…** to suppress the guide.

Authored values precede individually labeled FGD defaults. The shipped FGD uses
FOV 90, NearZ 4 and FarZ 750; these are authoring defaults, and may differ from
an engine constructor's defaults. Definition reloads update absent values
without writing entity properties or Undo history. Color edits use the shared
color picker operation and preserve the fourth component when omitted.

Limits: The guide requires loaded, compatible FGD property definitions and a
visible point source. Nonempty **target** or **Parent**, nonzero **Camera Space**
and legacy `angle` are unsupported branches and draw no replacement guide.
FOV must be strictly between 0 and 180; 0 < NearZ < FarZ ≤ 65536. Corners must
remain finite and distinct at native precision, with each coordinate offset
within ±65536 units of the origin. Values are never clamped; malformed,
duplicate, missing and oversized values have explicit diagnostics. Value
previews cap at 256 bytes; at most 16
unevaluated helper declarations and 128 displayed projectors are reported.
Model helpers, loaded meshes and brush sources keep their existing behavior.
The guide does not change bounds, picking, selection, placement, framing or
working regions, and does not preview projected textures, illumination,
shadows, targets, parent movement or runtime enabled state.

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
