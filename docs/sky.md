[← Manual](README.md)

# Previewing the 2D sky

Preview the world's selected sky on sky surfaces while editing, using materials and textures from the mounted game content.

1. Open **World settings…**, set `skyname` to a mounted sky such as `sky_dust`, and click **Apply properties**.
2. Create or select the sky boundary brushes and assign `tools/toolsskybox` or `tools/toolsskybox2d`.
3. Choose **Perspective** and **Textured**, and enable **Tool textures**.
4. Enable **2D sky preview** under **VIEWPORT**. The status names the sky and reports whether all six LDR faces are ready.
5. Navigate inside the sky boundary. The sky follows the viewing direction and stays at infinity when the camera moves. Sky surfaces remain pickable, and foreground geometry and selection outlines remain visible.
6. Disable **2D sky preview** to return to the ordinary tool surface.

![The selected CS:S sky rendered through sky surfaces](images/sky-preview.png)

The setting is shared with MCP through `set_view` using `sky_preview`. `render_view` accepts the same optional setting for a single render. `get_map_info` and rendered-image metadata report the selected sky, six material names, texture dimensions and individual load errors. These controls do not change the map or create undo steps; changing `skyname` uses ordinary undoable world settings.

## Compiled HDR skies

CS:S resolves the exact authored sky family from mounted game content. Stock families such as `sky_dust_hdr`, `train_hdr`, `militia_hdr` and `de_cobble_hdr` declare both HDR textures and LDR fallbacks. The game's `Sky` shader selects the supported channel; it does not add `_hdr` to a family such as `sky_dust` automatically.

1. Open **World settings…** and enter the mounted HDR family in **SkyBox Texture Name** (`skyname`), then click **Apply properties**. Custom entity definitions may label this field differently.
2. For a dual-channel map, open **Build settings…** and choose **Build lighting → Direct LDR + HDR**. Compile the map and check **HDR native map data** in **Build details…**.
3. Select HDR in the game before loading the map. Native HDR still requires complete map data and game/hardware support; see [HDR channels](compile-and-play.md#hdr-channels).

The compiler preserves `skyname`. Original sky materials and textures must remain available through the game's mounts; compilation does not automatically pack a custom sky into the BSP. Sky-specific HDR texture validation and portable custom sky packing remain unavailable. **Generate reflections** checks referenced sky asset existence and source hashes as part of its capture guards, without validating HDR sky texture structure.

The editor continues to preview the family's LDR `$basetexture` fallback. The status **six LDR faces ready** describes that preview, without establishing HDR asset readiness or the game's selected channel. Baked sunlight and sky ambient use authored `light_environment` values rather than deriving lighting from the sky image.

## Limits

The preview supports six mounted `UnlitGeneric` or `Sky` materials with static LDR `$basetexture` textures. Axis-aligned texture scale and translation are supported, including the vertical scale used by stock skies. Face dimensions may differ. All six faces must resolve; otherwise the ordinary tool surface remains and the status explains why. Textures are limited to 4096 pixels per axis and 64 MiB per file.

Flat shading, wireframe, diagnostic and planar views retain their existing behavior. **Tool textures** hides sky surfaces along with other tools. Only authored sky surfaces display the sky; the editor background remains unchanged. The preview does not reproduce compiled BSP visibility, 3D `sky_camera` regions, HDR rendering, fog, exposure, material tint, animation, rotated texture transforms or nested material effects. After changing mounted sky materials or textures, open **Browse materials…**
and click **Reload material assets**. See [Reload changed materials](textures.md#reload-changed-materials)
for diagnostics and limits.
