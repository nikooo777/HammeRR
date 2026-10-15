mod carve_ui;
mod checks_ui;
mod clipboard_ui;
mod compile_ui;
mod components_ui;
mod connection_ui;
mod context_ui;
mod game;
mod groups_ui;
mod hollow_ui;
mod identity;
mod inspector;
mod interaction;
mod layers_ui;
mod lighting_ui;
mod management_ui;
mod material_favorites;
mod material_replace_ui;
mod materials;
mod mcp;
mod merge_ui;
mod model_browser_ui;
mod model_preview;
mod model_ui;
mod placement_ui;
mod pointfile_ui;
mod polygon_ui;
mod prefabs;
mod prefabs_ui;
mod primitive_ui;
mod recent_maps;
mod recent_maps_ui;
mod recovery;
mod recovery_ui;
mod rename_ui;
mod repeat_ui;
mod runtime;
mod saved_views_ui;
mod session;
mod sounds_ui;
mod spatial;
mod statistics_ui;
mod steam;
mod surface_group_ui;
mod terrain_triangles_ui;
mod terrain_ui;
#[cfg(test)]
mod test_assets;
mod ui;
mod viewport;
mod volume_ui;

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

use mcp::EditorServer;
use render::GpuContext;
use runtime::{SharedSession, lock};
use session::{Actor, Session};
use source_formats::fgd::Catalog;
use source_formats::gamefs::GameFs;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

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

    ui::run(session, ctx, mcp_status, open_message)
}
