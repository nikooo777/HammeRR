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
#[cfg(feature = "test-support")]
pub use close::validate_terminal_close_output;
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

#[cfg(test)]
use source_formats::fgd::Catalog;
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
preview_snap_to_grid rigidly aligns the world selection bounds minimum with the shared grid; inspect grid_snap and use the normal transform commit/cancel. \
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
get_entity_connections inspects outgoing and incoming links, parsed fields and warnings. \
get_connection_choices provides map target names and FGD inputs/outputs. Begin a shared form \
with begin_connection_edit, change fields with update_connection_draft, then commit_connection_edit \
or cancel_connection_edit. delete_entity_connection removes one outgoing index. Use generation \
and revision guards when starting an edit or deleting from a previously inspected list. \
get_entity_references also inspects FGD property links, parent references and input parameters. \
Use begin_entity_rename, update_entity_rename, commit_entity_rename or cancel_entity_rename \
for safe naming: exact links update atomically, shared names require rename_group, and \
unchanged ambiguous references require accept_review after inspecting the preview. \
get_property_inspector reports world or common entity fields, stored/default values and mixed flags. \
Use begin_property_edit (world:true or entity ids), update_property_edit with a changes list, \
then commit_property_edit or cancel_property_edit using the returned token. Default resets \
write the FGD value; remove deletes the override. All targets apply in one undo step. \
get_object_report searches names/classes/materials/types/visibility/bounds; select_object_report \
selects all matches and can frame them. set_object_visibility hides/shows/isolates/restores \
saved VMF visibility. Hidden objects and child brushes are OMITTED from compilation. \
get_compile_settings and set_compile_settings select shared build lighting (fullbright/direct_ldr/direct_ldr_hdr) and physics (none/engine); direct_ldr_hdr keeps LDR and HDR channels separate, reports HDR counters in stats.hdr and aggregate work in lighting_total_work, and warns when HDR light samples/world lights/ambient are empty. Engine physics uses the verified installed CS:S Linux64 library in an isolated worker for world, clip, brush-model and horizontal world-fluid bodies; terrain contact meshes are supported and static props use model assets; world, terrain and brush-model triangles retain per-face surface properties through a checked table of at most 127 names including default. Brush-model base masses use the dominant emitted surface with collision volume and density, or shell area, thickness and density, capped at 50,000 kg; engine mass scale and overrides remain effective. Custom density controls and detail/model fluids remain unavailable. \
set_compile_settings also accepts bake_static_prop_vertices (default false) for direct static-prop vertex lighting. It requires direct_ldr or direct_ldr_hdr with bounces:0 through 4; invalid depths reject atomically. At depth N above zero, prop reception uses world/terrain lightmaps from depth N-1 and adds direct light once. Indirect reception requires measured TextureHeader reflectivity for every lit source, hashing each complete selected base VTF dependency. Props and brush models are not reflected sources. It preserves incompatible settings on rejection and needs no native capture runtime. get_compile_settings.prop_vertex_lighting reports current option availability and receiver limits, without inspecting assets. Admitted receivers use body0/root0, unit scale, one static bodypart/model, all 1–8 draw LODs with at most 256 hardware sections total, and plain opaque selected VertexLitGeneric materials; styles and unsupported materials fail requested builds unless explicitly opted out with disablevertexlighting=1. Author disableselfshadowing=1 through set_entity_properties to omit only that receiver from point/spot, sun, sky-ambient and reflected receiver shadow rays; other props, terrain and world stay active. lightingorigin preserves a unique visible info_lighting point for native runtime lighting; the bake still uses actual vertices and their normals/PVS, with no embedded-vertex recovery. ignorenormals=1 supports bounces:0 through 4, with constant 0.35 point/spot/sun reception, full-sphere sky ambient and spherical reflected reception with SDK angular normalization; real shadows remain active. Original vertices may touch opaque world brushes; buried vertices and terrain/other-prop support contacts reject explicitly. Complete independent LDR/HDR output is packed before publication; stats.prop_vertex_lighting reports actual hardware vertices and separate ldr_bytes/hdr_bytes. It also accepts pack_sky_assets (default false) to preserve the exact complete authored sky material/Patch/LDR/HDR closure; incomplete or unsupported families fail during compilation without automatic family fallback. Packing is independent of baked lighting and requires no native capture runtime. generate_reflections (default false) requires direct_ldr or direct_ldr_hdr and runs shared native capture/verification before publishing; HDR lighting captures both channels, while Fullbright reflection generation rejects atomically. Inspect get_native_capture_runtime and replace both machine-local overrides with set_native_capture_runtime; omitted/null roots clear to checked automatic defaults. \
inspect_baked_prop_vertices reads actual packed VHV from the last successful owned build on the CPU, shared with Build settings… / Build details… → Inspect baked prop vertices…. A fresh observation returns the build token; explicit native prop/section selections and nonzero page offsets require that token. Pages cap at 32 props, 32 sections and 256 hardware-color vertices. Reports identify native ordinal/model/origin, ordered LOD sections, independent LDR/HDR files and raw encoded BGRA8/RGBA8 bytes, not linear irradiance or game brightness. Intact stale output remains historical; replaced/corrupt output clears every row. Missing channels and disabled ordinals have no fallback. Inspection changes no map, history, selection, camera, settings or drafts and accepts no file path. \
Use get_compile_region and preview_compile_region for native sealed build bounds; apply_compile_region or cancel_compile_region owns the returned token. Only committed enabled regions affect builds; disable them for full-map leak checks. Jobs capture their start scope. \
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
        Some(ids) if !ids.is_empty() => parse_ids(ids)?,
        _ => session.selected_ids(),
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

    fn session(&self) -> MutexGuard<'_, Session> {
        lock(&self.session)
    }

    fn admitted(&self) -> Result<Self, String> {
        let mut server = self.clone();
        if server.operation.is_none() {
            server.operation = server
                ._document
                .as_ref()
                .map(DocumentHandle::admit)
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
        let output = result.output;
        let diagnostics: Vec<Value> = output.report.diagnostics.iter().map(|d| json!({
            "level": match d.level { compiler::Level::Info => "info", compiler::Level::Warning => "warning", compiler::Level::Error => "error" },
            "message": d.message,
            "object": d.object,
            "position": d.position.map(point),
        })).collect();
        let value = json!({
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
        let server = match self.admitted() {
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

#[cfg(test)]
mod tests;

#[cfg(test)]
mod admission_tests;

#[cfg(test)]
mod headless_tests;

#[cfg(test)]
mod hdr_sky_tests;

#[cfg(test)]
mod baked_lightmap_tests;

#[cfg(test)]
mod lighting_preview_tests;
#[cfg(test)]
mod normal_map_tests;

#[cfg(test)]
mod additive_sprite_tests;

#[cfg(test)]
mod atlas_tests;

impl EditorServer {
    fn tool_router() -> ToolRouter<Self> {
        Self::assets_tools()
            + Self::atlas_tools()
            + Self::build_tools()
            + Self::compile_region_tools()
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
            + Self::shortcut_tools()
            + Self::history_tools()
            + Self::instance_contents_tools()
            + Self::instance_reference_tools()
            + Self::instance_extraction_tools()
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
            + Self::view_tools()
            + Self::working_region_tools()
    }
}
