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
7. Read the retained contributors below the statistics. Click an authored root
   or external instance contributor to find its compiled fragments. Click a
   compiled match to inspect it. **Match page starts at** and **Find matches**
   page through results; **Contributor page starts at** and **Inspect face**
   page through contributors.
8. With one authored root face already selected in the editor, click
   **Inspect selected authored face** to find its fragments. These inspection
   lists leave editor selection, camera and pending reviews unchanged.
   **Close** or Escape closes the window.

![Baked HDR samples with decoded intensity statistics](images/baked-lightmaps.png)

Limits:

- Face numbers belong to the selected compiled BSP channel. They are not VMF
  side IDs, and changing the build can change their meaning. Authored origins
  come from contributor sets retained at actual compiler emission. One source
  may split into many faces; merged faces may have several contributors. The
  list identifies contributing surfaces, with no exact polygon footprint or
  authored vertex/luxel mapping. No baked viewport mode is provided.
- Terrain identifies its authored base face and compiled displacement. Instance
  origins identify the root helper, registered child filename and original child
  solid/side; they do not identify editable root faces. Generated compile-region
  shell faces are labeled explicitly. Other generated/unretained faces and
  omitted sources have explicit diagnostics.
- Mapping retains at most 65,536 faces, 262,144 contributor references and 8 MiB
  of metadata, with pages of at most 32 rows. Mapping unavailable diagnostics
  preserve ordinary compiled-face inspection and successful compilation.
- Fullbright builds and absent HDR channels have no baked samples. Unsupported
  face layouts, invalid sample ranges and unavailable styles/maps report errors.
- The image compresses each linear channel with Reinhard after dividing by 255,
  then applies gamma `1/2.2`. It uses nearest-sample reduction to at most
  128 pixels per axis. Statistics retain decoded RGBExp32 intensity; the image
  does not reproduce game exposure or tone mapping.
- Inspection reads only a successful build owned by this session, up to
  128 MiB. Changed or missing BSP output is unavailable. Map, source-file,
  content or build-setting changes label the retained build stale. Refresh
  inspection after changes; compile again for current samples. Explicit
  inspection verifies retained root and instance-child bytes as well as the
  output. Historical source IDs stay labeled stale even if those IDs still
  exist. A displayed geometry/face draft is explicitly reported as not baked.
- Inspection and image generation do not edit the map, history, selection or
  camera and do not require the viewport renderer.

The shared MCP operation is `inspect_baked_lightmaps`. Pass a compiled `face`,
`source: {"kind":"root","solid":123,"side":456}`, an instance source
`{"kind":"instance","instance":789,"file":"child.vmf","solid":12,"side":34}`,
or `selected_authored_face:true`. These selectors are mutually exclusive.
`offset`/`limit` page compiled faces or source matches; `contributor_offset` pages
contributors of the inspected face. Instance filenames only match retained
origins and never authorize arbitrary file reads.
