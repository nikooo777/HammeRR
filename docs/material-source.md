[← Manual](README.md)

# Material source

Read the selected material's mounted VMT source, find the directory or VPK that
provides it, and inspect resolved Patch declarations. Water materials also show
a bounded inventory of static material and texture dependencies.

1. Open **Browse materials…** and select a material, or enter its **Material path**.
2. Click **Source…** beside the preview. The new **Material source** window shows
   the normalized VMT path and each source file used for Patch resolution.
3. Under **Mounted source and Patch includes**, read the winning directory and
   file, or VPK index and storage paths. **Resolved shader declaration** and
   named texture parameters show the values after Patch replacement/insertion.
4. For a resolved **Water**, **Water_DX90** or **Water_DX9_HDR** material, scroll
   the resolved-declarations pane to **Water dependencies**. Read its state,
   **Model basis**, issues, typed references and **File** entries. Each inspected
   file shows its byte count, **SHA256** and **Origin**; textures also show
   **VTF header** and **Layout observation**.
5. Read **Selected VMT source** for the original selected file. Parse and include
   failures remain visible beside the raw source; unreadable or invalid UTF-8
   files report an explicit error.
6. Click **Refresh source** to reread the same selected material. Click **Close**
   or press Escape to return to the browser. Inspection does not apply a material
   or change the map, selection, history, camera, previews or cached textures.

![Material source, mounted origin, resolved declarations and Water dependencies](images/material-source.png)

Limits: the raw source display contains at most 64 KiB, with explicit truncation
at a UTF-8 boundary. Resolution reads at most nine VMT files, 1 MiB each and 4 MiB
in total, using the existing 64-level/4096-node material parser bounds. Material
paths contain at most 512 bytes. Origin paths, errors, shader names and texture
parameter values also have bounded reports and explicit truncation flags.

The first mounted file wins. A malformed, unreadable or dangling higher-priority
source reports an error; the inspector does not substitute lower-priority source.
Patch includes use the same mounted content lookup. A lookup that reaches a
changed mounted VPK index reports an error until the game content is remounted.
This applies to new asset reads, uncached catalogs and compilation. **Refresh source** rereads files within the existing mounts;
restart the editor to remount a replaced index. Cached browser entries and
previews are not watched; **Reload material assets** retries their reads but does
not remount content. This index check does not freeze numbered VPK payload
archives or provide a whole-build asset snapshot.

Search-path priority follows the declarations for each path ID, even when the
same directory appears under several IDs. For scalar Patch parameters in the
ordinary declaration report, `insert` adds or overwrites a value; `replace`
changes only a value already present. Insert runs before replace regardless of
section order in the file.

Ordinary shader and texture names remain declarations; their availability is
not checked. Thirteen common texture parameters, including `$albedo`, are
recognized. The Water inventory below adds file checks for its admitted subset;
neither report establishes that a shader is implemented or renders correctly.
Source inspection reads current files directly; cached previews update
separately through **Reload material assets**.

When document shutdown begins, **Source…** and **Refresh source** reject new
reads and preserve the last report. **Close** remains available.

MCP exposes the same read-only report through `get_material_source` with a
`material` path. Read, parse, include and decoding failures appear in the report's
`error`; invalid input paths reject the call.

## Water dependency inventory

**Model basis: CS:S SDK DX90 static Water** identifies an SDK-qualified
declaration model for Water, Water_DX90 and Water_DX9_HDR, including their checked
Patch chains. It is not a claim that the installed shader uses every listed
default. References retain their owning VMT/key, authored or Patch-effective
value, SDK default where applicable, evidence basis and mounted target.

| Reference | Inventory role |
| --- | --- |
| Patch `include` | Material source |
| `$normalmap` | Normal texture; absence selects SDK default `dev/water_normal` |
| `$basetexture` | Optional color texture |
| `$reflecttexture`, `$refracttexture` | Color textures, or role-specific runtime resources `_rt_WaterReflection` and `_rt_WaterRefraction`; omission records those SDK defaults |
| `$envmap` | Cubemap texture, or SDK-default runtime/local binding `env_cubemap` |
| `$bottommaterial` | Compiler material, not a floor texture |
| `$underwateroverlay` | Runtime material; literal `0` disables this reference |
| `%tooltexture` | Editor preview texture, separate from runtime roles |

Runtime resources have no mounted-file lookup. An absent optional reference
adds no inferred file; an explicitly blank required reference does not receive
a default. Bottom and overlay materials are followed as VMTs. A child using
another shader, including Refract or UnlitGeneric, leaves an unsupported partial
inventory rather than assuming its dependencies are known. Inventory states
do not change which materials the compiler accepts.

| Water dependencies state | Meaning |
| --- | --- |
| **Supported static inventory** | The admitted declaration graph was enumerated, required files were inspected and original source identities verified before return. |
| **Unsupported inventory** | A shader, declaration or texture layout falls outside the static subset. Observed rows are partial. |
| **Source unavailable** | A required source is missing, unsafe, unreadable, malformed or changed during inspection. Observed rows are partial. |
| **Inspection limit reached** | A bound stopped inspection. Observed rows are partial; read the issues and counters. |

Non-Water materials retain the ordinary declarations view. Proxies, conditional
or fallback blocks, ambiguous/duplicate declarations, conflicting Patch edits,
cycles and unknown Water fields are outside this subset. Water inventory admits
Patch `insert` only for an absent key or an identical existing value; `replace`
requires an existing key. VTF observations use the existing parser's headers
and framing: known versions/formats, dimensions
up to 4096 per axis, one authored frame and depth one, with a matching 2D or
square cubemap role. They do not decode pixels, verify normal vectors or certify
all native resource layouts.

Limits apply to the entire Water graph, including the original Patch chain:
nine VMTs at 1 MiB each and 4 MiB total; 64 distinct asset lookups including
missing paths; 128 edges; 16 issues; 16 MiB per VTF and 64 MiB total distinct
source payload. Material/graph depth is at most eight; the material parser
retains its 64-level/4096-node bounds. Source tracking allows 8192 unique
physical stamps and 8192 retained lookup-path rows, including repeated rows.
The report exposes its limits and counters.

Hashes identify the complete bytes inspected on first read. Original source
metadata and mounted lookup identities are verified before return; a later
Refresh starts a new observation. Inspection is synchronous, with no interactive
Cancel or latency guarantee. It provides no asset watching, durable source token,
rendering readiness, whole-map dependency inventory or asset packing. Water
assets still need to be available in the game's mounted content; see [Water](water.md).

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
