mod game;
mod inspector;
mod interaction;
mod materials;
mod mcp;
mod session;
mod spatial;
mod steam;
mod viewport;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use glam::{DVec2, DVec3, Vec3};
use map_core::ObjectId;
use map_core::edit;
use map_core::query::{self, Ray};
use mcp::{EditorServer, SharedSession, id_string, lock};
use render::{GpuContext, Projection};
use session::{Actor, Pointer, SelectMode, Session, TransformTool};
use slint::platform::Key;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use source_formats::fgd::{Catalog, ClassKind};
use source_formats::gamefs::GameFs;
use viewport::{Viewport, key_text};

slint::include_modules!();

const USAGE: &str = "\
usage: editor [map.vmf] [--game <gameinfo.txt>] [--fgd <file.fgd>] [--mcp-port <port> | --no-mcp] [--headless]

  --game       gameinfo.txt to load content from (default: Counter-Strike: Source via Steam)
  --fgd        entity definition file (default: installed CS:S SDK FGD)
  --mcp-port   port for the MCP server at http://127.0.0.1:<port>/mcp (default 7377)
  --no-mcp     don't start the MCP server
  --headless   no window; serve MCP over stdin/stdout instead";

const DEFAULT_MCP_PORT: u16 = 7377;
const BLOCK_SIZE: f64 = 128.0;
const ACTIVITY_SHOWN: usize = 30;
const SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(10);

struct ShutdownGuard(SharedSession);

impl Drop for ShutdownGuard {
    fn drop(&mut self) {
        lock(&self.0).shutdown();
        loop {
            let launching = lock(&self.0).launching();
            if !launching {
                break;
            }
            std::thread::sleep(SHUTDOWN_POLL_INTERVAL);
        }
    }
}

struct Args {
    map: Option<PathBuf>,
    gameinfo: Option<PathBuf>,
    fgd: Option<PathBuf>,
    mcp_port: Option<u16>,
    headless: bool,
}

impl Args {
    fn parse() -> Result<Self, String> {
        let mut args = std::env::args_os().skip(1);
        let mut parsed = Self {
            map: None,
            gameinfo: None,
            fgd: None,
            mcp_port: Some(DEFAULT_MCP_PORT),
            headless: false,
        };
        while let Some(arg) = args.next() {
            match arg.to_str() {
                Some("--game") => parsed.gameinfo = Some(args.next().ok_or(USAGE)?.into()),
                Some("--fgd") => parsed.fgd = Some(args.next().ok_or(USAGE)?.into()),
                Some("--mcp-port") => {
                    let port = args
                        .next()
                        .and_then(|p| p.to_str()?.parse().ok())
                        .ok_or(USAGE)?;
                    parsed.mcp_port = Some(port);
                }
                Some("--no-mcp") => parsed.mcp_port = None,
                Some("--headless") => parsed.headless = true,
                Some("-h" | "--help") => return Err(USAGE.into()),
                _ if parsed.map.is_none() => parsed.map = Some(arg.into()),
                _ => return Err(USAGE.into()),
            }
        }
        Ok(parsed)
    }
}

fn mount_game(gameinfo: Option<PathBuf>) -> Result<GameFs, String> {
    let gameinfo = gameinfo.or_else(steam::find_css_gameinfo).ok_or(
        "Counter-Strike: Source not found in any Steam library; pass --game <gameinfo.txt>",
    )?;
    GameFs::from_gameinfo(&gameinfo).map_err(|e| e.to_string())
}

struct Ui {
    ctx: GpuContext,
    session: SharedSession,
    viewport: Viewport,
    seen_scene: u64,
    seen_camera: u64,
    seen_view: u64,
    brush_drag: Option<BrushDrag>,
    clip_drag: Option<BrushDrag>,
    interaction: interaction::Interaction,
    seen_log: u64,
    game_log_window: Option<GameLogWindow>,
    last_log_refresh: Instant,
    entity_window: Option<EntityWindow>,
    color_window: Option<ColorWindow>,
    color_target: Option<(u32, String, u64)>,
    material_window: Option<MaterialWindow>,
    material_target: Option<(u32, String, u64)>,
    surface_window: Option<SurfaceWindow>,
    surface_target: Option<(Vec<map_core::surface::FaceId>, u64, u64)>,
    material_for_faces: bool,
    material_for_clip: bool,
    seen_definitions: u64,
}

#[derive(Clone, Copy)]
struct BrushDrag {
    start: DVec2,
    epoch: u64,
    generation: u64,
    revision: u64,
    projection: Projection,
}

type SharedUi = Rc<RefCell<Ui>>;

fn session_of(ui: &SharedUi) -> SharedSession {
    ui.borrow().session.clone()
}

fn refresh_spatial(app: &AppWindow, session: &Session) {
    refresh_interaction(app, session);
    refresh_clip(app, session);
    let settings = session.spatial();
    let projection = session.camera().projection;
    app.set_projection_index(projection.index() as i32);
    app.set_workplane_axis(
        match projection {
            Projection::Front => "Y",
            Projection::Side => "X",
            _ => "Z",
        }
        .into(),
    );
    app.set_grid_size(settings.grid_size.to_string().into());
    app.set_grid_visible(settings.grid_visible);
    app.set_snap_grid(settings.snap);
    app.set_drawing_block(settings.draw_block);
    app.set_brush_elevation(settings.elevation.to_string().into());
    app.set_brush_depth(settings.depth.to_string().into());
    app.set_brush_preview(
        session
            .block_preview()
            .map(|(mins, maxs)| {
                let size = maxs - mins;
                format!(
                    "{} × {} × {} units · release to place · Esc cancels",
                    size.x, size.y, size.z
                )
            })
            .unwrap_or_default()
            .into(),
    );
}

fn refresh_interaction(app: &AppWindow, session: &Session) {
    let tool = session.transform_tool();
    let index = match tool.mode {
        None => 0,
        Some(render::handles::Mode::Move) => 1,
        Some(render::handles::Mode::Rotate) => 2,
        Some(render::handles::Mode::Scale) => 3,
    };
    app.set_handles_mode_index(index);
    if index > 0 {
        app.set_transform_mode(index - 1);
    }
    app.set_transform_space_index(i32::from(tool.local));
    app.set_rotation_step(tool.angle_step.to_string().into());
    app.set_transform_texture_lock(tool.texture_lock);
    app.set_transform_pivot_mode(match tool.pivot {
        None => 0,
        Some(pivot) if pivot == DVec3::ZERO => 1,
        Some(_) => 2,
    });
    if let Some(pivot) = tool.pivot {
        app.set_pivot_x(pivot.x.to_string().into());
        app.set_pivot_y(pivot.y.to_string().into());
        app.set_pivot_z(pivot.z.to_string().into());
    }
    app.set_area_rule_index(i32::from(
        session.area_rule() == map_core::region::Match::Contained,
    ));
    app.set_area_mode_index(match session.area_mode() {
        SelectMode::Replace => 0,
        SelectMode::Add => 1,
        SelectMode::Remove => 2,
        SelectMode::Toggle => 3,
    });
    app.set_has_transform_preview(session.transform_preview().is_some());
    app.set_has_area_preview(session.area_preview().is_some());
    let summary = if let Some(drag) = session.transform_drag() {
        let transform = session.transform_preview().map(|preview| preview.transform);
        match (drag.mode, transform) {
            (render::handles::Mode::Move, Some(transform)) => format!(
                "Move · {:.3} {:.3} {:.3} units",
                transform.translation.x, transform.translation.y, transform.translation.z
            ),
            (render::handles::Mode::Rotate, Some(transform)) => format!(
                "Rotate {} · {:.3}°",
                drag.handle.name(),
                transform.rotation[drag.handle.axis().unwrap()]
            ),
            (render::handles::Mode::Scale, Some(transform)) => format!(
                "Scale · {:.3} {:.3} {:.3}",
                transform.scale.x, transform.scale.y, transform.scale.z
            ),
            _ => "Drag to preview · release applies · Esc cancels".into(),
        }
    } else if let Some(preview) = session.transform_preview() {
        format!("{} objects · transform preview", preview.ids.len())
    } else if let Some(preview) = session.area_preview() {
        if preview.faces {
            format!(
                "{} matching faces · selection preview",
                preview.matches.faces.len()
            )
        } else {
            format!(
                "{} matching objects · selection preview",
                preview.matches.objects.len()
            )
        }
    } else {
        String::new()
    };
    app.set_interaction_summary(summary.into());
}

fn transform_tool_from_controls(app: &AppWindow) -> Result<TransformTool, String> {
    let mode = match app.get_handles_mode_index() {
        0 => None,
        1 => Some(render::handles::Mode::Move),
        2 => Some(render::handles::Mode::Rotate),
        _ => Some(render::handles::Mode::Scale),
    };
    let pivot = match app.get_transform_pivot_mode() {
        0 => None,
        1 => Some(DVec3::ZERO),
        _ => Some(input_vector([
            app.get_pivot_x(),
            app.get_pivot_y(),
            app.get_pivot_z(),
        ])?),
    };
    Ok(TransformTool {
        mode,
        local: app.get_transform_space_index() == 1,
        pivot,
        texture_lock: app.get_transform_texture_lock(),
        angle_step: app
            .get_rotation_step()
            .parse()
            .map_err(|_| "Enter a numeric angle snap")?,
    })
}

fn bind_manipulation_controls(app: &AppWindow, shared: SharedSession) {
    app.on_transform_tool_changed({
        let (app, shared) = (app.as_weak(), shared.clone());
        move || {
            let app = app.unwrap();
            let mut session = lock(&shared);
            let result = transform_tool_from_controls(&app)
                .and_then(|tool| session.set_transform_tool(Actor::User, tool));
            if let Err(error) = result {
                app.set_message(error.into());
            }
            refresh_spatial(&app, &session);
        }
    });
    app.on_area_settings_changed({
        let (app, shared) = (app.as_weak(), shared.clone());
        move || {
            let app = app.unwrap();
            let mut session = lock(&shared);
            let rule = if app.get_area_rule_index() == 1 {
                map_core::region::Match::Contained
            } else {
                map_core::region::Match::Intersecting
            };
            let mode = match app.get_area_mode_index() {
                1 => SelectMode::Add,
                2 => SelectMode::Remove,
                3 => SelectMode::Toggle,
                _ => SelectMode::Replace,
            };
            session.set_area_settings(rule, mode);
            refresh_interaction(&app, &session);
        }
    });
    app.on_apply_transform_preview({
        let (app, shared) = (app.as_weak(), shared.clone());
        move || {
            let app = app.unwrap();
            let mut session = lock(&shared);
            if let Err(error) = session.commit_transform_preview(Actor::User) {
                app.set_message(error.into());
            }
            refresh_panel(&app, &mut session);
            refresh_interaction(&app, &session);
        }
    });
    app.on_apply_area_selection({
        let (app, shared) = (app.as_weak(), shared.clone());
        move || {
            let app = app.unwrap();
            let mut session = lock(&shared);
            if let Err(error) = session.commit_area_selection() {
                app.set_message(error.into());
            }
            refresh_panel(&app, &mut session);
            refresh_interaction(&app, &session);
        }
    });
    app.on_cancel_interaction({
        let app = app.as_weak();
        move || {
            let mut session = lock(&shared);
            session.cancel_transform_preview();
            session.cancel_area_selection();
            refresh_interaction(&app.unwrap(), &session);
        }
    });
}

fn refresh_clip(app: &AppWindow, session: &Session) {
    app.set_can_clip(session.can_clip_selection());
    app.set_clipping(session.clip_tool());
    app.set_has_clip_preview(session.clip_preview().is_some());
    if let Some(preview) = session.clip_preview() {
        app.set_clip_normal_x(preview.plane.normal.x.to_string().into());
        app.set_clip_normal_y(preview.plane.normal.y.to_string().into());
        app.set_clip_normal_z(preview.plane.normal.z.to_string().into());
        app.set_clip_distance(preview.plane.dist.to_string().into());
        app.set_clip_keep_index(match preview.keep {
            map_core::clip::Keep::Front => 0,
            map_core::clip::Keep::Back => 1,
            map_core::clip::Keep::Both => 2,
        });
        app.set_clip_material(preview.material.clone().unwrap_or_default().into());
        app.set_clip_summary(
            format!(
                "{} brushes cut · {} pieces · {} unchanged · preview only",
                preview.result.cuts.len(),
                preview
                    .result
                    .cuts
                    .iter()
                    .map(|cut| cut.solids.len())
                    .sum::<usize>(),
                preview.result.unchanged.len()
            )
            .into(),
        );
    } else {
        app.set_clip_summary(
            "Draw a cut or preview a plane · the map stays unchanged until Apply cut".into(),
        );
    }
}

fn clip_keep(app: &AppWindow) -> map_core::clip::Keep {
    match app.get_clip_keep_index() {
        0 => map_core::clip::Keep::Front,
        1 => map_core::clip::Keep::Back,
        _ => map_core::clip::Keep::Both,
    }
}

fn preview_clip(
    app: &AppWindow,
    session: &mut Session,
    plane: map_core::Plane,
) -> Result<(), String> {
    let material = app.get_clip_material().trim().to_owned();
    session.clip_brushes(
        Actor::User,
        &session.selected_ids(),
        plane,
        clip_keep(app),
        (!material.is_empty()).then_some(material),
        true,
    )?;
    refresh_clip(app, session);
    Ok(())
}

fn preview_clip_plane(app: &AppWindow, session: &mut Session) {
    let result = input_vector([
        app.get_clip_normal_x(),
        app.get_clip_normal_y(),
        app.get_clip_normal_z(),
    ])
    .and_then(|normal| {
        let dist = app
            .get_clip_distance()
            .parse()
            .map_err(|_| "Enter a numeric plane distance")?;
        preview_clip(app, session, map_core::Plane { normal, dist })
    });
    if let Err(error) = result {
        session.cancel_clip_preview();
        refresh_clip(app, session);
        app.set_message(error.into());
    }
}

fn apply_clip(app: &AppWindow, session: &mut Session) {
    match session.commit_clip_preview(Actor::User) {
        Ok(result) => app
            .set_message(format!("Clipped {} brushes in one undo step", result.cuts.len()).into()),
        Err(error) => app.set_message(error.into()),
    }
    refresh_spatial(app, session);
    refresh_panel(app, session);
}

