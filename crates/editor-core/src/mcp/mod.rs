//! MCP server exposing the editor to agents. Each tool is a thin wrapper over
//! one [`Session`] operation, the same ones the UI calls, so agent edits show
//! up live in the editor window and share its undo history.

use std::collections::BTreeMap;

use std::io::Cursor;

use std::path::{Path, PathBuf};

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

mod close;
mod http;
mod params;
mod rendering;
mod stdio;
mod tools;
mod workspace;

pub use close::{TerminalClose, TerminalOwner, TerminalUi};
pub use http::{
    ConnectionReceipt, HttpServer, HttpTerminalResponse, TerminalAttempt, TerminalResponseReceipt,
    spawn_http, spawn_workspace_http,
};
use params::*;
use rendering::{Offscreen, OffscreenState};
pub use stdio::{
    StdioAttempt, StdioFenceError, StdioOutput, StdioReceipt, StdioResponseFlushed, StdioTicket,
};
pub use workspace::{
    Workspace, WorkspaceOwner, WorkspaceServer, WorkspaceUi, WorkspaceUiFuture,
    serve_workspace_stdio,
};

use base64::Engine;

use glam::{DVec2, DVec3, Vec3};

use map_core::edit::{self, FaceTarget};

use map_core::query::{self, Hit, Ray};

use map_core::{Document, ObjectId, Plane};

use render::{Camera, GpuContext, GpuScene, Projection, Renderer, ViewOptions};

use rmcp::handler::server::router::tool::ToolRouter;

use rmcp::handler::server::wrapper::Parameters;

use rmcp::model::{CallToolResult, ContentBlock, ServerCapabilities, ServerConfig};

use rmcp::{ErrorData, ServerHandler, ServiceExt, schemars, tool, tool_handler, tool_router};

use serde::Deserialize;

use serde_json::{Value, json};

use source_formats::fgd::{Class, ClassKind};

use source_formats::vmf::{Entity, Solid};

use crate::game;

use crate::identity::{id_string, parse_id};

use crate::runtime::{
    CompileSubmission, DocumentHandle, DocumentOperation, LaunchSubmission, SharedSession, lock,
};

use crate::session::{
    Actor, ConnectionFields, ConnectionRecord, ObjectFilter, PropertyChange, PropertyTarget,
    SelectMode, Session,
};

const DEFAULT_MATERIAL: &str = "dev/dev_measuregeneric01";

const DEFAULT_RENDER_SIZE: (u32, u32) = (1024, 576);

const MAX_RENDER_SIDE: u32 = 2048;

