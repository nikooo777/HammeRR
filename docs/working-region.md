[← Manual](README.md)

# Working region

Limit editor display and picking with a temporary box, camera draw distance,
or object search criteria. Active filters combine with AND and keep complete
objects.

1. Open **View → Working region…**.
2. To use a box, check **Enable box**. Set **Minimum** and **Maximum**, or
   click **Use selection bounds**. Coordinates use world units.
3. To move the box with the camera, check **Center box on camera**. The
   entered corners define its dimensions; its center follows the camera.
4. To limit distance, check **Limit draw distance** and enter **Distance**
   in world units. Distance is measured from the camera to the nearest point
   on each whole object's bounds. Planar views use the view center.
5. To use search criteria, check **Enable object filter**. Set **Search**,
   **Type**, **Class**, **Entity name**, **Material**, or **Visibility**.
   Search, class, name and material match case-insensitive substrings. Check
   **Exact class** to match the complete class name. To add a bounds
   intersection rule, check **Limit object filter to bounds** and set its
   **Minimum** and **Maximum**.
6. Click **Apply** to update all filters together. Scroll the panel to reach
   additional criteria. **Cancel** closes it without applying field changes.
7. Click **Clear working region** in the panel or **View** menu to remove
   all working region filters and restore otherwise-visible objects.

## Limits

- The box, distance and object filter each work independently. Disable
  **Enable box** to use only distance or search criteria.
- Matching uses whole owners, including every visible child of a brush
  entity. Filters do not cut geometry or remove selected objects.
- Hidden objects remain hidden. **Visibility** matches saved hidden state;
  choosing **Hidden** does not reveal hidden objects. Editor-only hiding
  remains separate; use **View → Show all editor-hidden** to restore it.
- Terrain and loaded models use their displayed geometry or checked bounds.
- Objects with unresolved bounds stay visible to the box and distance filters.
- Saved visibility, source geometry, selection and Undo history stay unchanged.
- Filters are temporary and do not affect compilation. Use
  [Compile region…](compile-regions.md) for builds.
