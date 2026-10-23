[← Manual](README.md)

# Compiled assemblies

Compile supported contents of saved child VMFs into a parent map.

1. Save the child assembly inside the parent's directory tree.
2. Create an [instance reference](instance-authoring.md).
3. Open **Instance contents…** and click **Refresh instance contents**.
4. Resolve reported path, content or dependency errors.
5. Compile the parent.
6. Inspect **Instances**, **Files**, **Nesting depth** and diagnostics in **Build details…**.

## Supported contents

| Content | Support |
| --- | --- |
| World and ordinary `func_detail` brushes | Checked transformed geometry and textures. |
| Terrain | Supported explicit grids, borders and transformed geometry. |
| Point, spot and sun lights | Checked static or supported built-in appearance settings. |
| Static props | Supported models, skins, transforms and scoped lighting origins. |
| Player spawns | Supported CS:S spawn classes. |
| Stationary brush models | Strictly admitted classes, pivots and initial poses. |
| Overlays | Supported unnamed projections bound to direct child world/detail or terrain faces. |
| Cubemap samples | Supported positions and direct ordinary world/detail manual bindings. |
| Visibility helpers and occluders | Checked static helper classes and supported geometry. |

Compiler support does not imply a full parent viewport preview of each child entity.
Read [instance contents](instance-contents.md) for display limits.

## Rotate a stationary child brush

1. Open the child VMF as a normal map.
2. Select an eligible `func_brush`.
3. Enable **Advanced properties**.
4. Enter `angles` under **Property key**.
5. Enter pitch, yaw and roll under **Value**, then press Enter.
6. For example, enter `15 35 10`.
7. Save the child, return to the parent, and compile.

Angles must be finite and within ±360 degrees per component.
The installed FGD has no labeled rotation field for this class; use the raw property.

Compilation resolves and consumes any origin brush first.
It rotates the stationary body and attached texture coordinates before applying containing reference transforms.
The compiled body has zero entity angles; the child VMF keeps its authored values.

![Stationary brush assemblies with different initial poses in game](images/stationary-child-poses.png)

The foreground assembly combines initial pitch, yaw and roll with a containing reference rotation.

This does not enable moving models, parenting, named child endpoints or script-visible authored-angle preservation.
The parent reference preview remains unavailable for child models.

## Names and parameters

Use **Advanced properties → Fix Up Name** and **Entity Name Fix Up → Prefix** for supported scoped names.
Lighting-origin branches require explicit prefixes at every enclosing level.
Child helper names and references must agree before expansion.
Unsupported replacements, name fixups, scripts or dynamic references fail explicitly.

## Source ownership

The open parent document is authoritative. Child files are read from their checked saved paths.
Builds retain dependency identities; changed or missing children make old output stale or unavailable.
A failed expansion preserves the previous published BSP.

## Limits

- Four helper levels, 256 occurrences and 32 normalized requested paths, plus shared source and geometry budgets.
- Each reference resolves from the saved root parent's tree. Child symlinks and outside paths reject.
- Source and intermediate positions must remain within ±16384.
- General runtime entities, arbitrary parenting, dynamic scripts and unsupported initial poses remain unavailable.
- Child overlay and cubemap bindings cannot cross occurrence scopes or target unsupported owners.
- Inspect the current diagnostic for exact class and material restrictions; admission differs by compiler consumer.
