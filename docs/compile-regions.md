[← Manual](README.md)

# Compile regions

Build a sealed part of a brush map while keeping the full source editable.
The compiler adds six black solid boundary slabs to its temporary input; it never
inserts them into your map.

## Choose a region

1. Click **Compile region…** on the toolbar. An enabled region changes this
   label to **Region build…**; invalid active metadata shows **Region error…**.
2. Enter **Minimum** and **Maximum** X, Y and **Z · height** corners, or click
   **Use selection / map bounds**. Check **Enable region for builds**.
3. Click **Preview region**. Orange lines show the bounds while all source
   geometry stays visible. **Frame region** moves the editor camera to them.
4. Review the corners, then click **Apply region**. This is one Undo step.
   **Cancel**, Escape or closing the window discards only the draft you own.
5. Click **Compile** or **Run Map**. Open **Build details…** to inspect
   **Compile scope at build start** and the inclusion warning. A running build
   keeps its original bounds if you edit the region afterward.
6. Save the VMF to retain its native cordon metadata.

![Compile-region corners, preview and Apply controls](images/compile-region.png)

![Orange region bounds with full source geometry still visible](images/compile-region-preview.png)

**Review current draft** loads the current shared draft, including one created
by an agent. Applying or cancelling it requires explicit review; a replacement
draft cannot be cancelled by an older window token. Any map edit makes a
pending draft stale. Preview it again before applying.

## Include a water volume

1. Create [world water](compile-and-play.md#water) with a flat visible top face.
2. Include the entire water brush between **Minimum** and **Maximum**. Its
   sides may meet the region boundary, but cannot cross it. Click
   **Preview region**, then **Apply region**.
3. For floating objects, open **Build settings…** and choose
   **CS:S engine physics** under **Physics bodies**, then **Compile**.
4. A crossing-water diagnostic leaves the previous BSP intact. Enlarge the
   region, move the whole volume inside, or disable the region before rebuilding.

## Return to full-map builds

1. Open **Region build…**, uncheck **Enable region for builds**, then click
   **Preview region** and **Apply region**. The bounds remain saved.
2. To remove the saved metadata instead, click **Preview clear**, review the
   explanation, then **Apply region**. **Cancel** keeps it.
3. Compile again with the region disabled to check full-map leaks.

## Include terrain

1. Create [terrain](terrain.md) as world or detail geometry.
2. Open **Compile region…** / **Region build…**, then click
   **Use selection / map bounds** or set **Minimum** and **Maximum** manually.
   Suggested terrain bounds include its parent cage and displaced sheet.
   Leave enough room for player spawns, lights and other objects to retain.
3. Click **Preview region**, review the orange box, then **Apply region** and
   **Compile**. Use **CS:S engine physics** for native terrain collision.
4. A crossing-terrain diagnostic keeps the previous output. Include the entire
   cage and grid, move the whole patch, or disable the region. Terrain is never
   clipped or flattened to fit.

## Inclusion and limits

- One axis-aligned region, at least 16 units wide on every axis. Corners must
  stay within ±16,368 units so the exterior 16-unit boundary fits Source bounds.
- Intersecting world and detail brushes stay whole. Runtime brush entities
  stay whole when their resolved pivot is inside; origin brushes take priority
  over explicit origins. Point entities use their origins. Originless global
  entities, the sun and water LOD controllers remain. Sun and water-control
  markers keep their positions and settings even outside the region.
  Hidden objects remain excluded.
- References are retained unchanged. Targets outside the region may be absent.
  The solid boundary can hide leaks, block views and change light or sound;
  region output is for partial-map work.
- Fully contained structural world water retains native water leaves and engine
  fluid bodies. The compiler's existing flat-top, overlap and material limits
  still apply. Entirely outside water is omitted.
- World/detail terrain is retained when its complete base cage and displaced
  grid fit. Combined bounds entirely outside the region are omitted. Existing
  [terrain compiler limits](compile-and-play.md#terrain-displacements) still
  apply; terrain in runtime brush models remains unsupported.
- Overlays, areaportals, areaportal windows, occluders,
  instances and visibility clusters require full-map builds. Retained slime,
  detail/model water and boundary-crossing water or terrain are also unsupported. These
  cases stop compilation and preserve prior output.
- At most 16,384 source objects, 65,536 source sides and 128 sides per brush.
  Bounds, source IDs and the mounted solid rendering `tools/toolsblack` material
  must be valid. Missing or incompatible boundary assets stop the build.
- Imported legacy cordons and a single enabled box under active plural cordons
  can compile. Plural or multiple records need **Preview clear** and
  **Apply region** before authoring a replacement. Clearing is undoable.
- Unapplied drafts never affect compilation. Region settings belong to the
  VMF, independently of named build recipes.
