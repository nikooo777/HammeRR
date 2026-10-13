# HammeRR

**A modern, AI-first Source map editor for Linux.**

HammeRR is written in Rust for Counter-Strike: Source. It reads and writes
Hammer's VMF format, renders the game's own materials, and gives humans and
AI agents the same editing operations through a shared session and MCP.

The aim is a new editor with integrated tooling and a modern interface.
Development is active; see [current limitations](#status) before compiling
maps for play.

![Current editor showing an MCP-built courtyard and the light inspector](docs/images/editor.png)

*A courtyard built through MCP, with textured brushes, detailed light helpers,
and the shared entity inspector and activity panel.*

[Features](#features) · [Quick start](#quick-start) · [Editing](#editing) ·
[MCP](#mcp) · [Compile and play](#compile-and-play) · [Status](#status)

## Features

| Area | Available now |
| --- | --- |
| Maps and content | VMF editing and saving; byte-for-byte round trips for unmodified files; game materials from Steam libraries, search paths and VPK archives. |
| Viewport and brushes | Textured, flat, wireframe and diagnostic modes in every projection; grids, fractional snapping, brush drawing, move/rotate/scale handles, exact transforms, and texture locking. |
| Saved views | Named perspective and planar cameras stored in the VMF, with restore, rename, replace, delete and undo. |
| Selection | Object and face marquees, 3D volumes and visible-surface paint strokes, with shared previews and replace/add/remove/toggle rules. |
| Brush hollowing and carving | Wall shells and combined convex cutter subtraction, with inherited materials and lighting, cancellable previews and one-step undo. |
| Brush clipping | Planar cuts, three-point surface picking, exact planes and 3D move/rotation handles; keep either side or both, cap materials and one-step undo. |
| Brush components | Viewport/list selection, move handles, exact offsets, component scaling/rotation, corner welding and edge/face splitting, with guarded previews and atomic undo. |
| Brush shapes | Parameterized wedges, cylinders, cones, spheres and arches, with material previews, editable dimensions and cancellable geometry previews. |
| Recovery | Periodic and manual snapshots, previous-save backups, safe recovery copies, persistent settings and matching MCP controls. |
| Map statistics | Authored geometry, entities, material/model usage, terrain, lightmap scales and hidden-object counts with paginated breakdowns. |
| Map checks | Geometry, IDs, textures, entity logic, asset paths and spawn checks; issue navigation and reviewed, undoable repairs. |
| Design context | Map briefs, object notes and editor-only regions; shared drafts, search, relevant-context retrieval, bounded storage and undo. |
| Face materials | Individual face selection, sampled material/projection strokes and batch painting, edge wrapping, world/face alignment, UV controls, per-face/grouped fitting, view projection and placement, with cancellable previews and grouped material thumbnails. |
| Material replacement | Exact map/object/face searches, explicit hidden-face handling, matching-face selection, guarded previews and atomic replacement. |
| Face lighting | Batch lightmap scales and smoothing groups, mixed-value controls, cancellable previews, and density/group viewport diagnostics. |
| Entities | FGD class search, typed properties, choices, spawn flags, lights and player spawns, plus brush entity conversion. |
| Entity logic | Connection editing, target/input suggestions and firing rules; reference inspection and safe renaming with previews, Cancel and atomic undo. |
| Map management | World settings, mixed-value property batches and default resets; object reports, search, selection, framing and visibility restore. |
| Humans and agents | Shared map, selection, camera, previews and undo history. MCP edits appear live in the editor. |
| Build and play | Native BSP compilation, shared stage/status reporting and cancellation, leak paths, CS:S launch controls, and game logs. |

## Quick start

Requires **Rust 1.92+**. The graphical editor and rendered views need a
**Vulkan-compatible graphics backend**; headless editing and compilation can
run without graphics initialization. Install Counter-Strike:
Source to use its materials; HammeRR discovers the game through Steam.

```sh
cargo build --release -p editor -p compiler -j 8
./target/release/editor path/to/map.vmf
```

Omit the filename to start an empty map. The editor also discovers the
installed CS:S SDK entity definitions.

<details>
<summary>Launch options</summary>

| Option | Purpose |
| --- | --- |
| `--game <gameinfo.txt>` | Use a specific game content installation. |
| `--fgd <file.fgd>` | Load another entity definition file and its includes. |
| `--mcp-port <port>` | Change the HTTP MCP port; the default is `7377`. |
| `--no-mcp` | Run the GUI without the MCP server. |
| `--headless` | Serve MCP over stdio; initialize graphics only when a rendered view is requested. |

```sh
cargo test --workspace -j 8
```

</details>

Asset paths stay inside each configured content mount. To use external content,
add its directory to `gameinfo.txt` search paths; the mount root may be a symlink.
Links inside a mount must resolve within that mount, and directory cycles are
not followed repeatedly.

## Editing

Geometry and material previews use current-map snapshots without copying undo
history. Cancel discards the draft; Apply adds one normal undo step.

### Navigation and selection

| Action | Control |
| --- | --- |
| Fly in perspective | Hold right mouse; move with WASD and Q/E. |
| Change projection | Choose **Perspective**, **Top · XY**, **Front · XZ** or **Side · YZ**. |
| Pan / zoom in planar views | Right-drag / mouse wheel; zoom follows the cursor. |
| Frame selection or map | **Frame · F**; frames the map when nothing is selected. |
| Save / return to a camera | **Saved views…** beside Frame. |
| Select / toggle selection | Click / Ctrl+click. |
| Select an area | Left-drag away from a handle; choose **Intersects** or **Fully inside** and a selection rule. Ctrl+drag toggles. |
| Paint a selection | Choose **Paint selection**, adjust the pixel radius, then drag across visible objects or faces. Release applies; Escape or **Cancel** discards the stroke. |
| Transform with handles | Choose **Move handles**, **Rotate handles** or **Scale handles**; drag a handle and release to apply. Escape cancels. |
| Nudge selection | Arrows and PgUp/PgDn; Shift uses at most one unit. |
| Duplicate / delete | Ctrl+D / Del. |
| Undo / redo | Ctrl+Z / Ctrl+Y. |

**Grid** accepts spacing from 0.125 to 4096 units. **Snap** and **Show grid**
are independent. In planar views, arrows move within the displayed plane;
PgUp/PgDn move along its depth axis.

Framing fits the viewport's width and height while preserving the camera's
direction and field of view. This also applies to portrait MCP renders and
**Frame box** in volume selection.

### Viewport modes

Choose **Textured**, **Flat shading**, **Wireframe**, **Lightmap density** or
**Smoothing groups** in the **Viewport** section. Modes work in Perspective,
Top, Front and Side. Planar cameras now honor the selected mode; choose
**Wireframe** for the traditional outline view or to see geometry through solids.
Terrain wireframes include the displaced triangle edges. Light icons, selection
highlights and editing guides remain visible.

Flat shading ignores material textures and transparency to show solid geometry.
Density and smoothing views inspect authored metadata; they do not preview baked
lighting. Tool-brush visibility uses the same filter in every mode.

| Flat shading | Wireframe |
| --- | --- |
| ![Untextured geometry and a light helper](docs/images/flat-view.png) | ![Brush edges and the same light helper](docs/images/wireframe-view.png) |

### Saved camera views

Open **Saved views…**, name the current camera and choose **Save current as new**.
Select a view and **Go to view** to restore its projection, position, direction,
field of view and planar zoom. **Rename** changes its name; **Replace with current**
updates its camera. **Delete view** removes it. These edits use map undo and
persist when you save the VMF; moving to a saved view leaves the map unchanged.

A map can hold 64 views with names up to 80 characters. **Cancel / close**
discards the name field; applied changes remain undoable. Existing Hammer
camera records are preserved separately.

![Named camera views and shared restore/edit controls](docs/images/saved-views.png)

### Brushes and transforms

In a planar view, click **Draw brush**, set **Start Z/Y/X** and **Depth Z/Y/X**,
then drag a footprint. The preview shows snapped dimensions; releasing
places the brush in one undo step. Escape cancels, and **Done** returns to
selection. **Add brush** places a default block in perspective.

Choose a handle tool above the viewport for movement arrows and plane handles,
rotation rings, or axis and uniform scale handles. **World axes** and **Local
axes** control the frame; local axes follow the first selected object's brush
orientation or entity angles. **Snap** uses the grid for relative movement and
scaled dimensions, and **Angle snap** sets rotation increments. Releasing a
drag applies one undo step; Escape or **Cancel** discards the draft.

Enable **Transform selection** in the inspector for exact XYZ inputs. Choose
the selection center, world origin or custom coordinates as the pivot. These
settings also control handles. **Follow move and rotation** moves the texture
with the brush. **Stretch textures when scaling** controls scaling separately:
enable it to preserve corner UVs, or disable it to keep the texture's size and
add repetitions as the brush grows. Both off keeps the original world
projection. The same choices are available in the scale-handle toolbar.
Numeric transforms remain exact, independent of grid snapping. During a shared
preview the inspector shows its actual values and a read-only summary of every
active operation, including combined transforms. Use **Apply transform** or
**Cancel** before applying another numeric operation.

| Texture keeps its size | Texture stretches with the brush |
| --- | --- |
| ![A resized brush with repeated texture tiles](docs/images/object-texture-repeat.png) | ![The same resized brush with stretched texture tiles](docs/images/object-texture-stretch.png) |

Choose **Mirror** for brush reflection. Check X, Y or Z, choose world/local axes
and a pivot, then **Mirror selection**. **Mirror textures with geometry** keeps
corresponding corner UVs; disabling it retains the original world projection.
Odd axis combinations reverse plane winding while preserving side IDs. Brush
entities keep their ownership and mirror their origins. Point entities and
terrain currently reject the whole operation without changing the map.
Reflected geometry and pivots must stay within ±65536 editor units; compilation
still uses Source's narrower coordinate limits.

Choose **Shear** to shift one coordinate in proportion to another. For example,
**Shift coordinate X**, **By coordinate Y**, factor **0.5** moves X by half the
Y distance from the pivot. World/local frame and pivot choices apply as usual.
Factors range from −64 to 64, and the two axes must differ. **Keep textures
attached while shearing** preserves corresponding corner UVs; disabling it
retains world projection. Brush entities keep their solid IDs and transform
origins with the geometry. Nonzero shear rejects point entities and terrain
atomically; zero is an identity operation.

| Before shear | After shear with attached textures |
| --- | --- |
| ![An oblique brush before shearing](docs/images/brush-shear-before.png) | ![The same brush sheared along X by its Z coordinate](docs/images/brush-shear-after.png) |

Area selection works in perspective, Top, Front and Side, for objects or
faces. It includes geometry through the view's depth and skips hidden brushes
and hidden tool materials. A brush entity is selected as a whole. **Fully
inside** requires all its visible brushes to fit; **Intersects** accepts any
intersection. Selection previews highlight candidates without changing the
map or adding undo history.

Open **Select a volume…** to select inside exact XYZ bounds. Start from selection
or map bounds, choose objects or faces and a selection rule, then **Preview
matches**. The blue box and highlights survive camera changes; **Frame box**
shows the region. **Select matches** applies it; **Cancel** leaves selection alone.

Choose **Paint selection** for a circular brush that selects the nearest visible
object or face at each sample. **Radius** uses screen pixels; the same selection
rules apply, and Ctrl temporarily toggles. Repeated passes count each hit once.
The shared preview shows matching geometry and a brush outline. Release or
**Apply selection** commits it; **Cancel** leaves the selection alone.

Strokes sample a fixed 2-pixel lattice and interpolate between pointer updates.
Tiny features between samples can be missed. Hidden objects and tool brushes
are skipped; face strokes ignore point helpers. Imported terrain uses its
supporting brush faces for selection queries.

| Volume controls | Shared region preview |
| --- | --- |
| ![Volume bounds, match rules and cancellation](docs/images/volume-selection.png) | ![Blue volume guide and matching brush](docs/images/volume-preview.png) |

![Paint selection with a pixel-radius brush, candidate highlights and Apply/Cancel](docs/images/paint-selection.png)

### Vertices, edges and faces

Select brushes, then open **Reshape brushes…**. **Brush scope** chooses one
brush by ID, material and position, or **All selected** captures up to sixteen
brushes. Choose **Add to scope** to build a subset across pages, or **Remove from
scope** to drop a child. **Inspect this brush** replaces the captured scope.
Removing its final member stops picking. Large brush entities have paginated
child choices; changing scope keeps their owner and siblings intact. Choose **Edges**, **Vertices** or
**Faces**, then **Pick in viewport**. Click to select, Ctrl-click to toggle, or
use the component list to choose overlapping parts. The original brush scope
stays available as you select components. An empty click clears the component
selection. **Stop viewport picking**, **Cancel** or Esc returns to object picking
and discards the component draft.

| One child from a large entity | Subset spanning pages |
| --- | --- |
| ![Inspecting one child from the final page of a large brush entity](docs/images/component-brush-scope.png) | ![Adding children from separate pages to a fixed brush scope](docs/images/component-brush-subset.png) |

Corners and edges can be selected through surfaces, matching their blue guides.
Face clicks select the nearest visible brush surface. Hidden brushes have no
guides and cannot be picked. The list still provides precise access to every
component in the selected scope.

Drag a colored arrow to move along a world axis, a plane square to move along
two axes, or the center square to move within the view plane. Release applies
one undo step; Escape cancels. **Show viewport move handles** toggles the guides.
Selection and the original brush scope remain available after applying a move.

Alternatively, enter an XYZ offset in world units. **Use editor grid snapping**
follows the shared grid and Snap setting for both handles and numeric previews.
**Preview move** shows the proposed shape; **Apply move** records one undo step.
**Cancel** discards the draft and clears the component markers.

Orange marks selected components; blue marks available corners and edges. Offsets are
measured from the committed map, so revising a preview does not accumulate moves.
Invalid offsets leave the last valid preview visible and block drag commit until
the next valid update. Zero total displacement adds no history. Faces must remain flat
and the brush convex: moving one corner of a box usually requires moving its
connected edge or the whole face together. Displacement
movement is not supported yet. Existing face IDs, materials, lighting fields
and unknown metadata are retained. Textures stay anchored in world space by
default. Enable **Keep textures at corners** to stretch or rotate the texture
with each reshaped face. Every corner must keep its original texture coordinates;
moves that cannot do that with Source's projection are rejected. Disable the
option or move the whole face when that happens.

Choose **Scale selection** or **Rotate selection** to reshape the selected
faces, edges or corners. Scale uses positive XYZ factors: **1** keeps that axis
unchanged, **0.5** halves it and **2** doubles it. Rotation uses degrees around
world axes in X, then Y, then Z order. **Pivot** uses the selected components'
center or an exact world position; unselected corners stay fixed. For example,
scaling a box's top face creates a frustum, while rotating that face can make a
sloped roof. Twisted, collapsed or concave results are rejected.

**Keep textures at corners** attaches every corner UV when Source's projection
can represent it. **Preview scale/rotation**, **Apply**, **Discard preview** and
**Cancel** share the same guarded workflow and one-step undo. Move handles pause
during these previews. Scale factors and angles are measured from the committed
map, so revising a preview does not accumulate transforms.

| Component scaling | Component rotation |
| --- | --- |
| ![Scale factors and selected-center pivot with cancellable preview](docs/images/component-scale-controls.png) | ![Degree inputs and custom world pivot with Apply and Cancel](docs/images/component-rotate-controls.png) |

| Shared frustum preview | Shared sloped-roof preview |
| --- | --- |
| ![Top-face scaling leaves the bottom corners fixed](docs/images/component-scale-preview.png) | ![Top-face rotation creates a sloped roof](docs/images/component-rotate-preview.png) |

Choose **Weld corners**, select **Vertices**, and select at least two corners in
each brush. **Preview weld** merges them at their average position. For example,
welding a box's four upper corners creates a pyramid. Under **Merge corners at**,
choose **Average selected corners**, **Existing brush corner**, or **World
position**. The corner picker keeps the original brush choices during preview;
world-position fields start at the selected corners' shared center and accept
exact X/Y/Z coordinates. An explicit target uses the same position in every
edited brush. Collapsed faces are removed;
the preview shows their count and highlights the resulting corner. Surviving
faces keep their IDs, material, world texture projection and metadata.
Nonplanar, concave, empty or redundant geometry is rejected. Movement handles
and selection changes pause during the topology preview. **Apply weld** records
one undo step; **Discard preview** restores the original corner selection.

Enable **Keep textures at surviving corners** to attach textures during a weld.
Surviving corners retain their UVs; the merged corner averages its contributors'
UVs separately on each source face. Incompatible projections reject the preview
without replacing the previous valid draft. World projection remains the default.

| Welding controls | Shared pyramid preview |
| --- | --- |
| ![Merged selection and Apply/Cancel controls](docs/images/brush-welding.png) | ![Four box corners welded into a pyramid apex](docs/images/component-weld-preview.png) |

| Exact weld target | Shared target preview |
| --- | --- |
| ![World-position weld inputs with Apply, Discard and Cancel](docs/images/brush-weld-position.png) | ![A pyramid welded to a fractional custom apex](docs/images/component-weld-position.png) |

Choose **Split and raise**, then select one face or edge per brush. A face gains
a corner at its center, raised along the face normal. An edge gains a corner at
its midpoint, raised outward between its two face normals. Enter **Raise outward**
in world units or use the slider, then **Preview split**. Existing corners stay
in place, and the affected faces become triangles. Each source face keeps its ID
on the first triangle; added faces get new IDs and inherit its material, world
texture projection, lighting and metadata.

Enable **Keep textures at corners** to stretch each new triangle with its
raised corner. Original corner UVs stay fixed; the inserted corner keeps the UV
at the flat face center or edge midpoint. Changing this option requires another
preview before Apply.

The preview highlights the inserted corner and reports added faces.
**Apply split** verifies the planned IDs and records one undo step, keeping
the new corner selected for further editing. **Discard preview** restores the
original selection. Rises from 0.125 to 4096 units are supported when the result
stays convex and within the editing bounds. Crossing neighboring faces,
exceeding 64 faces or selecting multiple components in one brush is rejected.

| Face split controls | Shared face split preview |
| --- | --- |
| ![Raised corner controls with rise slider and Apply/Discard/Cancel](docs/images/brush-face-split.png) | ![A face split into four triangles around a raised corner](docs/images/component-face-split.png) |

| Edge split controls | Shared edge split preview |
| --- | --- |
| ![Edge split controls and inserted-corner selection](docs/images/brush-edge-split.png) | ![A raised edge midpoint shared by six new triangles](docs/images/component-edge-split.png) |

Choose **Roof ridge** and select one face with four corners per brush. Pick its
ridge direction, enter the height above the face or use the slider, then
**Preview ridge**. The two opposing edge midpoints rise together, forming two
roof faces while all old corners stay fixed. Each brush has its own direction
picker. New faces inherit the source material, precise world texture projection
and metadata. Incompatible shapes are rejected before allocation.

Enable **Keep roof texture at corners** to preserve the flat pieces' UVs on
both raised roof faces, including their edge midpoints. Neighboring faces keep
their world projection. This option also works when raising a flat partition;
changing it requires a new preview before Apply.

**Apply ridge** selects both new corners and records one undo step.
**Discard preview** restores the original face selection without consuming IDs;
**Cancel** closes the tool and drops its draft. Both directions have been checked
through saved-map native compilation; CS:S runtime coverage remains separate.

Choose **Flat partition** to stage two pieces for keeping or extrusion. Select one
convex face with three to eight corners per brush. Choose a boundary-edge pair,
or **Trim corner** to cut off a corner, then **Stage flat partition**. Quad
faces retain their two across-face directions and four trim-corner choices.
The viewport shows the dividing edge while the original brush stays unchanged.
Use **Highlight pieces** and **Frame highlighted pieces** to inspect either
piece. You can also click a visible piece in the viewport; Ctrl toggles its
highlight and an empty ordinary click clears highlights. Hidden objects and
foreground brushes follow the same visibility rules as face picking.
When both pieces are quads, enter the **Shared ridge height** and **Preview ridge**
to raise them together; **Return to flat** restores the staged surface.

**Apply raised partition** records one undo step. **Discard preview** restores
the original face selection, and **Cancel** closes the tool. **Keep flat pieces**
saves the dividing intent without changing brush geometry. Save the map, reopen
it later, and use **Saved flat pieces → Resume pieces** to inspect or extrude them,
or raise a supported two-quad ridge.
**Remove saved** removes an intent, including one whose source was deleted or
changed. Keeping, removing and applying raised pieces each share ordinary undo.
Applying a raised partition consumes its saved flat intent in the same edit.
Saved dividing edges stay visible within the active component scope.
Resolve a pending draft by keeping, applying or cancelling before saving,
compiling or playing. Highlighting controls inspection and framing; raising
always affects the shared ridge of both pieces.

Choose **Extrude highlighted pieces** to add separate volumes above one or both
flat pieces. Set a positive height, optionally choose a wall material with the
material browser, and keep the top texture attached if needed. Preview before
Apply; changing highlights requires a new preview. The original brushes and
saved pieces remain reusable. New volumes retain world or brush-entity ownership,
and Apply selects their faces in one undo step. **Return to flat** and **Cancel**
discard the added draft geometry.

Enable **Planned top material** to change the highlighted pieces' future roof
or prism tops. Choose a material in the grouped browser, or enter its name and
use **Preview material**. Each piece lists its effective material and whether
it is inherited or planned. Mixed highlights show a mixed value;
**Restore inheritance** returns them to the source face's material.
Flat previews use the shared renderer without adding coplanar brush faces.
Keep saves these choices; Cancel stores nothing. Extrusion bases retain the
original source material, and wall choices remain independent. Return to flat
before changing a material plan.

Enable **Top texture placement** to shift, scale or rotate highlighted pieces.
The fields show texels, world units per texel and degrees; negative scales
mirror the texture. **Preview placement** shows the result on the flat face.
**Use source placement** restores inheritance. Keep preserves the plan for
later, while raising or extruding applies it to the corresponding tops.

| Texture placement controls | Independent piece projections |
| --- | --- |
| ![Top texture placement with units, rotation slider and inheritance](docs/images/partition-texture-controls.png) | ![Different shifts, mirrored scales and rotation on two flat pieces](docs/images/partition-texture-preview.png) |

Enable **Adjust dividing edge** to move the two cut endpoints with percentage
inputs or sliders. **Preview cut** creates unequal or diagonal pieces while
retaining the original face. Keep these positions for later or extrude the
pieces. Saved positions and piece identity survive reflection and reopening.
Shared ridges can use adjusted endpoints when both raised quad pieces remain
planar and convex. Cuts without two quad pieces offer Keep and Extrude in the
menu. A diagonal cut that would bend a roof is rejected while retaining the
flat preview. Adjust the positions, extrude the pieces, or use
**Reset cut to midpoint**. The bottom footer keeps Apply, Discard and Cancel
accessible when the settings scroll.

| Planned top material controls | Flat planned surfaces |
| --- | --- |
| ![Highlighted-piece material choices with preview, browsing and inheritance](docs/images/partition-material-controls.png) | ![Two real materials on a flat checked face without adding brush sides](docs/images/partition-material-preview.png) |

| Boundary-cut controls | Triangle and pentagon prisms |
| --- | --- |
| ![Trim-corner direction and bounded flat piece controls](docs/images/partition-boundary-controls.png) | ![Independently styled triangle and pentagon pieces extruded above their source](docs/images/partition-boundary-preview.png) |

| Adjusted ridge controls | Unequal roof widths |
| --- | --- |
| ![Readable cut percentages and checked shared ridge controls](docs/images/partition-adjusted-ridge-controls.png) | ![An adjusted dividing edge raised with independent roof materials](docs/images/partition-adjusted-ridge-preview.png) |

| Cut controls | Adjusted pieces extruded to different heights |
| --- | --- |
| ![Percentage inputs and sliders for opposing edge positions](docs/images/partition-cut-controls.png) | ![A diagonal cut used to create two separately extruded volumes](docs/images/partition-cut-preview.png) |

| Extrusion controls | New volumes above the original brush |
| --- | --- |
| ![Height, wall material and top texture controls for highlighted pieces](docs/images/partition-extrusion-controls.png) | ![Two flat pieces extruded to different heights above an unchanged source brush](docs/images/partition-extrusion-preview.png) |

Extrusion outlines follow the visibility of the new volume, including its wall
materials and hidden owner. Visible walls remain highlighted when the source
brush is hidden by the tool-material filter.

| Flat pieces staged | Shared ridge raised |
| --- | --- |
| ![An independent dividing edge on an unchanged brush face](docs/images/component-partition-flat.png) | ![The staged pieces raised into two checked roof planes](docs/images/component-partition-raised.png) |

![Flat partition controls with source corners, midpoints and highlighted pieces](docs/images/component-partition-controls.png)

![Saved flat partitions with Resume and Remove controls](docs/images/component-partition-saved-controls.png)

| Ridge controls | Shared ridge preview |
| --- | --- |
| ![Roof ridge direction, height and Apply/Discard/Cancel controls](docs/images/brush-roof-ridge.png) | ![Two roof planes joined at the raised opposing edge midpoints](docs/images/component-roof-ridge.png) |

Flat cuts support convex faces with three to eight corners and create two pieces
with three to nine vertices each. Saved original-corner membership keeps piece
materials attached after reflection and reopening. Ridges require two quad
pieces; the operation menu offers flat keeping and extrusion for other shapes.
One batch supports 16 source brushes and 32 extruded pieces, with at most 256
generated planes. Previews validate the complete batch before allocating IDs.

| Polygon cut controls | Octagon source split and extruded |
| --- | --- |
| ![Complete polygon cut choices, endpoint sliders and Keep/Cancel controls](docs/images/polygon-partition-controls.png) | ![Two independently styled polygon prisms above an octagonal source face](docs/images/polygon-partition-preview.png) |

Movement and welding support 512 selected components across 16 brushes, with up
to 64 sides per brush. Duplicate IDs, open/redundant geometry and changes that
collapse edges are rejected before the map is changed. Movement preserves
topology; welding, splitting and ridges require a closed, convex result with the planned corners.

| Component controls | Shared shape preview |
| --- | --- |
| ![Face, vertex and edge selection, precise offsets and Apply/Cancel controls](docs/images/brush-components.png) | ![Selected top face moved upward with shared viewport handles](docs/images/component-preview.png) |

The same height change with world anchoring and with texture attachment:

| World-anchored texture | Texture kept at the original corners |
| --- | --- |
| ![Texture repeats as the brush grows taller](docs/images/texture-world.png) | ![Texture stretches with the taller brush](docs/images/texture-attached.png) |

### Hollow brushes

Select solid brushes or brush entities, then open **Hollow brushes…**. Set the
wall thickness and choose whether walls build **Inside the original size** or
**Outside the original size**. Inside preserves the outer shape; outside keeps
the original shape as the empty interior. Thickness is measured perpendicular
to each face, including slopes.

Under **Openings**, check any source faces that should stay open, such as the
roof or one side. Unchecked faces become walls, and each source must keep at
least one wall. The original boundary stays in place at an opening, so an open
roof keeps the side walls at full height.

**Preview walls** shows the proposed shell. **Look inside** moves the camera
into a cavity; select its number when hollowing several brushes. Wireframe also
reveals the walls from outside. **Create walls** replaces the original solids
in one undo step. **Cancel** discards the draft and retains camera navigation.

Each wall is an ordinary convex VMF brush. Inner surfaces inherit their source
face's material, world-space texture mapping and lighting settings. Brush
entities retain ownership of their walls. The tool rejects invalid inner space,
displacements and ambiguous geometry before replacing a valid preview. Limits
are 16 source brushes, 64 sides per brush, 256 output walls and thickness from
0.125 to 4096 units.

| Hollow controls | Inside the shared preview |
| --- | --- |
| ![Wall thickness, opening faces and cancellable shell preview](docs/images/hollow-brushes.png) | ![Textured interior of a hollowed brush before applying](docs/images/hollow-interior.png) |

![An open roof and front face, with full-height side walls](docs/images/hollow-open-shell.png)

### Carve brushes

Select a cutter and the brushes to carve, then open **Carve brushes…**. Choose
**Cutter · shape to subtract** and check the **Target brushes**. **Preview carve**
shows the resulting convex fragments and lists fully removed and unchanged
brushes. **Apply carve** records one undo step; **Discard preview** or **Cancel**
leaves the map unchanged. Unchecked candidates remain available during preview.

Enable **Use multiple cutter brushes** to subtract several shapes in one
preview. Each selected brush has explicit **Cutter** and **Target** roles.
Overlapping cutters remove their shared volume once; caller order produces the
same result. All cutters stay in the map. Move or delete them when the carved
space should be empty. Original fragment faces retain their surface settings; new cut faces
inherit the corresponding cutter face's material, world texture projection,
lighting and metadata. Brush-entity ownership and unrelated objects remain.
An entirely covered world brush can disappear; removing every brush from an
entity is rejected so its meaning cannot silently change.

Limits are 1–8 convex cutters, 16 targets, 256 intermediate/final fragments
and 64 faces per brush.
Displacements, invalid or thin fragments and budget overflow reject atomically.
Apply checks generated brush and face IDs against the exact preview.

| Carving controls | Shared fragment preview |
| --- | --- |
| ![Cutter selector, target checkboxes and guarded preview actions](docs/images/brush-carving.png) | ![Six carved fragments around the retained cutter](docs/images/carve-preview.png) |

| Multiple-cutter roles | Result with cutters hidden |
| --- | --- |
| ![Explicit cutter and target roles with cancellable batch preview](docs/images/multiple-cutter-controls.png) | ![Two combined cutter shapes leave a stepped cavity](docs/images/multiple-cutter-cavities.png) |

### Brush clipping

Select brushes or a brush entity, then click **Clip selected brushes**.
Draw a cut in Top, Front or Side, or enter a plane normal and distance.
Choose **Arrow side**, **Opposite side** or **Both sides**, and optionally
browse for the cut-face material. **Apply cut** commits the preview in one
undo step; **Cancel cut** or Escape discards it.

Choose **Pick three points** to click visible brush surfaces in any view.
You can change views between points. Orange markers show their positions;
the third distinct, noncollinear point previews the cut. **Remove last point**
lets you revise it, **Start again** clears the points, and **Use plane / line**
returns to the other controls. Point placement follows editor snapping.

Once a cut is previewed, choose **Move plane** or **Rotate plane** under
**Handles**. Drag a world-axis arrow, movement square or rotation ring.
**Snap** uses the editor grid for movement and the displayed angle step for
rotation. Releasing a handle keeps the preview; **Apply cut** changes the map.
**Cancel positioning** or Escape during a drag restores its starting plane.
Three-point markers follow the plane, and the side and material choices stay set.

| Position the cut | Rotate the plane |
| --- | --- |
| ![Shared cut-plane move handles](docs/images/cut-plane-move.png) | ![Rotation rings on the positioned cut preview](docs/images/cut-plane-rotate.png) |

![Three-point cut with point markers and the shared plane preview](docs/images/three-point-cut.png)

Existing faces keep their texture projections. Brushes the plane does not
cross stay unchanged. Intersected displacements cannot be clipped yet.

### Brush shapes

Open **Brush shapes…**, choose a shape and enter its world bounds. Cylinders,
cones and spheres use segment counts; arches also have an inset, start angle
and sweep. Choose the height/depth axis and browse for the face material.
**Preview shape** shows the geometry without changing the map. **Focus preview**
frames it; **Add shape** creates and selects every brush in one undo step.
**Cancel**, Escape or closing the window discards the preview.

Curves are polygonal and inscribed in the supplied bounds. Arch bounds describe
the full ellipse; a 180° sweep builds half of it, and 360° builds a ring. The
preview reports the actual size and face count. Arches consist of separate
convex brushes; all shapes remain ordinary editable VMF geometry.

| Shape controls | Shapes created through MCP |
| --- | --- |
| ![Brush shape dimensions, material preview and cancellation controls](docs/images/brush-shapes.png) | ![Wedge, cylinder, cone, sphere and segmented arch](docs/images/brush-primitives.png) |

### Map statistics

Open **Map statistics…** to see authored brush sides, world/entity brushes,
point/brush entities, connections, materials, model references and terrain.
The report includes hidden objects and refreshes when the map changes. Choose a
usage breakdown and page through materials, entity classes, model paths,
lightmap scales or displacement powers, ordered by frequency.

Counts describe the current map structure, excluding pending previews. Brush
sides are plane records, not compiled faces; model paths are references, not
verified assets. Hidden brush totals include brushes inside hidden entities.
Use **Object report…** to search IDs and frame individual or matching objects.
MCP `get_map_statistics` exposes the same totals and pages with generation and
revision markers; `get_object_report` provides ID lookup and navigation targets.

![Authored map totals and paginated material usage](docs/images/map-statistics.png)

### Pointfiles and leak navigation

Open **Pointfile…** to load a `.lin` path or explore the current compile's leak.
**Browse…** chooses a file; **Load** validates it and shows the red path without
moving the camera. Use **Frame path**, enter a point number and **Go**, or step
with **Previous** and **Next**. Perspective follows the path's direction;
planar views center on the point while keeping their zoom.

**Unload path** removes the overlay without deleting the file. Closing the
window leaves the path loaded. Map edits make external paths stale and hide
them until reloaded; a current compile result replaces them, and opening a map
clears them. Loading and navigation do not change map data or undo history.
Files accept 2–65,536 finite XYZ points, up to 2 MiB. Paths too large for
planar zoom explain the limit; use perspective framing or individual points.

MCP shares this through `load_pointfile`, `get_pointfile`, `frame_pointfile`,
`navigate_pointfile` and `clear_pointfile`. Inspection exposes path length,
bounds, paginated coordinates, current/stale state and a token for guarded
actions. MCP point indices start at zero; the human controls start at one.

| Pointfile controls | Loaded path in the existing demo map |
| --- | --- |
| ![Pointfile loading, framing and point-by-point navigation](docs/images/pointfile.png) | ![Red pointfile path drawn through geometry](docs/images/pointfile-viewport.png) |

### Check a map

Opening or recovering a VMF with duplicate object or face IDs reports the
offending ID and preserves the current map, preview and undo history. Fix those
IDs before reopening; HammeRR does not silently renumber imported objects.
Root visgroup extension fields and blocks are retained on save and reopen.

Open **Check map…** and choose **Run checks**. Filter errors and warnings,
select an issue to read its explanation, then use **Select issue** or
**Focus issue** to locate it. Reports show when edits or changed entity
definitions make them stale.

**Review repair…** shows the exact before/after values for a supported repair.
**Apply repair** records one undo step; **Cancel repair** changes nothing.
Initial repairs reset invalid texture projections and zero lightmap scales.
Other issues link you to the affected content for manual editing.

![Map issues, face navigation and an explicit repair preview](docs/images/map-checks.png)

Checks include hidden content and inspect applied edits. Custom properties are
preserved. Spawn checks warn when a visible team spawn's standing hull overlaps
visible world or `func_detail` brushes with known player-blocking contents,
including player clips. Touching floors are allowed; uncertain materials and
invalid origins report unchecked clearance. Select or Focus locates the spawn.
Models, terrain, other brush entities, gameplay objectives, full asset
dependencies and engine-specific resource limits still need further checks.
Compile and test in CS:S to validate leaks and playability.

### Design context

Open **Design context…** to add a **Map brief**, a **Note on selection**, or a
**New region**. Keep intent, constraints and useful observations close to the
geometry they describe. Region bounds and guide colors are editable; guides can
be hidden or focused. Search all notes or find relevant context for the selected
object, a point or an area.

**Save note** records one undo step. **Cancel edit** and **Cancel / close** discard
the shared draft. Notes show their author and whether the source changed since
they were saved. Copied objects retain notes with observations marked for review;
deleting objects removes their notes, and undo restores both.

![Design notes, region bounds and shared editing controls](docs/images/design-context.png)

Notes are editor metadata, excluded from compiled BSPs. Limits are 200 summary
characters, 4 KiB of text per note and 1 MiB shared across the map. MCP retrieves
relevant context in bounded pages and exposes summaries during object inspection.
Notes are advisory; current instructions take precedence. Region change tracking
currently covers the whole map. Other editors' handling of this metadata has not
yet been verified.

### Autosave and recovery

**Recovery…** lists periodic snapshots, manual checkpoints and previous saved
versions. Autosave captures changed maps every two minutes by default, including
unnamed maps. It leaves the original VMF, dirty state and undo history alone.
You can change the interval and retention, or create a snapshot immediately.

Select a snapshot and choose **Open recovered copy**. It opens as an unsaved
map; choose a save path to keep it. Replacing unsaved edits and deleting a
snapshot require explicit confirmation. **Cancel / close** discards unapplied
settings fields. Settings and recovery files persist across editor restarts.

![Autosave settings, retained snapshots and safe recovery controls](docs/images/recovery.png)

Recovery uses `$XDG_STATE_HOME/hammerr/recovery`, falling back to
`~/.local/state/hammerr/recovery`. Set `HAMMERR_RECOVERY_DIR` to an absolute path
for a separate store. Turning autosave off leaves manual snapshots and backups
available. Failed snapshots appear in Recovery and the activity log.

### Face textures and materials

Choose **Faces · materials & textures**, then select surfaces with click
and Ctrl+click. **Apply** assigns the chosen material to those faces.
**Edit face textures…** controls offsets, scales, rotation, one-tile
fitting, edge/center placement and world/face alignment. **Preview textures** shows a draft; **Apply textures** commits it
in one undo step. **Cancel**, Escape or closing the dialog discards it.

To reuse a texture, select one source face and choose **Sample face texture**.
Select targets, open **Edit face textures…**, then choose material painting,
projection copying or wrapping across a shared edge. Wrapping keeps the seam
continuous; each target must touch the source. Samples retain their captured
values until sampled again or another map is opened.

**Wrap connected faces** carries the sample through selected intermediate
faces, folding it at every shared edge. Select an open strip around a column
or across several folds, then preview and apply once. Disconnected targets and
incompatible closed loops explain why they cannot wrap. The source stays
unchanged; up to 256 target faces are supported. Displacements are excluded.

| Connected wrapping | Texture continued around a faceted column |
| --- | --- |
| ![Connected face wrapping with preview, apply and cancellation](docs/images/connected-wrapping.png) | ![Sampled texture propagated across five successive shared edges](docs/images/connected-wrapping-viewport.png) |

**Browse materials…** offers thumbnail pages, a larger preview, search,
folder/category groups, and map-used, recent and favorite materials.
Choosing a thumbnail selects a candidate; **Use material** accepts it.
**Cancel** keeps the previous choice.

| Material browser | Face texture controls |
| --- | --- |
| ![Material thumbnails, categories and favorites](docs/images/material-browser.png) | ![Face material, offset, scale, rotation and preview controls](docs/images/face-textures.png) |

Blank texture fields preserve existing values when selections are mixed.
Fitting uses the material's real dimensions. Categories are inferred from
material paths. Favorites persist across sessions in the configured recovery
directory; recent applications last for the session. The browser and MCP share
up to 512 favorites and report storage failures while retaining the previous
choices. Fix a rejected preferences file and restart to load it again; failed
saves can be retried.

### Map textures across faces

Select faces and open **Map textures together…**. **Fit faces together** uses
one face's texture direction and fits across the combined bounds of coplanar
faces. **Project along current view** uses the camera's right and down directions
for a shared parallel projection. Faces edge-on to that view are rejected.

Choose a shared material and set the horizontal/vertical tile counts. **1 × 1**
fits one tile across the whole group; **2 × 1** repeats it twice horizontally.
Fractions enlarge the texture. **Preview mapping**, **Apply mapping** and
**Cancel** share the face-preview workflow and one-step undo. Geometry and
face lighting remain unchanged. View projection uses the camera at preview time
and does not add perspective distortion.

| Grouped texture controls | One texture fitted across adjoining faces |
| --- | --- |
| ![Shared material, reference direction and tile counts](docs/images/grouped-textures.png) | ![Continuous grouped texture mapping created through MCP](docs/images/grouped-textures-viewport.png) |

### Replace a material

Open **Replace materials…** and choose the source material. Search the whole
map, selected objects, or selected faces. **Find faces** reports matches and
how many hidden faces were skipped; enable **Include hidden faces** to include
them. Matching uses the exact material path and ignores case.

**Select matches** selects every result. Choose a replacement with its material
preview, then **Preview replacement** and **Replace faces**. **Cancel**, Escape
or closing the dialog discards its draft. Replacement preserves UV placement,
lighting settings and hidden flags, and undoes in one step. Changed maps or
search selections require a fresh search before applying.

![Material replacement scope, match counts, previews and cancellation](docs/images/material-replacement.png)


### Texture paint strokes

Select one face and choose **Sample face texture**, then choose **Paint texture**
in the viewport toolbar. The source material and thumbnail appear below the
brush controls. Choose **Material only** to keep the target UVs, or **Copy
projection** to copy the sampled axes, offsets, scales and rotation. Adjust
**Radius**, then drag over visible faces.

The draft shows the texture changes and the number of faces that will change.
Release or **Apply textures** commits the whole stroke in one undo step;
**Cancel** or Escape discards it. Selection stays unchanged. Repeated passes
paint a face once, and empty or unchanged strokes add no undo entry. Hidden
faces and point helpers are skipped. A projection that collapses on a target
surface is rejected. Sampling another source cancels the active draft.

![Sampled wood-floor painting across five stair treads with shared preview and cancellation](docs/images/texture-strokes.png)

### Face lighting

Select faces and open **Face lighting…**. Set a lightmap scale in world units
per lighting pixel, or use the 8/16/32/64 presets. Smaller values mean finer
authored lighting detail. Blank fields preserve each face's current scale.

Toggle groups individually to preserve other memberships across a mixed
selection. Groups **1–24** allow smoothing; shared **25–32** groups prevent it.
**Clear all groups** removes memberships, while **Reset group changes** restores
the current selection's values. **Preview settings**, **Apply settings** and
**Cancel** share the same draft and undo behavior as face texture editing.

Choose **Lightmap density** to inspect the authored grid, or **Group membership**
to highlight one group. These views work in perspective and planar cameras; choose Wireframe for
outline editing. The native compiler preserves the settings but does not yet bake
lighting or compute smoothed lighting normals.

| Face lighting controls | Authored lightmap density |
| --- | --- |
| ![Mixed smoothing groups, scale presets and preview controls](docs/images/face-lighting.png) | ![Lightmap density grids on brushes with different face scales](docs/images/face-lighting-density.png) |

### Entities and lights

Click **Entity…** to search point classes and place lights, spawns or other
entities. The initial position uses the camera or planar workplane and
shared snapping settings. Select an entity to edit its typed properties,
choose enumerated values and toggle spawn flags. Invalid values leave the
map unchanged.

The inspector puts common settings first: position and rotation components,
color pickers, brightness sliders, numeric inputs and material previews.
**?** shows descriptions; **Advanced properties** exposes engine keys and
custom fields. Entity, color, material and texture dialogs all have Cancel
buttons and Escape dismissal.

Point lights use a bundled [CC0 bulb model by Josh Dean / Poly Haven](https://polyhaven.com/a/lightbulb_led).
Spotlights and sunlight have direction helpers; selected spotlights show
cone guides. These are editor markers, visible in both the viewport and MCP
renders. They add no models or geometry to the exported map. See
[asset provenance](crates/render/assets/lightbulb/SOURCE.txt).

Select world brushes and use **Tie to entity…** to create a brush entity.
**Move to world** converts it back. Both operations share undo history with
agent edits. **Load FGD…** in the class picker loads other definitions;
custom classes and unknown keys remain editable.

### Entity connections

Select a point or brush entity and open **Connections…**. **Outgoing** lists
its outputs; **Incoming** shows map connections targeting it and lets you
edit their source. Click **Add connection** or **Edit connection**, then
choose an output, target and input. Suggestions use the map's named entities
and inherited FGD definitions; custom names and I/O are also supported.

Set the parameter, delay in seconds, and **Unlimited**, **Once** or a
specific firing count. **Apply connection** creates one undo step. **Cancel**,
Escape or closing the dialog discards unapplied fields. Deletion is undoable.
MCP can inspect and change the same draft while the dialog is open.

Missing targets, undeclared I/O and parameter type mismatches show warnings.
Invalid encoding, negative delays and invalid counts prevent Apply. Map edits
invalidate an open draft, so it cannot overwrite a shifted connection index.
Incoming links resolve named targets, prefix wildcards and `!self`; other
procedural targets resolve in game.

New connections use CS:S's comma-separated format, so their fields cannot
contain commas, quotes or control characters. Imported escape-separated
connections retain their delimiter; other imported records stay untouched
until explicitly edited or deleted.

![Entity connection form with readable fields, suggestions and Cancel](docs/images/entity-connections.png)

### References and safe renaming

Select an entity and open **References and rename…**, or edit its **Name**.
The preview lists incoming connections, FGD entity-reference properties,
parent links and reference parameters such as `SetParent`. Enter a new name
to see which fields will change. **Apply rename** updates the name and exact
references in one undo step. **Cancel**, Escape or closing the window discards
the draft.

Names shared by several entities require **Rename all … entities**; the preview
lists every member, including hidden entities. An occupied destination name
blocks Apply. Wildcards, procedural targets, ambiguous class references and
untyped text stay unchanged. References whose meaning may change are listed
under **Needs your attention**, with an explanation of each effect. Review them
before choosing **I’ve reviewed these effects; leave these fields unchanged**.
Changing the name or group resets that acknowledgment. **What will change**
lists the link updates separately. **Show engine details** reveals raw keys,
class names and IDs when needed.

Attachment suffixes, duplicate outputs, connection delimiters and imported
numeric formatting are preserved. Edits, undo/redo and FGD changes invalidate
an open preview. MCP shares the same draft, preview and guards. Raw MCP property
edits and explicit Advanced-property removal remain available; use this workflow
when renaming should propagate references. Embedded scripts, runtime-created
links and arbitrary text expressions require manual inspection.

![Reference rename preview with exact updates and unchanged references to review](docs/images/entity-rename.png)

### World settings and property batches

**World settings…** edits map-wide values such as the sky. Select several
entities and open **Edit selected entity properties…** to work on their
common properties. Mixed values and flags are identified; color and material
pickers are available. Changing one flag or color component preserves the
other values on each entity.

Edits are staged until **Apply properties**, then saved as one undo step.
**Cancel** discards the draft. **Use default** explicitly writes the FGD
default; **Remove override** deletes the stored key. The inspector distinguishes
stored values from defaults. Names use **References and rename…**.

### Finding and organizing objects

**Object report…** searches IDs, names, classes and materials, with type and
visibility filters and optional XYZ region bounds. Select or frame an
individual result or all matches,
including results beyond the current page. Selection can replace, add, remove
or toggle matching objects.

**Hide selected**, **Isolate selected**, **Show selected** and **Show all**
change saved visibility in one undo step. **Restore visibility** restores
the previous state, including hidden brushes inside entities. Newly created
objects keep their current visibility; deleted objects stay deleted.
**Hidden objects are omitted from compilation**, including imported hidden
objects. Show them before building if they should appear in the game.

| Property batches | Object report |
| --- | --- |
| ![Shared property draft with mixed values, brightness, defaults and Cancel](docs/images/batch-properties.png) | ![Object report with filters, selection modes and visibility actions](docs/images/object-report.png) |

## MCP

The GUI serves MCP at **`http://127.0.0.1:7377/mcp`**. Connect an agent with:

```json
{
  "mcpServers": {
    "hammerr": {
      "url": "http://127.0.0.1:7377/mcp"
    }
  }
}
```

For an isolated session without a window:

```sh
./target/release/editor path/to/map.vmf --headless
```

Human controls and MCP calls use the same Session operations. Agents can
build geometry, place lights, edit individual faces, preview changes,
compile maps and launch the game. `render_view` returns a PNG from any
viewpoint; `pick_at_pixel` resolves a rendered pixel to an object and face.

For example, place a warm light:

```json
{
  "name": "create_entity",
  "arguments": {
    "classname": "light",
    "origin": [0, 0, 128],
    "properties": { "_light": "255 220 180 400" }
  }
}
```

| Courtyard built through MCP | Face selection and fitted crate textures |
| --- | --- |
| ![MCP render of textured walls, stairs, crates and light helpers](docs/images/agent-render.png) | ![MCP close-up of stacked crates with one face selected](docs/images/agent-block.png) |

<details>
<summary>Tool reference by task</summary>

| Task | Tools |
| --- | --- |
| Pointfiles | `load_pointfile`, `get_pointfile`, `navigate_pointfile`, `frame_pointfile`, `clear_pointfile` |
| Files and inspection | `new_map`, `open_map`, `save_map`, `get_map_info`, `get_map_statistics`, `find_objects`, `get_objects` |
| Selection and history | `select`, `get_selection`, `set_selection_mode`, `undo`, `redo` |
| Object edits | `move_objects`, `duplicate_objects`, `delete_objects` |
| Spatial authoring | `set_spatial_settings`, `draw_block`, `cancel_block_preview`, `create_block`, `transform_objects` |
| Transform handles and previews | `set_transform_tool`, `get_transform_handles`, `begin_transform_drag`, `update_transform_drag`, `commit_transform_preview`, `cancel_transform_preview` |
| Area selection | `set_area_selection_options`, `select_area`, `get_area_selection`, `commit_area_selection`, `cancel_area_selection` |
| Paint selection | `set_paint_tool`, `begin_paint_selection`, `update_paint_selection`, `get_paint_selection`, `commit_paint_selection`, `cancel_paint_selection` |
| Brush clipping | `set_clip_tool`, `clip_brushes`, `get_clip_preview`, `set_clip_points`, `pick_clip_point`, `get_clip_handles`, `set_clip_handle_options`, `begin_clip_drag`, `update_clip_drag`, `finish_clip_drag`, `cancel_clip_drag`, `commit_clip_preview`, `cancel_clip_preview` |
| Brush hollowing | `preview_hollow`, `get_hollow_preview`, `look_inside_hollow`, `commit_hollow`, `cancel_hollow` |
| Brush carving | `get_carve_preview`, `preview_carve`, `preview_carve_many`, `commit_carve`, `cancel_carve` |
| Brush components | `get_component_sources`, `update_component_scope`, `get_brush_components`, `set_component_picking`, `pick_brush_components`, `select_brush_components`, `preview_component_move`, `commit_component_move`, `cancel_component_move` |
| Component reshaping | `preview_component_scale`, `preview_component_rotate`, `preview_component_weld`, `preview_component_split`, `preview_component_crease`, `commit_component_edit`, `cancel_component_edit` |
| Staged flat partitions | `preview_component_partition`, `update_component_partition_cut`, `update_component_partition_material`, `update_component_partition_texture`, `raise_component_partition`, `select_component_partitions`, `frame_component_partitions`, `pick_component_partitions`, `get_brush_components`, `commit_component_edit`, `cancel_component_edit` |
| Partition extrusion | `preview_partition_extrusion`, `select_component_partitions`, `frame_component_partitions`, `pick_component_partitions`, `get_brush_components`, `commit_component_edit`, `cancel_component_edit` |
| Saved flat pieces | `get_saved_component_partitions`, `store_component_partition`, `resume_component_partitions`, `remove_saved_component_partitions` |
| Component move handles | `get_component_handles`, `set_component_move_options`, `begin_component_drag`, `update_component_drag`, `commit_component_drag`, `cancel_component_drag` |
| Brush shapes | `create_primitive`, `get_primitive_preview`, `frame_primitive_preview`, `commit_primitive_preview`, `cancel_primitive_preview` |
| Entity authoring | `create_entity`, `set_entity_properties`, `tie_to_entity`, `move_to_world` |
| Entity connections | `get_entity_connections`, `get_connection_choices`, `begin_connection_edit`, `update_connection_draft`, `commit_connection_edit`, `cancel_connection_edit`, `delete_entity_connection` |
| References and renaming | `get_entity_references`, `begin_entity_rename`, `update_entity_rename`, `commit_entity_rename`, `cancel_entity_rename` |
| Map reports and visibility | `get_object_report`, `select_object_report`, `set_object_visibility` |
| World settings and property batches | `get_property_inspector`, `begin_property_edit`, `update_property_edit`, `get_property_edit`, `commit_property_edit`, `cancel_property_edit` |
| Entity definitions and controls | `search_entity_classes`, `get_entity_class`, `load_entity_definitions`, `get_entity_inspector`, `set_entity_color`, `set_entity_component` |
| Map checks | `run_map_checks`, `get_map_checks`, `select_map_issue`, `preview_map_fixes`, `apply_map_fixes`, `cancel_map_fixes` |
| Design context | `get_context_state`, `list_context_notes`, `get_context_note`, `get_context`, `begin_context_edit`, `update_context_edit`, `apply_context_edit`, `cancel_context_edit`, `delete_context_note`, `set_context_view`, `focus_context` |
| Recovery | `get_recovery`, `set_recovery_settings`, `create_recovery_snapshot`, `restore_recovery_snapshot`, `delete_recovery_snapshot` |
| Volume selection | `select_volume`, `get_volume_selection`, `commit_volume_selection`, `cancel_volume_selection`, `frame_volume_selection` |
| Face textures | `select_faces`, `get_face_selection`, `set_material`, `edit_faces`, `sample_face_texture`, `get_surface_sample`, `paint_face_texture`, `propagate_face_texture`, `project_face_group`, `commit_face_preview`, `cancel_face_preview` |
| Texture strokes | `begin_texture_stroke`, `update_texture_stroke`, `get_texture_stroke`, `commit_texture_stroke`, `cancel_texture_stroke`, `set_paint_tool` |
| Material replacement | `find_material_faces`, `get_material_search`, `select_material_faces`, `replace_materials`, `commit_face_preview`, `cancel_face_preview` |
| Face lighting and diagnostics | `edit_faces`, `get_face_selection`, `get_objects`, `commit_face_preview`, `cancel_face_preview`, `set_view`, `render_view` |
| Material browsing | `search_materials`, `browse_materials`, `preview_material`, `set_material_favorite` |
| Saved cameras | `get_camera_views`, `save_camera_view`, `restore_camera_view`, `rename_camera_view`, `delete_camera_view` |
| Camera and visual checks | `get_camera`, `set_camera`, `frame_selection`, `set_view`, `render_view`, `pick_at_pixel`, `raycast` |
| Compile and play | `get_compile_settings`, `set_compile_settings`, `start_compile`, `get_compile_status`, `cancel_compile`, `compile_map`, `run_map`, `launch_game`, `game_log`, `stop_game` |

`transform_objects` accepts independent `texture_lock` (follow movement and
rotation) and `texture_scale_lock` (stretch during scaling). Both use the chosen
pivot and world/local frame. Omit `texture_scale_lock` to inherit `texture_lock`
and retain the previous complete-lock/world-fixed behavior. The same flags in
`set_transform_tool` configure handles; omitted fields preserve their current
settings. `get_transform_handles.state` reports both effective values for
settings and drafts. Preview, commit, cancel and undo share the Session path.

For example, double the brush width while keeping its texture size:

```json
{
  "name": "transform_objects",
  "arguments": {
    "scale": [2, 1, 1],
    "texture_lock": true,
    "texture_scale_lock": false,
    "preview": true
  }
}
```

Mirror through the same transform tool, including shared preview/apply/cancel:

```json
{
  "tool": "transform_objects",
  "arguments": {
    "reflection": [true, false, false],
    "pivot": [0, 0, 0],
    "space": "world",
    "texture_scale_lock": true,
    "preview": true
  }
}
```

`get_transform_handles.state.preview.reflection` reports the chosen axes.
Positive numeric scale factors remain independent of reflection.

Single-axis shear uses the same tool and shared draft:

```json
{
  "tool": "transform_objects",
  "arguments": {
    "shear": {"axis": "x", "reference": "y", "factor": 0.5},
    "space": "local",
    "texture_scale_lock": true,
    "preview": true
  }
}
```

`get_transform_handles.state.preview.shear` reports these inputs. In combined
operations, scale/reflection precedes shear, rotation follows it, then
translation. Texture scale locking controls shear attachment separately from
movement/rotation locking.

`get_transform_handles.state.preview.display` exposes the same operation
classification and summary as the inspector. Zero shear and whole-turn-only
rotation report `identity`; compound drafts list all active operations.

`edit_faces` supports material, UV offsets/scales, rotation, fitting, lightmap
scale, smoothing groups and non-destructive previews. Commit or cancel the
shared preview from either front end. Material browsing and inspector tools
return the same catalog, labels and control types used by the GUI.

For example, preview denser lightmaps and add group 3 to the selected faces:

```json
{
  "name": "edit_faces",
  "arguments": { "lightmap_scale": 8, "smoothing_add": [3], "preview": true }
}
```

Pass the returned `state.preview_token` as `token` to `commit_face_preview`
or `cancel_face_preview` to guard against another client replacing the draft.
`smoothing_groups` replaces memberships; `[]` clears them. `smoothing_add` and
`smoothing_remove` preserve unrelated groups. `set_view` shares `surface` values
`textured`, `flat`, `wireframe`, `lightmap_density` and `smoothing_groups`, with `smoothing_group`
from 1 to 32. The same options on `render_view` inspect a view without changing
the user's settings.

The repeatable workflow checks run through MCP and save fixtures, rendered
views where relevant, and a JSON report:

```sh
python -B scripts/validate_face_lighting.py --output /tmp/hammerr-lighting-check
python -B scripts/validate_material_replacement.py --output /tmp/hammerr-material-check
python -B scripts/validate_surface_group.py --output /tmp/hammerr-group-check
python -B scripts/validate_volume_selection.py --output /tmp/hammerr-volume-check
python -B scripts/validate_paint_selection.py --output /tmp/hammerr-paint-check
python -B scripts/validate_texture_strokes.py --output /tmp/hammerr-texture-check
python -B scripts/validate_texture_chain.py --output /tmp/hammerr-chain-check
python -B scripts/validate_map_statistics.py --output /tmp/hammerr-statistics-check
python -B scripts/validate_pointfiles.py --output /tmp/hammerr-pointfile-check
python -B scripts/validate_component_sources.py --output /tmp/hammerr-source-check
python -B scripts/validate_components.py --output /tmp/hammerr-component-check
python -B scripts/validate_component_topology.py --output /tmp/hammerr-topology-check
python -B scripts/validate_component_transforms.py --output /tmp/hammerr-transform-check
python -B scripts/validate_transform_display.py --output /tmp/hammerr-transform-display-check
python -B scripts/validate_component_partitions.py --output /tmp/hammerr-partition-check
python -B scripts/validate_roof_texture_attachment.py --output /tmp/hammerr-roof-texture-check
python -B scripts/validate_split_texture_attachment.py --output /tmp/hammerr-split-texture-check
python -B scripts/validate_weld_texture_attachment.py --output /tmp/hammerr-weld-texture-check
python -B scripts/validate_partition_picking.py --output /tmp/hammerr-partition-picking-check
python -B scripts/validate_saved_partitions.py --output /tmp/hammerr-saved-partitions-check
python -B scripts/validate_partition_material_metadata.py --output /tmp/hammerr-partition-material-metadata
python -B scripts/validate_partition_materials.py --output /tmp/hammerr-partition-materials
python -B scripts/validate_partition_textures.py --render --output /tmp/hammerr-partition-textures
python -B scripts/validate_adjusted_partition_ridges.py --output /tmp/hammerr-adjusted-ridges
python -B scripts/validate_boundary_partitions.py --output /tmp/hammerr-boundary-partitions
python -B scripts/validate_polygon_partitions.py --output /tmp/hammerr-polygon-partitions
python -B scripts/validate_material_favorites.py --output /tmp/hammerr-material-favorites
python -B scripts/validate_spawn_clearance.py --output /tmp/hammerr-spawn-clearance
python -B scripts/validate_partition_extrusion.py --output /tmp/hammerr-extrusion-check
python -B scripts/validate_partition_extrusion.py --adjusted --output /tmp/hammerr-adjusted-cut-check
python -B scripts/validate_texture_lock_modes.py --output /tmp/hammerr-texture-modes-check
python -B scripts/validate_brush_reflection.py --output /tmp/hammerr-reflection-check
python -B scripts/validate_brush_shear.py --output /tmp/hammerr-shear-check
python -B scripts/validate_precision_cycles.py --output /tmp/hammerr-precision-check
python -B scripts/validate_precision_cycles.py --operation scale --output /tmp/hammerr-scale-cycles
python -B scripts/validate_precision_cycles.py --operation rotate --output /tmp/hammerr-rotation-cycles
python -B scripts/validate_precision_cycles.py --operation reflection --output /tmp/hammerr-reflection-cycles
python -B scripts/validate_precision_cycles.py --operation shear --output /tmp/hammerr-shear-cycles
python -B scripts/validate_carving.py --output /tmp/hammerr-carving-check
python -B scripts/validate_carve_many.py --output /tmp/hammerr-carve-many-check
python -B scripts/validate_component_picking.py --output /tmp/hammerr-component-picking-check
python -B scripts/validate_geometry_tools.py --output /tmp/hammerr-geometry-tools-check
python -B scripts/validate_hollow.py --output /tmp/hammerr-hollow-check
python -B scripts/validate_compile_jobs.py --output /tmp/hammerr-compile-check
python -B scripts/validate_origin_brushes.py --output /tmp/hammerr-origin-check
python -B scripts/validate_cubemap_samples.py --output /tmp/hammerr-cubemap-check
python -B scripts/validate_visibility.py --output /tmp/hammerr-visibility-check
python -B scripts/validate_water_metadata.py --output /tmp/hammerr-water-check
python -B scripts/validate_sky_visibility.py --output /tmp/hammerr-sky-check
python -B scripts/validate_saved_views.py --output /tmp/hammerr-views-check
python -B scripts/validate_view_modes.py --output /tmp/hammerr-modes-check
```

Use a new output directory for each run. Add `--url http://127.0.0.1:7377/mcp`
to check an idle GUI session with a clean map and empty undo/redo history.
Each script temporarily opens its fixture, then restores the original file,
camera, selection and view.

The audit, component source/topology/transform, partition, precision, carving,
favorites, spawn-clearance, origin-brush, cubemap-sample, visibility, water-metadata and sky-visibility validators
use private headless sessions and do not accept a GUI URL. Add `--render` to the topology, boundary or polygon
validator for offscreen GPU pixel checks without opening a desktop window.

Check extrusion guide visibility with a private offscreen GPU session:

```sh
python -B scripts/validate_partition_visibility.py --output /tmp/hammerr-partition-visibility
```

`get_component_sources` pages the selected owners' committed brushes without
expanding full topology. It returns ID, owner, material, bounds, total, active
scope and `snapshot`; `offset` defaults to 0 and `limit` to 100 (1–256).
Capture one to sixteen returned IDs with `set_component_picking`. Source pages
stay unchanged during previews and do not alter map history.
`update_component_scope` takes numeric `solid`, `kind`, `action` (`replace`, `add`
or `remove`) and the current `snapshot`. It builds page-spanning subsets through
the same Session operation as the dialog. Duplicate additions with the same kind
preserve valid drafts; invalid/stale/over-limit requests preserve state. Removing
the final member clears picking, markers and drafts without map edits.

Component queries return full stored precision. Use each response's `snapshot`
when selecting components or previewing a move, and the preview's `token` for
Apply/Cancel. One side ID selects a whole face, two incident side IDs identify an edge,
and three or more identify a vertex. Vertex-array indices describe that response only. `render_view` and the
human viewport show the same draft. Map edits and competing geometry tools
invalidate it. Camera and map coordinates in MCP responses retain their full
precision, so reusing a reported camera preserves its orientation precisely.
Set `texture_lock: true` in `preview_component_move` to keep texture coordinates
at corresponding face corners. The default is world anchoring. The response
reports `texture_mode` and the preview's `texture_lock` setting; incompatible
deformations leave the previous valid draft intact.

Repeated edit/save/reopen checks cover fractional translations, positive scaling
and rotation of thin, oblique and large-coordinate brushes, with world/entity
ownership and attached corner UVs. The installed SDK test samples 845 brushes
from 217 maps. Drift must stay below `1e-5` world units and `1e-4` texels; undo
and redo must be exact. Affine cycles use a fixed world pivot and mathematically
inverted compound rotations. `validate_precision_cycles.py` checks translation
by default; `--operation scale` or `--operation rotate` checks those shared
operations through private headless MCP sessions. Round trips through other
editors remain unverified. These tolerances describe editable VMF geometry;
BSP uses float32 values and compiler plane snapping. A rotated thin-slope fixture
also compiles inside a sealed room; exact export/runtime precision is a separate
validation requirement.

`preview_component_scale` accepts positive XYZ `factors`;
`preview_component_rotate` accepts XYZ `degrees`. Both require the latest
component `snapshot` and accept optional world `pivot` and `texture_lock`.
`get_brush_components.component_center` reports the source selection's default
pivot; `preview.transform` reports exact parameters and the resolved pivot.
Use `commit_component_edit` or `cancel_component_edit` with the returned token.

`preview_component_weld` uses the current selected vertices and `snapshot`.
Omit target inputs for a per-brush mean, pass `target: [x, y, z]` for a fixed
position, or `target_corner: {solid, sides}` for a source corner. The two target
inputs are mutually exclusive. `get_brush_components.weld_targets` lists
original corner choices and `weld_center` gives their selected center;
`preview.weld_target` records the chosen mode.
With no target inputs, each brush welds to its own average position.
Textures remain world-anchored by default. Optional `texture_lock: true`
preserves surviving corner UVs and averages merged contributors separately on
each face; the shared report uses `texture_mode: "weld_attached"`.
The response reports `preview.operation`, merged positions and removed side IDs
in `preview.weld`. `selection` identifies the resulting corners;
`source_selection` retains the original inputs while the draft is open.
Use `commit_component_edit` or `cancel_component_edit` with its exact token.
Selection changes and handles remain unavailable until Apply or Cancel.

`preview_component_split` accepts `rise` and the latest component `snapshot`.
Select one face or edge per brush first. `preview.split` reports inserted
positions, source/new side IDs and each generated face's source. New faces
inherit world texture projections by default. Optional `texture_lock: true`
preserves each triangle's original corner UVs and its flat center/midpoint UV.
`preview.texture_lock` and `texture_mode` expose the chosen policy.
Apply verifies allocation and switches
active picking to vertices in the same captured brush scope. Discard preserves
the original picking kind and selection. Both use the generic component
commit/cancel calls.

`preview_component_crease` accepts `pairs: [{solid, edges: [neighbor_side,
neighbor_side]}]`, `rise` and the latest `snapshot`. Select one quad face per
brush first; `get_brush_components.crease_choices` lists the two opposing pairs
and source midpoints for each selected face. Every face needs one pair, with
independent directions supported in a batch. `preview.crease` reports source/new
side IDs, raised positions and resulting corner selections. Both old corners
and world projections stay fixed. Nonplanar or concave results reject before
allocation. Apply replays and verifies generated IDs through
`commit_component_edit`; discard uses `cancel_component_edit`.

Optional `roof_texture_lock: true` preserves corresponding UVs on the two roof
faces. Neighboring side projections remain unchanged. Direct crease defaults
to world projection; `raise_component_partition` retains its current policy
when the field is omitted, including after returning to flat.
`preview.crease.roof_texture_lock` and `preview.partition.roof_texture_lock`
report the setting; raised attached drafts report `texture_mode: "roof_attached"`.

`preview_component_partition` accepts `pairs` and the latest `snapshot`.
`get_brush_components.partition_choices` lists every boundary-edge pair for
three-to-eight-corner source faces, with up to 28 choices. Quads retain their two
opposing pairs first, followed by four adjacent pairs. Opposing quad cuts make
two quads; adjacent quad cuts make a triangle and pentagon. Flat draft topology
is returned under `preview.partition`.
Vertices retain source corner/edge incidence and exact source positions;
`preview_position` includes the current ridge height. Loops have independent
IDs **0** and **1** within each brush and inherit `source_side`. Each loop reports
its `shape` and three to nine vertex refs. `preview.partition.ridge_supported`
reports the two-quad shape requirement; positive heights still validate planarity
and convexity. Adjacent quad cuts keep the trimmed corner
as triangle **0**, including after reflection and saved reopening.
`select_component_partitions` takes `partitions: [{solid, partition}]`, a token
and optional replace/add/remove/toggle mode for highlighting and framing.
`frame_component_partitions` accepts that token and optional `aspect`.
`pick_component_partitions` takes the same token and viewport `x`, `y`, optional
size and camera. It returns visible loop refs and world hit positions through
the same query as a human click. Boundary hits may contain both pieces in stable
order; pass a ref and the unchanged token to `select_component_partitions`.
`raise_component_partition` takes the current token and `rise`; zero restores
flat topology, otherwise use 0.125–4096 units. Successful height changes return
a new token. Commit raised geometry with `commit_component_edit` or discard
with `cancel_component_edit`. Flat commit and save/compile/play are blocked
while this temporary draft remains pending.

`store_component_partition` takes the current flat draft `token`, keeps its
bounded editor metadata and closes the draft without changing brush planes.
`get_saved_component_partitions` returns `entries`, per-source `valid`/`error`,
metadata limits and the latest `snapshot`. `resume_component_partitions` and
`remove_saved_component_partitions` take `sources: [{solid, side}]` and that
snapshot. Resume supports one source per brush across 1–16 brushes; remove
supports 1–64 saved records, including stale sources. Piece identities survive
affine edits and reflection. Saved intents leave native BSP output unchanged.
The map-wide registry allows 64 faces and 64 KiB of key/value text. Unknown
metadata is preserved; unsupported versions are reported and cannot be
overwritten by these operations.

`update_component_partition_cut` accepts the current flat draft `token` and
`edge_parameters: [first, second]`, two finite values strictly between 0 and 1.
It applies positions to every captured source brush and returns a replacement
token, preserving highlights and saved piece identity. Edges follow sorted
neighboring side IDs; each endpoint runs between the sorted corner incidence
lists reported in vertex provenance. Invalid or collapsed cuts retain the draft.
Return to flat before adjusting a raised/extruded preview. Save positions with
Keep, extrude, or raise the adjusted pieces. Positive ridge heights use the
actual endpoints and require two planar, convex quad roofs. Cuts without two
quad pieces can be kept and extruded. Nonplanar cuts are
rejected atomically; `rise: 0` retains their flat topology.
`preview.partition.edge_parameters` reports shared values or null for mixed
sources; saved inspection lists each source's positions. Existing midpoint
records remain compatible, and adjusted metadata retains the same global budgets.

`update_component_partition_material` takes the current flat draft `token`,
`partitions: [{solid, partition}]` and an optional `material`. Set 1–32 distinct
pieces across the captured source brushes; null or omission restores source
inheritance. Names are normalized and share the saved metadata budget. Successful
updates replace the token; invalid or stale updates retain the draft. Each loop
reports `top_material`, `effective_material` and `inherited`; saved inspection
reports both `top_materials` and `effective_materials`. Keep persists the plan,
Cancel discards it, and geometry Apply replays the corresponding roof/top choices.
Tool filters and picking follow each piece's effective material.

`update_component_partition_texture` takes the same token and piece refs, plus
`texture: {shift: [u,v], scale: [u,v], rotation: degrees}`. Shifts are texels;
scales are world units per texel and may be negative to mirror the texture.
Rotation turns the inherited source axes by the difference from the source
rotation. Null or omission restores source placement. Each loop reports
`texture_plan`, `effective_texture` and `texture_inherited`; saved inspection
includes `top_textures`. Keep, reopening, reflected piece identity, Cancel and
geometry Apply follow the same material-plan lifecycle. All five values must
be finite within ±65536, with scale magnitude at least 1e-8. Raised roofs and
prism tops use the planned placement, optionally attached at corresponding
corners. Bases, walls, lightmap scale and smoothing keep their own settings.

`preview_partition_extrusion` takes the current flat draft `token`, positive
`height` (0.125–4096), optional `wall_material` and optional `texture_lock`
(default false). It extrudes the 1–32 highlighted pieces across up to 16 source
brushes. Updates replace the token and rebuild from the original source; invalid
updates retain the previous draft. Tops use their planned material or source
inheritance; bases retain the original source face material. Outer
walls inherit neighboring faces, and the divider inherits the source face.
All boundary and divider walls receive fresh world UVs and the optional material
override.
Attached top UVs preserve corresponding corners, reported as `top_attached`.

`preview.extrusion` lists every output solid, source piece, owner, bounds and
face role, source side, plane, material and UV projection. Framing includes the
prisms; visible body picking returns the original piece ref. After highlights
change, preview again before Apply. `commit_component_edit` checks exact replay,
adds the prisms in one undo step and selects only their faces. Original brushes
and saved intents remain unchanged. Return to flat with
`raise_component_partition` and zero rise, or use `cancel_component_edit`.
Inward/subtractive extrusion, free-edge partitions and per-piece explicit
projection axes, fit and justification remain unfinished.

`get_carve_preview` exposes candidates, limits and the latest `stamp`.
`preview_carve` takes numeric `cutter` and `targets` brush IDs, including children
of brush entities. Its `preview.result` lists changed fragments, fully removed
and unchanged brushes, and face inheritance. `commit_carve` or `cancel_carve`
requires the exact `preview.token`. Invalid updates retain the valid draft;
source changes and competing geometry tools invalidate it.

`preview_carve_many` accepts a bounded `cutters` list, `targets` and the same
`stamp`. `preview.result.cutters` reports their normalized order, and face
inheritance names the actual cutter for every generated face. Single and
multiple cutters share `commit_carve`/`cancel_carve` and one-step undo.

For component picking, `set_component_picking` captures the brush IDs and kind;
`get_brush_components` reports that shared mode. `pick_brush_components` accepts
`kind`, viewport `x`/`y`, image `width`/`height`, an optional camera and a 1–32 pixel
radius (default 10). Coordinates start at the top-left; a pixel center is
`column + 0.5, row + 0.5`. It returns up to 64 candidates ordered by screen
distance, depth and stable IDs, plus the snapshot for `select_brush_components`.
Queries do not change selection. Edges are clipped at the near plane and use
perspective-correct world positions. Use `enabled: false` on
`set_component_picking` to exit the mode and clear its draft and guides.

`get_component_handles` exposes the same world/pixel paths and hit tests as the
viewport. Configure snapping and texture attachment with
`set_component_move_options`, then `begin_component_drag` with a handle and the
latest component snapshot. Updates use its `drag.token` and either viewport
pointers or a total two-value `offset`. Axis handles require a zero second
value; plane handles use world axes and `view` uses captured screen right/up.
Commit or cancel that exact drag token. Invalid geometry preserves the preceding
valid draft; camera, selection, source and competing-tool changes invalidate
the gesture. A numeric preview can also become the starting offset for a drag.

For hollowing, get the current `stamp` from `get_hollow_preview`, then pass it
to `preview_hollow` with thickness, direction and optional object IDs. The result
lists planned wall IDs and cavity centers/bounds. Inspect the rendered draft,
optionally use `look_inside_hollow`, then apply or cancel its exact token.
Map/selection edits and competing geometry tools invalidate the draft.

The state also lists source `faces` with IDs, normals, centers and materials.
Pass selected `{solid, side}` references as `open_faces` to leave those faces
open; omit the array for a closed shell. Invalid openings retain the last valid
preview. A shell with openings needs other world geometry to seal the map.

`clip_brushes` accepts a plane normal/distance, three points, or a snapped
planar line. Use `preview: true` to inspect the shared cut before committing
or cancelling it. `get_clip_preview` reports the resulting brushes and faces;
`render_view` and picking use the same preview geometry as the viewport.

For interactive three-point cuts, pass zero to three world coordinates to
`set_clip_points` with the latest `get_clip_preview.snapshot`. Alternatively,
start with an empty point array, then call `pick_clip_point` with viewport pixels
and that snapshot. It picks committed brush surfaces and applies the shared snap
setting. The third point previews the plane. Revise the array to remove or replace
points; existing clip commit/cancel calls finish the draft.

`get_clip_handles` returns the same world/pixel paths and hit tests as the
viewport. Set `move`, `rotate` or `off` with `set_clip_handle_options`, then
`begin_clip_drag` with the latest clipping snapshot. Pass its `drag.token` to
`update_clip_drag`, using a viewport pointer or total `amount: [a, b]` since begin:
world units for movement, degrees for rotation. Axis/ring handles require `b: 0`.
`finish_clip_drag` leaves a preview for inspection and explicit
`commit_clip_preview`; `cancel_clip_drag` restores the plane before positioning.
Invalid updates keep the last valid preview and block finishing. Camera, view
and handle-option changes cancel positioning; source changes discard the cut.

`transform_objects` accepts `space: "world"` or `"local"` and `preview: true`.
Handle tools support numeric amounts and viewport pointers, with updates
measured from the gesture's start. `get_transform_handles` returns the rendered
paths in world coordinates and pixels, and can hit-test a pixel. Shared drafts
can be inspected, applied or cancelled from the editor or MCP.

`select_area` takes pixel corners, viewport dimensions and an optional camera
override, plus `contained`/`intersecting` and selection rules. Use `preview:
true` to inspect candidates before applying. Camera overrides leave the user's
view unchanged; face mode returns stable solid/side IDs.

`select_volume` takes world-space `mins`/`maxs`, `selection_mode` (`objects`
or `faces`), a match rule and a combination mode. Use `preview: true`, inspect
the paginated matches, then pass the returned preview token to commit, cancel
or frame. Camera changes preserve the box; map, selection and visibility
changes invalidate its candidates.

`begin_paint_selection` takes a `pointer` with `position`, `width` and `height`,
an optional camera, radius, selection mode and combination rule. Extend its
path with `update_paint_selection` and ordered pixel `points`. Inspect the
returned token, paginated matches and work limits, then commit or cancel.
Camera, selection, map and visibility changes invalidate the stroke. Invalid
or over-budget updates preserve the previous preview. A stroke accepts up to
256 points, 32,768 new samples and 1,048,576 pixel visits per update, with
262,144 samples total.
Selection strokes add no map undo step.

`sample_face_texture` captures a source for `begin_texture_stroke`. Use `mode: "material"` or `"projection"`, then send pixel points to `update_texture_stroke`.
Inspect `get_texture_stroke` and `render_view`; commit or cancel with the token.
Invalid updates preserve the previous draft. `set_paint_tool` accepts
`operation` to choose selection, material or projection in the shared toolbar.

`save_camera_view` stores the current shared camera under `name`; `replace_id`
updates an existing view. Pass both `generation` and `revision` from
`get_camera_views` to guard saved-view actions against map changes. Restore
moves the camera without changing selection, map data or edit history.

`begin_connection_edit` starts an addition or edits an outgoing `index`.
`update_connection_draft` accepts readable fields; `delay` and `times` are
strings so unfinished values can be shared with the UI. Inspect feedback,
then commit or cancel. Pass the draft's `token` to guard against another
operation replacing the form. Starting an edit or deleting supports optional
`generation` and `revision` guards from `get_entity_connections`.

`get_entity_references` includes proven links and untyped text candidates.
Begin a rename, update `new_name` and optionally `rename_group`, then inspect
the returned preview. If `requires_review` is true, acknowledge it with
`accept_review: true` in a subsequent update. Commit or cancel with the draft's
`token`; starting a rename accepts generation/revision guards from inspection.
Preview rows include the same readable `display` labels and explanations used
by the UI. `show_details` controls the shared engine-details toggle.

`begin_property_edit` takes `world: true` or entity `ids` (selection by
default). Update the returned `token` with a `changes` list; each operation
uses `action: "set"`, `"remove"`, `"default"`, `"flag"` or `"component"`.
Pass inspection `generation`/`revision` guards when starting from an earlier
report. Inspect the preview, then commit or cancel. Updates replace the
pending list;
failed validation keeps the previous draft. `get_property_inspector` reports
stored/default values per target and mixed flags.

`get_object_report` accepts a `filter` with `query`, `classname`, `targetname`,
`material`, `kind`, `hidden` and `within`. `select_object_report` uses the same
filter across all pages, with optional `frame: true`. `set_object_visibility`
accepts `hide`, `show`, `isolate`, `show_all` or `restore`. Its result includes
hidden counts and the compiler's visibility policy.

</details>

## Compile and play

| Control | Behavior |
| --- | --- |
| **Compile** | Build the current snapshot, including unsaved edits, into a BSP beside the VMF. Unsaved maps prompt for an output filename. |
| **Run Map** | Compile and launch the snapshot in the installed Linux CS:S client with `-insecure`. |
| **Build details…** | Choose lighting and physics bodies; inspect stages, elapsed time, snapshot freshness and diagnostics. |
| **Play built map** | Launch the last successful build from Build details without compiling again. |
| **Cancel build** | Stop the active build while preserving previous output. |
| **Game Log** | Read console and launcher output. |
| **Stop Map** | Stop the game process started by this editor. |
| **Leak path** | Toggle the red leak path drawn through geometry after a failed compile. |

Compilation reports stages as it runs in the background, including visible and
audible space. Structural world leaves form separate visibility clusters;
portal-facing floods produce conservative visible sets, and audible sets union
the visible neighbors. **Build details…** and MCP compile statistics report
`clusters`, `visibility_portals` and `visibility_bytes`. Detail brushes and
brush-model leaves do not create world visibility clusters. Tight portal-flow
clipping remains unfinished; the current bounds favor retaining visible geometry.
Sky visibility follows compiled world sky surfaces and the visible sets.
**tools/toolsskybox** marks 3D sky; materials with `%compile2dsky` mark 2D sky.
Enclosed spaces and disconnected rooms do not inherit unrelated sky, while
portal-visible sky propagates from the original surface clusters. Hidden and
brush-model sky do not seed world flags. **Build details…** and MCP statistics
report `sky_clusters`; the compiler sets no radial-culling flag.
Visibility work and native lump limits fail with diagnostics and retain previous
output. **Cancel build** waits for the worker to stop; cancelled builds keep the previous BSP and pointfile.
Output is published atomically, and a build cannot be cancelled after publication.
Closing Build details leaves the build running.

**Build lighting** offers **Fullbright** (the default, for fast geometry checks)
and **Direct LDR**. Direct LDR bakes unnamed, static **light** and **light_spot** entities into native
world-face lightmaps, with colored light, constant/linear/quadratic attenuation
and opaque world/detail brush shadows. Border samples clamp to the actual face;
fractional, rotated and mirrored texture projections retain their lightmap axes.
Build details and MCP results report baked faces, samples, light sources and bytes.
Spotlights support pitch/yaw or a visible compiled target, inner/outer cones up
to 90 degrees and focus falloff. Viewport helpers share the direction logic.
Matching 180-degree cones become point lights; invalid cones and target
references retain previous output.

World/detail brush shadows use a bounded spatial index and exact convex-hull
tests. A 70-brush fixture produces the same BSP while using 70% fewer lighting
work units. Large diagonal rays can still visit many candidates.

Direct LDR limits lighting to 16 MiB, 8,192 light sources and bounded CPU work.
Named/animated lights, percentage-distance falloff and invalid values fail with
object diagnostics and preserve the previous BSP. Other light types, transparent
shadows, smoothed/bump normals, bounced light, sky/ambient cubes, HDR, props,
terrain and brush-model lighting remain unfinished. A private headless CS:S check loaded and
rendered the red-light fixture with Fullbright off; playable collision and
production lighting still need validation.

**Physics bodies** offers **No physics bodies** (the default) and **CS:S engine
physics**. Engine physics runs the verified installed Linux64 collision library
in the bundled `hammerr-physics` worker. It emits checked world, player/NPC clip
and brush-model bodies, preserving local pivots. Build details and MCP report
the body, hull, model and byte counts. Keep `hammerr-physics` beside the editor or
standalone compiler; the quick-start build produces all three executables.

The worker checks its library build and generated geometry, bounds memory/work,
and stops on cancellation or after 30 seconds. Missing or unverified libraries
fail explicitly and retain previous output. Editing works without this backend.
Bodies currently use default surface properties; brush models use a one-kilogram
mass fallback. Horizontal structural-world liquids also receive clipped fluid
bodies with native surface planes and damping defaults. Material friction/density,
authored masses and detail/model liquids remain unfinished. World terrain receives
native triangle-soup contact bodies when engine physics is selected; static
props use the collision assets supplied with their models.
Private CS:S checks establish world body contact;
one live player spawn/teleport remains grounded in both modes, while a physics
crate falls through the map without bodies and rests on the floor with engine
physics. Sliding and rotating doors, a continuously rotating brush, fractional
pivots and trigger outputs also pass an MCP-authored CS:S fixture. Contact with
moving bodies and broader collision still need validation.

Compilation requires finite brush geometry and entity origins within
**±16,384 units**. Out-of-range objects produce diagnostics before BSP
construction and leave existing output intact. Editor tool bounds and camera
zoom have separate limits; they do not extend the playable Source world.

Material compile flags recognize `1` and case-insensitive `true`; `0` and
`false` disable them in both the editor and compiler. A skip face hides that
surface while preserving the brush's other contents. Non-solid materials retain
visible geometry without sealing the map. Unsupported flags and omitted hint,
areaportal or empty-content brushes are reported in build diagnostics.

Brush entities retain explicit **origin** coordinates. To define a pivot with
geometry, put one visible **tools/toolsorigin** brush in the same entity as its
body; the volume’s bounds center overrides the explicit origin. The compiler
consumes that volume and writes the remaining model in local coordinates while
preserving world-space texture and lightmap placement. The source VMF stays
unchanged. Duplicate pivots, world/func_detail origin brushes, origin-only
entities and local model bounds outside ±16,384 units fail with object
diagnostics and preserve previous output. Hidden origin volumes are ignored.

Visible **env_cubemap** helpers now compile into native sample records. Use the
entity inspector or MCP entity properties to choose **Cubemap Size**: game
default or 1–256 pixels. **Build details…**, compile status and MCP results report
the sample count. The compiler accepts up to 1,024 visible samples and rejects
invalid sizes, out-of-range origins or brush geometry on the point helper.
Hidden helpers are omitted. Reflection material binding, manual face lists and
texture capture remain unfinished and appear in build diagnostics; sample
records alone do not provide baked reflections.

World **water volumes** now compile native leaf regions and surface fog IDs.
Create a world brush with **tools/toolsnodraw** on its sides and a water material
on its horizontal top through the face inspector or MCP `edit_faces`. Connected
water with the same surface height and material shares the lowest depth;
different heights and materials remain separate. **Build details…** and MCP
statistics report `water_regions` and `water_leaves`. The compiler supplies a
native **water_lod_control** with distances 1,000 and 2,000 when none is authored;
existing controls are preserved, and the source document stays unchanged.

Sloped or enclosed tops and incompatible overlapping liquids fail with object
diagnostics and preserve previous output. Hidden volumes are omitted; detail
and brush-model liquids report their unsupported region metadata. With **CS:S
engine physics** selected, compiled wet-leaf hulls form fluid bodies cut against
solid floors/walls; connected compatible regions share a compound body. Shared
build results report `physics_fluids`. Private CS:S checks verify a crate rising
and floating in the water. Depth-specific material patches, below-water materials,
currents, authored damping and broader rendering/movement remain unfinished.

**Static props** compile model dictionaries, fractional transforms, collision
choices, skins, fading and lighting-origin references into native records.
Place a **prop_static** through the entity browser or MCP `create_entity`, then
edit its model and properties with the same inspector/session operations.
Build details and MCP report prop, model and leaf-reference counts. Supported
MDL versions are 44 and 48; models need the `$staticprop` flag. **Use VPhysics**
also requires a matching, checked `.phy` asset. Invalid assets, unsupported
scaling/lightmap requests and ambiguous lighting origins retain previous output.
Leaf membership uses conservative model-header bounds; baked prop lighting,
prop shadows and exact hull clipping remain unfinished.
Static-prop asset collision works in either physics build mode; embedded
brush/fluid body counts are reported separately.

Imported **world displacements** compile native power-2/3/4 surface grids, alpha
weights, triangle tags and face/leaf references, with matching UI/MCP build
counts. Their parent volumes do not seal the map or become solid boxes. Engine
physics adds checked terrain contact meshes and reports `physics_mesh_triangles`.
Native no-physics, no-hull and no-ray flags are retained. Invalid grids, unsupported
entity/detail terrain, subdivision and triangle removal retain previous output.
Baked terrain lighting and neighbor sewing remain unfinished; these surfaces
currently render without lightmaps. Terrain creation/editing controls remain
roadmap work; use existing terrain in imported VMFs.

Native BSP inspection also validates terrain lightmap sample streams, triangle
references and allocation bounds. Shared triangle ordering and recovered surface
samples provide the next terrain-lighting prerequisite.

**Play built map** shows the BSP it will launch. It becomes available after a
successful compile and again after the managed game stops. Map changes, missing
output, changed build settings, an unavailable Linux client or an active build/game disable it with an
explanation. A failed rebuild retains the previous successful build when that
snapshot is still current. Launches always include `-insecure`.

Through MCP, `start_compile` returns immediately with a job token. Poll
`get_compile_status`; pass that token to `cancel_compile`. The existing
`compile_map` and `run_map` calls wait for completion and expose the same status.
A map edit marks the build snapshot stale and prevents launching it.
`get_compile_settings` lists modes and limitations. Use `set_compile_settings`
with `{"lighting":"direct_ldr"}` or `{"lighting":"fullbright"}` before compiling.
Add `"physics":"engine"` or `"physics":"none"` to the same call, or update just
that field. Omitted fields retain their values; invalid combinations leave all
settings unchanged.
Settings apply to all build paths without editing the VMF or undo history; an
active job freezes them. A previous build stays stale while its mode differs.
`get_compile_status.launch` reports readiness, the BSP path or the reason play
is unavailable; `launch_game` performs the same launch as **Play built map**.

![Build lighting choice, baked sample counts, stage, output and diagnostics](docs/images/build-status.png)

Maps need sealed world geometry and
at least one point entity in open space. A leak produces a `.lin` pointfile
and preserves any existing BSP. Editing hides the previous leak path until
the next compilation.

Each game run uses a uniquely named BSP copy and its own logs. The copy is
removed when the game exits; logs remain until the next run or editor
shutdown. Closing the editor stops its managed game process. Existing maps
and unrelated game processes remain untouched.

<details>
<summary>Standalone compiler and room example</summary>

```sh
cargo run --release -p compiler --bin hammerr-compile -- \
  map.vmf --game /path/to/cstrike/gameinfo.txt
```

Use `-o /path/to/map.bsp` to choose the output and `--lighting direct_ldr`
to bake supported point/spot lights. Add `--physics engine` for native brush bodies;
omit these flags for Fullbright and no physics bodies. To generate a sealed room
with both player spawns:

```sh
cargo run -p compiler --example room -- room.vmf
```

After compiling, copy the BSP to `cstrike/maps/` and run `map room` in the
CS:S console. The room has been compiled and loaded in Linux CS:S with a
player spawned inside and fullbright rendering enabled by the engine.

</details>

## Status

**The compiler is experimental.** It produces BSP v20 maps with conservative
portal-based visible/audible sets and a shared Fullbright/Direct LDR choice.
Direct LDR emits bounded static point/spot samples and world-light records;
production lighting and tight portal clipping remain unfinished. The expanded
visibility and lighting pipelines need CS:S runtime validation.

Authored brush planes and axial/non-axial collision bevels use checked support.
Snapping and approximate plane reuse must preserve whole authored faces and
supporting boundaries in working geometry and emitted BSP float records.
Native sloped, near-axis, distant, reflected and thin-brush fixtures verify
surface preservation and swept hulls against an independent geometric reference.
Optional engine physics now emits bounded world/clip and brush-model records.
Private CS:S checks verify live player grounding in both modes and a physics
crate resting on native world bodies. Door opening/closing, rotating pivots and
trigger outputs pass; moving-body contact and full collision remain unfinished.

<details>
<summary>Headless physics backend probe</summary>

```sh
python3 -B scripts/validate_vphysics_backend.py \
  --game-dir "/path/to/Counter-Strike Source"
```

This developer check verifies the inspected CS:S Linux64
**VPhysicsCollision007** library: convex construction, guarded serialization,
compound geometry/tags and length-prefixed solid loading. It runs in a timed
child with a 24 GiB address-space limit and rejects unrecognized library builds.
It does not launch the game. The compiler's optional **CS:S engine physics** mode
uses an isolated Rust worker to construct and verify owned brush bodies; the
probe remains a developer check of the underlying installed library.

The BSP reader/writer now checks native physics model/solid lengths, key data,
model references and the end marker, with exact round trips across all 21
installed maps. Unknown VPHY types remain opaque. Checked outer records do not
make arbitrary compact-body internals safe for the engine loader.

With **Cage** and **grim** installed, the runtime check builds its fixtures through
MCP and runs CS:S with `-insecure` on a private headless display:

```sh
systemd-run --user --scope -p MemoryMax=24G -p MemorySwapMax=0 \
  python3 -B scripts/validate_native_runtime.py
```

It checks a single player spawn/teleport, baked red lightmaps and a physics
crate with and without native bodies. Maps, config writes, scripts and logs use
private directories; the check sends no mouse or keyboard events to the desktop.
Add `--fluid --output /tmp/hammerr-fluid-check` to include a pool and verify
buoyancy. `scripts/validate_water_metadata.py --physics engine` checks native
fluid records and both MCP build paths without launching the game.
`scripts/validate_brush_runtime.py` checks doors, rotating brushes and trigger
connections in the same private display. Direction properties such as Move
direction accept Pitch/Yaw/Roll through both the inspector and MCP.
`scripts/validate_static_props.py` checks native props through both MCP build
paths; add `--runtime` to verify their collision choices in private CS:S.
`scripts/validate_terrain.py --runtime` checks imported terrain through both MCP
build paths, asymmetric heights, player hulls, ray flags and dynamic contact.
`scripts/validate_spotlights.py` checks native cones, target aiming and both MCP
build paths, including hidden lights and rejected output preservation.
`scripts/validate_shadow_index.py` checks a dense mixed-light fixture through
both MCP paths; `--reference` compares its BSP and work against an earlier run.

</details>

Tighter VVIS portal flow, VRAD, broader physics collision, terrain lighting/sewing,
production static-prop lighting, areaportals and overlays remain unfinished. Horizontal world water
now has native leaf/surface metadata and optional fluid bodies with verified crate
buoyancy; depth/below-water material handling and broader rendering remain pending. Cubemap samples
compile, while reflection binding/capture and lighting-origin references remain
pending. Brush-entity origin volumes and explicit pivots compile and pass basic
door, rotation and trigger runtime checks. Other brush behaviors and moving-body
contact remain pending. Unsupported geometry is reported
in diagnostics. The BSP reader/writer is verified against stock CS:S maps,
including bounded native LDR/HDR lighting ranges, styles, bump maps and
world-light records, static-prop layouts and displacement grids/neighbors.
Displacement geometry records round trip exactly across stock maps; world grids
and optional native contact meshes now compile. A tested CPU reference recovers projected luxels and
point-light/brush-shadow contributions and drives the optional Direct LDR bake.

Editor work ahead includes arbitrary flat partitions, inward extrusion,
per-piece UV editing, displacement clipping and authoring, model rendering and broader
map checks.

Saved flat pieces support independent planned top materials through shared
editor/MCP drafts, grouped material browsing and flat previews. Keep stores
bounded editor intent; raising and extrusion apply the plan to real roof or
top faces. Bases and walls retain their own material policies.

## Project layout

| Crate | Responsibility |
| --- | --- |
| `source-formats` | Valve file formats and the game filesystem. |
| `map-core` | Geometry, displacements, undoable documents, edits and picking. |
| `render` | wgpu renderer, materials, camera and offscreen snapshots. |
| `editor` | Slint UI, shared Session and MCP server. |
| `compiler` | Native VMF-to-BSP compilation and the `hammerr-compile` CLI. |

Counter-Strike: Source and its assets are not included.
