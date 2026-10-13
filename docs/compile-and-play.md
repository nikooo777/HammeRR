[← Manual](README.md)

# Compile and play

HammeRR includes its own native compiler, so it builds Counter-Strike: Source maps on Linux without the Windows tools. The compiler is **experimental**: compiled maps load and play in CS:S, but visibility and lighting aren't production quality yet. See [Status](status.md).

## Build a playable map

1. Open a sealed map with a player spawn and click **Build settings…** in the toolbar. Settings are available before your first build.
2. Choose the build settings:
   - **Build lighting**: **Fullbright** for a geometry check, or **Direct LDR** to bake lights.
   - **Physics bodies**: **CS:S engine physics** if the map needs physics bodies.
   - **Sampling quality**, for Direct LDR sky and model ambient: **Fast (32 directions)**, **Balanced (128 directions)** or **Detailed (512 directions)**.
3. Click **Close**, then **Compile** to build with the chosen settings, or **Run Map** to build and launch CS:S. Choose an output file if the map has not been saved.
4. The progress bar below the toolbar offers **Build details…** to inspect the build. **Play built map** in that window launches the last successful build again without compiling, as long as the map and settings haven't changed.

## Controls

| Control | Where | What it does |
| --- | --- | --- |
| **Compile** | Toolbar | Builds the current map, including unsaved edits, into a BSP next to the VMF. Unsaved maps ask for an output file. |
| **Run Map** | Toolbar | Compiles, then launches the map in the Linux CS:S client. |
| **Build settings…** | Toolbar | Opens lighting, physics and sky settings before or after a build. |
| **Build details…** | Bar below the toolbar, after the first build | Lighting, physics and sky settings, progress, timings, statistics and diagnostics. |
| **Play built map** | Build details | Launches the last successful build without compiling again. |
| **Cancel build** | Bar below the toolbar and Build details | Stops the build and keeps the previous output. |
| **Game Log** | Toolbar | Shows the game's console and launcher output. |
| **Stop Map** | Toolbar | Stops the game started by the editor. |
| **Leak path** | Toolbar | Shows or hides the red leak line after a failed build. |

Builds run in the background, and closing Build details leaves them running. Output is written in one step at the end: a failed or cancelled build never replaces the previous BSP or pointfile.

![Build settings, statistics, stage, output and diagnostics](images/build-status.png)

## Build settings

Choose these with **Build settings…** in the toolbar, before or after compiling. **Build details…** in the progress bar opens the same window after a build job exists. Agents use `get_compile_settings` and `set_compile_settings` without needing a previous build. Settings apply to every build (Compile, Run Map and agent builds), aren't stored in the VMF, and can't change while a build is running.

![Lighting and sky settings before the first build](images/build-settings.png)

**Build lighting**