fn cancel_clip(app: &AppWindow, session: &mut Session) {
    session.cancel_clip_preview();
    drop(session.set_clip_tool(Actor::User, false));
    refresh_clip(app, session);
}

fn bind_clip_controls(app: &AppWindow, shared: SharedSession) {
    app.on_start_clip({
        let (app, shared) = (app.as_weak(), shared.clone());
        move || {
            let app = app.unwrap();
            let mut session = lock(&shared);
            if let Err(error) = session.set_clip_tool(Actor::User, true) {
                app.set_message(error.into());
            }
            refresh_spatial(&app, &session);
        }
    });
    app.on_preview_clip_plane({
        let (app, shared) = (app.as_weak(), shared.clone());
        move || preview_clip_plane(&app.unwrap(), &mut lock(&shared))
    });
    app.on_clip_settings_changed({
        let (app, shared) = (app.as_weak(), shared.clone());
        move || {
            let app = app.unwrap();
            let mut session = lock(&shared);
            if let Some(plane) = session.clip_preview().map(|preview| preview.plane)
                && let Err(error) = preview_clip(&app, &mut session, plane)
            {
                session.cancel_clip_preview();
                refresh_clip(&app, &session);
                app.set_message(error.into());
            }
        }
    });
    app.on_clip_inputs_changed({
        let (app, shared) = (app.as_weak(), shared.clone());
        move || {
            let mut session = lock(&shared);
            session.cancel_clip_preview();
            refresh_clip(&app.unwrap(), &session);
        }
    });
    app.on_apply_clip({
        let (app, shared) = (app.as_weak(), shared.clone());
        move || apply_clip(&app.unwrap(), &mut lock(&shared))
    });
    app.on_cancel_clip({
        let app = app.as_weak();
        move || cancel_clip(&app.unwrap(), &mut lock(&shared))
    });
}

fn clip_drag_plane(
    session: &Session,
    drag: BrushDrag,
    end: DVec2,
) -> Result<map_core::Plane, String> {
    if !session.clip_tool()
        || drag.generation != session.document_generation()
        || drag.revision != session.document().revision()
        || drag.projection != session.camera().projection
        || drag.epoch != session.clip_epoch()
    {
        return Err("The cut was cancelled or its map/settings changed".into());
    }
    session.clip_line_plane(drag.projection, drag.start, end)
}

fn spatial_changed(app: &AppWindow, ui: &SharedUi) {
    let result = (|| {
        let grid_size = app
            .get_grid_size()
            .parse()
            .map_err(|_| "Enter a valid grid spacing")?;
        let elevation = app
            .get_brush_elevation()
            .parse()
            .map_err(|_| "Enter a valid start coordinate")?;
        let depth = app
            .get_brush_depth()
            .parse()
            .map_err(|_| "Enter a valid brush depth")?;
        lock(&session_of(ui)).set_spatial(
            Actor::User,
            spatial::Update {
                grid_size: Some(grid_size),
                grid_visible: Some(app.get_grid_visible()),
                snap: Some(app.get_snap_grid()),
                elevation: Some(elevation),
                depth: Some(depth),
                ..spatial::Update::default()
            },
        )
    })();
    if let Err(message) = result {
        app.set_message(message.into());
        refresh_spatial(app, &lock(&session_of(ui)));
    }
}

fn frame_selection(app: &AppWindow, ui: &SharedUi) {
    let aspect = app.get_viewport_width() / app.get_viewport_height().max(1.0);
    if let Err(message) = lock(&session_of(ui)).frame_selection(Actor::User, aspect) {
        app.set_message(message.into());
    }
}

fn input_vector(values: [SharedString; 3]) -> Result<DVec3, String> {
    let mut result = DVec3::ZERO;
    for (index, value) in values.iter().enumerate() {
        result[index] = value
            .trim()
            .parse::<f64>()
            .map_err(|_| "Enter a number for each X, Y and Z component")?;
    }
    if !result.is_finite() {
        return Err("Transform components must be finite".into());
    }
    Ok(result)
}

fn apply_selection_transform(app: &AppWindow, session: &mut Session) -> Result<(), String> {
    let values = input_vector([
        app.get_transform_x(),
        app.get_transform_y(),
        app.get_transform_z(),
    ])?;
    let mut transform = edit::Transform {
        texture_lock: app.get_transform_texture_lock(),
        ..edit::Transform::default()
    };
    match app.get_transform_mode() {
        0 => transform.translation = values,
        1 => transform.rotation = values,
        2 => transform.scale = values,
        _ => return Err("Choose Move, Rotate or Scale".into()),
    }
    transform.basis = session.transform_basis(
        &session.selected_ids(),
        app.get_transform_space_index() == 1,
    )?;
    transform.translation = transform.basis * transform.translation;
    let pivot = if app.get_transform_mode() == 0 {
        None
    } else {
        match app.get_transform_pivot_mode() {
            0 => None,
            1 => Some(DVec3::ZERO),
            2 => Some(input_vector([
                app.get_pivot_x(),
                app.get_pivot_y(),
                app.get_pivot_z(),
            ])?),
            _ => return Err("Choose a pivot".into()),
        }
    };
    let ids = session.selected_ids();
    session.set_transform_tool(Actor::User, transform_tool_from_controls(app)?)?;
    session.transform_objects(Actor::User, &ids, transform, pivot)?;
    Ok(())
}

fn apply_transform(app: &AppWindow, ui: &SharedUi) {
    let result = apply_selection_transform(app, &mut lock(&session_of(ui)));
    if let Err(message) = result {
        app.set_message(message.into());
    }
}

fn plane_coordinates(
    camera: render::Camera,
    position: (f32, f32),
    size: (f32, f32),
    elevation: f64,
) -> Option<DVec2> {
    if size.0 <= 0.0 || size.1 <= 0.0 {
        return None;
    }
    let ndc = (
        position.0 * 2.0 / size.0 - 1.0,
        1.0 - position.1 * 2.0 / size.1,
    );
    let point = camera
        .plane_point(ndc, size.0 / size.1, elevation as f32)?
        .as_dvec3();
    let (horizontal, vertical, _) = camera.projection.axes()?;
    Some(DVec2::new(point[horizontal], point[vertical]))
}

fn drag_bounds(session: &Session, drag: BrushDrag, end: DVec2) -> Result<(DVec3, DVec3), String> {
    if drag.generation != session.document_generation()
        || drag.revision != session.document().revision()
        || drag.projection != session.camera().projection
        || drag.epoch != session.brush_epoch()
    {
        return Err("The brush drag was cancelled or its map/settings changed".into());
    }
    session.drawn_block_bounds(drag.projection, drag.start, end, None, None)
}

/// The horizontal world axis closest to the camera's forward direction, and
/// the one to its right.
fn nudge_axes(forward: Vec3) -> (DVec3, DVec3) {
    let f = if forward.x.abs() >= forward.y.abs() {
        DVec3::new(forward.x.signum().into(), 0.0, 0.0)
    } else {
        DVec3::new(0.0, forward.y.signum().into(), 0.0)
    };
    (f, DVec3::new(f.y, -f.x, 0.0))
}

fn selection_summary(session: &Session) -> (String, String, Option<Vec<KeyValue>>) {
    let doc = session.document();
    let ids = session.selected_ids();
    if session.face_mode() {
        let faces = session.selected_faces();
        let names = faces
            .iter()
            .filter_map(|face| map_core::surface::side(doc, *face))
            .map(|side| side.material.to_ascii_lowercase())
            .collect::<std::collections::BTreeSet<_>>();
        return (
            if faces.is_empty() {
                "Select a face in the viewport".into()
            } else {
                format!(
                    "{} selected face{}",
                    faces.len(),
                    if faces.len() == 1 { "" } else { "s" }
                )
            },
            names.into_iter().collect::<Vec<_>>().join(", "),
            None,
        );
    }
    let size_text = |bounds: Option<(DVec3, DVec3)>| {
        bounds.map_or_else(String::new, |(lo, hi)| {
            let (size, center) = (hi - lo, (lo + hi) * 0.5);
            format!(
                "{} × {} × {} at ({}, {}, {})",
                size.x.round(),
                size.y.round(),
                size.z.round(),
                center.x.round(),
                center.y.round(),
                center.z.round()
            )
        })
    };
    match ids[..] {
        [] => ("Nothing selected".into(), String::new(), None),
        [ObjectId::Entity(n)] => {
            let Some(entity) = doc.entity(n) else {
                return ("Nothing selected".into(), String::new(), None);
            };
            let title = inspector::title(entity.classname().unwrap_or("entity"));
            let mut detail = if entity.solids.is_empty() {
                String::new()
            } else {
                size_text(query::entity_bounds(entity))
            };
            if !entity.solids.is_empty() {
                detail += &format!(" · {} brushes", entity.solids.len());
            }
            let rows = entity_property_rows(session, entity);
            (title, detail, Some(rows))
        }
        [ObjectId::Solid(n)] => {
            let materials: Vec<String> = doc
                .solid(n)
                .map(|s| {
                    let mut m: Vec<String> = s
                        .sides
                        .iter()
                        .map(|f| f.material.to_ascii_lowercase())
                        .collect();
                    m.sort_unstable();
                    m.dedup();
                    m
                })
                .unwrap_or_default();
            let detail = format!(
                "{}\n{}",
                size_text(query::bounds(doc, ids[0])),
                materials.join(", ")
            );
            (format!("Brush · {}", id_string(ids[0])), detail, None)
        }
        _ => (
            format!("{} objects", ids.len()),
            size_text(query::bounds_of(doc, &ids)),
            None,
        ),
    }
}

fn entity_property_rows(session: &Session, entity: &source_formats::vmf::Entity) -> Vec<KeyValue> {
    let class = entity
        .classname()
        .and_then(|name| session.definitions().class(name));
    inspector::inspect(entity, class)
        .properties
        .into_iter()
        .map(property_row)
        .collect()
}

fn property_row(field: inspector::Field) -> KeyValue {
    use inspector::Control;
    let choice_index = field
        .choices
        .iter()
        .position(|choice| choice.value == field.value)
        .unwrap_or_default() as i32;
    let color = field.color.unwrap_or([0; 3]);
    let range = field.range.unwrap_or([0.0, 100.0]);
    let control = match field.control {
        Control::Text => "text",
        Control::Number => "number",
        Control::Slider => "slider",
        Control::Choices => "choices",
        Control::Flags => "flags",
        Control::Boolean => "boolean",
        Control::Vector => "vector",
        Control::Color => "color",
        Control::Material => "material",
    };
    KeyValue {
        key: field.key.into(),
        label: field.label.into(),
        description: field.description.into(),
        type_name: field.kind.into(),
        numeric: matches!(field.control, Control::Number | Control::Slider),
        read_only: field.read_only,
        choices: ModelRc::new(VecModel::from(
            field
                .choices
                .iter()
                .map(|choice| choice.label.clone().into())
                .collect::<Vec<_>>(),
        )),
        choice_values: ModelRc::new(VecModel::from(
            field
                .choices
                .into_iter()
                .map(|choice| choice.value.into())
                .collect::<Vec<_>>(),
        )),
        choice_index,
        flags: ModelRc::new(VecModel::from(
            field
                .flags
                .into_iter()
                .map(|flag| EntityFlag {
                    label: flag.label.into(),
                    value: flag.value.into(),
                    checked: flag.checked,
                })
                .collect::<Vec<_>>(),
        )),
        control: control.into(),
        advanced: field.advanced,
        components: ModelRc::new(VecModel::from(
            field
                .components
                .into_iter()
                .map(Into::into)
                .collect::<Vec<_>>(),
        )),
        component_labels: ModelRc::new(VecModel::from(
            field
                .component_labels
                .into_iter()
                .map(Into::into)
                .collect::<Vec<_>>(),
        )),
        swatch: slint::Color::from_rgb_u8(color[0], color[1], color[2]),
        has_intensity: field.intensity.is_some(),
        intensity_value: widget_number(field.intensity.unwrap_or_default()),
        intensity: field
            .intensity
            .map(|value| value.to_string())
            .unwrap_or_default()
            .into(),
        number_value: field.value.parse().map(widget_number).unwrap_or_default(),
        minimum: widget_number(range[0]),
        maximum: widget_number(range[1]),
        value: field.value.into(),
    }
}

fn widget_number(value: f64) -> f32 {
    value.clamp(-f64::from(f32::MAX), f64::from(f32::MAX)) as f32
}

fn refresh_entity_window(window: &EntityWindow, session: &Session) {
    let kind = if window.get_brush_mode() {
        ClassKind::Solid
    } else {
        ClassKind::Point
    };
    let classes: Vec<EntityClassRow> = session
        .search_entity_classes(&window.get_filter(), Some(kind), 1000)
        .iter()
        .map(|class| EntityClassRow {
            classname: class.name.clone().into(),
            description: class.description.clone().into(),
        })
        .collect();
    window.set_classes(ModelRc::new(VecModel::from(classes)));
    let definition = match session.definitions().class(&window.get_classname()) {
        Some(class) => entity_class_preview(class),
        None => "Custom class. You can place it and edit its raw properties. Load an FGD for property definitions.".into(),
    };
    window.set_definition(definition.into());
}

fn entity_class_preview(class: &source_formats::fgd::Class) -> String {
    let mut text = format!(
        "{} ({})\n\n{}\n\nProperties\n",
        class.name,
        class.kind.label(),
        class.description
    );
    for property in &class.properties {
        text += &format!(
            "\n{} — {} ({}){}\n",
            property.label,
            property.name,
            property.kind,
            if property.read_only {
                " [read-only]"
            } else {
                ""
            }
        );
        if let Some(default) = property.default_value() {
            text += &format!("Default: {default}\n");
        }
        if !property.description.is_empty() {
            text += &format!("{}\n", property.description);
        }
        for choice in &property.choices {
            text += &format!(
                "  {}: {}{}\n",
                choice.value,
                choice.label,
                if choice.selected { " [default]" } else { "" }
            );
        }
    }
    for (label, members) in [("Inputs", &class.inputs), ("Outputs", &class.outputs)] {
        text += &format!("\n{label}\n");
        for member in members {
            text += &format!(
                "{} ({}) — {}\n",
                member.name, member.kind, member.description
            );
        }
    }
    text += "\nHelpers\n";
    for helper in &class.helpers {
        text += &format!("{}({})\n", helper.name, helper.arguments.join(", "));
    }
    text
}

