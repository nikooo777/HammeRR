[← Manual](README.md)

# Material source

Read the selected material's mounted VMT source, find the directory or VPK that
provides it, and inspect resolved Patch declarations.

1. Open **Browse materials…** and select a material, or enter its **Material path**.
2. Click **Source…** beside the preview. The new **Material source** window shows
   the normalized VMT path and each source file used for Patch resolution.
3. Under **Mounted source and Patch includes**, read the winning directory and
   file, or VPK index and storage paths. **Resolved shader declaration** and
   named texture parameters show the values after Patch replacement/insertion.
4. Read **Selected VMT source** for the original selected file. Parse and include
   failures remain visible beside the raw source; unreadable or invalid UTF-8
   files report an explicit error.
5. Click **Refresh source** to reread the same selected material. Click **Close**
   or press Escape to return to the browser. Inspection does not apply a material
   or change the map, selection, history, camera, previews or cached textures.

![Material source, mounted origin and resolved declarations](images/material-source.png)

Limits: the raw source display contains at most 64 KiB, with explicit truncation
at a UTF-8 boundary. Resolution reads at most nine VMT files, 1 MiB each and 4 MiB
in total, using the existing 64-level/4096-node material parser bounds. Material
paths contain at most 512 bytes. Origin paths, errors, shader names and texture
parameter values also have bounded reports and explicit truncation flags.

The first mounted file wins. A malformed, unreadable or dangling higher-priority
source reports an error; the inspector does not substitute lower-priority source.
Patch includes use the same mounted content lookup. Changed VPK indexes require
remounting the game content before provenance can be trusted.

Shader and texture names are declarations. Inspection does not establish that a
shader is implemented, that its referenced textures exist, or that the material
renders correctly. Twelve common texture parameters are recognized; arbitrary
shader parameters and conditional fallback blocks are not interpreted. Source
inspection reads current files directly; cached previews retain their existing
behavior and update through **Reload material assets**.

When document shutdown begins, **Source…** and **Refresh source** reject new
reads and preserve the last report. **Close** remains available.

MCP exposes the same read-only report through `get_material_source` with a
`material` path. Read, parse, include and decoding failures appear in the report's
`error`; invalid input paths reject the call.

## Inspect the display image's alpha

1. In **Browse materials…**, select a material or enter its **Material path**.
2. Click **Alpha** below the selected preview. Black represents alpha 0, white
   alpha 255 and gray intermediate source values. The preview itself is opaque.
3. Read the source alpha range, display texture path, mip and VMT identity
   beside the image. **Cached display** identifies an already loaded image;
   **Current load** identifies a material not held in the display cache.
4. Click **Color** to restore the unchanged RGBA preview. Neither action applies
   the material or changes the map, camera, selection, history or pending edits.
5. Use **Reload material assets** after changing cached source files. Missing
   textures show **No texture preview**; read/parse/decode errors remain visible.

![Selected material alpha and source identity](images/material-alpha-preview.png)

This inspects decoded alpha from the selected display image and mip, using the
same source as Color. It does not simulate alpha testing, blending, shader
opacity, proxy animation, other texture layers or native appearance. Formats
without stored alpha decode as opaque. Cached pixels retain the texture path
and resolved VMT identity captured when loaded; switching channels does not
combine them with fresh VMT declarations. Uncached materials retain the existing
current-file loading behavior. The virtual texture path is separate from the
current mounted-file provenance shown by **Source…**.

MCP `preview_material` accepts optional `channel:"color"|"alpha"`, defaulting to
Color. It returns the shared image, channel, source identity/cache/mip metadata
and decoded `alpha_range` without starting graphics. Missing images have no PNG
or invented alpha values; read/decode failures reject explicitly.