const INSTRUCTIONS: &str = "\
inspect_baked_surfaces and set_baked_surfaces share Build settings… / Build details… → Baked surfaces…. Inspect before preparing, then retain the opaque preparation token for cancel/clear/show/hide/lighting/frame and bounded1–64-row pages. Prepare reads actual final world/detail, terrain and initial-pose brush surfaces from the successful owned lit BSP; background preparation is cancellable. Lighting changes channel, exposure_ev and stored-ID style_gains atomically. Style0 defaults to1 and accepts an explicit gain; omitted nonzero styles are off. Directional reconstruction is automatic; inspect_baked_lightmaps retains exact single-face/map0–3 inspection. Absent channels, unsupported inputs and stale original sources refuse explicitly. Hide preserves options, Clear resets them, and only Frame changes the camera. \
inspect_compiled_visibility reads owned compiler world-leaf/cluster joins, original portal rings and separate native PVS/PAS pages. Start with a summary, retain build_token for explicit targets and pages. Target {kind:world_light,channel:ldr|hdr,ordinal:N} uses a POINT/SPOT/SURFACE record's stored source cluster from the same owned BSP, without position lookup. The target retains its requested identity; world_light reports channel/ordinal/cluster and selection describes that cluster. Directional sky, unknown kinds, unbaked channels, invalid ordinals and invalid source clusters reject. UI: Compiled lights → Inspect source visibility. set_compiled_visibility_overlay destination mode accepts the same light target for complete PVS/PAS bounds; selected guides and frame_compiled_visibility stay leaf/portal only. Guides and framing require current output; intact historical records remain readable. Failed guide changes preserve the previous guide. PVS/PAS is potential visibility/audibility, not native light winners, falloff, ray occlusion or exact finite-area visibility. Native bounds are not exact leaf cells. Changed output clears data and stale guides are suppressed. Omit the overlay target to clear it. No file-path input or authored selection mapping. \
HammeRR, a map editor for Counter-Strike: Source (Valve Source engine, Hammer-compatible VMF files). \
The desktop HTTP server shares the live editor session; a stdio server owns a private \
session. Edits use the session's shared undo history.\n\
Coordinates: Z is up, units are Hammer units (about an inch). A player is 32x32x72 \
(crouched 36), steps are up to 18 high, doors are typically 56x112, walls 8-16 thick. \
Keep brush coordinates inside +-16384. set_spatial_settings configures the shared grid \
(0.125 to 4096 units), snapping, workplane elevation and brush depth.\n\
Objects are addressed as 'solid:<id>' (world brushes) and 'entity:<id>' (point or brush \
entities; a brush entity owns its brushes). Tools that take ids default to the current \
selection when ids are omitted, and creation tools select what they create.\n\
Materials are paths under materials/ without extension, e.g. 'tools/toolsnodraw', \
'tools/toolsskybox', 'dev/dev_measuregeneric01'; find more with search_materials. \
Sprite Name fields are model-resource references: a material choice must be authored \
as 'materials/<catalog-name>.vmt', retaining nested directories. Existing supported \
'sprites/<basename>.spr' values remain unchanged until an explicit edit.\n\
Verify work visually with render_view (focus on objects, or an explicit camera) and \
pick_at_pixel to identify what is at a point in a rendered image. Save with save_map. Capture get_map_info.replacement_review and pass expected_review when replacing a reviewed map through new_map, open_map, open_recent_map or restore_recovery_snapshot. Pass its generation as expected_generation to delayed save_map, start_compile, compile_map or run_map requests to reject a replaced buffer. \
set_camera selects perspective/top/front/side; planar cameras accept center and ortho_height. \
frame_selection frames the selection or whole map. Named cameras persist through save_camera_view, get_camera_views, restore_camera_view, rename_camera_view and delete_camera_view; use generation/revision guards for stale-state protection. draw_block uses plane coordinates \
(top XY, front XZ, side YZ), shared snapping and depth; preview=true leaves the map \
and undo history untouched. Inspect get_map_info.block_preview and cancel with cancel_block_preview. \
transform_objects applies exact move/rotation/positive scale, brush reflection and single-axis shear, with an optional world-space pivot \
(default: selection bounds center) and independent texture_lock (move/rotation) and texture_scale_lock (stretch); omitted scale lock inherits movement locking. Rotation is world X/Y/Z, in degrees. \
space: local uses the first selected object's frame; preview: true creates a shared transform draft. \
preview_snap_to_grid aligns the world selection bounds minimum with the shared grid by default; mode=each_object snaps each whole object independently. Inspect grid_snap and use the normal transform commit/cancel. \
set_transform_tool configures move/rotate/scale handles, pivots and angle snap. Inspect \
get_transform_handles, begin_transform_drag, then update_transform_drag with total amounts \
or viewport pointers. Apply with commit_transform_preview or cancel_transform_preview. \
select_area selects objects or faces through a pixel rectangle in any projection; choose \
contained/intersecting and replace/add/remove/toggle. Use preview: true, inspect \
get_area_selection and render_view, then commit_area_selection or cancel_area_selection. \
select_volume queries a world-space box, with shared candidate previews, framing and token-guarded commit/cancel. \
set_paint_tool configures a circular selection brush; begin_paint_selection starts a visible-hit stroke, \
update_paint_selection extends its pixel path, and get_paint_selection exposes candidates and limits. \
Use its token with commit_paint_selection or cancel_paint_selection; render_view shows the shared preview. \
For sampled material/projection strokes, sample_face_texture then begin_texture_stroke, update_texture_stroke, \
get_texture_stroke and commit_texture_stroke or cancel_texture_stroke share one undoable preview with the UI. \
get_brush_components exposes precise faces/vertices/edges with stable incident-face IDs and current viewport picking scope. set_component_picking captures a brush scope; pick_brush_components queries the same screen candidates as the UI before guarded selection. select_brush_components, preview_component_move, commit_component_move and cancel_component_move share convex topology-preserving edits with the UI. Use the latest snapshot and preview token; textures default to world anchoring, with optional texture_lock to preserve corresponding corner UVs. \
For a roof ridge, select one quad face per brush, inspect crease_choices, then preview_component_crease with opposing edge pairs and rise; apply/discard through commit_component_edit/cancel_component_edit. \
select_faces switches to shared face selection; edit_faces changes material, UVs and fitting. \
Use preview: true for a draft, then commit_face_preview or cancel_face_preview. \
Use get_overlay_state and preview_overlay to fit a new overlay to one explicit visible world/detail quad, then apply_overlay or cancel_overlay with its owned token. review_overlay explicitly transfers a shared draft to a new ownership token. \
preview_overlay_refit keeps an existing single-face binding; browse_overlay_targets and preview_overlay_rebind explicitly move that same overlay to a different eligible face while preserving settings. Both use the same owned Review/Apply/Cancel operations. \
get_face_selection inspects selected faces and pending texture changes. browse_materials \
provides grouped catalog metadata; set_material_favorite shares browser favorites. \
find_material_faces creates an exact map/selection material report with explicit hidden handling. \
Use its token with select_material_faces or replace_materials; get_material_search paginates results. \
project_face_group fits a continuous tiled texture across coplanar faces or projects along the current view. \
propagate_face_texture folds a sample through selected connected faces and rejects disconnected targets or incompatible loops. \
preview_hollow builds inward/outward wall shells with shared geometry and cavity inspection. Get the stamp from get_hollow_preview, inspect with look_inside_hollow, then commit_hollow or cancel_hollow using the token. \
clip_brushes cuts brushes by a normal/distance, three points, or a snapped planar line. \
Use preview: true, inspect get_clip_preview and render_view, then commit_clip_preview \
or cancel_clip_preview. Keep front (arrow side), back, or both; displacements cannot be clipped. \
get_clip_handles and set_clip_handle_options expose shared cut-plane movement/rotation. Begin with begin_clip_drag and the latest clipping snapshot; update_clip_drag takes the returned token and pointer or total amounts. finish_clip_drag only finishes positioning; commit_clip_preview explicitly applies the cut. cancel_clip_drag restores the starting plane. \
Discover entity classes with search_entity_classes and get_entity_class. create_entity \
applies FGD defaults, including light brightness; set_entity_properties validates declared \
types. Custom classes and keys remain supported. Placed lights are stored in the VMF; \
inspect_entity_graph queries bounded committed authored I/O nodes/edges from explicit roots, direction and depth; use its snapshot for select_entity_graph or frame_entity_graph. Exact named fan-out and !self are followed; runtime/class/prefix-pattern candidates remain explicit unresolved records. Search and pages affect presentation only; selection/framing use the complete bounded traversal. \
get_entity_connections inspects outgoing and incoming links, parsed fields and warnings. \
get_connection_choices provides map target names and FGD inputs/outputs. Begin a shared form \
with begin_connection_edit, change fields with update_connection_draft, then commit_connection_edit \
or cancel_connection_edit. delete_entity_connection removes one outgoing index. Use generation \
and revision guards when starting an edit or deleting from a previously inspected list. \
get_entity_references also inspects FGD property links, parent references and input parameters. \
Use begin_entity_rename, update_entity_rename, commit_entity_rename or cancel_entity_rename \
for safe naming: exact links update atomically, shared names require rename_group, and \
unchanged ambiguous references require accept_review after inspecting the preview. \
get_property_inspector reports world or common entity fields, stored/default values and mixed flags. World Native direct emission (_hammerr_emission_native_direct) is an intrinsic default-off exact 0/1 policy beside Emission file, not a build option or recipe field. Enable it through the same property draft only with a nonblank valid Emission file; file/policy edits validate together atomically. It adds per-constant-RGB integration-triangle centroid point approximations for native selected/folded runtime direct lighting and replaces only intrinsic direct emission in indexed probes. Sampled maps/VHV/detail direct and reflected ambient remain. Native slot/angular/folding approximations and inherited static-cache additions on dynamic-lit full-VHV props prevent universal appearance parity. Fullbright validates without emitting records; static previews do not simulate native selection. Inspect actual per-channel native_surface_lights and native_policy under stats.material_emission. \
Use begin_property_edit (world:true or entity ids), update_property_edit with a changes list, \
then commit_property_edit or cancel_property_edit using the returned token. Default resets \
write the FGD value; remove deletes the override. All targets apply in one undo step. \
get_object_report searches names/classes/materials/types/visibility/bounds; select_object_report \
selects all matches and can frame them. set_object_visibility hides/shows/isolates/restores \
saved VMF visibility. Hidden objects and child brushes are OMITTED from compilation. \
get_compile_settings and set_compile_settings select shared build lighting (fullbright/direct_ldr/direct_ldr_hdr) and physics (none/engine); direct_ldr_hdr keeps LDR and HDR channels separate, reports HDR counters in stats.hdr and aggregate work in lighting_total_work, and warns when HDR light samples/world lights/ambient are empty. Engine physics uses the verified installed CS:S Linux64 library in an isolated worker for world, clip, brush-model and horizontal world-fluid bodies; terrain contact meshes are supported and static props use model assets; world, terrain and brush-model triangles retain per-face surface properties through a checked table of at most 127 names including default. Brush-model base masses use the dominant emitted surface with collision volume and density, or shell area, thickness and density, capped at 50,000 kg; engine mass scale and overrides remain effective. Custom density controls and detail/model fluids remain unavailable. \
set_compile_settings also accepts refine_ambient_probes (default false), requiring direct_ldr/direct_ldr_hdr plus balanced/detailed sampling. It evaluates at most twice the baseline candidate gathers and may replace only the final probe per leaf, preserving earlier anchors/counts/spacing; LDR/HDR select independently and uniform/tied scores keep baseline. All extra work remains charged and cancellable. ambient_probe_refinement reports availability/limits, not a general quality or native-pixel guarantee. Invalid combinations reject atomically; recipes and job settings retain the option. set_compile_settings also accepts reflect_static_props (default false), requiring bake_static_prop_vertices plus a lit mode. Invalid combinations reject atomically without enabling prerequisites; disable reflect_static_props in the same update when disabling a prerequisite. Eligible opaque or strict cutout VHV props provide reflected style-0 sources from actual root-LOD triangles and selected skins/UVs. The shared zero-to-four bounce count uses previous-depth world/prop snapshots; ambient/details sample final output. Runtime direct styles stay separate; switched indirect ambient remains unsupported. set_compile_settings also accepts bake_static_prop_vertices (default false) for a stored static style-0 prop vertex base. Supported effective point/spot styles remain native runtime direct lighting through WorldLight records and are excluded from VHV. Editor VHV preview stays static; switched indirect ambient remains unsupported. It requires direct_ldr or direct_ldr_hdr with bounces:0 through 4; invalid depths reject atomically. At depth N above zero, prop reception uses world/terrain and eligible stationary requested brush-model lightmaps from depth N-1 and adds direct light once. Indirect reception requires texture-header or authored material reflectivity for every lit source. Dimensions independently use diffuse or tool VTF headers, or the SDK no-preview 128x128 convention; every complete VTF actually read for metrics is hashed. Authored $reflectivity accepts one ASCII [r g b] triple of finite linear values from 0 to 1, at most 128 bytes; only trimmed %notooltexture 0 or 1 selects the no-preview convention before dimension reads. Empty or other values and failed required providers reject without fallback. Eligible stationary Shadows Yes root/assembled-child func_brush baked maps supply reflected light alongside world/terrain; moving, parented and unrequested brush-model sources remain unavailable. Eligible VHV static-prop sources require reflect_static_props; other MDL sources remain unavailable. It preserves incompatible settings on rejection and needs no native capture runtime. get_compile_settings.prop_vertex_lighting reports current option availability and receiver limits, without inspecting assets. Admitted receivers use body0/root0, unit scale, one static bodypart/model, all 1–8 draw LODs with at most 256 hardware sections total, and plain opaque or strict default-alpha-test selected VertexLitGeneric materials; styles and unsupported materials fail requested builds unless explicitly opted out with disablevertexlighting=1. Author disableselfshadowing=1 through set_entity_properties to omit only that receiver from point/spot, sun, sky-ambient and reflected receiver shadow rays; other props, terrain and world stay active. lightingorigin preserves a unique visible info_lighting point for native runtime lighting; the bake still uses actual vertices and their normals/PVS, with no embedded-vertex recovery. ignorenormals=1 supports bounces:0 through 4, with constant 0.35 point/spot/sun reception, full-sphere sky ambient and spherical reflected reception with SDK angular normalization; real shadows remain active. Original vertices may touch opaque world brushes; buried vertices and terrain/other-prop support contacts reject explicitly. Complete independent LDR/HDR output is packed before publication; stats.prop_vertex_lighting reports actual hardware vertices and separate ldr_bytes/hdr_bytes. It also accepts colored_texture_shadows (default false), requiring lit mode and brush_texture_shadows without silently enabling prerequisites. It tints transport through existing admitted fractional base-alpha casters with gamma2.2 RGB and additive saturated per-channel byte coverage across direct/styles, sun/sky, bounces, probes, detail/VHV and emission. White RGB preserves scalar coverage, alpha0 stays clear, alpha255 stays opaque and binary cutouts are unchanged. Invalid updates reject atomically; recipes, per-map settings and job snapshots retain the value. No native VRAD or physical absorption parity is claimed. It also accepts brush_texture_shadows (default false, Direct LDR or Direct LDR + HDR only) for binary or fractional base-alpha shadows on plain LightmappedGeneric world/func_detail brush and terrain faces and same-model lit receivers at their admitted initial baked pose. brush_texture_shadowing reports availability, model scope and limits. Eligible stationary root/child func_brush Shadows Yes requests cast admitted opaque or opted-in texture-alpha shadows globally; unrequested model texture shadows remain within their applicable same-model receivers. Eligible stationary Shadows Yes root/assembled-child func_brush models supply reflected radiance from their existing baked maps; moving, parented and unrequested brush-model sources remain unavailable. Eligible VHV static-prop sources require reflect_static_props; other MDL sources remain unavailable. Map/VHV receivers preserve zero-to-four previous-depth bounce snapshots; ambient/details retain filtered final-map reception. Unsupported requested materials/scopes, alpha modulation and blend-shader terrain opacity stop publication; native model-requested prop texture shadows are independent. It also accepts pack_static_prop_assets (default false) to preserve original bounded static MDL 44/48, VVD, DX90 VTX, all declared opaque or default-alpha-test VertexLitGeneric terminals and supported original Patch materials, every declared basetexture VTF, plus matching PHY only for emitted VPhysics uses. Terminals allow only one canonical $basetexture, optionally plus literal $alphatest 1 at the default 0.5 cutoff; authored $alphatestreference is unsupported. Patch chains allow at most eight include edges, with one basetexture insert or replace section per Patch. All original VMTs and declared VTFs remain packed, including overridden textures. Deeper edits win within each type; replacements apply after all inserts. Combined edit sections, extra parameters, proxies and $envmap reject. It is independent of vertex lighting and works in every lighting mode. Packing preserves model flags. Compiler texture shadows require the original MDL CAST_TEXTURE_SHADOWS request; without it, enabled prop shadows remain opaque mesh shadows with the existing warning. Selected default-alpha-test vertex-lighting receivers accept the same bounded nested Patch basetexture edits and terminal contract. Every hardware vertex is retained with VHV alpha 255; receiver-only textures are read only if casting, packing or enabled reflected-source preparation needs them. Missing or unsupported dependencies stop publication; dynamic/detail models are excluded. Runtime requires standard CS:S shaders and surface/prop-data definitions with DX90 or newer. Enabling packing performs no asset lookup. It also accepts pack_water_assets (default false) to pack original files for complete supported static Water material/Patch/texture closures. Water packing requires static LDR/HDR cubemap siblings throughout bottom/underwater-overlay graphs. Explicit runtime Water surface envmaps and supported opaque SLIME runtime env_cubemap require pack_water_assets:true, generate_reflections:true and lighting:direct_ldr_hdr; complete generated sample pairs, bound aliases and original source custody are verified before publication. Omitted Water envmaps, runtime dependencies below the surface root and material cycles reject. Runtime Water envmapframe must be absent or zero and Patch chains cannot edit a key in multiple materials; all original declared resources remain packed, including overridden references. Static named-envmap packing remains available in every lighting mode without capture. It also accepts pack_sky_assets (default false) to preserve the exact complete authored sky material/Patch/LDR/HDR closure; incomplete or unsupported families fail during compilation without automatic family fallback. Packing is independent of baked lighting and requires no native capture runtime. generate_reflections (default false) requires direct_ldr or direct_ldr_hdr and runs shared native capture/verification before publishing; HDR lighting captures both channels, while Fullbright reflection generation rejects atomically. Assembled supported instance geometry uses the same single-bounce capture with visible root-map or admitted child env_cubemap helpers with automatic or direct local manual bindings; extra capture bounces remain unsupported. Automatic assignment includes transformed child faces. Root manual sides must target unique original root-map faces surviving compilation; child manual sides are remapped only from direct visible ordinary world/func_detail faces in the same child occurrence. The ordinary region/visible-face planner checks retained targets and rejects duplicate manual ownership. With visible instances, get_cubemap_bindings returns available:false, empty assignments and the common Session context reason, and preview/review/apply reject; an existing owned draft remains cancellable. Hidden references stay omitted and leave root-only inspection available without child reads. Inspect get_native_capture_runtime and replace both machine-local overrides with set_native_capture_runtime; omitted/null roots clear to checked automatic defaults. \
select_compiled_prop_source(build_token,prop) and select_compiled_world_light_source(build_token,channel,ordinal) require current owned native inspection plus an available verified captured source. They replace selection with exactly one root entity or captured outer containing reference in Objects mode; a root emitting face uses its exact FaceId in Faces mode. Existing entity results stay unchanged; a root surface returns target:{solid,side}, source_kind:root_face, mode:faces and selected_count:1. No group expansion, unhiding or child-file opening. Active viewport work/geometry drafts and stale/missing/invalid data/metadata refuse atomically; camera/map/history and ordinary property drafts remain unchanged. Native framing remains independent of source mapping availability. \
inspect_baked_prop_vertices reads actual packed VHV from the last successful owned build on the CPU, shared with Build settings… / Build details… → Inspect baked prop vertices…. A fresh observation returns the build token; explicit native prop/section selections and nonzero page offsets require that token. Pages cap at 32 props, 32 sections and 256 hardware-color vertices. Reports identify native ordinal/model/origin, captured root or nested authored source_association in prop rows and selected_prop, ordered LOD sections, independent LDR/HDR files and raw encoded BGRA8/RGBA8 bytes, not linear irradiance or game brightness. Intact stale output remains historical; replaced/corrupt output clears every row. Missing channels and disabled ordinals have no fallback. Captured prop source routes are immutable historical addresses, not current-map resolution. source_associations reports unavailable metadata independently of valid native/color data; source copies cap at 128 KiB, with prop_limit allowing smaller pages. Inspection changes no map, history, selection, camera, settings or drafts and accepts no file path. \
inspect_compiled_world_lights reads exact native LDR/HDR light records from the last successful owned build, shared with Build settings… / Build details… → Inspect compiled lights…. Pages cap at 64 and retain native ordinals, kinds, positions, intensity, normal, style, cone/attenuation fields and signed flags/owner/texinfo. Start at offset 0 without a token; retained requests use the returned build_token. Valid zero-record lit tables differ from Fullbright and unbaked HDR. Current/stale/unavailable provenance preserves intact historical output and clears changed/corrupt output. Point/spot and known sun/sky rows retain their captured entity origin or an explicit unavailable reason. Surface associations add compiled face, original canonical material and typed root or exact-instance face origin with solid/side and occurrence, without inventing a light entity. Root IDs and outer-helper/leaf-file/original-entity/build-local-expanded identities describe captured entity sources; ordered occurrence helper/file routes distinguish repeated leaf sources. Ambiguous, generated or unresolved face sources remain explicitly unavailable. Use inspect_baked_lightmaps source with the exact captured face route to find compiled matches. Historical routes are not resolved against current documents. Native table counts/digests guard association pairing. Use inspect_compiled_visibility target {kind:world_light,channel:ldr|hdr,ordinal:N} with the same build_token for the stored source cluster, shared with Inspect source visibility. PVS/PAS does not show native light winners, falloff, ray occlusion or exact finite-area visibility. Values do not establish screen brightness or runtime channel selection. No path argument or source/history/selection/camera/settings/draft mutation. \
frame_compiled_prop_origin requires current build_token and native prop ordinal; aspect defaults to 1. It reuses owned native inspection, frames a disclosed 16-unit point half-extent, preserves projection/selection/history/ordinary drafts and refuses stale/unavailable data or viewport work. Disabled/non-baked props and missing source associations do not erase native origins. No model bounds or source object resolution. \
frame_compiled_world_light frames one current owned point/spot origin or finite surface centroid using explicit build_token, channel and native ordinal; optional aspect defaults to 1. It reuses owned inspection, preserves projection and ordinary drafts, and refuses stale/unavailable/directional sky/unknown records or active viewport gestures before changing the camera. Surface records remain emitting-triangle centroid point approximations. Known sun/sky sources can be selected through select_compiled_world_light_source, then framed through frame_selection. Returns the same captured source association without selecting source objects or changing viewport shading. \
Use get_compile_region and preview_compile_region for native sealed build bounds; apply_compile_region or cancel_compile_region owns the returned token. Only committed enabled regions affect builds; disable them for full-map leak checks. Jobs capture their start scope. \
Saved parents automatically expand nested func_instance world/func_detail brushes, unnamed unparented ordinary func_brush models with explicit finite origins and absent or checked-zero source angles, power 2–4 terrain, strictly admitted unnamed point/spot lights with built-in appearances and static props, scoped lighting-origin helpers, neutral T/CT player spawns, consumed visibility-cluster and occluder helpers, unnamed overlays on ordinary or actual terrain sides and cubemap samples with automatic or direct local ordinary-face manual bindings through four helper levels, with root-relative filenames and no extra compile switch. Child terrain is bounded to 65,536 source-grid vertices, 262,144 expanded grid vertices and 2,097,152 grid-work units; repeated occurrences and intermediate transforms count separately. Invalid or unsupported terrain fails rather than substituting brush cages. Optional stats.instances includes terrain_patches and terrain_vertices only when nonzero; these count generated patches and authored grid vertices before neighbor LOD adjustments. \
Child light_environment adds one unnamed static global sun/sky source with the normal FGD scalar casing, including _AmbientScaleHDR and SunSpreadAngle. Required origin and every transformed position stay finite within Source bounds; optional finite angles defaults to zero, explicit pitch lies in [-90,90], legacy angle is absent/zero, and pitch0 requires angles.x0. Both LDR/HDR sun/ambient/sentinel/scale/spread values use the ordinary compiler parser even in Fullbright, with no viewport headroom policy. Names/references/patterns must be absent or exactly empty (whitespace rejects); style/defaultstyle/spawnflags are zero. Brushes/I/O/opaque blocks/unknown fields/duplicates/replacements reject. The parent keeps world skyname/material/world settings; regions retain the global source independently of its marker. Baked lighting permits one source across root and children; repeated/root+child sources fail the existing global bake rule, while Fullbright keeps its no-bake semantics without a new eager cap. Optional omit-zero stats.instances.environment_lights counts visible expanded child helpers before regions, separately from point_lights/spot_lights and native SKY/SKY_AMBIENT worldlight records. Reference inspection reuses static_light_preview with an explicit sun/sky-unavailable reason; child illumination/icons/bounds/picking/framing remain unavailable. Actual brush preview/Frame and root drafts stay unchanged. Use existing create_entity/set_entity_properties/save_map/reference/compile operations; no new tool or setting. Child point/spot lights support only exact lowercase light/light_spot with canonical classname and explicit finite origin, valid ordinary brightness/attenuation/HDR/cone fields, absent style or offered Light pattern IDs 0–11, absent-or-zero defaultstyle/spawnflags and legacy angle, and absent-or-empty names, parents, patterns and other runtime references. An unnamed child light_spot may target one canonical visible info_target in its immediate child, using exact local spelling. Endpoint targetname and lamp target Prefix-compose with explicit Fix Up Name and fixup_style 0 on every ancestor; missing, duplicate, external, coincident and colliding targets reject before regions in every mode. Original endpoints and every intermediate position/direction use shared ordinary validation. spotlight_targets counts endpoints before regions; spotlight_target_preview explicitly reports unavailable target geometry, bounds, picking, framing and child targeting. Ordinary origin-based regions apply; a retained lit lamp without its endpoint fails rather than inventing a direction. IDs 0–11 follow the installed FGD choices. Open the child normally, use existing create_entity/set_entity_properties to author Light pattern, then save/reopen/reference it. Environment lights stay Normal-only and Initially dark remains unsupported. Repeated root/child lamps share global channels by numeric style; the four-style face limit including Normal still applies. VHV stores a style-0 base while supported point/spot styles remain runtime direct lighting. Point/spot expansion counts include patterned lamps; child illumination/icons and animated preview remain unavailable. Unknown properties, blocks, connections and duplicate case aliases reject. Each child spotlight requires explicit canonical pitch in [-90,90]; the -90 downward default is supported, while zero pitch requires the first angles component zero. Optional angles defaults to zero. Existing create_entity/set_entity_properties/save_map operations author the child. stats.instances.point_lights and spot_lights omit zero values and count expansion before compile-region filtering; emitted lights are selected by transformed origin. \
Child prop_static supports canonical lowercase properties, explicit finite origin, unit scale, body0 and normal-white rendering. Skin is a nonnegative integer with absent/default0; retained model headers validate the actual family range after region selection. ignorenormals/disableshadows/disablevertexlighting/disableselfshadowing accept integer0/1 (absent0), reusing ordinary lighting consumers. Vertex-bake opt-out excludes only that receiver and never bypasses required MDL/PHY. fademindist/fademaxdist/fadescale accept finite native-f32 values with defaults -1/0/1; end and scale must be nonnegative. A negative start inherits a positive end; end0 emits start0 and disables the authored fade interval. screenspacefade accepts integer0/1 (default0), interpreting start/end as distance or screen pixel width; no start-end ordering is imposed. mindxlevel/maxdxlevel accept u16 integers (default0); minimum must not exceed a nonzero maximum. Zero DX bounds use the ordinary unrestricted defaults. Shared header-independent visibility validation runs during reference refresh without model reads. Other non-neutral flags/body/scale remain unsupported; refresh does not certify model families or runtime fade/DX appearance. The opt-in vertex bake retains its ordinary unrestricted-DX receiver requirement. Names/targets/parents stay absent or empty; unknown fields, duplicate case aliases, brushes, connections and opaque blocks reject. solid accepts 0, 2 or 6; absent uses ordinary 6. Mode 6 requires the original matching PHY even with Fullbright and physics:none. Modes 0/2 leave unused PHY unread; region-omitted props leave MDL/PHY unread. The ordinary post-region consumer checks source/intermediate model boxes using its original MDL header and retains original consumed model/PHY sources through publication, stale inspection and Run; job.static_prop_source_issue identifies changes and byte repair requires recompilation. Optional stats.instances.static_props counts expansion before region filtering and omits zero. Child info_lighting requires canonical lowercase unique classname/origin/targetname, a finite source/intermediate point, optional finite irrelevant angles/angle and empty neutral name/target/parent references; brushes, connections, unknown properties and opaque blocks reject. Helpers and nonempty prop lightingorigin use strictly local occurrence bindings: set a canonical targetname on the helper, the same local lightingorigin on its prop, and explicit targetname plus fixup_style:0 (Prefix, also the absent default) on every containing func_instance. Nonempty ASCII letter/digit/underscore components compose outer-inner-local names up to 127 bytes; Postfix/None, automatic/fallback names and global escapes reject on name-bearing branches. Missing/hidden local helpers fail only retained props; parent/sibling helpers cannot substitute. Region-exterior helpers remain available. Root props may use the exact complete prefixed name. Duplicate/case-ambiguous local names and generated collisions, including visible root helpers, reject. Optional stats.instances.lighting_origins counts expanded visible helpers including unreferenced ones before regions and omits zero. Author through existing create_entity/set_entity_properties/save_map and compile operations. \
Child info_overlay supports unnamed projections with 1–64 distinct positive side IDs from direct visible world/func_detail brushes in the same occurrence: ordinary sides on brushes without terrain or actual displacement-bearing sides at powers 2–4. Mixed ordinary/terrain target lists retain authored order. Non-displaced cage sides of terrain-bearing solids, runtime models, consumed helper targets and parent/descendant/sibling/repeated/root/generated IDs cannot supply local targets. Authored and rewritten lists each fit 512 bytes. Canonical keys retain the normal overlay writer casing; runtime names/references must be absent or exactly empty, optional angles/angle must be zero and spawnflags/mindxlevel/maxdxlevel integer0, and I/O/opaque blocks/unknown fields/duplicate case aliases reject. BasisOrigin and optional marker origin transform as points; BasisU/V/Normal rotate without translation, while local uv0–3 and texture/order/fade strings remain authored. Source and every intermediate frame/marker validate before regions. Optional omit-zero stats.instances.overlays counts visible expanded child helpers before regions; stats.overlays and overlay_face_refs retain their ordinary emitted totals. Reference inspection's overlay_preview explicitly reports unavailable child projection/icons/overlay bounds/picking/framing/drafts; actual target mesh, owner bounds/picking and overall Frame behavior stay unchanged. Root-face overlay drafts remain available. Open the child normally for existing overlay operations. For terrain, preview/review/apply the unnamed overlay on an ordinary world quad first, then create/sculpt terrain on that stable side, optionally tie its brush to func_detail, and save/reopen/reference the child. Direct terrain fitting, refit, rebind and corners accept one isolated planar displacement with straight quad borders, disjoint terrain base bounds and no visible instances. Cubemap manual targets still exclude every side of terrain-bearing solids; no new setting/tool. Ordinary retained target, region, CSG and 1024/64/65536 limits still apply. Individual child source/retained-target errors keep root owner/true leaf/original helper diagnostics; global budget errors stay global. Child func_occluder helpers preserve 1–16 visible ordinary convex brushes as one unnamed stationary helper. StartActive keeps its installed-FGD spelling and accepts 0/1 with absent/default1; optional origin stays absent or finite and compile-bounded, angles/angle stay absent or zero, and runtime names/references must be absent or exactly empty. Hidden helpers and all-hidden helper brushes omit expansion; truly empty helpers reject. Optional omit-zero stats.instances.occluders counts visible expanded child helpers before compile-region filtering, separately from retained compiler occluder/polygon counts. occluder_preview returns available:false with its shared reason: no child helper reference geometry, icons, bounds, picking or framing is available. Occluders add no physical detail brushes or reference mesh; helper-only assemblies can be ready with identities/counts, zero geometry and null owner bounds, and Frame refuses without moving the camera. Refresh checks bounded source shape and every transformed pose; retained material, CSG, native rounding, area and global1024-helper checks remain compiler-owned. Regions select each whole helper. Child func_viscluster uses canonical unique lowercase keys and one visible convex brush of at most 64 sides, without terrain, I/O or opaque blocks. Optional origin must be finite and compile-bounded at source and every intermediate pose; an absent origin stays absent. angles/angle must be absent or zero; spawnflags/mindxlevel/maxdxlevel must be absent or integer0; targetname/target/parentname/name/globalname/killtarget/lightingorigin must be absent or trim-empty. Other keys reject. Hidden helpers and all-hidden helper brushes omit expansion. Optional stats.instances.visclusters counts visible expanded helpers before compile-region filtering and omits zero; stats.viscluster_helpers separately counts retained root plus expanded helpers after region selection under the shared 16-helper limit. Helpers add no physical detail brushes or reference mesh. Source/hull and transform limits apply before region omission; retained helpers use the ordinary dry-leaf, area, membership and ambiguity checks. Reference inspection reports viscluster_preview:{available:false,reason}; helper geometry/icons/bounds/picking/framing remain unavailable. \
Child env_cubemap samples require canonical unique lowercase properties, explicit finite compile-bounded source/intermediate origins and ordinary cubemapsize0–9 (absent/default0). An absent or trim-empty sides list selects automatic assignment; otherwise it must contain positive distinct IDs from visible ordinary world/func_detail brushes directly in that same child occurrence. Parent/descendant/sibling/repeated/root/generated targets, terrain-bearing solids and runtime-model sides reject; nested children bind their own direct geometry. Authored and rewritten lists each have a 512-byte limit. Ordinary sample-only builds warn that sides are unused; Generate reflections applies the compiled bindings. Optional angles/angle must be finite zero, spawnflags/mindxlevel/maxdxlevel integer0 and runtime names/references trim-empty; brushes, I/O, opaque blocks and other keys reject. No orientation is synthesized. After precise source/intermediate bounds checks, final sample coordinates narrow to Source f32 before integer truncation. Optional stats.instances.cubemaps counts visible expanded samples before compile-region filtering and omits zero; stats.cubemap_samples counts retained root plus child samples. Ordinary BSP builds share 1024 samples; reflection generation shares 1–32 distinct native integer positions and cube edges within get_native_capture_runtime.maximum_sample_size after regions. Sample-only references can be ready with zero geometry/null bounds; cubemap_preview:{available:false,reason} explicitly reports no icons, geometry, bounds, picking or framing. Refresh does not capture or certify reflections. \
Child player spawns support exact info_player_terrorist/info_player_counterterrorist with canonical unique lowercase properties, explicit finite source/intermediate origin, optional finite angles (absent zero), absent/zero legacy angle, integer spawnflags0 and empty names/targets/parents/global references. Brushes, connections, opaque blocks and other keys reject. Existing generic entity/property/save/compile operations apply; stats.instances.player_spawns counts visible expanded spawns before region filtering and omits zero. Emitted spawns follow ordinary transformed-origin selection and flood/leak behavior; no player-model asset is read. \
Child func_brush models retain neutral installed-FGD defaults except vrad_brush_cast_shadows:1 explicitly requests opaque stationary global shadows; spawnflags absent/native0 or explicit0/2. Compile requires a finite explicit origin or one visible resolved origin volume with ordinary body geometry remaining. The source-volume AABB center overrides a valid explicit origin; present invalid origins reject. Material resolution consumes the volume before instance transforms, retaining original material/source custody; body materials stay ordinary SOLID, with liquids, terrain and special contents refused before regions. Source/intermediate world and final local hulls stay within compile bounds. Refresh reads no materials and defers effective pivots and model-local bounds, including when an authored origin could be overridden. Optional stats.instances.brush_models and reference stats.brush_models count whole visible occurrences before regions and omit zero. Compiled stats.instances.consumed_origin_brushes counts consumed volumes per expanded occurrence before regions and omits zero. Source-only reference stats omit that unresolved count; brush_model_preview.consumed_origin_brushes is null, explicitly unknown. brush_model_preview reports preparation:deferred and deferred_checks:[effective_pivot,model_local_bounds,materials,native_compilation], with no model geometry/icons/bounds/picking/framing; model-only references remain ready with identities/counts and null bounds, while mixed world/detail reference geometry stays unchanged. \
get_compile_settings.brush_model_shadowing describes Advanced properties → Shadows → Yes (vrad_brush_cast_shadows:1), independent of brush_texture_shadows. Strict unnamed unparented ordinary opaque func_brush requests require an admitted initial pose. Root requests require explicit finite origin; a child may resolve and consume one origin volume before validating its remaining body, and child source angles remain zero. Requests validate source/material semantics before regions in all modes; Fullbright reports admitted source requests without baking, including requests omitted by compile regions. stats.brush_shadow_models counts actual retained prepared global casters in lit modes, zero in Fullbright, whose diagnostics explicitly report the unbaked state. Models remain excluded from reflected sources; geometry, collision, PVS and probe placement are unchanged. No new setting or tool. \
inspect_instance_contents/set_instance_contents expose the same explicitly refreshed flat-color reference preview as Instance contents…. Terrain rendering, bounds and root-helper picking use real displaced triangles; face_provenance rows retain root owner, true leaf file and original brush/side IDs plus terrain:true, power, vertices and triangles. The preview also caps complete parent/child terrain context at 262,144 vertices. Refresh fails if adding child contents would change an ordinary parent patch's displaced vertices or triangle indices relative to its ordinary context; existing parent-to-parent LOD alone does not block preview, and compilation remains available. There is no cage fallback or child editing. Child light_environment adds optional omit-zero stats.environment_lights before regions, distinct from point/spot counts and native SKY records. The existing static_light_preview is available:false with the shared explicit sun/sky illumination/icon/bounds/picking/framing reason when this count is positive; point/spot-only reason text stays unchanged. Light-only references keep ready identities/counts with zero mesh/null bounds and ordinary camera-preserving Frame refusal. Mixed brush preview/Frame and root drafts stay unchanged. Child lights add counts only: no child icons, illumination or light geometry is shown in the reference or authored-lighting preview. Child props likewise add counts only: static_prop_preview.available is false with a reason, and models/bounds/picking/framing are unavailable. Child lighting-origin helpers also add counts only: lighting_origin_preview.available is false with a reason; no helper geometry, icons, bounds, picking or framing is available. Child visibility helpers add omit-zero stats.visclusters before compile-region filtering and viscluster_preview:{available:false,reason}; helper geometry, icons, bounds, picking and framing are unavailable, with zero added physical detail/reference geometry. Child overlays add omit-zero stats.overlays before compile-region filtering and overlay_preview:{available:false,reason}; child projection/icons and overlay-specific bounds/picking/framing/drafts are unavailable, while actual target-brush display/bounds/picking/Frame and root-face drafts stay unchanged. Open the child as a normal map to author overlays through existing shared operations. Child occluder helpers add omit-zero stats.occluders before compile-region filtering and occluder_preview:{available:false,reason}; helper geometry, icons, bounds, picking and framing are unavailable, with zero added physical detail/reference geometry. Child cubemap samples add omit-zero stats.cubemaps before compile-region filtering and cubemap_preview:{available:false,reason}; sample geometry, icons, bounds, picking and framing are unavailable. Child player spawns add counts only: player_spawn_preview.available is false with a reason; no spawn model/icon, bounds, picking or framing is provided. limits.lighting_origin_name_bytes reports 127. A light/model/helper/spawn-only assembly can be ready with available:false, zero geometry counts and null owner content bounds, retaining dependency identities but offering no reference geometry to pick or frame. \
Start asynchronous builds with start_compile; poll get_compile_status and cancel_compile using the job token. Compile synchronously with compile_map or compile and play with run_map; inspect engine output with \
game_log and stop the managed client with stop_game. Failed compiles expose leak paths \
in the viewport; set_view controls their visibility.";

