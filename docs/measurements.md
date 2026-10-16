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
4. Click **Frame** to center the ruler in the current projection. Inspect the
   ruler, then click **Apply** to save it in one Undo step. Save the VMF to retain
   it on disk.
5. Select a saved ruler to change its name or endpoints, then click **Preview**
   and **Apply**. **Delete** removes the selected saved ruler; Undo restores it.
6. Click **New measurement** to enter another ruler. **Show guides** controls
   their visibility for the current session.
7. **Cancel**, Escape or window close discards only the preview this window owns
   and leaves applied edits in Undo history. **Review preview** loads an MCP
   preview and explicitly takes ownership before editing, applying or cancelling
   it.

![Saved measurement and exact endpoint controls](images/measurements.png)

MCP uses `get_measurements` and `set_measurement`. The latter supports `preview`,
`review`, `apply`, `cancel`, `delete`, `frame` and `view`; inspection provides the
snapshot and ownership token needed for guarded actions. UI and MCP use the same
Session operations and rendered guide mesh.

Limits:

- A map supports 128 rulers. Names contain 1–80 characters and are unique without
  regard to case after trimming. Control characters are rejected.
- Both endpoints must be distinct and finite, within ±65536 on each axis.
  Endpoint coordinates retain double precision; rendered lines use the
  renderer's single precision. Readouts display six decimal places.
- Endpoints are numeric world coordinates. They are not snapped or attached to
  objects and do not follow later geometry changes. Viewport endpoint picking,
  dimension labels in the viewport and configurable 3D workplanes remain absent.
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