fn refresh_entity_picker(ui: &SharedUi) {
    let (session, window) = {
        let ui = ui.borrow();
        (
            ui.session.clone(),
            ui.entity_window.as_ref().map(ComponentHandle::clone_strong),
        )
    };
    if let Some(window) = window {
        refresh_entity_window(&window, &lock(&session));
    }
}

fn load_fgd(app: &AppWindow, ui: &SharedUi) {
    let (app, ui) = (app.as_weak(), ui.clone());
    slint::spawn_local(async move {
        let Some(file) = rfd::AsyncFileDialog::new()
            .add_filter("Entity definitions", &["fgd"])
            .pick_file()
            .await
        else {
            return;
        };
        let result = Catalog::load(file.path());
        match result {
            Ok(catalog) => {
                lock(&session_of(&ui)).set_definitions(Actor::User, catalog);
                if let Some(window) = &ui.borrow().entity_window {
                    window.set_message("Entity definitions loaded".into());
                }
                refresh_entity_picker(&ui);
            }
            Err(error) => {
                if let Some(app) = app.upgrade() {
                    app.set_message(error.clone().into());
                }
                if let Some(window) = &ui.borrow().entity_window {
                    window.set_message(error.into());
                }
            }
        }
    })
    .expect("event loop is running");
}

fn show_entity_window(app: &AppWindow, ui: &SharedUi, brush_mode: bool) {
    if ui.borrow().entity_window.is_none() {
        let window = match EntityWindow::new() {
            Ok(window) => window,
            Err(error) => {
                app.set_message(error.to_string().into());
                return;
            }
        };
        window.on_cancel({
            let window = window.as_weak();
            move || {
                let _ = window.unwrap().hide();
            }
        });
        window.on_filter_changed({
            let ui = ui.clone();
            move || refresh_entity_picker(&ui)
        });
        window.on_class_selected({
            let ui = ui.clone();
            move || refresh_entity_picker(&ui)
        });
        window.on_load_fgd({
            let (app, ui) = (app.as_weak(), ui.clone());
            move || load_fgd(&app.unwrap(), &ui)
        });
        window.on_create({
            let (app, ui) = (app.as_weak(), ui.clone());
            move || {
                let Some(window) = ui
                    .borrow()
                    .entity_window
                    .as_ref()
                    .map(ComponentHandle::clone_strong)
                else {
                    return;
                };
                let shared = session_of(&ui);
                let mut session = lock(&shared);
                let result = if window.get_brush_mode() {
                    let ids = session.selected_ids();
                    session.tie_to_entity(
                        Actor::User,
                        &ids,
                        &window.get_classname(),
                        BTreeMap::new(),
                    )
                } else {
                    let position = format!(
                        "{} {} {}",
                        window.get_origin_x(),
                        window.get_origin_y(),
                        window.get_origin_z()
                    );
                    source_formats::fgd::vector(&position, 3)
                        .ok_or_else(|| "position expects three finite coordinates".into())
                        .and_then(|values| {
                            session.create_entity(
                                Actor::User,
                                &window.get_classname(),
                                Some(DVec3::new(values[0], values[1], values[2])),
                                BTreeMap::new(),
                            )
                        })
                };
                match result {
                    Ok(_) => {
                        if let Err(error) = window.hide() {
                            app.unwrap().set_message(error.to_string().into());
                        }
                    }
                    Err(error) => {
                        window.set_message(error.clone().into());
                        app.unwrap().set_message(error.into());
                    }
                }
            }
        });
        ui.borrow_mut().entity_window = Some(window);
    }
    let shared = session_of(ui);
    let session = lock(&shared);
    let Some(window) = ui
        .borrow()
        .entity_window
        .as_ref()
        .map(ComponentHandle::clone_strong)
    else {
        return;
    };
    window.set_brush_mode(brush_mode);
    window.set_filter("".into());
    let kind = if brush_mode {
        ClassKind::Solid
    } else {
        ClassKind::Point
    };
    let selected = single_selected_entity(&session)
        .and_then(|id| session.document().entity(id))
        .and_then(|entity| entity.classname())
        .filter(|name| {
            session
                .definitions()
                .class(name)
                .is_some_and(|class| class.kind == kind)
        });
    window.set_classname(
        selected
            .unwrap_or(if brush_mode { "func_detail" } else { "light" })
            .into(),
    );
    let origin = session.placement_origin();
    window.set_origin_x(origin.x.to_string().into());
    window.set_origin_y(origin.y.to_string().into());
    window.set_origin_z(origin.z.to_string().into());
    window.set_message("".into());
    refresh_entity_window(&window, &session);
    if let Err(error) = window.show() {
        app.set_message(error.to_string().into());
    }
}

fn preview_image(preview: &session::MaterialPreview) -> slint::Image {
    preview
        .image
        .as_ref()
        .map_or_else(slint::Image::default, |image| {
            let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                &image.pixels,
                image.width,
                image.height,
            );
            slint::Image::from_rgba8(buffer)
        })
}

fn refresh_material_preview(app: &AppWindow, session: &mut Session) {
    match session.material_preview(&app.get_material()) {
        Ok(preview) => {
            app.set_has_material_preview(preview.image.is_some());
            app.set_material_preview(preview_image(&preview));
        }
        Err(_) => {
            app.set_has_material_preview(false);
            app.set_material_preview(slint::Image::default());
        }
    }
}

fn refresh_material_window(window: &MaterialWindow, session: &mut Session) {
    window.set_favorite(session.is_material_favorite(&window.get_material()));
    match session.material_preview(&window.get_material()) {
        Ok(preview) => {
            window.set_available(preview.image.is_some());
            window.set_preview(preview_image(&preview));
            window.set_detail(
                format!(
                    "{}\n{} × {}{}",
                    preview.name,
                    preview.width,
                    preview.height,
                    if preview.missing {
                        " · Missing texture"
                    } else if preview.tool {
                        " · Tool material"
                    } else {
                        ""
                    }
                )
                .into(),
            );
        }
        Err(error) => {
            window.set_available(false);
            window.set_preview(slint::Image::default());
            window.set_detail(error.into());
        }
    }
}

fn optional_number(value: SharedString) -> Result<Option<f64>, String> {
    if value.trim().is_empty() {
        return Ok(None);
    }
    let number = value
        .trim()
        .parse::<f64>()
        .map_err(|_| format!("Enter a number: {value}"))?;
    if !number.is_finite() {
        return Err("Texture values must be finite".into());
    }
    Ok(Some(number))
}

fn surface_patch(window: &SurfaceWindow) -> Result<map_core::surface::SurfaceEdit, String> {
    Ok(map_core::surface::SurfaceEdit {
        material: (!window.get_material().trim().is_empty())
            .then(|| window.get_material().trim().to_owned()),
        shift_u: optional_number(window.get_shift_u())?,
        shift_v: optional_number(window.get_shift_v())?,
        scale_u: optional_number(window.get_scale_u())?,
        scale_v: optional_number(window.get_scale_v())?,
        rotation: optional_number(window.get_rotation())?,
        fit: None,
    })
}

fn apply_surface_dialog(
    window: &SurfaceWindow,
    session: &mut Session,
    target: &(Vec<map_core::surface::FaceId>, u64, u64),
    preview: bool,
) -> Result<usize, String> {
    if session.document_generation() != target.1
        || session.document().revision() != target.2
        || session.selected_faces() != target.0
    {
        return Err("Map or face selection changed. Cancel and reopen texture editing.".into());
    }
    session.edit_faces(
        Actor::User,
        &target.0,
        surface_patch(window)?,
        window.get_fit(),
        preview,
    )
}

fn common_face_number(
    document: &map_core::Document,
    faces: &[map_core::surface::FaceId],
    value: fn(&source_formats::vmf::Side) -> f64,
) -> SharedString {
    let values: Vec<_> = faces
        .iter()
        .filter_map(|face| map_core::surface::side(document, *face))
        .map(value)
        .collect();
    values
        .first()
        .filter(|first| values.iter().all(|value| value == *first))
        .map(|value| value.to_string())
        .unwrap_or_default()
        .into()
}

fn refresh_surface_dialog(
    window: &SurfaceWindow,
    session: &mut Session,
    target: &(Vec<map_core::surface::FaceId>, u64, u64),
) {
    let stale = session.document_generation() != target.1
        || session.document().revision() != target.2
        || session.selected_faces() != target.0;
    window.set_stale(stale);
    if stale {
        window.set_message(
            "Map or face selection changed. Cancel and reopen texture editing.".into(),
        );
        return;
    }
    let Some(document) = session.face_preview_document() else {
        window.set_message("No active preview. Preview or apply the current fields.".into());
        return;
    };
    if session.preview_faces() != target.0 {
        window.set_message(
            "A preview targets other faces. Preview these fields to replace it.".into(),
        );
        return;
    }
    let names: Vec<_> = target
        .0
        .iter()
        .filter_map(|face| map_core::surface::side(document, *face))
        .map(|side| side.material.to_ascii_lowercase())
        .collect();
    let material = names
        .first()
        .filter(|first| names.iter().all(|name| name == *first))
        .cloned()
        .unwrap_or_default();
    window.set_material(material.clone().into());
    window.set_shift_u(common_face_number(document, &target.0, |side| {
        side.u_axis.offset
    }));
    window.set_shift_v(common_face_number(document, &target.0, |side| {
        side.v_axis.offset
    }));
    window.set_scale_u(common_face_number(document, &target.0, |side| {
        side.u_axis.scale
    }));
    window.set_scale_v(common_face_number(document, &target.0, |side| {
        side.v_axis.scale
    }));
    window.set_rotation(common_face_number(document, &target.0, |side| {
        side.rotation
    }));
    window.set_preview(
        session
            .material_preview(&material)
            .ok()
            .map(|preview| preview_image(&preview))
            .unwrap_or_default(),
    );
    window.set_message("Shared preview shown in the viewport. Apply to keep it.".into());
}

fn show_surface_window(app: &AppWindow, ui: &SharedUi) {
    if ui.borrow().surface_window.is_none() {
        let window = match SurfaceWindow::new() {
            Ok(window) => window,
            Err(error) => {
                app.set_message(error.to_string().into());
                return;
            }
        };
        window.on_cancel({
            let (window, ui) = (window.as_weak(), ui.clone());
            move || {
                lock(&session_of(&ui)).cancel_face_preview();
                let _ = window.unwrap().hide();
            }
        });
        window.window().on_close_requested({
            let ui = ui.clone();
            move || {
                lock(&session_of(&ui)).cancel_face_preview();
                slint::CloseRequestResponse::HideWindow
            }
        });
        window.on_browse({
            let (app, ui) = (app.as_weak(), ui.clone());
            move || show_material_window(&app.unwrap(), &ui, "@faces")
        });
        window.on_preview_edit({
            let (window, ui) = (window.as_weak(), ui.clone());
            move || {
                let window = window.unwrap();
                let target = ui.borrow().surface_target.clone();
                if let Some(target) = target {
                    let result =
                        apply_surface_dialog(&window, &mut lock(&session_of(&ui)), &target, true);
                    window.set_message(match result {
                        Ok(_) => "Preview shown in the viewport. Apply to keep it.".into(),
                        Err(error) => error.into(),
                    });
                }
            }
        });
        window.on_apply({
            let (window, ui) = (window.as_weak(), ui.clone());
            move || {
                let window = window.unwrap();
                let target = ui.borrow().surface_target.clone();
                if let Some(target) = target {
                    match apply_surface_dialog(&window, &mut lock(&session_of(&ui)), &target, false)
                    {
                        Ok(_) => {
                            let _ = window.hide();
                        }
                        Err(error) => window.set_message(error.into()),
                    }
                }
            }
        });
        ui.borrow_mut().surface_window = Some(window);
    }
    let shared = session_of(ui);
    let mut session = lock(&shared);
    let faces = session.selected_faces();
    if faces.is_empty() {
        app.set_message("Select faces in the viewport first.".into());
        return;
    }
    session.cancel_face_preview();
    let window = ui.borrow().surface_window.as_ref().unwrap().clone_strong();
    window.set_selection_label(
        format!(
            "Textures on {} selected face{}",
            faces.len(),
            if faces.len() == 1 { "" } else { "s" }
        )
        .into(),
    );
    let names: Vec<_> = faces
        .iter()
        .filter_map(|face| map_core::surface::side(session.document(), *face))
        .map(|side| side.material.to_ascii_lowercase())
        .collect();
    let material = names
        .first()
        .filter(|first| names.iter().all(|name| name == *first))
        .cloned()
        .unwrap_or_default();
    window.set_material(material.clone().into());
    window.set_preview(
        session
            .material_preview(&material)
            .ok()
            .map(|preview| preview_image(&preview))
            .unwrap_or_default(),
    );
    window.set_shift_u(common_face_number(session.document(), &faces, |side| {
        side.u_axis.offset
    }));
    window.set_shift_v(common_face_number(session.document(), &faces, |side| {
        side.v_axis.offset
    }));
    window.set_scale_u(common_face_number(session.document(), &faces, |side| {
        side.u_axis.scale
    }));
    window.set_scale_v(common_face_number(session.document(), &faces, |side| {
        side.v_axis.scale
    }));
    window.set_rotation(common_face_number(session.document(), &faces, |side| {
        side.rotation
    }));
    window.set_fit(false);
    window.set_stale(false);
    window.set_message("Preview changes before applying, or Cancel to discard.".into());
    ui.borrow_mut().surface_target = Some((
        faces,
        session.document_generation(),
        session.document().revision(),
    ));
    drop(session);
    if let Err(error) = window.show() {
        app.set_message(error.to_string().into());
    }
}