type ToolResult = Result<CallToolResult, ErrorData>;

fn ok(value: Value) -> ToolResult {
    Ok(CallToolResult::success(vec![ContentBlock::text(
        value.to_string(),
    )]))
}

fn fail(message: impl Into<String>) -> ToolResult {
    Ok(CallToolResult::error(vec![ContentBlock::text(
        message.into(),
    )]))
}

/// Runs a fallible tool body, reporting errors as tool errors the agent can
/// read rather than protocol failures.
fn run(f: impl FnOnce() -> Result<Value, String>) -> ToolResult {
    match f() {
        Ok(value) => ok(value),
        Err(message) => fail(message),
    }
}

fn parse_ids(ids: &[String]) -> Result<Vec<ObjectId>, String> {
    ids.iter().map(|s| parse_id(s)).collect()
}

/// Explicit ids, or the selection when none are given.
fn ids_or_selection(session: &Session, ids: Option<&[String]>) -> Result<Vec<ObjectId>, String> {
    let ids = match ids {
        Some([]) => {
            return Err("explicit ids must not be empty; omit ids to use the selection".into());
        }
        Some(ids) => parse_ids(ids)?,
        None => session.selected_ids(),
    };
    if ids.is_empty() {
        return Err("no ids given and nothing is selected".into());
    }
    Ok(ids)
}

