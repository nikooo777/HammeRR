[← Manual](README.md)

# Occluders

Author fixed occlusion polygons that CS:S can use to reject hidden renderable objects. The native compiler emits their geometry, area binding and runtime entity index. Renderer culling and activation changes still need an in-game validation pass.

## Author and compile

1. Create one convex brush inside a sealed map, contained within the opaque wall or other permanent cover it represents. Occluders are invisible helpers; they do not provide visible walls or collision.
2. Select the brush in **Objects** mode. Type `tools/toolsoccluder` in the material inspector and click **Apply**. For unused sides, switch **Selection** to **Faces · materials & textures**, select those sides and apply `tools/toolsnodraw`.
3. Return to **Objects**, select the brush and click **Tie to entity…**. Choose `func_occluder`.
4. In **Advanced properties**, set `StartActive` to `1` for an initially active occluder or `0` for an inactive one. Set its **Name** if another entity should control it.
5. Use **Connections…** on the controlling entity to add an output targeting that name, with input `Activate`, `Deactivate` or `Toggle`. These connections are preserved in the compiled entity; their runtime effect remains unvalidated.
6. Click **Compile**, then **Build details…**. Check **Occluders**, **Polygons** and **Vertex references**, and review **Diagnostics**. A helper crossing multiple visibility areas must be split into separate entities within those areas.

![Build details with compilation statistics and diagnostics](images/build-status.png)

![Entity connections with target and input controls](images/entity-connections.png)

## Limits

- One visible convex brush per entity. Multiple-brush occluders and cross-occluder CSG optimization are not implemented. Hidden entities and hidden child brushes are omitted.
- Occlusion geometry stays in world coordinates. Parenting is rejected; changing an entity origin does not move its compiled polygons. Use ordinary brush editing to reposition the helper.
- Nodraw faces are excluded, except trigger-marked faces. The material must be available in mounted content so the compiler can check its flags. At least one active face must remain. Hint, skip, origin and terrain tooling are rejected.
- Every emitted occluder needs one adjacent open visibility area. Area checks include the polygon interiors and adjacent leaves; ambiguous placements fail with an object-linked diagnostic. Keep helpers clear of area boundaries.
- CS:S transmits occluder indices in ten bits, allowing 1,024 indexed entities. HammeRR additionally bounds each helper to 256 sides and 4,096 polygon vertices, total output to 65,536 polygons and 262,144 vertex references, and area assignment to four million work units. Global BSP vertex and plane budgets also apply.
- The brush is consumed without adding a runtime brush model, render face, collision brush or PVS split. The runtime entity retains its name, activation setting and connections and receives a fresh `occludernumber`.
- Failed or cancelled compilation preserves the previous BSP. Compilation leaves authored geometry, properties and undo history unchanged.

Agents use the same operations: `create_block`, `set_material`, `tie_to_entity`, `set_entity_properties`, entity connections, `compile_map` or `start_compile`, and `get_compile_status`. No occluder-specific build setting is required.
