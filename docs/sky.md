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

## Inspect mounted sky assets

Inspect the applied family's material and texture structure before choosing a sky or diagnosing a missing channel.

1. Open **World settings…** and click **Inspect applied sky…**. The inspector uses the applied `skyname`; pending property edits remain staged. To inspect a changed name, click **Apply properties**, reopen **World settings…**, and inspect again.
2. Read **LDR assets** and **HDR assets**. Both describe supported material declarations and VTF structure, independently of the editor preview and the game's active channel.
3. Compare the six face buttons: **Right**, **Back**, **Left**, **Front**, **Up** and **Down** show LDR/HDR readiness together. Choose a face to read its exact `skybox/<skyname><suffix>` material, shader, winning directory or VPK source, additional Patch sources, texture references, dimensions, format and issues.
4. Repair the named material or texture in the reported mount, then click **Refresh sky assets**. Read failures remain visible; a broken winning asset does not silently select a healthy lower mount.
5. Click **Close** to return to the existing property edit. Inspection does not change the map, Undo, camera, selection, clipboard or drafts, and starts no game or compiler process.

![Six sky faces with LDR and HDR asset status and selected-face details](images/sky-assets.png)

MCP provides the same report through `inspect_sky_assets {}`. It reads current mounted files each time and preserves valid authored names and references. Overlong invalid values and bounded diagnostics carry explicit truncation indicators. It does not repair an invalid name, append `_hdr`, choose a substitute family or copy assets into the map.

Inspection supports static `Sky` and `UnlitGeneric` material declarations with bounded Patch resolution. LDR requires `$basetexture`; HDR requires `Sky` with `$hdrbasetexture` in RGBA16F or RGBA16 format. VTF layout must be complete, with one frame, one face, depth one and at most 4096 pixels per axis. Compressed HDR declarations, unknown parameters and nested material effects produce explicit issues. The report permits 128 distinct asset lookups, 64 MiB total reads and 1 MiB per material; limits and read counts accompany the result. Family names are limited to 256 bytes and must be relative, without surrounding whitespace or control characters.

Structural readiness does not decode HDR pixel radiance or establish shader rendering. A declared texture transform can pass inspection while remaining unsupported by the LDR preview. Automatic compiler admission checks for sky structure, portable custom sky packing and an HDR editor preview remain separate work.

## Compiled HDR skies

CS:S resolves the exact authored sky family from mounted game content. Stock families such as `sky_dust_hdr`, `train_hdr`, `militia_hdr` and `de_cobble_hdr` declare both HDR textures and LDR fallbacks. The game's `Sky` shader selects the supported channel; it does not add `_hdr` to a family such as `sky_dust` automatically.

1. Open **World settings…** and enter the mounted HDR family in **SkyBox Texture Name** (`skyname`), then click **Apply properties**. Custom entity definitions may label this field differently.
2. For a dual-channel map, open **Build settings…** and choose **Build lighting → Direct LDR + HDR**. Compile the map and check **HDR native map data** in **Build details…**.
3. Select HDR in the game before loading the map. Native HDR still requires complete map data and game/hardware support; see [HDR channels](compile-and-play.md#hdr-channels).

The compiler preserves `skyname`. Original sky materials and textures must remain available through the game's mounts; compilation does not automatically pack a custom sky into the BSP. **Inspect applied sky…** checks supported material and HDR VTF structure separately from compilation. **Generate reflections** checks referenced sky asset existence and source hashes as part of its capture guards, without establishing HDR sky radiance or shader rendering.

The editor continues to preview the family's LDR `$basetexture` fallback. The status **six LDR faces ready** describes that preview, without establishing HDR asset readiness or the game's selected channel. Baked sunlight and sky ambient use authored `light_environment` values rather than deriving lighting from the sky image.

## Limits

The preview supports six mounted `UnlitGeneric` or `Sky` materials with static LDR `$basetexture` textures. Axis-aligned texture scale and translation are supported, including the vertical scale used by stock skies. Face dimensions may differ. All six faces must resolve; otherwise the ordinary tool surface remains and the status explains why. Textures are limited to 4096 pixels per axis and 64 MiB per file.

Flat shading, wireframe, diagnostic and planar views retain their existing behavior. **Tool textures** hides sky surfaces along with other tools. Only authored sky surfaces display the sky; the editor background remains unchanged. The preview does not reproduce compiled BSP visibility, 3D `sky_camera` regions, HDR rendering, fog, exposure, material tint, animation, rotated texture transforms or nested material effects. After changing mounted sky materials or textures, open **Browse materials…**
and click **Reload material assets**. See [Reload changed materials](textures.md#reload-changed-materials)
for diagnostics and limits.
