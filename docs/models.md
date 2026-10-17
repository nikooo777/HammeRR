[← Manual](README.md)

# Models

View Source model meshes and checked reference poses with their materials and skins, inspect their bounds, and pick them in the map.

1. Click **Entity…**, choose `prop_static` for static scenery or `prop_dynamic` for skeletal/weapon models or bodygroup changes, then **Place entity**.
2. Click **Browse…** beside **World Model**. Search by model name or folder, then choose **All models**, **Used in this map** or a folder from the group menu. **Previous** and **Next** move through the results.
3. Select a model to see its isolated preview, size and triangle count. Click **Show materials** to expand its material paths. Skeletal models show **Reference pose**; animation sequences are not played. Change **Skin** to inspect another texture variant; numbers start at **0**. If the model has bodygroups, use their named dropdowns; **None** hides that group’s mesh. You can also enter a mounted model path and click **Preview model**.
4. Click **Use model** to apply its path, skin and bodygroups together. **Cancel** or **Escape** closes the browser without changing the entity. The map and camera stay unchanged while browsing. Reopening the same entity’s browser resumes a valid pending choice, including one prepared by an agent.
5. The mesh appears automatically when its MDL, VVD and DX90 VTX files are supported. You can still type a path directly into **World Model**.
6. Set **Position**, **Rotation** and **Skin** in the inspector. Skin numbers start at **0**. For a dynamic model, set **Model scale** to resize the mesh uniformly about its origin. Use **Tint** and its color picker to multiply the model textures by an RGB color; white keeps their original colors. **Cancel** abandons the picker, while **Apply** writes one Undo step. Source origins, angles, scales and tints drive the same mesh in the editor and agent renders.
7. Click the visible mesh to select it. Area/volume rules test its model bounds. Click **Frame · F** to inspect its full bounds. Hidden model entities stay hidden.
8. Click **Model details…** for geometry counts, the active skin’s materials, bodygroup choices and transformed bounds.
9. If the model has attachment points, choose a name under **Attachment**. Its **Position** is shown in world coordinates for the displayed reference pose.
10. Turn on **Show attachment axes** to mark that point in the viewport: X is red, Y green and Z blue.
11. Click **Frame attachment** to focus the viewport on it. Inspection and guides leave the map and Undo history unchanged; framing moves the camera.
12. For changed VMT/VTF files or newly added model materials, use **Browse materials… → Reload material assets**. The viewport, model browser and **Show materials** paths refresh without decoding model geometry again. Material read or search errors show missing textures and an explanation while the mesh and bounds remain available. Repair the files and reload again.
13. **Reload models** rereads model files after external geometry changes. It cancels pending snap-to-grid previews; preview again after reloading because model bounds may change. **Cancel**, **Escape** or closing the window clears its own attachment guide and closes the window. An agent’s replacement guide is preserved.

Use [Surface placement](placement.md) to drop models onto floors or ceilings, or align them against a picked surface using their displayed mesh vertices.

![Searchable model selection with an isolated skin preview](images/model-browser.png)

![Source model meshes with authored positions, rotations and skins](images/model-previews.png)

![Dynamic model tint previews](images/model-tints.png)

![Model geometry, skin materials and reference-pose details](images/model-details.png)

![Reference-pose model selection](images/reference-model-browser.png)

![Named bodygroup selection](images/bodygroup-model-browser.png)

![Named attachment inspection and viewport controls](images/model-attachments.png)

![Framed reference-pose attachment axes](images/model-attachment-guide.png)

## Preview opacity

Use static transparency on a dynamic model without changing its placement or
selection bounds.

1. Select a supported `prop_dynamic` or other dynamic model entity.
2. Set **Render mode** to **Texture · opacity**.
3. Set **Opacity** from 0 to 255 with its slider or numeric input. Zero hides
   the mesh; 255 keeps the material's original alpha. **Tint** remains independent.
4. Choose **Normal** to restore ordinary material rendering; Normal ignores
   Opacity. Use Undo to restore a property edit.
5. Open **Model details…** to read the opacity summary or an unsupported-appearance
   diagnostic. Unsupported modes or Render FX retain a diagnostic helper.

![Dynamic models using authored opacity](images/model-opacity.png)

