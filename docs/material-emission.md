[← Manual](README.md)

# Material emission

Bake light from an opaque world, detail or stationary brush-model surface.

1. Create an ASCII file in mounted game content, for example `scripts/room.rad`.
2. Add one material, RGB color and brightness per line:

   ```text
   lights/panel 255 192 128 255
   ```

3. Open **World settings…**. Set **Emission file** to `scripts/room.rad` and press Enter.
4. Click **Apply properties**. **Cancel** discards the draft.
5. Open **Build settings…** and choose **Direct LDR** or **Direct LDR + HDR**.
6. Choose **Sampling quality**, then **Compile**.
7. Open **Build details…**. Check **Diagnostics** for the emitted face count and errors.

![A white panel casts red baked light onto a surface and a prop in CS:S](images/material-emission-native.png)

Blank text or **Remove override** turns emission off. **Fullbright** checks the
file and sources but does not bake emission. Unused records are allowed;
no rendered source faces means no emission is baked.

Brightness is uniform per surface area. Doubling area or brightness doubles
emitted power. Texture size, UV scale and source reflectivity do not set power.
RGB values are 0–255; brightness must be finite and nonnegative.

Use separate records for different channels:

```text
ldr: lights/panel 255 192 128 255
hdr: lights/panel 128 192 255 510
```

An unprefixed record supplies both channels. Blank lines and whole-line `//`
comments are accepted. Duplicate material/channel assignments reject.

## Use a stationary brush source

1. Select an unnamed, unparented `func_brush`.
2. Enable **Advanced properties** and set **Origin (X Y Z)**.
3. Set **Shadows** to **Yes**.
4. Set **Pitch Yaw Roll (Y Z X)** for an initial rotation, if needed.
5. Apply a declared emission material to its source faces, then compile.

The source uses its compiled initial pose. Other solids in the same model
still block light. Supported child models use their assembled pose once.
See [stationary shadow sources](brush-texture-shadows.md#cast-from-a-stationary-root-brush-model)
for the shared admission rules.

![A rotated stationary panel casts red light onto a floor and prop in CS:S](images/stationary-emission-native.png)

## Limits

- Sources: drawn opaque flat world, `func_detail` or eligible **Shadows → Yes**
  `func_brush` faces. Moving, unrequested and MDL models, terrain, liquid,
  transparent and merged sources with multiple original contributors are
  unavailable. Sources are checked before compile-region filtering.
- Existing supported baked receivers and diffuse bounces receive emission.
  Dynamic models receive it through indexed ambient cubes only. The file does
  not add shader glow or native direct-light records.
- Paired HDR requires a real authored point, spot or environment light.
  Zero brightness is allowed. The engine requires a nonempty HDR light table.
- Sampling can miss thin blockers or apertures between samples. **Detailed**
  uses smaller cells and more work. Exceeded budgets and unconditioned
  source contacts stop the build; quality does not reduce automatically.
- At most 262,144 cells per receiver, 48 subdivisions, 65,536 source sides and
  262,144 source triangles.
- File: 64 KiB, 4,096 records, 1,024 bytes per line. Path: 1,024 ASCII bytes,
  mounted and relative. Includes, traversal, implicit file searches and
  child-owned emission files are unavailable. Root declarations apply to
  supported assembled child world/detail and stationary brush-model faces.
- VRAD numeric brightness parity is unavailable.

See [MCP emission](mcp.md#material-emission) for the shared property workflow.