fn refresh_material_catalog(window: &MaterialWindow, session: &mut Session) {
    let catalog = match session.material_catalog(
        &window.get_filter(),
        &window.get_group(),
        window.get_offset().max(0) as usize,
        36,
    ) {
        Ok(catalog) => catalog,
        Err(error) => {
            window.set_detail(error.into());
            return;
        }
    };
    let groups = catalog
        .groups
        .into_iter()
        .map(|group| MaterialGroup {
            id: group.id.into(),
            label: format!("{} ({})", group.label, group.count).into(),
        })
        .collect::<Vec<_>>();
    window.set_groups(ModelRc::new(VecModel::from(groups)));
    window.set_total(catalog.total as i32);
    window.set_catalog_label(if catalog.total == 0 {
        "No matching materials".into()
    } else {
        format!(
            "{}–{} of {} materials",
            catalog.offset + 1,
            catalog.offset + catalog.entries.len(),
            catalog.total
        )
        .into()
    });
    let tiles = catalog
        .entries
        .into_iter()
        .map(|entry| MaterialTile {
            preview: session
                .material_preview(&entry.name)
                .ok()
                .map(|preview| preview_image(&preview))
                .unwrap_or_default(),
            name: entry.name.into(),
            favorite: entry.favorite,
        })
        .collect::<Vec<_>>();
    let rows = tiles
        .chunks(3)
        .map(|tiles| MaterialTileRow {
            tiles: ModelRc::new(VecModel::from(tiles.to_vec())),
        })
        .collect::<Vec<_>>();
    window.set_rows(ModelRc::new(VecModel::from(rows)));
    window.set_favorite(session.is_material_favorite(&window.get_material()));
}

fn show_material_window(app: &AppWindow, ui: &SharedUi, key: &str) {
    if ui.borrow().material_window.is_none() {
        let window = match MaterialWindow::new() {
            Ok(window) => window,
            Err(error) => {
                app.set_message(error.to_string().into());
                return;
            }
        };
        window.on_search({
            let (window, ui) = (window.as_weak(), ui.clone());
            move || {
                let window = window.unwrap();
                refresh_material_catalog(&window, &mut lock(&session_of(&ui)));
            }
        });
        window.on_cancel({
            let window = window.as_weak();
            move || {
                let _ = window.unwrap().hide();
            }
        });
        window.on_toggle_favorite({
            let (window, ui) = (window.as_weak(), ui.clone());
            move || {
                let window = window.unwrap();
                let shared = session_of(&ui);
                let mut session = lock(&shared);
                if let Err(error) =
                    session.favorite_material(&window.get_material(), !window.get_favorite())
                {
                    window.set_detail(error.into());
                }
                refresh_material_catalog(&window, &mut session);
            }
        });
        window.on_preview_material({
            let (window, ui) = (window.as_weak(), ui.clone());
            move || refresh_material_window(&window.unwrap(), &mut lock(&session_of(&ui)))
        });
        window.on_use_material({
            let (window, app, ui) = (window.as_weak(), app.as_weak(), ui.clone());
            move || {
                let window = window.unwrap();
                let app = app.unwrap();
                let material = window.get_material();
                let target = ui.borrow().material_target.clone();
                let shared = session_of(&ui);
                let mut session = lock(&shared);
                if let Some((entity, key, generation)) = target {
                    if session.document_generation() != generation {
                        window.set_detail("Map changed. Open this picker again.".into());
                        return;
                    }
                    let result = session.set_entity_properties(
                        Actor::User,
                        entity,
                        &[(key, Some(material.to_string()))],
                    );
                    if let Err(error) = result {
                        window.set_detail(error.into());
                        return;
                    }
                } else if ui.borrow().material_for_clip {
                    app.set_clip_material(material);
                    session.cancel_clip_preview();
                    refresh_clip(&app, &session);
                } else if ui.borrow().material_for_faces {
                    if let Some(surface) = &ui.borrow().surface_window {
                        surface.set_material(material.clone());
                        surface.set_preview(
                            session
                                .material_preview(&material)
                                .ok()
                                .map(|preview| preview_image(&preview))
                                .unwrap_or_default(),
                        );
                    }
                } else {
                    app.set_material(material);
                    refresh_material_preview(&app, &mut session);
                }
                drop(session);
                let _ = window.hide();
            }
        });
        ui.borrow_mut().material_window = Some(window);
    }
    let shared = session_of(ui);
    let mut session = lock(&shared);
    ui.borrow_mut().material_for_faces = key == "@faces";
    ui.borrow_mut().material_for_clip = key == "@clip";
    let target = if key.is_empty() || key == "@faces" || key == "@clip" {
        None
    } else {
        single_selected_entity(&session)
            .map(|id| (id, key.to_owned(), session.document_generation()))
    };
    let material = if key == "@clip" {
        app.get_clip_material().to_string()
    } else if key == "@faces" {
        ui.borrow()
            .surface_window
            .as_ref()
            .map(|window| window.get_material().to_string())
            .unwrap_or_default()
    } else {
        target
            .as_ref()
            .and_then(|(id, key, _)| {
                session
                    .entity_inspector(*id)
                    .ok()?
                    .properties
                    .into_iter()
                    .find(|field| field.key == *key)
                    .map(|field| field.value)
            })
            .unwrap_or_else(|| app.get_material().to_string())
    };
    ui.borrow_mut().material_target = target;
    let window = ui.borrow().material_window.as_ref().unwrap().clone_strong();
    window.set_filter("".into());
    window.set_material(material.into());
    window.set_group("all".into());
    window.set_offset(0);
    refresh_material_catalog(&window, &mut session);
    refresh_material_window(&window, &mut session);
    drop(session);
    if let Err(error) = window.show() {
        app.set_message(error.to_string().into());
    }
}

fn show_color_window(app: &AppWindow, ui: &SharedUi, key: &str) {
    if ui.borrow().color_window.is_none() {
        let window = match ColorWindow::new() {
            Ok(window) => window,
            Err(error) => {
                app.set_message(error.to_string().into());
                return;
            }
        };
        window.on_cancel({
            let window = window.as_weak();
            move || {
                let _ = window.unwrap().hide();
            }
        });
        window.on_apply({
            let (window, ui) = (window.as_weak(), ui.clone());
            move || {
                let window = window.unwrap();
                let Some((id, key, generation)) = ui.borrow().color_target.clone() else {
                    return;
                };
                let intensity = if window.get_has_intensity() {
                    match window.get_intensity().parse::<f64>() {
                        Ok(value) => Some(value),
                        Err(_) => {
                            window.set_message("Enter a valid brightness.".into());
                            return;
                        }
                    }
                } else {
                    None
                };
                let color = [
                    window.get_red() as u8,
                    window.get_green() as u8,
                    window.get_blue() as u8,
                ];
                let shared = session_of(&ui);
                let mut session = lock(&shared);
                if session.document_generation() != generation {
                    window.set_message("Map changed. Open this picker again.".into());
                    return;
                }
                let result = session.set_entity_color(Actor::User, id, &key, color, intensity);
                match result {
                    Ok(()) => {
                        let _ = window.hide();
                    }
                    Err(error) => window.set_message(error.into()),
                }
            }
        });
        ui.borrow_mut().color_window = Some(window);
    }
    let shared = session_of(ui);
    let session = lock(&shared);
    let Some(id) = single_selected_entity(&session) else {
        return;
    };
    let Ok(inspector) = session.entity_inspector(id) else {
        return;
    };
    let Some(field) = inspector
        .properties
        .into_iter()
        .find(|field| field.key == key)
    else {
        return;
    };
    let Some(color) = field.color else {
        return;
    };
    let generation = session.document_generation();
    drop(session);
    ui.borrow_mut().color_target = Some((id, key.into(), generation));
    let window = ui.borrow().color_window.as_ref().unwrap().clone_strong();
    window.set_field_label(field.label.into());
    window.set_red(i32::from(color[0]));
    window.set_green(i32::from(color[1]));
    window.set_blue(i32::from(color[2]));
    window.set_has_intensity(field.intensity.is_some());
    window.set_intensity(field.intensity.unwrap_or_default().to_string().into());
    window.set_brightness(widget_number(field.intensity.unwrap_or_default()));
    window.set_message("".into());
    if let Err(error) = window.show() {
        app.set_message(error.to_string().into());
    }
}

fn refresh_panel(app: &AppWindow, session: &mut Session) {
    refresh_clip(app, session);
    let doc = session.document();
    app.set_map_title(session.title().into());
    app.set_can_undo(doc.undo_labels().next().is_some());
    app.set_can_redo(doc.redo_labels().next().is_some());
    app.set_compiling(session.compiling());
    app.set_launching(session.launching());
    app.set_game_running(session.game_running());
    app.set_has_selection(!session.selection().is_empty());
    app.set_face_mode(session.face_mode());
    app.set_has_faces(!session.selected_faces().is_empty());
    app.set_selection_pivot(
        session
            .selection_pivot(&session.selected_ids())
            .map(|pivot| format!("X {:.3} · Y {:.3} · Z {:.3}", pivot.x, pivot.y, pivot.z))
            .unwrap_or_default()
            .into(),
    );
    let (title, detail, properties) = selection_summary(session);
    let selection_changed =
        app.get_selection_title() != title || app.get_selection_detail() != detail;
    if selection_changed {
        let faces = session.selected_faces();
        let material = if let Some(face) = faces.first() {
            map_core::surface::side(doc, *face)
        } else {
            match session.selected_ids().as_slice() {
                [ObjectId::Solid(id)] => doc.solid(*id).and_then(|solid| solid.sides.first()),
                [ObjectId::Entity(id)] => doc
                    .entity(*id)
                    .and_then(|entity| entity.solids.first())
                    .and_then(|solid| solid.sides.first()),
                _ => None,
            }
        };
        if let Some(side) = material {
            app.set_material(side.material.to_ascii_lowercase().into());
        }
    }
    app.set_selection_title(title.into());
    app.set_selection_detail(detail.into());
    app.set_entity_selected(properties.is_some());
    app.set_brush_entity_selected(
        single_selected_entity(session)
            .and_then(|id| doc.entity(id))
            .is_some_and(|entity| !entity.solids.is_empty()),
    );
    app.set_can_tie(
        !session.selection().is_empty()
            && session
                .selection()
                .iter()
                .all(|id| matches!(id, ObjectId::Solid(_))),
    );
    app.set_can_apply_material(
        !session.selected_faces().is_empty()
            || session.selection().iter().any(|id| match id {
                ObjectId::Solid(_) => true,
                ObjectId::Entity(id) => doc
                    .entity(*id)
                    .is_some_and(|entity| !entity.solids.is_empty()),
            }),
    );
    app.set_entity_properties(ModelRc::new(VecModel::from(properties.unwrap_or_default())));
    let activity: Vec<Activity> = session
        .recent_log()
        .take(ACTIVITY_SHOWN)
        .map(|e| Activity {
            actor: e.actor.label().into(),
            text: e.text.clone().into(),
        })
        .collect();
    app.set_activity(ModelRc::new(VecModel::from(activity)));
    if app.get_can_apply_material() {
        refresh_material_preview(app, session);
    }
}

fn report(app: &AppWindow, result: Result<String, String>) {
    match result {
        Ok(message) | Err(message) => app.set_message(message.into()),
    }
}

/// Asks before throwing away unsaved changes, then runs `then`.
fn confirm_discard(ui: &SharedUi, then: impl FnOnce() + 'static) {
    let dirty = lock(&session_of(ui)).document().is_dirty();
    if !dirty {
        then();
        return;
    }
    let dialog = rfd::AsyncMessageDialog::new()
        .set_title("Unsaved changes")
        .set_description("The map has unsaved changes. Discard them?")
        .set_buttons(rfd::MessageButtons::YesNoCancel);
    slint::spawn_local(async move {
        if dialog.show().await == rfd::MessageDialogResult::Yes {
            then();
        }
    })
    .expect("event loop is running");
}

fn open_dialog(app: &AppWindow, ui: &SharedUi) {
    let start_dir = {
        let shared = session_of(ui);
        let session = lock(&shared);
        session
            .path()
            .and_then(|p| p.parent().map(Path::to_owned))
            .or_else(|| steam::sample_maps_dir(&session.game().game_dir.join("gameinfo.txt")))
    };
    let mut dialog = rfd::AsyncFileDialog::new()
        .set_title("Open map")
        .add_filter("Valve Map File", &["vmf"]);
    if let Some(dir) = start_dir {
        dialog = dialog.set_directory(dir);
    }
    let (app, ui) = (app.as_weak(), ui.clone());
    slint::spawn_local(async move {
        let Some(file) = dialog.pick_file().await else {
            return;
        };
        let path = file.path().to_owned();
        let ui2 = ui.clone();
        confirm_discard(&ui, move || {
            let result = lock(&session_of(&ui2)).open(Actor::User, &path, true);
            if let Some(app) = app.upgrade() {
                report(&app, result);
            }
        });
    })
    .expect("event loop is running");
}

fn save(app: &AppWindow, ui: &SharedUi) {
    let has_path = lock(&session_of(ui)).path().is_some();
    if has_path {
        let result = lock(&session_of(ui)).save(Actor::User, None);
        report(app, result.map(|p| format!("Saved {}", p.display())));
        return;
    }
    let dialog = rfd::AsyncFileDialog::new()
        .set_title("Save map")
        .set_file_name("untitled.vmf")
        .add_filter("Valve Map File", &["vmf"]);
    let (app, ui) = (app.as_weak(), ui.clone());
    slint::spawn_local(async move {
        let Some(file) = dialog.save_file().await else {
            return;
        };
        let result = lock(&session_of(&ui)).save(Actor::User, Some(file.path()));
        if let Some(app) = app.upgrade() {
            report(&app, result.map(|p| format!("Saved {}", p.display())));
        }
    })
    .expect("event loop is running");
}

fn start_compile(app: &AppWindow, session: SharedSession, path: Option<&Path>, play: bool) {
    let busy = {
        let session = lock(&session);
        session.game_running() || session.launching()
    };
    if play && busy {
        app.set_message(
            "CS:S is already launching or running; close it before running another map".into(),
        );
        return;
    }
    let request = match lock(&session).begin_compile(Actor::User, path) {
        Ok(request) => request,
        Err(message) => {
            app.set_message(message.into());
            return;
        }
    };
    let worker_session = session.clone();
    let spawned = std::thread::Builder::new()
        .name("map-compile".into())
        .spawn(move || {
            let result = request.run();
            lock(&worker_session).finish_compile(Actor::User, &result);
            if play && result.as_ref().is_ok_and(|r| r.output.bsp.is_some()) {
                let _ = mcp::launch(&worker_session, Actor::User);
            }
        });
    if let Err(error) = spawned {
        lock(&session).finish_compile(Actor::User, &Err(error.to_string()));
    }
}

