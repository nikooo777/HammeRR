[← Manual](README.md)

# Material detail scattering

Materials can generate non-colliding client-only detail models on exposed world
and `func_detail` brush faces and terrain. The compiler writes native model
records into the BSP's version-4 `dprp` game lump. The source map keeps its
original brushes and properties.

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
attempts per million square units. For ordinary brush faces, the compiler takes
the integer number of samples separately for each triangle of each final
clipped face. Small faces
can therefore generate no models. `amount` values above a combined total of
one are normalized; a smaller total leaves empty samples. Flat brush faces use
alpha one and select the group with the highest supported alpha.

Terrain takes its sample count from the whole base quad, so increasing grid
resolution does not remove samples. Samples spread uniformly over the base
grid, then follow its final compiled triangles, including LOD snapping.
Position and blend alpha interpolate from the native triangle vertices;
orientation follows its geometric normal. Neighboring dictionary alpha groups
are chosen probabilistically: an alpha halfway between two groups gives each
group half the samples on average. An empty selected group emits no model.

Remove the example's zero-degree angle limits to include sculpted slopes.
To control terrain distribution, paint the material onto a terrain face, open
**Edit terrain…**, and choose **Paint blend** or **Fill blend** under
**Operation**. **Blend value · 0 first texture, 255 second texture** also maps
to dictionary alpha 0–1. Click **Preview**, then **Apply**, and rebuild to see
the new distribution. The existing `preview_terrain_blend` and
`commit_face_preview` MCP calls provide the same edits.

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

![Material-generated detail models in CS:S](images/material-detail-scattering.png)

![Foliage generated on displaced terrain using its blend alpha](images/terrain-detail-scattering.png)

Limits: model-only detail types on world and `func_detail` surfaces. Terrain
requires a parallelogram base quad, including rectangles and sheared or sloped
parallelograms; skewed base quads reject. Sprites, procedural shapes, runtime
brush models, non-normal detail orientation, and fluid surfaces reject. Selected groups must contain supported
model entries and finite values; an unsupported selected group rejects the
complete build before replacing its previous BSP. Unused groups do not reject
a build: flat faces can select an empty alpha-one group even when lower groups
contain sprites, while terrain only accepts a lower group when it is supported.
Dictionaries are limited to 2 MiB, eight levels and 32,768 nodes. Each build
allows 65,536 sample attempts, 16,384 combined
explicit/generated instances and 1,024 distinct detail models. Terrain lookup
construction and sampling share a four-million-step work limit. Samples use a
small outward probe inset from face boundaries to choose an open visible BSP
leaf while the stored model origin stays on its surface. Models keep white
unbaked lighting; baked detail lighting and light styles remain unfinished.

Compile regions scatter only on the retained, clipped exposed surfaces. Region
clipping changes the count and seed; the source brush stays whole. Generated
origins stay inside the region and their open leaves, but model geometry can
extend past the boundary. Explicit `prop_detail` entities still require full-map
builds. Terrain must stay wholly inside the region, including its complete
base and displaced grid; crossing terrain rejects rather than being clipped.

Groups may be empty, including the alpha-zero groups in installed model detail
types. Flat faces selecting an empty group emit no models and produce an
informational diagnostic. Terrain samples selecting an empty group are
skipped. Neither path falls back to a different group.

**Detail material file** (`detailmaterial`) selects the runtime sprite atlas;
it does not select the model dictionary. Maps without exposed `%detailtype`
materials do not read the dictionary and retain their existing detail payload.
