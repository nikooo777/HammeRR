//! MCP server exposing the editor to agents. Each tool is a thin wrapper over
//! one [`Session`] operation, the same ones the UI calls, so agent edits show
//! up live in the editor window and share its undo history.

use std::collections::BTreeMap;
use std::io::Cursor;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use base64::Engine;
use glam::{DVec3, Vec3};
use map_core::edit::{self, FaceTarget};
use map_core::query::{self, Hit, Ray};
use map_core::{Document, ObjectId, Plane};
use render::{Camera, GpuContext, GpuScene, Renderer, ViewOptions};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, ContentBlock, ServerCapabilities, ServerConfig};
use rmcp::{ErrorData, ServerHandler, ServiceExt, schemars, tool, tool_handler, tool_router};
use serde::Deserialize;
use serde_json::{Value, json};
use source_formats::vmf::{Entity, Solid};

use crate::game;
use crate::session::{Actor, SelectMode, Session};

pub type SharedSession = Arc<Mutex<Session>>;

pub fn lock(session: &Mutex<Session>) -> MutexGuard<'_, Session> {
    session.lock().unwrap_or_else(PoisonError::into_inner)
}

pub fn launch(session: &SharedSession, actor: Actor) -> Result<game::Log, String> {
    let request = {
        let mut state = lock(session);
        match state.begin_launch(actor) {
            Ok(request) => request,
            Err(error) => {
                state.log(actor, format!("game launch failed: {error}"));
                return Err(error);
            }
        }
    };
    let result = request.run();
    lock(session).finish_launch(actor, result)
}

const DEFAULT_MATERIAL: &str = "dev/dev_measuregeneric01";
const DEFAULT_RENDER_SIZE: (u32, u32) = (1024, 576);
const MAX_RENDER_SIDE: u32 = 2048;

const INSTRUCTIONS: &str = "\
HammeRR, a map editor for Counter-Strike: Source (Valve Source engine, Hammer-compatible VMF files). \
The user has the same map open in an editor window and sees every change live; all edits \
are undoable and share the user's undo history.\n\
Coordinates: Z is up, units are Hammer units (about an inch). A player is 32x32x72 \
(crouched 36), steps are up to 18 high, doors are typically 56x112, walls 8-16 thick. \
Keep brush coordinates on a grid (16, 8 or 1) and inside +-16384.\n\
Objects are addressed as 'solid:<id>' (world brushes) and 'entity:<id>' (point or brush \
entities; a brush entity owns its brushes). Tools that take ids default to the current \
selection when ids are omitted, and creation tools select what they create.\n\
Materials are paths under materials/ without extension, e.g. 'tools/toolsnodraw', \
'tools/toolsskybox', 'dev/dev_measuregeneric01'; find more with search_materials.\n\
Verify work visually with render_view (focus on objects, or an explicit camera) and \
pick_at_pixel to identify what is at a point in a rendered image. Save with save_map. \
Compile with compile_map or compile and play with run_map; inspect engine output with \
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

pub fn id_string(id: ObjectId) -> String {
    match id {
        ObjectId::Solid(n) => format!("solid:{n}"),
        ObjectId::Entity(n) => format!("entity:{n}"),
    }
}

