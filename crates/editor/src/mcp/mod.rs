//! MCP server exposing the editor to agents. Each tool is a thin wrapper over
//! one [`Session`] operation, the same ones the UI calls, so agent edits show
//! up live in the editor window and share its undo history.

use std::collections::BTreeMap;

use std::io::Cursor;

use std::net::SocketAddr;

use std::path::{Path, PathBuf};

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

mod params;
mod rendering;
mod tools;
use params::*;
use rendering::{Offscreen, OffscreenState};

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

use source_formats::fgd::{Catalog, Class, ClassKind};

use source_formats::vmf::{Entity, Solid};

use crate::game;

use crate::identity::{id_string, parse_id};

use crate::runtime::{SharedSession, finish_compile_worker, launch, lock, start_compile};

use crate::session::{
    Actor, ConnectionFields, ConnectionRecord, ObjectFilter, PropertyChange, PropertyTarget,
    SelectMode, Session,
};

const DEFAULT_MATERIAL: &str = "dev/dev_measuregeneric01";

const DEFAULT_RENDER_SIZE: (u32, u32) = (1024, 576);

const MAX_RENDER_SIDE: u32 = 2048;

const INSTRUCTIONS: &str = "\
HammeRR, a map editor for Counter-Strike: Source (Valve Source engine, Hammer-compatible VMF files). \
The user has the same map open in an editor window and sees every change live; all edits \
are undoable and share the user's undo history.\n\
Coordinates: Z is up, units are Hammer units (about an inch). A player is 32x32x72 \
(crouched 36), steps are up to 18 high, doors are typically 56x112, walls 8-16 thick. \
Keep brush coordinates inside +-16384. set_spatial_settings configures the shared grid \
(0.125 to 4096 units), snapping, workplane elevation and brush depth.\n\
Objects are addressed as 'solid:<id>' (world brushes) and 'entity:<id>' (point or brush \
entities; a brush entity owns its brushes). Tools that take ids default to the current \
selection when ids are omitted, and creation tools select what they create.\n\
Materials are paths under materials/ without extension, e.g. 'tools/toolsnodraw', \
'tools/toolsskybox', 'dev/dev_measuregeneric01'; find more with search_materials.\n\
Verify work visually with render_view (focus on objects, or an explicit camera) and \
pick_at_pixel to identify what is at a point in a rendered image. Save with save_map. \
set_camera selects perspective/top/front/side; planar cameras accept center and ortho_height. \
frame_selection frames the selection or whole map. Named cameras persist through save_camera_view, get_camera_views, restore_camera_view, rename_camera_view and delete_camera_view; use generation/revision guards for stale-state protection. draw_block uses plane coordinates \
(top XY, front XZ, side YZ), shared snapping and depth; preview=true leaves the map \
and undo history untouched. Inspect get_map_info.block_preview and cancel with cancel_block_preview. \
transform_objects applies exact move/rotation/positive scale, brush reflection and single-axis shear, with an optional world-space pivot \
(default: selection bounds center) and independent texture_lock (move/rotation) and texture_scale_lock (stretch); omitted scale lock inherits movement locking. Rotation is world X/Y/Z, in degrees. \
space: local uses the first selected object's frame; preview: true creates a shared transform draft. \
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
get_compile_settings and set_compile_settings select shared build lighting (fullbright/direct_ldr) and physics (none/engine). Engine physics uses the verified installed CS:S Linux64 library in an isolated worker for world, clip, brush-model and horizontal world-fluid bodies; terrain contact meshes are supported and static props use model assets; world/terrain triangles retain VMT surface properties through a checked 127-entry native table; brush-model surfaces, density and compiler-derived masses remain unfinished. \
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
        "map": log.map, "console": log.console, "launcher": log.launcher, "truncated": log.truncated})
}

#[derive(Clone)]
pub struct EditorServer {
    session: SharedSession,
    offscreen: Arc<Mutex<OffscreenState>>,
    model_renderer: Arc<Mutex<crate::model_preview::PreviewRenderer>>,
    tool_router: ToolRouter<Self>,
}

impl EditorServer {
    pub fn new(session: SharedSession, ctx: Option<GpuContext>) -> Self {
        Self {
            session,
            model_renderer: Arc::default(),
            offscreen: Arc::new(Mutex::new(OffscreenState {
                renderer: ctx.map(Offscreen::new),
                error: None,
            })),
            tool_router: Self::tool_router(),
        }
    }

    fn session(&self) -> MutexGuard<'_, Session> {
        lock(&self.session)
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
        if play {
            let session = self.session();
            if session.game_running() || session.launching() {
                return fail(
                    "CS:S is already launching or running; close it before running another map",
                );
            }
        }
        let request = {
            let path = args.path.as_ref().map(PathBuf::from);
            match self.session().begin_compile(Actor::Agent, path.as_deref()) {
                Ok(request) => request,
                Err(message) => return fail(message),
            }
        };
        let session = self.session.clone();
        let worker = tokio::task::spawn_blocking(move || {
            let result = request.run();
            let game = finish_compile_worker(&session, Actor::Agent, &result, play)?;
            let result = result?;
            Ok((result, game))
        })
        .await;
        let result = match worker {
            Ok(result) => result,
            Err(error) => {
                let error = error.to_string();
                let mut session = self.session();
                session.finish_compile(Actor::Agent, &Err(error.clone()));
                if play {
                    let _ = session.finish_launch(Actor::Agent, Err(error.clone()));
                }
                Err(error)
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
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(rmcp::model::Implementation::new(
                "hammerr",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(INSTRUCTIONS)
    }
}

/// Serves MCP over streamable HTTP at `http://<addr>/mcp` until the process
/// exits. Runs on its own thread with its own async runtime.
pub fn spawn_http(server: EditorServer, addr: SocketAddr) -> Result<SocketAddr, String> {
    use rmcp::transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    let listener = runtime
        .block_on(tokio::net::TcpListener::bind(addr))
        .map_err(|e| format!("MCP server could not listen on {addr}: {e}"))?;
    let local = listener.local_addr().map_err(|e| e.to_string())?;
    std::thread::Builder::new()
        .name("mcp-http".into())
        .spawn(move || {
            let service: StreamableHttpService<EditorServer, LocalSessionManager> =
                StreamableHttpService::new(
                    move || Ok(server.clone()),
                    Default::default(),
                    StreamableHttpServerConfig::default(),
                );
            let router = axum::Router::new().nest_service("/mcp", service);
            if let Err(e) = runtime.block_on(async { axum::serve(listener, router).await }) {
                eprintln!("MCP server stopped: {e}");
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(local)
}

/// Serves MCP over stdin/stdout until the client disconnects.
pub fn serve_stdio(server: EditorServer) -> Result<(), String> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    runtime.block_on(async {
        let running = server
            .serve(rmcp::transport::stdio())
            .await
            .map_err(|e| e.to_string())?;
        running.waiting().await.map(drop).map_err(|e| e.to_string())
    })
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod headless_tests;

impl EditorServer {
    fn tool_router() -> ToolRouter<Self> {
        Self::assets_tools()
            + Self::build_tools()
            + Self::checks_tools()
            + Self::clipboard_tools()
            + Self::components_tools()
            + Self::context_tools()
            + Self::entities_tools()
            + Self::files_tools()
            + Self::geometry_tools()
            + Self::groups_tools()
            + Self::materials_tools()
            + Self::partitions_tools()
            + Self::placement_tools()
            + Self::recovery_tools()
            + Self::selection_tools()
            + Self::terrain_tools()
            + Self::view_tools()
    }
}
