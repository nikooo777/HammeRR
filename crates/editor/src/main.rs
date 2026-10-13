mod carve_ui;
mod checks_ui;
mod compile_ui;
mod components_ui;
mod connection_ui;
mod context_ui;
mod game;
mod hollow_ui;
mod inspector;
mod interaction;
mod lighting_ui;
mod management_ui;
mod material_favorites;
mod material_replace_ui;
mod materials;
mod mcp;
mod pointfile_ui;
mod primitive_ui;
mod recovery;
mod recovery_ui;
mod rename_ui;
mod saved_views_ui;
mod session;
mod spatial;
mod statistics_ui;
mod steam;
mod surface_group_ui;
#[cfg(test)]
mod test_assets;
mod viewport;
mod volume_ui;

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
  --headless   no window; serve MCP over stdin/stdout; graphics initialized on first render";

const DEFAULT_MCP_PORT: u16 = 7377;
const BLOCK_SIZE: f64 = 128.0;
const ACTIVITY_SHOWN: usize = 30;
const SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(10);

struct ShutdownGuard(SharedSession);

impl Drop for ShutdownGuard {
    fn drop(&mut self) {
        lock(&self.0).shutdown();
        loop {
            let busy = {
                let session = lock(&self.0);
                session.launching() || session.recovery_busy()
            };
            if !busy {
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
    connection_window: Option<ConnectionWindow>,
    seen_connections: (u64, u64, u64),
    properties_window: Option<PropertiesWindow>,
    report_window: Option<ReportWindow>,
    statistics_window: Option<StatisticsWindow>,
    pointfile_window: Option<PointfileWindow>,
    components_window: Option<ComponentsWindow>,
    hollow_window: Option<HollowWindow>,
    carve_window: Option<CarveWindow>,
    recovery_window: Option<RecoveryWindow>,
    checks_window: Option<ChecksWindow>,
    context_window: Option<ContextWindow>,
    primitive_window: Option<PrimitiveWindow>,
    seen_primitive: u64,
    seen_context: (u64, u64),
    seen_checks: (u64, u64, u64),
    seen_recovery: (u64, u64),
    seen_management: (u64, u64, u64),
    rename_window: Option<RenameWindow>,
    seen_rename: (u64, u64, u64),
    color_window: Option<ColorWindow>,
    color_target: Option<(u32, String, u64)>,
    material_window: Option<MaterialWindow>,
    material_target: Option<(u32, String, u64)>,
    surface_window: Option<SurfaceWindow>,
    surface_target: Option<(Vec<map_core::surface::FaceId>, u64, u64)>,
    lighting: Option<lighting_ui::Controller>,
    material_replace: Option<material_replace_ui::Controller>,
    surface_group: Option<surface_group_ui::Controller>,
    volume: Option<volume_ui::Controller>,
    compile_status: Option<compile_ui::Controller>,
    saved_views: Option<saved_views_ui::Controller>,
    material_for_group: bool,
    material_for_replace: Option<bool>,
    material_for_faces: bool,
    material_for_clip: bool,
    material_for_primitive: bool,
    material_for_extrusion: bool,
    material_for_partition: bool,
    partition_material_target: Option<components_ui::PartitionMaterialTarget>,
    extrusion_material_target: Option<(Option<u64>, u64)>,
    primitive_material_target: Option<(Option<u64>, u64)>,
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
    let index = if session.paint_enabled() {
        if session.paint_operation() == session::paint::Operation::Selection {
            4
        } else {
            5
        }
    } else {
        index
    };
    app.set_handles_mode_index(index);
    app.set_paint_radius(session.paint_radius() as f32);
    app.set_texture_paint_mode(i32::from(
        session.paint_operation() == session::paint::Operation::Projection,
    ));
    app.set_has_texture_stroke(
        session
            .paint_stroke()
            .is_some_and(|stroke| stroke.texture.is_some()),
    );
    app.set_has_paint_preview(session.paint_stroke().is_some());
    app.set_paint_token(
        session
            .paint_stroke()
            .map(|stroke| stroke.token.to_string())
            .unwrap_or_default()
            .into(),
    );
    if index > 0 && index < 4 {
        app.set_transform_mode(index - 1);
    }
    app.set_transform_space_index(i32::from(tool.local));
    app.set_rotation_step(tool.angle_step.to_string().into());
    let (texture_lock, texture_scale_lock) = session.transform_preview().map_or(
        (
            tool.texture_lock,
            tool.texture_scale_lock.unwrap_or(tool.texture_lock),
        ),
        |preview| {
            (
                preview.transform.texture_lock,
                preview
                    .transform
                    .texture_scale_lock
                    .unwrap_or(preview.transform.texture_lock),
            )
        },
    );
    app.set_transform_texture_lock(texture_lock);
    app.set_transform_texture_scale_lock(texture_scale_lock);
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
    let selection_mode = session
        .paint_stroke()
        .map_or(session.area_mode(), |stroke| stroke.spec.mode);
    app.set_area_mode_index(match selection_mode {
        SelectMode::Replace => 0,
        SelectMode::Add => 1,
        SelectMode::Remove => 2,
        SelectMode::Toggle => 3,
    });
    if let Some(preview) = session.transform_preview() {
        let spec = preview.transform;
        let presentation = preview.presentation();
        app.set_transform_mode(presentation.primary.index());
        app.set_transform_preview_summary(presentation.summary.into());
        if let Some(shear) = spec.shear {
            app.set_transform_shear_axis(shear.axis as i32);
            app.set_transform_shear_reference(shear.reference as i32);
            app.set_transform_shear_factor(shear.factor.to_string().into());
        }
        app.set_transform_reflect_x(spec.reflection[0]);
        app.set_transform_reflect_y(spec.reflection[1]);
        app.set_transform_reflect_z(spec.reflection[2]);
        let translation = spec.basis.transpose() * spec.translation;
        app.set_transform_preview_values(
            Rc::new(VecModel::from(
                [translation, spec.rotation, spec.scale]
                    .iter()
                    .flat_map(|value| {
                        value
                            .to_array()
                            .map(|value| SharedString::from(value.to_string()))
                    })
                    .collect::<Vec<_>>(),
            ))
            .into(),
        );
        app.set_transform_space_index(i32::from(
            !spec.basis.abs_diff_eq(glam::DMat3::IDENTITY, 1e-8),
        ));
        app.set_transform_pivot_mode(2);
        app.set_pivot_x(spec.pivot.x.to_string().into());
        app.set_pivot_y(spec.pivot.y.to_string().into());
        app.set_pivot_z(spec.pivot.z.to_string().into());
    } else {
        app.set_transform_preview_summary("".into());
    }
    app.set_has_transform_preview(session.transform_preview().is_some());
    app.set_has_area_preview(session.area_preview().is_some());
    let summary = if let Some(drag) = session.component_drag() {
        drag.error.clone().unwrap_or_else(|| {
            format!(
                "Move components · {} · release applies · Esc cancels",
                drag.handle.name()
            )
        })
    } else if let Some(picking) = session.component_picking() {
        picking.hint()
    } else if let Some(drag) = session.transform_drag() {
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
    } else if let Some(stroke) = session.paint_stroke() {
        if let Some(texture) = &stroke.texture {
            format!(
                "{} {} will change · texture preview",
                texture.changed,
                if texture.changed == 1 {
                    "face"
                } else {
                    "faces"
                }
            )
        } else {
            format!(
                "{} {} · paint preview",
                stroke.objects.len() + stroke.faces.len(),
                match (stroke.spec.faces, stroke.objects.len() + stroke.faces.len()) {
                    (true, 1) => "face",
                    (true, _) => "faces",
                    (false, 1) => "object",
                    (false, _) => "objects",
                }
            )
        }
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
        3 => Some(render::handles::Mode::Scale),
        _ => None,
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
        texture_scale_lock: Some(app.get_transform_texture_scale_lock()),
        angle_step: app
            .get_rotation_step()
            .parse()
            .map_err(|_| "Enter a numeric angle snap")?,
    })
}

fn paint_operation_from_controls(app: &AppWindow) -> session::paint::Operation {
    if app.get_handles_mode_index() != 5 {
        session::paint::Operation::Selection
    } else if app.get_texture_paint_mode() == 1 {
        session::paint::Operation::Projection
    } else {
        session::paint::Operation::Material
    }
}

fn refresh_texture_brush_sample(app: &AppWindow, session: &mut Session) {
    let material = session
        .surface_sample()
        .map(|sample| sample.material.to_ascii_lowercase())
        .unwrap_or_default();
    if app.get_texture_sample_material().as_str() != material {
        let preview = if material.is_empty() {
            slint::Image::default()
        } else {
            session
                .material_preview(&material)
                .ok()
                .map(|preview| preview_image(&preview))
                .unwrap_or_default()
        };
        app.set_texture_sample_material(material.into());
        app.set_texture_sample_preview(preview);
    }
}

fn bind_manipulation_controls(app: &AppWindow, shared: SharedSession) {
    app.on_transform_tool_changed({
        let (app, shared) = (app.as_weak(), shared.clone());
        move || {
            let app = app.unwrap();
            let mut session = lock(&shared);
            let result = transform_tool_from_controls(&app)
                .and_then(|tool| session.set_transform_tool(Actor::User, tool))
                .and_then(|()| {
                    session.set_paint_operation(
                        Actor::User,
                        app.get_handles_mode_index() >= 4,
                        f64::from(app.get_paint_radius()),
                        paint_operation_from_controls(&app),
                    )
                });
            if let Err(error) = result {
                app.set_message(error.into());
            }
            refresh_spatial(&app, &session);
        }
    });
    app.on_paint_settings_changed({
        let (app, shared) = (app.as_weak(), shared.clone());
        move || {
            let app = app.unwrap();
            let mut session = lock(&shared);
            if let Err(error) = session.set_paint_operation(
                Actor::User,
                app.get_handles_mode_index() >= 4,
                f64::from(app.get_paint_radius()),
                paint_operation_from_controls(&app),
            ) {
                app.set_message(error.into());
            }
            refresh_interaction(&app, &session);
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
            session.cancel_area_selection();
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
            let result = if app.get_has_paint_preview() {
                app.get_paint_token()
                    .parse()
                    .map_err(|_| "Paint preview changed; refresh it".to_string())
                    .and_then(|token| session.commit_active_stroke(Actor::User, token))
                    .map(|_| ())
            } else {
                session.commit_area_selection().map(|_| ())
            };
            if let Err(error) = result {
                app.set_message(error.into());
            }
            refresh_panel(&app, &mut session);
            refresh_interaction(&app, &session);
        }
    });
    app.on_cancel_interaction({
        let app = app.as_weak();
        move || {
            let app = app.unwrap();
            let mut session = lock(&shared);
            if session.paint_stroke().is_some() {
                let result = app
                    .get_paint_token()
                    .parse()
                    .map_err(|_| "Paint preview changed; refresh it".to_string())
                    .and_then(|token| session.cancel_paint_selection(token));
                if let Err(error) = result {
                    app.set_message(error.into());
                }
            } else {
                session.cancel_transform_preview();
                session.cancel_area_selection();
                session.clear_volume_selection();
            }
            refresh_interaction(&app, &session);
        }
    });
}

fn refresh_clip(app: &AppWindow, session: &Session) {
    app.set_can_clip(session.can_clip_selection());
    app.set_clipping(session.clip_tool());
    app.set_clip_positioning(session.clip_drag().is_some());
    let options = session.clip_handle_options();
    app.set_clip_handles_mode(match options.mode {
        Some(render::handles::Mode::Move) => 1,
        Some(render::handles::Mode::Rotate) => 2,
        _ => 0,
    });
    app.set_clip_handles_snap(options.snap);
    app.set_clip_angle_step(options.angle_step.to_string().into());
    app.set_has_clip_preview(session.clip_preview().is_some());
    app.set_clip_point_mode(session.clip_points().is_some());
    app.set_clip_point_count(session.clip_points().map_or(0, |d| d.points.len() as i32));
    if let Some(draft) = session.clip_points() {
        let positions = draft
            .points
            .iter()
            .enumerate()
            .map(|(i, p)| format!("{}: {:.2}, {:.2}, {:.2}", i + 1, p.x, p.y, p.z))
            .collect::<Vec<_>>()
            .join("  ·  ");
        app.set_clip_points_summary(
            format!(
                "{} / 3 points · click surfaces to add points; drag handles to position the plane\n{}",
                draft.points.len(),
                positions
            )
            .into(),
        );
    }
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
        if let Some(error) = session.clip_drag().and_then(|drag| drag.error.as_ref()) {
            app.set_clip_summary(error.clone().into());
        }
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

fn change_clip_points(
    app: &AppWindow,
    session: &mut Session,
    points: Vec<DVec3>,
) -> Result<(), String> {
    let material = app.get_clip_material().trim().to_owned();
    session.set_clip_points(
        Actor::User,
        session::clip_points::Draft {
            points,
            keep: clip_keep(app),
            material: (!material.is_empty()).then_some(material),
            snap: true,
        },
        &session.clip_point_stamp(),
    )
}

fn bind_clip_controls(app: &AppWindow, shared: SharedSession) {
    app.on_clip_handles_changed({
        let (app, shared) = (app.as_weak(), shared.clone());
        move || {
            let app = app.unwrap();
            let mut session = lock(&shared);
            let snapshot = session.clip_point_stamp();
            let result = app
                .get_clip_angle_step()
                .parse::<f64>()
                .map_err(|_| "Enter a rotation step in degrees".to_owned())
                .and_then(|angle_step| {
                    session.set_clip_handle_options(
                        session::clip_drag::Options {
                            mode: match app.get_clip_handles_mode() {
                                1 => Some(render::handles::Mode::Move),
                                2 => Some(render::handles::Mode::Rotate),
                                _ => None,
                            },
                            snap: app.get_clip_handles_snap(),
                            angle_step,
                        },
                        &snapshot,
                    )
                });
            app.set_message(result.err().unwrap_or_default().into());
            refresh_clip(&app, &session);
        }
    });
    app.on_cancel_clip_positioning({
        let (app, shared) = (app.as_weak(), shared.clone());
        move || {
            let app = app.unwrap();
            let mut session = lock(&shared);
            if let Some(token) = session.clip_drag().map(|d| d.token) {
                let result = session.cancel_clip_drag(token);
                app.set_message(result.err().unwrap_or_default().into());
            }
            refresh_clip(&app, &session);
        }
    });
    app.on_clip_points_action({
        let (app, shared) = (app.as_weak(), shared.clone());
        move |action| {
            let app = app.unwrap();
            let mut session = lock(&shared);
            if action == 3 {
                session.cancel_clip_preview();
                refresh_clip(&app, &session);
                return;
            }
            let mut points = session
                .clip_points()
                .map_or_else(Vec::new, |d| d.points.clone());
            if action == 1 {
                points.pop();
            } else {
                points.clear();
            }
            let result = change_clip_points(&app, &mut session, points);
            app.set_message(result.err().unwrap_or_default().into());
            refresh_clip(&app, &session);
        }
    });
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
            if let Some(points) = session.clip_points().map(|d| d.points.clone()) {
                let result = change_clip_points(&app, &mut session, points);
                app.set_message(result.err().unwrap_or_default().into());
                refresh_clip(&app, &session);
                return;
            }
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
            let app = app.unwrap();
            let mut session = lock(&shared);
            if let Some(points) = session.clip_points().map(|d| d.points.clone()) {
                let result = change_clip_points(&app, &mut session, points);
                app.set_message(result.err().unwrap_or_default().into());
            } else {
                session.cancel_clip_preview();
            }
            refresh_clip(&app, &session);
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
    let values = if app.get_transform_mode() >= 3 {
        DVec3::ZERO
    } else {
        input_vector([
            app.get_transform_x(),
            app.get_transform_y(),
            app.get_transform_z(),
        ])?
    };
    let mut transform = edit::Transform {
        texture_lock: app.get_transform_texture_lock(),
        texture_scale_lock: Some(app.get_transform_texture_scale_lock()),
        ..edit::Transform::default()
    };
    match app.get_transform_mode() {
        0 => transform.translation = values,
        1 => transform.rotation = values,
        2 => transform.scale = values,
        3 => {
            transform.reflection = [
                app.get_transform_reflect_x(),
                app.get_transform_reflect_y(),
                app.get_transform_reflect_z(),
            ];
            if !transform.reflection.into_iter().any(|axis| axis) {
                return Err("Choose at least one mirror axis".into());
            }
        }
        4 => {
            transform.shear = Some(map_core::transform::Shear {
                axis: app.get_transform_shear_axis() as usize,
                reference: app.get_transform_shear_reference() as usize,
                factor: app
                    .get_transform_shear_factor()
                    .parse()
                    .map_err(|_| "Enter a numeric shear factor")?,
            });
        }
        _ => return Err("Choose Move, Rotate, Scale, Mirror or Shear".into()),
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
    use map_core::surface::{Alignment, Justify};
    let justify = |index| match index {
        1 => Some((Justify::Start, 0)),
        2 => Some((Justify::Center, 0)),
        3 => Some((Justify::End, 0)),
        _ => None,
    };
    Ok(map_core::surface::SurfaceEdit {
        material: (!window.get_material().trim().is_empty())
            .then(|| window.get_material().trim().to_owned()),
        shift_u: optional_number(window.get_shift_u())?,
        shift_v: optional_number(window.get_shift_v())?,
        scale_u: optional_number(window.get_scale_u())?,
        scale_v: optional_number(window.get_scale_v())?,
        rotation: optional_number(window.get_rotation())?,
        alignment: match window.get_alignment_index() {
            1 => Some(Alignment::World),
            2 => Some(Alignment::Face),
            _ => None,
        },
        justify_u: justify(window.get_justify_u_index()),
        justify_v: justify(window.get_justify_v_index()),
        ..Default::default()
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
    if window.get_operation_index() == 4 {
        return session
            .propagate_face_texture(Actor::User, &target.0, preview)
            .map(|result| result.changed);
    }
    if window.get_operation_index() != 0 {
        use map_core::surface::PaintMode;
        return session.paint_face_texture(
            Actor::User,
            &target.0,
            match window.get_operation_index() {
                1 => PaintMode::Material,
                2 => PaintMode::Projection,
                _ => PaintMode::Wrap,
            },
            preview,
        );
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
    refresh_surface_sample(window, session);
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

fn refresh_surface_sample(window: &SurfaceWindow, session: &mut Session) {
    let sample = session.surface_sample().cloned();
    window.set_sample_present(sample.is_some());
    window.set_sample_label(
        sample
            .as_ref()
            .map(|sample| {
                format!(
                    "{} · brush {}, face {}",
                    sample.material.to_ascii_lowercase(),
                    sample.face.solid,
                    sample.face.side
                )
            })
            .unwrap_or_else(|| {
                "Select one source face and choose Sample face texture first.".into()
            })
            .into(),
    );
    window.set_sample_preview(
        sample
            .and_then(|sample| session.material_preview(&sample.material).ok())
            .map(|preview| preview_image(&preview))
            .unwrap_or_default(),
    );
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
    window.set_operation_index(0);
    window.set_alignment_index(0);
    window.set_justify_u_index(0);
    window.set_justify_v_index(0);
    refresh_surface_sample(&window, &mut session);
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

fn show_connection_window(app: &AppWindow, ui: &SharedUi) {
    let shared = session_of(ui);
    let session = lock(&shared);
    let Some(source) = single_selected_entity(&session) else {
        app.set_message("Select an entity to inspect its connections.".into());
        return;
    };
    if ui.borrow().connection_window.is_none() {
        let window = match ConnectionWindow::new() {
            Ok(window) => window,
            Err(error) => {
                app.set_message(error.to_string().into());
                return;
            }
        };
        connection_ui::bind(&window, &shared);
        ui.borrow_mut().connection_window = Some(window);
    }
    let window = ui
        .borrow()
        .connection_window
        .as_ref()
        .unwrap()
        .clone_strong();
    window.set_source_id(id_string(ObjectId::Entity(source)).into());
    connection_ui::refresh(&window, &session);
    if let Err(error) = window.show() {
        app.set_message(error.to_string().into());
    }
}

fn show_properties_window(app: &AppWindow, ui: &SharedUi, world: bool) {
    let shared = session_of(ui);
    let mut session = lock(&shared);
    let target = if world {
        session::PropertyTarget::World
    } else {
        let ids = session
            .selected_ids()
            .into_iter()
            .map(|id| match id {
                ObjectId::Entity(id) => Ok(id),
                _ => Err("Select entities to edit their common properties."),
            })
            .collect::<Result<Vec<_>, _>>();
        match ids {
            Ok(ids) => session::PropertyTarget::Entities(ids),
            Err(error) => {
                app.set_message(error.into());
                return;
            }
        }
    };
    if ui.borrow().properties_window.is_none() {
        let result = PropertiesWindow::new().and_then(|window| {
            management_ui::bind_properties(&window, &shared)?;
            Ok(window)
        });
        match result {
            Ok(window) => ui.borrow_mut().properties_window = Some(window),
            Err(error) => {
                app.set_message(error.to_string().into());
                return;
            }
        }
    }
    if session
        .property_draft()
        .is_none_or(|draft| draft.target != target)
        && let Err(error) = session.begin_property_edit(target)
    {
        app.set_message(error.into());
        return;
    }
    let window = ui
        .borrow()
        .properties_window
        .as_ref()
        .unwrap()
        .clone_strong();
    management_ui::refresh_properties(&window, &session);
    if let Err(error) = window.show() {
        app.set_message(error.to_string().into());
    }
}

fn show_report_window(app: &AppWindow, ui: &SharedUi) {
    let shared = session_of(ui);
    if ui.borrow().report_window.is_none() {
        match ReportWindow::new() {
            Ok(window) => {
                management_ui::bind_report(&window, &shared);
                ui.borrow_mut().report_window = Some(window);
            }
            Err(error) => {
                app.set_message(error.to_string().into());
                return;
            }
        }
    }
    let window = ui.borrow().report_window.as_ref().unwrap().clone_strong();
    window.set_aspect((app.get_viewport_width() / app.get_viewport_height().max(1.0)).max(0.01));
    management_ui::refresh_report(&window, &lock(&shared));
    if let Err(error) = window.show() {
        app.set_message(error.to_string().into());
    }
}

fn show_rename_window(app: &AppWindow, ui: &SharedUi, name: Option<String>) {
    let shared = session_of(ui);
    let mut session = lock(&shared);
    let Some(entity) = single_selected_entity(&session) else {
        app.set_message("Select an entity to inspect its references.".into());
        return;
    };
    if ui.borrow().rename_window.is_none() {
        let window = match RenameWindow::new() {
            Ok(window) => window,
            Err(error) => {
                app.set_message(error.to_string().into());
                return;
            }
        };
        rename_ui::bind(&window, &shared);
        ui.borrow_mut().rename_window = Some(window);
    }
    let begin = if name.is_none()
        && session
            .rename_draft()
            .is_some_and(|draft| draft.entity == entity)
    {
        Ok(())
    } else {
        session.begin_entity_rename(entity)
    };
    let result = begin.and_then(|()| {
        if let Some(name) = name {
            session.update_entity_rename(name, false, false)
        } else {
            Ok(())
        }
    });
    if let Err(error) = result {
        app.set_message(error.into());
        return;
    }
    let window = ui.borrow().rename_window.as_ref().unwrap().clone_strong();
    rename_ui::refresh(&window, &session);
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
    window.set_favorites_status(
        catalog
            .favorites
            .error
            .as_deref()
            .unwrap_or(match catalog.favorites.persistence {
                material_favorites::Persistence::Session => "Favorites last for this session",
                material_favorites::Persistence::Persistent => "Favorites saved across sessions",
                material_favorites::Persistence::Blocked => "Favorites storage unavailable",
            })
            .into(),
    );
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
                } else if let Some(replacement) = ui.borrow().material_for_replace {
                    if let Some(controller) = &ui.borrow().material_replace {
                        controller.choose_material(&mut session, replacement, &material);
                    }
                } else if ui.borrow().material_for_clip {
                    app.set_clip_material(material);
                    if let Some(points) = session.clip_points().map(|d| d.points.clone()) {
                        if let Err(error) = change_clip_points(&app, &mut session, points) {
                            window.set_detail(error.into());
                            return;
                        }
                    } else {
                        session.cancel_clip_preview();
                    }
                    refresh_clip(&app, &session);
                } else if ui.borrow().material_for_primitive {
                    if let Some(primitive) = &ui.borrow().primitive_window {
                        let Some(target) = ui.borrow().primitive_material_target else {
                            return;
                        };
                        if let Err(error) = primitive_ui::choose_material(
                            primitive,
                            &mut session,
                            target,
                            &material,
                        ) {
                            window.set_detail(error.into());
                            return;
                        }
                    }
                } else if ui.borrow().material_for_partition {
                    if let Some(components) = &ui.borrow().components_window {
                        let Some(target) = ui.borrow().partition_material_target.clone() else {
                            return;
                        };
                        if let Err(error) = components_ui::choose_partition_material(
                            components,
                            &mut session,
                            &target,
                            Some(material.as_str()),
                        ) {
                            window.set_detail(error.into());
                            return;
                        }
                    }
                } else if ui.borrow().material_for_extrusion {
                    if let Some(components) = &ui.borrow().components_window {
                        let Some(target) = ui.borrow().extrusion_material_target else {
                            return;
                        };
                        if let Err(error) = components_ui::choose_extrusion_material(
                            components,
                            &mut session,
                            target,
                            &material,
                        ) {
                            window.set_detail(error.into());
                            return;
                        }
                    }
                } else if ui.borrow().material_for_group {
                    if let Some(controller) = &ui.borrow().surface_group {
                        controller.choose_material(&mut session, &material);
                    }
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
    ui.borrow_mut().material_for_replace = match key {
        "@replace-source" => Some(false),
        "@replace-target" => Some(true),
        _ => None,
    };
    ui.borrow_mut().material_for_group = key == "@group";
    ui.borrow_mut().material_for_faces = key == "@faces";
    ui.borrow_mut().material_for_clip = key == "@clip";
    ui.borrow_mut().material_for_primitive = key == "@primitive";
    ui.borrow_mut().material_for_extrusion = key == "@extrusion";
    ui.borrow_mut().material_for_partition = key == "@partition";
    ui.borrow_mut().partition_material_target =
        (key == "@partition").then(|| components_ui::partition_material_target(&session));
    ui.borrow_mut().extrusion_material_target = (key == "@extrusion").then(|| {
        (
            session.partition_preview_token(),
            session.document_generation(),
        )
    });
    ui.borrow_mut().primitive_material_target = (key == "@primitive").then(|| {
        (
            session.primitive_preview().map(|preview| preview.token),
            session.document_generation(),
        )
    });
    let target = if key.is_empty()
        || key == "@faces"
        || key == "@clip"
        || key == "@primitive"
        || key == "@partition"
        || key == "@extrusion"
        || key.starts_with("@replace-")
        || key == "@group"
    {
        None
    } else {
        single_selected_entity(&session)
            .map(|id| (id, key.to_owned(), session.document_generation()))
    };
    let material = if let Some(replacement) = ui.borrow().material_for_replace {
        ui.borrow()
            .material_replace
            .as_ref()
            .map(|controller| {
                if replacement {
                    controller.window.get_replacement().to_string()
                } else {
                    controller.window.get_source().to_string()
                }
            })
            .unwrap_or_default()
    } else if key == "@partition" {
        ui.borrow()
            .components_window
            .as_ref()
            .map(|window| window.get_top_material().to_string())
            .unwrap_or_default()
    } else if key == "@extrusion" {
        ui.borrow()
            .components_window
            .as_ref()
            .map(|window| window.get_wall_material().to_string())
            .unwrap_or_default()
    } else if key == "@primitive" {
        ui.borrow()
            .primitive_window
            .as_ref()
            .map(|window| window.get_material().to_string())
            .unwrap_or_default()
    } else if key == "@group" {
        ui.borrow()
            .surface_group
            .as_ref()
            .map(|controller| controller.window.get_material().to_string())
            .unwrap_or_default()
    } else if key == "@clip" {
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
    app.set_can_sample_face(session.selected_faces().len() == 1);
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
    let shared = session_of(ui);
    let (dirty, generation, revision, partition) = {
        let session = lock(&shared);
        (
            session.document().is_dirty() || session.partition_preview_token().is_some(),
            session.document_generation(),
            session.document().revision(),
            session.partition_preview_token(),
        )
    };
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
            let mut session = lock(&shared);
            if session.document_generation() != generation
                || session.document().revision() != revision
                || session.partition_preview_token() != partition
            {
                session.log(
                    Actor::System,
                    "Map changed while confirming discard. Try again.",
                );
                return;
            }
            drop(session);
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
    if let Err(error) = mcp::start_compile(&session, Actor::User, path, play) {
        app.set_message(error.into());
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
    let object_shortcut = if control {
        key == "d"
    } else {
        [
            Key::Delete,
            Key::Backspace,
            Key::UpArrow,
            Key::DownArrow,
            Key::LeftArrow,
            Key::RightArrow,
            Key::PageUp,
            Key::PageDown,
        ]
        .iter()
        .any(|value| key == key_text(*value))
    };
    let components_active = {
        let shared = session_of(ui);
        let session = lock(&shared);
        session.component_picking().is_some() || !session.component_selection().is_empty()
    };
    if object_shortcut && components_active {
        app.set_message("Use Reshape brushes or its move handles. Finish component editing before editing whole objects.".into());
        return true;
    }
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
            if let Some(token) = session.clip_drag().map(|d| d.token) {
                let _ = session.cancel_clip_drag(token);
                ui.interaction.cancel(&mut session);
                refresh_clip(app, &session);
                return true;
            }
            if session.component_picking().is_some() || !session.component_selection().is_empty() {
                ui.interaction.cancel(&mut session);
                session.clear_component_edit();
                refresh_interaction(app, &session);
                return true;
            }
            if session.volume_preview().is_some() {
                session.clear_volume_selection();
                return true;
            }
            if ui.interaction.active()
                || session.transform_preview().is_some()
                || session.transform_drag().is_some()
                || session.area_preview().is_some()
                || session.paint_stroke().is_some()
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
        if session.primitive_preview().is_some() {
            session.clear_primitive_preview();
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
    let session = Arc::new(Mutex::new(Session::new(mount_game(args.gameinfo))));
    lock(&session).configure_recovery(recovery::default_directory());
    let _recovery_worker = recovery::Worker::start(&session)?;
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
    if args.headless {
        if let Some(Err(message)) = &open_message {
            eprintln!("{message}");
        }
        return Ok(mcp::serve_stdio(EditorServer::new(session.clone(), None))?);
    }

    let ctx = GpuContext::new()?;
    let server = EditorServer::new(session.clone(), Some(ctx.clone()));

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
        connection_window: None,
        seen_connections: (0, 0, 0),
        properties_window: None,
        report_window: None,
        statistics_window: None,
        pointfile_window: None,
        components_window: None,
        hollow_window: None,
        carve_window: None,
        recovery_window: None,
        checks_window: None,
        context_window: None,
        primitive_window: None,
        seen_primitive: 0,
        seen_context: (0, 0),
        seen_checks: (0, 0, 0),
        seen_recovery: (0, 0),
        seen_management: (0, 0, 0),
        rename_window: None,
        seen_rename: (0, 0, 0),
        color_window: None,
        color_target: None,
        material_window: None,
        material_target: None,
        surface_window: None,
        surface_target: None,
        lighting: None,
        material_replace: None,
        surface_group: None,
        volume: None,
        compile_status: None,
        saved_views: None,
        material_for_group: false,
        material_for_replace: None,
        material_for_faces: false,
        material_for_clip: false,
        material_for_primitive: false,
        material_for_extrusion: false,
        material_for_partition: false,
        partition_material_target: None,
        extrusion_material_target: None,
        primitive_material_target: None,
        seen_definitions: 0,
    }));

    app.window().on_close_requested({
        let ui = ui.clone();
        move || {
            confirm_discard(&ui, || {
                if let Err(error) = slint::quit_event_loop() {
                    eprintln!("could not close editor: {error}");
                }
            });
            slint::CloseRequestResponse::KeepWindowShown
        }
    });

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
    app.on_saved_views({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            let mut ui = ui.borrow_mut();
            if ui.saved_views.is_none() {
                match saved_views_ui::Controller::new(&ui.session) {
                    Ok(controller) => ui.saved_views = Some(controller),
                    Err(error) => {
                        app.unwrap().set_message(error.to_string().into());
                        return;
                    }
                }
            }
            if let Err(error) = ui.saved_views.as_ref().unwrap().show(&lock(&ui.session)) {
                app.unwrap().set_message(error.to_string().into());
            }
        }
    });
    app.on_build_status({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            let mut ui = ui.borrow_mut();
            if ui.compile_status.is_none() {
                match compile_ui::Controller::new(&ui.session) {
                    Ok(controller) => ui.compile_status = Some(controller),
                    Err(error) => {
                        app.unwrap().set_message(error.to_string().into());
                        return;
                    }
                }
            }
            if let Err(error) = ui.compile_status.as_ref().unwrap().show(&lock(&ui.session)) {
                app.unwrap().set_message(error.to_string().into());
            }
        }
    });
    app.on_cancel_build({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            let session = session_of(&ui);
            let mut session = lock(&session);
            if let Ok(token) = app.unwrap().get_compile_token().parse() {
                report(
                    &app.unwrap(),
                    session
                        .cancel_compile(Actor::User, token)
                        .map(|()| "Cancelling compilation".into()),
                );
            }
        }
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
    app.on_world_settings({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || show_properties_window(&app.unwrap(), &ui, true)
    });
    app.on_batch_properties({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || show_properties_window(&app.unwrap(), &ui, false)
    });
    app.on_object_report({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || show_report_window(&app.unwrap(), &ui)
    });
    app.on_hollow_brushes({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            if ui.borrow().hollow_window.is_none() {
                match HollowWindow::new() {
                    Ok(window) => {
                        hollow_ui::bind(&window, &session_of(&ui));
                        ui.borrow_mut().hollow_window = Some(window);
                    }
                    Err(error) => {
                        app.unwrap().set_message(error.to_string().into());
                        return;
                    }
                }
            }
            let window = ui.borrow().hollow_window.as_ref().unwrap().clone_strong();
            hollow_ui::refresh(&window, &lock(&session_of(&ui)));
            if let Err(error) = window.show() {
                app.unwrap().set_message(error.to_string().into());
            }
        }
    });
    app.on_carve_brushes({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            if ui.borrow().carve_window.is_none() {
                match CarveWindow::new() {
                    Ok(window) => {
                        carve_ui::bind(&window, &session_of(&ui));
                        ui.borrow_mut().carve_window = Some(window);
                    }
                    Err(error) => {
                        app.unwrap().set_message(error.to_string().into());
                        return;
                    }
                }
            }
            let window = ui.borrow().carve_window.as_ref().unwrap().clone_strong();
            carve_ui::refresh(&window, &lock(&session_of(&ui)));
            if let Err(error) = window.show() {
                app.unwrap().set_message(error.to_string().into());
            }
        }
    });
    app.on_brush_components({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            if ui.borrow().components_window.is_none() {
                match ComponentsWindow::new() {
                    Ok(window) => {
                        components_ui::bind(&window, &session_of(&ui));
                        window.on_browse_partition_material({
                            let (app, ui) = (app.clone(), ui.clone());
                            move || show_material_window(&app.unwrap(), &ui, "@partition")
                        });
                        window.on_browse_extrusion_material({
                            let (app, ui) = (app.clone(), ui.clone());
                            move || show_material_window(&app.unwrap(), &ui, "@extrusion")
                        });
                        ui.borrow_mut().components_window = Some(window);
                    }
                    Err(error) => {
                        app.unwrap().set_message(error.to_string().into());
                        return;
                    }
                }
            }
            let window = ui
                .borrow()
                .components_window
                .as_ref()
                .unwrap()
                .clone_strong();
            components_ui::refresh(&window, &lock(&session_of(&ui)));
            window.set_aspect(
                (app.unwrap().get_viewport_width() / app.unwrap().get_viewport_height().max(1.0))
                    .max(0.01),
            );
            if let Err(error) = window.show() {
                app.unwrap().set_message(error.to_string().into());
            }
        }
    });
    app.on_pointfile({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            if ui.borrow().pointfile_window.is_none() {
                match PointfileWindow::new() {
                    Ok(window) => {
                        pointfile_ui::bind(&window, &session_of(&ui));
                        ui.borrow_mut().pointfile_window = Some(window);
                    }
                    Err(error) => {
                        app.unwrap().set_message(error.to_string().into());
                        return;
                    }
                }
            }
            let app = app.unwrap();
            let window = ui
                .borrow()
                .pointfile_window
                .as_ref()
                .unwrap()
                .clone_strong();
            window.set_aspect(
                (app.get_viewport_width() / app.get_viewport_height().max(1.0)).max(0.01),
            );
            pointfile_ui::refresh(&window, &lock(&session_of(&ui)));
            if let Err(error) = window.show() {
                app.set_message(error.to_string().into());
            }
        }
    });
    app.on_map_statistics({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            if ui.borrow().statistics_window.is_none() {
                match StatisticsWindow::new() {
                    Ok(window) => {
                        statistics_ui::bind(&window, &session_of(&ui));
                        ui.borrow_mut().statistics_window = Some(window);
                    }
                    Err(error) => {
                        app.unwrap().set_message(error.to_string().into());
                        return;
                    }
                }
            }
            let window = ui
                .borrow()
                .statistics_window
                .as_ref()
                .unwrap()
                .clone_strong();
            statistics_ui::refresh(&window, &lock(&session_of(&ui)));
            if let Err(error) = window.show() {
                app.unwrap().set_message(error.to_string().into());
            }
        }
    });
    app.on_map_checks({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            let app = app.unwrap();
            if ui.borrow().checks_window.is_none() {
                match ChecksWindow::new() {
                    Ok(window) => {
                        checks_ui::bind(&window, &session_of(&ui));
                        ui.borrow_mut().checks_window = Some(window);
                    }
                    Err(error) => {
                        app.set_message(error.to_string().into());
                        return;
                    }
                }
            }
            let window = ui.borrow().checks_window.as_ref().unwrap().clone_strong();
            window.set_aspect(
                (app.get_viewport_width() / app.get_viewport_height().max(1.0)).max(0.01),
            );
            window.set_message("".into());
            checks_ui::refresh(&window, &lock(&session_of(&ui)));
            if let Err(error) = window.show() {
                app.set_message(error.to_string().into());
            }
        }
    });
    app.on_brush_shapes({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            let app = app.unwrap();
            if ui.borrow().primitive_window.is_none() {
                match PrimitiveWindow::new() {
                    Ok(window) => {
                        primitive_ui::bind(&window, &session_of(&ui));
                        window.on_browse_material({
                            let (app, ui) = (app.as_weak(), ui.clone());
                            move || show_material_window(&app.unwrap(), &ui, "@primitive")
                        });
                        ui.borrow_mut().primitive_window = Some(window);
                    }
                    Err(error) => {
                        app.set_message(error.to_string().into());
                        return;
                    }
                }
            }
            let window = ui
                .borrow()
                .primitive_window
                .as_ref()
                .unwrap()
                .clone_strong();
            window.set_aspect(
                (app.get_viewport_width() / app.get_viewport_height().max(1.0)).max(0.01),
            );
            primitive_ui::initialize(&window, &mut lock(&session_of(&ui)), &app.get_material());
            if let Err(error) = window.show() {
                app.set_message(error.to_string().into());
            }
        }
    });
    app.on_design_context({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            let app = app.unwrap();
            if ui.borrow().context_window.is_none() {
                match ContextWindow::new() {
                    Ok(window) => {
                        context_ui::bind(&window, &session_of(&ui));
                        ui.borrow_mut().context_window = Some(window);
                    }
                    Err(error) => {
                        app.set_message(error.to_string().into());
                        return;
                    }
                }
            }
            let window = ui.borrow().context_window.as_ref().unwrap().clone_strong();
            window.set_aspect(
                (app.get_viewport_width() / app.get_viewport_height().max(1.0)).max(0.01),
            );
            window.set_message("".into());
            context_ui::refresh(&window, &lock(&session_of(&ui)));
            if let Err(error) = window.show() {
                app.set_message(error.to_string().into());
            }
        }
    });
    app.on_recovery({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            let app = app.unwrap();
            if ui.borrow().recovery_window.is_none() {
                match RecoveryWindow::new() {
                    Ok(window) => {
                        recovery_ui::bind(&window, &session_of(&ui));
                        ui.borrow_mut().recovery_window = Some(window);
                    }
                    Err(error) => {
                        app.set_message(error.to_string().into());
                        return;
                    }
                }
            }
            let window = ui.borrow().recovery_window.as_ref().unwrap().clone_strong();
            window.set_settings_dirty(false);
            window.set_discard_changes(false);
            window.set_confirm_delete(false);
            window.set_message("".into());
            recovery_ui::refresh(&window, &lock(&session_of(&ui)));
            if let Err(error) = window.show() {
                app.set_message(error.to_string().into());
            }
        }
    });
    app.on_edit_connections({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || show_connection_window(&app.unwrap(), &ui)
    });
    app.on_edit_face_textures({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || show_surface_window(&app.unwrap(), &ui)
    });
    app.on_select_volume({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            let shared = session_of(&ui);
            if ui.borrow().volume.is_none() {
                let viewport = app.clone();
                match volume_ui::Controller::new(&shared, move || {
                    let app = viewport.unwrap();
                    (app.get_viewport_width() / app.get_viewport_height().max(1.0)).max(0.01)
                }) {
                    Ok(controller) => ui.borrow_mut().volume = Some(controller),
                    Err(error) => {
                        app.unwrap().set_message(error.to_string().into());
                        return;
                    }
                }
            }
            let ui = ui.borrow();
            let controller = ui.volume.as_ref().unwrap();
            controller.initialize(&mut lock(&shared));
            let _ = controller.window.show();
        }
    });
    app.on_map_face_group({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            let shared = session_of(&ui);
            if ui.borrow().surface_group.is_none() {
                match surface_group_ui::Controller::new(&shared) {
                    Ok(controller) => {
                        controller.window.on_browse({
                            let (app, ui) = (app.clone(), ui.clone());
                            move || show_material_window(&app.unwrap(), &ui, "@group")
                        });
                        ui.borrow_mut().surface_group = Some(controller);
                    }
                    Err(error) => {
                        app.unwrap().set_message(error.to_string().into());
                        return;
                    }
                }
            }
            let ui = ui.borrow();
            let controller = ui.surface_group.as_ref().unwrap();
            match controller.initialize(&mut lock(&shared)) {
                Ok(()) => {
                    let _ = controller.window.show();
                }
                Err(error) => app.unwrap().set_message(error.into()),
            }
        }
    });
    app.on_replace_materials({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            let shared = session_of(&ui);
            if ui.borrow().material_replace.is_none() {
                match material_replace_ui::Controller::new(&shared) {
                    Ok(controller) => {
                        controller.window.on_browse({
                            let (app, ui) = (app.clone(), ui.clone());
                            move |replacement| {
                                show_material_window(
                                    &app.unwrap(),
                                    &ui,
                                    if replacement {
                                        "@replace-target"
                                    } else {
                                        "@replace-source"
                                    },
                                )
                            }
                        });
                        ui.borrow_mut().material_replace = Some(controller);
                    }
                    Err(error) => {
                        app.unwrap().set_message(error.to_string().into());
                        return;
                    }
                }
            }
            let ui = ui.borrow();
            let controller = ui.material_replace.as_ref().unwrap();
            controller.initialize(&mut lock(&shared), &app.unwrap().get_material());
            let _ = controller.window.show();
        }
    });
    app.on_edit_face_lighting({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            let shared = session_of(&ui);
            if ui.borrow().lighting.is_none() {
                match lighting_ui::Controller::new(&shared) {
                    Ok(controller) => ui.borrow_mut().lighting = Some(controller),
                    Err(error) => {
                        app.unwrap().set_message(error.to_string().into());
                        return;
                    }
                }
            }
            let ui = ui.borrow();
            let controller = ui.lighting.as_ref().unwrap();
            let result = controller.initialize(&mut lock(&shared));
            match result {
                Ok(()) => {
                    let _ = controller.window.show();
                }
                Err(error) => app.unwrap().set_message(error.into()),
            }
        }
    });
    app.on_sample_face_texture({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            let app = app.unwrap();
            let shared = session_of(&ui);
            let mut session = lock(&shared);
            let result = match session.selected_faces().as_slice() {
                [face] => session.sample_face_texture(*face),
                _ => Err("Select exactly one source face.".into()),
            };
            app.set_message(match result {
                Ok(()) => {
                    "Texture sampled. Choose Paint texture or open Edit face textures.".into()
                }
                Err(error) => error.into(),
            });
        }
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
    app.on_edit_references({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || show_rename_window(&app.unwrap(), &ui, None)
    });
    app.on_property_edited({
        let (app, ui) = (app.as_weak(), ui.clone());
        move |key, value| {
            let key = key.trim().to_owned();
            if key.is_empty() {
                return;
            }
            if key.eq_ignore_ascii_case("targetname") {
                show_rename_window(&app.unwrap(), &ui, Some(value.to_string()));
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
    app.on_surface_view_changed({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            let app = app.unwrap();
            let mode = render::SurfaceView::from_index(app.get_surface_view()).unwrap_or_default();
            let result = lock(&session_of(&ui))
                .set_surface_view(mode, app.get_smoothing_view_group() as u32);
            if let Err(error) = result {
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
                let pointer = session::Pointer {
                    position: glam::DVec2::new(f64::from(x), f64::from(y)),
                    size: (
                        viewport_size.0.round().max(1.0) as u32,
                        viewport_size.1.round().max(1.0) as u32,
                    ),
                };
                if session.clip_points().is_some()
                    || (down && session.pick_clip_handle(pointer).is_some())
                    || session.clip_drag().is_some()
                    || (!down && ui.interaction.active() && session.clip_tool())
                {
                    session.publish_camera(ui.viewport.camera);
                    let result = if down {
                        ui.interaction.down(&mut session, pointer, control)
                    } else {
                        ui.interaction.up(&mut session, pointer)
                    };
                    if let Err(error) = result {
                        app.set_message(error.into());
                    }
                    refresh_clip(&app, &session);
                    return;
                }
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
                    if drag.epoch != session.clip_epoch() {
                        return;
                    }
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
                if drag.epoch != session.clip_epoch() {
                    ui.clip_drag = None;
                    return;
                }
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
                if let Some(active) = &mut ui.clip_drag {
                    active.epoch = session.clip_epoch();
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
            if session.clip_tool() {
                refresh_clip(&app, &session);
            }
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
                app.set_compile_label(session.compile_label().into());
                app.set_compile_token(
                    session
                        .compile_token()
                        .map(|token| token.to_string())
                        .unwrap_or_default()
                        .into(),
                );
                app.set_compile_cancellable(session.compile_cancellable());
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
                    if let Some(controller) = &ui.volume
                        && controller.window.window().is_visible()
                    {
                        controller.refresh(&session);
                    }
                    if let Some(controller) = &ui.surface_group
                        && controller.window.window().is_visible()
                    {
                        controller.refresh(&session);
                    }
                    if let Some(controller) = &ui.material_replace
                        && controller.window.window().is_visible()
                    {
                        controller.refresh(&session);
                    }
                    if let Some(controller) = &ui.lighting
                        && controller.window.window().is_visible()
                    {
                        controller.refresh(&session);
                    }
                    if let Some(window) = &ui.surface_window
                        && window.window().is_visible()
                        && let Some(target) = &ui.surface_target
                    {
                        refresh_surface_dialog(window, &mut session, target);
                    }
                    app.set_show_tools(session.view().tool_textures);
                    app.set_surface_view(session.view().surface.index());
                    app.set_smoothing_view_group(session.view().smoothing_group as i32);
                    app.set_show_leak(session.view().leak_path);
                    panel_dirty = true;
                }
                if session.view_serial() != ui.seen_view {
                    ui.seen_view = session.view_serial();
                    ui.viewport.options = session.view();
                    ui.viewport.redraw();
                    refresh_spatial(&app, &session);
                    refresh_texture_brush_sample(&app, &mut session);
                    if let Some(controller) = &ui.volume
                        && controller.window.window().is_visible()
                    {
                        controller.refresh(&session);
                    }
                    if let Some(controller) = &ui.surface_group
                        && controller.window.window().is_visible()
                    {
                        controller.refresh(&session);
                    }
                    if let Some(controller) = &ui.material_replace
                        && controller.window.window().is_visible()
                    {
                        controller.refresh(&session);
                    }
                    if let Some(controller) = &ui.lighting
                        && controller.window.window().is_visible()
                    {
                        controller.refresh(&session);
                    }
                    if let Some(window) = &ui.surface_window
                        && window.window().is_visible()
                    {
                        refresh_surface_sample(window, &mut session);
                    }
                    if let Some(window) = &ui.material_window
                        && window.window().is_visible()
                    {
                        refresh_material_catalog(window, &mut session);
                    }
                    app.set_show_tools(session.view().tool_textures);
                    app.set_surface_view(session.view().surface.index());
                    app.set_smoothing_view_group(session.view().smoothing_group as i32);
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
                let connections = (
                    session.connection_serial(),
                    session.scene_serial(),
                    session.definitions_serial(),
                );
                if connections != ui.seen_connections {
                    ui.seen_connections = connections;
                    if let Some(window) = &ui.connection_window
                        && window.window().is_visible()
                    {
                        connection_ui::refresh(window, &session);
                    }
                }
                let management = (
                    session.management_serial(),
                    session.scene_serial(),
                    session.definitions_serial(),
                );
                let checks = (
                    session.map_check_serial(),
                    session.scene_serial(),
                    session.definitions_serial(),
                );
                if checks != ui.seen_checks {
                    ui.seen_checks = checks;
                    if let Some(window) = &ui.checks_window
                        && window.window().is_visible()
                    {
                        checks_ui::refresh(window, &session);
                    }
                }
                if session.primitive_serial() != ui.seen_primitive {
                    ui.seen_primitive = session.primitive_serial();
                    if let Some(window) = &ui.primitive_window
                        && window.window().is_visible()
                    {
                        primitive_ui::refresh(window, &mut session);
                    }
                }
                let context = (session.context_serial(), session.scene_serial());
                if context != ui.seen_context {
                    ui.seen_context = context;
                    if let Some(window) = &ui.context_window
                        && window.window().is_visible()
                    {
                        context_ui::refresh(window, &session);
                    }
                }
                let recovery = (session.recovery_serial(), session.scene_serial());
                if recovery != ui.seen_recovery {
                    ui.seen_recovery = recovery;
                    if let Some(window) = &ui.recovery_window
                        && window.window().is_visible()
                    {
                        recovery_ui::refresh(window, &session);
                    }
                }
                if management != ui.seen_management {
                    ui.seen_management = management;
                    if let Some(window) = &ui.properties_window
                        && window.window().is_visible()
                    {
                        management_ui::refresh_properties(window, &session);
                    }
                    if let Some(window) = &ui.report_window
                        && window.window().is_visible()
                    {
                        management_ui::refresh_report(window, &session);
                    }
                }
                let rename = (
                    session.rename_serial(),
                    session.scene_serial(),
                    session.definitions_serial(),
                );
                if rename != ui.seen_rename {
                    ui.seen_rename = rename;
                    if let Some(window) = &ui.rename_window
                        && window.window().is_visible()
                    {
                        rename_ui::refresh(window, &session);
                    }
                }
                if panel_dirty {
                    refresh_panel(&app, &mut session);
                }
            }

            if now.duration_since(ui.last_log_refresh) >= Duration::from_millis(500) {
                ui.last_log_refresh = now;
                if let Some(window) = &ui.hollow_window
                    && window.window().is_visible()
                {
                    hollow_ui::refresh_changed(window, &lock(&ui.session));
                }
                if let Some(window) = &ui.carve_window
                    && window.window().is_visible()
                {
                    carve_ui::refresh_changed(window, &lock(&ui.session));
                }
                if let Some(window) = &ui.components_window
                    && window.window().is_visible()
                {
                    window.set_aspect(
                        (app.get_viewport_width() / app.get_viewport_height().max(1.0)).max(0.01),
                    );
                    components_ui::refresh_changed(window, &lock(&ui.session));
                }
                if let Some(window) = &ui.pointfile_window
                    && window.window().is_visible()
                {
                    window.set_aspect(
                        (app.get_viewport_width() / app.get_viewport_height().max(1.0)).max(0.01),
                    );
                    pointfile_ui::refresh_changed(window, &lock(&ui.session));
                }
                if let Some(window) = &ui.statistics_window
                    && window.window().is_visible()
                {
                    statistics_ui::refresh_changed(window, &lock(&ui.session));
                }
                if let Some(controller) = &ui.saved_views
                    && controller.window.window().is_visible()
                {
                    controller.refresh(&lock(&ui.session));
                }
                if let Some(controller) = &ui.compile_status
                    && controller.window.window().is_visible()
                {
                    controller.refresh(&lock(&ui.session));
                }
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
        app.set_rotation_step("15".into());
        app.set_handles_mode_index(4);
        app.set_paint_radius(24.0);
        app.invoke_transform_tool_changed();
        assert!(lock(&shared).paint_enabled());
        assert_eq!(lock(&shared).paint_radius(), 24.0);
        app.set_paint_radius(32.0);
        app.invoke_paint_settings_changed();
        assert_eq!(lock(&shared).paint_radius(), 32.0);
        lock(&shared).select(&[], SelectMode::Replace).unwrap();
        for apply in [false, true] {
            let mut state = lock(&shared);
            let spec = session::paint::Spec {
                camera: state.camera(),
                pointer: Pointer {
                    position: DVec2::new(400.0, 300.0),
                    size: (800, 600),
                },
                radius: state.paint_radius(),
                mode: SelectMode::Replace,
                faces: false,
            };
            let old = state.begin_paint_selection(spec).unwrap();
            refresh_interaction(&app, &state);
            let current = state.begin_paint_selection(spec).unwrap();
            drop(state);
            app.invoke_cancel_interaction();
            assert_eq!(lock(&shared).paint_stroke().unwrap().token, current);
            assert_ne!(old, current);
            refresh_interaction(&app, &lock(&shared));
            save_ui_snapshot(app.window(), "paint-selection-controls");
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
            assert!(!app.get_has_paint_preview());
            assert_eq!(
                lock(&shared).selected_ids(),
                if apply { vec![id] } else { vec![] }
            );
            assert_eq!(lock(&shared).document().vmf(), &original);
            assert_eq!(lock(&shared).document().undo_labels().count(), 1);
        }
    }
    #[test]
    fn texture_brush_controls_preview_apply_and_cancel_without_desktop_input() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let mut state = Session::new(Err("no assets needed".into()));
        state
            .create_block(
                Actor::Agent,
                DVec3::ZERO,
                DVec3::splat(64.0),
                "brick/test",
                false,
            )
            .unwrap();
        let ObjectId::Solid(source) = state
            .create_block(
                Actor::Agent,
                DVec3::new(256.0, 0.0, 0.0),
                DVec3::new(320.0, 64.0, 64.0),
                "metal/sample",
                false,
            )
            .unwrap()
        else {
            unreachable!()
        };
        let side = state
            .document()
            .solid(source)
            .unwrap()
            .sides
            .iter()
            .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
            .unwrap()
            .id;
        let face = map_core::surface::FaceId {
            solid: source,
            side,
        };
        state.sample_face_texture(face).unwrap();
        state.select_faces(&[face], SelectMode::Replace).unwrap();
        state.set_camera(render::Camera::planar(
            Projection::Top,
            Vec3::splat(32.0),
            256.0,
        ));
        let shared = Arc::new(Mutex::new(state));
        let app = AppWindow::new().unwrap();
        bind_manipulation_controls(&app, shared.clone());
        refresh_panel(&app, &mut lock(&shared));
        refresh_texture_brush_sample(&app, &mut lock(&shared));
        app.show().unwrap();
        app.window().set_size(slint::PhysicalSize::new(1600, 960));
        app.set_handles_mode_index(5);
        app.set_texture_paint_mode(1);
        app.invoke_transform_tool_changed();
        assert_eq!(
            lock(&shared).paint_operation(),
            session::paint::Operation::Projection
        );
        assert_eq!(app.get_texture_sample_material(), "metal/sample");
        for apply in [false, true] {
            let mut state = lock(&shared);
            let before = state.document().vmf().clone();
            let history = state.document().undo_labels().count();
            let spec = session::paint::Spec {
                camera: state.camera(),
                pointer: Pointer {
                    position: DVec2::new(400.0, 300.0),
                    size: (800, 600),
                },
                radius: 12.0,
                mode: SelectMode::Replace,
                faces: true,
            };
            let old = state
                .begin_texture_stroke(spec, session::paint::Operation::Projection)
                .unwrap();
            refresh_interaction(&app, &state);
            let token = state
                .begin_texture_stroke(spec, session::paint::Operation::Projection)
                .unwrap();
            let preview = state.render_document().vmf().clone();
            drop(state);
            app.invoke_apply_area_selection();
            assert_ne!(old, token);
            assert_eq!(lock(&shared).paint_stroke().unwrap().token, token);
            refresh_interaction(&app, &lock(&shared));
            save_ui_snapshot(app.window(), "texture-stroke-controls");
            for label in ["Apply textures", "Cancel"] {
                assert_dialog_button_visible(&app, label);
            }
            i_slint_backend_testing::ElementHandle::find_by_accessible_label(
                &app,
                if apply { "Apply textures" } else { "Cancel" },
            )
            .next()
            .unwrap()
            .invoke_accessible_default_action();
            assert!(lock(&shared).paint_stroke().is_none());
            assert_eq!(
                lock(&shared).document().vmf(),
                if apply { &preview } else { &before }
            );
            assert_eq!(lock(&shared).selected_faces(), vec![face]);
            assert_eq!(
                lock(&shared).document().undo_labels().count(),
                history + usize::from(apply)
            );
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
            assert!(button.absolute_position().y + button.size().height <= 342.5);
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
        app.invoke_start_clip();
        app.invoke_clip_points_action(0);
        assert!(app.get_clip_point_mode());
        let points = vec![
            DVec3::new(64.0, 0.0, 0.0),
            DVec3::new(64.0, 96.0, 0.0),
            DVec3::new(64.0, 96.0, 64.0),
        ];
        change_clip_points(&app, &mut lock(&shared), points.clone()).unwrap();
        refresh_clip(&app, &lock(&shared));
        assert!(app.get_has_clip_preview());
        app.set_clip_material("tools/toolsnodraw".into());
        app.invoke_clip_inputs_changed();
        assert_eq!(lock(&shared).clip_points().unwrap().points, points);
        assert_eq!(
            lock(&shared).clip_preview().unwrap().material.as_deref(),
            Some("tools/toolsnodraw")
        );
        save_ui_snapshot(app.window(), "three-point-clipping");
        app.set_clip_handles_mode(2);
        app.set_clip_angle_step("30".into());
        app.invoke_clip_handles_changed();
        let (token, before_plane) = {
            let mut state = lock(&shared);
            let before = state.clip_preview().unwrap().plane;
            let stamp = state.clip_point_stamp();
            let token = state
                .begin_clip_drag(render::handles::Handle::Z, None, &stamp)
                .unwrap();
            state
                .update_clip_drag(token, None, Some(DVec2::new(31.0, 0.0)))
                .unwrap();
            refresh_clip(&app, &state);
            (token, before)
        };
        assert!(app.get_clip_positioning());
        assert_dialog_button_visible(&app, "Cancel positioning");
        save_ui_snapshot(app.window(), "cut-plane-handles");
        i_slint_backend_testing::ElementHandle::find_by_accessible_label(
            &app,
            "Cancel positioning",
        )
        .next()
        .unwrap()
        .invoke_accessible_default_action();
        assert!(!app.get_clip_positioning());
        assert_eq!(lock(&shared).clip_preview().unwrap().plane, before_plane);
        assert!(lock(&shared).finish_clip_drag(token).is_err());
        assert_eq!(lock(&shared).document().vmf(), &original);
        app.set_clip_angle_step("0".into());
        app.invoke_clip_handles_changed();
        assert!(app.get_message().contains("angle step"));

        for label in [
            "Use plane / line",
            "Remove last point",
            "Start again",
            "Apply cut",
            "Cancel cut",
        ] {
            assert_dialog_button_visible(&app, label);
        }
        app.invoke_clip_points_action(1);
        assert_eq!(app.get_clip_point_count(), 2);
        assert!(!app.get_has_clip_preview());
        app.invoke_cancel_clip();
        assert!(lock(&shared).clip_points().is_none());
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

    #[test]
    fn connection_controls_share_agent_drafts_and_reject_stale_forms() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let mut session = Session::new(Err("no assets needed".into()));
        session.set_definitions(Actor::Agent, Catalog::parse(r#"
@PointClass = logic_relay [ targetname(string) : "Name" input Trigger(void) : "Trigger the relay" output OnTrigger(void) : "Fires when triggered" ]
"#).unwrap());
        let ObjectId::Entity(source) = session
            .create_entity(
                Actor::Agent,
                "logic_relay",
                Some(DVec3::ZERO),
                BTreeMap::from([("targetname".into(), "courtyard_relay".into())]),
            )
            .unwrap()
        else {
            panic!()
        };
        let original = session.document().vmf().clone();
        let shared = Arc::new(Mutex::new(session));
        let window = ConnectionWindow::new().unwrap();
        connection_ui::bind(&window, &shared);
        window.set_source_id(id_string(ObjectId::Entity(source)).into());
        connection_ui::refresh(&window, &lock(&shared));
        window.show().unwrap();
        i_slint_backend_testing::ElementHandle::find_by_accessible_label(&window, "Add connection")
            .next()
            .unwrap()
            .invoke_accessible_default_action();
        window.set_target("!self".into());
        window.set_input("Trigger".into());
        window.set_delay("0.25".into());
        window.set_times("1".into());
        window.invoke_fields_changed();
        assert!(window.get_can_apply());
        assert_eq!(lock(&shared).document().vmf(), &original);
        assert_eq!(
            lock(&shared).connection_draft().unwrap().fields.delay,
            "0.25"
        );
        save_ui_snapshot(window.window(), "entity-connections");
        assert_dialog_button_visible(&window, "Cancel");
        assert_dialog_button_visible(&window, "Apply connection");
        i_slint_backend_testing::ElementHandle::find_by_accessible_label(
            &window,
            "Apply connection",
        )
        .next()
        .unwrap()
        .invoke_accessible_default_action();
        assert_eq!(lock(&shared).connection_records(source).unwrap().len(), 1);
        assert_eq!(lock(&shared).incoming_connections(source).unwrap().len(), 1);
        window.invoke_choose(id_string(ObjectId::Entity(source)).into(), 0);
        save_ui_snapshot(window.window(), "entity-connections-edit");
        let old_token = lock(&shared).connection_draft().unwrap().token;
        lock(&shared).begin_connection_edit(source, None).unwrap();
        window.invoke_apply();
        assert!(window.get_message().contains("draft changed"));
        assert_ne!(lock(&shared).connection_draft().unwrap().token, old_token);
        assert_eq!(lock(&shared).connection_records(source).unwrap().len(), 1);
        window.set_target("!self".into());
        window.set_input("Trigger".into());
        window.set_delay("-1".into());
        window.invoke_fields_changed();
        assert!(!window.get_can_apply());
        window.invoke_apply();
        assert_eq!(lock(&shared).connection_records(source).unwrap().len(), 1);
        i_slint_backend_testing::ElementHandle::find_by_accessible_label(&window, "Cancel")
            .next()
            .unwrap()
            .invoke_accessible_default_action();
        assert!(lock(&shared).connection_draft().is_none());
        window.show().unwrap();
        connection_ui::refresh(&window, &lock(&shared));
        window.invoke_add();
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::KeyPressed {
                text: Key::Escape.into(),
            });
        assert!(lock(&shared).connection_draft().is_none());
        assert!(!window.window().is_visible());
        window.show().unwrap();
        connection_ui::refresh(&window, &lock(&shared));
        window.invoke_delete_connection(0);
        assert!(lock(&shared).connection_records(source).unwrap().is_empty());
        lock(&shared).undo(Actor::Agent).unwrap();
        assert_eq!(lock(&shared).connection_records(source).unwrap().len(), 1);
        lock(&shared).undo(Actor::Agent).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &original);
    }

    #[test]
    fn rename_controls_share_previews_review_cancel_and_stale_guards() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let mut session = Session::new(Err("no assets needed".into()));
        session.set_definitions(Actor::Agent, Catalog::parse(r#"
@PointClass = logic_relay [ targetname(target_source) : "Name" target(target_destination) : "Target" parentname(target_destination) : "Parent" ]
"#).unwrap());
        let ObjectId::Entity(target) = session
            .create_entity(
                Actor::Agent,
                "logic_relay",
                Some(DVec3::ZERO),
                BTreeMap::from([("targetname".into(), "courtyard_relay".into())]),
            )
            .unwrap()
        else {
            panic!()
        };
        let ObjectId::Entity(source) = session
            .create_entity(
                Actor::Agent,
                "logic_relay",
                Some(DVec3::X * 64.0),
                BTreeMap::from([
                    ("targetname".into(), "entrance_trigger".into()),
                    ("target".into(), "courtyard_relay".into()),
                    ("parentname".into(), "courtyard_relay,hinge".into()),
                    ("notes".into(), "courtyard_relay".into()),
                ]),
            )
            .unwrap()
        else {
            panic!()
        };
        session
            .set_entity_connection(
                Actor::Agent,
                source,
                None,
                &map_core::connections::Spec {
                    output: "OnTrigger".into(),
                    target: "courtyard*".into(),
                    input: "Trigger".into(),
                    parameter: "".into(),
                    delay: 0.25,
                    times: -1,
                },
            )
            .unwrap();
        let original = session.document().vmf().clone();
        session.begin_entity_rename(target).unwrap();
        let shared = Arc::new(Mutex::new(session));
        let window = RenameWindow::new().unwrap();
        rename_ui::bind(&window, &shared);
        rename_ui::refresh(&window, &lock(&shared));
        window.show().unwrap();
        window.set_new_name("garden_relay".into());
        window.invoke_fields_changed();
        assert!(window.get_requires_review());
        assert!(!window.get_can_apply());
        assert_eq!(window.get_changes().row_count(), 2);
        assert_eq!(window.get_attention().row_count(), 2);
        assert!(!window.get_show_details());
        for row in window.get_references().iter() {
            assert!(!row.label.contains("entity:"));
            assert!(!row.label.contains("logic_relay"));
        }
        let parent = window
            .get_changes()
            .iter()
            .find(|row| row.label.ends_with(" · Parent"))
            .unwrap();
        assert!(
            parent.label.contains("Entrance trigger · Parent"),
            "{}",
            parent.label
        );
        assert!(parent.reason.contains("Attachment “hinge” stays the same"));
        let wildcard = window
            .get_attention()
            .iter()
            .find(|row| row.change.contains("courtyard*"))
            .unwrap();
        assert!(wildcard.label.contains("When triggered · recipient"));
        assert!(wildcard.reason.contains("will no longer include"));
        window.set_show_details(true);
        window.invoke_fields_changed();
        assert!(lock(&shared).rename_draft().unwrap().show_details);
        assert!(
            window
                .get_changes()
                .row_data(0)
                .unwrap()
                .details
                .contains("logic_relay")
        );
        assert!(!window.get_can_apply());
        window.set_show_details(false);
        window.invoke_fields_changed();
        assert_eq!(lock(&shared).document().vmf(), &original);
        window.invoke_apply();
        assert_eq!(lock(&shared).document().vmf(), &original);
        window.set_accept_review(true);
        window.invoke_fields_changed();
        assert!(window.get_can_apply());
        save_ui_snapshot(window.window(), "entity-rename");
        assert_dialog_button_visible(&window, "New entity name");
        assert_dialog_button_visible(&window, "Current entity name");
        let name_field = i_slint_backend_testing::ElementHandle::find_by_accessible_label(
            &window,
            "New entity name",
        )
        .next()
        .unwrap();
        let summary = i_slint_backend_testing::ElementHandle::find_by_accessible_label(
            &window,
            "Rename summary",
        )
        .next()
        .unwrap();
        assert!(
            name_field.absolute_position().y + name_field.size().height
                <= summary.absolute_position().y,
            "Name field overlaps the rename summary"
        );
        assert_dialog_button_visible(&window, "Cancel");
        assert_dialog_button_visible(&window, "Apply rename");
        i_slint_backend_testing::ElementHandle::find_by_accessible_label(&window, "Apply rename")
            .next()
            .unwrap()
            .invoke_accessible_default_action();
        assert!(lock(&shared).rename_draft().is_none());
        assert_eq!(
            lock(&shared)
                .document()
                .entity(source)
                .unwrap()
                .properties
                .get("parentname"),
            Some("garden_relay,hinge")
        );
        lock(&shared).undo(Actor::Agent).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &original);
        lock(&shared).begin_entity_rename(target).unwrap();
        rename_ui::refresh(&window, &lock(&shared));
        window.show().unwrap();
        window.set_new_name("another_name".into());
        window.invoke_fields_changed();
        i_slint_backend_testing::ElementHandle::find_by_accessible_label(&window, "Cancel")
            .next()
            .unwrap()
            .invoke_accessible_default_action();
        assert!(lock(&shared).rename_draft().is_none());
        assert_eq!(lock(&shared).document().vmf(), &original);
        lock(&shared).begin_entity_rename(target).unwrap();
        rename_ui::refresh(&window, &lock(&shared));
        window.show().unwrap();
        lock(&shared).begin_entity_rename(source).unwrap();
        window.invoke_apply();
        assert!(window.get_message().contains("draft changed"));
        assert_eq!(lock(&shared).rename_draft().unwrap().entity, source);
        rename_ui::refresh(&window, &lock(&shared));
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::KeyPressed {
                text: Key::Escape.into(),
            });
        assert!(lock(&shared).rename_draft().is_none());
        assert!(!window.window().is_visible());
        assert_eq!(lock(&shared).document().vmf(), &original);
    }

    #[test]
    fn management_controls_stage_cancel_apply_and_report_without_desktop_input() {
        use slint::Model;
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let mut session = Session::new(Err("test assets".into()));
        session.set_definitions(
            Actor::Agent,
            Catalog::parse(
                r#"
@PointClass = light [
    _light(color255) : "Light color" : "255 255 255 200"
    style(choices) : "Appearance" : 0 = [ 0 : "Normal" 1 : "Flicker" ]
    spawnflags(flags) = [ 1 : "Initially dark" : 0 ]
]
"#,
            )
            .unwrap(),
        );
        let mut ids = Vec::new();
        for (i, value) in ["255 190 100 300", "100 170 255 600"].iter().enumerate() {
            let ObjectId::Entity(id) = session
                .create_entity(
                    Actor::Agent,
                    "light",
                    Some(DVec3::new(i as f64 * 100.0, 0.0, 64.0)),
                    BTreeMap::from([("_light".into(), value.to_string())]),
                )
                .unwrap()
            else {
                panic!()
            };
            ids.push(id);
        }
        let before = session.document().vmf().clone();
        session
            .begin_property_edit(session::PropertyTarget::Entities(ids.clone()))
            .unwrap();
        let shared = Arc::new(Mutex::new(session));
        let window = PropertiesWindow::new().unwrap();
        management_ui::bind_properties(&window, &shared).unwrap();
        management_ui::refresh_properties(&window, &lock(&shared));
        window.show().unwrap();
        window.invoke_component_edit("_light".into(), 3, "450".into());
        assert!(window.get_can_apply());
        assert_eq!(lock(&shared).document().vmf(), &before);
        assert!(
            window
                .get_fields()
                .iter()
                .any(|row| row.status.contains("Mixed"))
        );
        save_ui_snapshot(window.window(), "batch-properties");
        for label in ["Cancel", "Apply properties"] {
            assert_dialog_button_visible(&window, label);
        }
        window.invoke_cancel();
        assert!(lock(&shared).property_draft().is_none());
        assert_eq!(lock(&shared).document().vmf(), &before);
        lock(&shared)
            .begin_property_edit(session::PropertyTarget::Entities(ids.clone()))
            .unwrap();
        management_ui::refresh_properties(&window, &lock(&shared));
        window.invoke_edit("style".into(), "1".into());
        window.invoke_apply();
        assert_eq!(
            lock(&shared)
                .document()
                .entity(ids[1])
                .unwrap()
                .properties
                .get("style"),
            Some("1")
        );
        lock(&shared).undo(Actor::Agent).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &before);
        lock(&shared)
            .begin_property_edit(session::PropertyTarget::World)
            .unwrap();
        management_ui::refresh_properties(&window, &lock(&shared));
        let old = window.get_draft_token();
        lock(&shared)
            .begin_property_edit(session::PropertyTarget::Entities(ids.clone()))
            .unwrap();
        window.invoke_edit("skyname".into(), "sky_bad".into());
        assert!(window.get_message().contains("replaced"));
        window.set_draft_token(old);
        window.invoke_cancel();
        assert!(lock(&shared).property_draft().is_some());
        let report = ReportWindow::new().unwrap();
        management_ui::bind_report(&report, &shared);
        report.set_classname("light".into());
        report.invoke_search();
        assert_eq!(report.get_rows().row_count(), 2);
        report.set_limit_bounds(true);
        report.set_min_x("80".into());
        report.set_max_x("120".into());
        report.invoke_search();
        assert_eq!(report.get_rows().row_count(), 1);
        report.invoke_select_all(false);
        assert_eq!(lock(&shared).selected_ids(), [ObjectId::Entity(ids[1])]);
        report.set_min_x("oops".into());
        report.invoke_search();
        assert_eq!(report.get_rows().row_count(), 0);
        report.set_limit_bounds(false);
        report.invoke_search();
        report.invoke_select_all(true);
        assert_eq!(lock(&shared).selected_ids().len(), 2);
        report.invoke_visibility_action("hide".into());
        report.set_visibility(2);
        report.invoke_search();
        assert_eq!(report.get_rows().row_count(), 2);
        report.show().unwrap();
        save_ui_snapshot(report.window(), "object-report");
        for label in [
            "Close",
            "Show all",
            "Restore visibility",
            "Select all matches",
        ] {
            assert_dialog_button_visible(&report, label);
        }
        report.invoke_visibility_action("restore".into());
        assert_eq!(report.get_rows().row_count(), 0);
        report.set_visibility(0);
        report.invoke_search();
        let id = report.get_rows().row_data(0).unwrap().id;
        lock(&shared).new_map(Actor::Agent, true).unwrap();
        report.invoke_select_result(id, false);
        assert!(report.get_message().contains("Map changed"));
        assert!(lock(&shared).selected_ids().is_empty());
    }

    #[test]
    fn statistics_controls_refresh_page_and_close_without_editing() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let mut state = Session::new(Err("no assets needed".into()));
        for index in 0..54 {
            let min = DVec3::X * index as f64 * 96.0;
            state
                .create_block(
                    Actor::Agent,
                    min,
                    min + DVec3::splat(64.0),
                    &format!("brick/example_{index:02}"),
                    false,
                )
                .unwrap();
        }
        let before = state.document().vmf().clone();
        let history = state.document().undo_labels().count();
        let shared = Arc::new(Mutex::new(state));
        let window = StatisticsWindow::new().unwrap();
        statistics_ui::bind(&window, &shared);
        window.invoke_refresh();
        assert_eq!(window.get_brushes(), "54");
        assert_eq!(window.get_faces(), "324");
        assert_eq!(window.get_rows().row_count(), 50);
        assert!(window.get_has_next());
        window.set_page(1);
        window.invoke_refresh();
        assert_eq!(window.get_rows().row_count(), 4);
        assert!(!window.get_has_next());
        window.set_page(0);
        window.invoke_refresh();
        window.show().unwrap();
        save_ui_snapshot(window.window(), "map-statistics");
        for label in ["Refresh", "Close", "Next"] {
            assert_dialog_button_visible(&window, label);
        }
        window.invoke_cancel();
        assert!(!window.window().is_visible());
        assert_eq!(lock(&shared).document().vmf(), &before);
        assert_eq!(lock(&shared).document().undo_labels().count(), history);
        window.set_section(1);
        window.invoke_refresh();
        assert_eq!(window.get_rows().row_count(), 0);
        lock(&shared).new_map(Actor::Agent, true).unwrap();
        statistics_ui::refresh_changed(&window, &lock(&shared));
        assert_eq!(window.get_brushes(), "0");
    }

    #[test]
    fn pointfile_controls_load_navigate_unload_and_close_without_map_edits() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("courtyard.lin");
        std::fs::write(&path, "0 0 64\n128 0 64\n128 128 64\n256 128 128\n").unwrap();
        let state = Session::new(Err("no assets needed".into()));
        let before = state.document().vmf().clone();
        let shared = Arc::new(Mutex::new(state));
        let window = PointfileWindow::new().unwrap();
        pointfile_ui::bind(&window, &shared);
        pointfile_ui::refresh(&window, &lock(&shared));
        assert!(!window.get_current());
        window.set_path(path.display().to_string().into());
        window.invoke_load();
        assert_eq!(window.get_total(), 4);
        assert!(window.get_current());
        window.invoke_navigate(1);
        assert_eq!(window.get_point(), 2);
        assert_eq!(lock(&shared).camera().position, Vec3::new(128.0, 0.0, 64.0));
        window.show().unwrap();
        save_ui_snapshot(window.window(), "pointfile");
        for label in ["Load", "Go", "Frame path", "Unload path", "Close"] {
            assert_dialog_button_visible(&window, label);
        }
        window.invoke_frame();
        window.invoke_cancel();
        assert!(!window.window().is_visible());
        assert!(lock(&shared).pointfile_points().is_some());
        window.invoke_clear();
        assert!(!window.get_has_path());
        assert_eq!(lock(&shared).document().vmf(), &before);
        assert_eq!(lock(&shared).document().undo_labels().count(), 0);
    }

    #[test]
    fn component_controls_select_preview_reject_apply_and_cancel() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let mut state = Session::new(Err("no assets needed".into()));
        state
            .create_block(
                Actor::Agent,
                DVec3::ZERO,
                DVec3::splat(64.0),
                "brick/test",
                false,
            )
            .unwrap();
        let id = state.component_solids()[0];
        let shape = state.component_topology(&[id]).unwrap().remove(0);
        let edge = shape
            .edges
            .iter()
            .find(|edge| {
                edge.vertices
                    .iter()
                    .all(|index| shape.vertices[*index].position.z == 64.0)
            })
            .unwrap();
        let key = format!(
            "{}:{}",
            id,
            edge.sides
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(",")
        );
        let original = state.document().vmf().clone();
        let shared = Arc::new(Mutex::new(state));
        let window = ComponentsWindow::new().unwrap();
        components_ui::bind(&window, &shared);
        components_ui::refresh(&window, &lock(&shared));
        window.invoke_toggle_picking();
        assert!(window.get_picking());
        assert_eq!(lock(&shared).component_solids(), vec![id]);
        window.set_kind(1);
        window.invoke_change_kind();
        assert_eq!(
            lock(&shared).component_picking().unwrap().kind,
            session::component_picking::Kind::Vertex
        );
        window.set_kind(0);
        window.invoke_change_kind();
        window.invoke_choose(key.into());
        assert!(lock(&shared).component_picking().is_some());
        assert_eq!(lock(&shared).component_selection().len(), 1);
        window.set_move_z("16".into());
        window.invoke_preview_edit();
        assert!(window.get_ready());
        assert_eq!(lock(&shared).document().vmf(), &original);
        let valid = lock(&shared).render_document().vmf().clone();
        window.show().unwrap();
        save_ui_snapshot(window.window(), "brush-components");
        assert_dialog_button_visible(&window, "Stop viewport picking");
        for label in [
            "Cancel",
            "Apply move",
            "Preview move",
            "Select all shown",
            "Clear selection",
        ] {
            assert_dialog_button_visible(&window, label);
        }
        window.set_move_z("-64".into());
        window.invoke_edited();
        window.invoke_preview_edit();
        assert!(!window.get_ready());
        assert!(!window.get_message().is_empty());
        assert_eq!(lock(&shared).render_document().vmf(), &valid);
        window.invoke_apply();
        assert_eq!(lock(&shared).document().vmf(), &original);
        window.set_move_z("16".into());
        window.invoke_preview_edit();
        window.invoke_apply();
        assert_eq!(lock(&shared).document().vmf(), &valid);
        window.invoke_preview_edit();
        window.invoke_cancel();
        assert!(!window.window().is_visible());
        assert_eq!(lock(&shared).document().vmf(), &valid);
        assert_eq!(lock(&shared).render_document().vmf(), &valid);
        assert!(lock(&shared).component_selection().is_empty());
        window.set_kind(2);
        window.invoke_refresh();
        let bottom = window
            .get_rows()
            .iter()
            .find(|row| row.label.starts_with("Bottom"))
            .unwrap();
        window.invoke_choose(bottom.key);
        window.set_move_z("-16".into());
        window.invoke_preview_edit();
        assert!(window.get_ready());
        assert_eq!(
            lock(&shared)
                .component_selection()
                .iter()
                .next()
                .unwrap()
                .sides
                .len(),
            1
        );
        window.show().unwrap();
        save_ui_snapshot(window.window(), "brush-faces");
        window.invoke_cancel();
        assert_eq!(lock(&shared).document().vmf(), &valid);
        lock(&shared).undo(Actor::Agent).unwrap();
        window.invoke_refresh();
        let top = window
            .get_rows()
            .iter()
            .find(|row| row.label.starts_with("Top"))
            .unwrap();
        window.invoke_choose(top.key);
        window.set_move_z("16".into());
        window.set_texture_lock(true);
        window.invoke_edited();
        window.invoke_preview_edit();
        assert!(window.get_ready());
        assert_eq!(
            lock(&shared).component_report(&[]).unwrap()["texture_mode"],
            "attached"
        );
        window.show().unwrap();
        window.window().take_snapshot().unwrap();
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(250));
        save_ui_snapshot(window.window(), "brush-texture-attachment");
        window.invoke_apply();
        lock(&shared).undo(Actor::Agent).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &original);
        window.invoke_cancel();
    }

    #[test]
    fn partition_texture_controls_share_preview_inheritance_keep_cancel_and_undo() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let mut state = Session::new(Err("no assets needed".into()));
        let ObjectId::Solid(id) = state
            .create_block(
                Actor::Agent,
                DVec3::ZERO,
                DVec3::splat(64.0),
                "brick/test",
                false,
            )
            .unwrap()
        else {
            unreachable!()
        };
        let side = state
            .document()
            .solid(id)
            .unwrap()
            .sides
            .iter()
            .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
            .unwrap()
            .id;
        state
            .select_components(
                &[map_core::subobject::Component {
                    solid: id,
                    sides: vec![side],
                }],
                SelectMode::Replace,
                &state.component_stamp(),
            )
            .unwrap();
        let original = state.document().vmf().clone();
        let shared = Arc::new(Mutex::new(state));
        let window = ComponentsWindow::new().unwrap();
        components_ui::bind(&window, &shared);
        window.set_kind(2);
        window.set_operation(6);
        components_ui::refresh(&window, &lock(&shared));
        window.invoke_preview_edit();
        assert!(window.get_ready());
        window.set_show_top_texture(true);
        window.set_texture_shift_u("13.125".into());
        window.set_texture_shift_v("-27.5".into());
        window.set_texture_scale_u("-0.375".into());
        window.set_texture_scale_v("0.625".into());
        window.set_texture_rotation("37".into());
        window.invoke_edited();
        assert!(!window.get_ready());
        window.invoke_preview_edit();
        assert!(window.get_message().contains("texture placement"));
        window.invoke_preview_partition_texture();
        assert!(window.get_ready());
        assert_eq!(window.get_texture_shift_u(), "13.125");
        window.set_texture_scale_u("0".into());
        window.invoke_preview_partition_texture();
        assert!(window.get_message().contains("nonzero scales"));
        window.set_texture_scale_u("-0.375".into());
        window.invoke_preview_partition_texture();
        assert!(window.get_ready());
        window.show().unwrap();
        window
            .window()
            .set_size(slint::PhysicalSize::new(1100, 900));
        save_ui_snapshot(window.window(), "partition-texture-placement");
        assert_dialog_button_visible(&window, "Cancel");
        window.invoke_inherit_partition_texture();
        assert!(window.get_top_texture_summary().contains("inherited"));
        window.invoke_choose_texture_rotation(90.0);
        assert!(!window.get_ready());
        window.invoke_preview_partition_texture();
        window.invoke_apply();
        let kept = lock(&shared).document().vmf().clone();
        assert_ne!(kept, original);
        window.invoke_resume_saved_partition();
        assert_eq!(window.get_texture_rotation(), "90");
        window.set_texture_rotation("-45".into());
        window.invoke_preview_partition_texture();
        window.invoke_cancel();
        assert_eq!(lock(&shared).document().vmf(), &kept);
        lock(&shared).undo(Actor::Agent).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &original);
    }

    #[test]
    fn flat_partition_controls_stage_highlight_frame_raise_flatten_apply_and_cancel() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let mut state = Session::new(if std::env::var_os("HAMMERR_UI_SNAPSHOTS").is_some() {
            mount_game(None)
        } else {
            Err("no assets needed".into())
        });
        let ObjectId::Solid(id) = state
            .create_block(
                Actor::Agent,
                DVec3::ZERO,
                DVec3::splat(64.0),
                "brick/test",
                false,
            )
            .unwrap()
        else {
            unreachable!()
        };
        let side = state
            .document()
            .solid(id)
            .unwrap()
            .sides
            .iter()
            .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
            .unwrap()
            .id;
        state
            .set_component_picking(
                Some(session::component_picking::Kind::Face),
                &[id],
                &state.component_stamp(),
            )
            .unwrap();
        state
            .select_components(
                &[map_core::subobject::Component {
                    solid: id,
                    sides: vec![side],
                }],
                SelectMode::Replace,
                &state.component_stamp(),
            )
            .unwrap();
        let source = state.document().vmf().clone();
        let history = state.document().undo_labels().count();
        let shared = Arc::new(Mutex::new(state));
        let window = ComponentsWindow::new().unwrap();
        components_ui::bind(&window, &shared);
        components_ui::refresh(&window, &lock(&shared));
        window.set_operation(6);
        window.set_kind(2);
        window.show().unwrap();
        window
            .window()
            .set_size(slint::PhysicalSize::new(1100, 900));
        assert_dialog_button_visible(&window, "Stage flat partition");
        window.invoke_preview_edit();
        save_ui_snapshot(window.window(), "flat-face-partition");
        assert_eq!(window.get_operation(), 6);
        assert!(window.get_has_partition_preview() && window.get_partition_flat());
        assert!(window.get_ready());
        assert_eq!(window.get_partition_rows().row_count(), 2);
        window.set_show_top_material(true);
        window.set_top_material("brick/brickwall003a".into());
        window.invoke_edited();
        assert!(!window.get_ready());
        components_ui::refresh(&window, &lock(&shared));
        assert!(!window.get_ready());
        window.invoke_preview_edit();
        assert!(window.get_message().contains("planned top material"));
        assert!(!window.get_ready());
        window.invoke_preview_partition_material();
        assert!(window.get_ready());
        assert_eq!(window.get_top_material(), "brick/brickwall003a");
        assert!(
            window
                .get_partition_rows()
                .iter()
                .all(|row| row.detail.contains("planned"))
        );
        let picker_target = {
            let state = lock(&shared);
            components_ui::partition_material_target(&state)
        };
        let row = window.get_partition_rows().row_data(0).unwrap();
        window.invoke_choose_partition(row.key.clone());
        assert!(
            components_ui::choose_partition_material(
                &window,
                &mut lock(&shared),
                &picker_target,
                Some("wood/stale")
            )
            .is_err()
        );
        window.invoke_choose_partition(row.key);
        for label in [
            "Preview material",
            "Restore inheritance",
            "Keep flat pieces",
            "Cancel",
        ] {
            assert_dialog_button_visible(&window, label);
        }
        window.window().set_size(slint::PhysicalSize::new(880, 540));
        assert_dialog_button_visible(&window, "Keep flat pieces");
        assert_dialog_button_visible(&window, "Cancel");
        window
            .window()
            .set_size(slint::PhysicalSize::new(1100, 900));
        save_ui_snapshot(window.window(), "partition-top-material");
        window.invoke_inherit_partition_material();
        assert!(window.get_ready());
        assert_eq!(window.get_top_material(), "brick/test");
        window.set_show_top_material(false);

        assert!(
            window
                .get_preview_summary()
                .contains("2 flat pieces staged")
        );
        for label in [
            "Frame highlighted pieces",
            "Partition ridge height",
            "Keep roof texture at corners",
            "Preview ridge",
            "Return to flat",
            "Keep flat pieces",
            "Discard preview",
            "Cancel",
        ] {
            assert_dialog_button_visible(&window, label);
        }
        assert_eq!(lock(&shared).document().vmf(), &source);
        let first = window.get_partition_rows().row_data(0).unwrap();
        window.invoke_choose_partition(first.key);
        assert!(!window.get_partition_rows().row_data(0).unwrap().selected);
        window.invoke_frame_partitions();
        assert!(window.get_message().is_empty());
        assert_eq!(lock(&shared).document().undo_labels().count(), history);
        window.set_show_cut_settings(true);
        window.invoke_choose_cut_position(0, 20.0);
        window.invoke_choose_cut_position(1, 80.0);
        assert!(!window.get_ready());
        window.invoke_preview_edit();
        assert!(window.get_message().contains("Preview the cut positions"));
        window.invoke_preview_cut();
        assert!(window.get_ready());
        assert_eq!(
            lock(&shared).component_report(&[]).unwrap()["preview"]["partition"]["edge_parameters"],
            serde_json::json!([0.2, 0.8])
        );
        save_ui_snapshot(window.window(), "adjusted-flat-partition");
        for label in [
            "Partition edge A position",
            "Partition edge B position",
            "Preview cut",
            "Keep flat pieces",
            "Cancel",
        ] {
            assert_dialog_button_visible(&window, label);
        }
        let cut_token = lock(&shared).partition_preview_token();
        window.invoke_choose_cut_position(0, 0.0);
        window.invoke_preview_cut();
        assert!(!window.get_ready());
        assert_eq!(lock(&shared).partition_preview_token(), cut_token);
        window.invoke_choose_cut_position(0, 13.7);
        window.invoke_choose_cut_position(1, 68.3);
        window.invoke_preview_cut();
        assert!(window.get_ready());
        assert!((window.get_cut_first_value() - 13.7).abs() < 0.001);
        assert!((window.get_cut_second_value() - 68.3).abs() < 0.001);
        assert_dialog_button_visible(&window, "Reset cut to midpoint");
        window.invoke_choose_rise(16.0);
        window.invoke_preview_edit();
        assert!(window.get_message().contains("nonplanar"));
        window.invoke_choose_cut_position(1, 13.7);
        window.invoke_preview_cut();
        window.set_show_cut_settings(false);
        window.set_roof_texture_lock(true);
        window.invoke_choose_rise(16.125);
        window.invoke_preview_edit();
        assert!(window.get_ready() && !window.get_partition_flat());
        let report = lock(&shared).component_report(&[id]).unwrap();
        assert!(
            (report["preview"]["partition"]["edge_parameters"][0]
                .as_f64()
                .unwrap()
                - 0.137)
                .abs()
                < 1e-10
        );
        assert!(!window.get_cut_is_midpoint());
        assert_eq!(window.get_cut_first(), "13.7");
        assert_eq!(window.get_cut_second(), "13.7");
        window.set_show_cut_settings(false);
        save_ui_snapshot(window.window(), "adjusted-partition-ridge");
        window.invoke_flatten_partitions();
        assert!(window.get_ready() && window.get_partition_flat());

        window.invoke_choose_cut_position(0, 50.0);
        window.invoke_choose_cut_position(1, 50.0);
        window.invoke_preview_cut();
        assert!(window.get_ready());
        window.set_show_cut_settings(false);
        window.set_roof_texture_lock(true);
        window.invoke_choose_rise(16.125);
        window.invoke_preview_edit();
        save_ui_snapshot(window.window(), "raised-face-partition");
        assert!(window.get_ready() && !window.get_partition_flat());
        assert_eq!(
            lock(&shared).component_report(&[id]).unwrap()["texture_mode"],
            "roof_attached"
        );
        let raised = lock(&shared).render_document().vmf().clone();
        window.invoke_flatten_partitions();
        assert!(window.get_partition_flat() && window.get_ready());
        assert_eq!(lock(&shared).render_document().vmf(), &source);
        assert!(!window.get_partition_rows().row_data(0).unwrap().selected);
        window.set_split_rise("0.01".into());
        window.invoke_edited();
        window.invoke_preview_edit();
        assert!(!window.get_ready());
        assert!(window.get_message().contains("0.125"));
        assert!(window.get_has_partition_preview());
        window.invoke_choose_rise(16.125);
        window.invoke_preview_edit();
        assert_eq!(lock(&shared).render_document().vmf(), &raised);
        window.invoke_discard_preview();
        assert!(!window.get_has_partition_preview());
        assert_eq!(lock(&shared).document().vmf(), &source);
        window.invoke_preview_edit();
        window.set_roof_texture_lock(true);
        window.invoke_choose_rise(16.125);
        window.invoke_preview_edit();
        window.invoke_apply();
        assert_eq!(lock(&shared).document().vmf(), &raised);
        assert_eq!(lock(&shared).document().undo_labels().count(), history + 1);
        lock(&shared).undo(Actor::Agent).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &source);
        components_ui::refresh(&window, &lock(&shared));
        window.set_kind(2);
        window.invoke_change_kind();
        window.set_operation(6);
        let top = window
            .get_rows()
            .iter()
            .find(|row| row.key == format!("{id}:{side}"))
            .unwrap();
        window.invoke_choose(top.key);
        window.invoke_preview_edit();
        assert!(window.get_has_partition_preview());
        window.invoke_apply();
        assert!(window.get_message().contains("Kept 1 flat partitions"));
        assert!(!window.get_has_partition_preview());
        assert_eq!(window.get_saved_partitions().row_count(), 1);
        let stored = lock(&shared).document().vmf().clone();
        assert_eq!(stored.world, source.world);
        save_ui_snapshot(window.window(), "saved-face-partitions");
        assert_dialog_button_visible(&window, "Resume pieces");
        assert_dialog_button_visible(&window, "Remove saved");
        window.invoke_resume_saved_partition();
        assert!(window.get_has_partition_preview() && window.get_partition_flat());
        assert_eq!(lock(&shared).document().vmf(), &stored);
        window.set_partition_mode(1);
        window.invoke_edited();
        window.set_extrusion_texture_lock(true);
        window.invoke_choose_extrusion_height(32.125);
        let target = {
            let session = lock(&shared);
            (
                session.partition_preview_token(),
                session.document_generation(),
            )
        };
        components_ui::choose_extrusion_material(
            &window,
            &mut lock(&shared),
            target,
            "concrete/test",
        )
        .unwrap();
        assert!(!window.get_ready());
        window.invoke_preview_edit();
        assert!(window.get_ready() && window.get_has_extrusion_preview());
        save_ui_snapshot(window.window(), "partition-extrusion-controls");
        for label in [
            "Extrusion height",
            "Keep top texture at corners",
            "Choose wall material",
            "Browse…",
            "Preview extrusion",
            "Apply extrusion",
            "Cancel",
        ] {
            assert_dialog_button_visible(&window, label);
        }
        window.window().set_size(slint::PhysicalSize::new(880, 720));
        save_ui_snapshot(window.window(), "partition-extrusion-compact");
        assert_dialog_button_visible(&window, "Apply extrusion");
        assert_dialog_button_visible(&window, "Cancel");
        window
            .window()
            .set_size(slint::PhysicalSize::new(1100, 900));
        assert_eq!(lock(&shared).document().vmf(), &stored);
        assert!(
            components_ui::choose_extrusion_material(
                &window,
                &mut lock(&shared),
                target,
                "brick/test"
            )
            .is_err()
        );
        let token = lock(&shared).partition_preview_token().unwrap();
        window.invoke_choose_partition(window.get_partition_rows().row_data(0).unwrap().key);
        assert!(!window.get_ready());
        window.invoke_preview_edit();
        assert!(window.get_ready());
        assert_ne!(lock(&shared).partition_preview_token(), Some(token));
        window.invoke_apply();
        assert!(window.get_message().contains("Added 1 volumes"));
        assert_eq!(lock(&shared).selected_faces().len(), 6);
        lock(&shared).undo(Actor::Agent).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &stored);
        components_ui::refresh(&window, &lock(&shared));
        window.invoke_resume_saved_partition();
        window.set_partition_mode(1);
        window.invoke_preview_edit();
        assert_eq!(lock(&shared).render_document().vmf().world.solids.len(), 3);
        {
            let session = lock(&shared);
            assert_eq!(
                session.render_document().solid(id),
                session.document().solid(id)
            );
        }
        window.invoke_flatten_partitions();
        assert!(!window.get_has_extrusion_preview());
        assert_eq!(lock(&shared).render_document().vmf(), &stored);
        window.invoke_discard_preview();
        window.invoke_remove_saved_partition();
        assert_eq!(lock(&shared).document().vmf(), &source);
        lock(&shared).undo(Actor::Agent).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &stored);
        lock(&shared).undo(Actor::Agent).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &source);
        components_ui::refresh(&window, &lock(&shared));
        window.set_kind(2);
        window.invoke_change_kind();
        window.set_operation(6);
        let top = window
            .get_rows()
            .iter()
            .find(|row| row.key == format!("{id}:{side}"))
            .unwrap();
        window.invoke_choose(top.key);
        window.set_operation(5);
        window.invoke_refresh();
        assert_eq!(
            window
                .get_crease_rows()
                .row_data(0)
                .unwrap()
                .choices
                .row_count(),
            2
        );
        let agent_token = {
            let mut session = lock(&shared);
            let choices = session.component_partition_choices().unwrap();
            let stamp = session.component_stamp();
            session
                .preview_component_partition(
                    &[crate::session::components::CreaseEdges {
                        solid: id,
                        edges: choices[0].1[3].edges,
                    }],
                    &stamp,
                )
                .unwrap()
        };
        window.invoke_refresh();
        assert_eq!(window.get_operation(), 6);
        assert_eq!(
            window
                .get_crease_rows()
                .row_data(0)
                .unwrap()
                .choices
                .row_count(),
            6
        );
        assert_eq!(window.get_crease_rows().row_data(0).unwrap().choice, 3);
        lock(&shared).cancel_component_move(agent_token).unwrap();
        window.invoke_refresh();
        let row = window.get_crease_rows().row_data(0).unwrap();
        assert_eq!(row.choices.row_count(), 6);
        assert!(row.choices.row_data(2).unwrap().starts_with("Trim corner"));
        window.invoke_choose_crease_pair(row.key, 2);
        window.invoke_preview_edit();
        assert!(window.get_has_partition_preview() && window.get_ready());
        assert!(!window.get_can_raise_partition());
        assert!(
            window
                .get_partition_rows()
                .row_data(0)
                .unwrap()
                .label
                .contains("triangle")
        );
        assert!(
            window
                .get_partition_rows()
                .row_data(1)
                .unwrap()
                .label
                .contains("pentagon")
        );
        window.set_show_cut_settings(true);
        window.invoke_choose_cut_position(0, 20.0);
        window.invoke_choose_cut_position(1, 80.0);
        window.invoke_preview_cut();
        assert!(window.get_ready());
        save_ui_snapshot(window.window(), "boundary-partition-controls");
        window.invoke_apply();
        assert!(!window.get_has_partition_preview());
        let boundary_stored = lock(&shared).document().vmf().clone();
        window.invoke_resume_saved_partition();
        assert!(window.get_ready() && window.get_partition_flat());
        assert!(!window.get_can_raise_partition());
        window.invoke_preview_edit();
        assert!(window.get_ready() && window.get_partition_flat());
        window.set_partition_mode(1);
        window.invoke_choose_extrusion_height(16.125);
        window.invoke_preview_edit();
        assert!(window.get_ready() && window.get_has_extrusion_preview());
        window.set_show_cut_settings(false);
        save_ui_snapshot(window.window(), "boundary-partition-extrusion");
        window.invoke_cancel();
        assert_eq!(lock(&shared).partition_preview_token(), None);
        assert_eq!(lock(&shared).document().vmf(), &boundary_stored);
        lock(&shared).undo(Actor::Agent).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &source);
    }

    #[test]
    fn polygon_partition_controls_offer_complete_choices_keep_resume_and_cancel_extrusion() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        for corners in [3, 5, 6, 8] {
            let mut state = Session::new(Err("no assets needed".into()));
            let ids = state
                .create_primitive(
                    Actor::Agent,
                    map_core::primitive::Spec {
                        shape: map_core::primitive::Shape::Cylinder,
                        segments: corners,
                        mins: DVec3::ZERO,
                        maxs: DVec3::splat(64.0),
                        material: "brick/test".into(),
                        ..Default::default()
                    },
                    false,
                )
                .unwrap();
            let ObjectId::Solid(id) = ids[0] else {
                unreachable!()
            };
            let side = state
                .document()
                .solid(id)
                .unwrap()
                .sides
                .iter()
                .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
                .unwrap()
                .id;
            state
                .set_component_picking(
                    Some(session::component_picking::Kind::Face),
                    &[id],
                    &state.component_stamp(),
                )
                .unwrap();
            state
                .select_components(
                    &[map_core::subobject::Component {
                        solid: id,
                        sides: vec![side],
                    }],
                    SelectMode::Replace,
                    &state.component_stamp(),
                )
                .unwrap();
            let source = state.document().vmf().clone();
            let shared = Arc::new(Mutex::new(state));
            let window = ComponentsWindow::new().unwrap();
            components_ui::bind(&window, &shared);
            window.set_kind(2);
            window.set_operation(6);
            components_ui::refresh(&window, &lock(&shared));
            window.show().unwrap();
            window
                .window()
                .set_size(slint::PhysicalSize::new(1100, 900));
            let row = window.get_crease_rows().row_data(0).unwrap();
            assert_eq!(
                row.choices.row_count(),
                (corners * (corners - 1) / 2) as usize
            );
            assert!(
                row.choices
                    .iter()
                    .any(|label| label.starts_with("Trim corner"))
            );
            let choice = if corners == 3 {
                0
            } else {
                row.choices
                    .iter()
                    .position(|label| label.starts_with("Cut from"))
                    .unwrap() as i32
            };
            window.set_operation(5);
            window.invoke_refresh();
            assert_eq!(window.get_crease_rows().row_count(), 0);
            let agent_token = {
                let mut state = lock(&shared);
                let pair = state.component_partition_choices().unwrap()[0].1[choice as usize].edges;
                let stamp = state.component_stamp();
                state
                    .preview_component_partition(
                        &[session::components::CreaseEdges {
                            solid: id,
                            edges: pair,
                        }],
                        &stamp,
                    )
                    .unwrap()
            };
            window.invoke_refresh();
            assert_eq!(window.get_operation(), 6);
            assert_eq!(
                window
                    .get_crease_rows()
                    .row_data(0)
                    .unwrap()
                    .choices
                    .row_count(),
                (corners * (corners - 1) / 2) as usize
            );
            assert_eq!(window.get_crease_rows().row_data(0).unwrap().choice, choice);
            lock(&shared).cancel_component_move(agent_token).unwrap();
            window.invoke_refresh();
            window.invoke_choose_crease_pair(row.key, choice);
            window.invoke_preview_edit();
            assert!(window.get_ready() && window.get_partition_flat());
            assert!(!window.get_can_raise_partition());
            assert_eq!(window.get_partition_rows().row_count(), 2);
            assert!(
                window
                    .get_preview_summary()
                    .contains("extrude highlighted pieces")
            );
            assert!(!window.get_preview_summary().contains("shared ridge"));
            let edge_labels: Vec<_> = window
                .get_rows()
                .iter()
                .filter(|row| row.label.contains("Edge midpoint"))
                .map(|row| row.label.to_string())
                .collect();
            assert_eq!(
                edge_labels,
                [
                    format!("Brush {id} · Edge midpoint 1"),
                    format!("Brush {id} · Edge midpoint 2"),
                ]
            );
            window.set_show_cut_settings(true);
            window.invoke_choose_cut_position(0, 20.0);
            window.invoke_choose_cut_position(1, 80.0);
            window.invoke_preview_cut();
            assert!(window.get_ready());
            window.set_show_top_material(true);
            window.set_top_material("brick/polygon".into());
            window.invoke_edited();
            window.invoke_preview_partition_material();
            assert!(window.get_ready());
            window.invoke_frame_partitions();
            assert!(window.get_message().is_empty());
            assert_dialog_button_visible(&window, "Keep flat pieces");
            assert_dialog_button_visible(&window, "Cancel");
            save_ui_snapshot(
                window.window(),
                &format!("polygon-{corners}-partition-controls"),
            );
            window.invoke_apply();
            assert!(window.get_message().contains("Kept 1 flat partitions"));
            let stored = lock(&shared).document().vmf().clone();
            assert_eq!(stored.world, source.world);
            window.invoke_resume_saved_partition();
            assert!(window.get_ready() && window.get_partition_flat());
            assert!(!window.get_can_raise_partition());
            assert!(
                window
                    .get_partition_rows()
                    .iter()
                    .all(|row| row.detail.contains("brick/polygon"))
            );
            window.set_partition_mode(1);
            window.invoke_choose_extrusion_height(16.125);
            window.invoke_preview_edit();
            assert!(window.get_ready() && window.get_has_extrusion_preview());
            assert_dialog_button_visible(&window, "Apply extrusion");
            assert_dialog_button_visible(&window, "Cancel");
            let draft = lock(&shared).component_report(&[]).unwrap();
            assert_eq!(
                draft["preview"]["extrusion"]["pieces"]
                    .as_array()
                    .unwrap()
                    .len(),
                2
            );
            window.invoke_cancel();
            assert_eq!(lock(&shared).partition_preview_token(), None);
            assert_eq!(lock(&shared).document().vmf(), &stored);
            lock(&shared).undo(Actor::Agent).unwrap();
            assert_eq!(lock(&shared).document().vmf(), &source);
        }
    }

    #[test]
    fn component_source_controls_page_large_owners_and_edit_one_child() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let mut state = Session::new(Err("no assets needed".into()));
        let mut ids = Vec::new();
        for index in 0..105 {
            let ObjectId::Solid(id) = state
                .create_block(
                    Actor::Agent,
                    DVec3::new(index as f64 * 96.0, 0.0, 0.0),
                    DVec3::new(index as f64 * 96.0 + 64.0, 64.0, 64.0),
                    "brick/test",
                    false,
                )
                .unwrap()
            else {
                unreachable!()
            };
            ids.push(id);
        }
        let owner = state
            .edit(Actor::Agent, "owner", |tx| {
                map_core::edit::tie_to_entity(tx, &ids, "func_detail", &[])
            })
            .unwrap();
        state.select(&[owner], SelectMode::Replace).unwrap();
        let original = state.document().vmf().clone();
        let shared = Arc::new(Mutex::new(state));
        let window = ComponentsWindow::new().unwrap();
        components_ui::bind(&window, &shared);
        window.set_kind(2);
        components_ui::refresh(&window, &lock(&shared));
        assert_eq!(window.get_source_total(), 105);
        assert!(window.get_rows().row_count() == 0);
        assert!(!window.get_source_all());
        assert!(window.get_summary().contains("Choose a brush"));
        window.invoke_page_sources(1);
        assert_eq!(window.get_source_ids().row_count(), 5);
        assert_eq!(window.get_source_summary(), "101–105 of 105");
        window.set_source_index(1);
        window.invoke_choose_source();
        assert!(window.get_picking());
        assert_eq!(window.get_rows().row_count(), 6);
        assert_eq!(lock(&shared).component_solids(), vec![ids[100]]);
        assert_eq!(lock(&shared).selected_ids(), vec![owner]);
        window.show().unwrap();
        save_ui_snapshot(window.window(), "component-brush-scope");
        for label in [
            "Previous brushes",
            "Next brushes",
            "All selected",
            "Cancel",
            "Preview move",
        ] {
            assert_dialog_button_visible(&window, label);
        }
        window.invoke_choose(window.get_rows().row_data(0).unwrap().key);
        window.set_move_z("8".into());
        window.set_snap(false);
        window.invoke_preview_edit();
        assert!(window.get_ready(), "{}", window.get_message());
        window.invoke_apply();
        let changed = lock(&shared).document().vmf().clone();
        assert_eq!(
            &changed.entities[0].solids[..100],
            &original.entities[0].solids[..100]
        );
        assert_eq!(
            &changed.entities[0].solids[101..],
            &original.entities[0].solids[101..]
        );
        assert_ne!(
            changed.entities[0].solids[100],
            original.entities[0].solids[100]
        );
        lock(&shared).undo(Actor::User).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &original);
        components_ui::refresh(&window, &lock(&shared));
        window.set_source_index(1);
        window.invoke_choose_source();
        window.set_source_mode(1);
        window.invoke_page_sources(-1);
        window.set_source_index(1);
        window.invoke_choose_source();
        assert_eq!(lock(&shared).component_solids(), vec![ids[0], ids[100]]);
        assert_eq!(window.get_rows().row_count(), 12);
        assert!(
            window
                .get_rows()
                .row_data(0)
                .unwrap()
                .label
                .starts_with(&format!("Brush {} · ", ids[0]))
        );
        assert!(
            window
                .get_rows()
                .row_data(6)
                .unwrap()
                .label
                .starts_with(&format!("Brush {} · ", ids[100]))
        );
        save_ui_snapshot(window.window(), "component-brush-subset");
        window.set_source_index(1);
        let stamp = lock(&shared).component_stamp();
        window.invoke_choose_source();
        assert_eq!(lock(&shared).component_stamp(), stamp);
        window.set_source_mode(2);
        window.invoke_page_sources(1);
        window.set_source_index(1);
        window.invoke_choose_source();
        assert_eq!(lock(&shared).component_solids(), vec![ids[0]]);
        window.invoke_page_sources(-1);
        window.set_source_index(1);
        window.invoke_choose_source();
        assert!(!window.get_picking());
        assert_eq!(window.get_source_mode(), 0);
        assert_eq!(lock(&shared).selected_ids(), vec![owner]);
        assert_eq!(lock(&shared).document().vmf(), &original);
        window.invoke_cancel();
    }

    #[test]
    fn component_scale_rotate_controls_share_pivots_and_preserve_valid_drafts() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let mut state = Session::new(Err("no assets needed".into()));
        let ObjectId::Solid(id) = state
            .create_block(
                Actor::Agent,
                DVec3::ZERO,
                DVec3::splat(64.0),
                "brick/test",
                false,
            )
            .unwrap()
        else {
            unreachable!()
        };
        let shape = state.component_topology(&[id]).unwrap().remove(0);
        let side = shape
            .faces
            .iter()
            .find(|face| {
                face.vertices
                    .iter()
                    .all(|index| shape.vertices[*index].position.z == 64.0)
            })
            .unwrap()
            .side;
        state
            .select_components(
                &[map_core::subobject::Component {
                    solid: id,
                    sides: vec![side],
                }],
                SelectMode::Replace,
                &state.component_stamp(),
            )
            .unwrap();
        let original = state.document().vmf().clone();
        let shared = Arc::new(Mutex::new(state));
        let window = ComponentsWindow::new().unwrap();
        components_ui::bind(&window, &shared);
        window.set_kind(2);
        window.set_operation(3);
        components_ui::refresh(&window, &lock(&shared));
        assert!(window.get_transform_center().contains("(32, 32, 64)"));
        window.invoke_preview_edit();
        assert!(window.get_ready());
        let draft = lock(&shared).render_document().vmf().clone();
        window.show().unwrap();
        save_ui_snapshot(window.window(), "component-scale");
        for label in [
            "Cancel",
            "Apply scale",
            "Discard preview",
            "Preview scale",
            "Component transform X",
            "Component transform Y",
            "Component transform Z",
        ] {
            assert_dialog_button_visible(&window, label);
        }
        window.set_scale_x("0".into());
        window.invoke_edited();
        window.invoke_preview_edit();
        assert!(!window.get_ready());
        window.invoke_apply();
        assert_eq!(lock(&shared).document().vmf(), &original);
        assert_eq!(lock(&shared).render_document().vmf(), &draft);
        window.invoke_discard_preview();
        window.set_operation(4);
        window.set_pivot_mode(1);
        window.invoke_choose_pivot();
        assert_eq!(window.get_pivot_x(), "32");
        assert_eq!(window.get_pivot_y(), "32");
        assert_eq!(window.get_pivot_z(), "64");
        window.invoke_preview_edit();
        assert!(window.get_ready());
        save_ui_snapshot(window.window(), "component-rotate");
        for label in [
            "Apply rotation",
            "Preview rotation",
            "Component pivot X",
            "Component pivot Y",
            "Component pivot Z",
        ] {
            assert_dialog_button_visible(&window, label);
        }
        let rotated = lock(&shared).render_document().vmf().clone();
        window.invoke_apply();
        assert_eq!(lock(&shared).document().vmf(), &rotated);
        lock(&shared).undo(Actor::User).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &original);
        let mut session = lock(&shared);
        let stamp = session.component_stamp();
        session
            .select_components(
                &[map_core::subobject::Component {
                    solid: id,
                    sides: vec![side],
                }],
                SelectMode::Replace,
                &stamp,
            )
            .unwrap();
        let stamp = session.component_stamp();
        session
            .preview_component_transform(
                map_core::subobject::ComponentTransform::Scale(DVec3::new(0.75, 0.5, 1.0)),
                Some(DVec3::new(16.125, 24.25, 64.0)),
                false,
                &stamp,
            )
            .unwrap();
        components_ui::refresh(&window, &session);
        assert_eq!(window.get_operation(), 3);
        assert_eq!(window.get_scale_x(), "0.75");
        assert_eq!(window.get_pivot_x(), "16.125");
        assert!(window.get_ready());
        drop(session);
        window.invoke_cancel();
    }

    #[test]
    fn component_weld_target_controls_preview_existing_corners_and_world_positions() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let mut state = Session::new(Err("no assets needed".into()));
        let ObjectId::Solid(id) = state
            .create_block(
                Actor::Agent,
                DVec3::ZERO,
                DVec3::splat(64.0),
                "brick/test",
                false,
            )
            .unwrap()
        else {
            unreachable!()
        };
        let selected = state.component_topology(&[id]).unwrap()[0]
            .vertices
            .iter()
            .filter(|vertex| vertex.position.z == 64.0)
            .map(|vertex| map_core::subobject::Component {
                solid: id,
                sides: vertex.sides.clone(),
            })
            .collect::<Vec<_>>();
        state
            .select_components(&selected, SelectMode::Replace, &state.component_stamp())
            .unwrap();
        let original = state.document().vmf().clone();
        let corner_index = state
            .component_weld_targets()
            .unwrap()
            .iter()
            .position(|(component, _)| component == &selected[0])
            .unwrap();
        let shared = Arc::new(Mutex::new(state));
        let window = ComponentsWindow::new().unwrap();
        components_ui::bind(&window, &shared);
        window.set_kind(1);
        window.set_operation(1);
        components_ui::refresh(&window, &lock(&shared));
        assert_eq!(window.get_weld_corner_labels().row_count(), 8);
        window.set_weld_target_mode(2);
        window.invoke_choose_weld_mode();
        assert_eq!(window.get_weld_x(), "32");
        assert_eq!(window.get_weld_y(), "32");
        assert_eq!(window.get_weld_z(), "64");
        window.set_weld_x("16.125".into());
        window.set_weld_y("24.25".into());
        window.set_weld_z("96.5".into());
        window.set_texture_lock(true);
        window.invoke_preview_edit();
        assert!(window.get_ready());
        assert!(
            window
                .get_preview_summary()
                .contains("Target: (16.125, 24.25, 96.5)")
        );
        let draft = lock(&shared).render_document().vmf().clone();
        window.show().unwrap();
        save_ui_snapshot(window.window(), "brush-weld-position");
        for label in [
            "Cancel",
            "Apply weld",
            "Discard preview",
            "Preview weld",
            "Weld target X",
            "Weld target Y",
            "Weld target Z",
            "Keep textures at surviving corners",
        ] {
            assert_dialog_button_visible(&window, label);
        }
        window.set_weld_z("65537".into());
        window.invoke_edited();
        window.invoke_preview_edit();
        assert!(!window.get_ready());
        assert_eq!(lock(&shared).render_document().vmf(), &draft);
        window.invoke_apply();
        assert_eq!(lock(&shared).document().vmf(), &original);
        window.set_weld_z("96.5".into());
        window.invoke_preview_edit();
        window.invoke_discard_preview();
        assert_eq!(lock(&shared).render_document().vmf(), &original);
        window.set_weld_target_mode(1);
        window.set_weld_corner(corner_index as i32);
        window.invoke_preview_edit();
        assert!(window.get_ready());
        assert_eq!(window.get_weld_target_mode(), 1);
        save_ui_snapshot(window.window(), "brush-weld-corner");
        window.invoke_apply();
        lock(&shared).undo(Actor::User).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &original);
        window.invoke_cancel();
    }

    #[test]
    fn component_split_controls_show_new_corners_validate_rise_and_restore_selection() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let mut state = Session::new(Err("no assets needed".into()));
        let ObjectId::Solid(id) = state
            .create_block(
                Actor::Agent,
                DVec3::ZERO,
                DVec3::splat(64.0),
                "brick/test",
                false,
            )
            .unwrap()
        else {
            unreachable!()
        };
        let original = state.document().vmf().clone();
        let shared = Arc::new(Mutex::new(state));
        let window = ComponentsWindow::new().unwrap();
        components_ui::bind(&window, &shared);
        window.set_kind(2);
        window.set_operation(2);
        components_ui::refresh(&window, &lock(&shared));
        let top = window
            .get_rows()
            .iter()
            .find(|row| row.label.starts_with("Top"))
            .unwrap();
        window.invoke_choose(top.key);
        window.set_texture_lock(true);
        window.invoke_preview_edit();
        assert!(window.get_ready());
        assert!(window.get_topology_preview());
        assert_eq!(
            lock(&shared).component_report(&[id]).unwrap()["texture_mode"],
            "attached"
        );
        assert_eq!(window.get_kind(), 1);
        assert_eq!(window.get_rows().row_count(), 9);
        assert!(
            window
                .get_preview_summary()
                .contains("Add a corner and 3 faces")
        );
        let draft = lock(&shared).render_document().vmf().clone();
        window.show().unwrap();
        save_ui_snapshot(window.window(), "brush-face-split");
        for label in [
            "Apply split",
            "Discard preview",
            "Cancel",
            "Keep textures at corners",
        ] {
            assert_dialog_button_visible(&window, label);
        }
        window.set_split_rise("0".into());
        window.invoke_edited();
        window.invoke_preview_edit();
        assert!(!window.get_ready());
        assert!(!window.get_message().is_empty());
        assert_eq!(lock(&shared).render_document().vmf(), &draft);
        window.invoke_apply();
        assert_eq!(lock(&shared).document().vmf(), &original);
        window.invoke_choose_rise(32.0);
        assert_eq!(window.get_split_rise(), "32");
        assert!(!window.get_ready());
        window.invoke_preview_edit();
        assert!(window.get_ready());
        window.invoke_discard_preview();
        assert_eq!(window.get_kind(), 2);
        assert_eq!(lock(&shared).component_selection().len(), 1);
        assert_eq!(lock(&shared).document().vmf(), &original);
        window.invoke_preview_edit();
        window.invoke_apply();
        assert_eq!(window.get_kind(), 1);
        assert_eq!(lock(&shared).component_selection().len(), 1);
        lock(&shared).undo(Actor::Agent).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &original);

        window.set_kind(0);
        window.invoke_refresh();
        let shape = lock(&shared).component_topology(&[id]).unwrap().remove(0);
        let edge = shape
            .edges
            .iter()
            .find(|edge| {
                edge.vertices
                    .iter()
                    .all(|index| shape.vertices[*index].position.z == 64.0)
            })
            .unwrap();
        let key = format!(
            "{}:{}",
            id,
            edge.sides
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(",")
        );
        window.invoke_choose(key.into());
        window.set_split_rise("16".into());
        window.invoke_preview_edit();
        assert!(window.get_ready());
        window.show().unwrap();
        save_ui_snapshot(window.window(), "brush-edge-split");
        window.invoke_apply();
        assert_eq!(
            lock(&shared).component_topology(&[id]).unwrap()[0]
                .faces
                .len(),
            10
        );
        lock(&shared).undo(Actor::Agent).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &original);
        window.invoke_cancel();
    }

    #[test]
    fn ridge_controls_share_edge_choices_valid_drafts_cancel_apply_and_undo() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let mut state = Session::new(Err("no assets needed".into()));
        state
            .create_block(
                Actor::Agent,
                DVec3::ZERO,
                DVec3::splat(64.0),
                "brick/test",
                false,
            )
            .unwrap();
        let original = state.document().vmf().clone();
        let shared = Arc::new(Mutex::new(state));
        let window = ComponentsWindow::new().unwrap();
        components_ui::bind(&window, &shared);
        window.set_kind(2);
        window.set_operation(5);
        components_ui::refresh(&window, &lock(&shared));
        let top = window
            .get_rows()
            .iter()
            .find(|row| row.label.starts_with("Top"))
            .unwrap();
        window.invoke_choose(top.key);
        assert_eq!(window.get_crease_rows().row_count(), 1);
        let row = window.get_crease_rows().row_data(0).unwrap();
        assert_eq!(row.choices.row_count(), 2);
        assert!(row.choices.row_data(0).unwrap().contains("direction"));
        assert!(row.choices.row_data(1).unwrap().contains("direction"));
        window.invoke_preview_edit();
        assert!(window.get_ready());
        assert!(window.get_topology_preview());
        assert_eq!(window.get_kind(), 1);
        assert_eq!(window.get_rows().row_count(), 10);
        assert!(
            window
                .get_preview_summary()
                .contains("Add 2 corners and 1 face")
        );
        let draft = lock(&shared).render_document().vmf().clone();
        assert_eq!(lock(&shared).document().vmf(), &original);
        window.show().unwrap();
        save_ui_snapshot(window.window(), "brush-roof-ridge");
        for label in ["Apply ridge", "Discard preview", "Cancel", "Ridge rise"] {
            assert_dialog_button_visible(&window, label);
        }
        window.set_split_rise("0".into());
        window.invoke_edited();
        window.invoke_preview_edit();
        assert!(!window.get_ready());
        assert_eq!(lock(&shared).render_document().vmf(), &draft);
        window.invoke_apply();
        assert_eq!(lock(&shared).document().vmf(), &original);
        window.invoke_choose_rise(24.0);
        window.invoke_choose_crease_pair(row.key.clone(), 1);
        window.invoke_preview_edit();
        assert!(window.get_ready());
        assert_ne!(lock(&shared).render_document().vmf(), &draft);
        window.invoke_discard_preview();
        assert_eq!(window.get_kind(), 2);
        assert_eq!(lock(&shared).component_selection().len(), 1);
        assert_eq!(lock(&shared).document().vmf(), &original);
        window.invoke_preview_edit();
        window.invoke_apply();
        assert_eq!(lock(&shared).component_selection().len(), 2);
        lock(&shared).undo(Actor::Agent).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &original);
        window.set_kind(2);
        window.invoke_refresh();
        let top = window
            .get_rows()
            .iter()
            .find(|row| row.label.starts_with("Top"))
            .unwrap();
        window.invoke_choose(top.key);
        let report = lock(&shared).component_report(&[]).unwrap();
        let edges = report["crease_choices"][0]["pairs"][0]["edges"]
            .as_array()
            .unwrap()
            .iter()
            .map(|value| value.as_u64().unwrap() as u32)
            .collect::<Vec<_>>();
        let id = report["crease_choices"][0]["component"]["solid"]
            .as_u64()
            .unwrap() as u32;
        {
            let mut session = lock(&shared);
            let stamp = session.component_stamp();
            session
                .preview_component_crease(
                    &[session::components::CreaseEdges {
                        solid: id,
                        edges: [edges[0], edges[1]],
                    }],
                    12.5,
                    &stamp,
                )
                .unwrap();
        }
        components_ui::refresh(&window, &lock(&shared));
        assert_eq!(window.get_split_rise(), "12.5");
        assert_eq!(window.get_crease_rows().row_data(0).unwrap().choice, 0);
        assert!(window.get_ready());
        window.invoke_cancel();
        assert_eq!(lock(&shared).document().vmf(), &original);
        assert!(lock(&shared).component_report(&[]).unwrap()["preview"].is_null());
    }

    #[test]
    fn component_weld_controls_preview_discard_apply_and_undo() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let mut state = Session::new(Err("no assets needed".into()));
        let ObjectId::Solid(id) = state
            .create_block(
                Actor::Agent,
                DVec3::ZERO,
                DVec3::splat(64.0),
                "brick/test",
                false,
            )
            .unwrap()
        else {
            unreachable!()
        };
        let original = state.document().vmf().clone();
        let shared = Arc::new(Mutex::new(state));
        let window = ComponentsWindow::new().unwrap();
        components_ui::bind(&window, &shared);
        window.set_kind(1);
        window.set_operation(1);
        components_ui::refresh(&window, &lock(&shared));
        let shape = lock(&shared).component_topology(&[id]).unwrap().remove(0);
        for vertex in shape
            .vertices
            .iter()
            .filter(|vertex| vertex.position.z == 64.0)
        {
            let key = format!(
                "{}:{}",
                id,
                vertex
                    .sides
                    .iter()
                    .map(u32::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            );
            window.invoke_choose(key.into());
        }
        window.invoke_preview_edit();
        assert!(window.get_ready());
        assert!(window.get_topology_preview());
        assert!(window.get_summary().contains("1 selected"));
        assert!(
            window
                .get_preview_summary()
                .contains("Merge 4 corners into 1")
        );
        assert_eq!(lock(&shared).document().vmf(), &original);
        let draft = lock(&shared).render_document().vmf().clone();
        window.show().unwrap();
        save_ui_snapshot(window.window(), "brush-weld");
        assert_dialog_button_visible(&window, "Cancel");
        assert_dialog_button_visible(&window, "Apply weld");
        assert_dialog_button_visible(&window, "Discard preview");
        window.invoke_discard_preview();
        assert!(!window.get_topology_preview());
        assert!(!window.get_ready());
        assert_eq!(lock(&shared).component_selection().len(), 4);
        assert_eq!(lock(&shared).document().vmf(), &original);
        window.invoke_preview_edit();
        window.invoke_apply();
        assert_eq!(lock(&shared).document().vmf(), &draft);
        assert_eq!(lock(&shared).component_selection().len(), 1);
        lock(&shared).undo(Actor::Agent).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &original);
        window.invoke_refresh();
        window.invoke_cancel();
    }

    #[test]
    fn carving_multiple_cutters_exposes_roles_and_preserves_preview_cancel_apply() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                mock_time: true,
                renderer_name: Some("software".into()),
                threading: false,
            },
        )))
        .unwrap();
        let mut state = Session::new(Err("no assets needed".into()));
        let mut ids = Vec::new();
        for (mins, maxs) in [
            (DVec3::ZERO, DVec3::splat(128.0)),
            (DVec3::splat(16.0), DVec3::splat(80.0)),
            (DVec3::splat(48.0), DVec3::splat(112.0)),
            (DVec3::splat(256.0), DVec3::splat(288.0)),
        ] {
            ids.push(
                state
                    .create_block(Actor::Agent, mins, maxs, "brick/test", false)
                    .unwrap(),
            );
        }
        state.select(&ids, SelectMode::Replace).unwrap();
        let original = state.document().vmf().clone();
        let shared = Arc::new(Mutex::new(state));
        let window = CarveWindow::new().unwrap();
        carve_ui::bind(&window, &shared);
        carve_ui::refresh(&window, &lock(&shared));
        window.set_cutter(1);
        window.invoke_choose_cutter();
        window.set_multiple_cutters(true);
        window.invoke_toggle_multiple();
        let second = window.get_targets().row_data(2).unwrap().solid;
        let ignored = window.get_targets().row_data(3).unwrap().solid;
        window.invoke_choose_cut_role(second.clone());
        window.invoke_choose_target(ignored);
        window.invoke_preview_edit();
        assert!(window.get_ready());
        assert!(window.get_multiple_cutters());
        assert_eq!(
            window.get_targets().iter().filter(|row| row.cutter).count(),
            2
        );
        assert!(!window.get_targets().row_data(3).unwrap().selected);
        let draft = lock(&shared).render_document().vmf().clone();
        assert_eq!(lock(&shared).document().vmf(), &original);
        window.show().unwrap();
        save_ui_snapshot(window.window(), "carving-multiple-cutters");
        for label in [
            "Cancel",
            "Discard preview",
            "Preview carve",
            "Apply carve",
            "Use multiple cutter brushes",
        ] {
            assert_dialog_button_visible(&window, label);
        }
        window.invoke_choose_cut_role(second.clone());
        assert!(!window.get_ready());
        window.invoke_apply();
        assert_eq!(lock(&shared).document().vmf(), &original);
        window.invoke_choose_cut_role(second);
        window.invoke_preview_edit();
        window.invoke_discard();
        assert_eq!(lock(&shared).render_document().vmf(), &original);
        window.invoke_preview_edit();
        assert!(window.get_ready());
        window.invoke_apply();
        assert_eq!(lock(&shared).document().vmf(), &draft);
        lock(&shared).undo(Actor::User).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &original);
        lock(&shared).select(&ids, SelectMode::Replace).unwrap();
        carve_ui::refresh(&window, &lock(&shared));
        window.set_cutter(1);
        window.invoke_choose_cutter();
        window.set_multiple_cutters(true);
        window.invoke_toggle_multiple();
        window.invoke_choose_cut_role(window.get_targets().row_data(2).unwrap().solid);
        window.invoke_choose_target(window.get_targets().row_data(3).unwrap().solid);
        window.invoke_preview_edit();
        assert!(window.get_ready(), "{}", window.get_message());
        window.invoke_cancel();
        assert_eq!(lock(&shared).render_document().vmf(), &original);
    }

    #[test]
    fn carve_controls_keep_targets_show_preview_and_share_apply_discard_cancel() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                mock_time: true,
                renderer_name: Some("software".into()),
                threading: false,
            },
        )))
        .unwrap();
        let mut state = Session::new(Err("no assets needed".into()));
        let target = state
            .create_block(
                Actor::Agent,
                DVec3::ZERO,
                DVec3::splat(64.0),
                "brick/test",
                false,
            )
            .unwrap();
        let untouched = state
            .create_block(
                Actor::Agent,
                DVec3::splat(256.0),
                DVec3::splat(288.0),
                "brick/test",
                false,
            )
            .unwrap();
        let cutter = state
            .create_block(
                Actor::Agent,
                DVec3::splat(16.0),
                DVec3::splat(48.0),
                "brick/test",
                false,
            )
            .unwrap();
        state
            .select(&[target, untouched, cutter], SelectMode::Replace)
            .unwrap();
        let original = state.document().vmf().clone();
        let shared = Arc::new(Mutex::new(state));
        let window = CarveWindow::new().unwrap();
        carve_ui::bind(&window, &shared);
        carve_ui::refresh(&window, &lock(&shared));
        assert_eq!(window.get_cutter(), 2);
        let untouched_key = window.get_targets().row_data(1).unwrap().solid;
        window.invoke_choose_target(untouched_key.clone());
        window.invoke_preview_edit();
        assert!(window.get_ready());
        assert!(window.get_summary().contains("6 fragments"));
        assert_eq!(window.get_targets().row_count(), 3);
        assert!(!window.get_targets().row_data(1).unwrap().selected);
        assert_eq!(lock(&shared).document().vmf(), &original);
        let draft = lock(&shared).render_document().vmf().clone();
        window.show().unwrap();
        save_ui_snapshot(window.window(), "brush-carving");
        for label in ["Cancel", "Discard preview", "Preview carve", "Apply carve"] {
            assert_dialog_button_visible(&window, label);
        }
        let target_key = window.get_targets().row_data(0).unwrap().solid;
        window.invoke_choose_target(target_key.clone());
        assert!(!window.get_ready());
        assert!(!window.get_can_preview());
        window.invoke_apply();
        assert_eq!(lock(&shared).document().vmf(), &original);
        window.invoke_choose_target(target_key);
        window.invoke_preview_edit();
        assert!(window.get_ready());
        window.invoke_discard();
        assert!(!window.get_ready());
        assert_eq!(lock(&shared).render_document().vmf(), &original);
        window.invoke_preview_edit();
        assert!(window.get_ready());
        window.invoke_apply();
        assert_eq!(lock(&shared).document().vmf(), &draft);
        lock(&shared).undo(Actor::User).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &original);
        lock(&shared)
            .select(&[target, untouched, cutter], SelectMode::Replace)
            .unwrap();
        carve_ui::refresh(&window, &lock(&shared));
        window.invoke_preview_edit();
        assert!(window.get_ready());
        window.invoke_cancel();
        assert!(!window.window().is_visible());
        assert_eq!(lock(&shared).render_document().vmf(), &original);
        assert!(lock(&shared).carve_state()["preview"].is_null());
    }

    #[test]
    fn hollow_controls_preview_validate_look_inside_apply_and_cancel() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let mut state = Session::new(Err("no assets needed".into()));
        state
            .create_block(
                Actor::Agent,
                DVec3::ZERO,
                DVec3::splat(256.0),
                "brick/test",
                false,
            )
            .unwrap();
        let original = state.document().vmf().clone();
        let shared = Arc::new(Mutex::new(state));
        let window = HollowWindow::new().unwrap();
        hollow_ui::bind(&window, &shared);
        hollow_ui::refresh(&window, &lock(&shared));
        window.invoke_preview_edit();
        assert!(window.get_ready());
        assert_eq!(lock(&shared).document().vmf(), &original);
        let valid = lock(&shared).render_document().vmf().clone();
        window.show().unwrap();
        save_ui_snapshot(window.window(), "hollow-brushes");
        for label in ["Cancel", "Create walls", "Preview walls", "Look inside"] {
            assert_dialog_button_visible(&window, label);
        }
        window.invoke_look_inside();
        assert_eq!(lock(&shared).camera().position, Vec3::splat(128.0));
        window.set_thickness("128".into());
        window.invoke_edited();
        window.invoke_preview_edit();
        assert!(!window.get_ready());
        assert!(!window.get_message().is_empty());
        assert_eq!(lock(&shared).render_document().vmf(), &valid);
        window.invoke_apply();
        assert_eq!(lock(&shared).document().vmf(), &original);
        window.set_thickness("16".into());
        window.invoke_preview_edit();
        window.invoke_apply();
        assert_eq!(lock(&shared).document().vmf(), &valid);
        lock(&shared).undo(Actor::Agent).unwrap();
        hollow_ui::refresh(&window, &lock(&shared));
        window.invoke_preview_edit();
        window.invoke_cancel();
        assert!(!window.window().is_visible());
        assert_eq!(lock(&shared).document().vmf(), &original);
        assert_eq!(lock(&shared).render_document().vmf(), &original);
        hollow_ui::refresh(&window, &lock(&shared));
        let top = window
            .get_faces()
            .iter()
            .find(|face| face.label.starts_with("Top"))
            .unwrap();
        window.invoke_choose_opening(top.key);
        window.invoke_preview_edit();
        assert!(window.get_ready());
        assert_eq!(
            lock(&shared).hollow_state()["preview"]["result"]["brushes"]
                .as_array()
                .unwrap()
                .len(),
            5
        );
        window.show().unwrap();
        window.window().take_snapshot().unwrap();
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(250));
        save_ui_snapshot(window.window(), "hollow-openings");
        let open = lock(&shared).render_document().vmf().clone();
        for row in window
            .get_faces()
            .iter()
            .filter(|face| !face.open)
            .collect::<Vec<_>>()
        {
            window.invoke_choose_opening(row.key);
        }
        window.invoke_preview_edit();
        assert!(!window.get_ready());
        assert!(window.get_message().contains("at least one wall"));
        assert_eq!(lock(&shared).render_document().vmf(), &open);
        assert_dialog_button_visible(&window, "Close all openings");
        window.invoke_close_openings();
        window.invoke_preview_edit();
        assert!(window.get_ready());
        assert_eq!(
            lock(&shared).hollow_state()["preview"]["result"]["brushes"]
                .as_array()
                .unwrap()
                .len(),
            6
        );
        window.invoke_cancel();
        assert_eq!(lock(&shared).render_document().vmf(), &original);
    }

    fn save_ui_snapshot(window: &slint::Window, name: &str) {
        window.take_snapshot().unwrap();
        i_slint_backend_testing::mock_elapsed_time(Duration::from_millis(250));
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

    #[test]
    fn saved_view_controls_restore_rename_replace_cancel_and_guard_map_changes() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let shared = Arc::new(Mutex::new(Session::new(Err("no assets".into()))));
        let controller = saved_views_ui::Controller::new(&shared).unwrap();
        controller.show(&lock(&shared)).unwrap();
        controller.window.set_view_name("Courtyard entrance".into());
        controller.window.invoke_save_new();
        let original = lock(&shared).camera();
        assert_eq!(lock(&shared).camera_views().unwrap().views.len(), 1);
        lock(&shared).set_projection(Actor::Agent, render::Projection::Top);
        controller.window.set_view_name("Top · route layout".into());
        controller.window.invoke_save_new();
        assert_eq!(lock(&shared).camera_views().unwrap().views.len(), 2);
        controller.window.invoke_choose("1".into());
        let history = lock(&shared).document().undo_labels().count();
        controller.window.invoke_restore_view();
        assert_eq!(lock(&shared).camera(), original);
        assert_eq!(lock(&shared).document().undo_labels().count(), history);
        controller.window.set_view_name("Entrance overview".into());
        controller.window.invoke_rename_view();
        assert_eq!(
            lock(&shared).camera_views().unwrap().views[0].name,
            "Entrance overview"
        );
        lock(&shared).set_projection(Actor::Agent, render::Projection::Front);
        controller.window.invoke_replace_view();
        assert_eq!(
            lock(&shared).camera_views().unwrap().views[0].projection,
            "front"
        );
        let before = lock(&shared).document().vmf().clone();
        controller.window.set_view_name("top · route layout".into());
        controller.window.invoke_rename_view();
        assert!(controller.window.get_message().contains("already"));
        assert_eq!(lock(&shared).document().vmf(), &before);
        controller.window.invoke_choose("1".into());
        save_ui_snapshot(controller.window.window(), "saved-views");
        controller.window.invoke_close_dialog();
        assert_eq!(lock(&shared).document().vmf(), &before);
        controller.show(&lock(&shared)).unwrap();
        controller.window.invoke_choose("1".into());
        lock(&shared)
            .rename_camera_view(Actor::Agent, 1, "Agent name", None)
            .unwrap();
        controller.window.invoke_delete_view();
        assert!(controller.window.get_message().contains("Map changed"));
        controller.refresh(&lock(&shared));
        assert!(controller.window.get_selected_id().is_empty());
        controller.window.invoke_choose("1".into());
        controller.window.invoke_delete_view();
        assert_eq!(lock(&shared).camera_views().unwrap().views.len(), 1);
        lock(&shared).undo(Actor::User).unwrap();
        assert_eq!(lock(&shared).camera_views().unwrap().views.len(), 2);
    }

    #[test]
    fn compile_controls_share_cancel_status_and_guard_replaced_jobs() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let (directory, mut game, _) = game::tests::fixture(true);
        let materials = directory.path().join("materials");
        std::fs::create_dir(&materials).unwrap();
        std::fs::write(materials.join("water.vmt"), "Water { $abovewater 1 }").unwrap();
        std::fs::write(
            materials.join("nodraw.vmt"),
            "UnlitGeneric { %compilenodraw 1 }",
        )
        .unwrap();
        std::fs::write(materials.join("sky.vmt"), "UnlitGeneric { %compilesky 1 }").unwrap();
        std::fs::create_dir(materials.join("dev")).unwrap();
        std::fs::write(
            materials.join("dev/dev_measuregeneric01.vmt"),
            "LightmappedGeneric {}",
        )
        .unwrap();
        game.mounts.push(source_formats::gamefs::MountedPath {
            ids: vec!["game".into()],
            mount: source_formats::gamefs::Mount::Directory(directory.path().to_owned()),
        });
        let physics_root = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .map(|path| path.join(".local/share/Steam/steamapps/common/Counter-Strike Source"));
        let native_physics = physics_root
            .as_ref()
            .is_some_and(|root| root.join("bin/linux64/vphysics.so").is_file())
            && std::env::current_exe()
                .unwrap()
                .parent()
                .unwrap()
                .parent()
                .unwrap()
                .join("hammerr-physics")
                .is_file();
        if native_physics {
            game.base_dir = physics_root.unwrap();
        }
        let shared = Arc::new(Mutex::new(Session::new(Ok(game))));
        let controller = compile_ui::Controller::new(&shared).unwrap();
        controller.show(&lock(&shared)).unwrap();
        assert_eq!(controller.window.get_lighting_choice(), 0);
        assert_eq!(controller.window.get_selected_lighting(), "Fullbright");
        assert_eq!(
            controller.window.get_selected_physics(),
            "No physics bodies"
        );
        controller.window.invoke_change_physics(1);
        controller.refresh(&lock(&shared));
        assert_eq!(
            controller.window.get_selected_physics(),
            "CS:S engine physics"
        );
        assert_eq!(lock(&shared).compile_settings()["physics"], "engine");
        controller.window.invoke_change_physics(0);
        controller.refresh(&lock(&shared));
        assert_eq!(
            controller.window.get_selected_physics(),
            "No physics bodies"
        );
        controller.window.invoke_change_lighting(1);
        assert_eq!(lock(&shared).compile_settings()["lighting"], "direct_ldr");
        controller.refresh(&lock(&shared));
        assert_eq!(controller.window.get_lighting_choice(), 1);
        assert_eq!(controller.window.get_selected_lighting(), "Direct LDR");
        assert!(
            controller
                .window
                .get_lighting_description()
                .contains("static point lights")
        );
        controller.window.invoke_change_lighting(0);
        controller.refresh(&lock(&shared));
        assert!(controller.window.get_summary().contains("No build"));
        assert!(!controller.window.get_launch_ready());
        let path = directory.path().join("room.bsp");
        let request = lock(&shared)
            .begin_compile(Actor::User, Some(&path))
            .unwrap();
        controller.refresh(&lock(&shared));
        assert!(controller.window.get_cancellable());
        assert!(!controller.window.get_launch_ready());
        assert!(controller.window.get_summary().contains("Waiting"));
        controller.window.invoke_cancel_build();
        let result = request.run();
        lock(&shared).finish_compile(Actor::User, &result);
        controller.refresh(&lock(&shared));
        assert!(controller.window.get_summary().contains("cancelled"));
        assert!(!controller.window.get_cancellable());
        let request = lock(&shared)
            .begin_compile(Actor::Agent, Some(&path))
            .unwrap();
        controller.window.invoke_cancel_build();
        assert!(controller.window.get_message().contains("replaced"));
        assert_eq!(lock(&shared).compile_status()["job"]["status"], "running");
        let result = request.run();
        lock(&shared).finish_compile(Actor::Agent, &result);
        controller.refresh(&lock(&shared));
        assert!(controller.window.get_summary().contains("failed"));
        assert!(!controller.window.get_diagnostics().is_empty());
        let vmf_path = directory.path().join("room.vmf");
        let mut room = map_core::Document::new(test_assets::room());
        room.edit("cubemap sample", |tx| {
            map_core::edit::create_entity(
                tx,
                "env_cubemap",
                Some(DVec3::new(0.0, 0.0, 64.0)),
                &[("cubemapsize".into(), "9".into())],
                Vec::new(),
            )?;
            Ok(())
        })
        .unwrap();
        room.edit("pool", |tx| {
            let ObjectId::Solid(id) = map_core::edit::create_block(
                tx,
                DVec3::new(-128.0, -128.0, 0.0),
                DVec3::new(128.0, 128.0, 64.0),
                "nodraw",
            )?
            else {
                unreachable!()
            };
            for side in &mut tx.solid_mut(id)?.sides {
                if map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99 {
                    side.material = "water".into();
                }
            }
            Ok(())
        })
        .unwrap();
        room.edit("sky ceiling", |tx| {
            let id = tx
                .vmf()
                .world
                .solids
                .iter()
                .find(|solid| solid.sides.iter().all(|side| side.plane.0[0].z >= 256.0))
                .unwrap()
                .id;
            for side in &mut tx.solid_mut(id)?.sides {
                if map_core::Plane::from_points(side.plane.0).unwrap().normal.z < -0.99 {
                    side.material = "sky".into();
                }
            }
            Ok(())
        })
        .unwrap();
        room.edit("baked point light", |tx| {
            map_core::edit::create_entity(
                tx,
                "light",
                Some(DVec3::new(0.0, -160.0, 128.0)),
                &[
                    ("_light".into(), "255 170 100 300".into()),
                    ("_quadratic_attn".into(), "1".into()),
                ],
                Vec::new(),
            )
        })
        .unwrap();
        std::fs::write(&vmf_path, room.vmf().write(&Default::default())).unwrap();
        lock(&shared).open(Actor::User, &vmf_path, true).unwrap();
        controller.window.invoke_change_lighting(1);
        if native_physics {
            controller.window.invoke_change_physics(1);
        }
        let request = lock(&shared)
            .begin_compile(Actor::Agent, Some(&path))
            .unwrap();
        let result = request.run();
        lock(&shared).finish_compile(Actor::Agent, &result);
        controller.show(&lock(&shared)).unwrap();
        assert!(controller.window.get_summary().starts_with("Compiled in"));
        assert!(
            controller
                .window
                .get_details()
                .contains("Cubemap samples: 1")
        );
        assert_eq!(
            lock(&shared).compile_status()["job"]["result"]["stats"]["cubemap_samples"],
            1
        );
        assert!(controller.window.get_details().contains("Visibility:"));
        let stats = lock(&shared).compile_status()["job"]["result"]["stats"].clone();
        assert_eq!(stats["lighting_mode"], "direct_ldr");
        assert_eq!(stats["world_lights"], 1);
        if native_physics {
            assert_eq!(stats["physics_mode"], "engine");
            assert!(stats["physics_convexes"].as_u64().unwrap() > 0);
            assert!(controller.window.get_details().contains("brush hulls"));
        }
        assert!(controller.window.get_details().contains("Baked lighting:"));
        assert!(stats["clusters"].as_u64().unwrap() > 1);
        assert_eq!(stats["water_regions"], 1);
        let leaves = stats["water_leaves"].as_u64().unwrap();
        assert!(leaves > 0);
        assert!(
            controller
                .window
                .get_details()
                .contains(&format!("Water: 1 region · {leaves} submerged"))
        );
        assert!(
            controller
                .window
                .get_diagnostics()
                .contains("physics fluid volumes")
        );
        let sky = stats["sky_clusters"].as_u64().unwrap();
        assert!(sky > 0);
        assert!(
            controller
                .window
                .get_details()
                .contains(&format!("Sky visible from: {sky} clusters"))
        );
        assert!(controller.window.get_launch_ready());
        assert!(controller.window.get_launch_details().contains("room.bsp"));
        assert!(
            controller
                .window
                .get_details()
                .contains("current map snapshot")
        );
        save_ui_snapshot(controller.window.window(), "build-status");
        assert_eq!(controller.window.get_selected_lighting(), "Direct LDR");
        controller.window.invoke_close_dialog();
        assert!(!controller.window.window().is_visible());
        assert_eq!(lock(&shared).compile_status()["job"]["status"], "completed");
        controller.window.invoke_play_build();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while lock(&shared).launching() {
            assert!(
                std::time::Instant::now() < deadline,
                "game launch timed out"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(lock(&shared).game_running());
        controller.refresh(&lock(&shared));
        assert!(!controller.window.get_launch_ready());
        lock(&shared).stop_game(Actor::User).unwrap();
        controller.refresh(&lock(&shared));
        assert!(controller.window.get_launch_ready());
        lock(&shared).new_map(Actor::Agent, true).unwrap();
        controller.window.invoke_play_build();
        assert!(!lock(&shared).launching());
        controller.refresh(&lock(&shared));
        assert!(!controller.window.get_launch_ready());
    }

    #[test]
    fn volume_controls_preview_frame_cancel_and_guard_shared_selection() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let mut session = Session::new(Err("no assets".into()));
        let a = session
            .create_block(
                Actor::User,
                DVec3::ZERO,
                DVec3::splat(64.0),
                "brick/test",
                false,
            )
            .unwrap();
        let b = session
            .create_block(
                Actor::User,
                DVec3::new(128.0, 0.0, 0.0),
                DVec3::new(192.0, 64.0, 64.0),
                "brick/test",
                false,
            )
            .unwrap();
        let before = session.document().vmf().clone();
        let history = session.document().undo_labels().count();
        let shared = Arc::new(Mutex::new(session));
        let controller = volume_ui::Controller::new(&shared, || 0.4).unwrap();
        controller.initialize(&mut lock(&shared));
        let window = &controller.window;
        window.show().unwrap();
        window.invoke_preview_edit();
        assert!(window.get_ready());
        assert_eq!(lock(&shared).selected_ids(), [b]);
        window.invoke_frame();
        assert!(lock(&shared).volume_preview().is_some());
        {
            let session = lock(&shared);
            let spec = session.volume_preview().unwrap().spec;
            let matrix = session.camera().view_projection(0.4);
            for x in [spec.mins.x, spec.maxs.x] {
                for y in [spec.mins.y, spec.maxs.y] {
                    for z in [spec.mins.z, spec.maxs.z] {
                        let point = matrix.project_point3(Vec3::new(x as f32, y as f32, z as f32));
                        assert!(point.x.abs() <= 1.0001 && point.y.abs() <= 1.0001);
                    }
                }
            }
        }
        save_ui_snapshot(window.window(), "volume-selection");
        for label in [
            "Cancel",
            "Frame box",
            "Preview matches",
            "Select matches",
            "Use selection bounds",
        ] {
            assert_dialog_button_visible(window, label);
        }
        let foreign = lock(&shared)
            .preview_volume_selection(session::volume::Spec {
                mins: DVec3::splat(-1.0),
                maxs: DVec3::splat(65.0),
                rule: map_core::region::Match::Contained,
                mode: SelectMode::Replace,
                faces: false,
            })
            .unwrap();
        controller.refresh(&lock(&shared));
        assert!(!window.get_ready());
        window.invoke_apply();
        assert_eq!(lock(&shared).selected_ids(), [b]);
        window.invoke_cancel();
        assert_eq!(lock(&shared).volume_preview().unwrap().token, foreign);
        controller.initialize(&mut lock(&shared));
        window.set_min_x("0".into());
        window.set_max_x("64".into());
        window.invoke_edited();
        window.invoke_preview_edit();
        assert!(window.get_ready());
        window.invoke_apply();
        assert_eq!(lock(&shared).selected_ids(), [a]);
        assert_eq!(lock(&shared).document().vmf(), &before);
        assert_eq!(lock(&shared).document().undo_labels().count(), history);
        controller.initialize(&mut lock(&shared));
        window.set_max_x("-100".into());
        window.invoke_edited();
        window.invoke_preview_edit();
        assert!(!window.get_ready());
        assert!(window.get_message().contains("positive"));
        window.invoke_use_bounds();
        window.invoke_preview_edit();
        assert!(window.get_ready());
        lock(&shared).new_map(Actor::Agent, true).unwrap();
        controller.refresh(&lock(&shared));
        assert!(!window.get_ready());
        window.invoke_apply();
        assert!(lock(&shared).selected_ids().is_empty());
    }

    #[test]
    fn grouped_surface_controls_apply_exact_preview_and_preserve_foreign_drafts() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let (_directory, game) = test_assets::game();
        let mut session = Session::new(Ok(game));
        let mut faces = Vec::new();
        for x in [0.0, 128.0] {
            let ObjectId::Solid(solid) = session
                .create_block(
                    Actor::User,
                    DVec3::new(x, 0.0, 0.0),
                    DVec3::new(x + 128.0, 64.0, 16.0),
                    "test/checker",
                    false,
                )
                .unwrap()
            else {
                panic!()
            };
            let side = session
                .document()
                .solid(solid)
                .unwrap()
                .sides
                .iter()
                .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal == DVec3::Z)
                .unwrap()
                .id;
            faces.push(map_core::surface::FaceId { solid, side });
        }
        session.select_faces(&faces, SelectMode::Replace).unwrap();
        let before = session.document().vmf().clone();
        let shared = Arc::new(Mutex::new(session));
        let controller = surface_group_ui::Controller::new(&shared).unwrap();
        controller.initialize(&mut lock(&shared)).unwrap();
        let window = &controller.window;
        window.show().unwrap();
        window.invoke_preview_edit();
        assert!(lock(&shared).has_face_preview());
        save_ui_snapshot(window.window(), "grouped-textures");
        for label in [
            "Cancel",
            "Choose material…",
            "Preview mapping",
            "Apply mapping",
        ] {
            assert_dialog_button_visible(window, label);
        }
        window.invoke_cancel();
        assert_eq!(lock(&shared).document().vmf(), &before);
        assert!(!lock(&shared).has_face_preview());
        window.invoke_preview_edit();
        lock(&shared)
            .edit_faces(
                Actor::Agent,
                &faces,
                map_core::surface::SurfaceEdit {
                    shift_u: Some(25.0),
                    ..Default::default()
                },
                false,
                true,
            )
            .unwrap();
        let foreign = lock(&shared).face_preview_token();
        window.invoke_apply();
        assert!(window.get_message().contains("Another face preview"));
        window.invoke_cancel();
        assert_eq!(lock(&shared).face_preview_token(), foreign);
        window.set_projection(1);
        window.invoke_edited();
        window.set_repeats_u("2".into());
        window.invoke_preview_edit();
        assert_ne!(lock(&shared).face_preview_token(), foreign);
        save_ui_snapshot(window.window(), "view-projected-textures");
        let draft = lock(&shared).face_preview_document().unwrap().vmf().clone();
        window.invoke_apply();
        assert_eq!(lock(&shared).document().vmf(), &draft);
        lock(&shared).undo(Actor::Agent).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &before);
        controller.initialize(&mut lock(&shared)).unwrap();
        lock(&shared)
            .select_faces(&faces[..1], SelectMode::Replace)
            .unwrap();
        controller.refresh(&lock(&shared));
        assert!(window.get_stale());
        window.invoke_apply();
        assert_eq!(lock(&shared).document().vmf(), &before);
    }

    #[test]
    fn material_replacement_controls_preview_cancel_and_reject_foreign_drafts() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
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
        let before = session.document().vmf().clone();
        let shared = std::sync::Arc::new(std::sync::Mutex::new(session));
        let controller = material_replace_ui::Controller::new(&shared).unwrap();
        controller.initialize(&mut lock(&shared), "dev/dev_measuregeneric01");
        let window = &controller.window;
        window.invoke_find();
        assert!(window.get_ready());
        controller.choose_material(&mut lock(&shared), true, "dev/dev_measurewall01a");
        window.invoke_preview_edit();
        assert!(lock(&shared).has_face_preview());
        assert_eq!(lock(&shared).document().vmf(), &before);
        window.show().unwrap();
        save_ui_snapshot(window.window(), "material-replacement");
        for id in ["source-thumbnail", "replacement-thumbnail"] {
            let image = i_slint_backend_testing::ElementHandle::find_by_element_id(
                window,
                &format!("MaterialReplaceWindow::{id}"),
            )
            .next()
            .unwrap();
            assert!(image.size().width <= 80.0 && image.size().height <= 80.0);
        }

        for label in [
            "Cancel",
            "Find faces",
            "Select matches",
            "Preview replacement",
            "Replace faces",
        ] {
            assert_dialog_button_visible(window, label);
        }
        window.invoke_cancel();
        assert!(!lock(&shared).has_face_preview());
        assert_eq!(lock(&shared).document().vmf(), &before);
        window.invoke_preview_edit();
        let token = lock(&shared).material_search_report().unwrap().token;
        lock(&shared)
            .replace_materials(Actor::Agent, token, "wood/foreign", true)
            .unwrap();
        controller.refresh(&lock(&shared));
        assert!(!window.get_can_apply());
        window.invoke_apply();
        assert!(window.get_message().contains("Another face preview"));
        let foreign = lock(&shared).face_preview_token();
        window.invoke_cancel();
        assert_eq!(lock(&shared).face_preview_token(), foreign);
        window.invoke_preview_edit();
        window.set_replacement("metal/changed".into());
        window.invoke_replacement_edited();
        assert!(!lock(&shared).has_face_preview());
        window.invoke_apply();
        assert!(
            lock(&shared)
                .document()
                .vmf()
                .solids()
                .flat_map(|solid| &solid.sides)
                .all(|side| side.material == "METAL/CHANGED")
        );
        lock(&shared).undo(Actor::User).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &before);
        window.invoke_find();
        lock(&shared).new_map(Actor::Agent, true).unwrap();
        controller.refresh(&lock(&shared));
        assert!(!window.get_ready());
        window.invoke_apply();
        assert!(lock(&shared).document().vmf().world.solids.is_empty());
    }

    #[test]
    fn lighting_controls_preserve_mixed_groups_guard_previews_and_cancel() {
        use map_core::surface::{self, FaceId, SurfaceEdit};
        use slint::Model;
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let mut session = Session::new(Err("no assets needed".into()));
        let ObjectId::Solid(id) = session
            .create_block(
                Actor::User,
                DVec3::ZERO,
                DVec3::splat(64.0),
                "brick/test",
                false,
            )
            .unwrap()
        else {
            panic!()
        };
        let faces = session.document().solid(id).unwrap().sides[..2]
            .iter()
            .map(|side| FaceId {
                solid: id,
                side: side.id,
            })
            .collect::<Vec<_>>();
        session
            .edit_faces(
                Actor::Agent,
                &faces[..1],
                SurfaceEdit {
                    lightmap_scale: Some(8),
                    smoothing_groups: Some(1),
                    ..Default::default()
                },
                false,
                false,
            )
            .unwrap();
        session
            .edit_faces(
                Actor::Agent,
                &faces[1..],
                SurfaceEdit {
                    smoothing_groups: Some(2),
                    ..Default::default()
                },
                false,
                false,
            )
            .unwrap();
        session.select_faces(&faces, SelectMode::Replace).unwrap();
        let before = session.document().vmf().clone();
        let shared = Arc::new(Mutex::new(session));
        let controller = lighting_ui::Controller::new(&shared).unwrap();
        controller.initialize(&mut lock(&shared)).unwrap();
        let window = &controller.window;
        assert!(window.get_lightmap_scale().is_empty());
        assert_eq!(window.get_groups().row_data(0).unwrap().state, -1);
        assert_eq!(window.get_groups().row_data(1).unwrap().state, -1);
        window.show().unwrap();
        window.invoke_toggle_group(2, true);
        window.set_lightmap_scale("4".into());
        window.invoke_edited();
        let original_history = lock(&shared).document().undo_labels().count();
        for mode in render::SurfaceView::ALL {
            window.set_view_mode(mode.index());
            window.invoke_view_changed();
            assert_eq!(lock(&shared).view().surface, mode);
            assert_eq!(
                lock(&shared).document().undo_labels().count(),
                original_history
            );
        }
        window.set_view_mode(render::SurfaceView::LightmapDensity.index());
        window.invoke_view_changed();
        window.invoke_preview_edit();
        assert_eq!(lock(&shared).document().vmf(), &before);
        for (face, mask) in faces.iter().zip([5, 6]) {
            let session = lock(&shared);
            let side = session.preview_side(*face).unwrap();
            assert_eq!(side.smoothing_groups, Some(mask));
            assert_eq!(side.lightmap_scale, 4);
        }
        controller.refresh(&lock(&shared));
        save_ui_snapshot(window.window(), "face-lighting");
        for label in ["Cancel", "Preview settings", "Apply settings"] {
            assert_dialog_button_visible(window, label);
        }
        window.set_lightmap_scale("0".into());
        window.invoke_edited();
        assert!(!lock(&shared).has_face_preview());
        window.invoke_preview_edit();
        assert!(!lock(&shared).has_face_preview());
        assert!(window.get_message().contains("1–1024"));
        window.set_lightmap_scale("8".into());
        window.invoke_edited();
        window.invoke_preview_edit();
        let old = lock(&shared).face_preview_token();
        lock(&shared)
            .edit_faces(
                Actor::Agent,
                &faces,
                SurfaceEdit {
                    lightmap_scale: Some(32),
                    ..Default::default()
                },
                false,
                true,
            )
            .unwrap();
        controller.refresh(&lock(&shared));
        assert!(!window.get_can_apply());
        window.invoke_apply();
        assert_eq!(lock(&shared).document().vmf(), &before);
        let replacement = lock(&shared).face_preview_token();
        assert_ne!(old, replacement);
        window.invoke_cancel();
        assert_eq!(lock(&shared).face_preview_token(), replacement);
        controller.initialize(&mut lock(&shared)).unwrap();
        window.show().unwrap();
        window.invoke_clear_groups();
        window.invoke_preview_edit();
        window.invoke_apply();
        for face in &faces {
            assert_eq!(
                surface::side(lock(&shared).document(), *face)
                    .unwrap()
                    .smoothing_groups,
                Some(0)
            );
        }
        lock(&shared).undo(Actor::User).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &before);
        controller.initialize(&mut lock(&shared)).unwrap();
        window.show().unwrap();
        window.invoke_toggle_group(31, true);
        window.invoke_reset_groups();
        assert_eq!(window.get_groups().row_data(31).unwrap().action, 0);
        window.invoke_preview_edit();
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::KeyPressed {
                text: slint::platform::Key::Escape.into(),
            });
        assert!(!window.window().is_visible());
        assert!(!lock(&shared).has_face_preview());
        assert_eq!(lock(&shared).document().vmf(), &before);

        controller.initialize(&mut lock(&shared)).unwrap();
        window.set_lightmap_scale("32".into());
        window.invoke_edited();
        lock(&shared)
            .select_faces(&faces[..1], SelectMode::Replace)
            .unwrap();
        controller.refresh(&lock(&shared));
        assert!(window.get_stale());
        window.invoke_apply();
        assert_eq!(lock(&shared).document().vmf(), &before);
        window.invoke_cancel();

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("imported-lightmap-scale.vmf");
        let mut imported = before.clone();
        for side in &mut imported.world.solids[0].sides {
            side.lightmap_scale = 2048;
        }
        std::fs::write(
            &path,
            imported.write(&source_formats::keyvalues::WriteOptions::default()),
        )
        .unwrap();
        lock(&shared).open(Actor::User, &path, true).unwrap();
        lock(&shared)
            .select_faces(&faces, SelectMode::Replace)
            .unwrap();
        controller.initialize(&mut lock(&shared)).unwrap();
        assert_eq!(window.get_lightmap_scale(), "2048");
        window.invoke_toggle_group(31, true);
        window.invoke_apply();
        for face in &faces {
            let session = lock(&shared);
            let side = surface::side(session.document(), *face).unwrap();
            assert_eq!(side.lightmap_scale, 2048);
            assert_ne!(side.smoothing_groups.unwrap() & (1 << 31), 0);
        }
    }

    #[test]
    fn primitive_controls_preview_validate_frame_apply_and_cancel_shared_geometry() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let session = Session::new(mount_game(None));
        let shared = Arc::new(Mutex::new(session));
        let window = PrimitiveWindow::new().unwrap();
        primitive_ui::bind(&window, &shared);
        primitive_ui::initialize(&window, &mut lock(&shared), "dev/dev_measuregeneric01");
        window.show().unwrap();
        let before = lock(&shared).document().vmf().clone();
        window.set_shape(4);
        window.set_axis(1);
        window.set_min_y("-16".into());
        window.set_max_y("16".into());
        window.set_min_z("-128".into());
        window.set_max_z("128".into());
        window.set_min_x("-128".into());
        window.set_max_x("128".into());
        window.invoke_edited();
        assert!(window.get_can_preview());
        window.invoke_preview_shape();
        assert!(!window.get_token().is_empty());
        assert_eq!(lock(&shared).document().vmf(), &before);
        assert_eq!(lock(&shared).primitive_preview().unwrap().ids.len(), 12);
        window.invoke_focus_preview();
        assert!(lock(&shared).primitive_preview().is_some());
        save_ui_snapshot(window.window(), "brush-shapes");
        for label in [
            "Preview shape",
            "Focus preview",
            "Add shape",
            "Browse…",
            "Cancel",
        ] {
            assert_dialog_button_visible(&window, label);
        }
        window.set_min_x("-".into());
        window.invoke_edited();
        assert!(!window.get_can_preview());
        assert!(window.get_token().is_empty());
        assert!(lock(&shared).primitive_preview().is_none());
        window.set_min_x("-128".into());
        window.invoke_edited();
        window.invoke_preview_shape();
        let old = window.get_token();
        let material_target = {
            let session = lock(&shared);
            (
                session.primitive_preview().map(|preview| preview.token),
                session.document_generation(),
            )
        };
        lock(&shared)
            .create_primitive(Actor::Agent, map_core::primitive::Spec::default(), true)
            .unwrap();
        assert!(
            primitive_ui::choose_material(
                &window,
                &mut lock(&shared),
                material_target,
                "brick/changed"
            )
            .is_err()
        );
        window.set_token(old);
        window.invoke_apply();
        assert!(window.get_message().contains("replaced"));
        assert_eq!(lock(&shared).document().vmf(), &before);
        assert_eq!(window.get_shape(), 0);
        window.invoke_cancel();
        assert!(lock(&shared).primitive_preview().is_none());
        assert_eq!(lock(&shared).document().vmf(), &before);
        window.show().unwrap();
        window.set_shape(3);
        window.set_segments(12);
        window.set_bands(6);
        window.invoke_edited();
        window.invoke_preview_shape();
        window.invoke_apply();
        assert!(lock(&shared).primitive_preview().is_none());
        assert_eq!(lock(&shared).document().vmf().world.solids.len(), 1);
        assert_eq!(
            lock(&shared).document().vmf().world.solids[0].sides.len(),
            72
        );
        assert_eq!(lock(&shared).selected_ids().len(), 1);
        lock(&shared).undo(Actor::User).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &before);
    }

    #[test]
    fn design_context_controls_share_drafts_queries_regions_and_cancel() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let mut session = Session::new(Err("no assets needed".into()));
        session
            .create_block(
                Actor::User,
                DVec3::ZERO,
                DVec3::splat(64.0),
                "brick/test",
                true,
            )
            .unwrap();
        let shared = Arc::new(Mutex::new(session));
        let window = ContextWindow::new().unwrap();
        context_ui::bind(&window, &shared);
        context_ui::refresh(&window, &lock(&shared));
        window.show().unwrap();
        let before = lock(&shared).document().vmf().clone();
        window.invoke_begin("map".into());
        window.set_summary("Courtyard lighting".into());
        window.set_body("A quiet service courtyard at dusk. Keep warm light near the entrance and cooler light along the escape route.".into());
        window.invoke_edit();
        assert!(window.get_can_apply());
        assert_eq!(lock(&shared).document().vmf(), &before);
        window.invoke_apply();
        assert_eq!(window.get_rows().row_count(), 1);
        assert_eq!(
            lock(&shared)
                .get_context_note(map_core::context::Scope::Map)
                .unwrap()["note"]["author"],
            "human"
        );
        window.invoke_begin("new_region".into());
        window.set_summary("Keep the entrance readable".into());
        window.set_body("Leave the doorway clear and give it a warm pool of light.\n\nAvoid props above waist height along the approach. The player should see the exit from the center of the courtyard.".into());
        window.set_min_x("-128".into());
        window.set_min_y("-96".into());
        window.set_min_z("0".into());
        window.set_max_x("128".into());
        window.set_max_y("96".into());
        window.set_max_z("160".into());
        window.invoke_edit();
        assert!(window.get_can_apply());
        window.invoke_apply();
        window.invoke_begin("region:1".into());
        save_ui_snapshot(window.window(), "design-context");
        for label in [
            "Map brief",
            "Note on selection",
            "New region",
            "Focus in map",
            "Cancel edit",
            "Save note",
            "Delete region",
            "Cancel / close",
        ] {
            assert_dialog_button_visible(&window, label);
        }
        window.invoke_focus_note();
        assert_eq!(lock(&shared).context_state()["focused_region"], 1);
        window.set_filter_mode(4);
        window.invoke_filter();
        assert_eq!(window.get_rows().row_count(), 2);
        window.set_filter_mode(5);
        window.set_query_min_x("10000".into());
        window.set_include_map(false);
        window.invoke_filter();
        assert_eq!(window.get_rows().row_count(), 0);
        window.set_query_min_x("0".into());
        window.invoke_filter();
        assert_eq!(window.get_rows().row_count(), 1);
        window.set_filter_mode(6);
        window.set_query("props".into());
        window.invoke_filter();
        assert_eq!(window.get_rows().row_count(), 1);
        window.set_query("".into());
        window.set_filter_mode(0);
        window.invoke_filter();
        let saved = lock(&shared).document().vmf().clone();
        window.set_min_x("unfinished".into());
        window.invoke_edit();
        assert!(!window.get_can_apply());
        window.invoke_cancel_edit();
        assert_eq!(lock(&shared).document().vmf(), &saved);
        window.invoke_begin("region:1".into());
        let old_token = window.get_token();
        lock(&shared)
            .begin_context_edit(Some(map_core::context::Scope::Map))
            .unwrap();
        window.set_token(old_token);
        window.invoke_apply();
        assert!(window.get_message().contains("replaced"));
        assert_eq!(lock(&shared).document().vmf(), &saved);
        window.invoke_cancel_edit();
        window.invoke_begin("region:1".into());
        window.invoke_delete_note();
        assert!(window.get_message().contains("Confirm"));
        window.set_confirm_delete(true);
        window.invoke_delete_note();
        assert_eq!(lock(&shared).context_state()["notes"], 1);
        lock(&shared).undo(Actor::Agent).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &saved);
        window.invoke_begin("map".into());
        window.set_body("Unapplied change".into());
        window.invoke_edit();
        window.invoke_cancel();
        assert!(lock(&shared).context_draft().is_none());
        assert_eq!(lock(&shared).document().vmf(), &saved);
    }

    #[test]
    fn map_checks_controls_share_selection_repair_cancel_and_stale_guards() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let mut session = Session::new(Err("no assets needed".into()));
        session
            .create_block(
                Actor::User,
                DVec3::ZERO,
                DVec3::splat(64.0),
                "brick/test",
                false,
            )
            .unwrap();
        let mut vmf = session.document().vmf().clone();
        vmf.world.solids[0].sides[0].u_axis.scale = 0.0;
        vmf.world.solids[0].sides[1].lightmap_scale = 0;
        let path = directory.path().join("courtyard.vmf");
        std::fs::write(
            &path,
            vmf.write(&source_formats::keyvalues::WriteOptions::default()),
        )
        .unwrap();
        session.open(Actor::User, &path, true).unwrap();
        let shared = Arc::new(Mutex::new(session));
        let window = ChecksWindow::new().unwrap();
        checks_ui::bind(&window, &shared);
        checks_ui::refresh(&window, &lock(&shared));
        assert!(!window.get_can_repair());
        window.show().unwrap();
        window.invoke_run_checks();
        assert!(window.get_rows().row_count() >= 6);
        window.invoke_inspect("0".into());
        assert_eq!(window.get_detail_title(), "Invalid texture projection");
        assert!(window.get_can_select() && window.get_can_repair());
        window.invoke_navigate(true);
        assert_eq!(lock(&shared).selected_faces().len(), 1);
        let before = lock(&shared).document().vmf().clone();
        window.invoke_review_repair();
        assert!(!window.get_preview_token().is_empty());
        assert_eq!(lock(&shared).document().vmf(), &before);
        save_ui_snapshot(window.window(), "map-checks");
        for label in [
            "Run checks",
            "Select issue",
            "Focus issue",
            "Review repair…",
            "Cancel repair",
            "Apply repair",
            "Cancel / close",
        ] {
            assert_dialog_button_visible(&window, label);
        }
        window.invoke_cancel_repair();
        assert!(lock(&shared).map_fix_draft().is_none());
        assert_eq!(lock(&shared).document().vmf(), &before);
        window.invoke_review_repair();
        window.invoke_apply_repair();
        assert_eq!(lock(&shared).document().undo_labels().count(), 1);
        assert_ne!(lock(&shared).document().vmf(), &before);
        lock(&shared).undo(Actor::Agent).unwrap();
        assert_eq!(lock(&shared).document().vmf(), &before);
        checks_ui::refresh(&window, &lock(&shared));
        assert!(window.get_stale());
        assert!(!window.get_can_repair());
        window.invoke_run_checks();
        window.invoke_inspect("0".into());
        window.invoke_review_repair();
        let old = window.get_preview_token();
        let report = lock(&shared).map_check_report().unwrap().token;
        lock(&shared).preview_map_fixes(report, &[1]).unwrap();
        window.set_preview_token(old);
        window.invoke_apply_repair();
        assert!(window.get_message().contains("replaced"));
        assert_eq!(lock(&shared).document().vmf(), &before);
        window.set_query("missing_team_spawn".into());
        window.invoke_filter();
        assert_eq!(window.get_rows().row_count(), 2);
        window.invoke_cancel();
        assert!(lock(&shared).map_fix_draft().is_none());
        assert_eq!(lock(&shared).document().vmf(), &before);
    }

    #[test]
    fn recovery_controls_guard_settings_and_restore_and_share_snapshot_operations() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let mut session = Session::new(Err("no assets needed".into()));
        session.configure_recovery(Ok(directory.path().join("recovery")));
        session
            .create_block(
                Actor::User,
                DVec3::ZERO,
                DVec3::splat(64.0),
                "brick/test",
                false,
            )
            .unwrap();
        session
            .save(Actor::User, Some(&directory.path().join("courtyard.vmf")))
            .unwrap();
        let shared = Arc::new(Mutex::new(session));
        let entry = recovery::capture(&shared, Actor::User, recovery::Kind::Manual).unwrap();
        lock(&shared)
            .create_block(
                Actor::Agent,
                DVec3::splat(128.0),
                DVec3::splat(192.0),
                "extra",
                false,
            )
            .unwrap();
        let window = RecoveryWindow::new().unwrap();
        recovery_ui::bind(&window, &shared);
        recovery_ui::refresh(&window, &lock(&shared));
        window.show().unwrap();
        window.invoke_select_snapshot(entry.id.clone().into());
        save_ui_snapshot(window.window(), "recovery");
        for label in [
            "Cancel / close",
            "Apply settings",
            "Save snapshot now",
            "Open recovered copy",
            "Delete snapshot",
        ] {
            assert_dialog_button_visible(&window, label);
        }
        window.set_interval_seconds(60);
        window.set_settings_dirty(true);
        window.invoke_apply_settings();
        assert_eq!(lock(&shared).recovery_settings().interval_seconds, 60);
        window.set_keep_count(3);
        window.set_settings_dirty(true);
        let newer = recovery::Settings {
            enabled: false,
            interval_seconds: 90,
            keep_per_map: 7,
        };
        lock(&shared)
            .set_recovery_settings(Actor::Agent, newer.clone(), None)
            .unwrap();
        window.invoke_apply_settings();
        assert!(window.get_message().contains("changed elsewhere"));
        assert_eq!(lock(&shared).recovery_settings(), &newer);
        window.invoke_reload_settings();
        assert_eq!(window.get_keep_count(), 7);
        let before = lock(&shared).document().vmf().clone();
        window.invoke_restore_snapshot();
        assert!(window.get_message().contains("unsaved changes"));
        assert_eq!(lock(&shared).document().vmf(), &before);
        window.set_discard_changes(true);
        lock(&shared)
            .create_block(
                Actor::Agent,
                DVec3::splat(256.0),
                DVec3::splat(320.0),
                "extra",
                false,
            )
            .unwrap();
        window.invoke_restore_snapshot();
        assert!(window.get_message().contains("Map changed"));
        assert!(!window.get_discard_changes());
        window.set_discard_changes(true);
        i_slint_backend_testing::ElementHandle::find_by_accessible_label(
            &window,
            "Open recovered copy",
        )
        .next()
        .unwrap()
        .invoke_accessible_default_action();
        assert_eq!(lock(&shared).document().objects().count(), 1);
        {
            let session = lock(&shared);
            assert!(session.document().is_dirty() && session.path().is_none());
        }
        window.show().unwrap();
        window.invoke_select_snapshot(entry.id.clone().into());
        window.invoke_delete_snapshot();
        assert_eq!(lock(&shared).recovery_entries().unwrap().entries.len(), 1);
        window.set_confirm_delete(true);
        window.invoke_delete_snapshot();
        assert!(lock(&shared).recovery_entries().unwrap().entries.is_empty());
        window.invoke_snapshot_now();
        for _ in 0..200 {
            if !lock(&shared).recovery_busy() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!lock(&shared).recovery_busy());
        assert_eq!(lock(&shared).recovery_entries().unwrap().entries.len(), 1);
        window.set_settings_dirty(true);
        window.set_interval_seconds(300);
        window
            .window()
            .dispatch_event(slint::platform::WindowEvent::KeyPressed {
                text: slint::platform::Key::Escape.into(),
            });
        assert!(!window.window().is_visible());
        assert_eq!(lock(&shared).recovery_settings().interval_seconds, 90);
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
        assert_eq!(
            material.get_favorites_status(),
            "Favorites last for this session"
        );
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
        let favorites_directory = tempfile::tempdir().unwrap();
        session.configure_recovery(Ok(favorites_directory.path().join("recovery")));
        session.favorite_material("brick/wall", true).unwrap();
        material.set_group("all".into());
        material.set_offset(0);
        material.set_material("dev/dev_measuregeneric01".into());
        refresh_material_catalog(&material, &mut session);
        refresh_material_window(&material, &mut session);
        assert_eq!(
            material.get_favorites_status(),
            "Favorites saved across sessions"
        );
        save_ui_snapshot(material.window(), "material-favorites-persistent");
        let favorite_path = favorites_directory
            .path()
            .join("recovery")
            .join(material_favorites::FILE_NAME);
        let mut permissions = std::fs::metadata(&favorite_path).unwrap().permissions();
        permissions.set_readonly(true);
        std::fs::set_permissions(&favorite_path, permissions).unwrap();
        assert!(session.favorite_material("wood/floor", true).is_err());
        refresh_material_catalog(&material, &mut session);
        refresh_material_window(&material, &mut session);
        assert!(material.get_favorites_status().contains("could not save"));
        save_ui_snapshot(material.window(), "material-favorites-error");
        let status = i_slint_backend_testing::ElementHandle::find_by_element_id(
            &material,
            "MaterialWindow::favorites-state",
        )
        .next()
        .unwrap();
        assert!(
            status.absolute_position().y + status.size().height
                <= material.window().size().height as f32
        );
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
        surface.set_operation_index(3);
        surface.set_sample_present(true);
        surface.set_sample_label("dev/dev_measuregeneric01 · brush 2, face 6".into());
        surface.set_sample_preview(surface.get_preview());
        save_ui_snapshot(surface.window(), "face-wrap");
        surface.set_operation_index(4);
        save_ui_snapshot(surface.window(), "face-chain");
        for label in ["Cancel", "Preview textures", "Apply textures"] {
            assert_dialog_button_visible(&surface, label);
        }
        surface.hide().unwrap();
    }

    #[test]
    fn texture_dialog_paints_sampled_material_and_preview_matches_apply() {
        i_slint_backend_testing::init_no_event_loop();
        let window = SurfaceWindow::new().unwrap();
        let mut session = Session::new(Err("no assets needed".into()));
        let ObjectId::Solid(id) = session
            .create_block(
                Actor::User,
                DVec3::ZERO,
                DVec3::splat(64.0),
                "brick/source",
                false,
            )
            .unwrap()
        else {
            panic!()
        };
        let faces: Vec<_> = session
            .document()
            .solid(id)
            .unwrap()
            .sides
            .iter()
            .map(|side| map_core::surface::FaceId {
                solid: id,
                side: side.id,
            })
            .collect();
        session
            .edit_faces(
                Actor::Agent,
                &[faces[0]],
                map_core::surface::SurfaceEdit {
                    material: Some("metal/sample".into()),
                    ..Default::default()
                },
                false,
                false,
            )
            .unwrap();
        session.sample_face_texture(faces[0]).unwrap();
        session
            .select_faces(&[faces[1]], SelectMode::Replace)
            .unwrap();
        let target = (
            vec![faces[1]],
            session.document_generation(),
            session.document().revision(),
        );
        refresh_surface_sample(&window, &mut session);
        assert!(window.get_sample_present());
        assert!(window.get_sample_label().contains("metal/sample"));
        window.set_operation_index(1);
        let original = session.document().vmf().clone();
        apply_surface_dialog(&window, &mut session, &target, true).unwrap();
        assert_eq!(session.document().vmf(), &original);
        let preview = session.face_preview_document().unwrap().vmf().clone();
        apply_surface_dialog(&window, &mut session, &target, false).unwrap();
        assert_eq!(session.document().vmf(), &preview);
        session.undo(Actor::User).unwrap();
        assert_eq!(session.document().vmf(), &original);
        assert!(apply_surface_dialog(&window, &mut session, &target, false).is_err());
        let targets = session
            .document()
            .solid(id)
            .unwrap()
            .sides
            .iter()
            .filter(|side| {
                let normal = map_core::Plane::from_points(side.plane.0).unwrap().normal;
                normal == DVec3::NEG_Y || normal == DVec3::NEG_Z
            })
            .map(|side| map_core::surface::FaceId {
                solid: id,
                side: side.id,
            })
            .collect::<Vec<_>>();
        session.select_faces(&targets, SelectMode::Replace).unwrap();
        let target = (
            session.selected_faces(),
            session.document_generation(),
            session.document().revision(),
        );
        window.set_operation_index(4);
        assert_eq!(
            apply_surface_dialog(&window, &mut session, &target, true).unwrap(),
            2
        );
        let preview = session.face_preview_document().unwrap().vmf().clone();
        session.cancel_face_preview();
        assert_eq!(session.document().vmf(), &original);
        assert_eq!(
            apply_surface_dialog(&window, &mut session, &target, false).unwrap(),
            2
        );
        assert_eq!(session.document().vmf(), &preview);
        session.undo(Actor::User).unwrap();
        assert_eq!(session.document().vmf(), &original);
        window.set_alignment_index(2);
        window.set_justify_u_index(1);
        window.set_justify_v_index(3);
        let patch = surface_patch(&window).unwrap();
        assert_eq!(patch.alignment, Some(map_core::surface::Alignment::Face));
        assert_eq!(
            patch.justify_u,
            Some((map_core::surface::Justify::Start, 0))
        );
        assert_eq!(patch.justify_v, Some((map_core::surface::Justify::End, 0)));
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
    fn replacement_transform_drafts_display_shared_operations_without_editing_source() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let app = AppWindow::new().unwrap();
        let mut state = Session::new(Err("no assets needed".into()));
        let id = state
            .create_block(
                Actor::Agent,
                DVec3::ZERO,
                DVec3::splat(64.0),
                "brick/test",
                false,
            )
            .unwrap();
        state
            .set_transform_tool(Actor::User, Default::default())
            .unwrap();
        let before = state.document().vmf().clone();
        let history = state.document().undo_labels().count();
        app.set_has_selection(true);
        app.set_can_apply_material(true);
        app.set_show_transform(true);
        app.show().unwrap();
        app.window().set_size(slint::PhysicalSize::new(1600, 960));
        let shear = map_core::transform::Shear {
            axis: 0,
            reference: 1,
            factor: 0.5,
        };
        for previous in [3, 4] {
            for (mode, spec, values) in [
                (
                    0,
                    edit::Transform {
                        translation: DVec3::new(12.125, -7.25, 3.5),
                        ..Default::default()
                    },
                    ["12.125", "-7.25", "3.5"],
                ),
                (
                    1,
                    edit::Transform {
                        rotation: DVec3::new(17.0, -23.0, 31.0),
                        ..Default::default()
                    },
                    ["17", "-23", "31"],
                ),
                (
                    2,
                    edit::Transform {
                        scale: DVec3::new(1.25, 0.75, 1.5),
                        ..Default::default()
                    },
                    ["1.25", "0.75", "1.5"],
                ),
                (
                    0,
                    edit::Transform {
                        shear: Some(map_core::transform::Shear {
                            factor: 0.0,
                            ..shear
                        }),
                        ..Default::default()
                    },
                    ["0", "0", "0"],
                ),
            ] {
                state
                    .preview_transform(
                        Actor::Agent,
                        &[id],
                        edit::Transform {
                            reflection: [previous == 3, false, false],
                            shear: (previous == 4).then_some(shear),
                            ..Default::default()
                        },
                        None,
                    )
                    .unwrap();
                refresh_interaction(&app, &state);
                save_ui_snapshot(app.window(), "transform-before-replacement");
                assert_eq!(app.get_transform_mode(), previous);
                state
                    .preview_transform(Actor::Agent, &[id], spec, None)
                    .unwrap();
                let expected = state.transform_preview().unwrap().document.vmf().clone();
                let summary = state.transform_preview().unwrap().presentation().summary;
                refresh_interaction(&app, &state);
                save_ui_snapshot(
                    app.window(),
                    &format!("transform-replacement-{previous}-{mode}"),
                );
                assert_eq!(app.get_transform_mode(), mode);
                assert_eq!(
                    [
                        app.get_transform_x(),
                        app.get_transform_y(),
                        app.get_transform_z()
                    ],
                    values.map(SharedString::from)
                );
                assert_eq!(app.get_transform_preview_summary(), summary);
                assert_eq!(state.transform_tool().mode, None);
                assert_eq!(state.document().vmf(), &before);
                assert_eq!(state.document().undo_labels().count(), history);
                state.cancel_transform_preview();
                refresh_interaction(&app, &state);
                assert_eq!(state.document().vmf(), &before);
                assert_eq!(app.get_transform_preview_summary(), "");
                state
                    .preview_transform(Actor::Agent, &[id], spec, None)
                    .unwrap();
                state.commit_transform_preview(Actor::User).unwrap();
                assert_eq!(state.document().vmf(), &expected);
                if expected != before {
                    assert_eq!(state.document().undo_labels().count(), history + 1);
                    state.undo(Actor::Agent).unwrap();
                }
                assert_eq!(state.document().vmf(), &before);
            }
        }
        state
            .preview_transform(
                Actor::Agent,
                &[id],
                edit::Transform {
                    translation: DVec3::new(8.0, 0.0, 0.0),
                    rotation: DVec3::new(0.0, 0.0, 15.0),
                    scale: DVec3::new(1.25, 1.0, 1.0),
                    reflection: [true, false, false],
                    shear: Some(shear),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        let presentation = state.transform_preview().unwrap().presentation();
        assert_eq!(
            presentation
                .operations
                .iter()
                .map(|op| op.name())
                .collect::<Vec<_>>(),
            ["move", "rotate", "scale", "mirror", "shear"]
        );
        refresh_interaction(&app, &state);
        save_ui_snapshot(app.window(), "transform-compound-summary");
        assert_eq!(app.get_transform_mode(), 4);
        assert_eq!(app.get_transform_preview_summary(), presentation.summary);
        for label in [
            "Move: 8, 0, 0 units",
            "Rotate: 0, 0, 15 degrees",
            "Scale: 1.25, 1, 1 factors",
            "Mirror: X",
            "Shear: X by Y × 0.5",
        ] {
            assert!(app.get_transform_preview_summary().contains(label));
        }
        assert_dialog_button_visible(&app, "Apply transform");
        assert_dialog_button_visible(&app, "Cancel");
        assert_eq!(state.document().vmf(), &before);
        assert_eq!(state.document().undo_labels().count(), history);
    }

    #[test]
    fn shear_controls_share_numeric_apply_and_agent_draft_values() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let app = AppWindow::new().unwrap();
        let mut state = Session::new(Err("no assets needed".into()));
        let id = state
            .create_block(
                Actor::Agent,
                DVec3::ZERO,
                DVec3::splat(64.0),
                "brick/test",
                false,
            )
            .unwrap();
        let before = state.document().vmf().clone();
        app.set_has_selection(true);
        app.set_can_apply_material(true);
        app.set_show_transform(true);
        app.set_transform_mode(4);
        app.set_transform_shear_axis(0);
        app.set_transform_shear_reference(1);
        app.set_transform_shear_factor("0.5".into());
        app.set_transform_pivot_mode(1);
        apply_selection_transform(&app, &mut state).unwrap();
        let bounds = map_core::query::bounds_of(state.document(), &[id]).unwrap();
        assert!(bounds.1.abs_diff_eq(DVec3::new(96.0, 64.0, 64.0), 1e-8));
        state.undo(Actor::User).unwrap();
        assert_eq!(state.document().vmf(), &before);
        state
            .preview_transform(
                Actor::Agent,
                &[id],
                edit::Transform {
                    shear: Some(map_core::transform::Shear {
                        axis: 2,
                        reference: 0,
                        factor: -0.75,
                    }),
                    texture_scale_lock: Some(false),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        refresh_interaction(&app, &state);
        app.show().unwrap();
        save_ui_snapshot(app.window(), "brush-shear");
        assert_eq!(app.get_transform_mode(), 4);
        assert_eq!(app.get_transform_shear_axis(), 2);
        assert_eq!(app.get_transform_shear_reference(), 0);
        assert_eq!(app.get_transform_shear_factor(), "-0.75");
        assert_dialog_button_visible(&app, "Keep textures attached while shearing");
        state.cancel_transform_preview();
        assert_eq!(state.document().vmf(), &before);
    }

    #[test]
    fn mirror_controls_share_brush_transform_and_agent_preview() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let app = AppWindow::new().unwrap();
        let mut state = Session::new(Err("no assets needed".into()));
        let id = state
            .create_block(
                Actor::Agent,
                DVec3::ZERO,
                DVec3::splat(64.0),
                "brick/test",
                false,
            )
            .unwrap();
        let before = state.document().vmf().clone();
        app.set_has_selection(true);
        app.set_can_apply_material(true);
        app.set_show_transform(true);
        app.set_transform_mode(3);
        app.set_transform_reflect_x(true);
        app.set_transform_reflect_y(false);
        app.set_transform_reflect_z(false);
        app.set_transform_pivot_mode(1);
        apply_selection_transform(&app, &mut state).unwrap();
        let bounds = map_core::query::bounds_of(state.document(), &[id]).unwrap();
        assert!(bounds.0.abs_diff_eq(DVec3::new(-64.0, 0.0, 0.0), 1e-8));
        state.undo(Actor::User).unwrap();
        assert_eq!(state.document().vmf(), &before);
        state
            .preview_transform(
                Actor::Agent,
                &[id],
                edit::Transform {
                    reflection: [false, true, true],
                    texture_scale_lock: Some(false),
                    ..Default::default()
                },
                Some(DVec3::new(12.125, 7.25, 3.5)),
            )
            .unwrap();
        refresh_interaction(&app, &state);
        app.show().unwrap();
        save_ui_snapshot(app.window(), "brush-reflection");
        assert_eq!(app.get_transform_mode(), 3);
        assert!(!app.get_transform_reflect_x());
        assert!(app.get_transform_reflect_y());
        assert!(app.get_transform_reflect_z());
        assert!(!app.get_transform_texture_scale_lock());
        for label in [
            "Mirror X",
            "Mirror Y",
            "Mirror Z",
            "Mirror textures with geometry",
        ] {
            assert_dialog_button_visible(&app, label);
        }
        state.cancel_transform_preview();
        assert_eq!(state.document().vmf(), &before);
        app.set_transform_reflect_y(false);
        app.set_transform_reflect_z(false);
        assert!(
            apply_selection_transform(&app, &mut state)
                .unwrap_err()
                .contains("at least one")
        );
        assert_eq!(state.document().vmf(), &before);
    }

    #[test]
    fn independent_texture_controls_share_numeric_and_agent_preview_modes() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let app = AppWindow::new().unwrap();
        let mut state = Session::new(Err("no assets needed".into()));
        let id = state
            .create_block(
                Actor::Agent,
                DVec3::ZERO,
                DVec3::splat(64.0),
                "brick/test",
                false,
            )
            .unwrap();
        let before = state.document().vmf().clone();
        app.set_has_selection(true);
        app.set_can_apply_material(true);
        app.set_show_transform(true);
        app.set_transform_mode(2);
        app.set_transform_x("2".into());
        app.set_transform_y("1".into());
        app.set_transform_z("1".into());
        app.set_transform_texture_lock(true);
        app.set_transform_texture_scale_lock(false);
        let tool = transform_tool_from_controls(&app).unwrap();
        assert!(tool.texture_lock);
        assert_eq!(tool.texture_scale_lock, Some(false));
        state.set_transform_tool(Actor::User, tool).unwrap();
        apply_selection_transform(&app, &mut state).unwrap();
        let ObjectId::Solid(solid) = id else {
            unreachable!()
        };
        for (old, new) in before.world.solids[0]
            .sides
            .iter()
            .zip(&state.document().solid(solid).unwrap().sides)
        {
            assert_eq!((old.u_axis, old.v_axis), (new.u_axis, new.v_axis));
        }
        state.undo(Actor::User).unwrap();
        assert_eq!(state.document().vmf(), &before);
        state
            .preview_transform(
                Actor::Agent,
                &[id],
                edit::Transform {
                    scale: DVec3::new(2.0, 1.0, 1.0),
                    texture_lock: false,
                    texture_scale_lock: Some(true),
                    ..Default::default()
                },
                None,
            )
            .unwrap();
        refresh_interaction(&app, &state);
        assert!(!app.get_transform_texture_lock());
        assert!(app.get_transform_texture_scale_lock());
        app.show().unwrap();
        save_ui_snapshot(app.window(), "independent-texture-scale-lock");
        assert_eq!(app.get_transform_x(), "2");
        assert_eq!(app.get_transform_y(), "1");
        assert_eq!(app.get_transform_z(), "1");
        for label in ["Follow move and rotation", "Stretch textures when scaling"] {
            assert_dialog_button_visible(&app, label);
        }
        state.cancel_transform_preview();
        assert_eq!(state.document().vmf(), &before);
    }

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
