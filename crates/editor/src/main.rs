mod game;
mod mcp;
mod session;
mod steam;
mod viewport;

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use glam::{DVec3, Vec3};
use map_core::ObjectId;
use map_core::edit::{self, FaceTarget};
use map_core::query::{self, Ray};
use mcp::{EditorServer, SharedSession, id_string, lock};
use render::GpuContext;
use session::{Actor, SelectMode, Session};
use slint::platform::Key;
use slint::{ComponentHandle, ModelRc, VecModel};
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
const GRID: f64 = 16.0;
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
    seen_log: u64,
    game_log_window: Option<GameLogWindow>,
    last_log_refresh: Instant,
    entity_window: Option<EntityWindow>,
    seen_definitions: u64,
}

type SharedUi = Rc<RefCell<Ui>>;

fn session_of(ui: &SharedUi) -> SharedSession {
    ui.borrow().session.clone()
}

fn snap(v: f64) -> f64 {
    (v / GRID).round() * GRID
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
            let title = format!(
                "{} · {}",
                entity.classname().unwrap_or("entity"),
                id_string(ids[0])
            );
            let mut detail = size_text(query::entity_bounds(entity));
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
    let mut names: Vec<String> = entity
        .properties
        .iter()
        .map(|(key, _)| key.into())
        .collect();
    if let Some(class) = class {
        for property in &class.properties {
            if !names
                .iter()
                .any(|name| name.eq_ignore_ascii_case(&property.name))
            {
                names.push(property.name.clone());
            }
        }
    }
    names
        .into_iter()
        .map(|name| {
            let property = class.and_then(|class| class.property(&name));
            let value = entity
                .properties
                .get(&name)
                .map(str::to_owned)
                .or_else(|| property.and_then(|property| property.default_value()))
                .unwrap_or_default();
            let mut labels = Vec::new();
            let mut values = Vec::new();
            let mut flags = Vec::new();
            let mask = value.parse::<u64>().unwrap_or_default();
            if let Some(property) = property {
                for choice in &property.choices {
                    if property.kind == "flags" {
                        let bit = choice.value.parse::<u64>().unwrap_or_default();
                        flags.push(EntityFlag {
                            label: choice.label.clone().into(),
                            value: choice.value.clone().into(),
                            checked: bit != 0 && mask & bit == bit,
                        });
                    } else {
                        labels.push(slint::SharedString::from(format!(
                            "{} ({})",
                            choice.label, choice.value
                        )));
                        values.push(slint::SharedString::from(choice.value.clone()));
                    }
                }
            }
            if !values.is_empty() && !values.iter().any(|item| item.as_str() == value) {
                labels.insert(0, format!("{value} (stored value)").into());
                values.insert(0, value.clone().into());
            }
            let choice_index = values
                .iter()
                .position(|item| item.as_str() == value)
                .unwrap_or_default() as i32;
            KeyValue {
                key: name.clone().into(),
                value: value.into(),
                label: property
                    .map_or(name.as_str(), |property| property.label.as_str())
                    .into(),
                description: property
                    .map_or("", |property| property.description.as_str())
                    .into(),
                type_name: property
                    .map_or("string", |property| property.kind.as_str())
                    .into(),
                read_only: property.is_some_and(|property| property.read_only),
                choices: ModelRc::new(VecModel::from(labels)),
                choice_values: ModelRc::new(VecModel::from(values)),
                choice_index,
                flags: ModelRc::new(VecModel::from(flags)),
            }
        })
        .collect()
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

fn refresh_panel(app: &AppWindow, session: &Session) {
    let doc = session.document();
    app.set_map_title(session.title().into());
    app.set_can_undo(doc.undo_labels().next().is_some());
    app.set_can_redo(doc.redo_labels().next().is_some());
    app.set_compiling(session.compiling());
    app.set_launching(session.launching());
    app.set_game_running(session.game_running());
    app.set_has_selection(!session.selection().is_empty());
    let (title, detail, properties) = selection_summary(session);
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
        .set_buttons(rfd::MessageButtons::YesNo);
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
            Ok(window) => ui.game_log_window = Some(window),
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
    let (origin, direction) = camera.ray(0.0, 0.0, 1.0);
    let ray = Ray {
        origin: origin.as_dvec3(),
        direction: direction.as_dvec3(),
    };
    let half = BLOCK_SIZE / 2.0;
    let (mins, maxs) = match session.pick(&ray) {
        Some(hit) => {
            let base = DVec3::new(snap(hit.point.x), snap(hit.point.y), snap(hit.point.z));
            (
                base - DVec3::new(half, half, 0.0),
                base + DVec3::new(half, half, BLOCK_SIZE),
            )
        }
        None => {
            let p = ray.origin + ray.direction * 384.0;
            let center = DVec3::new(snap(p.x), snap(p.y), snap(p.z));
            (center - DVec3::splat(half), center + DVec3::splat(half))
        }
    };
    let material = app.get_material().to_string();
    let result = session
        .edit(Actor::User, "create block", |tx| {
            edit::create_block(tx, mins, maxs, &material)
        })
        .and_then(|id| session.select(&[id], SelectMode::Replace));
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
                let offset = DVec3::new(GRID, GRID, 0.0);
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
        drop(lock(&session_of(ui)).select(&[], SelectMode::Replace));
        return true;
    }
    let (forward, right) = nudge_axes(ui.borrow().viewport.camera.forward());
    let direction = [
        (Key::UpArrow, forward),
        (Key::DownArrow, -forward),
        (Key::RightArrow, right),
        (Key::LeftArrow, -right),
        (Key::PageUp, DVec3::Z),
        (Key::PageDown, DVec3::NEG_Z),
    ]
    .into_iter()
    .find(|(k, _)| key == key_text(*k))
    .map(|(_, d)| d);
    if let Some(direction) = direction {
        let step = if shift { 1.0 } else { GRID };
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
        seen_log: 0,
        game_log_window: None,
        last_log_refresh: Instant::now(),
        entity_window: None,
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
    app.on_apply_material({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            let app = app.unwrap();
            let material = app.get_material().trim().to_owned();
            edit_selection(
                &app,
                &ui,
                |_| format!("apply {material}"),
                |tx, ids| {
                    let targets: Vec<FaceTarget> =
                        ids.iter().copied().map(FaceTarget::Object).collect();
                    edit::set_material(tx, &targets, &material).map(drop)
                },
            );
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
                refresh_panel(&app, &session);
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
                refresh_panel(&app, &session);
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
                refresh_panel(&app, &session);
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
            let result = match (hit, click.control) {
                (Some(hit), true) => session.select(&[hit.object], SelectMode::Toggle),
                (Some(hit), false) => session.select(&[hit.object], SelectMode::Replace),
                (None, true) => Ok(()),
                (None, false) => session.select(&[], SelectMode::Replace),
            };
            if let Err(message) = result {
                app.set_message(message.into());
            }
        }
    });
    app.on_mouse_moved({
        let ui = ui.clone();
        move |x, y| ui.borrow_mut().viewport.mouse_moved(x, y)
    });
    app.on_wheel({
        let ui = ui.clone();
        move |delta| ui.borrow_mut().viewport.wheel(delta)
    });
    app.on_viewport_focus_lost({
        let (app, ui) = (app.as_weak(), ui.clone());
        move || {
            ui.borrow_mut().viewport.release_all();
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
                    ui.viewport.redraw();
                } else {
                    session.publish_camera(ui.viewport.camera);
                }
                if session.scene_serial() != ui.seen_scene {
                    ui.seen_scene = session.scene_serial();
                    let mesh = session.build_scene();
                    ui.viewport.options = session.view();
                    ui.viewport.set_scene(&ui.ctx, &mesh, session.materials());
                    app.set_show_tools(session.view().tool_textures);
                    app.set_show_leak(session.view().leak_path);
                    panel_dirty = true;
                }
                if session.log_serial() != ui.seen_log {
                    ui.seen_log = session.log_serial();
                    if let Some(entry) = session.recent_log().next() {
                        app.set_message(format!("{}: {}", entry.actor.label(), entry.text).into());
                    }
                    panel_dirty = true;
                }
                if panel_dirty {
                    refresh_panel(&app, &session);
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
            if let Some(frame) = ui.viewport.render(&ui.ctx, size) {
                app.set_frame(frame);
                let c = &ui.viewport.camera;
                app.set_camera(
                    format!(
                        "{:.0} {:.0} {:.0}  yaw {:.0}° pitch {:.0}°  speed {:.0}",
                        c.position.x,
                        c.position.y,
                        c.position.z,
                        c.yaw_degrees,
                        c.pitch_degrees,
                        ui.viewport.speed
                    )
                    .into(),
                );
            }
        }
    });

    app.run()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::Model;
    use source_formats::vmf::Entity;

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
        assert!(row("editor_id").read_only);
        assert_eq!(row("custom_key").value, "keep me");
        assert_eq!(row("custom_key").type_name, "string");
        assert!(entity.properties.get("_light").is_none());
    }
}