fn vec3(v: [f64; 3]) -> DVec3 {
    DVec3::from_array(v)
}

fn point(v: DVec3) -> Value {
    json!(v.to_array())
}

fn bounds_json(bounds: Option<(DVec3, DVec3)>) -> Value {
    bounds.map_or(
        Value::Null,
        |(lo, hi)| json!({ "mins": point(lo), "maxs": point(hi) }),
    )
}

fn materials_of<'a>(solids: impl IntoIterator<Item = &'a Solid>) -> Vec<String> {
    let mut names: Vec<String> = solids
        .into_iter()
        .flat_map(|s| {
            s.sides
                .iter()
                .map(|side| side.material.to_ascii_lowercase())
        })
        .collect();
    names.sort_unstable();
    names.dedup();
    names
}

fn summary(doc: &Document, id: ObjectId) -> Value {
    let bounds = bounds_json(query::bounds(doc, id));
    match id {
        ObjectId::Solid(n) => {
            let materials = doc.solid(n).map(|s| materials_of([s])).unwrap_or_default();
            json!({ "id": id_string(id), "kind": "solid", "bounds": bounds, "materials": materials })
        }
        ObjectId::Entity(n) => {
            let Some(e) = doc.entity(n) else {
                return Value::Null;
            };
            json!({
                "id": id_string(id),
                "kind": if e.solids.is_empty() { "point_entity" } else { "brush_entity" },
                "classname": e.classname(),
                "targetname": e.properties.get("targetname"),
                "origin": e.origin().map(point),
                "brushes": e.solids.len(),
                "bounds": bounds,
            })
        }
    }
}

