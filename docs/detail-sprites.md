[← Manual](README.md)

# Explicit detail sprites

Place individual decorative cards or fixed cross/tri shapes from the world's detail atlas. Compilation
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
5. Optionally add the following **Advanced properties**. Leave them omitted for
   the original plain card at scale one.

   | Property key | Value | Meaning |
   | --- | --- | --- |
   | `scale` | `1.5` | Positive finite native-float scale; default `1` |
   | `sprite_shape` | `tri` | Fixed `cross` or `tri` geometry; omit or leave empty for a plain card |
   | `shape_angle` | `35` | Tri fan angle, integer 0–255 degrees; default `0` |
   | `shape_size` | `0.5` | Tri offset as a fraction of full width, 0–1; default `0` |
   | `sway` | `0.5` | Runtime sway strength, 0–1; default `0` |

   Both shapes require `detailOrientation 0`. Cross shapes require omitted or
   zero angle/size. Clear `sprite_shape` and keep both shape parameters zero to
   return to a plain card. Use `scale` for size; keep `modelscale` at `1`.
6. Open **Build settings…**, choose **Direct LDR** under **Build lighting**,
   then **Close** and **Compile**. **Build details…** reports **Detail props**,
   **Sprite entries** and **Diagnostics**.
7. Click **Run Map** to inspect the sprite in CS:S. `r_drawdetailprops 0` hides
   details and `r_drawdetailprops 1` restores them.

![Explicit atlas sprites in Counter-Strike: Source](images/explicit-detail-sprites.png)

Limits: all five position/texture fields are required and must contain the
specified number of finite values. `position_lr` must be right of and below
`position_ul`. Texture X/Y must be nonnegative, width/height at least one logical
texel, and the rectangle must fit the positive `tex_total_size`. UVs use a
half-texel inset in logical atlas coordinates, independently of the VTF's actual
resolution. The example makes a 40-by-45-unit card with its bottom center at the
entity origin.

`detailOrientation` must be an integer from 0 to 2 for plain cards; cross/tri
shapes require 0. `scale` must remain positive and finite after conversion to a
native float. Fixed/full cards, cross/tri shapes and vertical cards with styles
or encoded nonzero sway use the native half-precision path: scale must fit
2⁻²⁴ through 65504. Positive half conversion truncates downward; full raw-scale
rest bounds are checked. An unstyled vertical card with sway quantized to zero
retains its positive-float fast path. Native intermediate width/height values
must also remain finite and noncollapsed, even when final scaled dimensions fit.
Nonzero `spriterandomscale` rejects; authored scaling is deterministic.

Cross geometry centers its horizontal branches regardless of the horizontal
anchor. Tri geometry retains the anchor and uses the stored fan angle and offset.
`shape_size` and `sway` convert to float, multiply by 255 and truncate to an SDK
byte: `0.5` becomes 127, decoded as 127/255. In CS:S, enter
`cl_detail_max_sway 8` to allow motion; its default `0` disables sway. This client
control is global and is not saved as a compiler/map setting. Enter
`cl_detail_max_sway 0` to disable it again. Bounds cover the geometry at rest;
sway and client player avoidance can move vertices beyond them.

Without an authored origin or rotation, both default to zero. Origins and complete card/shape
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

**Fullbright** keeps white unbaked colors. Lit builds sample a plain card's
scaled anchor midpoint or a cross/tri shape's stored origin, using the stored
local-up normal. Shape angle and size do not move that receiver. The sampling
point must remain open and visible. Static lighting, style extras and the shared final HDR base follow
the [same detail-lighting rules](material-detail-scattering.md). Unsupported
inputs and exceeded budgets preserve the previous output. Compilation leaves
the source map and Undo history unchanged.
