[← Manual](README.md)

# Sounds

Find a sound, listen to a short preview, and assign a file or sound-script name to an entity.

1. Click **Entity…**, choose `ambient_generic`, then **Place entity**. Other entity fields declared as sounds can use the same browser.
2. Click **Browse…** beside **Sound Name** in the inspector.
3. Search by filename, folder or sound-script name. Choose **All sounds**, **Sound files**, **Sound scripts**, **Used in this map**, a folder or a script file from the group menu. **Previous** and **Next** move through results.
4. Select a result to inspect its codec, size and available audio information. For a script, **Audition variant** chooses which listed wave to preview. You can also enter a sound path or script name and click **Inspect sound**.
5. Set **Preview volume**, then click **Play**. **Stop** stops the displayed preview. Nothing plays automatically. Volume changes apply to the next preview and leave the entity's game volume unchanged.
6. Click **Use sound** to apply the file path or script name in one Undo step. For a script, the audition variant stays a preview choice; the game chooses its own variant.
7. **Cancel**, **Escape** or closing the window discards its pending choice and stops playback started by that window. A replacement choice or playback started elsewhere is preserved.
8. Use **Reload sounds** after changing sound assets or scripts, then cancel and reopen the browser.

![Grouped sound exploration, script variants and audition controls](images/sound-browser.png)

Limits: isolated previews require FFmpeg and ffplay on `PATH`. Browsing, header inspection and assignment work without them. Previews cover at most ten seconds, decoded to stereo PCM at 44.1 kHz. MP3 source duration and rate may be unknown until decoding. The preview uses the selected wave without game mixing, distance attenuation, DSP, pitch ranges, random selection or looping. Voice/sentence synthesis, soundscapes, gender placeholders and script include/base directives and map-specific script overrides are not reproduced. Ambiguous script names are rejected; choose a sound file instead. Missing or malformed script files are reported in catalog warnings; a bad manifest leaves file browsing available.

The browser reads the active game's `game_sounds_manifest.txt`, including `preload_file` and `precache_file` entries. **Used in this map** matches entity fields declared as sounds in the loaded FGD. Finish geometry previews before browsing. Changed maps, selection, definitions, sound assets or geometry previews invalidate pending choices. Reopening the same field resumes a valid pending choice, including one prepared by an agent. An unseen agent change at **Use sound** is shown for review before it can be applied.

HammeRR caps source audio files at 16 MiB, decoded previews at 2 MiB, decoding at five seconds, catalog pages at 100 entries, mounted audio files at 100,000, manifest files at 128, and script entries at 32,768 total (8,192 per file). Script files are limited to 2 MiB each and 32 MiB together, with 128 waves per entry and 32 nested blocks. These are application resource limits. Sound browsing, audition and exported agent audio preserve map source, selection, camera and Undo history.
