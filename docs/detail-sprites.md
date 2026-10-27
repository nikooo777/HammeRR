[← Manual](README.md)

# Detail sprites

Compile decorative atlas cards or fixed cross/tri shapes without collision.

1. Open **World settings…** and set **Detail material file**.
2. Place custom class `prop_detail_sprite` through **Entity…**.
3. Under **Advanced properties**, set the atlas and shape values below.
4. Compile with **Direct LDR**.
5. Inspect **Detail props**, **Sprite entries** and **Diagnostics**.
6. Play the built map.

| Property | Example | Meaning |
| --- | --- | --- |
| `position_ul` | `-20 45` | Upper-left corner in local world units. |
| `position_lr` | `20 0` | Lower-right corner in local world units. |
| `tex_ul` | `0 0` | Atlas origin. |
| `tex_size` | `128 128` | Atlas region size. |
| `tex_total_size` | `512` | Logical atlas size. |
| `detailOrientation` | `2` | Upright camera-facing card. |
| `scale` | `1.5` | Positive uniform size multiplier. |
| `sprite_shape` | `tri` | Fixed `cross` or `tri`; omit for a card. |
| `shape_angle` | `35` | Tri fan angle, 0–255 degrees. |
| `shape_size` | `0.5` | Tri offset fraction, 0–1. |
| `sway` | `0.5` | Runtime sway strength, 0–1. |

Orientation 0 is fixed; 1 is a full billboard; 2 remains vertical.
Cross and tri shapes require orientation 0. Cross requires zero angle and size.
Use **Baked lighting tint · linear RGB** for supported lighting attenuation.

## Limits

Atlas textures must meet the [native texture limits](textures.md#native-texture-files).

- All detail sprites share the world's atlas material.
- No collision or ordinary runtime entity remains after compilation.
- The editor helper does not display the final native card or sway.
- Unsupported atlas declarations, geometry or combinations fail explicitly.