fn solid_detail(solid: &Solid) -> Value {
    let faces: Vec<Value> = solid
        .sides
        .iter()
        .map(|side| {
            let normal = Plane::from_points(side.plane.0).map(|p| point(p.normal));
            json!({
                "side": side.id,
                "material": side.material.to_ascii_lowercase(),
                "normal": normal,
                "plane": side.plane.0.map(point),
                "uaxis": side.u_axis.to_string(),
                "vaxis": side.v_axis.to_string(),
                "rotation": side.rotation,
                "lightmap_scale": side.lightmap_scale, "smoothing_groups": map_core::surface::smoothing_numbers(side.smoothing_groups.unwrap_or(0)), "smoothing_mask": side.smoothing_groups.unwrap_or(0),
                "shift_u": side.u_axis.offset, "shift_v": side.v_axis.offset,
                "scale_u": side.u_axis.scale, "scale_v": side.v_axis.scale,
                "displacement": side.displacement.as_ref().map(|d| d.power),
            })
        })
        .collect();
    json!({ "solid": solid.id, "bounds": bounds_json(query::solid_bounds(solid)), "faces": faces })
}

fn entity_detail(entity: &Entity) -> Value {
    let properties: Vec<Value> = entity
        .properties
        .iter()
        .map(|(k, v)| json!([k, v]))
        .collect();
    let connections: Vec<Value> = entity
        .connections
        .iter()
        .flatten()
        .map(|c| {
            let fields: Vec<&str> = c.fields().collect();
            json!({ "output": c.output, "target_input_param_delay_times": fields })
        })
        .collect();
    let brushes: Vec<Value> = entity
        .solids
        .iter()
        .map(|s| {
            let mut brush = solid_detail(s);
            brush["materials"] = json!(materials_of([s]));
            brush
        })
        .collect();
    json!({ "properties": properties, "connections": connections, "brushes": brushes })
}

