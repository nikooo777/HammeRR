[← Manual](README.md)

# Cubemap faces

Inspect reflection assignments for root-authored faces and author manual bindings to root `env_cubemap` samples. Automatic choices include samples from visible instances.

1. Place an `env_cubemap` sample. Save the map first if it contains visible instance references.
2. Open **Cubemap faces…**.
3. Select a root sample and inspect its face assignments. **Root faces · assembled sample choices** identifies the report's scope.
4. Use **Select face** or **Select bound faces** to inspect targets.
5. Select eligible destination faces in the map.
6. Click **Bind selected faces**, then **Apply**.

**Remove selected bindings** removes only those manual entries.
**Clear all manual bindings** returns the sample to automatic assignment.
**Cancel** discards the owned draft. Use **Refresh** after source changes.
An automatic choice from a child shows its helper and complete instance path.
Removing a root binding may select a child sample.

## Limits

- Manual bindings target supported visible world/detail faces.
- Missing, duplicate or unsupported targets reject before applying changes.
- Bindings do not capture reflection textures. Enable [Generate reflections](reflections.md) for that.
- The report lists root-authored faces; it does not list every face in the assembly. Sample choices and validation use the complete assembled compiler plan.
- Child faces and helpers are read-only from the parent map. Open the child source to edit its own faces, including when it has nested references, then save it.
- Changes to referenced sources or material sources invalidate the draft. With visible instances, this also covers the saved parent. Refreshing an identical assignment does not make an old draft valid; preview it again.
- Unsupported or unavailable instance content rejects inspection and drafts without changing the map.
