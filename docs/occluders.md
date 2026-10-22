[← Manual](README.md)

# Occluders

Author fixed occlusion polygons that CS:S can use to reject hidden renderable objects. The native compiler emits their geometry, area binding and runtime entity index. Active helpers cull covered props; their activation can be controlled by entity inputs.

## Author and compile

1. Create 1–16 convex brushes inside one visibility area of a sealed map, contained within the opaque wall or other permanent cover they represent. Brushes may be separated, share a face or overlap. The compiler keeps their exterior polygons and preserves gaps and concave outlines. Occluders are invisible helpers; they do not provide visible walls or collision.
2. Select the brushes in **Objects** mode. Type `tools/toolsoccluder` in the material inspector and click **Apply**. For unused sides, switch **Selection** to **Faces · materials & textures**, select those sides and apply `tools/toolsnodraw`.
3. Return to **Objects**, select all the helper brushes and click **Tie to entity…**. Choose `func_occluder`.
4. Set **Advanced properties → Start Active** to **Yes** (stored `StartActive:1`, the default) or **No** (`StartActive:0`). For a root-map helper, set its **Name** if another entity should control it.
5. Use **Connections…** on the controlling entity to add an output targeting that name, with input `Activate`, `Deactivate` or `Toggle`. These inputs enable, disable or toggle runtime culling.
6. Click **Compile**, then **Build details…**. Check **Occluders**, **Polygons** and **Vertex references**, and review **Diagnostics**. A helper crossing multiple visibility areas must be split into separate entities within those areas.

![Build details with compilation statistics and diagnostics](images/build-status.png)

![Entity connections with target and input controls](images/entity-connections.png)

## Occluders in referenced assemblies

Use unnamed stationary helpers in a saved [referenced assembly](instances.md).
Each child entity keeps its 1–16 visible ordinary convex brushes as one occluder,
including separated islands, exact face contacts and supported overlaps. Moving
or rotating the containing reference transforms the whole helper; runtime
parenting and named child control are unsupported.

1. Open the child VMF as a normal map and follow **Author and compile** steps
   1–4. Keep **Name** empty, add no connections and choose **Start Active → Yes**
   or **No**. Normal authoring stores the installed FGD default `StartActive:1`.
2. Save the child, return to its parent and create or refresh its reference
   through **Instance reference…** or **Instance contents… → Refresh instance
   contents**. Opening the child as a normal map retains ordinary brush editing.
3. Click **Compile**, then **Build details…**. **Occluder helpers:** counts
   expanded child entities before compile-region filtering. **Occluders**,
   **Polygons** and **Vertex references** describe retained root and child output.

**Instance contents…** reports the same pre-region helper count. Child occluders
add no reference geometry, icons, content bounds, picking or framing.
**Frame instance contents** is unavailable for a helper-only reference and
leaves the camera unchanged. Mixed references keep their real brush/terrain
surfaces, bounds and picking. A ready reference checks source and transformed
geometry admission; compilation still checks materials, union boundaries,
visibility areas and retained global limits.

Child properties use canonical lowercase keys except `StartActive`, which
accepts only 0 or 1 when present. Optional `angles` must be `0 0 0`; optional
`angle`, `spawnflags`, `mindxlevel` and `maxdxlevel` must be zero.
`targetname`, `target`, `parentname`, `name`, `globalname`, `killtarget`
and `lightingorigin` must be absent or exactly empty; whitespace is rejected.
Unknown properties, duplicate case aliases, opaque blocks and connections
reject. Absent origins and angles remain absent.

Optional origins and every brush vertex must be finite and within ±16384 units
at the source and each inner-to-outer instance pose, even if a later transform
would bring them back inside. Each helper permits at most 256 input sides
and, at each checked pose, 256 polygons and 4,096 polygon vertices. Source and
intermediate checks precede region omission and spend the ordinary instance
expansion budgets. Child diagnostics identify the containing root reference,
true leaf file and original child entity, brush or face.

## Limits

- Each entity supports 1–16 visible convex brushes. Overlapping volumes and exact face contacts produce the union boundary; contained brushes and duplicate faces do not add internal polygons. Disjoint rotated shapes can have overlapping bounds. Hidden entities and hidden brushes are omitted. Child helpers containing only hidden brushes are omitted; a truly empty child helper rejects. Separate `func_occluder` entities retain independent geometry and activation.
- Interacting brushes must have closed convex topology with at most 64 sides each. Edge-only and point-only contacts, gaps or thin overlaps within 0.01 units of contact, nearly coplanar faces and collapsed or tiny clipping fragments are rejected. Align shared faces exactly, overlap by more than 0.01 units, simplify the helper or use separate entities. At coincident exterior faces, an active face takes precedence over nodraw; the first active brush owns duplicate coverage.
- Occlusion geometry stays in world coordinates. Parenting is rejected; changing an entity origin does not move its compiled polygons. Use ordinary brush editing to reposition the helper.
- [Compile regions](compile-regions.md#include-overlays-and-visibility-helpers) retain a helper only when its combined visible brush bounds fit completely. Disjoint helpers are omitted; crossing bounds stop the build. Editor origins do not determine inclusion.
- Nodraw faces are excluded, except trigger-marked faces. The material must be available in mounted content so the compiler can check its flags. At least one active face must remain. Hint, skip, origin and terrain tooling are rejected.
- Every emitted occluder needs one adjacent open visibility area. Area checks include the polygon interiors and adjacent leaves; ambiguous placements fail with an object-linked diagnostic. Keep helpers clear of area boundaries.
- CS:S transmits occluder indices in ten bits, allowing 1,024 indexed entities across retained root helpers and all child occurrences after region selection. HammeRR additionally bounds each helper to 256 combined input sides and 4,096 input polygon vertices, then 256 boundary polygons and 4,096 boundary vertices after union. Across the map, union work is limited to four million work units, one million allocated points and 65,536 generated fragments. Native output is limited to 65,536 polygons and 262,144 vertex references; area assignment has a separate four-million-work-unit limit. Global BSP vertex and plane budgets also apply.
- The helper brushes are consumed without adding a runtime brush model, render face, collision brush or PVS split. Each retained helper receives a fresh `occludernumber` and its activation setting. Root helpers also retain their names and connections; child helpers remain unnamed and stationary.
- Failed or cancelled compilation preserves the previous BSP. Compilation leaves authored geometry, properties and undo history unchanged.

Agents use the same operations: `create_block`, `set_material`, `tie_to_entity`, `set_entity_properties`, entity connections, `compile_map` or `start_compile`, and `get_compile_status`. No occluder-specific build setting is required.
