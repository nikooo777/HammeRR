[← Manual](README.md)

# Generated reflections

Capture scene cubemaps and pack them into the published BSP.

## Generate reflections

1. Place `env_cubemap` near reflective surfaces.
2. Set **Position** and **Advanced properties → Cubemap Size**.
3. Save the map.
4. Open **Build settings…**.
5. Choose **Direct LDR** or **Direct LDR + HDR**.
6. Enable **Generate reflections**.
7. Check the capture runtime readiness message.
8. Close settings, then click **Compile** or **Run Map**.
9. Read capture and verification progress in **Build details…**.

LDR builds capture LDR. Dual-channel builds capture LDR, then HDR.
The BSP is published only after all required captures and verification succeed.

## Configure the capture runtime

1. Click **Configure capture…** in build settings.
2. Enter **CS:S installation override**, or leave it empty for the verified mounted installation.
3. Enter **Steam client override**, or leave it empty for the unique running client.
4. Click **Apply capture runtime**.
5. Check the resolved paths and readiness.

**Cancel runtime edit** discards pending paths. Configuration does not launch the game.
These machine paths stay outside build recipes.

## Reflective props

Materials with `$envmap env_cubemap` need scene samples for scene reflections.
A reflective-prop warning identifies eligible inspected materials when generation is disabled.
Sample records alone contain no captured textures.

## Face assignment

Empty **Brush faces** uses automatic assignment.
Use [Cubemap faces…](cubemap-faces.md) for supported manual bindings.
[Collapse references](instances.md#collapse-references-into-editable-geometry)
can make checked child samples and their direct world/detail bindings editable.
The same preparation preserves sample positions and sizes; manual bindings
follow the fresh authored side IDs. This conversion does not capture reflections.
Supported samples and ordinary world/detail bindings can also come from [referenced assemblies](instances.md).

## Limits

- Fullbright cannot generate reflections.
- Steam, the supported CS:S runtime and the private capture dependencies must be available.
- Capture uses private clients with `-insecure` and `sv_lan 1`.
- Missing channels, unsupported capture scenes, failed verification or cleanup errors stop publication.
- Replaced or rewritten Cloud logs stop capture. Audit limits are 64 MiB total and 8 MiB added during capture.
- **Cancel build** waits for owned capture cleanup.
- Generation captures one reflection bounce, not repeated recursive reflection builds.
- Root binding inspection is unavailable with visible instance references; generation can still use supported assembled samples.
