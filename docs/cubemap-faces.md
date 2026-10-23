[← Manual](README.md)

# Cubemap faces

Inspect automatic reflection assignments and author manual face bindings.

1. Place an `env_cubemap` sample.
2. Open **Cubemap faces…**.
3. Select a sample and inspect its face assignments.
4. Use **Select face** or **Select bound faces** to inspect targets.
5. Select eligible destination faces in the map.
6. Click **Bind selected faces**, then **Apply**.

**Remove selected bindings** removes only those manual entries.
**Clear all manual bindings** returns the sample to automatic assignment.
**Cancel** discards the owned draft. Use **Refresh** after source changes.

## Limits

- Manual bindings target supported visible world/detail faces.
- Missing, duplicate or unsupported targets reject before applying changes.
- Bindings do not capture reflection textures. Enable [Generate reflections](reflections.md) for that.
- Inspection is unavailable with visible instance references because an authored-only report would be incomplete.
- Supported child manual bindings can still compile through the instance pipeline.