- **Fullbright** (default): no lighting. Fastest, for checking geometry.
- **Direct LDR**: bakes direct light from static lights, spotlights and the sun, plus diffuse sky ambient, into lightmaps with shadows. See [Lighting](#lighting).

**Physics bodies**

- **No physics bodies** (default).
- **CS:S engine physics**: builds collision bodies with the game's own Linux collision library, run inside the `hammerr-physics` helper. See [Physics](#physics).

**Sampling quality** (Direct LDR only)

- **Fast (32 directions)** (default), **Balanced (128 directions)** or **Detailed (512 directions)**: how many directions are sampled for [sky ambient and soft sun](#sun-and-sky), and [model ambient](#model-ambient-lighting). More directions improve visibility sampling near narrow openings and make sun shadows less stepped, and take longer. Set **SunSpreadAngle** above zero to soften the sun's shadows.

A previous build counts as out of date once its settings differ from the current ones.

## What gets compiled

### World geometry and visibility

- Brushes become BSP geometry. Out-of-range or invalid objects are reported before building starts.
- The world is split into visibility clusters, with visible and audible sets computed from portals. `func_detail` and brush entities don't split visibility.
- Visibility is **conservative**: the game may draw more than it needs to, never less. Tighter portal clipping comes later.

### Sky

- `tools/toolsskybox` marks 3D sky. Materials with `%compile2dsky` mark 2D sky.
- Sky visibility follows the visible sets, so enclosed or disconnected rooms don't inherit sky they can't see. Hidden and brush-entity sky faces are ignored.

### Material compile flags

- Flags accept `1` or `true` to enable and `0` or `false` to disable, in any letter case.
- Skip faces hide that surface but keep the rest of the brush.
- Non-solid materials stay visible but don't seal the map.
- Hint, areaportal and empty-content brushes are left out. Unsupported flags are listed in the build diagnostics.

### Lighting

With **Direct LDR**:

- Static `light`, `light_spot` and `light_environment` entities are baked in color.
- Opaque world brushes, detail brushes and terrain cast shadows.
- Each face uses its [lightmap scale](textures.md#face-lighting).
- Terrain is lit using its real raised surface, with smooth normals within each patch. The viewport shades terrain with the same normals.

![Baked Direct LDR lighting in Counter-Strike: Source](images/compiled-terrain.png)

#### Falloff

Lights use the classic constant/linear/quadratic attenuation unless you set a positive **50 percent falloff distance**:

1. Select a `light` or `light_spot` and set its color and brightness under **Brightness**.
2. Set **50 percent falloff distance** to a positive distance in Hammer units: this is where the light reaches half brightness.
3. Set **0 percent falloff distance** to a larger distance, where the light is nearly black. Leave it at zero to use twice the 50 percent distance.
4. Set **hard falloff** to `1` to fade smoothly to black exactly at the 0 percent distance, or leave it at `0` to allow a dim tail beyond that distance.
5. Compile with **Build lighting** set to **Direct LDR**.

The percentage distances override constant, linear and quadratic attenuation. The zero-percent distance is an approximation without hard falloff; invalid distances fail the build with an object-specific diagnostic.

#### Spotlights

Spotlights aim with pitch/yaw or at a target entity, with inner and outer cones up to 90° and focus falloff. A 180° cone becomes a point light.

#### Sun and sky

A `light_environment` adds sunlight and soft sky light:

1. Give a world surface the `tools/toolsskybox` material so the light has sky to reach.
2. Click **Entity…**, search for `light_environment`, select it and click **Place entity**. Use one visible sun entity per map.
3. Set the sunlight color and intensity under **Brightness**. Aim it with **Pitch Yaw Roll (Y Z X)** and **Pitch**; `-90` for **Pitch** shines straight down.
4. Set the sky color and intensity under **Ambient**. Set its brightness to zero to turn sky ambient off. An absent or empty ambient property uses half the sun's brightness; a newly placed entity uses its FGD default.
5. Leave **SunSpreadAngle** at `0` for hard shadows, or enter an angle in degrees to soften them; try `5` as a starting point. Open **Build settings…**, choose **Direct LDR** and select a **Sampling quality**.
6. Click **Close**, then **Run Map** to inspect the baked result in CS:S.

![Soft sun shadow on the floor in Counter-Strike: Source](images/soft-sun.png)

Sunlight requires sky in the sampled sun directions, while ambient samples all visible sky directions. A closed room stays dark, and opaque brushes and terrain block both. Larger sun angles widen the transition from shadow to light; higher quality reduces stepping in that transition. The sun's central direction still determines surface brightness. Higher sky quality takes longer and can reach the compiler's lighting-work limit.

#### Limits and errors

- Up to 16 MiB of lighting data and 8,192 baked light records.
- One visible `light_environment` per map.
- **SunSpreadAngle** must be finite and between 0 and 90 degrees. Zero uses one sun ray, regardless of sky quality; positive spread uses the chosen 32, 128 or 512 directions per sun sample.
- Named or animated lights, invalid falloff values and invalid sun angles fail with a diagnostic that names the object.

Not yet: other light types, transparent shadows, smoothing groups and bump maps on brush faces, bounced world light, HDR, baked prop lighting and moving brush lightmaps. Fixed sun samples can leave steps at low quality; adaptive sampling remains unfinished.

#### Model ambient lighting

Ambient probes let models pick up reflected light from world surfaces and the sky.

1. Set up point lights, spotlights or a sun and sky as above.
2. Open **Build settings…**, choose **Direct LDR**, and choose **Sampling quality**.
3. Click **Close**, then **Run Map**. Models receive the compiled ambient light; the editor viewport still does not render model assets.
4. Open **Build details…** to see **Model ambient** probe counts and diagnostics.

![Sky and surface ambient tint on the weapon model in Counter-Strike: Source](images/model-ambient.png)

The compiler places one quantized probe in each open world leaf, avoiding detail solids and space below terrain. It gathers reflected face-average lighting and visible sky radiance, without adding direct point/spot lights twice. Narrow or occupied leaves can lack a safe probe; diagnostics report those omissions.

Limits: face averages smooth over local lighting detail; adaptive probe placement, static-prop occlusion and baked prop lightmaps remain unfinished. These probes do not add bounced lighting to world lightmaps.

### Physics

With **CS:S engine physics**:

- Collision bodies are built for the world, player and NPC clips, brush entities (around their pivots), water and terrain. Static props use the collision that ships with their models.
- The helper checks the collision library's version and the generated geometry, limits its memory and work, and stops after 30 seconds or when the build is cancelled. A missing or unrecognized library fails the build clearly and keeps the previous output. Editing never needs the library.
- Bodies use default surface properties, and brush entities fall back to a one-kilogram mass.

Not yet: material friction and density, authored masses, liquids in detail brushes or brush entities, and broader collision with moving bodies.

### Brush entities and pivots

Brush entities keep their **origin** property. To set the pivot with geometry instead, put one visible `tools/toolsorigin` brush inside the entity: the center of that brush becomes the pivot. The compiler removes the origin brush and writes the rest of the entity relative to it, keeping texture and lightmap placement. The VMF itself is not changed.

These fail with a diagnostic: more than one origin brush in an entity, origin brushes in the world or `func_detail`, entities made only of an origin brush, and models larger than ±16,384 units around their pivot. Hidden origin brushes are ignored.

### Water

1. Create a world brush with `tools/toolsnodraw` on its sides.
2. Give its flat top face a water material, with the face inspector or an agent.

- Connected water with the same height and material forms one region.
- If the map has no `water_lod_control`, one is added to the BSP with distances of 1,000 and 2,000. Existing controls are kept.
- With engine physics, water also gets fluid bodies: objects float.
- Sloped or enclosed tops and conflicting overlapping liquids fail with a diagnostic. Hidden water is left out. Water inside detail brushes or brush entities is reported as unsupported.

Not yet: depth-specific and below-water materials, currents and custom damping.

### Cubemaps

- Visible `env_cubemap` entities become cubemap samples, up to 1,024.
- **Cubemap Size** can be the game default or 1–256 pixels.

Not yet: reflections are not baked. Material binding, face lists and capture are still missing, so samples alone give no reflections.

### Static props

- `prop_static` compiles its model, position and rotation (fractional values included), collision setting, skin, fading and lighting origin.
- Models must be MDL version 44 or 48 and marked `$staticprop`. **Use VPhysics** also needs the model's matching `.phy` file.
- Invalid assets, scaling, lightmap requests and ambiguous lighting origins fail with a diagnostic.

Not yet: baked prop lighting, prop shadows and exact placement in the BSP tree.

### Terrain (displacements)

- Terrain from imported VMFs compiles, at power 2, 3 and 4, with blend alpha and its no-physics, no-hull and no-ray flags.
- Terrain doesn't seal the map, and its parent brush doesn't become a solid box.
- With engine physics, terrain gets collision meshes.
- Neighboring patches of the same power that share a full edge are linked to each other in the BSP. Build details and agents report the number of linked pairs. Ambiguous edges, different powers and mismatched surfaces stay unlinked, with a diagnostic.
- Direct LDR lights terrain of every size, with smooth normals within each patch, and raised terrain shades the world beneath it. Terrain with unlit materials needs **Fullbright** for now.

Not yet: creating or editing terrain in HammeRR, subdivision, removed triangles, terrain on entities or detail brushes, smoothing normals across neighboring patches, LOD between different powers or partial edges, and neighbors that touch only at a corner.

## Coordinate limits

Brush geometry and entity origins must stay within **±16,384 units**. Objects outside that range are reported before building, and the existing output is kept. The editor itself allows larger coordinates for editing, but the playable Source world doesn't extend further.

## Leaks and pointfiles

A map needs sealed world geometry and at least one point entity in open space. When a build leaks, HammeRR writes a `.lin` pointfile, keeps any existing BSP and draws the leak path in red. Toggle it with **Leak path**. Editing hides the path until the next build.

To follow a leak, open **Pointfile…**:

1. After a leaking build, its path is already there. To open another `.lin` file, type its path or click **Browse…**, then **Load**.
2. **Frame path** shows all of it. Type a point number and click **Go**, or step with **Previous** and **Next**. Perspective views look along the path; planar views center on the point.
3. **Unload path** removes the overlay without deleting the file.

- Loading and navigating never change the map.
- Editing the map hides an external path until you reload it. A new build replaces it, and opening another map clears it.
- Files may hold 2–65,536 points and be up to 2 MiB.

| Pointfile controls | Leak path drawn through the map |
| --- | --- |
| ![Pointfile loading, framing and point-by-point navigation](images/pointfile.png) | ![Red pointfile path drawn through geometry](images/pointfile-viewport.png) |

## Playing

**Run Map** and **Play built map** launch the Linux CS:S client with `-insecure`.

- Each run uses its own uniquely named copy of the BSP, which is removed when the game exits. Its logs stay until the next run or until the editor closes.
- Closing the editor stops the game it started. Your other maps and game processes are never touched.

**Play built map** shows which BSP it will launch. It is disabled, with an explanation, when the map changed since the build, the output is missing, the build settings changed, no Linux client is installed, or a build or game is already running. A failed rebuild keeps the previous successful build playable if the map hasn't changed since.

## Standalone compiler

```sh
./target/release/hammerr-compile map.vmf --game /path/to/cstrike/gameinfo.txt
```

| Option | Purpose |
| --- | --- |
| `-o <map.bsp>` | Output file. Defaults to the VMF's name with `.bsp`. |
| `--lighting fullbright\|direct_ldr` | Lighting mode. Defaults to `fullbright`. |
| `--physics none\|engine` | Physics bodies. Defaults to `none`. Needs `hammerr-physics` next to the compiler. |
| `--sky-quality fast\|balanced\|detailed` | Sky ambient sampling for `direct_ldr`. Defaults to `fast`. |

To try it on a generated test room with both teams' spawns:

```sh
cargo run -p compiler --example room -- room.vmf
./target/release/hammerr-compile room.vmf --game /path/to/cstrike/gameinfo.txt
```

Copy `room.bsp` to `cstrike/maps/` and run `map room` in the CS:S console.
