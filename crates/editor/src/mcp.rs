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
use crate::session::{
    Actor, ConnectionFields, ConnectionRecord, ObjectFilter, PropertyChange, PropertyTarget,
    SelectMode, Session,
};

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
frame_selection frames the selection or whole map. draw_block uses plane coordinates \
(top XY, front XZ, side YZ), shared snapping and depth; preview=true leaves the map \
and undo history untouched. Inspect get_map_info.block_preview and cancel with cancel_block_preview. \
transform_objects applies exact move/rotation/positive scale, with an optional world-space pivot \
(default: selection bounds center) and texture locking. Rotation is world X/Y/Z, in degrees. \
space: local uses the first selected object's frame; preview: true creates a shared transform draft. \
set_transform_tool configures move/rotate/scale handles, pivots and angle snap. Inspect \
get_transform_handles, begin_transform_drag, then update_transform_drag with total amounts \
or viewport pointers. Apply with commit_transform_preview or cancel_transform_preview. \
select_area selects objects or faces through a pixel rectangle in any projection; choose \
contained/intersecting and replace/add/remove/toggle. Use preview: true, inspect \
get_area_selection and render_view, then commit_area_selection or cancel_area_selection. \
select_faces switches to shared face selection; edit_faces changes material, UVs and fitting. \
Use preview: true for a draft, then commit_face_preview or cancel_face_preview. \
get_face_selection inspects selected faces and pending texture changes. browse_materials \
provides grouped catalog metadata; set_material_favorite shares browser favorites. \
clip_brushes cuts brushes by a normal/distance, three points, or a snapped planar line. \
Use preview: true, inspect get_clip_preview and render_view, then commit_clip_preview \
or cancel_clip_preview. Keep front (arrow side), back, or both; displacements cannot be clipped. \
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
The current compiler still produces fullbright maps without baked lighting. \
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
                "rotation": side.rotation,
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
        "projection": camera.projection.name(),
        "ortho_height": camera.ortho_height,
        "center": (camera.projection != Projection::Perspective).then(|| point((camera.position + camera.forward() * Camera::ORTHO_DISTANCE).as_dvec3())),
    })
}