fn detail(doc: &Document, id: ObjectId) -> Result<Value, String> {
    let mut value = summary(doc, id);
    let extra = match id {
        ObjectId::Solid(n) => solid_detail(doc.solid(n).ok_or(format!("{id} does not exist"))?),
        ObjectId::Entity(n) => entity_detail(doc.entity(n).ok_or(format!("{id} does not exist"))?),
    };
    if let (Value::Object(base), Value::Object(extra)) = (&mut value, extra) {
        base.extend(extra);
    }
    Ok(value)
}

fn selected_component_report(session: &Session) -> Result<Value, String> {
    session.component_report(
        &session
            .component_selection()
            .iter()
            .map(|component| component.solid)
            .collect::<Vec<_>>(),
    )
}

fn hit_json(doc: &Document, hit: &Hit) -> Value {
    let material = hit.solid.zip(hit.side).and_then(|(solid, side)| {
        let owner = doc.owner_of_solid(solid)?;
        let solid = match owner {
            ObjectId::Solid(n) => doc.solid(n)?,
            ObjectId::Entity(n) => doc.entity(n)?.solids.iter().find(|s| s.id == solid)?,
        };
        Some(
            solid
                .sides
                .iter()
                .find(|s| s.id == side)?
                .material
                .to_ascii_lowercase(),
        )
    });
    json!({
        "object": id_string(hit.object),
        "solid": hit.solid,
        "side": hit.side,
        "material": material,
        "distance": hit.distance,
        "point": point(hit.point),
    })
}

