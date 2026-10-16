[← Manual](README.md)

# Measurement guides

Store named world-space rulers to check clearances, heights and distances. Each
ruler reports its Euclidean distance and signed X, Y and Z components in Hammer
units. Saved rulers persist in the VMF and appear in the desktop viewport and
MCP renders.

1. Click **Measurements…** beside **Saved views…**.
2. Enter a **Name** and exact **Start** and **End** coordinates under **X**, **Y**
   and **Z**. The readout shows the distance and signed components.
3. Click **Preview**. The gold line joins the endpoints; the red, green and blue
   segments show the X, Y and Z components. Endpoint crosses identify both ends.
4. To place an endpoint on geometry, click **Pick start** or **Pick end** after
   **Preview**. The window hides. Click a visible brush face, terrain triangle or
   supported model mesh in the viewport. The window reopens with the picked
   exact world coordinates and updated distance. A miss leaves the endpoint
   unchanged. Escape or **Cancel picking** stops picking and keeps the preview.
5. Click **Frame** to center the ruler in the current projection. Inspect the
   ruler, then click **Apply** to save it in one Undo step. Save the VMF to retain
   it on disk.
6. Select a saved ruler to change its name or endpoints, then click **Preview**
   and **Apply**. **Delete** removes the selected saved ruler; Undo restores it.
7. Click **New measurement** to enter another ruler. **Show guides** controls
   their visibility for the current session.
8. **Cancel**, window close or Escape without an active endpoint pick discards
   only the preview this window owns
   and leaves applied edits in Undo history. **Review preview** loads an MCP
   preview and explicitly takes ownership before editing, applying or cancelling
   it.

![Surface-picked ruler with exact endpoint controls](images/measurements-surface-picked.png)

MCP uses `get_measurements` and `set_measurement`. The latter supports `preview`,
`review`, `apply`, `cancel`, `delete`, `frame`, `view`, `begin_pick`, `pick_ray`,
`pick_pixel` and `cancel_pick`; inspection provides the
snapshot and ownership token needed for guarded actions. UI and MCP use the same
Session operations and rendered guide mesh.

For MCP surface picks, preview first, then pass the owned token, `endpoint`
(`start` or `end`), `width`, `height` and latest `pick_snapshot` to `begin_pick`.
An optional `camera` matches `render_view` without moving the desktop camera.
Use the returned token for `pick_ray` with `ray_origin`/`ray_direction`, or
`pick_pixel` with integer `x`/`y` and the captured dimensions. Pixel picks use
the pixel center. A successful pick returns a new token and `last_pick` with
the target, brush/side IDs when available, world point and surface normal.
`cancel_pick` returns another token while retaining the endpoints. Apply uses
the latest token after picking ends.

Limits:

- A map supports 128 rulers. Names contain 1–80 characters and are unique without
  regard to case after trimming. Control characters are rejected.
- Both endpoints must be distinct and finite, within ±65536 on each axis.
  Endpoint coordinates retain double precision; rendered lines use the
  renderer's single precision. Readouts display six decimal places.
- Endpoints retain world coordinates from numeric input or surface picks. They
  are not snapped or attached to objects and do not follow later geometry
  changes. Dimension labels in the viewport and configurable 3D workplanes
  remain absent.
- Surface picks follow authored visibility, tool filters and the working
  region. Point-entity helpers and unavailable model meshes supply no target.
  Empty space has no plane fallback. Coincident endpoints reject. Camera, scene
  or viewport changes require another **Pick start** or **Pick end**; cancellation
  still works for an owned stale pick. MCP dimensions range from 1 to 8192.
- Guides cannot be picked. They remain visible through geometry and independently
  of object visibility, tool textures and working-region filtering. Hide them
  with **Show guides**. Framing changes only the camera.
- **Frame** preserves the current projection. Rulers exceeding a planar camera's
  supported view height return an error; switch to **Perspective** or use a wider
  viewport. MCP framing accepts aspect ratios from 0.01 to 100.
- Apply or cancel geometry previews before changing rulers. Changed maps,
  replaced tokens and foreign ownership reject stale actions. Saved measurements
  remain unchanged until **Apply**; edits to the numeric form require another
  **Preview**.
- Rulers use the editor-only `hammerr_measurements` VMF block and never become
  BSP geometry or entities. Unknown extension fields are retained. Unsupported
  versions, malformed records, duplicate IDs/names and excessive imported counts
  produce an explicit error in the window and MCP inspection; original metadata
  is preserved and measurement edits are disabled. Other-editor round trips
  remain unverified.
