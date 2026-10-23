[← Manual](README.md)

# Fog preview

Inspect an authored fog controller in the editor.

1. Use **Entity…** to place `env_fog_controller`.
2. Set `fogcolor`, `fogstart`, `fogend`, `fogmaxdensity` and `fogenable` in its properties.
3. Choose **Perspective** and **Textured**.
4. Open **Fog preview…**.
5. Enable **Enable fog preview**.
6. Choose an **Active controller**.
7. Click **Apply preview**.

| Fog off | Fog on |
| --- | --- |
| ![The courtyard without fog](images/fog-off.png) | ![The same courtyard with distance fog](images/fog-on.png) |

**Cancel** discards the choice. Use **Refresh choices** after changes from another interface.
To disable fog, clear **Enable fog preview**, then click **Apply preview**.

## Limits

- Static initial settings only; perspective Textured surfaces and model meshes receive fog.
- The preview requires an explicit, visible controller. Deleted or invalid controllers report an error.
- Start and end must satisfy `0 ≤ start < end`. Density is 0–1; RGB channels are integers from 0–255.
- Radial fog, secondary colors, far-Z clipping, volumes, transitions and nondefault HDR/zoom scales are unsupported.
- Sky, helpers, selection outlines and the grid do not receive fog.
- This is an editor approximation, not a game brightness match.