fn camera_json(camera: &Camera) -> Value {
    json!({
        "position": point(camera.position.as_dvec3()),
        "yaw": camera.yaw_degrees,
        "pitch": camera.pitch_degrees,
        "fov": camera.fov_y_degrees,
        "forward": point(camera.forward().as_dvec3()),
        "projection": camera.projection.name(),
        "ortho_height": camera.ortho_height,
        "center": (camera.projection != Projection::Perspective).then(|| point((camera.position + camera.forward() * Camera::ORTHO_DISTANCE).as_dvec3())),
    })
}

fn game_log_json(log: &game::Log) -> Value {
    let (status, exit_code, error) = match &log.status {
        game::Status::Running => ("running", None, None),
        game::Status::Exited { code, .. } => ("exited", *code, None),
        game::Status::Failed(error) => ("failed", None, Some(error.as_str())),
    };
    json!({"status": status, "exit_code": exit_code, "error": error, "pid": log.pid,
        "map": log.map, "console": log.console, "launcher": log.launcher, "truncated": log.truncated,"native_activity":log.native_activity})
}

#[derive(Clone)]
pub struct EditorServer {
    session: SharedSession,
    operation: Option<DocumentOperation>,
    _document: Option<DocumentHandle>,
    stdio_output: Option<StdioOutput>,
    terminal: Option<TerminalClose>,
    offscreen: Arc<Mutex<OffscreenState>>,
    model_renderer: Arc<Mutex<crate::model_preview::PreviewRenderer>>,
    tool_router: ToolRouter<Self>,
    tool: Option<String>,
}

struct ToolSession<'a> {
    guard: MutexGuard<'a, Session>,
}

impl std::ops::Deref for ToolSession<'_> {
    type Target = Session;

    fn deref(&self) -> &Session {
        &self.guard
    }
}

impl std::ops::DerefMut for ToolSession<'_> {
    fn deref_mut(&mut self) -> &mut Session {
        &mut self.guard
    }
}

impl Drop for ToolSession<'_> {
    fn drop(&mut self) {
        self.guard.set_log_tool(None);
    }
}

impl EditorServer {
    pub fn new(session: SharedSession, ctx: Option<GpuContext>) -> Self {
        Self {
            session,
            _document: None,
            operation: None,
            stdio_output: None,
            terminal: None,
            model_renderer: Arc::default(),
            offscreen: Arc::new(Mutex::new(OffscreenState {
                renderer: ctx.map(Offscreen::new),
                error: None,
            })),
            tool_router: Self::tool_router(),
            tool: None,
        }
    }

    pub fn for_document(document: DocumentHandle, ctx: Option<GpuContext>) -> Self {
        let mut server = Self::new(document.session().clone(), ctx);
        server._document = Some(document);
        server
    }

    pub fn stdio_output(&self) -> Option<StdioOutput> {
        self.stdio_output.clone()
    }

    pub fn with_terminal_close(mut self, terminal: TerminalClose) -> Result<Self, String> {
        if self.operation.is_some()
            || self._document.as_ref().map(DocumentHandle::id) != Some(terminal.document_id())
        {
            return Err("MCP terminal Close requires its idle owning document".into());
        }
        self.terminal = Some(terminal);
        Ok(self)
    }

    fn document_access(&self) -> crate::runtime::DocumentAccess {
        self._document.as_ref().map_or_else(
            || crate::runtime::DocumentAccess::unmanaged(self.session.clone()),
            |document| crate::runtime::DocumentAccess::for_document(document.clone()),
        )
    }

    fn session(&self) -> ToolSession<'_> {
        let mut guard = lock(&self.session);
        guard.set_log_tool(self.tool.clone());
        ToolSession { guard }
    }

    fn admitted(&self, tool: &str) -> Result<Self, String> {
        let mut server = self.clone();
        server.tool = Some(tool.to_owned());
        if server.operation.is_none() {
            server.operation = server
                ._document
                .as_ref()
                .map(|document| {
                    document.admit_named(&format!(
                        "MCP {}",
                        map_core::entity_helpers::bounded_preview(tool, 96)
                    ))
                })
                .transpose()?;
        }
        Ok(server)
    }

    fn worker<F, R>(&self, work: F) -> impl FnOnce() -> R + Send + 'static
    where
        F: FnOnce() -> R + Send + 'static,
    {
        // Abandoned requests must retire before their last runtime owner drains.
        let worker = (work, self.clone());
        move || {
            let (work, _scope) = worker;
            work()
        }
    }
}

