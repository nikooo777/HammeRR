[← Manual](README.md)

# Baked lightmaps

Inspect the actual face-lighting samples from a successful build. Compare LDR
and HDR channels, styles and directional maps without launching the game.

1. Choose **Build settings…**, set **Build lighting** to **Direct LDR** or
   **Direct LDR + HDR**, close the window, then click **Compile**.
2. Open **Build details…** in the build bar below the toolbar, then click
   **Inspect baked lightmaps…**. **Build settings…** also opens the build window.
3. Check the build status and available channels. Choose **Channel** and a
   zero-based **Compiled face**, then click **Inspect face**.
4. Read the sample dimensions, styles, stored base average and minimum, mean
   and maximum linear RGB intensity beside the image.
5. Change **Style index** to select an entry in that face's style list.
   **Map index** `0` selects the base map; `1`–`3` select directional bump maps
   when the face has them. Click **Inspect face** after changing a selection.
6. Change **Face page starts at**, then click **Inspect face** to load another
   page of 32 compiled faces.
   **Close** or Escape closes the window.

![Baked HDR samples with decoded intensity statistics](images/baked-lightmaps.png)

Limits:

- Face numbers belong to the selected compiled BSP channel. They are not VMF
  side IDs, and changing the build can change their meaning. No automatic
  authored-face correspondence or baked viewport mode is provided.
- Fullbright builds and absent HDR channels have no baked samples. Unsupported
  face layouts, invalid sample ranges and unavailable styles/maps report errors.
- The image compresses each linear channel with Reinhard after dividing by 255,
  then applies gamma `1/2.2`. It uses nearest-sample reduction to at most
  128 pixels per axis. Statistics retain decoded RGBExp32 intensity; the image
  does not reproduce game exposure or tone mapping.
- Inspection reads only a successful build owned by this session, up to
  128 MiB. Changed or missing BSP output is unavailable. Map, source-file,
  content or build-setting changes label the retained build stale. Refresh
  inspection after changes; compile again for current samples.
- Inspection and image generation do not edit the map, history, selection or
  camera and do not require the viewport renderer.
