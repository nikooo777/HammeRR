[← Manual](README.md)

# Instance contents

Inspect resolved child geometry without opening each child map.

1. Create a saved [instance reference](instance-authoring.md).
2. Open **Instance contents…**.
3. Click **Refresh instance contents**.
4. Read dependency, content and unsupported-feature diagnostics.
5. Enable **Instance contents** in **Viewport**.
6. Click **Frame instance contents** to inspect the resolved geometry.

The refresh reads committed references explicitly. The visibility switch does not load files.
**Cancel refresh** stops the owned refresh. **Clear** removes the prepared reference display.

![Three transformed references to one arch assembly](images/instance-assemblies.png)

## Limits

- Reference geometry is a checked guide, not a complete game preview.
- Supported brush and terrain geometry can be displayed; child entities have separate count and availability reports.
- Child lights, props, spawns, samples and visibility helpers can compile without a corresponding reference mesh or icon.
- Overlay target geometry can appear without its projected material.
- Source edits or changed dependencies require another refresh.
- Missing or unsupported child content reports an error; stale data is not presented as current.