fn compile_map(app: &AppWindow, ui: &SharedUi, play: bool) {
    let session = session_of(ui);
    if lock(&session).path().is_some() {
        start_compile(app, session, None, play);
        return;
    }
    let dialog = rfd::AsyncFileDialog::new()
        .set_title("Compile map")
        .set_file_name("untitled.bsp")
        .add_filter("Source map", &["bsp"]);
    let app = app.as_weak();
    slint::spawn_local(async move {
        let Some(file) = dialog.save_file().await else {
            return;
        };
        if let Some(app) = app.upgrade() {
            start_compile(&app, session, Some(file.path()), play);
        }
    })
    .expect("event loop is running");
}

fn show_game_log(app: &AppWindow, ui: &SharedUi) {
    let session = session_of(ui);
    let log = match lock(&session).game_log(game::DEFAULT_LOG_LINES) {
        Ok(log) => log,
        Err(error) => {
            app.set_message(error.into());
            return;
        }
    };
    let mut ui = ui.borrow_mut();
    if ui.game_log_window.is_none() {
        match GameLogWindow::new() {
            Ok(window) => {
                window.on_cancel({
                    let window = window.as_weak();
                    move || {
                        let _ = window.unwrap().hide();
                    }
                });
                ui.game_log_window = Some(window);
            }
            Err(error) => {
                app.set_message(error.to_string().into());
                return;
            }
        }
    }
    if let Some(window) = &ui.game_log_window {
        window.set_content(log.text().into());
        if let Err(error) = window.show() {
            app.set_message(error.to_string().into());
        }
    }
}

/// Edits the selection as the user and shows any error in the status bar.
fn edit_selection(
    app: &AppWindow,
    ui: &SharedUi,
    label: impl Fn(usize) -> String,
    f: impl FnOnce(&mut map_core::Transaction, &[ObjectId]) -> Result<(), map_core::EditError>,
) {
    let shared = session_of(ui);
    let mut session = lock(&shared);
    let ids = session.selected_ids();
    if ids.is_empty() {
        app.set_message("Nothing selected".into());
        return;
    }
    if let Err(message) = session.edit(Actor::User, &label(ids.len()), |tx| f(tx, &ids)) {
        app.set_message(message.into());
    }
}

fn create_block(app: &AppWindow, ui: &SharedUi) {
    let ui = ui.borrow();
    let camera = ui.viewport.camera;
    let mut session = lock(&ui.session);
    if camera.projection != Projection::Perspective {
        let drawing = !session.spatial().draw_block;
        if let Err(message) = session.set_spatial(
            Actor::User,
            spatial::Update {
                draw_block: Some(drawing),
                ..spatial::Update::default()
            },
        ) {
            app.set_message(message.into());
        }
        return;
    }
    let (origin, direction) = camera.ray(0.0, 0.0, 1.0);
    let ray = Ray {
        origin: origin.as_dvec3(),
        direction: direction.as_dvec3(),
    };
    let half = BLOCK_SIZE / 2.0;
    let (mins, maxs) = match session.pick(&ray) {
        Some(hit) => {
            let base = session.spatial().snap_point(hit.point);
            (
                base - DVec3::new(half, half, 0.0),
                base + DVec3::new(half, half, BLOCK_SIZE),
            )
        }
        None => {
            let p = ray.origin + ray.direction * 384.0;
            let center = session.spatial().snap_point(p);
            (center - DVec3::splat(half), center + DVec3::splat(half))
        }
    };
    let material = app.get_material().to_string();
    let result = session.create_block(Actor::User, mins, maxs, &material, true);
    if let Err(message) = result {
        app.set_message(message.into());
    }
}

fn single_selected_entity(session: &Session) -> Option<u32> {
    match session.selected_ids()[..] {
        [ObjectId::Entity(n)] => Some(n),
        _ => None,
    }
}

