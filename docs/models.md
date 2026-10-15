[← Manual](README.md)

# Models

View Source static model meshes with their materials and skins, inspect their bounds, and pick them in the map.

1. Click **Entity…**, choose `prop_static`, then **Place entity**.
2. Set **World Model** to a mounted asset, such as `models/props_junk/wood_crate001a.mdl`. The mesh appears automatically when its MDL, VVD and DX90 VTX files are supported.
3. Set **Position**, **Rotation** and **Skin** in the inspector. Skin numbers start at **0**. Source origins and angles drive the same mesh in the editor and agent renders.
4. Click the visible mesh to select it. Click **Frame · F** to inspect its full bounds. Hidden model entities stay hidden.
5. Click **Model details…** for geometry counts, the active skin's materials and transformed bounds. **Reload models** rereads model files after external changes. **Cancel** closes the read-only window.

![Source model meshes with authored positions, rotations and skins](images/model-previews.png)

![Model geometry, skin materials and bounds](images/model-details.png)

Limits: static-prop MDL versions 44/48 with VVD4 and DX90 VTX7 companions, body 0, LOD 0 and scale 1. Unsupported, missing or malformed assets retain a helper; inspection reports the reason. Skeletal animation, flexes, other bodygroups, scaling, entity tint/transparency overrides, baked prop lighting and visual model browsing remain planned. Marquee and volume tests still use point-entity helper bounds. Reload refreshes model geometry and material search paths; changed VMT/VTF contents retain the existing texture cache.

HammeRR limits each companion file to 64 MiB, the model cache to 128 entries and 256 MiB, and a scene to one million model vertices and six million model indices. Entities beyond those budgets retain helpers. These are editor resource limits. Inspection and rendering preserve map properties and Undo history.
