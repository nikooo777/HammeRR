[← Manual](README.md)

# Fly speed and navigation defaults

Set the perspective camera's flight speed and optionally save it for startup.

1. Under **Viewport**, enter **Fly speed** in units per second and press Enter.
   Values from 25 to 20,000 are accepted; the factory value is 600.
2. Hold the right mouse button and use WASD/QE to fly. Shift moves four times
   faster. While holding the right button, scroll to change the same fly speed.
3. Open **Navigation defaults…** and compare **Current session** with
   **Startup defaults**.
4. Click **Save current as defaults** to save the current speed for future
   startup. Ordinary input and wheel changes remain temporary.
5. **Apply saved defaults** restores the saved speed in this session.
   **Reload defaults** rereads storage without applying it.
6. **Reset saved defaults** explicitly saves the factory speed. Apply it
   separately if wanted. **Close**, Escape or window close dismisses the dialog;
   completed Save, Apply and Reset actions have already taken effect.

![Current fly speed, startup defaults and explicit actions](images/navigation-defaults.png)

Limits:

- Only fly speed is stored. Camera positions, orientation, look sensitivity,
  ordinary wheel dolly, planar zoom and saved views are separate.
- Speed and defaults operations preserve map data, camera, selection, history
  and owned review/pick state. Moving keyboard focus out of the viewport still
  releases navigation keys and cancels live desktop mouse gestures.
- Malformed, unavailable or changed storage shows **needs attention** and keeps
  the valid current speed. Reload or explicitly Reset to recover. Failed writes
  report an error; Save does not overwrite malformed storage automatically.
- The preference file is bounded to 4096 bytes. Symlinks and nonregular targets
  reject. Save, Apply and Reset check the inspected source and current speed;
  outside changes require another inspection. Save and Reset coordinate with
  other editors through a nonblocking lock. A busy lock preserves the file;
  Reload before retrying. Changed directories require explicit Reload.
  Programs that ignore the lock can still race with publication.
- If a write completes but verification fails, the dialog reports the completed
  write and its error. Current fly speed stays unchanged; Reload to inspect
  storage before continuing.
