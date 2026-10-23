[← Manual](README.md)

# Measurement guides

Save named rulers for clearances, heights and distances.

1. Open **Measurements…**.
2. Enter **Name**, **Start** and **End** coordinates.
3. Click **Preview**.
4. Use **Pick start** or **Pick end** to choose a visible surface point.
5. Click **Frame** to inspect the ruler.
6. Click **Apply** to save it.

![A saved height guide at the courtyard doorway](images/doorway-measurement.png)

The gold line is the distance. Red, green and blue segments show signed X, Y and Z components.
Use **New measurement**, **Delete** and **Show guides** to manage rulers.

## Follow an entity

1. Preview a ruler.
2. Enter `entity:<id>` beside **Attach start origin** or **Attach end origin**.
3. Click that attachment control, then **Apply**.

Use **Attach start local** or **Attach end local** to retain an offset in the entity's rigid local frame.
**Detach start** or **Detach end** freezes a resolvable point.
**Use start world** or **Use end world** explicitly replaces an unavailable attachment with stored coordinates.

## Limits

- Rulers persist in the VMF; visibility is session-local.
- Surface picks remain world points, not attached surface references.
- Local attachments follow origin and rotation, not model scale, parenting or animated bones.
- Missing or ambiguous targets show unavailable measurements rather than invented distances.
- Up to 128 measurements; names are limited to 80 characters.
- **Cancel** discards only the owned preview. Applied edits remain undoable.
