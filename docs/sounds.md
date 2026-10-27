[← Manual](README.md)

# Sounds

Find a sound, listen to a short preview, and assign it to an entity.

1. Place `ambient_generic`, or select an entity with a sound field.
2. Click **Browse…** beside **Sound Name**.
3. Search by filename, folder or sound-script name.
4. Select a result. For scripts, choose an **Audition variant**.
5. Set **Preview volume**, then click **Play**.
6. Click **Stop** to stop playback.
7. Click **Use sound**, or **Cancel** to discard the choice.

Nothing plays automatically. Preview volume does not change the entity's game volume.
Use **Reload sounds** after changing files or scripts, then reopen the browser.

## Limits

- Playback requires FFmpeg and ffplay. Browsing and assignment work without them.
- Previews cover at most ten seconds. Source files are limited to 16 MiB.
- The preview uses one wave without game mixing, attenuation, DSP, random selection or looping.
- The game chooses its own sound-script variant.
- Soundscapes, sentences, gender placeholders and map-specific script overrides are unsupported.
- Ambiguous scripts reject. Malformed scripts show warnings; catalog read failures show an unavailable state.
- A `wave` value must be text. A `rndwave` value must be a block of wave entries.
- Playback observation errors show a failed state with the error. **Stop** remains available.
