mod steam;
mod viewport;

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use render::{Camera, GpuContext, SceneMesh};
use slint::ComponentHandle;
use source_formats::gamefs::GameFs;
use source_formats::vmf::Vmf;

use viewport::Viewport;

slint::include_modules!();

const USAGE: &str = "usage: editor [map.vmf] [--game <gameinfo.txt>]";

struct Args {
    map: Option<PathBuf>,
    gameinfo: Option<PathBuf>,
}

impl Args {
    fn parse() -> Result<Self, String> {
        let mut args = std::env::args_os().skip(1);
        let (mut map, mut gameinfo) = (None, None);
        while let Some(arg) = args.next() {
            match arg.to_str() {
                Some("--game") => gameinfo = Some(args.next().ok_or(USAGE)?.into()),
                Some("-h" | "--help") => return Err(USAGE.into()),
                _ if map.is_none() => map = Some(arg.into()),
                _ => return Err(USAGE.into()),
            }
        }
        Ok(Self { map, gameinfo })
    }
}

struct Editor {
    ctx: GpuContext,
    viewport: Viewport,
    game: Result<GameFs, String>,
    map_path: Option<PathBuf>,
}

impl Editor {
    /// Loads a map and returns a status message.
    fn open(&mut self, path: &Path) -> Result<String, String> {
        let game = self.game.as_ref().map_err(Clone::clone)?;
        let start = Instant::now();
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let vmf = Vmf::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        let mesh = SceneMesh::build(&vmf, game);
        let camera =
            Camera::from_vmf(&vmf).unwrap_or_else(|| Camera::overview(mesh.mins, mesh.maxs));
        self.viewport.set_scene(&self.ctx, &mesh, camera);
        self.map_path = Some(path.to_owned());

        let s = &mesh.stats;
        let mut message = format!(
            "Loaded {} in {:.0?}: {} brushes, {} displacements, {} point entities, {} triangles",
            path.file_name().unwrap_or_default().to_string_lossy(),
            start.elapsed(),
            s.brushes,
            s.displacements,
            s.point_entities,
            mesh.indices.len() / 3,
        );
        if s.missing_materials > 0 {
            message += &format!(", {} missing materials", s.missing_materials);
        }
        if s.invalid_brushes > 0 {
            message += &format!(", {} invalid brushes", s.invalid_brushes);
        }
        Ok(message)
    }
}

fn mount_game(gameinfo: Option<PathBuf>) -> Result<GameFs, String> {
    let gameinfo = gameinfo.or_else(steam::find_css_gameinfo).ok_or(
        "Counter-Strike: Source not found in any Steam library; pass --game <gameinfo.txt>",
    )?;
    GameFs::from_gameinfo(&gameinfo).map_err(|e| e.to_string())
}

fn open_map(app: &AppWindow, editor: &mut Editor, path: &Path) {
    match editor.open(path) {
        Ok(message) => {
            app.set_map_name(path.file_name().unwrap_or_default().to_string_lossy().as_ref().into());
            app.set_message(message.into());
        }
        Err(message) => app.set_message(message.into()),
    }
}

fn camera_text(viewport: &Viewport) -> String {
    let c = &viewport.camera;
    format!(
        "{:.0} {:.0} {:.0}  yaw {:.0}° pitch {:.0}°  speed {:.0}",
        c.position.x, c.position.y, c.position.z, c.yaw_degrees, c.pitch_degrees, viewport.speed
    )
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse()?;
    let ctx = GpuContext::new()?;
    slint::BackendSelector::new()
        .require_wgpu_30(slint::wgpu_30::WGPUConfiguration::Manual {
            instance: ctx.instance.clone(),
            adapter: ctx.adapter.clone(),
            device: ctx.device.clone(),
            queue: ctx.queue.clone(),
        })
        .select()?;

    let app = AppWindow::new()?;
    app.set_stats(ctx.adapter.get_info().name.into());
    let game = mount_game(args.gameinfo);
    match &game {
        Ok(fs) => app.set_message(format!("Game: {}", fs.game_dir.display()).into()),
        Err(e) => app.set_message(e.clone().into()),
    }
    let editor = Rc::new(RefCell::new(Editor {
        viewport: Viewport::new(&ctx),
        ctx,
        game,
        map_path: None,
    }));
    if let Some(map) = &args.map {
        open_map(&app, &mut editor.borrow_mut(), map);
    }

    app.on_open_map({
        let (app, editor) = (app.as_weak(), editor.clone());
        move || {
            let (app, editor) = (app.clone(), editor.clone());
            let start_dir = {
                let editor = editor.borrow();
                editor
                    .map_path
                    .as_ref()
                    .and_then(|p| p.parent().map(Path::to_owned))
                    .or_else(|| {
                        let game = editor.game.as_ref().ok()?;
                        steam::sample_maps_dir(&game.game_dir.join("gameinfo.txt"))
                    })
            };
            let dialog = rfd::AsyncFileDialog::new()
                .set_title("Open map")
                .add_filter("Valve Map File", &["vmf"]);
            let dialog = match start_dir {
                Some(dir) => dialog.set_directory(dir),
                None => dialog,
            };
            slint::spawn_local(async move {
                let Some(file) = dialog.pick_file().await else {
                    return;
                };
                if let Some(app) = app.upgrade() {
                    open_map(&app, &mut editor.borrow_mut(), file.path());
                }
            })
            .expect("event loop is running");
        }
    });

    app.on_options_changed({
        let (app, editor) = (app.as_weak(), editor.clone());
        move || {
            let app = app.unwrap();
            let viewport = &mut editor.borrow_mut().viewport;
            viewport.options.tool_textures = app.get_show_tools();
            viewport.redraw();
        }
    });

    app.on_key({
        let editor = editor.clone();
        move |text, down| editor.borrow_mut().viewport.key(&text, down)
    });

    app.on_mouse_button({
        let (app, editor) = (app.as_weak(), editor.clone());
        move |button, down, x, y| {
            let viewport = &mut editor.borrow_mut().viewport;
            viewport.mouse_button(button, down, x, y);
            app.unwrap().set_flying(viewport.looking());
        }
    });

    app.on_mouse_moved({
        let editor = editor.clone();
        move |x, y| editor.borrow_mut().viewport.mouse_moved(x, y)
    });

    app.on_wheel({
        let editor = editor.clone();
        move |delta| editor.borrow_mut().viewport.wheel(delta)
    });

    app.on_viewport_focus_lost({
        let (app, editor) = (app.as_weak(), editor.clone());
        move || {
            editor.borrow_mut().viewport.release_all();
            app.unwrap().set_flying(false);
        }
    });

    let frame_timer = slint::Timer::default();
    let mut last_tick = Instant::now();
    frame_timer.start(slint::TimerMode::Repeated, Duration::from_millis(8), {
        let (app, editor) = (app.as_weak(), editor.clone());
        move || {
            let Some(app) = app.upgrade() else { return };
            let now = Instant::now();
            let dt = (now - last_tick).as_secs_f32().min(0.1);
            last_tick = now;

            let editor = &mut *editor.borrow_mut();
            editor.viewport.update(dt);
            let scale = app.window().scale_factor();
            let size = (
                (app.get_viewport_width() * scale).round() as u32,
                (app.get_viewport_height() * scale).round() as u32,
            );
            if let Some(frame) = editor.viewport.render(&editor.ctx, size) {
                app.set_frame(frame);
                app.set_camera(camera_text(&editor.viewport).into());
            }
        }
    });

    app.run()?;
    Ok(())
}
