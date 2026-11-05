[← Manual](README.md)

# Particle effects

Find a named particle system and assign it to an entity.

1. Select one or more `info_particle_system` entities.
2. Click **Browse…** beside **Particle System Name**, or open **Entity → Choose particles…**.
3. Search by system, particle file or material.
4. Select a result to inspect its definition and child systems.
5. Click **Apply** to assign the effect to every captured entity in one Undo
   step, or **Cancel** to discard the choice.

The browser lists each captured source value. Mixed values do not select an
asset automatically.

Use **Reload particles** after changing assets. Reopen the browser afterward.

## Limits

- Up to 64 compatible whole entities per captured choice. Apply rechecks the current manifest and PCF definition before writing any entity.
- Definition metadata only. The editor does not render or animate particles.
- Discovery uses the active `particles/particles_manifest.txt` and mounted content order.
- Supported PCFs use DMX binary 2, format pcf 1.
- Map-specific manifests, unlisted files, include/base directives and external references are unsupported.
- Missing, corrupt or over-budget active data fails explicitly. Ambiguous system names cannot be chosen.
- Limits: 128 manifest files, 16 MiB per PCF, 64 MiB total and 16,384 systems.

<!-- page-nav -->

---

[← Material source](material-source.md) · [Manual](README.md) · [Sounds →](sounds.md)