fn render_size(width: Option<u32>, height: Option<u32>) -> (u32, u32) {
    let clamp = |v: u32| v.clamp(16, MAX_RENDER_SIDE);
    (
        clamp(width.unwrap_or(DEFAULT_RENDER_SIZE.0)),
        clamp(height.unwrap_or(DEFAULT_RENDER_SIZE.1)),
    )
}

fn single_entity(session: &Session, id: Option<&str>) -> Result<u32, String> {
    let id = match id {
        Some(id) => parse_id(id)?,
        None => match session.selected_ids()[..] {
            [only] => only,
            _ => return Err("give an entity id or select exactly one entity".into()),
        },
    };
    match id {
        ObjectId::Entity(n) => Ok(n),
        ObjectId::Solid(_) => Err(format!("{} is a world brush, not an entity", id_string(id))),
    }
}

impl EditorServer {
    pub(in crate::mcp) async fn compile_snapshot(
        &self,
        args: CompileArgs,
        play: bool,
    ) -> ToolResult {
        let access = self.document_access();
        let result = if access.managed() {
            let path = args.path.as_ref().map(PathBuf::from);
            let handle = match access.submit_compile(
                Actor::Agent,
                path.as_deref(),
                play,
                args.expected_generation,
            ) {
                Ok(handle) => handle,
                Err(error) => return fail(error),
            };
            handle.await
        } else {
            let submission = {
                let path = args.path.as_ref().map(PathBuf::from);
                match CompileSubmission::begin(
                    &self.session,
                    Actor::Agent,
                    path.as_deref(),
                    play,
                    args.expected_generation,
                ) {
                    Ok(submission) => submission,
                    Err(message) => return fail(message),
                }
            };
            let completion = submission.completion();
            let worker = tokio::task::spawn_blocking(self.worker(move || submission.run())).await;
            match worker {
                Ok(result) => result,
                Err(error) => {
                    let error = error.to_string();
                    completion.fail(error.clone());
                    Err(error)
                }
            }
        };
        let (result, game) = match result {
            Ok(result) => result,
            Err(message) => return fail(message),
        };
        let job_token = result.job_token().to_string();
        let output = result.output;
        let diagnostics: Vec<Value> = output
            .report
            .diagnostics
            .iter()
            .enumerate()
            .map(|(index, diagnostic)| {
                crate::session::compilation::diagnostic_json(index, diagnostic)
            })
            .collect();
        let value = json!({
            "job_token":job_token,
            "success": output.bsp.is_some(),
            "bsp": output.bsp.as_ref().map(|_| result.path.display().to_string()),
            "diagnostics": diagnostics,
            "leak": output.leak.as_ref().map(|leak| json!({ "entity": format!("entity:{}", leak.entity), "points": leak.points.iter().copied().map(point).collect::<Vec<_>>(), "pointfile": result.path.with_extension("lin").display().to_string() })),
            "stats": crate::session::compilation::stats_json(&output.stats),
            "game": game.as_ref().map(game_log_json),
        });
        if output.bsp.is_some() {
            ok(value)
        } else {
            Ok(CallToolResult::error(vec![ContentBlock::text(
                value.to_string(),
            )]))
        }
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for EditorServer {
    async fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParams,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::CallToolResponse, ErrorData> {
        if request.name.as_ref() == close::TOOL_NAME {
            let context = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
            return self.tool_router.call(context).await;
        }
        let server = match self.admitted(request.name.as_ref()) {
            Ok(server) => server,
            Err(error) => return fail(error).map(Into::into),
        };
        let context = rmcp::handler::server::tool::ToolCallContext::new(&server, request, context);
        server.tool_router.call(context).await
    }

    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(rmcp::model::Implementation::new(
                "hammerr",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(INSTRUCTIONS)
    }
}

/// Serves MCP over stdin/stdout until the client disconnects.
pub fn serve_stdio(mut server: EditorServer) -> Result<(), String> {
    let flags;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let owner = if server.terminal.is_none() {
        server
            ._document
            .clone()
            .map(|document| {
                let owner = TerminalOwner::start(document, None)?;
                server.terminal = Some(owner.handle());
                Ok::<_, String>(owner)
            })
            .transpose()?
    } else {
        None
    };
    let (read, write, process_flags) = {
        let _entered = runtime.enter();
        stdio::process_transport()?
    };
    flags = process_flags;
    let result = runtime.block_on(run_stdio(server, read, write));
    let result = match owner {
        Some(mut owner) => match (result, owner.shutdown()) {
            (Ok(()), result) | (result, Ok(())) => result,
            (Err(service), Err(close)) => Err(format!("{service}; terminal Close failed: {close}")),
        },
        None => result,
    };
    drop(runtime);
    match (result, flags.finish()) {
        (Ok(()), result) | (result, Ok(())) => result,
        (Err(service), Err(flags)) => Err(format!("{service}; {flags}")),
    }
}

async fn run_stdio<R, W>(mut server: EditorServer, read: R, write: W) -> Result<(), String>
where
    R: tokio::io::AsyncRead + Send + Unpin + 'static,
    W: tokio::io::AsyncWrite + Send + Unpin + 'static,
{
    let (transport, output) = stdio::transport(read, write);
    server.stdio_output = Some(output);
    let terminal = server.terminal.clone();
    let running = server
        .serve(transport)
        .await
        .map_err(|error| error.to_string())?;
    if let Some(terminal) = terminal {
        let stop = Mutex::new(Some(running.cancellation_token()));
        terminal.set_transport_stop(move || {
            if let Some(stop) = stop.lock().unwrap_or_else(PoisonError::into_inner).take() {
                stop.cancel();
            }
        });
    }
    match running.waiting().await.map_err(|error| error.to_string())? {
        rmcp::service::QuitReason::Closed | rmcp::service::QuitReason::Cancelled => Ok(()),
        rmcp::service::QuitReason::JoinError(error) => {
            Err(format!("MCP stdio service failed: {error}"))
        }
        reason => Err(format!(
            "MCP stdio service stopped unexpectedly: {reason:?}"
        )),
    }
}

impl EditorServer {
    fn tool_router() -> ToolRouter<Self> {
        Self::assets_tools()
            + Self::atlas_tools()
            + Self::baked_surfaces_tools()
            + Self::build_tools()
            + Self::compile_region_tools()
            + Self::compiled_details_tools()
            + Self::checks_tools()
            + Self::clipboard_tools()
            + Self::color_palette_tools()
            + Self::components_tools()
            + Self::context_tools()
            + Self::cubemap_bindings_tools()
            + Self::entities_tools()
            + Self::entity_placement_tools()
            + Self::files_tools()
            + Self::geometry_tools()
            + Self::groups_tools()
            + Self::grid_preferences_tools()
            + Self::preview_preferences_tools()
            + Self::navigation_preferences_tools()
            + Self::pivot_preferences_tools()
            + Self::shortcut_tools()
            + Self::history_tools()
            + Self::instance_contents_tools()
            + Self::instance_reference_tools()
            + Self::instance_extraction_tools()
            + Self::external_comparison_tools()
            + Self::instance_collapse_tools()
            + Self::linked_path_tools()
            + Self::materials_tools()
            + Self::measurement_tools()
            + Self::overlay_tools()
            + Self::partitions_tools()
            + Self::particles_tools()
            + Self::placement_tools()
            + Self::recovery_tools()
            + Self::selection_tools()
            + Self::terrain_tools()
            + Self::transform_pivot_tools()
            + Self::sky3d_preview_tools()
            + Self::color_correction_tools()
            + Self::compiled_face_framing_tools()
            + Self::entity_graph_tools()
            + Self::view_tools()
            + Self::prop_leaf_bounds_tools()
            + Self::working_region_tools()
    }
}
