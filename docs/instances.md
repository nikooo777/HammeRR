[← Manual](README.md)

# Compile referenced brush assemblies

Reuse saved VMFs' world and detail brushes, including nested references, at
several positions and rotations. Compilation expands the referenced geometry.
The editor can resolve a separate, explicitly refreshed
[flat reference preview](instance-contents.md).
Use [Extract selection to instance…](instance-extraction.md) to create a child
from supported brushes already in the parent.

1. Save the parent map. Save a child VMF inside the same directory tree, such as
   `parts/arch.vmf`. Keep the child's visible contents to world brushes,
   ordinary `func_detail` brushes and supported nested `func_instance` references.
   Its world settings do not replace the parent's.
2. Click **Instance reference…**. Set **VMF reference** to `parts/arch.vmf`,
   choose **Position** and **Rotation**, then click **Create**. **Cancel** leaves
   the map unchanged. See [Instance authoring](instance-authoring.md) for browsing,
   editing and file-check controls.
3. To inspect child shapes, enable **Instance contents**, open **Instance contents…**
   and click **Refresh instance contents**. The layer is a flat reference view,
   not a compiled or editable copy of the child.
4. Move or rotate the helper with the ordinary transform controls. Its origin
   places the child's origin. The `angles` property uses Source pitch, yaw, roll;
   brushes and attached texture coordinates rotate together. Keep replacement
   fields empty; named runtime entities are unsupported.
5. Click **Compile**. Unsaved parent edits are included. Open **Build details…**
   in the bar below the toolbar to inspect **Instances**, **Files**, **Nesting
   depth**, world/detail brush counts and each relative dependency's byte count
   and **SHA256**.
6. Recompile after changing any child or descendant. Changed, missing or
   retargeted dependencies make the completed build stale and disable **Play
   built map**. Launch also verifies exact dependency contents before starting
   the game.

![Instance counts, dependency identity and build diagnostics](images/build-instance-dependencies.png)

Limits:

- At most four helper levels, counting a root helper as level one. The complete
  expansion allows 256 visible helper occurrences and 32 distinct normalized
  requested file paths. Repeated references count as separate occurrences.
- Every path, including a reference inside a child, starts at the saved root
  parent's referring directory. If `parts/arch.vmf` references `trim.vmf`, it
  loads the root's `trim.vmf`, not `parts/trim.vmf`. Paths must explicitly end in
  `.vmf` (case-insensitive extension) and stay inside that root directory tree.
  Actual path spelling is preserved for case-sensitive lookup.
  Backslashes normalize to slashes; absolute paths, traversal and symlink child
  components reject. No game-content, maps-root or InstancePath search occurs.
- A symlink parent resolves children beside its referring filename. Its physical
  source and referring-directory identity remain guarded. Save an untitled
  parent first; an output BSP filename cannot establish its source directory.
- Ancestor cycles reject, including aliases through hard links. Repeated siblings
  and shared descendants are allowed within the global limits.
- Runtime entities, parameter helpers, replacements, terrain,
  active child compile regions and unknown semantic metadata reject. Ordinary
  hidden child objects remain excluded; hidden helpers never load their files.
  Fix-up settings cannot introduce runtime name rewriting in this brush-only
  subset.
- Children are bounded to 4 MiB each, 32 MiB total, 32 syntax levels and checked
  object/side/work budgets. Syntax depth is separate from helper nesting depth.
  Each inner-to-outer transform and the final geometry must satisfy normal
  compile bounds, even when a later transform would bring an intermediate back
  into range. Diagnostics identify the root helper and leaf child object rather
  than offering a generated child face as an editable parent face.
- Expansion happens before parent compile-region selection. Geometry intersecting
  the region remains eligible even when its instance marker is outside. Ordinary
  region geometry restrictions still apply. **Generate reflections** with
  instances is unavailable.
- Status and readiness use metadata/alias checks; launch verifies exact bytes.
  Metadata-preserving edits may be detected only by launch or a fresh build.
- Missing, unsupported, changed or cancelled inputs preserve the previous BSP
  and pointfile. A successful build retains its dependency identities separately
  from later failed jobs. External mutation after final verification remains a
  filesystem race; the editor does not lock referenced files.
- In-context editing, automatic search and runtime-name expansion remain
  unavailable. Compiler calls without a saved source context continue to reject
  unresolved instances.

MCP uses the same `create_entity`, `set_entity_properties`, transforms,
`compile_map`, `start_compile` and `run_map` operations. No extra compile switch
is required. Build reports expose optional `stats.instances`; completed jobs
also report dependency freshness. `get_compile_status.launch` and
`inspect_baked_lightmaps` respect the retained build's dependencies.

Instance origins retain the root helper, true leaf filename and original
solid/side IDs. Repeated placements can share that tuple and return multiple
baked-face matches; it is not a unique placement identifier.
