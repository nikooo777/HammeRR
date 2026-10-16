use crate::identity::id_string;
use crate::*;
use glam::{DVec2, DVec3, Vec3};
use map_core::{
    ObjectId, edit,
    query::{self, Ray},
};
use render::Projection;
use session::{Pointer, SelectMode, TransformTool};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel, platform::Key};
use source_formats::fgd::ClassKind;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};
use viewport::{Viewport, key_text};

mod clipping;
use clipping::*;
mod entities;
use entities::*;
mod files;
use files::*;
mod input;
use input::*;
mod manipulation;
use manipulation::*;
mod materials;
use materials::*;
mod navigation;
use navigation::*;
mod object_picking;

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
    object_cycle: object_picking::Gesture,
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
    merge_window: Option<MergeWindow>,
    recovery_window: Option<RecoveryWindow>,
    recent_maps_window: Option<RecentMapsWindow>,
    checks_window: Option<ChecksWindow>,
    context_window: Option<ContextWindow>,
    primitive_window: Option<PrimitiveWindow>,
    polygon: Option<polygon_ui::Controller>,
    linked_path: Option<linked_path_ui::Controller>,
    seen_primitive: u64,
    seen_context: (u64, u64),
    seen_checks: (u64, u64, u64),
    seen_recovery: (u64, u64),
    seen_recent_maps: (u64, u64),
    seen_management: (u64, u64, u64),
    rename_window: Option<RenameWindow>,
    seen_rename: (u64, u64, u64),
    color_window: Option<ColorWindow>,
    model: Option<model_ui::Controller>,
    model_browser: Option<model_browser_ui::Controller>,
    sounds: Option<sounds_ui::Controller>,
    particles: Option<particles_ui::Controller>,
    placement: Option<placement_ui::Controller>,
    entity_placement: Option<entity_placement_ui::Controller>,
    repeat: Option<repeat_ui::Controller>,
    clipboard: Option<clipboard_ui::Controller>,
    object_groups: Option<groups_ui::Controller>,
    layers: Option<layers_ui::Controller>,
    compile_region: Option<compile_region_ui::Controller>,
    fog: Option<fog_ui::Controller>,
    working_region: Option<working_region_ui::Controller>,
    overlay: Option<overlay_ui::Controller>,
    cubemap: Option<cubemap_ui::Controller>,
    prefabs: Option<prefabs_ui::Controller>,
    color_target: Option<(u32, String, u64)>,
    material_window: Option<MaterialWindow>,
    material_target: Option<(u32, String, u64)>,
    surface_window: Option<SurfaceWindow>,
    surface_target: Option<(Vec<map_core::surface::FaceId>, u64, u64)>,
    lighting: Option<lighting_ui::Controller>,
    terrain: Option<terrain_ui::Controller>,
    terrain_triangles: Option<terrain_triangles_ui::Controller>,
    pick_release: bool,
    material_replace: Option<material_replace_ui::Controller>,
    surface_group: Option<surface_group_ui::Controller>,
    volume: Option<volume_ui::Controller>,
    compile_status: Option<compile_ui::Controller>,
    saved_views: Option<saved_views_ui::Controller>,
    history: Option<history_ui::Controller>,
    measurements: Option<measurements_ui::Controller>,
    grid_defaults: Option<grid_defaults_ui::Controller>,
    material_for_group: bool,
    overlay_material_target: Option<overlay_ui::MaterialTarget>,
    material_for_replace: Option<bool>,
    material_for_faces: bool,
    material_for_clip: bool,
    material_for_primitive: bool,
    material_for_polygon: bool,
    material_for_extrusion: bool,
    material_for_partition: bool,
    partition_material_target: Option<components_ui::PartitionMaterialTarget>,
    extrusion_material_target: Option<(Option<u64>, u64)>,
    primitive_material_target: Option<(Option<u64>, u64)>,
    seen_definitions: u64,
    seen_materials: u64,
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

fn refresh_view_controls(app: &AppWindow, session: &Session) {
    app.set_selection_outlines(session.view().selection_outlines);
    app.set_rope_preview(session.view().rope_preview);
    app.set_rope_preview_status(
        session.rope_preview_state()["summary"]
            .as_str()
            .unwrap()
            .into(),
    );
}

fn bind_view_controls(app: &AppWindow, shared: SharedSession) {
    app.on_options_changed({
        let app = app.as_weak();
        move || {
            let app = app.unwrap();
            let mut session = lock(&shared);
            let mut view = session.view();
            view.tool_textures = app.get_show_tools();
            view.sky_preview = app.get_sky_preview();
            view.rope_preview = app.get_rope_preview();
            view.selection_outlines = app.get_selection_outlines();
            view.leak_path = app.get_show_leak();
            session.set_view(view);
            refresh_view_controls(&app, &session);
        }
    });
}

