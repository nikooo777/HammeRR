# Material detail scattering

Materials can generate non-colliding client-only detail models on exposed world
and `func_detail` brush faces. The compiler writes native model records into the
BSP's version-4 `dprp` game lump. The source map keeps its original brushes and
properties.

1. Put a model-only detail dictionary in mounted game content. Its outer block
   contains named detail types; each type has a `density` and one or more groups.
   Groups contain `alpha` and named model entries. Each entry needs a `model`
   path. Models must be compiled with `$staticprop` and use supported
   `UnlitGeneric` materials.
2. Add `%detailtype` to the ground VMT, naming the dictionary type. Material
   patches resolve through the same mounted content as ordinary materials.
3. Open **World settings…**, set **Detail.vbsp file** (`detailvbsp`) to that
   game-relative `.vbsp` path, and click **Apply properties**. Leaving it empty
   uses `detail.vbsp`.
4. Paint the material onto the intended exposed faces using the normal material
   controls. Hidden brushes and hidden `func_detail` owners are excluded.
5. Build the map. **Build details…** reports **Detail props** and **Model assets**
   together with explicit model detail props. Run the built map to inspect the
   generated models.

A minimal dictionary using an installed CS:S detail model is:

```text
detail
{
    shrubs
    {
        density 64
        group
        {
            alpha 1
            shrub
            {
                model "models/props/de_inferno/bushgreensmall.mdl"
                amount 1
                upright 1
                minAngle 0
                maxAngle 0
            }
        }
    }
}
```

The corresponding VMT declares `%detailtype shrubs`. `density` gives sample
attempts per million square units. The compiler takes the integer number of
samples separately for each triangle of each final clipped face. Small faces
can therefore generate no models. `amount` values above a combined total of
one are normalized; a smaller total leaves empty samples. Flat brush faces use
alpha one and select the group with the highest supported alpha.

`upright 1` keeps models vertical with a random yaw. With `upright 0`, models
follow the face normal. `minAngle` and `maxAngle` are degrees from world up:
placement is certain below the minimum, fades between the two limits, and stops
beyond the maximum. Both default to 180 degrees. A changed authored face ID or
final clipped polygon changes that face's deterministic seed. Rebuilding the
same map and mounted content produces the same records; placements are not
intended to match Valve VBSP byte for byte.

The existing MCP operations use the same compiler: paint through `edit_faces`,
change `detailvbsp` through the world property draft, then call `compile_map` or
`start_compile`. No separate scatter objects or authoring operation are needed.

Limits: model-only detail types on planar world and `func_detail` surfaces;
terrain, sprites, procedural shapes, runtime brush models, non-normal detail
orientation, and fluid surfaces reject. Referenced types must contain supported
model groups and finite values. Dictionaries are limited to 2 MiB, eight levels
and 32,768 nodes. Each build allows 65,536 sample attempts, 16,384 combined
explicit/generated instances and 1,024 distinct detail models. Samples use a
small outward probe inset from face boundaries to choose an open visible BSP
leaf while the stored model origin stays on its surface. Models keep white
unbaked lighting; baked detail
lighting and light styles remain unfinished.

Groups may be empty, including the alpha-zero groups in installed model detail
types. An empty selected group emits no models and produces an informational
diagnostic; it never falls back to a different group.

**Detail material file** (`detailmaterial`) selects the runtime sprite atlas;
it does not select the model dictionary. Maps without exposed `%detailtype`
materials do not read the dictionary and retain their existing detail payload.
