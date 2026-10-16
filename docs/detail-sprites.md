[← Manual](README.md)

# Explicit detail sprites

Place individual decorative cards from the world's detail atlas. Compilation
turns each `prop_detail_sprite` point entity into a native detail record and
removes its runtime entity. The sprite has no collision.

1. Open a sealed map with a player spawn. In **World settings…**, keep
   **Detail material file** at `detail/detailsprites`, or choose a supported
   mounted atlas material. All detail sprites in the map share this atlas.
2. Click **Entity…**. In **Classname (custom classes are supported)**, enter
   `prop_detail_sprite`. Set **Position** inside an open area, then click
   **Place entity**. **Cancel** closes the form without placing it.
3. Select the entity and enable **Advanced properties**. Add each row below
   using **Property key** and **Value**, pressing Enter after each value:

   | Property key | Value | Meaning |
   | --- | --- | --- |
   | `position_ul` | `-20 45` | Local upper-left corner in world units |
   | `position_lr` | `20 0` | Local lower-right corner in world units |
   | `tex_ul` | `0 0` | Logical atlas X and Y |
   | `tex_size` | `128 128` | Logical atlas width and height |
   | `tex_total_size` | `512` | Logical atlas size |
   | `detailOrientation` | `2` | Face the camera while remaining vertical |

4. Choose orientation `0` for a fixed card or `1` for a full billboard that also
   faces the camera's elevation. Orientation `0` is the default when omitted;
   its `angles` property uses pitch, yaw and roll, for example `0 45 0`.
5. Open **Build settings…**, choose **Direct LDR** under **Build lighting**,
   then **Close** and **Compile**. **Build details…** reports **Detail props**,
   **Sprite entries** and **Diagnostics**.
6. Click **Run Map** to inspect the sprite in CS:S. `r_drawdetailprops 0` hides
   details and `r_drawdetailprops 1` restores them.

![Explicit atlas sprites in Counter-Strike: Source](images/explicit-detail-sprites.png)

Limits: all five position/texture fields are required and must contain the
specified number of finite values. `position_lr` must be right of and below
`position_ul`. Texture X/Y must be nonnegative, width/height at least one logical
texel, and the rectangle must fit the positive `tex_total_size`. UVs use a
half-texel inset in logical atlas coordinates, independently of the VTF's actual
resolution. The example makes a 40-by-45-unit card with its bottom center at the
entity origin.

These explicit sprites use native scale one and plain type-one records.
Random scaling, procedural cross/tri shapes and sway are unsupported.
`detailOrientation` must be an integer from 0 to 2. Without an authored origin
or rotation, both default to zero. Origins and complete card
bounds must fit within ±16384 units; origins must resolve to open visible BSP
leaves. Fixed cards use rotated corners; full billboards use a conservative
sphere. Hidden source entities are omitted. No model asset is required.

[Compile regions](compile-regions.md) select explicit sprites by their authored
origin, including points exactly on the box. An omitted origin uses world zero.
Outside and hidden sprites are omitted before card or atlas validation.
Visible malformed or out-of-range origins still
reject before selection. Retained sprites keep their complete geometry, which
may extend beyond the box, and pass the normal atlas, bounds, open-leaf and
lighting checks. A selected boundary point may fail the open-leaf check; the
compiler does not move it inward.

Named lighting origins, authored tint, entity I/O, parenting, scripts, collision and
nondefault render/fade settings are unsupported. The editor shows the entity
helper; it does not preview the compiled atlas card.

Sprites share the [material-detail atlas requirements and
budgets](material-detail-scattering.md), including 4096 sprite dictionary entries,
16384 combined explicit/generated detail instances, a 512 MiB asset-read budget
and 65536 material lookups. A supported atlas uses a static square VTF at most
4096 pixels per side and 64 MiB, with the required `UnlitGeneric` flags.

**Fullbright** keeps white unbaked colors. Lit builds sample the scaled anchor
midpoint using the stored local-up normal and require that point to remain open
and visible. Static lighting, style extras and the shared final HDR base follow
the [same detail-lighting rules](material-detail-scattering.md). Unsupported
inputs and exceeded budgets preserve the previous output. Compilation leaves
the source map and Undo history unchanged.