fn refresh_panel(app: &AppWindow, session: &mut Session) {
    refresh_clip(app, session);
    refresh_view_controls(app, session);
    app.set_fog_preview_status(
        session.fog_preview_state()["summary"]
            .as_str()
            .unwrap()
            .into(),
    );
    let doc = session.document();
    app.set_map_title(session.title().into());
    app.set_can_undo(doc.undo_labels().next().is_some());
    app.set_can_redo(doc.redo_labels().next().is_some());
    app.set_compiling(session.compiling());
    app.set_compile_region_label(session.compile_region_label().into());
    app.set_working_region_active(session.working_region_enabled());
    app.set_launching(session.launching());
    app.set_game_running(session.game_running());
    app.set_has_selection(!session.selection().is_empty());
    app.set_face_mode(session.face_mode());
    app.set_has_faces(!session.selected_faces().is_empty());
    refresh_terrain_controls(app, session);
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
    app.set_entity_helper_status(
        single_selected_entity(session)
            .and_then(|id| session.entity_helper_report(id).ok())
            .map_or_else(String::new, |report| report.status)
            .into(),
    );
    app.set_can_cubemap_faces(session.selected_cubemap_helper().is_some());
    app.set_can_inspect_model(
        single_selected_entity(session)
            .and_then(|id| doc.entity(id))
            .is_some_and(|entity| {
                entity.solids.is_empty()
                    && entity
                        .properties
                        .get("model")
                        .is_some_and(|path| !path.trim().is_empty())
            }),
    );
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

pub(super) fn run(
    session: SharedSession,
    ctx: GpuContext,
    mcp_status: String,
    open_message: Option<Result<String, String>>,
) -> Result<(), Box<dyn std::error::Error>> {
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
        object_cycle: object_picking::Gesture::default(),
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
        merge_window: None,
        recovery_window: None,
        recent_maps_window: None,
        checks_window: None,
        context_window: None,
        primitive_window: None,
        polygon: None,
        linked_path: None,
        seen_primitive: 0,
        seen_context: (0, 0),
        seen_checks: (0, 0, 0),
        seen_recovery: (0, 0),
        seen_recent_maps: (0, 0),
        seen_management: (0, 0, 0),
        rename_window: None,
        seen_rename: (0, 0, 0),
        color_window: None,
        model: None,
        model_browser: None,
        sounds: None,
        particles: None,
        placement: None,
        entity_placement: None,
        repeat: None,
        clipboard: None,
        object_groups: None,
        layers: None,
        compile_region: None,
        fog: None,
        working_region: None,
        overlay: None,
        cubemap: None,
        prefabs: None,
        color_target: None,
        material_window: None,
        material_target: None,
        surface_window: None,
        surface_target: None,
        lighting: None,
        terrain: None,
        terrain_triangles: None,
        pick_release: false,
        material_replace: None,
        surface_group: None,
        volume: None,
        compile_status: None,
        saved_views: None,
        history: None,
        measurements: None,
        grid_defaults: None,
        material_for_group: false,
        overlay_material_target: None,
        material_for_replace: None,
        material_for_faces: false,
        material_for_clip: false,
        material_for_primitive: false,
        material_for_polygon: false,
        material_for_extrusion: false,
        material_for_partition: false,
        partition_material_target: None,
        extrusion_material_target: None,
        primitive_material_target: None,
        seen_definitions: 0,
        seen_materials: 0,
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
    app.on_recent_maps({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            let app = app.unwrap();
            if ui.borrow().recent_maps_window.is_none() {
                match RecentMapsWindow::new() {
                    Ok(window) => {
                        recent_maps_ui::bind(&window, &session_of(&ui));
                        ui.borrow_mut().recent_maps_window = Some(window);
                    }
                    Err(error) => {
                        app.set_message(error.to_string().into());
                        return;
                    }
                }
            }
            let window = ui
                .borrow()
                .recent_maps_window
                .as_ref()
                .unwrap()
                .clone_strong();
            window.set_discard_changes(false);
            window.set_confirm_clear(false);
            window.set_message("".into());
            recent_maps_ui::refresh(&window, &lock(&session_of(&ui)));
            if let Err(error) = window.show() {
                app.set_message(error.to_string().into());
            }
        }
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
    app.on_history({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            let mut ui = ui.borrow_mut();
            if ui.history.is_none() {
                match history_ui::Controller::new(&ui.session) {
                    Ok(controller) => ui.history = Some(controller),
                    Err(error) => {
                        app.unwrap().set_message(error.to_string().into());
                        return;
                    }
                }
            }
            if let Err(error) = ui.history.as_ref().unwrap().show(&lock(&ui.session)) {
                app.unwrap().set_message(error.to_string().into());
            }
        }
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
    app.on_measurements({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            let mut ui = ui.borrow_mut();
            if ui.measurements.is_none() {
                match measurements_ui::Controller::new(&ui.session) {
                    Ok(controller) => ui.measurements = Some(controller),
                    Err(error) => {
                        app.unwrap().set_message(error.to_string().into());
                        return;
                    }
                }
            }
            if let Err(error) = ui.measurements.as_ref().unwrap().show(&lock(&ui.session)) {
                app.unwrap().set_message(error.to_string().into());
            }
        }
    });
    app.on_grid_defaults({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            let mut ui = ui.borrow_mut();
            if ui.grid_defaults.is_none() {
                match grid_defaults_ui::Controller::new(&ui.session) {
                    Ok(controller) => ui.grid_defaults = Some(controller),
                    Err(error) => {
                        app.unwrap().set_message(error.to_string().into());
                        return;
                    }
                }
            }
            if let Err(error) = ui.grid_defaults.as_ref().unwrap().show(&lock(&ui.session)) {
                app.unwrap().set_message(error.to_string().into());
            }
        }
    });
    app.on_fog_preview({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            let mut ui = ui.borrow_mut();
            if ui.fog.is_none() {
                match fog_ui::Controller::new(&ui.session) {
                    Ok(controller) => ui.fog = Some(controller),
                    Err(error) => {
                        app.unwrap().set_message(error.to_string().into());
                        return;
                    }
                }
            }
            if let Err(error) = ui.fog.as_ref().unwrap().show(&lock(&ui.session)) {
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
    app.on_merge_brushes({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            if ui.borrow().merge_window.is_none() {
                match MergeWindow::new() {
                    Ok(window) => {
                        merge_ui::bind(&window, &session_of(&ui));
                        ui.borrow_mut().merge_window = Some(window);
                    }
                    Err(error) => {
                        app.unwrap().set_message(error.to_string().into());
                        return;
                    }
                }
            }
            let window = ui.borrow().merge_window.as_ref().unwrap().clone_strong();
            merge_ui::refresh(&window, &lock(&session_of(&ui)));
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
            components_ui::refresh_material_previews(&window, &lock(&session_of(&ui)));
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
    app.on_draw_polygon({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            let app = app.unwrap();
            if ui.borrow().polygon.is_none() {
                match polygon_ui::Controller::new(&session_of(&ui)) {
                    Ok(controller) => {
                        controller.window.on_browse_material({
                            let (app, ui) = (app.as_weak(), ui.clone());
                            move || show_material_window(&app.unwrap(), &ui, "@polygon")
                        });
                        ui.borrow_mut().polygon = Some(controller);
                    }
                    Err(error) => {
                        app.set_message(error.to_string().into());
                        return;
                    }
                }
            }
            let shared = session_of(&ui);
            let ui = ui.borrow();
            let controller = ui.polygon.as_ref().unwrap();
            controller
                .window
                .set_aspect(app.get_viewport_width() / app.get_viewport_height().max(1.0));
            match controller.initialize(&mut lock(&shared), &app.get_material()) {
                Ok(()) => {
                    let _ = controller.window.show();
                }
                Err(error) => app.set_message(error.into()),
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
    app.on_create_linked_path({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            let shared = session_of(&ui);
            if ui.borrow().linked_path.is_none() {
                match linked_path_ui::Controller::new(&shared) {
                    Ok(controller) => ui.borrow_mut().linked_path = Some(controller),
                    Err(error) => {
                        app.unwrap().set_message(error.to_string().into());
                        return;
                    }
                }
            }
            if let Err(error) = ui
                .borrow()
                .linked_path
                .as_ref()
                .unwrap()
                .show(&lock(&shared))
            {
                app.unwrap().set_message(error.to_string().into());
            }
        }
    });
    app.on_cubemap_faces({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            let shared = session_of(&ui);
            if ui.borrow().cubemap.is_none() {
                match cubemap_ui::Controller::new(&shared) {
                    Ok(controller) => ui.borrow_mut().cubemap = Some(controller),
                    Err(error) => {
                        app.unwrap().set_message(error.to_string().into());
                        return;
                    }
                }
            }
            if let Err(error) = ui.borrow().cubemap.as_ref().unwrap().show(&lock(&shared)) {
                app.unwrap().set_message(error.to_string().into());
            }
        }
    });
    app.on_create_overlay({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            let shared = session_of(&ui);
            if ui.borrow().overlay.is_none() {
                match overlay_ui::Controller::new(&shared) {
                    Ok(controller) => {
                        controller.window.on_browse_material({
                            let (app, ui) = (app.clone(), ui.clone());
                            move || show_material_window(&app.unwrap(), &ui, "@overlay")
                        });
                        ui.borrow_mut().overlay = Some(controller);
                    }
                    Err(error) => {
                        app.unwrap().set_message(error.to_string().into());
                        return;
                    }
                }
            }
            let ui = ui.borrow();
            if let Err(error) = ui
                .overlay
                .as_ref()
                .unwrap()
                .show(&mut lock(&shared), &app.unwrap().get_material())
            {
                app.unwrap().set_message(error.to_string().into());
            }
        }
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
    bind_terrain_controls(&app, session_of(&ui));
    app.on_cancel_terrain_center_pick({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            if let Some(controller) = &ui.borrow().terrain {
                controller.cancel_center_pick();
            }
            app.unwrap().set_terrain_center_picking(false);
        }
    });
    app.on_cancel_terrain_handles({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            if let Some(controller) = &ui.borrow().terrain {
                controller.cancel_handles(&mut lock(&session_of(&ui)), true);
            }
            app.unwrap().set_terrain_handling(false);
        }
    });
    app.on_cancel_terrain_stroke({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            if let Some(controller) = &ui.borrow().terrain {
                controller.cancel_stroke(&mut lock(&session_of(&ui)));
            }
            app.unwrap().set_terrain_stroking(false);
        }
    });
    app.on_change_terrain_resolution({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            let shared = session_of(&ui);
            if ui.borrow().terrain.is_none() {
                match terrain_ui::Controller::new(&shared) {
                    Ok(controller) => ui.borrow_mut().terrain = Some(controller),
                    Err(error) => {
                        app.unwrap().set_message(error.to_string().into());
                        return;
                    }
                }
            }
            let ui = ui.borrow();
            let controller = ui.terrain.as_ref().unwrap();
            match controller.initialize(&mut lock(&shared)) {
                Ok(()) => {
                    let _ = controller.window.show();
                }
                Err(error) => app.unwrap().set_message(error.into()),
            }
        }
    });
    app.on_cancel_terrain_triangle_pick({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            if let Some(controller) = &ui.borrow().terrain_triangles {
                controller.cancel_pick();
            }
            app.unwrap().set_terrain_triangle_picking(false);
        }
    });
    app.on_edit_terrain_triangles({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            let shared = session_of(&ui);
            if ui.borrow().terrain_triangles.is_none() {
                match terrain_triangles_ui::Controller::new(&shared) {
                    Ok(controller) => ui.borrow_mut().terrain_triangles = Some(controller),
                    Err(error) => {
                        app.unwrap().set_message(error.to_string().into());
                        return;
                    }
                }
            }
            let ui = ui.borrow();
            let controller = ui.terrain_triangles.as_ref().unwrap();
            let app = app.unwrap();
            controller
                .window
                .set_aspect(app.get_viewport_width() / app.get_viewport_height().max(1.0));
            match controller.initialize(&mut lock(&shared)) {
                Ok(()) => {
                    let _ = controller.window.show();
                }
                Err(error) => app.set_message(error.into()),
            }
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
    app.on_cancel_surface_pick({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            cancel_surface_pick(&app.unwrap(), &ui);
        }
    });
    app.on_copy_objects({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || copy_clipboard_selection(&app.unwrap(), &ui, false)
    });
    app.on_cut_objects({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || copy_clipboard_selection(&app.unwrap(), &ui, true)
    });
    app.on_paste_objects({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            let shared = session_of(&ui);
            if ui.borrow().clipboard.is_none() {
                match clipboard_ui::Controller::new(&shared) {
                    Ok(controller) => ui.borrow_mut().clipboard = Some(controller),
                    Err(error) => {
                        app.unwrap().set_message(error.to_string().into());
                        return;
                    }
                }
            }
            let ui = ui.borrow();
            let controller = ui.clipboard.as_ref().unwrap();
            controller.initialize(&mut lock(&shared), true);
            let _ = controller.window.show();
        }
    });
    app.on_repeat_array({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            let shared = session_of(&ui);
            if ui.borrow().repeat.is_none() {
                match repeat_ui::Controller::new(&shared) {
                    Ok(controller) => ui.borrow_mut().repeat = Some(controller),
                    Err(error) => {
                        app.unwrap().set_message(error.to_string().into());
                        return;
                    }
                }
            }
            let ui = ui.borrow();
            let controller = ui.repeat.as_ref().unwrap();
            match controller.initialize(&lock(&shared)) {
                Ok(()) => {
                    let _ = controller.window.show();
                }
                Err(error) => app.unwrap().set_message(error.into()),
            }
        }
    });
    app.on_object_groups({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            let mut ui = ui.borrow_mut();
            if ui.object_groups.is_none() {
                match groups_ui::Controller::new(&ui.session) {
                    Ok(controller) => ui.object_groups = Some(controller),
                    Err(error) => {
                        app.unwrap().set_message(error.to_string().into());
                        return;
                    }
                }
            }
            if let Err(error) = ui.object_groups.as_ref().unwrap().show(&lock(&ui.session)) {
                app.unwrap().set_message(error.to_string().into());
            }
        }
    });
    app.on_working_region({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            let shared = session_of(&ui);
            if ui.borrow().working_region.is_none() {
                match working_region_ui::Controller::new(&shared) {
                    Ok(controller) => ui.borrow_mut().working_region = Some(controller),
                    Err(error) => {
                        app.unwrap().set_message(error.to_string().into());
                        return;
                    }
                }
            }
            if let Err(error) = ui
                .borrow()
                .working_region
                .as_ref()
                .unwrap()
                .show(&mut lock(&shared))
            {
                app.unwrap().set_message(error.to_string().into());
            }
        }
    });
    working_region_ui::bind_clear(&app, session_of(&ui));
    app.on_compile_region({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            let shared = session_of(&ui);
            if ui.borrow().compile_region.is_none() {
                let viewport = app.clone();
                match compile_region_ui::Controller::new(&shared, move || {
                    let app = viewport.unwrap();
                    (app.get_viewport_width() / app.get_viewport_height().max(1.0)).max(0.01)
                }) {
                    Ok(controller) => ui.borrow_mut().compile_region = Some(controller),
                    Err(error) => {
                        app.unwrap().set_message(error.to_string().into());
                        return;
                    }
                }
            }
            let ui = ui.borrow();
            if let Err(error) = ui.compile_region.as_ref().unwrap().show(&lock(&shared)) {
                app.unwrap().set_message(error.to_string().into());
            }
        }
    });
    app.on_layers({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            let mut ui = ui.borrow_mut();
            if ui.layers.is_none() {
                match layers_ui::Controller::new(&ui.session) {
                    Ok(controller) => ui.layers = Some(controller),
                    Err(error) => {
                        app.unwrap().set_message(error.to_string().into());
                        return;
                    }
                }
            }
            if let Err(error) = ui.layers.as_ref().unwrap().show(&lock(&ui.session)) {
                app.unwrap().set_message(error.to_string().into());
            }
        }
    });
    app.on_prefabs({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            let mut ui = ui.borrow_mut();
            if ui.prefabs.is_none() {
                match prefabs_ui::Controller::new(&ui.session) {
                    Ok(controller) => ui.prefabs = Some(controller),
                    Err(error) => {
                        app.unwrap().set_message(error.to_string().into());
                        return;
                    }
                }
            }
            if let Err(error) = ui.prefabs.as_ref().unwrap().show(&lock(&ui.session)) {
                app.unwrap().set_message(error.to_string().into());
            }
        }
    });
    app.on_place_on_surface({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            let shared = session_of(&ui);
            if ui.borrow().placement.is_none() {
                match placement_ui::Controller::new(&shared) {
                    Ok(controller) => ui.borrow_mut().placement = Some(controller),
                    Err(error) => {
                        app.unwrap().set_message(error.to_string().into());
                        return;
                    }
                }
            }
            let ui = ui.borrow();
            let controller = ui.placement.as_ref().unwrap();
            controller.window.set_aspect(
                app.unwrap().get_viewport_width() / app.unwrap().get_viewport_height().max(1.0),
            );
            let result = controller.initialize(&mut lock(&shared));
            match result {
                Ok(()) => {
                    let _ = controller.window.show();
                }
                Err(error) => app.unwrap().set_message(error.into()),
            }
        }
    });
    app.on_browse_particle({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            let shared = session_of(&ui);
            if ui.borrow().particles.is_none() {
                match particles_ui::Controller::new(&shared) {
                    Ok(controller) => ui.borrow_mut().particles = Some(controller),
                    Err(error) => {
                        app.unwrap().set_message(error.to_string().into());
                        return;
                    }
                }
            }
            let ui = ui.borrow();
            let controller = ui.particles.as_ref().unwrap();
            let result = {
                let mut session = lock(&shared);
                single_selected_entity(&session)
                    .ok_or("Select a particle system entity".to_owned())
                    .and_then(|id| controller.initialize(&mut session, id))
            };
            match result {
                Ok(()) => {
                    let _ = controller.window.show();
                }
                Err(error) => app.unwrap().set_message(error.into()),
            }
        }
    });
    app.on_browse_sound({
        let (app, ui) = (app.as_weak(), ui.clone());
        move |key| {
            let shared = session_of(&ui);
            if ui.borrow().sounds.is_none() {
                match sounds_ui::Controller::new(&shared) {
                    Ok(controller) => ui.borrow_mut().sounds = Some(controller),
                    Err(error) => {
                        app.unwrap().set_message(error.to_string().into());
                        return;
                    }
                }
            }
            let ui = ui.borrow();
            let controller = ui.sounds.as_ref().unwrap();
            let result = {
                let mut session = lock(&shared);
                single_selected_entity(&session)
                    .ok_or("Select an entity with a sound field".to_owned())
                    .and_then(|id| controller.initialize(&mut session, id, &key))
            };
            match result {
                Ok(()) => {
                    let _ = controller.window.show();
                }
                Err(error) => app.unwrap().set_message(error.into()),
            }
        }
    });
    app.on_browse_model({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            let shared = session_of(&ui);
            if ui.borrow().model_browser.is_none() {
                match model_browser_ui::Controller::new(&shared) {
                    Ok(controller) => ui.borrow_mut().model_browser = Some(controller),
                    Err(error) => {
                        app.unwrap().set_message(error.to_string().into());
                        return;
                    }
                }
            }
            let ui = ui.borrow();
            let controller = ui.model_browser.as_ref().unwrap();
            let result = {
                let mut session = lock(&shared);
                single_selected_entity(&session)
                    .ok_or("Select a model entity".to_owned())
                    .and_then(|id| controller.initialize(&mut session, id))
            };
            match result {
                Ok(()) => {
                    if !controller.window.get_path().is_empty()
                        && !controller.window.get_can_apply()
                    {
                        controller.window.invoke_preview_model();
                    }
                    let _ = controller.window.show();
                }
                Err(error) => app.unwrap().set_message(error.into()),
            }
        }
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
    app.on_inspect_model({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            let shared = session_of(&ui);
            if ui.borrow().model.is_none() {
                match model_ui::Controller::new(&shared) {
                    Ok(controller) => ui.borrow_mut().model = Some(controller),
                    Err(error) => {
                        app.unwrap().set_message(error.to_string().into());
                        return;
                    }
                }
            }
            let ui = ui.borrow();
            let mut session = lock(&shared);
            if let Some(id) = single_selected_entity(&session) {
                let controller = ui.model.as_ref().unwrap();
                controller.window.set_aspect(
                    app.unwrap().get_viewport_width() / app.unwrap().get_viewport_height().max(1.0),
                );
                controller.initialize(&mut session, id);
                let _ = controller.window.show();
            }
        }
    });
    bind_view_controls(&app, session_of(&ui));
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
        move |button, down, x, y, control, alt| {
            let app = app.unwrap();
            let viewport_size = (app.get_viewport_width(), app.get_viewport_height());
            let mut ui = ui.borrow_mut();
            if button == 0 && !down && ui.object_cycle.active() {
                let shared = ui.session.clone();
                let camera = ui.viewport.camera;
                let pointer = Pointer {
                    position: DVec2::new(f64::from(x), f64::from(y)),
                    size: (
                        viewport_size.0.round().max(1.0) as u32,
                        viewport_size.1.round().max(1.0) as u32,
                    ),
                };
                let mut session = lock(&shared);
                match ui.object_cycle.release(&mut session, camera, pointer) {
                    Ok(Some(message)) => app.set_message(message.into()),
                    Ok(None) => {}
                    Err(error) => app.set_message(error.into()),
                }
                refresh_interaction(&app, &session);
                return;
            }
            if button == 0 && down {
                ui.object_cycle = object_picking::Gesture::default();
            } else if button != 0 && down {
                ui.object_cycle.cancel();
            }
            if button == 0 && !down && ui.pick_release {
                ui.pick_release = false;
                return;
            }
            let entity_picking =
                lock(&ui.session).get_entity_placement()["draft"]["picking"] == true;
            if button == 0 && entity_picking {
                if down {
                    ui.pick_release = true;
                    if ui.entity_placement.is_none() {
                        match entity_placement_ui::Controller::new(&ui.session) {
                            Ok(controller) => ui.entity_placement = Some(controller),
                            Err(error) => {
                                app.set_message(error.to_string().into());
                                return;
                            }
                        }
                    }
                    let controller = ui.entity_placement.as_ref().unwrap();
                    let size = (viewport_size.0.max(1.0), viewport_size.1.max(1.0));
                    let (origin, direction) = ui.viewport.camera.ray(
                        2.0 * x / size.0 - 1.0,
                        1.0 - 2.0 * y / size.1,
                        size.0 / size.1,
                    );
                    let result = {
                        let mut session = lock(&ui.session);
                        controller.initialize(&session, "light");
                        controller.pick_at_ray(
                            &mut session,
                            Ray {
                                origin: origin.as_dvec3(),
                                direction: direction.as_dvec3(),
                            },
                        )
                    };
                    match result {
                        Ok(()) => {
                            let _ = controller.window.show();
                            app.set_message(
                                "Review the entity bounds, then Apply entity or Cancel.".into(),
                            );
                        }
                        Err(error) => {
                            if !controller.window.get_owned() {
                                let _ = controller.window.show();
                            }
                            app.set_message(error.into());
                        }
                    }
                }
                return;
            }
            let placement_picking = lock(&ui.session).surface_placement()["picking"] == true;
            if button == 0 && placement_picking {
                if down {
                    ui.pick_release = true;
                    if ui.placement.is_none() {
                        match placement_ui::Controller::new(&ui.session) {
                            Ok(controller) => ui.placement = Some(controller),
                            Err(error) => {
                                app.set_message(error.to_string().into());
                                return;
                            }
                        }
                    }
                    let controller = ui.placement.as_ref().unwrap();
                    let size = (viewport_size.0.max(1.0), viewport_size.1.max(1.0));
                    controller.window.set_aspect(size.0 / size.1);
                    let (origin, direction) = ui.viewport.camera.ray(
                        2.0 * x / size.0 - 1.0,
                        1.0 - 2.0 * y / size.1,
                        size.0 / size.1,
                    );
                    let result = {
                        let mut session = lock(&ui.session);
                        controller.initialize(&mut session).and_then(|_| {
                            controller.pick_at_ray(
                                &mut session,
                                Ray {
                                    origin: origin.as_dvec3(),
                                    direction: direction.as_dvec3(),
                                },
                            )
                        })
                    };
                    match result {
                        Ok(()) => {
                            let _ = controller.window.show();
                            app.set_message(
                                "Review the surface placement, then Apply or Cancel.".into(),
                            );
                        }
                        Err(error) => app.set_message(error.into()),
                    }
                }
                return;
            }
            if button == 0
                && let Some(controller) = &ui.terrain
                && controller.is_handling()
            {
                let pointer = Pointer {
                    position: glam::DVec2::new(f64::from(x), f64::from(y)),
                    size: (
                        viewport_size.0.round().max(1.0) as u32,
                        viewport_size.1.round().max(1.0) as u32,
                    ),
                };
                let mut session = lock(&ui.session);
                session.publish_camera(ui.viewport.camera);
                let result = if down {
                    controller.begin_handle(&mut session, pointer)
                } else if controller.handle_dragging() {
                    controller
                        .update_handle(&mut session, pointer)
                        .and_then(|_| controller.finish_handle(&mut session))
                } else {
                    Ok(())
                };
                if !down {
                    controller.release_handle();
                }
                app.set_terrain_handling(controller.is_handling());
                app.set_message(
                    result
                        .err()
                        .unwrap_or_else(|| {
                            "Drag a terrain handle, then review with Apply or Cancel.".into()
                        })
                        .into(),
                );
                return;
            }
            if button == 0
                && ui
                    .terrain
                    .as_ref()
                    .is_some_and(|controller| controller.is_stroking())
            {
                let controller = ui.terrain.as_ref().unwrap();
                let mut session = lock(&ui.session);
                let result = if down {
                    let size = (viewport_size.0.max(1.0), viewport_size.1.max(1.0));
                    let (origin, direction) = ui.viewport.camera.ray(
                        2.0 * x / size.0 - 1.0,
                        1.0 - 2.0 * y / size.1,
                        size.0 / size.1,
                    );
                    controller
                        .update_stroke(
                            &mut session,
                            &Ray {
                                origin: origin.as_dvec3(),
                                direction: direction.as_dvec3(),
                            },
                        )
                        .map(|_| "Terrain stroke preview. Release to review.".to_owned())
                } else if controller.stroke_dragging() {
                    controller
                        .finish_stroke(&mut session)
                        .map(|_| "Terrain stroke ready for Apply or Cancel.".to_owned())
                } else {
                    Ok("Drag on visible selected terrain, or cancel the stroke.".to_owned())
                };
                app.set_terrain_stroking(controller.is_stroking());
                app.set_message(result.unwrap_or_else(|error| error).into());
                return;
            }
            if button == 0
                && ui
                    .terrain_triangles
                    .as_ref()
                    .is_some_and(|controller| controller.is_picking())
            {
                if down {
                    ui.pick_release = true;
                    let controller = ui.terrain_triangles.as_ref().unwrap();
                    let size = (viewport_size.0.max(1.0), viewport_size.1.max(1.0));
                    let (origin, direction) = ui.viewport.camera.ray(
                        2.0 * x / size.0 - 1.0,
                        1.0 - 2.0 * y / size.1,
                        size.0 / size.1,
                    );
                    let result = controller.select_at_ray(
                        &mut lock(&ui.session),
                        &Ray {
                            origin: origin.as_dvec3(),
                            direction: direction.as_dvec3(),
                        },
                    );
                    app.set_terrain_triangle_picking(controller.is_picking());
                    app.set_message(match result {
                        Ok(id) => {
                            let _ = controller.window.show();
                            format!("Selected terrain component {id}.").into()
                        }
                        Err(error) => error.into(),
                    });
                }
                return;
            }
            if button == 0
                && ui
                    .terrain
                    .as_ref()
                    .is_some_and(|controller| controller.is_picking())
            {
                if down {
                    ui.pick_release = true;
                    let controller = ui.terrain.as_ref().unwrap();
                    let size = (viewport_size.0.max(1.0), viewport_size.1.max(1.0));
                    let (origin, direction) = ui.viewport.camera.ray(
                        2.0 * x / size.0 - 1.0,
                        1.0 - 2.0 * y / size.1,
                        size.0 / size.1,
                    );
                    let result = controller.select_center(
                        &mut lock(&ui.session),
                        &Ray {
                            origin: origin.as_dvec3(),
                            direction: direction.as_dvec3(),
                        },
                    );
                    app.set_terrain_center_picking(controller.is_picking());
                    app.set_message(match result {
                        Ok(_) => {
                            "Brush center selected. Preview the edit in the terrain dialog.".into()
                        }
                        Err(error) => error.into(),
                    });
                    if !controller.is_picking() {
                        let _ = controller.window.show();
                    }
                }
                return;
            }
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
                if down {
                    let camera = ui.viewport.camera;
                    match ui
                        .object_cycle
                        .begin(&session, camera, pointer, control, alt)
                    {
                        Ok(true) => return,
                        Ok(false) => {}
                        Err(error) => {
                            app.set_message(error.into());
                            return;
                        }
                    }
                }
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
                    (Some(hit), true) => session.select_grouped(&[hit.object], SelectMode::Toggle),
                    (Some(hit), false) => {
                        session.select_grouped(&[hit.object], SelectMode::Replace)
                    }
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
            if ui.object_cycle.moved(Pointer {
                position: DVec2::new(f64::from(x), f64::from(y)),
                size: (
                    size.0.round().max(1.0) as u32,
                    size.1.round().max(1.0) as u32,
                ),
            }) {
                return;
            }
            if let Some(controller) = &ui.terrain
                && controller.is_handling()
            {
                if controller.handle_dragging() {
                    let pointer = Pointer {
                        position: glam::DVec2::new(f64::from(x), f64::from(y)),
                        size: (
                            size.0.round().max(1.0) as u32,
                            size.1.round().max(1.0) as u32,
                        ),
                    };
                    let mut session = lock(&ui.session);
                    session.publish_camera(ui.viewport.camera);
                    if let Err(error) = controller.update_handle(&mut session, pointer) {
                        app.set_message(error.into());
                    }
                }
                return;
            }
            if let Some(controller) = &ui.terrain
                && controller.is_stroking()
            {
                if controller.stroke_dragging() {
                    let size = (size.0.max(1.0), size.1.max(1.0));
                    let (origin, direction) = ui.viewport.camera.ray(
                        2.0 * x / size.0 - 1.0,
                        1.0 - 2.0 * y / size.1,
                        size.0 / size.1,
                    );
                    if let Err(error) = controller.update_stroke(
                        &mut lock(&ui.session),
                        &Ray {
                            origin: origin.as_dvec3(),
                            direction: direction.as_dvec3(),
                        },
                    ) {
                        app.set_message(error.into());
                    }
                }
                return;
            }
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
            ui.object_cycle.cancel();
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
                ui.object_cycle.cancel();
                if let Some(controller) = &ui.terrain
                    && controller.is_stroking()
                    && controller.stroke_dragging()
                {
                    controller.discard_stroke(&mut lock(&ui.session));
                    app.unwrap().set_terrain_stroking(false);
                }
                if let Some(controller) = &ui.terrain
                    && controller.handle_dragging()
                {
                    controller.cancel_handles(&mut lock(&ui.session), false);
                    app.unwrap().set_terrain_handling(false);
                }
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
                if session.materials().generation() != ui.seen_materials {
                    ui.seen_materials = session.materials().generation();
                    refresh_material_preview(&app, &mut session);
                    refresh_texture_brush_sample_assets(&app, &session);
                    if let Some(window) = &ui.primitive_window
                        && window.window().is_visible()
                    {
                        primitive_ui::refresh(window, &mut session);
                    }
                    if let Some(window) = &ui.components_window
                        && window.window().is_visible()
                    {
                        components_ui::refresh_material_previews(window, &session);
                    }
                    if let Some(window) = &ui.surface_window
                        && window.window().is_visible()
                    {
                        materials::refresh_surface_material(window, &session);
                    }
                    if let Some(controller) = &ui.surface_group
                        && controller.window.window().is_visible()
                    {
                        controller.refresh_material(&session);
                    }
                    if let Some(controller) = &ui.material_replace
                        && controller.window.window().is_visible()
                    {
                        controller.refresh_materials(&session);
                    }
                    if let Some(controller) = &ui.polygon
                        && controller.window.window().is_visible()
                    {
                        controller.refresh_material(&session);
                    }
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
                    let mut preview_error = mesh.issue_summary();
                    if let Some(report) = session.material_reload_report()
                        && report.failed > 0
                    {
                        if !preview_error.is_empty() {
                            preview_error.push('\n');
                        }
                        preview_error.push_str(&report.summary);
                        if let Some(failure) = report.failures.first() {
                            preview_error
                                .push_str(&format!(" · {}: {}", failure.material, failure.error));
                        }
                    }
                    app.set_preview_error(preview_error.into());
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
                    if let Some(controller) = &ui.model
                        && controller.window.window().is_visible()
                    {
                        controller.refresh(&session);
                    }
                    if let Some(controller) = &ui.model_browser
                        && controller.window.window().is_visible()
                    {
                        controller.refresh(&session);
                    }
                    if let Some(controller) = &ui.particles
                        && controller.window.window().is_visible()
                    {
                        controller.refresh(&session);
                    }
                    if let Some(controller) = &ui.sounds
                        && controller.window.window().is_visible()
                    {
                        controller.refresh(&mut session);
                    }
                    if let Some(controller) = &ui.repeat
                        && controller.window.window().is_visible()
                    {
                        controller.refresh(&session);
                    }
                    if let Some(controller) = &ui.clipboard
                        && controller.window.window().is_visible()
                    {
                        controller.refresh(&session);
                    }
                    if let Some(controller) = &ui.placement
                        && (controller.window.window().is_visible() || controller.is_picking())
                    {
                        controller.refresh(&session);
                    }
                    if let Some(controller) = &ui.lighting
                        && controller.window.window().is_visible()
                    {
                        controller.refresh(&session);
                    }
                    if let Some(controller) = &ui.terrain
                        && (controller.window.window().is_visible()
                            || controller.is_stroking()
                            || controller.is_handling())
                    {
                        controller.refresh(&session);
                    }
                    if let Some(controller) = &ui.terrain_triangles
                        && (controller.window.window().is_visible() || controller.is_picking())
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
                    app.set_sky_preview(session.view().sky_preview);
                    app.set_sky_preview_status(
                        session
                            .skybox_preview()
                            .map_or_else(
                                || "Set world skyname to select a 2D sky".to_owned(),
                                |skybox| skybox.summary(),
                            )
                            .into(),
                    );
                    app.set_surface_view(session.view().surface.index());
                    app.set_smoothing_view_group(session.view().smoothing_group as i32);
                    app.set_show_leak(session.view().leak_path);
                    panel_dirty = true;
                }
                if session.view_serial() != ui.seen_view {
                    ui.seen_view = session.view_serial();
                    refresh_view_controls(&app, &session);
                    app.set_fog_preview_status(
                        session.fog_preview_state()["summary"]
                            .as_str()
                            .unwrap()
                            .into(),
                    );
                    app.set_working_region_active(session.working_region_enabled());
                    ui.viewport.options = session.view();
                    ui.viewport.redraw();
                    refresh_spatial(&app, &session);
                    refresh_texture_brush_sample(&app, &mut session);
                    if let Some(controller) = &ui.model_browser
                        && controller.window.window().is_visible()
                    {
                        controller.refresh(&session);
                    }
                    if let Some(controller) = &ui.particles
                        && controller.window.window().is_visible()
                    {
                        controller.refresh(&session);
                    }
                    if let Some(controller) = &ui.sounds
                        && controller.window.window().is_visible()
                    {
                        controller.refresh(&mut session);
                    }
                    if let Some(controller) = &ui.repeat
                        && controller.window.window().is_visible()
                    {
                        controller.refresh(&session);
                    }
                    if let Some(controller) = &ui.clipboard
                        && controller.window.window().is_visible()
                    {
                        controller.refresh(&session);
                    }
                    if let Some(controller) = &ui.placement
                        && (controller.window.window().is_visible() || controller.is_picking())
                    {
                        controller.refresh(&session);
                    }
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
                    if let Some(controller) = &ui.terrain
                        && controller.window.window().is_visible()
                    {
                        controller.refresh(&session);
                    }
                    if let Some(controller) = &ui.terrain_triangles
                        && (controller.window.window().is_visible() || controller.is_picking())
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
                        refresh_material_window(window, &mut session);
                    }
                    app.set_show_tools(session.view().tool_textures);
                    app.set_sky_preview(session.view().sky_preview);
                    app.set_sky_preview_status(
                        session
                            .skybox_preview()
                            .map_or_else(
                                || "Set world skyname to select a 2D sky".to_owned(),
                                |skybox| skybox.summary(),
                            )
                            .into(),
                    );
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
                    if let Some(controller) = &ui.polygon
                        && controller.window.window().is_visible()
                    {
                        controller.refresh(&session);
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
                let recent_maps = (session.recent_maps_serial(), session.scene_serial());
                if recent_maps != ui.seen_recent_maps {
                    ui.seen_recent_maps = recent_maps;
                    if let Some(window) = &ui.recent_maps_window
                        && window.window().is_visible()
                    {
                        recent_maps_ui::refresh(window, &session);
                    }
                }
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
                if let Some(window) = &ui.merge_window
                    && window.window().is_visible()
                {
                    merge_ui::refresh_changed(window, &lock(&ui.session));
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
                if let Some(controller) = &ui.object_groups
                    && controller.window.window().is_visible()
                {
                    controller.refresh(&lock(&ui.session));
                }
                if let Some(controller) = &ui.working_region
                    && controller.window.window().is_visible()
                {
                    controller.refresh(&mut lock(&ui.session));
                }
                if let Some(controller) = &ui.compile_region
                    && controller.window.window().is_visible()
                {
                    controller.refresh(&lock(&ui.session));
                }
                if let Some(controller) = &ui.overlay
                    && controller.window.window().is_visible()
                {
                    controller.refresh(&lock(&ui.session));
                }
                if let Some(controller) = &ui.linked_path
                    && controller.window.window().is_visible()
                {
                    controller.refresh(&lock(&ui.session));
                }
                if let Some(controller) = &ui.cubemap
                    && controller.window.window().is_visible()
                {
                    controller.refresh(&lock(&ui.session));
                }
                if let Some(controller) = &ui.layers
                    && controller.window.window().is_visible()
                {
                    controller.refresh(&lock(&ui.session));
                }
                if let Some(controller) = &ui.prefabs {
                    controller.refresh(&lock(&ui.session));
                }
                if let Some(controller) = &ui.history
                    && controller.window.window().is_visible()
                {
                    controller.refresh(&lock(&ui.session));
                }
                if let Some(controller) = &ui.saved_views
                    && controller.window.window().is_visible()
                {
                    controller.refresh(&lock(&ui.session));
                }
                if let Some(controller) = &ui.measurements
                    && controller.window.window().is_visible()
                {
                    controller.refresh(&lock(&ui.session));
                }
                if let Some(controller) = &ui.grid_defaults
                    && controller.window.window().is_visible()
                {
                    controller.refresh(&lock(&ui.session));
                }
                if let Some(controller) = &ui.sounds
                    && controller.window.window().is_visible()
                {
                    controller.refresh_playback(&mut lock(&ui.session));
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

            let entity_picking =
                lock(&ui.session).get_entity_placement()["draft"]["picking"] == true;
            app.set_surface_picking(
                lock(&ui.session).surface_placement()["picking"] == true || entity_picking,
            );
            if let Some(controller) = &ui.entity_placement
                && controller.window.window().is_visible()
            {
                controller.refresh(&lock(&ui.session));
            }
            app.set_terrain_triangle_picking(
                ui.terrain_triangles
                    .as_ref()
                    .is_some_and(|controller| controller.is_picking()),
            );
            app.set_terrain_center_picking(
                ui.terrain
                    .as_ref()
                    .is_some_and(|controller| controller.is_picking()),
            );
            app.set_terrain_stroking(
                ui.terrain
                    .as_ref()
                    .is_some_and(|controller| controller.is_stroking()),
            );
            app.set_terrain_handling(
                ui.terrain
                    .as_ref()
                    .is_some_and(|controller| controller.is_handling()),
            );
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
pub(crate) mod tests;

pub(crate) use entities::property_row;
pub(crate) use materials::{
    bind_material_reload, common_face_number, preview_image, refresh_material_catalog,
    refresh_material_window, review_material_choice,
};