Opacity preview applies in **Textured** mode with **Render FX → None** only.
It uses material alpha, depth testing and back-to-front batch-center sorting;
intersecting/cyclic transparent geometry and triangle-level sorting remain
outside this preview. Asset-only browser images retain original materials.
It does not reproduce native runtime effects, fades or animation. Literal FGD
model guides keep their independent flat editor appearance.

## Scale a dynamic model

1. Select a supported dynamic model entity, such as `prop_dynamic` or `prop_physics`.
2. Set **Model scale** to a positive value from **0.015625** to **64**. For example,
   **0.5** halves the mesh and **2** doubles it about the entity origin.
3. Click the resized mesh, select it with an area or volume, or use **Frame · F**.
   These operations use the scaled mesh or its transformed bounds.
4. Open **Model details…** to read the effective scale, world bounds and attachment
   positions. Ordinary attachment axes scale with the model; world-aligned
   attachments keep world axes while their positions move with the mesh.
5. Use **Undo** to restore the preceding scale. **Reload models** keeps the authored
   value when it rereads changed model files. The isolated browser preview stays
   at scale **1**; choosing a different model preserves the entity’s scale.

![Dynamic meshes with authored uniform model scale](images/model-scale.png)

The absent value is **1**. Values are evaluated at native float precision while
the original property string is preserved. The range, 64-byte property limit
and transformed mesh bounds within ±65536 are editor numeric limits. Imported
unsupported values remain visible for correction and retain a diagnostic helper;
they are not clamped. `prop_static`, native detail records and literal FGD model
guides retain their unit-scale restrictions. Dynamic runtime entities keep their
authored `modelscale` property in the BSP, but its native effect depends on the
entity class. Animation, timed scale inputs, physics/collision and native
appearance parity are not evaluated by this preview.

Limits: MDL versions 44/48 with VVD4 and DX90 VTX7 companions and LOD 0. Unsupported, missing or malformed model geometry retains a helper; inspection reports the reason. Attachment positions follow default local bone poses; animated, procedural/IK and included-model virtual attachment results are not evaluated. Attachment parenting and placement tools remain planned. Animation playback, procedural/IK poses, flexes, other render modes and baked prop lighting remain planned. Tint preview supports integer RGB channels from 0 to 255; an authored fourth component is preserved when editing RGB and does not control opacity. Nonwhite static-prop tints, nonnormal render modes and unsupported Render FX are rejected by the current compiler output and retain a helper; these are writer limits. `prop_static` requires a model built as a static prop and body 0; use a compatible dynamic entity for reference poses, scale or other bodygroup selections. Entirely blank choices show an editor helper, which remains selectable. Bodygroup choices and supported scales change displayed geometry; preview mesh hits and bounds do not validate engine collision shapes. Marquee and volume selection use loaded models’ transformed asset bounds; point picking intersects triangles even at zero opacity. Hidden models are excluded from region matching. Unsupported assets retain their existing helper bounds. **Reload models** refreshes model geometry; **Reload material assets** refreshes cached material contents and repeats material-path lookup using the cached MDL directories. If no candidate exists, the existing bare-name lookup remains visible in **Show materials** and missing images use the missing-texture display. Search failures expose a material-resolution error and discard the previous display bindings. See [Reload changed materials](textures.md#reload-changed-materials).

HammeRR limits each companion file to 64 MiB, the model cache to 128 entries and 256 MiB, and a scene to one million model vertices and six million model indices. Reference pose decoding supports up to 256 bones and three weights per vertex. Entities beyond those budgets retain helpers. These are editor resource limits. Inspection and rendering preserve map properties and Undo history.

The browser edits a single point entity’s **World Model**, **Skin** and bodygroups. Other model-reference fields and batch-property drafts retain text entry. Finish geometry previews before browsing. If the map, selection, definitions or a geometry preview changes, cancel and reopen the browser. Folder searches list mounted paths, including unsupported assets; selecting one explains why it cannot be previewed. Catalog pages contain at most 100 models. Isolated images are limited to 64–1024 pixels per side.

Content-directory read failures show **Model catalog unavailable** and clear the
displayed results. Repair the content path and edit the search field to retry.
Failed discovery is not cached as a successful empty result.