/// Keyboard commands; returns whether the key was handled.
fn command_key(app: &AppWindow, ui: &SharedUi, text: &str, control: bool, shift: bool) -> bool {
    let key = text.to_lowercase();
    if control {
        match key.as_str() {
            "z" if shift => drop(lock(&session_of(ui)).redo(Actor::User)),
            "z" => drop(lock(&session_of(ui)).undo(Actor::User)),
            "y" => drop(lock(&session_of(ui)).redo(Actor::User)),
            "s" => save(app, ui),
            "n" => {
                let ui2 = ui.clone();
                confirm_discard(ui, move || {
                    drop(lock(&session_of(&ui2)).new_map(Actor::User, true));
                });
            }
            "d" => {
                let shared = session_of(ui);
                let mut session = lock(&shared);
                let ids = session.selected_ids();
                let grid = session.spatial().grid_size;
                let offset = DVec3::new(grid, grid, 0.0);
                let result = session
                    .edit(
                        Actor::User,
                        &format!("duplicate {} objects", ids.len()),
                        |tx| edit::duplicate(tx, &ids, offset),
                    )
                    .and_then(|copies| session.select(&copies, SelectMode::Replace));
                if let Err(message) = result {
                    app.set_message(message.into());
                }
            }
            _ => return false,
        }
        return true;
    }
    if key == key_text(Key::Delete) || key == key_text(Key::Backspace) {
        edit_selection(app, ui, |n| format!("delete {n} objects"), edit::delete);
        return true;
    }
    if key == key_text(Key::Escape) {
        {
            let mut ui = ui.borrow_mut();
            let shared = ui.session.clone();
            let mut session = lock(&shared);
            if ui.interaction.active()
                || session.transform_preview().is_some()
                || session.transform_drag().is_some()
                || session.area_preview().is_some()
            {
                ui.interaction.cancel(&mut session);
                refresh_interaction(app, &session);
                return true;
            }
        }
        ui.borrow_mut().clip_drag = None;
        let cancelled = ui.borrow_mut().brush_drag.take().is_some();
        let shared = session_of(ui);
        let mut session = lock(&shared);
        if session.clip_tool() || session.clip_preview().is_some() {
            cancel_clip(app, &mut session);
            return true;
        }
        if cancelled || session.block_preview().is_some() {
            session.cancel_block_preview();
            return true;
        }
        if session.spatial().draw_block {
            drop(session.set_spatial(
                Actor::User,
                spatial::Update {
                    draw_block: Some(false),
                    ..spatial::Update::default()
                },
            ));
            return true;
        }
        if session.has_face_preview() {
            session.cancel_face_preview();
        } else if session.face_mode() {
            drop(session.select_faces(&[], SelectMode::Replace));
        } else {
            drop(session.select(&[], SelectMode::Replace));
        }
        return true;
    }
    if key == "f" {
        frame_selection(app, ui);
        return true;
    }
    let camera = ui.borrow().viewport.camera;
    let (forward, right, normal) = if let Some((_, _, normal)) = camera.projection.axes() {
        let mut axis = DVec3::ZERO;
        axis[normal] = 1.0;
        (camera.up().as_dvec3(), camera.right().as_dvec3(), axis)
    } else {
        let (forward, right) = nudge_axes(camera.forward());
        (forward, right, DVec3::Z)
    };
    let direction = [
        (Key::UpArrow, forward),
        (Key::DownArrow, -forward),
        (Key::RightArrow, right),
        (Key::LeftArrow, -right),
        (Key::PageUp, normal),
        (Key::PageDown, -normal),
    ]
    .into_iter()
    .find(|(k, _)| key == key_text(*k))
    .map(|(_, d)| d);
    if let Some(direction) = direction {
        let grid = lock(&session_of(ui)).spatial().grid_size;
        let step = if shift { grid.min(1.0) } else { grid };
        edit_selection(
            app,
            ui,
            |n| format!("nudge {n} objects"),
            |tx, ids| edit::translate(tx, ids, direction * step, true),
        );
        return true;
    }
    false
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse()?;
    let ctx = GpuContext::new()?;
    let session = Arc::new(Mutex::new(Session::new(mount_game(args.gameinfo))));
    let _shutdown = ShutdownGuard(session.clone());
    let fgd = args
        .fgd
        .or_else(|| steam::find_css_fgd(lock(&session).game()));
    if let Some(path) = fgd {
        match Catalog::load(&path) {
            Ok(catalog) => lock(&session).set_definitions(Actor::User, catalog),
            Err(error) => lock(&session).report_definitions_error(error),
        }
    } else {
        lock(&session)
            .report_definitions_error("CS:S SDK FGD not found; use --fgd or Load FGD".into());
    }
    let open_message = args
        .map
        .as_ref()
        .map(|map| lock(&session).open(Actor::User, map, true));
    let server = EditorServer::new(session.clone(), ctx.clone());

    if args.headless {
        if let Some(Err(message)) = &open_message {
            eprintln!("{message}");
        }
        return Ok(mcp::serve_stdio(server)?);
    }

    let mcp_status = match args.mcp_port {
        Some(port) => {
            match mcp::spawn_http(server, SocketAddr::from((Ipv4Addr::LOCALHOST, port))) {
                Ok(addr) => format!("MCP http://{addr}/mcp"),
                Err(message) => message,
            }
        }
        None => "MCP off".into(),
    };

    slint::BackendSelector::new()
        .require_wgpu_30(slint::wgpu_30::WGPUConfiguration::Manual {
            instance: ctx.instance.clone(),
            adapter: ctx.adapter.clone(),
            device: ctx.device.clone(),
            queue: ctx.queue.clone(),
        })
        .select()?;

    let app = AppWindow::new()?;
    app.window().on_close_requested(|| {
        if let Err(error) = slint::quit_event_loop() {
            eprintln!("could not close editor: {error}");
        }
        slint::CloseRequestResponse::HideWindow
    });
    app.set_mcp_status(mcp_status.into());
    app.set_stats(ctx.adapter.get_info().name.into());
    {
        let session = lock(&session);
        let message = match (&open_message, session.game_error()) {
            (Some(result), _) => result.clone().unwrap_or_else(|e| e),
            (None, Some(error)) => error.to_owned(),
            (None, None) => format!("Game: {}", session.game().game_dir.display()),
        };
        app.set_message(message.into());
    }

    let camera = lock(&session).camera();
    let ui: SharedUi = Rc::new(RefCell::new(Ui {
        viewport: Viewport::new(&ctx, camera),
        ctx,
        session,
        seen_scene: 0,
        seen_camera: 0,
        seen_view: 0,
        brush_drag: None,
        clip_drag: None,
        interaction: interaction::Interaction::default(),
        seen_log: 0,
        game_log_window: None,
        last_log_refresh: Instant::now(),
        entity_window: None,
        color_window: None,
        color_target: None,
        material_window: None,
        material_target: None,
        surface_window: None,
        surface_target: None,
        material_for_faces: false,
        material_for_clip: false,
        seen_definitions: 0,
    }));

    app.on_open_map({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || open_dialog(&app.unwrap(), &ui)
    });
    app.on_new_map({
        let ui = ui.clone();
        move || {
            let ui2 = ui.clone();
            confirm_discard(&ui, move || {
                drop(lock(&session_of(&ui2)).new_map(Actor::User, true))
            });
        }
    });
    app.on_save_map({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || save(&app.unwrap(), &ui)
    });
    app.on_compile_map({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || compile_map(&app.unwrap(), &ui, false)
    });
    app.on_run_map({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || compile_map(&app.unwrap(), &ui, true)
    });
    app.on_game_log({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || show_game_log(&app.unwrap(), &ui)
    });
    app.on_stop_game({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            if let Err(error) = lock(&session_of(&ui)).stop_game(Actor::User) {
                app.unwrap().set_message(error.into());
            }
        }
    });
    app.on_undo({
        let ui = ui.clone();
        move || drop(lock(&session_of(&ui)).undo(Actor::User))
    });
    app.on_redo({
        let ui = ui.clone();
        move || drop(lock(&session_of(&ui)).redo(Actor::User))
    });
    app.on_create_block({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || create_block(&app.unwrap(), &ui)
    });
    app.on_projection_changed({
        let ui = ui.clone();
        move |index| {
            let projection = [
                Projection::Perspective,
                Projection::Top,
                Projection::Front,
                Projection::Side,
            ][index.clamp(0, 3) as usize];
            lock(&session_of(&ui)).set_projection(Actor::User, projection);
        }
    });
    app.on_spatial_changed({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || spatial_changed(&app.unwrap(), &ui)
    });
    app.on_frame_selection({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || frame_selection(&app.unwrap(), &ui)
    });
    app.on_apply_transform({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || apply_transform(&app.unwrap(), &ui)
    });
    bind_clip_controls(&app, session_of(&ui));
    bind_manipulation_controls(&app, session_of(&ui));
    app.on_place_entity({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || show_entity_window(&app.unwrap(), &ui, false)
    });
    app.on_tie_entity({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || show_entity_window(&app.unwrap(), &ui, true)
    });
    app.on_move_to_world({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            let shared = session_of(&ui);
            let mut session = lock(&shared);
            if let Some(entity) = single_selected_entity(&session)
                && let Err(error) = session.move_to_world(Actor::User, entity)
            {
                app.unwrap().set_message(error.into());
            }
        }
    });
    app.on_delete_selection({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            edit_selection(
                &app.unwrap(),
                &ui,
                |n| format!("delete {n} objects"),
                edit::delete,
            )
        }
    });
    app.on_selection_mode_changed({
        let ui = ui.clone();
        move |mode| lock(&session_of(&ui)).set_face_mode(mode == 1)
    });
    app.on_edit_face_textures({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || show_surface_window(&app.unwrap(), &ui)
    });
    app.on_apply_material({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            let app = app.unwrap();
            let shared = session_of(&ui);
            let mut session = lock(&shared);
            let targets = session.material_targets();
            if let Err(error) =
                session.apply_material(Actor::User, &targets, app.get_material().trim())
            {
                app.set_message(error.into());
            }
        }
    });
    app.on_browse_material({
        let (app, ui) = (app.as_weak(), ui.clone());
        move |key| show_material_window(&app.unwrap(), &ui, &key)
    });
    app.on_material_changed({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || refresh_material_preview(&app.unwrap(), &mut lock(&session_of(&ui)))
    });
    app.on_pick_color({
        let (app, ui) = (app.as_weak(), ui.clone());
        move |key| show_color_window(&app.unwrap(), &ui, &key)
    });
    app.on_property_component_edited({
        let (app, ui) = (app.as_weak(), ui.clone());
        move |key, index, value| {
            let shared = session_of(&ui);
            let mut session = lock(&shared);
            let Some(id) = single_selected_entity(&session) else {
                return;
            };
            if let Err(error) =
                session.set_entity_component(Actor::User, id, &key, index as usize, &value)
            {
                let app = app.unwrap();
                refresh_panel(&app, &mut session);
                app.set_message(error.into());
            }
        }
    });
    app.on_property_edited({
        let (app, ui) = (app.as_weak(), ui.clone());
        move |key, value| {
            let key = key.trim().to_owned();
            if key.is_empty() {
                return;
            }
            let shared = session_of(&ui);
            let mut session = lock(&shared);
            let Some(entity) = single_selected_entity(&session) else {
                return;
            };
            let changes = [(key.clone(), Some(value.to_string()))];
            if let Err(message) = session.set_entity_properties(Actor::User, entity, &changes) {
                let app = app.unwrap();
                refresh_panel(&app, &mut session);
                app.set_message(message.into());
            }
        }
    });
    app.on_property_removed({
        let (app, ui) = (app.as_weak(), ui.clone());
        move |key| {
            let shared = session_of(&ui);
            let mut session = lock(&shared);
            let Some(entity) = single_selected_entity(&session) else {
                return;
            };
            let changes = [(key.to_string(), None)];
            if let Err(message) = session.set_entity_properties(Actor::User, entity, &changes) {
                let app = app.unwrap();
                refresh_panel(&app, &mut session);
                app.set_message(message.into());
            }
        }
    });
    app.on_flag_toggled({
        let (app, ui) = (app.as_weak(), ui.clone());
        move |key, flag, checked| {
            let shared = session_of(&ui);
            let mut session = lock(&shared);
            let Some(id) = single_selected_entity(&session) else {
                return;
            };
            let entity = session.document().entity(id).unwrap();
            let default = entity
                .classname()
                .and_then(|name| session.definitions().class(name))
                .and_then(|class| class.property(&key))
                .and_then(|property| property.default_value());
            let current = entity
                .properties
                .get(&key)
                .or(default.as_deref())
                .unwrap_or("0");
            let result = current
                .parse::<u64>()
                .and_then(|mask| {
                    flag.parse::<u64>()
                        .map(|flag| if checked { mask | flag } else { mask & !flag })
                })
                .map_err(|error| error.to_string())
                .and_then(|mask| {
                    session.set_entity_properties(
                        Actor::User,
                        id,
                        &[(key.to_string(), Some(mask.to_string()))],
                    )
                });
            if let Err(error) = result {
                let app = app.unwrap();
                refresh_panel(&app, &mut session);
                app.set_message(error.into());
            }
        }
    });
    app.on_options_changed({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            let app = app.unwrap();
            let shared = session_of(&ui);
            let mut session = lock(&shared);
            let mut view = session.view();
            view.tool_textures = app.get_show_tools();
            view.leak_path = app.get_show_leak();
            session.set_view(view);
        }
    });
    app.on_key({
        let (app, ui) = (app.as_weak(), ui.clone());
        move |text, down, control, shift| {
            if down && command_key(&app.unwrap(), &ui, &text, control, shift) {
                return true;
            }
            ui.borrow_mut().viewport.key(&text, down)
        }
    });
    app.on_mouse_button({
        let (app, ui) = (app.as_weak(), ui.clone());
        move |button, down, x, y, control| {
            let app = app.unwrap();
            let viewport_size = (app.get_viewport_width(), app.get_viewport_height());
            let mut ui = ui.borrow_mut();
            if button != 0 && down && ui.interaction.active() {
                let shared = ui.session.clone();
                ui.interaction.cancel(&mut lock(&shared));
                refresh_interaction(&app, &lock(&shared));
            }
            if button != 0 && down && ui.clip_drag.take().is_some() {
                let mut session = lock(&ui.session);
                session.cancel_clip_preview();
                refresh_clip(&app, &session);
            }
            if button == 0 {
                let shared = ui.session.clone();
                let mut session = lock(&shared);
                if down
                    && session.clip_tool()
                    && ui.viewport.camera.projection != Projection::Perspective
                {
                    if let Some(start) = plane_coordinates(
                        ui.viewport.camera,
                        (x, y),
                        viewport_size,
                        session.spatial().elevation,
                    ) {
                        session.cancel_clip_preview();
                        ui.clip_drag = Some(BrushDrag {
                            start,
                            epoch: session.clip_epoch(),
                            generation: session.document_generation(),
                            revision: session.document().revision(),
                            projection: ui.viewport.camera.projection,
                        });
                    }
                    return;
                }
                if !down && let Some(drag) = ui.clip_drag.take() {
                    let result = plane_coordinates(
                        ui.viewport.camera,
                        (x, y),
                        viewport_size,
                        session.spatial().elevation,
                    )
                    .ok_or("Viewport has no size".to_owned())
                    .and_then(|end| clip_drag_plane(&session, drag, end))
                    .and_then(|plane| preview_clip(&app, &mut session, plane));
                    if let Err(error) = result {
                        session.cancel_clip_preview();
                        refresh_clip(&app, &session);
                        app.set_message(error.into());
                    }
                    return;
                }
                if session.clip_tool() {
                    return;
                }
                if down
                    && session.spatial().draw_block
                    && ui.viewport.camera.projection != Projection::Perspective
                {
                    if let Some(start) = plane_coordinates(
                        ui.viewport.camera,
                        (x, y),
                        viewport_size,
                        session.spatial().elevation,
                    ) {
                        session.cancel_block_preview();
                        ui.brush_drag = Some(BrushDrag {
                            start,
                            epoch: session.brush_epoch(),
                            generation: session.document_generation(),
                            revision: session.document().revision(),
                            projection: ui.viewport.camera.projection,
                        });
                    }
                    return;
                }
                if !down && let Some(drag) = ui.brush_drag.take() {
                    let result = plane_coordinates(
                        ui.viewport.camera,
                        (x, y),
                        viewport_size,
                        session.spatial().elevation,
                    )
                    .ok_or_else(|| "Viewport has no size".to_string())
                    .and_then(|end| drag_bounds(&session, drag, end))
                    .and_then(|(mins, maxs)| {
                        session.create_block(Actor::User, mins, maxs, &app.get_material(), false)
                    });
                    session.cancel_block_preview();
                    if let Err(message) = result {
                        app.set_message(message.into());
                    }
                    return;
                }
            }
            if button == 0 {
                let shared = ui.session.clone();
                let mut session = lock(&shared);
                session.publish_camera(ui.viewport.camera);
                let pointer = Pointer {
                    position: DVec2::new(f64::from(x), f64::from(y)),
                    size: (
                        viewport_size.0.round().max(1.0) as u32,
                        viewport_size.1.round().max(1.0) as u32,
                    ),
                };
                let result = if down {
                    ui.interaction.down(&mut session, pointer, control)
                } else {
                    ui.interaction.up(&mut session, pointer)
                };
                refresh_interaction(&app, &session);
                match result {
                    Ok(true) => {
                        if !down {
                            ui.viewport
                                .mouse_button(button, down, (x, y), viewport_size, control);
                        }
                        return;
                    }
                    Err(error) => {
                        if !down {
                            ui.interaction.cancel(&mut session);
                            ui.viewport
                                .mouse_button(button, down, (x, y), viewport_size, control);
                        }
                        app.set_message(error.into());
                        return;
                    }
                    _ => {}
                }
            }
            let click = ui
                .viewport
                .mouse_button(button, down, (x, y), viewport_size, control);
            app.set_flying(ui.viewport.looking());
            let Some(click) = click else { return };
            let (origin, direction) =
                ui.viewport
                    .camera
                    .ray(click.ndc.0, click.ndc.1, click.aspect);
            let mut session = lock(&ui.session);
            let hit = session.pick(&Ray {
                origin: origin.as_dvec3(),
                direction: direction.as_dvec3(),
            });
            let result = if session.face_mode() {
                let face = hit.and_then(|hit| {
                    Some(map_core::surface::FaceId {
                        solid: hit.solid?,
                        side: hit.side?,
                    })
                });
                match (face, click.control) {
                    (Some(face), true) => session.select_faces(&[face], SelectMode::Toggle),
                    (Some(face), false) => session.select_faces(&[face], SelectMode::Replace),
                    (None, true) => Ok(()),
                    (None, false) => session.select_faces(&[], SelectMode::Replace),
                }
            } else {
                match (hit, click.control) {
                    (Some(hit), true) => session.select(&[hit.object], SelectMode::Toggle),
                    (Some(hit), false) => session.select(&[hit.object], SelectMode::Replace),
                    (None, true) => Ok(()),
                    (None, false) => session.select(&[], SelectMode::Replace),
                }
            };
            if let Err(message) = result {
                app.set_message(message.into());
            }
        }
    });
    app.on_mouse_moved({
        let (app, ui) = (app.as_weak(), ui.clone());
        move |x, y| {
            let app = app.unwrap();
            let size = (app.get_viewport_width(), app.get_viewport_height());
            let mut ui = ui.borrow_mut();
            ui.viewport.mouse_moved(x, y, size.1);
            if let Some(drag) = ui.clip_drag {
                let shared = ui.session.clone();
                let mut session = lock(&shared);
                let result = plane_coordinates(
                    ui.viewport.camera,
                    (x, y),
                    size,
                    session.spatial().elevation,
                )
                .ok_or("Viewport has no size".to_owned())
                .and_then(|end| clip_drag_plane(&session, drag, end))
                .and_then(|plane| preview_clip(&app, &mut session, plane));
                if result.is_err() {
                    session.clear_clip_preview();
                    refresh_clip(&app, &session);
                }
            }
            if let Some(drag) = ui.brush_drag {
                let shared = ui.session.clone();
                let mut session = lock(&shared);
                if let Some(end) = plane_coordinates(
                    ui.viewport.camera,
                    (x, y),
                    size,
                    session.spatial().elevation,
                ) {
                    match drag_bounds(&session, drag, end) {
                        Ok((mins, maxs)) => {
                            drop(session.preview_block(mins, maxs));
                        }
                        Err(_) => session.clear_block_preview(),
                    }
                }
            }
            let shared = ui.session.clone();
            let mut session = lock(&shared);
            let pointer = Pointer {
                position: DVec2::new(f64::from(x), f64::from(y)),
                size: (
                    size.0.round().max(1.0) as u32,
                    size.1.round().max(1.0) as u32,
                ),
            };
            if let Err(error) = ui.interaction.moved(&mut session, pointer) {
                app.set_message(error.into());
            }
            refresh_interaction(&app, &session);
        }
    });
    app.on_wheel({
        let (app, ui) = (app.as_weak(), ui.clone());
        move |delta, x, y| {
            let app = app.unwrap();
            let width = app.get_viewport_width().max(1.0);
            let height = app.get_viewport_height().max(1.0);
            let ndc = (x * 2.0 / width - 1.0, 1.0 - y * 2.0 / height);
            let mut ui = ui.borrow_mut();
            if ui.interaction.active() {
                let shared = ui.session.clone();
                ui.interaction.cancel(&mut lock(&shared));
                refresh_interaction(&app, &lock(&shared));
            }
            if ui.clip_drag.take().is_some() {
                let mut session = lock(&ui.session);
                session.cancel_clip_preview();
                refresh_clip(&app, &session);
            }
            ui.viewport.wheel(delta, ndc, width / height)
        }
    });
    app.on_viewport_focus_lost({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            {
                let mut ui = ui.borrow_mut();
                if ui.interaction.active() {
                    let shared = ui.session.clone();
                    ui.interaction.cancel(&mut lock(&shared));
                    refresh_interaction(&app.unwrap(), &lock(&shared));
                }
            }
            ui.borrow_mut().viewport.release_all();
            ui.borrow_mut().brush_drag = None;
            if ui.borrow_mut().clip_drag.take().is_some() {
                lock(&session_of(&ui)).cancel_clip_preview();
            }
            lock(&session_of(&ui)).cancel_block_preview();
            app.unwrap().set_flying(false);
        }
    });

    let frame_timer = slint::Timer::default();
    let mut last_tick = Instant::now();
    frame_timer.start(slint::TimerMode::Repeated, Duration::from_millis(8), {
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            let Some(app) = app.upgrade() else { return };
            let now = Instant::now();
            let dt = (now - last_tick).as_secs_f32().min(0.1);
            last_tick = now;

            let ui = &mut *ui.borrow_mut();
            ui.viewport.update(dt);
            let mut panel_dirty = false;
            {
                let mut session = lock(&ui.session);
                session.poll_game();
                if ui
                    .clip_drag
                    .is_some_and(|drag| drag.epoch != session.clip_epoch())
                {
                    ui.clip_drag = None;
                }
                if ui
                    .brush_drag
                    .is_some_and(|drag| drag.epoch != session.brush_epoch())
                {
                    ui.brush_drag = None;
                }
                if session.definitions_serial() != ui.seen_definitions {
                    ui.seen_definitions = session.definitions_serial();
                    if let Some(window) = &ui.entity_window {
                        refresh_entity_window(window, &session);
                    }
                    panel_dirty = true;
                }
                if session.camera_serial() != ui.seen_camera {
                    ui.seen_camera = session.camera_serial();
                    ui.viewport.camera = session.camera();
                    ui.viewport.release_all();
                    ui.brush_drag = None;
                    ui.viewport.redraw();
                    refresh_spatial(&app, &session);
                } else {
                    session.publish_camera(ui.viewport.camera);
                }
                if session.scene_serial() != ui.seen_scene {
                    ui.brush_drag = None;
                    ui.seen_scene = session.scene_serial();
                    let mesh = session.build_scene();
                    ui.viewport.set_scene(&ui.ctx, &mesh, session.materials());
                    if let Some(window) = &ui.surface_window
                        && window.window().is_visible()
                        && let Some(target) = &ui.surface_target
                    {
                        refresh_surface_dialog(window, &mut session, target);
                    }
                    app.set_show_tools(session.view().tool_textures);
                    app.set_show_leak(session.view().leak_path);
                    panel_dirty = true;
                }
                if session.view_serial() != ui.seen_view {
                    ui.seen_view = session.view_serial();
                    ui.viewport.options = session.view();
                    ui.viewport.redraw();
                    refresh_spatial(&app, &session);
                    if let Some(window) = &ui.material_window
                        && window.window().is_visible()
                    {
                        refresh_material_catalog(window, &mut session);
                    }
                    app.set_show_tools(session.view().tool_textures);
                    app.set_show_leak(session.view().leak_path);
                    if !session.spatial().draw_block {
                        ui.brush_drag = None;
                    }
                }
                if session.log_serial() != ui.seen_log {
                    ui.seen_log = session.log_serial();
                    if let Some(entry) = session.recent_log().next() {
                        app.set_message(format!("{}: {}", entry.actor.label(), entry.text).into());
                    }
                    panel_dirty = true;
                }
                if panel_dirty {
                    refresh_panel(&app, &mut session);
                }
            }

            if now.duration_since(ui.last_log_refresh) >= Duration::from_millis(500) {
                ui.last_log_refresh = now;
                if let Some(window) = &ui.game_log_window {
                    let text = lock(&ui.session)
                        .game_log(game::DEFAULT_LOG_LINES)
                        .map(|log| log.text())
                        .unwrap_or_else(|error| error);
                    window.set_content(text.into());
                }
            }

            let scale = app.window().scale_factor();
            let size = (
                (app.get_viewport_width() * scale).round() as u32,
                (app.get_viewport_height() * scale).round() as u32,
            );
            if let Some(frame) = ui.viewport.render(&ui.ctx, size, scale) {
                app.set_frame(frame);
                let c = &ui.viewport.camera;
                let status = if c.projection == Projection::Perspective {
                    format!(
                        "{:.0} {:.0} {:.0}  yaw {:.0}° pitch {:.0}°  speed {:.0}",
                        c.position.x,
                        c.position.y,
                        c.position.z,
                        c.yaw_degrees,
                        c.pitch_degrees,
                        ui.viewport.speed
                    )
                } else {
                    let center = c.position + c.forward() * render::Camera::ORTHO_DISTANCE;
                    format!(
                        "{} view · center {:.1} {:.1} {:.1} · {:.1} units tall",
                        c.projection.name(),
                        center.x,
                        center.y,
                        center.z,
                        c.ortho_height
                    )
                };
                app.set_camera(status.into());
            }
        }
    });

    app.run()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manipulation_controls_share_agent_previews_and_selection_without_desktop_input() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let mut state = Session::new(Err("no assets needed".into()));
        let id = state
            .create_block(
                Actor::Agent,
                DVec3::ZERO,
                DVec3::new(64.0, 64.0, 64.0),
                "brick/test",
                false,
            )
            .unwrap();
        let original = state.document().vmf().clone();
        state.set_camera(render::Camera::planar(
            Projection::Top,
            Vec3::new(32.0, 32.0, 32.0),
            256.0,
        ));
        let shared = Arc::new(Mutex::new(state));
        let app = AppWindow::new().unwrap();
        bind_manipulation_controls(&app, shared.clone());
        refresh_panel(&app, &mut lock(&shared));
        refresh_interaction(&app, &lock(&shared));
        app.show().unwrap();
        app.window().set_size(slint::PhysicalSize::new(1600, 960));
        app.set_handles_mode_index(2);
        app.set_transform_space_index(1);
        app.set_rotation_step("22.5".into());
        app.set_transform_pivot_mode(2);
        app.set_pivot_x("10".into());
        app.set_pivot_y("20".into());
        app.set_pivot_z("30".into());
        app.invoke_transform_tool_changed();
        let settings = lock(&shared).transform_tool();
        assert_eq!(settings.mode, Some(render::handles::Mode::Rotate));
        assert!(settings.local);
        assert_eq!(settings.pivot, Some(DVec3::new(10.0, 20.0, 30.0)));
        assert_eq!(settings.angle_step, 22.5);
        app.set_rotation_step("-1".into());
        app.invoke_transform_tool_changed();
        assert_eq!(lock(&shared).transform_tool(), settings);
        assert!(app.get_message().contains("rotation snap"));
        for apply in [false, true] {
            lock(&shared)
                .preview_transform(
                    Actor::Agent,
                    &[id],
                    edit::Transform {
                        translation: DVec3::new(32.0, 0.0, 0.0),
                        ..Default::default()
                    },
                    None,
                )
                .unwrap();
            refresh_interaction(&app, &lock(&shared));
            save_ui_snapshot(app.window(), "transform-handles-controls");
            for label in ["Apply transform", "Cancel"] {
                assert_dialog_button_visible(&app, label);
            }
            i_slint_backend_testing::ElementHandle::find_by_accessible_label(
                &app,
                if apply { "Apply transform" } else { "Cancel" },
            )
            .next()
            .unwrap()
            .invoke_accessible_default_action();
            assert!(!app.get_has_transform_preview());
            assert_eq!(
                lock(&shared).document().undo_labels().count(),
                if apply { 2 } else { 1 }
            );
        }
        lock(&shared).undo(Actor::Agent).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &original);
        app.set_area_rule_index(1);
        app.set_area_mode_index(1);
        app.invoke_area_settings_changed();
        assert_eq!(
            lock(&shared).area_rule(),
            map_core::region::Match::Contained
        );
        assert_eq!(lock(&shared).area_mode(), SelectMode::Add);
        lock(&shared).select(&[], SelectMode::Replace).unwrap();
        for apply in [false, true] {
            let mut state = lock(&shared);
            let camera = state.camera();
            state
                .preview_area_selection(
                    Actor::Agent,
                    camera,
                    Pointer {
                        position: DVec2::new(300.0, 300.0),
                        size: (1000, 1000),
                    },
                    DVec2::new(700.0, 700.0),
                    map_core::region::Match::Contained,
                    SelectMode::Add,
                )
                .unwrap();
            refresh_interaction(&app, &state);
            drop(state);
            save_ui_snapshot(app.window(), "area-selection-controls");
            for label in ["Apply selection", "Cancel"] {
                assert_dialog_button_visible(&app, label);
            }
            i_slint_backend_testing::ElementHandle::find_by_accessible_label(
                &app,
                if apply { "Apply selection" } else { "Cancel" },
            )
            .next()
            .unwrap()
            .invoke_accessible_default_action();
            assert!(!app.get_has_area_preview());
            assert_eq!(
                lock(&shared).selected_ids(),
                if apply { vec![id] } else { vec![] }
            );
            assert_eq!(lock(&shared).document().vmf(), &original);
            assert_eq!(lock(&shared).document().undo_labels().count(), 1);
        }
    }
    #[test]
    fn clipping_controls_preview_apply_cancel_and_escape_without_desktop_input() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let mut state = Session::new(Err("no assets needed".into()));
        let id = state
            .create_block(
                Actor::User,
                DVec3::ZERO,
                DVec3::new(128.0, 96.0, 64.0),
                "brick/test",
                false,
            )
            .unwrap();
        let original = state.document().vmf().clone();
        let revision = state.document().revision();
        let shared = Arc::new(Mutex::new(state));
        let app = AppWindow::new().unwrap();
        bind_clip_controls(&app, shared.clone());
        app.on_key({
            let (app, shared) = (app.as_weak(), shared.clone());
            move |text, down, _, _| {
                if down && text == key_text(Key::Escape) {
                    cancel_clip(&app.unwrap(), &mut lock(&shared));
                    true
                } else {
                    false
                }
            }
        });
        refresh_panel(&app, &mut lock(&shared));
        app.show().unwrap();
        app.window().set_size(slint::PhysicalSize::new(1600, 960));
        app.window().take_snapshot().unwrap();
        i_slint_backend_testing::ElementHandle::find_by_accessible_label(
            &app,
            "Clip selected brushes",
        )
        .next()
        .unwrap()
        .invoke_accessible_default_action();
        assert!(lock(&shared).clip_tool());
        app.set_clip_distance("64".into());
        for keep in [0, 1, 2] {
            app.set_clip_keep_index(keep);
            i_slint_backend_testing::ElementHandle::find_by_accessible_label(&app, "Preview plane")
                .next()
                .unwrap()
                .invoke_accessible_default_action();
            let state = lock(&shared);
            assert_eq!(state.document().vmf(), &original);
            assert_eq!(state.document().revision(), revision);
            let ObjectId::Solid(number) = id else {
                unreachable!()
            };
            let shape =
                map_core::BrushGeometry::from_solid(state.render_document().solid(number).unwrap())
                    .unwrap();
            if keep == 0 {
                assert_eq!(shape.mins.x, 64.0);
            }
            if keep == 1 {
                assert_eq!(shape.maxs.x, 64.0);
            }
            assert_eq!(
                state.render_document().vmf().world.solids.len(),
                if keep == 2 { 2 } else { 1 }
            );
        }
        save_ui_snapshot(app.window(), "clipping-controls");
        for label in ["Preview plane", "Apply cut", "Cancel cut"] {
            assert_dialog_button_visible(&app, label);
            let button =
                i_slint_backend_testing::ElementHandle::find_by_accessible_label(&app, label)
                    .next()
                    .unwrap();
            assert!(button.absolute_position().x + button.size().width <= 752.5);
            assert!(button.absolute_position().y + button.size().height <= 306.5);
        }
        i_slint_backend_testing::ElementHandle::find_by_accessible_label(&app, "Cancel cut")
            .next()
            .unwrap()
            .invoke_accessible_default_action();
        assert!(lock(&shared).clip_preview().is_none());
        assert!(!lock(&shared).clip_tool());
        assert_eq!(lock(&shared).document().vmf(), &original);
        app.invoke_start_clip();
        app.invoke_preview_clip_plane();
        app.window()
            .dispatch_event(slint::platform::WindowEvent::KeyPressed {
                text: Key::Escape.into(),
            });
        assert!(lock(&shared).clip_preview().is_none());
        assert!(!lock(&shared).clip_tool());
        app.invoke_start_clip();
        app.set_clip_normal_x("0".into());
        app.invoke_preview_clip_plane();
        assert!(!app.get_has_clip_preview());
        assert!(app.get_message().contains("nonzero"));
        app.set_clip_normal_x("1".into());
        app.invoke_preview_clip_plane();
        app.invoke_clip_inputs_changed();
        assert!(!app.get_has_clip_preview());
        app.invoke_preview_clip_plane();
        i_slint_backend_testing::ElementHandle::find_by_accessible_label(&app, "Apply cut")
            .next()
            .unwrap()
            .invoke_accessible_default_action();
        assert_eq!(lock(&shared).document().vmf().world.solids.len(), 2);
        assert_eq!(lock(&shared).document().undo_labels().count(), 2);
        lock(&shared).undo(Actor::Agent).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &original);
    }

    #[test]
    fn stale_cut_gestures_cannot_preview_after_mcp_cancel_or_map_replacement() {
        let mut session = Session::new(Err("no assets needed".into()));
        session
            .create_block(
                Actor::User,
                DVec3::splat(-32.0),
                DVec3::splat(32.0),
                "brick/test",
                false,
            )
            .unwrap();
        session.set_clip_tool(Actor::User, true).unwrap();
        let drag = BrushDrag {
            start: DVec2::new(-16.0, -32.0),
            epoch: session.clip_epoch(),
            generation: session.document_generation(),
            revision: session.document().revision(),
            projection: session.camera().projection,
        };
        let end = DVec2::new(16.0, 32.0);
        assert!(clip_drag_plane(&session, drag, end).is_ok());
        session.cancel_clip_preview();
        assert!(clip_drag_plane(&session, drag, end).is_err());
        session.new_map(Actor::Agent, true).unwrap();
        assert!(clip_drag_plane(&session, drag, end).is_err());
    }

    fn save_ui_snapshot(window: &slint::Window, name: &str) {
        let image = window.take_snapshot().unwrap();
        assert!(image.width() > 0 && image.height() > 0);
        if let Some(directory) = std::env::var_os("HAMMERR_UI_SNAPSHOTS") {
            let path = PathBuf::from(directory).join(format!("{name}.png"));
            let mut png = png::Encoder::new(
                std::fs::File::create(path).unwrap(),
                image.width(),
                image.height(),
            );
            png.set_color(png::ColorType::Rgba);
            png.set_depth(png::BitDepth::Eight);
            png.write_header()
                .unwrap()
                .write_image_data(image.as_bytes())
                .unwrap();
        }
    }

    fn assert_dialog_button_visible<C: ComponentHandle>(component: &C, label: &str) {
        let button =
            i_slint_backend_testing::ElementHandle::find_by_accessible_label(component, label)
                .next()
                .unwrap();
        let position = button.absolute_position();
        let size = button.size();
        let window = component.window().size();
        assert!(size.width > 0.0 && size.height > 0.0);
        assert!(position.x >= 0.0 && position.y >= 0.0);
        assert!(
            position.x + size.width <= window.width as f32 + 1.0,
            "{label} outside window"
        );
        assert!(
            position.y + size.height <= window.height as f32 + 1.0,
            "{label} outside window"
        );
    }

    #[test]
    fn material_catalog_and_texture_dialog_render_without_desktop_input() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let material = MaterialWindow::new().unwrap();
        let mut session = Session::new(mount_game(None));
        session
            .create_block(
                Actor::User,
                DVec3::ZERO,
                DVec3::splat(64.0),
                "dev/dev_measuregeneric01",
                false,
            )
            .unwrap();
        session
            .favorite_material("brick/brickwall001a", true)
            .unwrap();
        material.set_material("dev/dev_measuregeneric01".into());
        refresh_material_catalog(&material, &mut session);
        refresh_material_window(&material, &mut session);
        material.show().unwrap();
        save_ui_snapshot(material.window(), "material-browser");
        for label in ["Cancel", "Use material", "Next"] {
            assert_dialog_button_visible(&material, label);
        }
        assert!(material.get_total() > 0);
        let concrete = session
            .material_catalog("", "category:Concrete", 0, 36)
            .unwrap();
        if let Some(first) = concrete.entries.first() {
            material.set_group("category:Concrete".into());
            material.set_material(first.name.clone().into());
            refresh_material_catalog(&material, &mut session);
            refresh_material_window(&material, &mut session);
            save_ui_snapshot(material.window(), "materials-grouped");
            let preview = i_slint_backend_testing::ElementHandle::find_by_element_id(
                &material,
                "MaterialWindow::preview-image",
            )
            .next()
            .unwrap();
            assert!(
                preview.absolute_position().x + preview.size().width
                    <= material.window().size().width as f32
            );
        }
        material.set_group("used".into());
        refresh_material_catalog(&material, &mut session);
        assert_eq!(material.get_total(), 1);
        let cancelled = Rc::new(std::cell::Cell::new(false));
        material.on_cancel({
            let cancelled = cancelled.clone();
            move || cancelled.set(true)
        });
        i_slint_backend_testing::ElementHandle::find_by_accessible_label(&material, "Cancel")
            .next()
            .unwrap()
            .invoke_accessible_default_action();
        assert!(cancelled.get());
        material.hide().unwrap();
        let surface = SurfaceWindow::new().unwrap();
        surface.set_selection_label("Textures on 3 selected faces".into());
        surface.set_material("dev/dev_measuregeneric01".into());
        surface.set_preview(
            session
                .material_preview("dev/dev_measuregeneric01")
                .ok()
                .map(|preview| preview_image(&preview))
                .unwrap_or_default(),
        );
        surface.set_scale_u("0.25".into());
        surface.set_scale_v("0.25".into());
        surface.set_shift_u("0".into());
        surface.set_shift_v("0".into());
        surface.set_rotation("0".into());
        surface.set_message("Preview changes before applying, or Cancel to discard.".into());
        surface.show().unwrap();
        save_ui_snapshot(surface.window(), "face-textures");
        for label in ["Cancel", "Preview textures", "Apply textures"] {
            assert_dialog_button_visible(&surface, label);
        }
        surface.hide().unwrap();
    }

    #[test]
    fn texture_dialog_buttons_preview_cancel_apply_and_reject_stale_selections() {
        i_slint_backend_testing::init_no_event_loop();
        let window = SurfaceWindow::new().unwrap();
        let mut session = Session::new(Err("no assets needed".into()));
        let ObjectId::Solid(id) = session
            .create_block(
                Actor::User,
                DVec3::ZERO,
                DVec3::splat(64.0),
                "brick/wall",
                false,
            )
            .unwrap()
        else {
            panic!()
        };
        let face = map_core::surface::FaceId {
            solid: id,
            side: session.document().solid(id).unwrap().sides[0].id,
        };
        session.select_faces(&[face], SelectMode::Replace).unwrap();
        let target = (
            vec![face],
            session.document_generation(),
            session.document().revision(),
        );
        let before = session.document().vmf().clone();
        let session = Rc::new(RefCell::new(session));
        window.set_material("wood/floor".into());
        window.set_scale_u("0.5".into());
        window.set_rotation("45".into());
        window.on_preview_edit({
            let (session, window, target) = (session.clone(), window.as_weak(), target.clone());
            move || {
                let window = window.unwrap();
                if let Err(error) =
                    apply_surface_dialog(&window, &mut session.borrow_mut(), &target, true)
                {
                    window.set_message(error.into());
                }
            }
        });
        window.on_apply({
            let (session, window, target) = (session.clone(), window.as_weak(), target.clone());
            move || {
                let window = window.unwrap();
                if let Err(error) =
                    apply_surface_dialog(&window, &mut session.borrow_mut(), &target, false)
                {
                    window.set_message(error.into());
                }
            }
        });
        window.on_cancel({
            let session = session.clone();
            move || session.borrow_mut().cancel_face_preview()
        });
        window.show().unwrap();
        i_slint_backend_testing::ElementHandle::find_by_accessible_label(
            &window,
            "Preview textures",
        )
        .next()
        .unwrap()
        .invoke_accessible_default_action();
        assert!(session.borrow().has_face_preview());
        assert_eq!(session.borrow().document().vmf(), &before);
        session
            .borrow_mut()
            .edit_faces(
                Actor::Agent,
                &[face],
                map_core::surface::SurfaceEdit {
                    rotation: Some(90.0),
                    shift_u: Some(64.0),
                    ..Default::default()
                },
                false,
                true,
            )
            .unwrap();
        refresh_surface_dialog(&window, &mut session.borrow_mut(), &target);
        assert_eq!(window.get_rotation(), "90");
        assert_eq!(window.get_shift_u(), "64");
        i_slint_backend_testing::ElementHandle::find_by_accessible_label(&window, "Cancel")
            .next()
            .unwrap()
            .invoke_accessible_default_action();
        assert!(!session.borrow().has_face_preview());
        assert_eq!(session.borrow().document().vmf(), &before);
        window.set_material("wood/floor".into());
        window.invoke_preview_edit();
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::KeyPressed {
                text: slint::platform::Key::Escape.into(),
            });
        assert!(!session.borrow().has_face_preview());
        i_slint_backend_testing::ElementHandle::find_by_accessible_label(&window, "Apply textures")
            .next()
            .unwrap()
            .invoke_accessible_default_action();
        assert_eq!(
            map_core::surface::side(session.borrow().document(), face)
                .unwrap()
                .material,
            "WOOD/FLOOR"
        );
        session.borrow_mut().undo(Actor::Agent).unwrap();
        assert_eq!(session.borrow().document().vmf(), &before);
        window.invoke_apply();
        assert!(window.get_message().contains("changed"));
        refresh_surface_dialog(&window, &mut session.borrow_mut(), &target);
        assert!(window.get_stale());
        window.hide().unwrap();
    }

    use slint::Model;
    use source_formats::vmf::Entity;

    #[test]
    fn transform_controls_apply_usable_defaults_and_shared_selection_operations() {
        i_slint_backend_testing::init_no_event_loop();
        let app = AppWindow::new().unwrap();
        app.set_transform_x("90".into());
        app.set_transform_y("-45".into());
        app.set_transform_z("30".into());
        app.set_transform_mode(2);
        slint::platform::update_timers_and_animations();
        assert_eq!(
            input_vector([
                app.get_transform_x(),
                app.get_transform_y(),
                app.get_transform_z()
            ])
            .unwrap(),
            DVec3::ONE
        );
        app.set_transform_x("2".into());
        app.set_transform_mode(1);
        slint::platform::update_timers_and_animations();
        assert_eq!(
            input_vector([
                app.get_transform_x(),
                app.get_transform_y(),
                app.get_transform_z()
            ])
            .unwrap(),
            DVec3::ZERO
        );
        let mut session = Session::new(Err("no assets needed".into()));
        let id = session
            .create_block(
                Actor::User,
                DVec3::ZERO,
                DVec3::new(64.0, 32.0, 16.0),
                "brick/wall",
                false,
            )
            .unwrap();
        let session = Rc::new(RefCell::new(session));
        app.on_apply_transform({
            let session = session.clone();
            let app = app.as_weak();
            move || {
                let app = app.unwrap();
                if let Err(message) = apply_selection_transform(&app, &mut session.borrow_mut()) {
                    app.set_message(message.into());
                }
            }
        });
        app.set_has_selection(true);
        app.set_show_transform(true);
        app.set_can_apply_material(true);
        app.set_transform_mode(2);
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(1));
        app.set_transform_x("2".into());
        for (pivot_mode, expected) in [
            (
                0,
                (DVec3::new(-32.0, 0.0, 0.0), DVec3::new(96.0, 32.0, 16.0)),
            ),
            (1, (DVec3::ZERO, DVec3::new(128.0, 32.0, 16.0))),
            (
                2,
                (DVec3::new(-10.0, 0.0, 0.0), DVec3::new(118.0, 32.0, 16.0)),
            ),
        ] {
            app.set_transform_pivot_mode(pivot_mode);
            app.set_pivot_x("10".into());
            app.set_pivot_y("20".into());
            i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(1));
            let button = i_slint_backend_testing::ElementHandle::find_by_accessible_label(
                &app,
                "Scale selection",
            )
            .next()
            .unwrap();
            button.invoke_accessible_default_action();
            assert_eq!(
                query::bounds(session.borrow().document(), id),
                Some(expected)
            );
            session.borrow_mut().undo(Actor::Agent).unwrap();
        }
        let before = session.borrow().document().vmf().clone();
        app.set_transform_y("0".into());
        i_slint_backend_testing::ElementHandle::find_by_accessible_label(&app, "Scale selection")
            .next()
            .unwrap()
            .invoke_accessible_default_action();
        assert_eq!(session.borrow().document().vmf(), &before);
        assert!(app.get_message().contains("positive"));
        app.set_transform_pivot_mode(0);
        app.set_transform_mode(1);
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(1));
        app.set_transform_z("90".into());
        i_slint_backend_testing::ElementHandle::find_by_accessible_label(&app, "Rotate selection")
            .next()
            .unwrap()
            .invoke_accessible_default_action();
        let (mins, maxs) = query::bounds(session.borrow().document(), id).unwrap();
        assert!(mins.abs_diff_eq(DVec3::new(16.0, -16.0, 0.0), 1e-8));
        assert!(maxs.abs_diff_eq(DVec3::new(48.0, 48.0, 16.0), 1e-8));
        session.borrow_mut().undo(Actor::Agent).unwrap();
        app.set_transform_mode(0);
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(1));
        app.set_transform_x("0.125".into());
        app.set_transform_y("-0.25".into());
        app.set_transform_z("0.5".into());
        i_slint_backend_testing::ElementHandle::find_by_accessible_label(&app, "Move selection")
            .next()
            .unwrap()
            .invoke_accessible_default_action();
        assert_eq!(
            query::bounds(session.borrow().document(), id),
            Some((
                DVec3::new(0.125, -0.25, 0.5),
                DVec3::new(64.125, 31.75, 16.5)
            ))
        );
        app.set_transform_mode(2);
        slint::platform::update_timers_and_animations();
        app.set_transform_mode(0);
        slint::platform::update_timers_and_animations();
        assert_eq!(
            input_vector([
                app.get_transform_x(),
                app.get_transform_y(),
                app.get_transform_z()
            ])
            .unwrap(),
            DVec3::ZERO
        );
    }

    #[test]
    fn stale_brush_drags_cannot_create_geometry_in_a_replaced_document() {
        let mut session = Session::new(Err("no assets needed".into()));
        session.set_projection(Actor::User, Projection::Top);
        let drag = BrushDrag {
            start: DVec2::ZERO,
            epoch: session.brush_epoch(),
            generation: session.document_generation(),
            revision: session.document().revision(),
            projection: Projection::Top,
        };
        assert!(drag_bounds(&session, drag, DVec2::splat(32.0)).is_ok());
        session.new_map(Actor::Agent, true).unwrap();
        assert!(drag_bounds(&session, drag, DVec2::splat(32.0)).is_err());
        assert!(session.document().vmf().world.solids.is_empty());
    }

    #[test]
    fn shared_cancellation_and_settings_invalidate_active_human_drags() {
        for preview in [false, true] {
            let mut session = Session::new(Err("no assets needed".into()));
            session.set_projection(Actor::User, Projection::Top);
            let drag = BrushDrag {
                start: DVec2::ZERO,
                epoch: session.brush_epoch(),
                generation: session.document_generation(),
                revision: session.document().revision(),
                projection: Projection::Top,
            };
            if preview {
                session
                    .preview_block(DVec3::ZERO, DVec3::splat(32.0))
                    .unwrap();
            }
            assert!(drag_bounds(&session, drag, DVec2::splat(32.0)).is_ok());
            session.cancel_block_preview();
            assert!(drag_bounds(&session, drag, DVec2::splat(32.0)).is_err());
            let drag = BrushDrag {
                epoch: session.brush_epoch(),
                ..drag
            };
            session
                .set_spatial(
                    Actor::Agent,
                    spatial::Update {
                        grid_size: Some(8.0),
                        ..spatial::Update::default()
                    },
                )
                .unwrap();
            assert!(drag_bounds(&session, drag, DVec2::splat(32.0)).is_err());
            assert!(session.document().vmf().world.solids.is_empty());
            assert!(session.document().undo_labels().next().is_none());
        }
    }

    #[test]
    fn a_collapsed_snapped_preview_can_grow_into_a_valid_footprint() {
        let mut session = Session::new(Err("no assets needed".into()));
        session.set_projection(Actor::User, Projection::Top);
        let drag = BrushDrag {
            start: DVec2::ZERO,
            epoch: session.brush_epoch(),
            generation: session.document_generation(),
            revision: session.document().revision(),
            projection: Projection::Top,
        };
        session
            .preview_block(DVec3::ZERO, DVec3::splat(32.0))
            .unwrap();
        assert!(drag_bounds(&session, drag, DVec2::ONE).is_err());
        session.clear_block_preview();
        assert!(session.block_preview().is_none());
        let (mins, maxs) = drag_bounds(&session, drag, DVec2::splat(32.0)).unwrap();
        session.preview_block(mins, maxs).unwrap();
        assert_eq!(session.block_preview(), Some((mins, maxs)));
        assert!(session.document().undo_labels().next().is_none());
    }

    #[test]
    fn property_controls_preserve_imported_values_and_show_inherited_defaults() {
        let mut session = Session::new(Err("no game required".into()));
        session.set_definitions(
            Actor::User,
            Catalog::parse(
                r#"@BaseClass = Base [
                    _light(color255) : "Brightness" : "255 255 255 200"
                    spawnflags(flags) = [ 1 : "Enabled" : 0 2 : "Special" : 0 ]
                ]
                @PointClass base(Base) = light [
                    angles(angle) : "Orientation" : "0 0 0"
                    pitch(angle) : "Pitch" : 0
                    style(choices) : "Style" : 0 = [ 0 : "Steady" 1 : "Pulse" ]
                    editor_id(integer) readonly : "Editor ID"
                ]"#,
            )
            .unwrap(),
        );
        let mut entity = Entity::new(1, "light");
        entity.properties.set("style", "99");
        entity.properties.set("spawnflags", "9");
        entity.properties.set("custom_key", "keep me");
        let rows = entity_property_rows(&session, &entity);
        let row = |key| rows.iter().find(|row| row.key == key).unwrap();
        let style = row("style");
        assert_eq!(style.value, "99");
        assert_eq!(style.choice_index, 0);
        assert_eq!(style.choice_values.row_data(0).unwrap(), "99");
        assert_eq!(style.choice_values.row_count(), 3);
        let flags = row("spawnflags");
        assert_eq!(flags.value, "9");
        assert!(flags.flags.row_data(0).unwrap().checked);
        assert!(!flags.flags.row_data(1).unwrap().checked);
        assert_eq!(row("_light").value, "255 255 255 200");
        assert_eq!(row("_light").type_name, "color255");
        assert!(!row("angles").numeric);
        assert!(row("pitch").numeric);
        assert!(row("editor_id").read_only);
        assert_eq!(row("custom_key").value, "keep me");
        assert_eq!(row("custom_key").type_name, "string");
        assert!(entity.properties.get("_light").is_none());
    }
}
