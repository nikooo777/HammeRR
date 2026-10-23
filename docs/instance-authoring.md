[← Manual](README.md)

# Instance references

Place or edit a reference to a saved child VMF.

1. Save the parent map.
2. Put the child VMF inside the parent's directory tree.
3. Click **Instance reference…**.
4. Enter **VMF reference**, or use **Browse…**.
5. Set **Position** and **Rotation**.
6. Optionally click **Check file**.
7. Click **Create**.

For an existing helper, use **Edit instance reference…**, then **Apply**.
Use **Instance contents… → Refresh instance contents** to resolve and inspect the child.
The file check confirms presence only; it does not validate child contents.

## Bind light falloff distances

1. Open the child VMF and select a `light` or `light_spot`.
2. Enable **Advanced properties**. Set **Property key** to `_fifty_percent_distance` and **Value** to `$Half`, then press Enter.
3. Repeat for `_zero_percent_distance` with **Value** `$End`. Save the child.
4. In the parent, select the containing `func_instance`. Set unused `replace01`–`replace10` fields to values such as `$Half 64` and `$End 160`.
5. Save the parent, then use **Refresh instance contents** or compile.

Each reference can supply different distances. Each value must be one finite, nonnegative number.
For a positive 50% distance, the near-zero distance must be greater; near-zero `0` selects an automatic distance.
A zero 50% distance requires near-zero `0` and hard falloff `0`.
Invalid distances stop the build, including lights outside a build region.

Unresolved tokens stay as text. They exclude the lamp from authored-lighting preview and prevent standalone compilation, including **Fullbright**.
Reference inspection resolves values and reports counts. It does not preview child lighting.
Only immediate child point/spot distances support these bindings. Environment distances, attenuation coefficients, embedded tokens and ancestor forwarding remain unsupported.

## Bind spotlight cones and focus

1. Open the child VMF and select a `light_spot`.
2. Enable **Advanced properties**. Set `_inner_cone` to `$Inner`, `_cone` to `$Outer` and `_exponent` to `$Focus` using **Property key** and **Value**. Press Enter after each value.
3. Save the child. In the containing reference, set unused `replace01`–`replace10` fields to `$Inner 15`, `$Outer 60` and `$Focus 2`.
4. Refresh the reference contents or compile the parent.

Inner `0` uses 10 degrees. Outer `0` uses the effective inner angle.
A spotlight requires inner ≤ outer ≤ 90 degrees. The pair `180, 180` selects the ordinary point-light mode.
For spotlight cones, Focus must be finite, nonnegative and fit a native float. Point-light mode leaves focus unused.
Ordinary integer fields stay integer; replacement strings can supply fractional compiler values.
Unresolved tokens stay as text and exclude the spotlight from authored-lighting preview. The child cannot compile alone until its tokens resolve.

## Bind sun spread

1. Open the child VMF and select its `light_environment`.
2. Enable **Advanced properties**. Set **Property key** to `SunSpreadAngle` and **Value** to `$Spread`, then press Enter. Save the child.
3. In the parent, select the containing `func_instance`. Set an unused `replace01`–`replace10` field to `$Spread 5`.
4. Save the parent, then use **Refresh instance contents** or compile.

Each reference can supply a finite spread from 0 to 90 degrees. An absent or empty `SunSpreadAngle` property keeps the ordinary default of 0.
Lit builds retain one global sun across the parent and its children.
Unresolved tokens stay as text, exclude the sun from authored-lighting preview and prevent standalone compilation, including **Fullbright**.
Only the immediately containing reference supplies the value. Angles, pitch, embedded tokens and ancestor forwarding remain unsupported.

## Limits

- Paths must end in `.vmf` and stay inside the saved root parent's tree.
- Absolute paths, traversal, child symlinks and self-reference aliases reject.
- Nested references resolve from the saved root directory, not each child's directory.
- Position must fit ±16384; rotation must be finite at native precision.
- The form edits file, origin and angles. Unsupported imported fields can block Apply.
- No automatic file copying, path search, extension inference or child-content editing.
- Source changes stale the form. **Cancel** discards only the owned form.

See [compiled assemblies](instances.md) for supported content and name fixups.