#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct CameraSpec {
    /// "perspective", "top" (XY), "front" (XZ), or "side" (YZ).
    pub projection: Option<String>,
    /// Vertical span in Hammer units for a planar view, from 1 to 131072.
    pub ortho_height: Option<f32>,
    /// World-space center for a planar view; use position for perspective.
    pub center: Option<[f32; 3]>,
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
    fn resolve(&self, session: &Session, aspect: f32) -> Result<Camera, String> {
        if [self.fov, self.yaw, self.pitch, self.ortho_height]
            .into_iter()
            .flatten()
            .any(|value| !value.is_finite())
            || [self.position, self.center, self.look_at]
                .into_iter()
                .flatten()
                .any(|point| !Vec3::from_array(point).is_finite())
        {
            return Err("Camera coordinates and angles must be finite".into());
        }
        let mut camera = if let Some(projection) = &self.projection {
            session.camera_for_projection(
                Projection::parse(projection)
                    .ok_or("projection must be perspective, top, front or side")?,
            )
        } else {
            session.camera()
        };
        if let Some(height) = self.ortho_height {
            if !height.is_finite()
                || !(Camera::MIN_ORTHO_HEIGHT..=Camera::MAX_ORTHO_HEIGHT).contains(&height)
            {
                return Err("ortho_height must be between 1 and 131072 units".into());
            }
            camera.ortho_height = height;
        }
        if let Some(fov) = self.fov {
            camera.fov_y_degrees = fov.clamp(10.0, 150.0);
        }
        if let Some(focus) = &self.focus {
            let ids = parse_ids(focus)?;
            let (lo, hi) =
                query::bounds_of(session.document(), &ids).ok_or("focus objects have no bounds")?;
            return Ok(camera.frame_bounds(lo.as_vec3(), hi.as_vec3(), aspect));
        }
        if let Some(center) = self.center {
            if camera.projection == Projection::Perspective || self.position.is_some() {
                return Err(
                    "center requires a planar view and cannot be combined with position".into(),
                );
            }
            camera = Camera::planar(
                camera.projection,
                Vec3::from_array(center),
                camera.ortho_height,
            );
        }
        if let Some(p) = self.position {
            camera.position = Vec3::from_array(p);
        }
        if camera.projection != Projection::Perspective {
            if self.look_at.is_some() || self.yaw.is_some() || self.pitch.is_some() {
                return Err(
                    "Planar views have fixed axes; use center, position and ortho_height".into(),
                );
            }
            return Ok(camera);
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
    /// Free text across IDs, class, name and materials.
    pub query: Option<String>,
    /// Filter stored VMF visibility.
    pub hidden: Option<bool>,
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
pub struct PropertyTargetArgs {
    /// World settings when true. Otherwise use entity IDs or the selection.
    #[serde(default)]
    pub world: bool,
    pub ids: Option<Vec<String>>,
    /// Optional guards from get_property_inspector or get_object_report.
    pub generation: Option<u64>,
    pub revision: Option<u64>,
}

impl PropertyTargetArgs {
    fn target(self, session: &Session) -> Result<PropertyTarget, String> {
        if self
            .generation
            .is_some_and(|value| value != session.document_generation())
            || self
                .revision
                .is_some_and(|value| value != session.document().revision())
        {
            return Err("Map changed; inspect properties again".into());
        }
        if self.world {
            if self.ids.is_some() {
                return Err("Use world or entity IDs, not both".into());
            }
            return Ok(PropertyTarget::World);
        }
        let ids = ids_or_selection(session, self.ids.as_deref())?
            .into_iter()
            .map(|id| match id {
                ObjectId::Entity(id) => Ok(id),
                _ => {
                    Err("Property batches require entities; use world=true for map settings".into())
                }
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(PropertyTarget::Entities(ids))
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PropertyEditArgs {
    pub token: u64,
    /// Replaces the pending change list. Nothing is written until commit.
    pub changes: Vec<PropertyChange>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PropertyTokenArgs {
    pub token: u64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct VisibilityArgs {
    /// hide, show, isolate, show_all or restore. Hidden objects are NOT compiled.
    pub action: String,
    /// Defaults to selection for hide/show/isolate. Omit for show_all/restore.
    pub ids: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReportArgs {
    #[serde(default)]
    pub filter: ObjectFilter,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SelectReportArgs {
    #[serde(default)]
    pub filter: ObjectFilter,
    pub mode: Option<String>,
    #[serde(default)]
    pub frame: bool,
    pub aspect: Option<f32>,
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
    /// Snap corners using the shared grid settings; omitted preserves exact coordinates.
    #[serde(default)]
    pub snap: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DrawBlockArgs {
    /// First footprint corner in plane coordinates: XY for top, XZ for front, YZ for side.
    pub start: [f64; 2],
    pub end: [f64; 2],
    /// "top", "front", or "side"; omitted uses the current viewport projection.
    pub projection: Option<String>,
    pub elevation: Option<f64>,
    pub depth: Option<f64>,
    pub material: Option<String>,
    /// Show a non-mutating brush preview instead of creating it.
    #[serde(default)]
    pub preview: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FrameArgs {
    /// Viewport width divided by height; defaults to 1 for conservative framing.
    pub aspect: Option<f32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateEntityArgs {
    pub classname: String,
    /// Position [x, y, z]. Overrides properties.origin; otherwise uses the current workplane or a point ahead of the camera, with the shared snapping settings.
    pub origin: Option<[f64; 3]>,
    /// Extra key/values, e.g. {"targetname": "door1", "angles": "0 90 0"}.
    pub properties: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct EntityClassArgs {
    pub classname: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchEntityClassesArgs {
    /// Case-insensitive substring of the classname or description.
    pub query: Option<String>,
    /// "point" or "solid"; omitted searches both.
    pub kind: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DefinitionsArgs {
    /// Root FGD filename; includes resolve relative to each containing FGD.
    pub path: String,
}

fn class_json(class: &Class) -> Value {
    let properties: Vec<Value> = class.properties.iter().map(|property| json!({
        "name": property.name, "type": property.kind, "label": property.label,
        "description": property.description, "default": property.default_value(),
        "read_only": property.read_only,
        "choices": property.choices.iter().map(|choice| json!({"value": choice.value, "label": choice.label, "default_selected": choice.selected})).collect::<Vec<_>>()
    })).collect();
    let io = |members: &[source_formats::fgd::Io]| {
        members.iter().map(|member| json!({"name": member.name, "type": member.kind, "description": member.description})).collect::<Vec<_>>()
    };
    json!({"classname": class.name, "kind": class.kind.label(), "description": class.description,
        "bases": class.bases, "defaults": class.defaults(), "properties": properties,
        "inputs": io(&class.inputs), "outputs": io(&class.outputs),
        "helpers": class.helpers.iter().map(|helper| json!({"name": helper.name, "arguments": helper.arguments})).collect::<Vec<_>>()})
}

fn definitions_json(session: &Session) -> Value {
    json!({"classes": session.definitions().classes().count(),
        "sources": session.definitions().sources.iter().map(|path| path.display().to_string()).collect::<Vec<_>>(),
        "error": session.definitions_error()})
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
pub struct TransformArgs {
    /// Object IDs; omitted uses the shared selection. Repeated IDs are applied once.
    pub ids: Option<Vec<String>>,
    /// Exact world offset [dx, dy, dz], applied last; defaults to zero.
    pub translation: Option<[f64; 3]>,
    /// Right-handed degrees around world [X, Y, Z], applied X then Y then Z; defaults to zero.
    pub rotation: Option<[f64; 3]>,
    /// Positive world-axis factors [sx, sy, sz], applied before rotation; defaults to [1,1,1].
    pub scale: Option<[f64; 3]>,
    /// World pivot [x,y,z]; omitted uses the combined selection bounds center.
    pub pivot: Option<[f64; 3]>,
    /// Preserve UV coordinates at corresponding transformed points, including scaling; default true.
    pub texture_lock: Option<bool>,
    /// world (default) or local; local follows the first selected object's frame.
    pub space: Option<String>,
    /// Draft geometry without changing the document or history; commit or cancel separately.
    #[serde(default)]
    pub preview: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TransformToolArgs {
    /// select, move, rotate or scale; omitted preserves the current tool.
    pub mode: Option<String>,
    /// world or local.
    pub space: Option<String>,
    /// selection, world or custom.
    pub pivot_mode: Option<String>,
    pub pivot: Option<[f64; 3]>,
    pub texture_lock: Option<bool>,
    /// Rotation snapping in degrees, 0 disables it. Shared Snap also controls snapping.
    pub angle_step: Option<f64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PointerArgs {
    pub position: [f64; 2],
    pub width: u32,
    pub height: u32,
}

impl PointerArgs {
    fn resolve(self) -> Result<crate::session::Pointer, String> {
        crate::session::Pointer {
            position: glam::DVec2::from_array(self.position),
            size: (self.width, self.height),
        }
        .validate()
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct BeginTransformArgs {
    /// x/y/z, xy/xz/yz or view for movement, x/y/z for rotation, x/y/z or uniform for scaling.
    pub handle: String,
    /// Optional initial viewport point; omit to drive the handle with numeric amounts.
    pub pointer: Option<PointerArgs>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateTransformArgs {
    pub pointer: Option<PointerArgs>,
    /// Total offset from the start, in units for move, degrees for rotation or positive factor for scale.
    pub amount: Option<f64>,
    /// Total offsets along the two plane axes, or screen right/up for the view handle.
    pub plane_offset: Option<[f64; 2]>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct HandlesArgs {
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub camera: Option<CameraSpec>,
    /// Optional viewport point to hit-test against the rendered handles.
    pub point: Option<[f64; 2]>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AreaOptionsArgs {
    /// intersecting or contained.
    pub rule: Option<String>,
    /// replace, add, remove or toggle.
    pub mode: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AreaArgs {
    /// Viewport pixels, with origin at the top left; reverse drags are supported.
    pub start: [f64; 2],
    pub end: [f64; 2],
    pub width: u32,
    pub height: u32,
    pub camera: Option<CameraSpec>,
    pub rule: Option<String>,
    pub mode: Option<String>,
    /// Preview candidate highlights and rectangle without changing selection or history.
    #[serde(default)]
    pub preview: bool,
}

fn parse_select_mode(value: &str) -> Result<SelectMode, String> {
    match value {
        "replace" => Ok(SelectMode::Replace),
        "add" => Ok(SelectMode::Add),
        "remove" => Ok(SelectMode::Remove),
        "toggle" => Ok(SelectMode::Toggle),
        _ => Err("mode must be replace, add, remove or toggle".into()),
    }
}

fn select_mode_name(mode: SelectMode) -> &'static str {
    match mode {
        SelectMode::Replace => "replace",
        SelectMode::Add => "add",
        SelectMode::Remove => "remove",
        SelectMode::Toggle => "toggle",
    }
}

fn area_rule(value: &str) -> Result<map_core::region::Match, String> {
    match value {
        "contained" => Ok(map_core::region::Match::Contained),
        "intersecting" => Ok(map_core::region::Match::Intersecting),
        _ => Err("rule must be contained or intersecting".into()),
    }
}

fn area_rule_name(rule: map_core::region::Match) -> &'static str {
    match rule {
        map_core::region::Match::Contained => "contained",
        map_core::region::Match::Intersecting => "intersecting",
    }
}

fn transform_mode_name(mode: Option<render::handles::Mode>) -> &'static str {
    match mode {
        None => "select",
        Some(render::handles::Mode::Move) => "move",
        Some(render::handles::Mode::Rotate) => "rotate",
        Some(render::handles::Mode::Scale) => "scale",
    }
}

fn transform_state(session: &Session) -> Value {
    let tool = session.transform_tool();
    json!({"settings": {"mode": transform_mode_name(tool.mode), "space": if tool.local {"local"} else {"world"}, "pivot": tool.pivot.map(|pivot| pivot.to_array()), "texture_lock": tool.texture_lock, "angle_step": tool.angle_step},
        "drag": session.transform_drag().map(|drag| json!({"handle": drag.handle.name(), "mode": transform_mode_name(Some(drag.mode)), "amount": drag.amount.to_array(), "pivot": drag.pivot.to_array(), "basis": drag.basis.to_cols_array_2d(), "camera": camera_json(&drag.camera)})),
        "preview": session.transform_preview().map(|preview| json!({"ids": preview.ids.iter().copied().map(id_string).collect::<Vec<_>>(), "translation": preview.transform.translation.to_array(), "rotation": preview.transform.rotation.to_array(), "scale": preview.transform.scale.to_array(), "basis": preview.transform.basis.to_cols_array_2d(), "pivot": preview.transform.pivot.to_array(), "texture_lock": preview.transform.texture_lock, "bounds": bounds_json(query::bounds_of(&preview.document, &preview.ids)), "objects": preview.ids.iter().filter_map(|id| detail(&preview.document, *id).ok()).collect::<Vec<_>>()}))})
}

fn area_matches(matches: &map_core::region::Selection) -> Value {
    json!({"objects": matches.objects.iter().copied().map(id_string).collect::<Vec<_>>(), "faces": matches.faces.iter().map(|face| json!({"solid": face.solid, "side": face.side})).collect::<Vec<_>>()})
}

fn area_state(session: &Session) -> Value {
    json!({"rule": area_rule_name(session.area_rule()), "mode": select_mode_name(session.area_mode()), "preview": session.area_preview().map(|preview| json!({"start": preview.start.position.to_array(), "end": preview.end.to_array(), "width": preview.start.size.0, "height": preview.start.size.1, "camera": camera_json(&preview.camera), "rule": area_rule_name(preview.rule), "mode": select_mode_name(preview.mode), "selection_mode": if preview.faces {"faces"} else {"objects"}, "matches": area_matches(&preview.matches)}))})
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
pub struct SelectionModeArgs {
    pub mode: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SelectFacesArgs {
    pub faces: Vec<FaceRef>,
    pub mode: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct EditFacesArgs {
    pub faces: Option<Vec<FaceRef>>,
    pub material: Option<String>,
    pub shift_u: Option<f64>,
    pub shift_v: Option<f64>,
    pub scale_u: Option<f64>,
    pub scale_v: Option<f64>,
    pub rotation: Option<f64>,
    /// world or face. Resets the projection basis and rotation before optional rotation is applied.
    pub alignment: Option<String>,
    /// left, center or right within one material tile; changes offset only, after fit.
    pub justify_u: Option<String>,
    /// top, center or bottom within one material tile; changes offset only, after fit.
    pub justify_v: Option<String>,
    pub fit: Option<bool>,
    pub preview: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SampleFaceArgs {
    /// Omit to sample the single selected face. Captures committed values, not a preview.
    pub face: Option<FaceRef>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RecoveryListArgs {
    pub offset: Option<usize>,
    pub limit: Option<usize>,
    #[serde(default)]
    pub current_map_only: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RecoverySettingsArgs {
    pub enabled: Option<bool>,
    /// 30 to 3600 seconds; autosaves only when committed map content changed.
    pub interval_seconds: Option<u64>,
    /// 1 to 50 snapshots per map, including previous-file backups.
    pub keep_per_map: Option<usize>,
    /// Optional stale-settings guard from get_recovery.status.settings_revision.
    pub expected_revision: Option<u64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RecoveryIdArgs {
    pub id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RestoreRecoveryArgs {
    pub id: String,
    /// Explicitly discard unsaved edits in the current map.
    #[serde(default)]
    pub discard_changes: bool,
    /// Optional stale-map guard; supply both generation and revision from get_recovery.status.
    pub generation: Option<u64>,
    pub revision: Option<u64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PaintFacesArgs {
    pub faces: Option<Vec<FaceRef>>,
    /// material (default), projection (copy world-space UVs), or wrap (rotate UVs across a shared edge).
    pub mode: Option<String>,
    /// Use commit_face_preview or cancel_face_preview to finish the shared draft.
    #[serde(default)]
    pub preview: bool,
}

fn texture_justification(
    value: Option<&str>,
    vertical: bool,
) -> Result<Option<(map_core::surface::Justify, u32)>, String> {
    use map_core::surface::Justify;
    let placement = match value {
        None => return Ok(None),
        Some("center") => Justify::Center,
        Some("left") if !vertical => Justify::Start,
        Some("right") if !vertical => Justify::End,
        Some("top") if vertical => Justify::Start,
        Some("bottom") if vertical => Justify::End,
        _ => {
            return Err(if vertical {
                "justify_v must be top, center or bottom"
            } else {
                "justify_u must be left, center or right"
            }
            .into());
        }
    };
    Ok(Some((placement, 0)))
}

fn surface_sample_state(session: &Session) -> Value {
    session.surface_sample().map_or(Value::Null, |sample| json!({
        "face": {"solid": sample.face.solid, "side": sample.face.side},
        "material": sample.material, "uaxis": sample.projection.u.to_string(),
        "vaxis": sample.projection.v.to_string(), "rotation": sample.projection.rotation,
        "normal": sample.normal.to_array(), "vertices": sample.vertices.iter().map(|v| v.to_array()).collect::<Vec<_>>(),
        "displacement": sample.displaced,
        "source_unchanged": map_core::surface::sample(session.document(), sample.face).ok().as_ref() == Some(sample),
    }))
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct BrowseMaterialsArgs {
    pub query: Option<String>,
    pub group: Option<String>,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct FavoriteMaterialArgs {
    pub material: String,
    pub favorite: bool,
}

fn face_id(face: &FaceRef) -> map_core::surface::FaceId {
    map_core::surface::FaceId {
        solid: face.solid,
        side: face.side,
    }
}

fn face_state(session: &Session) -> Value {
    json!({"mode": if session.face_mode() { "faces" } else { "objects" }, "preview": session.has_face_preview(),
        "sample": surface_sample_state(session),
        "preview_faces": session.preview_faces().into_iter().filter_map(|face| {
            let side = session.preview_side(face)?;
            Some(json!({"solid": face.solid, "side": face.side, "material": side.material,
                "shift_u": side.u_axis.offset, "shift_v": side.v_axis.offset,
                "scale_u": side.u_axis.scale, "scale_v": side.v_axis.scale, "rotation": side.rotation,
                "uaxis": side.u_axis.to_string(), "vaxis": side.v_axis.to_string()}))
        }).collect::<Vec<_>>(),
        "faces": session.selected_faces().into_iter().filter_map(|face| {
            let side = map_core::surface::side(session.document(), face)?;
            Some(json!({"solid": face.solid, "side": face.side, "owner": session.document().owner_of_solid(face.solid).map(id_string),
                "material": side.material, "shift_u": side.u_axis.offset, "shift_v": side.v_axis.offset,
                "scale_u": side.u_axis.scale, "scale_v": side.v_axis.scale, "rotation": side.rotation,
                "uaxis": side.u_axis.to_string(), "vaxis": side.v_axis.to_string(),
                "preview": session.preview_side(face).map(|side| json!({"material": side.material, "shift_u": side.u_axis.offset, "shift_v": side.v_axis.offset, "scale_u": side.u_axis.scale, "scale_v": side.v_axis.scale, "rotation": side.rotation}))}))
        }).collect::<Vec<_>>()})
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
pub struct ClipLine {
    pub projection: String,
    pub start: [f64; 2],
    pub end: [f64; 2],
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ClipPlane {
    pub normal: Option<[f64; 3]>,
    pub distance: Option<f64>,
    /// Three points in VMF winding: (p0-p1) cross (p2-p1) points toward front.
    pub points: Option<[[f64; 3]; 3]>,
    /// A planar cut gesture using the shared grid, snapping and workplane.
    pub line: Option<ClipLine>,
}

impl ClipPlane {
    fn resolve(self, session: &Session) -> Result<Plane, String> {
        match (self.normal, self.distance, self.points, self.line) {
            (Some(normal), Some(dist), None, None) => map_core::clip::normalized_plane(Plane {
                normal: vec3(normal),
                dist,
            })
            .map_err(|error| error.to_string()),
            (None, None, Some(points), None) => {
                let points = points.map(vec3);
                if points.iter().any(|point| !point.is_finite()) {
                    return Err("Cut points must be finite".into());
                }
                let plane = Plane::from_points(points).ok_or("Cut points must not be collinear")?;
                map_core::clip::normalized_plane(plane).map_err(|error| error.to_string())
            }
            (None, None, None, Some(line)) => session.clip_line_plane(
                Projection::parse(&line.projection)
                    .ok_or("projection must be top, front or side")?,
                glam::DVec2::from_array(line.start),
                glam::DVec2::from_array(line.end),
            ),
            _ => Err(
                "Provide exactly one cut plane: normal + distance, three points, or a planar line"
                    .into(),
            ),
        }
    }
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ClipArgs {
    pub ids: Option<Vec<String>>,
    pub plane: ClipPlane,
    /// front is the arrow/positive-normal side, back the opposite; defaults to both.
    pub keep: Option<String>,
    /// New cap material; omitted inherits each source brush's first material.
    pub material: Option<String>,
    #[serde(default)]
    pub preview: bool,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ClipToolArgs {
    pub enabled: bool,
}

fn clip_result(result: &map_core::clip::ClipResult) -> Value {
    json!({"cuts": result.cuts.iter().map(|cut| json!({"source": cut.source, "solids": cut.solids})).collect::<Vec<_>>(), "objects": result.objects.iter().copied().map(id_string).collect::<Vec<_>>(), "unchanged": result.unchanged})
}

fn clip_state(session: &Session) -> Value {
    json!({"enabled": session.clip_tool(), "preview": session.clip_preview().map(|preview| json!({
        "plane": {"normal": preview.plane.normal.to_array(), "distance": preview.plane.dist},
        "keep": match preview.keep { map_core::clip::Keep::Front => "front", map_core::clip::Keep::Back => "back", map_core::clip::Keep::Both => "both" },
        "material": preview.material, "ids": preview.ids.iter().copied().map(id_string).collect::<Vec<_>>(),
        "result": clip_result(&preview.result),
        "pieces": preview.result.cuts.iter().flat_map(|cut| &cut.solids)
            .filter_map(|id| map_core::surface::solid(&preview.document, *id))
            .map(solid_detail).collect::<Vec<_>>()
    }))})
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
pub struct ConnectionArgs {
    pub id: Option<String>,
    /// Omit to add a connection; otherwise use the zero-based outgoing index.
    pub index: Option<usize>,
    /// Optional stale-map guard; supply both values from get_entity_connections.
    pub generation: Option<u64>,
    pub revision: Option<u64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ConnectionChoicesArgs {
    pub id: Option<String>,
    /// Target name, prefix wildcard or !self, to discover matching inputs.
    pub target: Option<String>,
    pub query: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RenameArgs {
    /// Entity ID; defaults to the single selected entity.
    pub id: Option<String>,
    /// Optional stale-map guard from get_entity_references; supply both values.
    pub generation: Option<u64>,
    pub revision: Option<u64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RenameFieldsArgs {
    /// Identity guard from begin_entity_rename.
    pub token: Option<u64>,
    pub new_name: Option<String>,
    /// Explicitly rename all entities sharing the old name, including hidden ones.
    pub rename_group: Option<bool>,
    /// Acknowledge references left unchanged. Set after inspecting the preview;
    /// changing the name or group resets this acknowledgment.
    pub accept_review: Option<bool>,
    /// Show raw engine keys, class names and IDs in the shared human preview.
    pub show_details: Option<bool>,
}

fn reference_json(reference: &map_core::references::Reference) -> Value {
    use map_core::references::Location;
    let location = match &reference.location {
        Location::Property { index, key } => json!({"kind":"property", "index":index, "key":key}),
        Location::Connection {
            index,
            output,
            field,
        } => {
            json!({"kind":"connection", "index":index, "output":output, "field":if *field == 0 {"target"} else {"parameter"}})
        }
    };
    json!({"source":id_string(ObjectId::Entity(reference.source)), "location":location,
        "value":reference.value, "target":reference.target, "declared":reference.declared, "certain":reference.certain,
        "reason":reference.reason, "hidden":reference.hidden})
}

fn rename_plan_json(session: &Session, plan: &map_core::references::RenamePlan) -> Value {
    json!({"entity":id_string(ObjectId::Entity(plan.entity)), "old_name":plan.old_name, "new_name":plan.new_name,
        "entities":plan.entities.iter().map(|id| id_string(ObjectId::Entity(*id))).collect::<Vec<_>>(),
        "references":plan.references.iter().map(|change| json!({"reference":reference_json(&change.reference),
            "after":change.after, "changed":change.changed,
            "review":change.review, "reason":change.reason,
            "display":session.rename_reference_view(change, &plan.new_name)})).collect::<Vec<_>>(), "error":plan.error})
}

fn rename_draft_json(session: &Session) -> Value {
    let Some(draft) = session.rename_draft() else {
        return Value::Null;
    };
    let (preview, error, review, can_apply) = match session.rename_preview() {
        Ok(plan) => (
            rename_plan_json(session, &plan),
            plan.error.clone(),
            plan.references.iter().any(|change| change.review),
            session.rename_can_apply(&plan),
        ),
        Err(error) => (Value::Null, Some(error), false, false),
    };
    json!({"entity":id_string(ObjectId::Entity(draft.entity)), "token":draft.token,
        "new_name":draft.new_name, "rename_group":draft.rename_group, "accept_review":draft.accept_review,
        "show_details":draft.show_details,
        "generation":draft.generation, "revision":draft.revision,
        "preview":preview, "error":error, "requires_review":review,
        "can_apply":can_apply})
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ConnectionFieldsArgs {
    /// Optional identity guard from begin_connection_edit; rejects a replaced draft.
    pub token: Option<u64>,
    pub output: Option<String>,
    pub target: Option<String>,
    pub input: Option<String>,
    /// Empty forwards the output value. Commas are unavailable in new CS:S connections.
    pub parameter: Option<String>,
    /// Seconds; strings allow unfinished numeric drafts shared with the UI.
    pub delay: Option<String>,
    /// -1 for unlimited, 1 for once, or a positive firing count.
    pub times: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ConnectionDraftArgs {
    /// Optional identity guard from the shared connection or rename draft.
    pub token: Option<u64>,
}

fn connection_json(record: ConnectionRecord) -> Value {
    json!({"source": id_string(ObjectId::Entity(record.source)), "index": record.index,
        "output": record.raw.output, "raw": record.raw.value,
        "fields": record.spec.map(|spec| json!({"output":spec.output,"target":spec.target,"input":spec.input,"parameter":spec.parameter,"delay":spec.delay,"times":spec.times})),
        "targets": record.targets.into_iter().map(|id| id_string(ObjectId::Entity(id))).collect::<Vec<_>>(), "warnings":record.warnings})
}

fn connection_draft_json(session: &Session) -> Value {
    let (error, warnings) = session.connection_draft_feedback();
    session.connection_draft().map_or(Value::Null, |draft| {
        json!({
            "source": id_string(ObjectId::Entity(draft.source)), "index":draft.index,"token":draft.token,
            "fields":draft.fields,"generation":draft.generation,"revision":draft.revision,
            "error":error,"warnings":warnings
        })
    })
}

fn connection_guard(session: &Session, args: &ConnectionArgs) -> Result<(), String> {
    match (args.generation, args.revision) {
        (Some(generation), Some(revision)) => {
            session.check_connection_snapshot(generation, revision)
        }
        (None, None) => Ok(()),
        _ => Err("Supply both generation and revision for the stale-map guard".into()),
    }
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
pub struct InspectorArgs {
    /// Entity id; defaults to the single selected entity.
    pub id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct EntityColorArgs {
    /// Entity id; defaults to the single selected entity.
    pub id: Option<String>,
    /// FGD color key, e.g. _light, _ambient or rendercolor.
    pub key: String,
    /// Red, green and blue channels in the range 0..255.
    pub color: [u8; 3],
    /// Nonnegative brightness; omitted preserves the existing component.
    pub intensity: Option<f64>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct MaterialPreviewArgs {
    /// Material path without materials/ or .vmt, e.g. concrete/concretefloor001a.
    pub material: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct EntityComponentArgs {
    pub id: Option<String>,
    /// Vector or color property key.
    pub key: String,
    /// Zero-based component; 0..2 for position/rotation/RGB, 3 for brightness.
    pub index: usize,
    /// Numeric component value, preserving the other components exactly.
    pub value: String,
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
        description = "Inspect autosave settings, pending changes, errors, storage location and retained recovery snapshots (newest first). Snapshots contain committed map data, not editor previews. Default page 50, maximum 200; current_map_only filters by the current document key.",
        annotations(read_only_hint = true)
    )]
    fn get_recovery(&self, Parameters(args): Parameters<RecoveryListArgs>) -> ToolResult {
        run(|| {
            let session = self.session();
            let status = session.recovery_status();
            let listing =
                session
                    .recovery_entries()
                    .unwrap_or_else(|error| crate::recovery::Listing {
                        entries: Vec::new(),
                        warnings: vec![error],
                    });
            let entries: Vec<_> = listing
                .entries
                .into_iter()
                .filter(|entry| {
                    !args.current_map_only
                        || Some(entry.document_key.as_str()) == status["document_key"].as_str()
                })
                .collect();
            let offset = args.offset.unwrap_or(0);
            Ok(
                json!({"status": status, "total": entries.len(), "offset": offset, "snapshots": entries.into_iter().skip(offset).take(args.limit.unwrap_or(50).min(200)).collect::<Vec<_>>(), "warnings": listing.warnings}),
            )
        })
    }

    #[tool(
        description = "Configure persistent autosave settings through the same operation as Recovery settings. enabled controls periodic/exit autosaves; manual snapshots and previous-file backups remain available. Interval is 30–3600 seconds, retention 1–50 snapshots per map. Omitted values keep the current setting. Changes do not edit the map."
    )]
    fn set_recovery_settings(
        &self,
        Parameters(args): Parameters<RecoverySettingsArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let mut settings = session.recovery_settings().clone();
            if let Some(value) = args.enabled {
                settings.enabled = value;
            }
            if let Some(value) = args.interval_seconds {
                settings.interval_seconds = value;
            }
            if let Some(value) = args.keep_per_map {
                settings.keep_per_map = value;
            }
            session.set_recovery_settings(Actor::Agent, settings, args.expected_revision)?;
            Ok(session.recovery_status())
        })
    }

    #[tool(
        description = "Write a manual recovery snapshot without saving over the map, bumping mapversion, changing dirty state or adding undo history. Uses committed content and excludes active previews. Does not hold the Session lock during serialization/file writing; edits made while it writes remain pending for the next snapshot."
    )]
    async fn create_recovery_snapshot(&self) -> ToolResult {
        let session = self.session.clone();
        match tokio::task::spawn_blocking(move || {
            crate::recovery::capture(&session, Actor::Agent, crate::recovery::Kind::Manual)
        })
        .await
        {
            Ok(Ok(entry)) => ok(json!({"snapshot": entry})),
            Ok(Err(error)) => fail(error),
            Err(error) => fail(format!("Recovery worker failed: {error}")),
        }
    }

    #[tool(
        description = "Open a retained recovery snapshot as a dirty, unnamed copy. It leaves the source file untouched and clears selection/history/previews like opening a map. Save with an explicit path to keep the recovered copy. Refuses to replace unsaved edits unless discard_changes is explicitly true. Invalid snapshots leave the current map unchanged."
    )]
    fn restore_recovery_snapshot(
        &self,
        Parameters(args): Parameters<RestoreRecoveryArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let expected = match (args.generation, args.revision) {
                (None, None) => None,
                (Some(generation), Some(revision)) => Some((generation, revision)),
                _ => {
                    return Err(
                        "Supply both generation and revision for the stale-map guard".into(),
                    );
                }
            };
            session.restore_recovery_snapshot(
                Actor::Agent,
                &args.id,
                args.discard_changes,
                expected,
            )?;
            Ok(
                json!({"restored": args.id, "path": session.path(), "unsaved_changes": session.document().is_dirty(), "status": session.recovery_status()}),
            )
        })
    }

    #[tool(
        description = "Permanently delete one recovery snapshot by its listed ID. Does not delete or edit the original map. Refuses during a snapshot write. Retention also prunes the oldest snapshots automatically when new ones are created."
    )]
    fn delete_recovery_snapshot(&self, Parameters(args): Parameters<RecoveryIdArgs>) -> ToolResult {
        run(|| {
            self.session()
                .delete_recovery_snapshot(Actor::Agent, &args.id)?;
            Ok(json!({"deleted": args.id}))
        })
    }

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
            "entity_definitions": definitions_json(&session),
            "compiling": session.compiling(),
            "launching": session.launching(),
            "game_running": session.game_running(),
            "leak": session.leak().map(|leak| json!({"entity": format!("entity:{}", leak.entity), "points": leak.points.iter().copied().map(point).collect::<Vec<_>>()})),
            "view": {"tool_textures": session.view().tool_textures, "leak_path": session.view().leak_path},
            "spatial": session.spatial(),
            "block_preview": bounds_json(session.block_preview()),
            "clipping": clip_state(&session),
            "transform": transform_state(&session),
            "area_selection": area_state(&session),
            "connection_draft": connection_draft_json(&session),
            "rename_draft": rename_draft_json(&session),
            "property_draft": session.property_draft(),
            "visibility": session.visibility_state(),
            "recovery": session.recovery_status(),
            "face_selection": face_state(&session),
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
        run(|| {
            let session = self.session();
            let found = session.object_report(&ObjectFilter {
                query: args.query,
                hidden: args.hidden,
                classname: args.classname,
                targetname: args.targetname,
                material: args.material,
                kind: args.kind,
                within: args.within,
            })?;
            let offset = args.offset.unwrap_or(0);
            let objects = found
                .iter()
                .skip(offset)
                .take(args.limit.unwrap_or(50).min(500))
                .map(|row| {
                    let mut value = summary(session.document(), row.object);
                    value["hidden"] = json!(row.hidden);
                    value["hidden_brushes"] = json!(row.hidden_brushes);
                    value
                })
                .collect::<Vec<_>>();
            Ok(json!({"total":found.len(), "offset":offset, "objects":objects}))
        })
    }

    #[tool(
        description = "Searchable object report with IDs, type, class, name, materials, visibility and bounds. Same filters as the human Object report. Default page 100, maximum 500.",
        annotations(read_only_hint = true)
    )]
    fn get_object_report(&self, Parameters(args): Parameters<ReportArgs>) -> ToolResult {
        run(|| {
            let session = self.session();
            let rows = session.object_report(&args.filter)?;
            let offset = args.offset.unwrap_or(0);
            Ok(
                json!({"total":rows.len(), "offset":offset, "objects":rows.iter().skip(offset).take(args.limit.unwrap_or(100).min(500)).collect::<Vec<_>>(), "visibility":session.visibility_state(), "generation":session.document_generation(), "revision":session.document().revision()}),
            )
        })
    }

    #[tool(
        description = "Select ALL matching report objects, including results beyond a page. Optionally frame the resulting selection. Uses shared report filters and replace/add/remove/toggle selection."
    )]
    fn select_object_report(&self, Parameters(args): Parameters<SelectReportArgs>) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let count = session.select_report(
                Actor::Agent,
                &args.filter,
                parse_select_mode(args.mode.as_deref().unwrap_or("replace"))?,
                args.frame,
                args.aspect.unwrap_or(1.6),
            )?;
            Ok(
                json!({"matched":count, "selection":session.selected_ids().into_iter().map(id_string).collect::<Vec<_>>()}),
            )
        })
    }

    #[tool(
        description = "Hide, show, isolate, show all or restore previous visibility, in one undo step. Saved VMF hidden state includes nested brushes. Hidden objects are omitted by the compiler. Restore keeps newly created objects and ignores deleted ones."
    )]
    fn set_object_visibility(&self, Parameters(args): Parameters<VisibilityArgs>) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let ids = if ["show_all", "restore"].contains(&args.action.as_str()) {
                args.ids
                    .as_deref()
                    .map(parse_ids)
                    .transpose()?
                    .unwrap_or_default()
            } else {
                ids_or_selection(&session, args.ids.as_deref())?
            };
            session.set_visibility(Actor::Agent, &args.action, &ids)?;
            Ok(session.visibility_state())
        })
    }

    #[tool(
        description = "Inspect world settings or common entity properties with typed controls, mixed values, individual stored values, FGD defaults and mixed flags. Absent values are reported separately from explicit overrides.",
        annotations(read_only_hint = true)
    )]
    fn get_property_inspector(
        &self,
        Parameters(args): Parameters<PropertyTargetArgs>,
    ) -> ToolResult {
        run(|| {
            let session = self.session();
            let report = session.inspect_properties(args.target(&session)?)?;
            serde_json::to_value(report).map_err(|error| error.to_string())
        })
    }

    #[tool(
        description = "Begin a shared cancellable property draft for world settings or selected entities. Returns a token required by update/commit/cancel. Replaces any previous property draft."
    )]
    fn begin_property_edit(&self, Parameters(args): Parameters<PropertyTargetArgs>) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let target = args.target(&session)?;
            session.begin_property_edit(target)?;
            Ok(
                json!({"draft":session.property_draft(), "preview":session.preview_property_edit()?}),
            )
        })
    }

    #[tool(
        description = "Replace pending property changes and validate every target before accepting. set writes a value; remove deletes the override; default explicitly writes the FGD default; flag changes one declared bit preserving others; component edits one vector/color component. Names use the dedicated safe rename workflow. No map mutation until commit."
    )]
    fn update_property_edit(&self, Parameters(args): Parameters<PropertyEditArgs>) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.update_property_edit(args.token, args.changes)?;
            Ok(
                json!({"draft":session.property_draft(), "preview":session.preview_property_edit()?}),
            )
        })
    }

    #[tool(
        description = "Read the shared property draft and preview; a stale map/FGD draft returns an error.",
        annotations(read_only_hint = true)
    )]
    fn get_property_edit(&self) -> ToolResult {
        run(|| {
            let session = self.session();
            Ok(
                json!({"draft":session.property_draft(), "preview":session.preview_property_edit()?}),
            )
        })
    }

    #[tool(
        description = "Apply the property draft as one undoable edit. Rejects stale tokens, map revisions, map replacement and changed entity definitions."
    )]
    fn commit_property_edit(&self, Parameters(args): Parameters<PropertyTokenArgs>) -> ToolResult {
        run(|| {
            self.session()
                .commit_property_edit(Actor::Agent, args.token)?;
            Ok(json!({"applied":true}))
        })
    }

    #[tool(
        description = "Discard the shared property draft without changing the map or undo history. A replaced draft cannot be cancelled by an old token."
    )]
    fn cancel_property_edit(&self, Parameters(args): Parameters<PropertyTokenArgs>) -> ToolResult {
        run(|| {
            self.session().cancel_property_edit(args.token)?;
            Ok(json!({"cancelled":true}))
        })
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
        description = "Inspect an entity using the same human-readable labels, common/advanced fields, typed controls, colors, brightness and choices as the property panel.",
        annotations(read_only_hint = true)
    )]
    fn get_entity_inspector(&self, Parameters(args): Parameters<InspectorArgs>) -> ToolResult {
        run(|| {
            let session = self.session();
            let id = single_entity(&session, args.id.as_deref())?;
            let inspector = session.entity_inspector(id)?;
            serde_json::to_value(inspector).map_err(|error| error.to_string())
        })
    }

    #[tool(
        description = "Set an entity color and optional brightness using the same color picker operation as the GUI. Supports color255 and normalized color1; preserves brightness when omitted."
    )]
    fn set_entity_color(&self, Parameters(args): Parameters<EntityColorArgs>) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let id = single_entity(&session, args.id.as_deref())?;
            session.set_entity_color(Actor::Agent, id, &args.key, args.color, args.intensity)?;
            detail(session.document(), ObjectId::Entity(id))
        })
    }

    #[tool(
        description = "Edit one position, rotation, color or brightness component while preserving the others. Uses the same operation as the GUI component inputs."
    )]
    fn set_entity_component(
        &self,
        Parameters(args): Parameters<EntityComponentArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let id = single_entity(&session, args.id.as_deref())?;
            session.set_entity_component(Actor::Agent, id, &args.key, args.index, &args.value)?;
            detail(session.document(), ObjectId::Entity(id))
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
            "face_selection": face_state(&session),
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
            let id = session.create_block(Actor::Agent, mins, maxs, material, args.snap)?;
            Ok(summary(session.document(), id))
        })
    }

    #[tool(
        description = "Inspect or change shared grid, snapping, workplane elevation, brush depth and the planar drawing tool. Changes are validated atomically; omitted values are preserved."
    )]
    fn set_spatial_settings(
        &self,
        Parameters(args): Parameters<crate::spatial::Update>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.set_spatial(Actor::Agent, args)?;
            Ok(json!(session.spatial()))
        })
    }

    #[tool(
        description = "Draw a brush footprint in top/front/side plane coordinates, using shared snapping, elevation and depth. Set preview=true to inspect the result in render_view without changing the document; repeat with preview=false to create it."
    )]
    fn draw_block(&self, Parameters(args): Parameters<DrawBlockArgs>) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let projection = args
                .projection
                .as_deref()
                .map(|value| Projection::parse(value).ok_or("Invalid projection"))
                .transpose()?
                .unwrap_or(session.camera().projection);
            let (mins, maxs) = session.drawn_block_bounds(
                projection,
                glam::DVec2::from_array(args.start),
                glam::DVec2::from_array(args.end),
                args.elevation,
                args.depth,
            )?;
            if args.preview {
                session.preview_block(mins, maxs)?;
                Ok(
                    json!({ "preview": true, "bounds": bounds_json(Some((mins, maxs))), "spatial": session.spatial() }),
                )
            } else {
                let id = session.create_block(
                    Actor::Agent,
                    mins,
                    maxs,
                    args.material.as_deref().unwrap_or(DEFAULT_MATERIAL),
                    false,
                )?;
                Ok(summary(session.document(), id))
            }
        })
    }

    #[tool(
        description = "Cancel the current brush preview without changing the map or undo history."
    )]
    fn cancel_block_preview(&self) -> ToolResult {
        self.session().cancel_block_preview();
        ok(json!({ "cancelled": true }))
    }

    #[tool(
        description = "Create and select a point entity, including lights and player spawns. Applies inherited FGD defaults and validates declared property types. Custom classes and keys are supported. Light brightness uses RGB plus intensity, e.g. _light='255 220 180 400'; lighting compilation is not implemented yet."
    )]
    fn create_entity(&self, Parameters(args): Parameters<CreateEntityArgs>) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let id = session.create_entity(
                Actor::Agent,
                &args.classname,
                args.origin.map(vec3),
                args.properties.unwrap_or_default(),
            )?;
            detail(session.document(), id)
        })
    }

    #[tool(
        description = "Search the loaded FGD point and brush entity classes by classname or description. Returns class summaries for human and agent entity creation.",
        annotations(read_only_hint = true)
    )]
    fn search_entity_classes(
        &self,
        Parameters(args): Parameters<SearchEntityClassesArgs>,
    ) -> ToolResult {
        run(|| {
            let kind = match args.kind.as_deref() {
                None => None,
                Some("point") => Some(ClassKind::Point),
                Some("solid") => Some(ClassKind::Solid),
                Some(_) => return Err("kind must be point or solid".into()),
            };
            let session = self.session();
            let classes: Vec<Value> = session
                .search_entity_classes(
                    args.query.as_deref().unwrap_or(""),
                    kind,
                    args.limit.unwrap_or(100),
                )
                .iter()
                .map(|class| {
                    json!({"classname": class.name, "kind": class.kind.label(), "description": class.description})
                })
                .collect();
            Ok(json!({"classes": classes, "definitions": definitions_json(&session)}))
        })
    }

    #[tool(
        description = "Inspect a loaded entity class: inherited property types, descriptions, defaults, choices, spawnflags, helpers, inputs and outputs.",
        annotations(read_only_hint = true)
    )]
    fn get_entity_class(&self, Parameters(args): Parameters<EntityClassArgs>) -> ToolResult {
        run(|| {
            self.session()
                .definitions()
                .class(&args.classname)
                .map(class_json)
                .ok_or_else(|| {
                    format!(
                        "no definition for {}; use search_entity_classes or load_entity_definitions",
                        args.classname
                    )
                })
        })
    }

    #[tool(
        description = "Inspect an entity's outgoing and incoming I/O connections, matching targets and validation warnings. Connection indices are zero-based; use generation/revision to guard edits.",
        annotations(read_only_hint = true)
    )]
    fn get_entity_connections(&self, Parameters(args): Parameters<EntityArgs>) -> ToolResult {
        run(|| {
            let session = self.session();
            let source = single_entity(&session, args.id.as_deref())?;
            Ok(
                json!({"source":id_string(ObjectId::Entity(source)), "generation":session.document_generation(), "revision":session.document().revision(),
                "outgoing":session.connection_records(source)?.into_iter().map(connection_json).collect::<Vec<_>>(),
                "incoming":session.incoming_connections(source)?.into_iter().map(connection_json).collect::<Vec<_>>(),
                "draft":connection_draft_json(&session)}),
            )
        })
    }

    #[tool(
        description = "Browse named map targets and procedural targets, source FGD outputs and the matching target classes' inputs. Suggestions are optional: custom names and I/O remain supported.",
        annotations(read_only_hint = true)
    )]
    fn get_connection_choices(
        &self,
        Parameters(args): Parameters<ConnectionChoicesArgs>,
    ) -> ToolResult {
        run(|| {
            let session = self.session();
            let source = single_entity(&session, args.id.as_deref())?;
            let io_json = |io: source_formats::fgd::Io| json!({"name":io.name,"type":io.kind,"description":io.description});
            Ok(
                json!({"outputs":session.connection_outputs(source).into_iter().map(io_json).collect::<Vec<_>>(),
                "inputs":session.connection_inputs(source, args.target.as_deref().unwrap_or_default()).into_iter().map(io_json).collect::<Vec<_>>(),
                "targets":session.connection_targets(args.query.as_deref().unwrap_or_default()).into_iter().map(|(name,description)| json!({"name":name,"description":description})).collect::<Vec<_>>() }),
            )
        })
    }

    #[tool(
        description = "Begin a shared, cancellable entity I/O form. Omit index to add; supply an outgoing index to edit. Leaves the map and undo history unchanged."
    )]
    fn begin_connection_edit(&self, Parameters(args): Parameters<ConnectionArgs>) -> ToolResult {
        run(|| {
            let mut session = self.session();
            connection_guard(&session, &args)?;
            let source = single_entity(&session, args.id.as_deref())?;
            session.begin_connection_edit(source, args.index)?;
            Ok(connection_draft_json(&session))
        })
    }

    #[tool(
        description = "Change fields in the shared connection draft. Unfinished or invalid values return feedback without editing the map. Delay and firing count are strings; -1 is unlimited."
    )]
    fn update_connection_draft(
        &self,
        Parameters(args): Parameters<ConnectionFieldsArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            if let Some(token) = args.token {
                session.check_connection_draft(token)?;
            }
            let fields = &session
                .connection_draft()
                .ok_or("Begin a connection edit first")?
                .fields;
            let fields = ConnectionFields {
                output: args.output.unwrap_or_else(|| fields.output.clone()),
                target: args.target.unwrap_or_else(|| fields.target.clone()),
                input: args.input.unwrap_or_else(|| fields.input.clone()),
                parameter: args.parameter.unwrap_or_else(|| fields.parameter.clone()),
                delay: args.delay.unwrap_or_else(|| fields.delay.clone()),
                times: args.times.unwrap_or_else(|| fields.times.clone()),
            };
            session.update_connection_draft(fields)?;
            Ok(connection_draft_json(&session))
        })
    }

    #[tool(
        description = "Apply the shared I/O draft in one undo step. Rejects invalid fields and drafts made stale by map edits; unresolved/custom I/O generates warnings, not rejection."
    )]
    fn commit_connection_edit(
        &self,
        Parameters(args): Parameters<ConnectionDraftArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            if let Some(token) = args.token {
                session.check_connection_draft(token)?;
            }
            let source = session
                .connection_draft()
                .ok_or("Begin a connection edit first")?
                .source;
            let index = session.commit_connection_edit(Actor::Agent)?;
            Ok(connection_json(
                session
                    .connection_records(source)?
                    .into_iter()
                    .nth(index)
                    .ok_or("Connection no longer exists")?,
            ))
        })
    }

    #[tool(
        description = "Discard the shared connection draft without changing the map or undo history."
    )]
    fn cancel_connection_edit(
        &self,
        Parameters(args): Parameters<ConnectionDraftArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            if let Some(token) = args.token {
                session.check_connection_draft(token)?;
            }
            session.cancel_connection_edit();
            Ok(json!({"cancelled":true}))
        })
    }

    #[tool(
        description = "Delete one outgoing entity connection by index in one undo step. Supply generation/revision from the inspected list to reject stale indices.",
        annotations(destructive_hint = true)
    )]
    fn delete_entity_connection(&self, Parameters(args): Parameters<ConnectionArgs>) -> ToolResult {
        run(|| {
            let mut session = self.session();
            connection_guard(&session, &args)?;
            let source = single_entity(&session, args.id.as_deref())?;
            let index = args
                .index
                .ok_or("Choose the outgoing connection index to delete")?;
            session.delete_entity_connection(Actor::Agent, source, index)?;
            Ok(json!({"deleted":index,"source":id_string(ObjectId::Entity(source))}))
        })
    }

    #[tool(
        description = "Inspect incoming I/O targets, entity reference parameters, FGD reference properties and exact untyped text candidates. Includes hidden sources. Use generation/revision guards when beginning a rename.",
        annotations(read_only_hint = true)
    )]
    fn get_entity_references(&self, Parameters(args): Parameters<EntityArgs>) -> ToolResult {
        run(|| {
            let session = self.session();
            let entity = single_entity(&session, args.id.as_deref())?;
            let references = session.entity_references(entity)?;
            Ok(json!({"entity":id_string(ObjectId::Entity(entity)),
                "generation":session.document_generation(), "revision":session.document().revision(),
                "references":references.iter().map(reference_json).collect::<Vec<_>>(),
                "draft":rename_draft_json(&session)}))
        })
    }

    #[tool(
        description = "Begin a shared entity rename preview without editing the map. Exact references can be propagated atomically; shared names require an explicit group rename. Wildcards and untyped text remain unchanged for review."
    )]
    fn begin_entity_rename(&self, Parameters(args): Parameters<RenameArgs>) -> ToolResult {
        run(|| {
            let mut session = self.session();
            match (args.generation, args.revision) {
                (Some(generation), Some(revision))
                    if generation != session.document_generation()
                        || revision != session.document().revision() =>
                {
                    return Err("Map changed. Inspect entity references again.".into());
                }
                (Some(_), Some(_)) | (None, None) => (),
                _ => {
                    return Err(
                        "Supply both generation and revision for the stale-map guard".into(),
                    );
                }
            }
            let entity = single_entity(&session, args.id.as_deref())?;
            session.begin_entity_rename(entity)?;
            Ok(rename_draft_json(&session))
        })
    }

    #[tool(
        description = "Change the proposed name or group option and inspect all affected references. No map edit. accept_review acknowledges unchanged references; changing name/group resets it, so review the preview before acknowledging in a subsequent call."
    )]
    fn update_entity_rename(&self, Parameters(args): Parameters<RenameFieldsArgs>) -> ToolResult {
        run(|| {
            let mut session = self.session();
            if let Some(token) = args.token {
                session.check_rename_draft(token)?;
            }
            let draft = session
                .rename_draft()
                .ok_or("Begin an entity rename first")?;
            let (name, group, review) = (
                args.new_name.unwrap_or_else(|| draft.new_name.clone()),
                args.rename_group.unwrap_or(draft.rename_group),
                args.accept_review.unwrap_or(draft.accept_review),
            );
            session.update_entity_rename(name, group, review)?;
            if let Some(show_details) = args.show_details {
                session.set_rename_details(show_details)?;
            }
            Ok(rename_draft_json(&session))
        })
    }

    #[tool(
        description = "Apply the shared rename and exact reference changes in one undo step. Rejects occupied names, invalid names, unreviewed ambiguous references, replaced drafts and map/FGD changes. Preserves imported connection formatting and unrelated data."
    )]
    fn commit_entity_rename(
        &self,
        Parameters(args): Parameters<ConnectionDraftArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            if let Some(token) = args.token {
                session.check_rename_draft(token)?;
            }
            let plan = session.commit_entity_rename(Actor::Agent)?;
            Ok(rename_plan_json(&session, &plan))
        })
    }

    #[tool(
        description = "Discard the shared entity rename preview without changing the map or undo history."
    )]
    fn cancel_entity_rename(
        &self,
        Parameters(args): Parameters<ConnectionDraftArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            if let Some(token) = args.token {
                session.check_rename_draft(token)?;
            }
            session.cancel_entity_rename();
            Ok(json!({"cancelled":true}))
        })
    }

    #[tool(
        description = "Load an FGD and its includes, resolving inherited entity definitions. Updates the GUI and MCP class catalog together; existing map data is preserved."
    )]
    async fn load_entity_definitions(
        &self,
        Parameters(args): Parameters<DefinitionsArgs>,
    ) -> ToolResult {
        let session = self.session.clone();
        match tokio::task::spawn_blocking(move || {
            let catalog = Catalog::load(&PathBuf::from(args.path))?;
            let mut session = lock(&session);
            session.set_definitions(Actor::Agent, catalog);
            Ok::<_, String>(definitions_json(&session))
        })
        .await
        {
            Ok(Ok(value)) => ok(value),
            Ok(Err(error)) => fail(error),
            Err(error) => fail(error.to_string()),
        }
    }

    #[tool(
        description = "Set and/or remove entity properties (including classname, origin, angles, targetname and light brightness). Validates declared FGD types, choices and read-only fields atomically. Unknown keys are preserved and editable."
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
            session.set_entity_properties(Actor::Agent, entity, &changes)?;
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
        description = "Move, rotate and scale objects in one atomic undoable edit, or create a shared draft with preview=true. Choose world (default) or local space. Scale about the pivot, rotate X/Y/Z in that frame, then translate in that frame. Defaults to selection and its bounds center. Point entities rotate their Source orientation and move with the group; model size stays unchanged. Positive scale only. Inspect get_transform_handles for the shared draft; commit_transform_preview or cancel_transform_preview completes it."
    )]
    fn transform_objects(&self, Parameters(args): Parameters<TransformArgs>) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let ids = ids_or_selection(&session, args.ids.as_deref())?;
            let local = match args.space.as_deref().unwrap_or("world") {
                "world" => false,
                "local" => true,
                _ => return Err("space must be world or local".into()),
            };
            let basis = session.transform_basis(&ids, local)?;
            let transform = edit::Transform {
                translation: args.translation.map(vec3).unwrap_or(DVec3::ZERO),
                rotation: args.rotation.map(vec3).unwrap_or(DVec3::ZERO),
                scale: args.scale.map(vec3).unwrap_or(DVec3::ONE),
                texture_lock: args.texture_lock.unwrap_or(true),
                basis,
                ..edit::Transform::default()
            };
            let transform = edit::Transform {
                translation: basis * transform.translation,
                ..transform
            };
            let pivot = if args.preview {
                session.preview_transform(Actor::Agent, &ids, transform, args.pivot.map(vec3))?
            } else {
                session.transform_objects(Actor::Agent, &ids, transform, args.pivot.map(vec3))?
            };
            let document = if args.preview {
                session.render_document()
            } else {
                session.document()
            };
            Ok(json!({
                "pivot": point(pivot),
                "bounds": bounds_json(query::bounds_of(document, &ids)),
                "objects": ids.iter().map(|&id| summary(document, id)).collect::<Vec<_>>(),
                "preview": args.preview, "state": transform_state(&session),
            }))
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
        description = "Switch the shared viewport selection mode: objects or faces. Clears the previous selection and cancels obsolete previews."
    )]
    fn set_selection_mode(&self, Parameters(args): Parameters<SelectionModeArgs>) -> ToolResult {
        run(|| {
            let enabled = match args.mode.as_str() {
                "objects" => false,
                "faces" => true,
                _ => return Err("mode must be objects or faces".into()),
            };
            self.session().set_face_mode(enabled);
            Ok(json!({"mode": args.mode}))
        })
    }

    #[tool(
        description = "Select individual brush faces using solid and side IDs from picking or object inspection. Shared with the user's face selection; replace/add/remove/toggle modes. Validates the whole batch."
    )]
    fn select_faces(&self, Parameters(args): Parameters<SelectFacesArgs>) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let mode = match args.mode.as_deref().unwrap_or("replace") {
                "replace" => SelectMode::Replace,
                "add" => SelectMode::Add,
                "remove" => SelectMode::Remove,
                "toggle" => SelectMode::Toggle,
                _ => return Err("mode must be replace, add, remove or toggle".into()),
            };
            session.select_faces(&args.faces.iter().map(face_id).collect::<Vec<_>>(), mode)?;
            Ok(face_state(&session))
        })
    }

    #[tool(
        description = "Inspect shared selected faces, texture parameters, owners and active preview. Face IDs work for world brushes and brushes inside entities.",
        annotations(read_only_hint = true)
    )]
    fn get_face_selection(&self) -> ToolResult {
        ok(face_state(&self.session()))
    }

    #[tool(
        description = "Edit materials and texture offsets (texels), scales (world units per texel) and absolute rotation (degrees about the face normal). Omitted values preserve each face's original value. Omitted faces use shared face selection. Fit maps one material tile onto each face's projected bounds using actual texture dimensions. Preview renders a draft without changing the map or undo history; commit or cancel it with the preview tools. Atomic, undoable when applied."
    )]
    fn edit_faces(&self, Parameters(args): Parameters<EditFacesArgs>) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let faces = args.faces.as_ref().map_or_else(
                || session.selected_faces(),
                |faces| faces.iter().map(face_id).collect(),
            );
            let changed = session.edit_faces(
                Actor::Agent,
                &faces,
                map_core::surface::SurfaceEdit {
                    material: args.material,
                    shift_u: args.shift_u,
                    shift_v: args.shift_v,
                    scale_u: args.scale_u,
                    scale_v: args.scale_v,
                    rotation: args.rotation,
                    alignment: match args.alignment.as_deref() {
                        None => None,
                        Some("world") => Some(map_core::surface::Alignment::World),
                        Some("face") => Some(map_core::surface::Alignment::Face),
                        _ => return Err("alignment must be world or face".into()),
                    },
                    justify_u: texture_justification(args.justify_u.as_deref(), false)?,
                    justify_v: texture_justification(args.justify_v.as_deref(), true)?,
                    ..Default::default()
                },
                args.fit.unwrap_or(false),
                args.preview.unwrap_or(false),
            )?;
            Ok(json!({"faces_changed": changed, "state": face_state(&session)}))
        })
    }

    #[tool(
        description = "Sample committed material and UV projection from one face into the shared texture sample. Select a source, sample it, then select targets and call paint_face_texture. The sample is a snapshot, survives selection/source edits, and clears on map replacement. Does not edit the map."
    )]
    fn sample_face_texture(&self, Parameters(args): Parameters<SampleFaceArgs>) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let face = match args.face {
                Some(face) => face_id(&face),
                None => match session.selected_faces().as_slice() {
                    [face] => *face,
                    _ => return Err("select exactly one source face or provide face".into()),
                },
            };
            session.sample_face_texture(face)?;
            Ok(json!({"sample": surface_sample_state(&session)}))
        })
    }

    #[tool(
        description = "Read the shared sampled material/projection, including original geometry and whether its source still matches. Null means no sample. Sampling does not automatically follow later source edits."
    )]
    fn get_surface_sample(&self) -> ToolResult {
        ok(json!({"sample": surface_sample_state(&self.session())}))
    }

    #[tool(
        description = "Paint sampled material onto target faces in one undo step. projection also copies UV vectors/scales/offsets and rejects projections that collapse on the target. wrap rotates around an actual shared edge and preserves its texel coordinates; displacements and unrelated faces are rejected. Omitted targets use selected faces. A failed batch changes nothing; preview shares commit_face_preview/cancel_face_preview and is invalidated by map or selection edits."
    )]
    fn paint_face_texture(&self, Parameters(args): Parameters<PaintFacesArgs>) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let faces = args.faces.as_ref().map_or_else(
                || session.selected_faces(),
                |faces| faces.iter().map(face_id).collect(),
            );
            let mode = match args.mode.as_deref().unwrap_or("material") {
                "material" => map_core::surface::PaintMode::Material,
                "projection" => map_core::surface::PaintMode::Projection,
                "wrap" => map_core::surface::PaintMode::Wrap,
                _ => return Err("mode must be material, projection or wrap".into()),
            };
            let changed = session.paint_face_texture(Actor::Agent, &faces, mode, args.preview)?;
            Ok(json!({"faces_changed": changed, "state": face_state(&session)}))
        })
    }

    #[tool(
        description = "Apply the current shared face-texture preview in one undo step. Fails if edits, document replacement or selection changes have invalidated it."
    )]
    fn commit_face_preview(&self) -> ToolResult {
        run(|| {
            let changed = self.session().commit_face_preview(Actor::Agent)?;
            Ok(json!({"faces_changed": changed}))
        })
    }

    #[tool(
        description = "Cancel the shared face-texture preview, restoring the viewport without changing the document or undo/redo history. Equivalent to the texture dialog's Cancel button."
    )]
    fn cancel_face_preview(&self) -> ToolResult {
        self.session().cancel_face_preview();
        ok(json!({"preview": false}))
    }

    #[tool(
        description = "Enable or leave the shared human clipping tool. Enabling needs selected brushes, leaves drawing mode and selects Top when in perspective. Disabling cancels the cut without an edit."
    )]
    fn set_clip_tool(&self, Parameters(args): Parameters<ClipToolArgs>) -> ToolResult {
        run(|| {
            let mut session = self.session();
            session.set_clip_tool(Actor::Agent, args.enabled)?;
            Ok(clip_state(&session))
        })
    }

    #[tool(
        description = "Clip selected world brushes or brush entities by normal/distance, three VMF-wound points, or a snapped planar line. Keep front (positive-normal arrow side), back or both. Non-crossing brushes stay unchanged; point entities and intersected displacements are rejected atomically. Existing UVs/metadata survive; caps use material or inherit the first source material. preview=true leaves document and undo untouched; inspect with get_clip_preview and render_view, then commit or cancel."
    )]
    fn clip_brushes(&self, Parameters(args): Parameters<ClipArgs>) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let plane = args.plane.resolve(&session)?;
            let ids = ids_or_selection(&session, args.ids.as_deref())?;
            let keep = match args.keep.as_deref().unwrap_or("both") {
                "front" => map_core::clip::Keep::Front,
                "back" => map_core::clip::Keep::Back,
                "both" => map_core::clip::Keep::Both,
                _ => return Err("keep must be front, back or both".into()),
            };
            let result = session.clip_brushes(
                Actor::Agent,
                &ids,
                plane,
                keep,
                args.material,
                args.preview,
            )?;
            Ok(json!({"result": clip_result(&result), "state": clip_state(&session)}))
        })
    }

    #[tool(
        description = "Inspect the shared clipping tool and cut preview, including normalized plane, keep side, material, result IDs, piece bounds and face projections.",
        annotations(read_only_hint = true)
    )]
    fn get_clip_preview(&self) -> ToolResult {
        ok(clip_state(&self.session()))
    }

    #[tool(
        description = "Apply the current shared cut preview in one undo step. Rejects cancelled or invalidated previews; selects the resulting objects and leaves clipping mode."
    )]
    fn commit_clip_preview(&self) -> ToolResult {
        run(|| {
            self.session()
                .commit_clip_preview(Actor::Agent)
                .map(|result| clip_result(&result))
        })
    }

    #[tool(
        description = "Discard the shared cut preview without changing map or undo history. Invalidates an active human cut gesture; clipping mode remains available for another cut."
    )]
    fn cancel_clip_preview(&self) -> ToolResult {
        let mut session = self.session();
        session.cancel_clip_preview();
        ok(clip_state(&session))
    }

    #[tool(
        description = "Read or set the shared viewport transform tool: select/move/rotate/scale, world/local axes, selection/world/custom pivot, texture locking and angle snap. Local axes follow the first selected brush's first face or point-entity orientation. Changing controls cancels pending transform/area interactions."
    )]
    fn set_transform_tool(&self, Parameters(args): Parameters<TransformToolArgs>) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let mut tool = session.transform_tool();
            if let Some(mode) = args.mode {
                tool.mode = match mode.as_str() {
                    "select" => None,
                    "move" => Some(render::handles::Mode::Move),
                    "rotate" => Some(render::handles::Mode::Rotate),
                    "scale" => Some(render::handles::Mode::Scale),
                    _ => return Err("mode must be select, move, rotate or scale".into()),
                };
            }
            if let Some(space) = args.space {
                tool.local = match space.as_str() {
                    "world" => false,
                    "local" => true,
                    _ => return Err("space must be world or local".into()),
                };
            }
            if let Some(mode) = args.pivot_mode {
                tool.pivot = match mode.as_str() {
                    "selection" => None,
                    "world" => Some(DVec3::ZERO),
                    "custom" => Some(
                        args.pivot
                            .map(vec3)
                            .or(tool.pivot)
                            .ok_or("custom pivot needs coordinates")?,
                    ),
                    _ => return Err("pivot_mode must be selection, world or custom".into()),
                };
            } else if let Some(pivot) = args.pivot {
                tool.pivot = Some(vec3(pivot));
            }
            tool.texture_lock = args.texture_lock.unwrap_or(tool.texture_lock);
            tool.angle_step = args.angle_step.unwrap_or(tool.angle_step);
            session.set_transform_tool(Actor::Agent, tool)?;
            Ok(transform_state(&session))
        })
    }

    #[tool(
        description = "Inspect the shared transform settings, gesture and draft, plus the same visible handle paths used by the renderer, in world coordinates and viewport pixels. Optional point hit-tests those paths. Camera overrides inspect a view without moving the user.",
        annotations(read_only_hint = true)
    )]
    fn get_transform_handles(&self, Parameters(args): Parameters<HandlesArgs>) -> ToolResult {
        run(|| {
            let session = self.session();
            let size = render_size(args.width, args.height);
            let camera = args
                .camera
                .unwrap_or_default()
                .resolve(&session, size.0 as f32 / size.1 as f32)?;
            let guide = session.transform_guide();
            let shapes = guide.map_or_else(Vec::new, |guide| {
                render::handles::shapes(&camera, size, &guide)
            });
            let picked = if let (Some(guide), Some(point)) = (guide, args.point) {
                let point = glam::Vec2::new(point[0] as f32, point[1] as f32);
                if !point.is_finite() {
                    return Err("Handle point must be finite".into());
                }
                render::handles::pick(&camera, size, &guide, point)
                    .map(render::handles::Handle::name)
            } else {
                None
            };
            Ok(
                json!({"state": transform_state(&session), "camera": camera_json(&camera), "width": size.0, "height": size.1, "picked": picked,
                "origin": guide.map(|guide| guide.origin.to_array()),
                "handles": shapes.iter().map(|shape| json!({"handle": shape.handle.name(), "color": shape.color, "segments": shape.segments.iter().map(|points| points.map(|point| point.to_array())).collect::<Vec<_>>(), "pixel_segments": shape.segments.iter().filter_map(|points| Some([render::handles::project(&camera, points[0], size)?.to_array(), render::handles::project(&camera, points[1], size)?.to_array()])).collect::<Vec<_>>()})).collect::<Vec<_>>() }),
            )
        })
    }

    #[tool(
        description = "Begin the shared transform-handle interaction for selected objects. Captures the pivot, axes and original geometry. Supply a pointer for viewport dragging, or omit it for numeric amounts; movement supports axis/plane/view handles, rotation axis rings, scaling axis/uniform handles."
    )]
    fn begin_transform_drag(&self, Parameters(args): Parameters<BeginTransformArgs>) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let handle =
                render::handles::Handle::parse(&args.handle).ok_or("Unknown transform handle")?;
            session.begin_transform_drag(
                handle,
                args.pointer.map(PointerArgs::resolve).transpose()?,
            )?;
            Ok(transform_state(&session))
        })
    }

    #[tool(
        description = "Update the current shared handle preview by pointer or numeric total amount from the drag start. Axis move uses units, rotation degrees, scale a positive factor; plane_offset supplies both plane/view offsets. Shared grid, Snap and angle step apply. Every update starts from the original document; it does not add undo history."
    )]
    fn update_transform_drag(
        &self,
        Parameters(args): Parameters<UpdateTransformArgs>,
    ) -> ToolResult {
        run(|| {
            if args.amount.is_some() && args.plane_offset.is_some() {
                return Err("Provide amount or plane_offset, not both".into());
            }
            let amount = args
                .plane_offset
                .map(glam::DVec2::from_array)
                .or(args.amount.map(|amount| glam::DVec2::new(amount, 0.0)));
            let mut session = self.session();
            session.update_transform_drag(
                args.pointer.map(PointerArgs::resolve).transpose()?,
                amount,
            )?;
            Ok(transform_state(&session))
        })
    }

    #[tool(
        description = "Apply the shared transform preview in one undo step, preserving the selected IDs. Rejects cancelled or invalidated previews. Equivalent to releasing a human handle drag or choosing Apply transform."
    )]
    fn commit_transform_preview(&self) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let pivot = session.commit_transform_preview(Actor::Agent)?;
            Ok(json!({"pivot": pivot.to_array(), "state": transform_state(&session)}))
        })
    }

    #[tool(
        description = "Discard the shared transform preview and active human/agent handle interaction without changing the map or undo history."
    )]
    fn cancel_transform_preview(&self) -> ToolResult {
        let mut session = self.session();
        session.cancel_transform_preview();
        ok(transform_state(&session))
    }

    #[tool(
        description = "Read or change shared marquee-selection rules: contained/intersecting and replace/add/remove/toggle. Changing options cancels an unfinished area selection."
    )]
    fn set_area_selection_options(
        &self,
        Parameters(args): Parameters<AreaOptionsArgs>,
    ) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let rule = args
                .rule
                .map(|rule| area_rule(&rule))
                .transpose()?
                .unwrap_or(session.area_rule());
            let mode = args
                .mode
                .map(|mode| parse_select_mode(&mode))
                .transpose()?
                .unwrap_or(session.area_mode());
            session.set_area_settings(rule, mode);
            Ok(area_state(&session))
        })
    }

    #[tool(
        description = "Select objects or faces through a viewport rectangle, matching the human marquee in perspective/top/front/side views. Uses actual brush/face geometry, point-entity bounds, hidden-object and tool-texture filters; includes geometry through depth. Brush entities select as owners. Contained requires the entire visible object/face inside; intersecting accepts overlap. preview=true leaves selection/history unchanged and shows candidate highlights. Camera overrides do not move the user."
    )]
    fn select_area(&self, Parameters(args): Parameters<AreaArgs>) -> ToolResult {
        run(|| {
            let pointer = crate::session::Pointer {
                position: glam::DVec2::from_array(args.start),
                size: (args.width, args.height),
            }
            .validate()?;
            let mut session = self.session();
            let camera = args
                .camera
                .unwrap_or_default()
                .resolve(&session, args.width as f32 / args.height as f32)?;
            let rule = args
                .rule
                .map(|rule| area_rule(&rule))
                .transpose()?
                .unwrap_or(session.area_rule());
            let mode = args
                .mode
                .map(|mode| parse_select_mode(&mode))
                .transpose()?
                .unwrap_or(session.area_mode());
            let matches = session.preview_area_selection(
                Actor::Agent,
                camera,
                pointer,
                glam::DVec2::from_array(args.end),
                rule,
                mode,
            )?;
            if !args.preview {
                session.commit_area_selection()?;
            }
            Ok(json!({"matches": area_matches(&matches), "state": area_state(&session)}))
        })
    }

    #[tool(
        description = "Inspect shared area-selection options and draft: viewport rectangle, camera, object/face mode and matching IDs.",
        annotations(read_only_hint = true)
    )]
    fn get_area_selection(&self) -> ToolResult {
        ok(area_state(&self.session()))
    }

    #[tool(
        description = "Apply the shared area-selection draft with its replace/add/remove/toggle rule. Selection changes do not add map undo history."
    )]
    fn commit_area_selection(&self) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let matches = session.commit_area_selection()?;
            Ok(json!({"matches": area_matches(&matches), "state": area_state(&session)}))
        })
    }

    #[tool(
        description = "Cancel the shared area-selection draft and invalidate an unfinished human marquee without changing selection or map history."
    )]
    fn cancel_area_selection(&self) -> ToolResult {
        let mut session = self.session();
        session.cancel_area_selection();
        ok(area_state(&session))
    }

    #[tool(
        description = "Browse the same grouped material catalog as the GUI. Groups include all, used, recent, favorites, folder:<path> and category:<label>. Categories are inferred from path names. Returns group counts after search, paged entries and favorite/used flags. preview_material supplies the matching thumbnail.",
        annotations(read_only_hint = true)
    )]
    fn browse_materials(&self, Parameters(args): Parameters<BrowseMaterialsArgs>) -> ToolResult {
        run(|| {
            let catalog = self.session().material_catalog(
                args.query.as_deref().unwrap_or(""),
                args.group.as_deref().unwrap_or("all"),
                args.offset.unwrap_or(0),
                args.limit.unwrap_or(36),
            )?;
            serde_json::to_value(catalog).map_err(|error| error.to_string())
        })
    }

    #[tool(
        description = "Add or remove a material in the shared session's favorites, also visible in the GUI catalog. Favorites and recent applications are session-local."
    )]
    fn set_material_favorite(
        &self,
        Parameters(args): Parameters<FavoriteMaterialArgs>,
    ) -> ToolResult {
        run(|| {
            self.session()
                .favorite_material(&args.material, args.favorite)?;
            Ok(json!({"material": args.material, "favorite": args.favorite}))
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
            if args.ids.is_some() {
                targets.extend(
                    ids_or_selection(&session, args.ids.as_deref())?
                        .into_iter()
                        .map(FaceTarget::Object),
                );
            }
            if args.ids.is_none() && args.faces.is_none() {
                targets = session.material_targets();
            }
            if targets.is_empty() {
                return Err("select objects or faces to apply a material".into());
            }
            let changed = session.apply_material(Actor::Agent, &targets, &args.material)?;
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
            let entity = session.tie_to_entity(
                Actor::Agent,
                &ids,
                &args.classname,
                args.properties.unwrap_or_default(),
            )?;
            detail(session.document(), entity)
        })
    }

    #[tool(description = "Move a brush entity's brushes back into the world, deleting the entity.")]
    fn move_to_world(&self, Parameters(args): Parameters<EntityArgs>) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let entity = single_entity(&session, args.id.as_deref())?;
            let solids = session.move_to_world(Actor::Agent, entity)?;
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
        description = "Frame the current selection, or the whole map when nothing is selected, preserving the perspective/top/front/side projection."
    )]
    fn frame_selection(&self, Parameters(args): Parameters<FrameArgs>) -> ToolResult {
        run(|| {
            let aspect = args.aspect.unwrap_or(1.0);
            if !aspect.is_finite() || aspect <= 0.0 {
                return Err("aspect must be positive and finite".into());
            }
            let mut session = self.session();
            session.frame_selection(Actor::Agent, aspect)?;
            Ok(camera_json(&session.camera()))
        })
    }

    #[tool(
        description = "Move the user's viewport camera or switch perspective/top/front/side. Perspective supports position + look_at; planar views support center + ortho_height. Pass focus to frame object IDs."
    )]
    fn set_camera(&self, Parameters(spec): Parameters<CameraSpec>) -> ToolResult {
        run(|| {
            let mut session = self.session();
            let camera = spec.resolve(&session, 1.0)?;
            session.set_camera(camera);
            session.log(Actor::Agent, "moved the camera");
            Ok(camera_json(&camera))
        })
    }

    #[tool(
        description = "Render the map to a PNG with the editor's renderer: textured perspective or planar brush wireframes, entity helpers, grid and shared brush preview. Selection is highlighted in red. Defaults to the user's view; pass camera.focus to frame objects.",
        annotations(read_only_hint = true)
    )]
    fn render_view(&self, Parameters(args): Parameters<RenderArgs>) -> ToolResult {
        let size = render_size(args.width, args.height);
        let (camera, mut view) = {
            let session = self.session();
            match args
                .camera
                .unwrap_or_default()
                .resolve(&session, size.0 as f32 / size.1 as f32)
            {
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
        let png = self.render_png(&camera, size, view);
        let data = base64::engine::general_purpose::STANDARD.encode(png);
        let info = json!({ "camera": camera_json(&camera), "width": size.0, "height": size.1,
            "view": { "tool_textures": view.tool_textures, "leak_path": view.leak_path,
                "grid_visible": view.grid_visible, "grid_size": view.grid_size,
                "grid_elevation": view.grid_elevation,
                "block_preview": view.block_preview.map(|bounds| bounds.map(|point| point.to_array())),
                "clip_preview": view.clip_preview.map(|guide| json!({"normal": guide.normal.to_array(), "distance": guide.distance, "bounds": guide.bounds.map(|point| point.to_array())})),
                "transform_handles": view.transform_handles.map(|guide| json!({"mode": transform_mode_name(Some(guide.mode)), "origin": guide.origin.to_array(), "axes": guide.axes.map(|axis| axis.to_array()), "active": guide.active.map(render::handles::Handle::name)})),
                "area_preview": view.area_preview.map(|guide| json!({"ndc": guide.corners.map(|point| point.to_array()), "contained": guide.contained, "aspect": guide.aspect, "camera": camera_json(&guide.camera)})) } });
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
            let (w, h) = render_size(args.width, args.height);
            let camera = args
                .camera
                .unwrap_or_default()
                .resolve(&session, w as f32 / h as f32)?;
            let ndc_x = 2.0 * (args.x + 0.5) / w as f32 - 1.0;
            let ndc_y = 1.0 - 2.0 * (args.y + 0.5) / h as f32;
            let (origin, direction) = camera.ray(ndc_x, ndc_y, w as f32 / h as f32);
            let ray = Ray {
                origin: origin.as_dvec3(),
                direction: direction.as_dvec3(),
            };
            let hit = session.pick(&ray);
            Ok(hit.map_or(json!({ "hit": null }), |hit| {
                hit_json(session.render_document(), &hit)
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
                hit_json(session.render_document(), &hit)
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

    #[tool(
        description = "Preview a game material as a PNG, with its name, original dimensions, tool status and missing-texture status. Uses the same thumbnail as the GUI material picker.",
        annotations(read_only_hint = true)
    )]
    fn preview_material(&self, Parameters(args): Parameters<MaterialPreviewArgs>) -> ToolResult {
        let preview = match self.session().material_preview(&args.material) {
            Ok(preview) => preview,
            Err(error) => return fail(error),
        };
        let mut content = vec![ContentBlock::text(json!({"material": preview.name, "width": preview.width, "height": preview.height, "missing": preview.missing, "tool": preview.tool}).to_string())];
        if let Some(image) = preview.image {
            let mut bytes = Vec::new();
            let mut encoder = png::Encoder::new(Cursor::new(&mut bytes), image.width, image.height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let result = encoder
                .write_header()
                .and_then(|mut writer| writer.write_image_data(&image.pixels));
            if let Err(error) = result {
                return fail(error.to_string());
            }
            content.push(ContentBlock::image(
                base64::engine::general_purpose::STANDARD.encode(bytes),
                "image/png",
            ));
        }
        Ok(CallToolResult::success(content))
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
    fn planar_camera_overrides_do_not_change_the_shared_view() {
        let session = Session::new(Err("no assets needed".into()));
        let original = session.camera();
        let spec = CameraSpec {
            projection: Some("top".into()),
            center: Some([10.0, -20.0, 0.0]),
            ortho_height: Some(256.0),
            ..CameraSpec::default()
        };
        let camera = spec.resolve(&session, 1.0).unwrap();
        assert_eq!(camera.projection, Projection::Top);
        assert_eq!(camera.ortho_height, 256.0);
        assert_eq!(camera.position.x, 10.0);
        assert_eq!(camera.position.y, -20.0);
        assert_eq!(session.camera(), original);
        assert!(
            CameraSpec {
                pitch: Some(20.0),
                ..spec
            }
            .resolve(&session, 1.0)
            .is_err()
        );
        assert!(
            CameraSpec {
                position: Some([f32::NAN, 0.0, 0.0]),
                ..CameraSpec::default()
            }
            .resolve(&session, 1.0)
            .is_err()
        );
    }

    #[test]
    fn clipping_plane_inputs_agree_and_reject_ambiguous_definitions() {
        let session = Session::new(Err("no assets needed".into()));
        let spec: ClipPlane =
            serde_json::from_value(json!({"normal": [3, 4, 5], "distance": 7})).unwrap();
        let plane = spec.resolve(&session).unwrap();
        let points = map_core::clip::plane_points(plane).map(|point| point.to_array());
        let spec: ClipPlane = serde_json::from_value(json!({"points": points})).unwrap();
        let actual = spec.resolve(&session).unwrap();
        assert!(actual.normal.abs_diff_eq(plane.normal, 1e-12));
        assert!((actual.dist - plane.dist).abs() < 1e-12);
        for projection in ["top", "front", "side"] {
            let spec: ClipPlane = serde_json::from_value(
                json!({"line": {"projection": projection, "start": [0, -32], "end": [0, 32]}}),
            )
            .unwrap();
            let actual = spec.resolve(&session).unwrap();
            let expected = session
                .clip_line_plane(
                    Projection::parse(projection).unwrap(),
                    glam::DVec2::new(0.0, -32.0),
                    glam::DVec2::new(0.0, 32.0),
                )
                .unwrap();
            assert_eq!(actual, expected);
        }
        for value in [
            json!({}),
            json!({"normal": [1, 0, 0]}),
            json!({"normal": [1, 0, 0], "distance": 0, "points": points}),
            json!({"points": [[0, 0, 0], [1, 1, 1], [2, 2, 2]]}),
            json!({"line": {"projection": "perspective", "start": [0, 0], "end": [32, 32]}}),
        ] {
            let spec: ClipPlane = serde_json::from_value(value).unwrap();
            assert!(spec.resolve(&session).is_err());
        }
    }

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