pub fn parse_id(text: &str) -> Result<ObjectId, String> {
    let bad = || format!("bad object id {text:?}; expected 'solid:<n>' or 'entity:<n>'");
    let (kind, number) = text.trim().split_once([':', ' ']).ok_or_else(bad)?;
    let number = number.trim().parse().map_err(|_| bad())?;
    match kind.to_ascii_lowercase().as_str() {
        "solid" | "brush" => Ok(ObjectId::Solid(number)),
        "entity" => Ok(ObjectId::Entity(number)),
        _ => Err(bad()),
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

fn round(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

fn point(v: DVec3) -> Value {
    json!([round(v.x), round(v.y), round(v.z)])
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
        .map(|s| json!({ "solid": s.id, "bounds": bounds_json(query::solid_bounds(s)), "materials": materials_of([s]) }))
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
        "distance": round(hit.distance),
        "point": point(hit.point),
    })
}

fn camera_json(camera: &Camera) -> Value {
    json!({
        "position": point(camera.position.as_dvec3()),
        "yaw": round(f64::from(camera.yaw_degrees)),
        "pitch": round(f64::from(camera.pitch_degrees)),
        "fov": round(f64::from(camera.fov_y_degrees)),
        "forward": point(camera.forward().as_dvec3()),
    })
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct CameraSpec {
    /// Frame these objects (ids). Viewed from the editor camera's direction.
    pub focus: Option<Vec<String>>,
    /// Eye position [x, y, z].
    pub position: Option<[f32; 3]>,
    /// Point to look at from `position`; overrides yaw/pitch.
    pub look_at: Option<[f32; 3]>,
    /// Heading in degrees; 0 looks along +X, 90 along +Y.
    pub yaw: Option<f32>,
    /// Degrees above the horizon; negative looks down.
    pub pitch: Option<f32>,
    /// Vertical field of view in degrees.
    pub fov: Option<f32>,
}

impl CameraSpec {
    /// Starts from the editor camera and applies the given overrides.
    fn resolve(&self, session: &Session) -> Result<Camera, String> {
        let mut camera = session.camera();
        if let Some(fov) = self.fov {
            camera.fov_y_degrees = fov.clamp(10.0, 150.0);
        }
        if let Some(focus) = &self.focus {
            let ids = parse_ids(focus)?;
            let (lo, hi) =
                query::bounds_of(session.document(), &ids).ok_or("focus objects have no bounds")?;
            return Ok(Camera::framing(
                lo.as_vec3(),
                hi.as_vec3(),
                camera.forward(),
                camera.fov_y_degrees,
            ));
        }
        if let Some(p) = self.position {
            camera.position = Vec3::from_array(p);
        }
        if let Some(target) = self.look_at {
            let fov = camera.fov_y_degrees;
            camera = Camera::looking_at(camera.position, Vec3::from_array(target));
            camera.fov_y_degrees = fov;
        } else {
            camera.yaw_degrees = self.yaw.unwrap_or(camera.yaw_degrees);
            camera.pitch_degrees = self
                .pitch
                .unwrap_or(camera.pitch_degrees)
                .clamp(-89.0, 89.0);
        }
        Ok(camera)
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct OpenMapArgs {
    /// Absolute path of a .vmf file.
    pub path: String,
    /// Open even if the current map has unsaved changes, losing them.
    #[serde(default)]
    pub discard_changes: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct NewMapArgs {
    #[serde(default)]
    pub discard_changes: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SaveMapArgs {
    /// Where to save; defaults to the map's current file.
    pub path: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FindArgs {
    /// Case-insensitive substring of the entity classname (e.g. 'info_player', 'func_').
    pub classname: Option<String>,
    /// Case-insensitive substring of targetname.
    pub targetname: Option<String>,
    /// Case-insensitive substring of a face material.
    pub material: Option<String>,
    /// Only objects whose bounds intersect this box: [[minx,miny,minz],[maxx,maxy,maxz]].
    pub within: Option<[[f64; 3]; 2]>,
    /// 'solid', 'entity', 'point_entity' or 'brush_entity'.
    pub kind: Option<String>,
    /// Maximum results (default 50, at most 500).
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct IdsArgs {
    /// Object ids; defaults to the selection.
    pub ids: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SelectArgs {
    pub ids: Vec<String>,
    /// 'replace' (default), 'add', 'remove' or 'toggle'.
    pub mode: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateBlockArgs {
    /// Box minimum corner; give mins+maxs or center+size.
    pub mins: Option<[f64; 3]>,
    pub maxs: Option<[f64; 3]>,
    pub center: Option<[f64; 3]>,
    pub size: Option<[f64; 3]>,
    /// Material for all faces (default dev/dev_measuregeneric01).
    pub material: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateEntityArgs {
    pub classname: String,
    /// Position [x, y, z] for point entities.
    pub origin: Option<[f64; 3]>,
    /// Extra key/values, e.g. {"targetname": "door1", "angles": "0 90 0"}.
    pub properties: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SetPropertiesArgs {
    /// Entity id ('entity:<n>'); defaults to the single selected entity.
    pub id: Option<String>,
    /// Keys to set.
    pub set: Option<BTreeMap<String, String>>,
    /// Keys to remove.
    pub remove: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct MoveArgs {
    pub ids: Option<Vec<String>>,
    /// Translation [dx, dy, dz].
    pub offset: [f64; 3],
    /// Keep textures fixed to the brushes (default true).
    pub texture_lock: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DuplicateArgs {
    pub ids: Option<Vec<String>>,
    /// Offset of the copies [dx, dy, dz].
    pub offset: [f64; 3],
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FaceRef {
    pub solid: u32,
    pub side: u32,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SetMaterialArgs {
    pub material: String,
    /// Objects whose faces all get the material; defaults to the selection
    /// when `faces` is also absent.
    pub ids: Option<Vec<String>>,
    /// Individual faces, as reported by get_objects or pick_at_pixel.
    pub faces: Option<Vec<FaceRef>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TieArgs {
    /// World brushes ('solid:<n>'); defaults to the selection.
    pub ids: Option<Vec<String>>,
    /// Brush entity class, e.g. func_detail, func_door, trigger_multiple.
    pub classname: String,
    pub properties: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct EntityArgs {
    /// Entity id; defaults to the single selected entity.
    pub id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct StepsArgs {
    /// How many steps (default 1).
    pub steps: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SetViewArgs {
    /// Draw clip, trigger, nodraw and skybox brushes.
    pub tool_textures: Option<bool>,
    /// Show the current compilation's leak path through geometry.
    pub leak_path: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RenderArgs {
    /// Camera overrides; defaults to the user's current view.
    pub camera: Option<CameraSpec>,
    /// Image size in pixels (default 1024x576).
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Draw tool brushes (nodraw, clip, trigger, skybox). Defaults to the editor setting.
    pub tool_textures: Option<bool>,
    pub leak_path: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PickArgs {
    /// Pixel column and row in an image rendered with the same camera and size.
    pub x: f32,
    pub y: f32,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub camera: Option<CameraSpec>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RaycastArgs {
    pub origin: [f64; 3],
    pub direction: [f64; 3],
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchMaterialsArgs {
    /// Case-insensitive substring, e.g. 'brick', 'cs_italy', 'tools/'.
    pub query: String,
    /// Maximum results (default 50).
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CompileArgs {
    /// Output .bsp path; defaults to the current map filename with a .bsp extension.
    pub path: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GameLogArgs {
    /// Tail lines per log (default 200, maximum 2000).
    pub lines: Option<usize>,
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

struct Offscreen {
    ctx: GpuContext,
    renderer: Renderer,
    scene: Option<(u64, GpuScene)>,
}

#[derive(Clone)]
pub struct EditorServer {
    session: SharedSession,
    offscreen: Arc<Mutex<Offscreen>>,
    tool_router: ToolRouter<Self>,
}

impl EditorServer {
    pub fn new(session: SharedSession, ctx: GpuContext) -> Self {
        Self {
            session,
            offscreen: Arc::new(Mutex::new(Offscreen {
                renderer: Renderer::new(&ctx),
                ctx,
                scene: None,
            })),
            tool_router: Self::tool_router(),
        }
    }

    fn session(&self) -> MutexGuard<'_, Session> {
        lock(&self.session)
    }

    fn render_png(&self, camera: &Camera, size: (u32, u32), view: ViewOptions) -> Vec<u8> {
        let mut offscreen = self
            .offscreen
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Offscreen {
            ctx,
            renderer,
            scene,
        } = &mut *offscreen;
        {
            let mut session = self.session();
            let serial = session.scene_serial();
            if scene.as_ref().is_none_or(|(s, _)| *s != serial) {
                let mesh = session.build_scene();
                *scene = Some((serial, renderer.upload(ctx, &mesh, session.materials())));
            }
        }
        let gpu_scene = &scene.as_ref().expect("scene was just built").1;
        let pixels = renderer.render_to_rgba(ctx, gpu_scene, camera, &view, size.0, size.1);
        let mut png = Vec::new();
        let mut encoder = png::Encoder::new(Cursor::new(&mut png), size.0, size.1);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .and_then(|mut w| w.write_image_data(&pixels))
            .expect("encoding a PNG in memory cannot fail");
        png
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

#[tool_router]
impl EditorServer {
    #[tool(
        description = "Compile the current map snapshot into a fullbright Source BSP. Returns structured diagnostics and a leak path on failure. Does not save the VMF; writes a .lin pointfile for leaks."
    )]
    async fn compile_map(&self, Parameters(args): Parameters<CompileArgs>) -> ToolResult {
        self.compile_snapshot(args, false).await
    }

    #[tool(
        description = "Compile the current unsaved map snapshot and launch it in the installed Linux CS:S client. Returns compile diagnostics, leak paths, and game process details. Does not save the VMF."
    )]
    async fn run_map(&self, Parameters(args): Parameters<CompileArgs>) -> ToolResult {
        self.compile_snapshot(args, true).await
    }

    async fn compile_snapshot(&self, args: CompileArgs, play: bool) -> ToolResult {
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
            lock(&session).finish_compile(Actor::Agent, &result);
            let result = result?;
            let game = if play && result.output.bsp.is_some() {
                Some(launch(&session, Actor::Agent)?)
            } else {
                None
            };
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
            "stats": { "brushes": output.stats.brushes, "nodes": output.stats.nodes, "leaves": output.stats.leaves, "faces": output.stats.faces, "models": output.stats.models, "elapsed_ms": output.stats.elapsed.as_millis() },
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

    #[tool(
        description = "Launch the latest successful compile of the current map in Linux CS:S. Rejects outdated compiles and duplicate launches. Use run_map to compile and launch in one call."
    )]
    async fn launch_game(&self) -> ToolResult {
        let session = self.session.clone();
        let result = tokio::task::spawn_blocking(move || launch(&session, Actor::Agent)).await;
        match result {
            Ok(Ok(log)) => ok(game_log_json(&log)),
            Ok(Err(error)) => fail(error),
            Err(error) => {
                let error = error.to_string();
                let _ = self
                    .session()
                    .finish_launch(Actor::Agent, Err(error.clone()));
                fail(error)
            }
        }
    }

    #[tool(
        description = "Read bounded tails of the most recent CS:S run's engine console and launcher output, including process status and exit code. Logs remain available after the game exits.",
        annotations(read_only_hint = true)
    )]
    fn game_log(&self, Parameters(args): Parameters<GameLogArgs>) -> ToolResult {
        run(|| {
            self.session()
                .game_log(args.lines.unwrap_or(game::DEFAULT_LOG_LINES))
                .map(|log| game_log_json(&log))
        })
    }

    #[tool(
        description = "Stop only the CS:S process launched by this HammeRR session. The game log remains readable afterward."
    )]
    async fn stop_game(&self) -> ToolResult {
        let session = self.session.clone();
        match tokio::task::spawn_blocking(move || lock(&session).stop_game(Actor::Agent)).await {
            Ok(Ok(())) => ok(json!({"stopped": true})),
            Ok(Err(error)) => fail(error),
            Err(error) => fail(error.to_string()),
        }
    }

    #[tool(
        description = "Summary of the open map: file, unsaved state, object counts, bounds, selection, undo history, camera and recent activity by the user and agents.",
        annotations(read_only_hint = true)
    )]
    fn get_map_info(&self) -> ToolResult {
        let mut session = self.session();
        session.poll_game();
        let doc = session.document();
        let vmf = doc.vmf();
        let point_entities = vmf.entities.iter().filter(|e| e.solids.is_empty()).count();
        let bounds = query::union(doc.objects().filter_map(|id| query::bounds(doc, id)));
        let activity: Vec<String> = session
            .recent_log()
            .take(10)
            .map(|e| format!("{}: {}", e.actor.label(), e.text))
            .collect();
        ok(json!({
            "path": session.path().map(|p| p.display().to_string()),
            "title": session.title(),
            "unsaved_changes": doc.is_dirty(),
            "game": session.game().game_dir.display().to_string(),
            "game_error": session.game_error(),
            "compiling": session.compiling(),
            "launching": session.launching(),
            "game_running": session.game_running(),
            "leak": session.leak().map(|leak| json!({"entity": format!("entity:{}", leak.entity), "points": leak.points.iter().copied().map(point).collect::<Vec<_>>()})),
            "view": {"tool_textures": session.view().tool_textures, "leak_path": session.view().leak_path},
            "world_brushes": vmf.world.solids.len(),
            "point_entities": point_entities,
            "brush_entities": vmf.entities.len() - point_entities,
            "worldspawn": vmf.world.properties.iter().map(|(k, v)| json!([k, v])).collect::<Vec<_>>(),
            "bounds": bounds_json(bounds),
            "selection": session.selection().iter().map(|&id| id_string(id)).collect::<Vec<_>>(),
            "undo": doc.undo_labels().take(10).collect::<Vec<_>>(),
            "redo": doc.redo_labels().take(10).collect::<Vec<_>>(),
            "camera": camera_json(&session.camera()),
            "recent_activity": activity,
        }))
    }

    #[tool(description = "Open a .vmf map file in the editor.")]
    fn open_map(&self, Parameters(args): Parameters<OpenMapArgs>) -> ToolResult {
        run(|| {
            let message = self.session().open(
                Actor::Agent,
                &PathBuf::from(&args.path),
                args.discard_changes,
            )?;
            Ok(json!({ "message": message }))
        })
    }

    #[tool(description = "Start a new empty map.")]
    fn new_map(&self, Parameters(args): Parameters<NewMapArgs>) -> ToolResult {
        run(|| {
            self.session().new_map(Actor::Agent, args.discard_changes)?;
            Ok(json!({ "message": "new map" }))
        })
    }

    #[tool(description = "Save the map as .vmf, to its current file or a new path.")]
    fn save_map(&self, Parameters(args): Parameters<SaveMapArgs>) -> ToolResult {
        run(|| {
            let path = self.session().save(
                Actor::Agent,
                args.path.as_ref().map(PathBuf::from).as_deref(),
            )?;
            Ok(json!({ "saved": path.display().to_string() }))
        })
    }

    #[tool(
        description = "Find objects by classname, targetname, material, kind and/or a bounding box. Returns summaries with ids and bounds.",
        annotations(read_only_hint = true)
    )]
    fn find_objects(&self, Parameters(args): Parameters<FindArgs>) -> ToolResult {
        let session = self.session();
        let doc = session.document();
        let lower = |s: &Option<String>| s.as_ref().map(|s| s.to_ascii_lowercase());
        let (classname, targetname, material) = (
            lower(&args.classname),
            lower(&args.targetname),
            lower(&args.material),
        );
        let contains = |value: Option<&str>, needle: &Option<String>| {
            needle
                .as_ref()
                .is_none_or(|n| value.is_some_and(|v| v.to_ascii_lowercase().contains(n)))
        };
        let matches = |id: ObjectId| -> bool {
            let (solids, entity): (Vec<&Solid>, Option<&Entity>) = match id {
                ObjectId::Solid(n) => (doc.solid(n).into_iter().collect(), None),
                ObjectId::Entity(n) => match doc.entity(n) {
                    Some(e) => (e.solids.iter().collect(), Some(e)),
                    None => return false,
                },
            };
            let kind_ok = args.kind.as_deref().is_none_or(|k| match k {
                "solid" => entity.is_none(),
                "entity" => entity.is_some(),
                "point_entity" => entity.is_some_and(|e| e.solids.is_empty()),
                "brush_entity" => entity.is_some_and(|e| !e.solids.is_empty()),
                _ => false,
            });
            let names_ok = if classname.is_some() || targetname.is_some() {
                entity.is_some_and(|e| {
                    contains(e.classname(), &classname)
                        && contains(e.properties.get("targetname"), &targetname)
                })
            } else {
                true
            };
            let material_ok = material.as_ref().is_none_or(|m| {
                solids.iter().any(|s| {
                    s.sides
                        .iter()
                        .any(|f| f.material.to_ascii_lowercase().contains(m))
                })
            });
            let within_ok = args.within.is_none_or(|[lo, hi]| {
                query::bounds(doc, id)
                    .is_some_and(|(a, b)| a.cmple(vec3(hi)).all() && b.cmpge(vec3(lo)).all())
            });
            kind_ok && names_ok && material_ok && within_ok
        };
        let found: Vec<ObjectId> = doc.objects().filter(|&id| matches(id)).collect();
        let limit = args.limit.unwrap_or(50).min(500);
        let offset = args.offset.unwrap_or(0);
        let page: Vec<Value> = found
            .iter()
            .skip(offset)
            .take(limit)
            .map(|&id| summary(doc, id))
            .collect();
        ok(json!({ "total": found.len(), "offset": offset, "objects": page }))
    }

    #[tool(
        description = "Full details of objects: entity key/values, outputs and brushes; brush faces with side ids, materials, normals, planes and texture axes. Defaults to the selection.",
        annotations(read_only_hint = true)
    )]
    fn get_objects(&self, Parameters(args): Parameters<IdsArgs>) -> ToolResult {
        run(|| {
            let session = self.session();
            let ids = ids_or_selection(&session, args.ids.as_deref())?;
            let objects = ids
                .iter()
                .map(|&id| detail(session.document(), id))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(json!({ "objects": objects }))
        })
    }

    #[tool(
        description = "Change the selection, which the user sees highlighted and which other tools default to."
    )]
    fn select(&self, Parameters(args): Parameters<SelectArgs>) -> ToolResult {
        run(|| {
            let mode = match args.mode.as_deref().unwrap_or("replace") {
                "replace" => SelectMode::Replace,
                "add" => SelectMode::Add,
                "remove" => SelectMode::Remove,
                "toggle" => SelectMode::Toggle,
                other => return Err(format!("unknown mode {other:?}")),
            };
            let mut session = self.session();
            session.select(&parse_ids(&args.ids)?, mode)?;
            let selection: Vec<String> = session
                .selection()
                .iter()
                .map(|&id| id_string(id))
                .collect();
            Ok(json!({ "selection": selection }))
        })
    }

    #[tool(
        description = "Selected objects with their summaries and combined bounds.",
        annotations(read_only_hint = true)
    )]
    fn get_selection(&self) -> ToolResult {
        let session = self.session();
        let ids = session.selected_ids();
        let doc = session.document();
        ok(json!({
            "objects": ids.iter().map(|&id| summary(doc, id)).collect::<Vec<_>>(),
            "bounds": bounds_json(query::bounds_of(doc, &ids)),
        }))
    }

    #[tool(
        description = "Create an axis-aligned box brush in the world, given mins+maxs or center+size, and select it."
    )]
    fn create_block(&self, Parameters(args): Parameters<CreateBlockArgs>) -> ToolResult {
        run(|| {
            let (mins, maxs) = match (args.mins, args.maxs, args.center, args.size) {
                (Some(lo), Some(hi), None, None) => (vec3(lo), vec3(hi)),
                (None, None, Some(c), Some(s)) => {
                    (vec3(c) - vec3(s) * 0.5, vec3(c) + vec3(s) * 0.5)
                }
                _ => return Err("give either mins and maxs, or center and size".into()),
            };
            let material = args.material.as_deref().unwrap_or(DEFAULT_MATERIAL);
            let mut session = self.session();
            let id = session.edit(Actor::Agent, "create block", |tx| {
                edit::create_block(tx, mins, maxs, material)
            })?;
            session.select(&[id], SelectMode::Replace)?;
            Ok(summary(session.document(), id))
        })
    }

    #[tool(
        description = "Create a point entity (e.g. info_player_terrorist, light, prop_static with model) and select it."
    )]
    fn create_entity(&self, Parameters(args): Parameters<CreateEntityArgs>) -> ToolResult {
        run(|| {
            let properties: Vec<(String, String)> =
                args.properties.unwrap_or_default().into_iter().collect();
            let mut session = self.session();
            let label = format!("create {}", args.classname);
            let id = session.edit(Actor::Agent, &label, |tx| {
                edit::create_entity(
                    tx,
                    &args.classname,
                    args.origin.map(vec3),
                    &properties,
                    Vec::new(),
                )
            })?;
            session.select(&[id], SelectMode::Replace)?;
            Ok(summary(session.document(), id))
        })
    }

    #[tool(
        description = "Set and/or remove key/values on an entity (including classname, origin, angles, targetname)."
    )]
    fn set_entity_properties(&self, Parameters(args): Parameters<SetPropertiesArgs>) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let entity = single_entity(&session, args.id.as_deref())?;
            let mut changes: Vec<(String, Option<String>)> = args
                .set
                .unwrap_or_default()
                .into_iter()
                .map(|(k, v)| (k, Some(v)))
                .collect();
            changes.extend(
                args.remove
                    .unwrap_or_default()
                    .into_iter()
                    .map(|k| (k, None)),
            );
            session.edit(Actor::Agent, "edit entity properties", |tx| {
                edit::set_properties(tx, entity, &changes)
            })?;
            detail(session.document(), ObjectId::Entity(entity))
        })
    }

    #[tool(
        description = "Delete objects (defaults to the selection).",
        annotations(destructive_hint = true)
    )]
    fn delete_objects(&self, Parameters(args): Parameters<IdsArgs>) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let ids = ids_or_selection(&session, args.ids.as_deref())?;
            session.edit(
                Actor::Agent,
                &format!("delete {} objects", ids.len()),
                |tx| edit::delete(tx, &ids),
            )?;
            Ok(json!({ "deleted": ids.iter().map(|&id| id_string(id)).collect::<Vec<_>>() }))
        })
    }

    #[tool(description = "Move objects by an offset (defaults to the selection).")]
    fn move_objects(&self, Parameters(args): Parameters<MoveArgs>) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let ids = ids_or_selection(&session, args.ids.as_deref())?;
            let lock = args.texture_lock.unwrap_or(true);
            session.edit(Actor::Agent, &format!("move {} objects", ids.len()), |tx| {
                edit::translate(tx, &ids, vec3(args.offset), lock)
            })?;
            let doc = session.document();
            Ok(json!({ "objects": ids.iter().map(|&id| summary(doc, id)).collect::<Vec<_>>() }))
        })
    }

    #[tool(
        description = "Copy objects with an offset and select the copies (defaults to the selection)."
    )]
    fn duplicate_objects(&self, Parameters(args): Parameters<DuplicateArgs>) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let ids = ids_or_selection(&session, args.ids.as_deref())?;
            let copies = session.edit(
                Actor::Agent,
                &format!("duplicate {} objects", ids.len()),
                |tx| edit::duplicate(tx, &ids, vec3(args.offset)),
            )?;
            session.select(&copies, SelectMode::Replace)?;
            let doc = session.document();
            Ok(json!({ "copies": copies.iter().map(|&id| summary(doc, id)).collect::<Vec<_>>() }))
        })
    }

    #[tool(
        description = "Apply a material to whole objects or to individual faces (defaults to the selection)."
    )]
    fn set_material(&self, Parameters(args): Parameters<SetMaterialArgs>) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let mut targets: Vec<FaceTarget> = args
                .faces
                .iter()
                .flatten()
                .map(|f| FaceTarget::Side {
                    solid: f.solid,
                    side: f.side,
                })
                .collect();
            if args.ids.is_some() || targets.is_empty() {
                targets.extend(
                    ids_or_selection(&session, args.ids.as_deref())?
                        .into_iter()
                        .map(FaceTarget::Object),
                );
            }
            let changed =
                session.edit(Actor::Agent, &format!("apply {}", args.material), |tx| {
                    edit::set_material(tx, &targets, &args.material)
                })?;
            Ok(json!({ "faces_changed": changed }))
        })
    }

    #[tool(
        description = "Turn world brushes into one brush entity (func_detail, func_door, trigger_*, ...) and select it."
    )]
    fn tie_to_entity(&self, Parameters(args): Parameters<TieArgs>) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let ids = ids_or_selection(&session, args.ids.as_deref())?;
            let solids = ids
                .iter()
                .map(|&id| match id {
                    ObjectId::Solid(n) => Ok(n),
                    ObjectId::Entity(_) => Err(format!("{} is not a world brush", id_string(id))),
                })
                .collect::<Result<Vec<_>, _>>()?;
            let properties: Vec<(String, String)> =
                args.properties.unwrap_or_default().into_iter().collect();
            let entity =
                session.edit(Actor::Agent, &format!("tie to {}", args.classname), |tx| {
                    edit::tie_to_entity(tx, &solids, &args.classname, &properties)
                })?;
            session.select(&[entity], SelectMode::Replace)?;
            detail(session.document(), entity)
        })
    }

    #[tool(description = "Move a brush entity's brushes back into the world, deleting the entity.")]
    fn move_to_world(&self, Parameters(args): Parameters<EntityArgs>) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let entity = single_entity(&session, args.id.as_deref())?;
            let solids = session.edit(Actor::Agent, "move to world", |tx| {
                edit::move_to_world(tx, entity)
            })?;
            session.select(&solids, SelectMode::Replace)?;
            Ok(json!({ "solids": solids.iter().map(|&id| id_string(id)).collect::<Vec<_>>() }))
        })
    }

    #[tool(description = "Undo the last steps, whoever made them.")]
    fn undo(&self, Parameters(args): Parameters<StepsArgs>) -> ToolResult {
        let mut session = self.session();
        let undone: Vec<String> = (0..args.steps.unwrap_or(1))
            .map_while(|_| session.undo(Actor::Agent))
            .collect();
        ok(json!({ "undone": undone }))
    }

    #[tool(
        description = "Show or hide tool textures and the current compile's leak path in the viewport."
    )]
    fn set_view(&self, Parameters(args): Parameters<SetViewArgs>) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let mut view = session.view();
            if let Some(tools) = args.tool_textures {
                view.tool_textures = tools;
            }
            if let Some(leak) = args.leak_path {
                view.leak_path = leak;
            }
            session.set_view(view);
            Ok(json!({ "tool_textures": view.tool_textures, "leak_path": view.leak_path }))
        })
    }

    #[tool(description = "Redo undone steps.")]
    fn redo(&self, Parameters(args): Parameters<StepsArgs>) -> ToolResult {
        let mut session = self.session();
        let redone: Vec<String> = (0..args.steps.unwrap_or(1))
            .map_while(|_| session.redo(Actor::Agent))
            .collect();
        ok(json!({ "redone": redone }))
    }

    #[tool(
        description = "The user's current viewport camera.",
        annotations(read_only_hint = true)
    )]
    fn get_camera(&self) -> ToolResult {
        ok(camera_json(&self.session().camera()))
    }

    #[tool(
        description = "Move the user's viewport camera, e.g. to show them something (focus on ids, or position + look_at)."
    )]
    fn set_camera(&self, Parameters(spec): Parameters<CameraSpec>) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let camera = spec.resolve(&session)?;
            session.set_camera(camera);
            session.log(Actor::Agent, "moved the camera");
            Ok(camera_json(&camera))
        })
    }

    #[tool(
        description = "Render the map to a PNG with the editor's renderer (textured, selection highlighted in red). Defaults to the user's view; pass camera.focus to frame objects.",
        annotations(read_only_hint = true)
    )]
    fn render_view(&self, Parameters(args): Parameters<RenderArgs>) -> ToolResult {
        let (camera, mut view) = {
            let session = self.session();
            match args.camera.unwrap_or_default().resolve(&session) {
                Ok(camera) => (camera, session.view()),
                Err(message) => return fail(message),
            }
        };
        if let Some(tools) = args.tool_textures {
            view.tool_textures = tools;
        }
        if let Some(leak) = args.leak_path {
            view.leak_path = leak;
        }
        let size = render_size(args.width, args.height);
        let png = self.render_png(&camera, size, view);
        let data = base64::engine::general_purpose::STANDARD.encode(png);
        let info = json!({ "camera": camera_json(&camera), "width": size.0, "height": size.1,
            "view": { "tool_textures": view.tool_textures, "leak_path": view.leak_path } });
        Ok(CallToolResult::success(vec![
            ContentBlock::image(data, "image/png"),
            ContentBlock::text(info.to_string()),
        ]))
    }

    #[tool(
        description = "Identify the object and face at a pixel of an image rendered by render_view with the same camera and size.",
        annotations(read_only_hint = true)
    )]
    fn pick_at_pixel(&self, Parameters(args): Parameters<PickArgs>) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let camera = args.camera.unwrap_or_default().resolve(&session)?;
            let (w, h) = render_size(args.width, args.height);
            let ndc_x = 2.0 * (args.x + 0.5) / w as f32 - 1.0;
            let ndc_y = 1.0 - 2.0 * (args.y + 0.5) / h as f32;
            let (origin, direction) = camera.ray(ndc_x, ndc_y, w as f32 / h as f32);
            let ray = Ray {
                origin: origin.as_dvec3(),
                direction: direction.as_dvec3(),
            };
            let hit = session.pick(&ray);
            Ok(hit.map_or(json!({ "hit": null }), |hit| {
                hit_json(session.document(), &hit)
            }))
        })
    }

    #[tool(
        description = "Cast a ray and report the first visible object and face hit, e.g. to find the floor below a point.",
        annotations(read_only_hint = true)
    )]
    fn raycast(&self, Parameters(args): Parameters<RaycastArgs>) -> ToolResult {
        run(|| {
            if vec3(args.direction).length_squared() == 0.0 {
                return Err("direction must not be zero".into());
            }
            let mut session = self.session();
            let ray = Ray {
                origin: vec3(args.origin),
                direction: vec3(args.direction),
            };
            let hit = session.pick(&ray);
            Ok(hit.map_or(json!({ "hit": null }), |hit| {
                hit_json(session.document(), &hit)
            }))
        })
    }

    #[tool(
        description = "Search the game's materials by name.",
        annotations(read_only_hint = true)
    )]
    fn search_materials(&self, Parameters(args): Parameters<SearchMaterialsArgs>) -> ToolResult {
        let names = self
            .session()
            .search_materials(&args.query, args.limit.unwrap_or(50));
        ok(json!({ "materials": names }))
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
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip() {
        for id in [ObjectId::Solid(12), ObjectId::Entity(7)] {
            assert_eq!(parse_id(&id_string(id)), Ok(id));
        }
        assert_eq!(parse_id("entity 7"), Ok(ObjectId::Entity(7)));
        assert!(parse_id("7").is_err());
        assert!(parse_id("prop:7").is_err());
    }
}
