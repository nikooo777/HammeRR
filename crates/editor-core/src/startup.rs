use crate::runtime::{SharedSession, lock};
use crate::session::{Actor, Session};
use crate::{recovery, steam};
use source_formats::{fgd::Catalog, gamefs::GameFs};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const USAGE: &str = "\
usage: editor | hammerr-headless [map.vmf] [--game <gameinfo.txt>] [--fgd <file.fgd>] [--mcp-port <port> | --no-mcp] [--headless]

  --game       gameinfo.txt to load content from (default: Counter-Strike: Source via Steam)
  --engine-root native CS:S installation for reflection capture (default: configured game root)
  --steam-root  Steam client configuration root (default: unique validated client)
  --fgd        entity definition file (default: installed CS:S SDK FGD)
  --mcp-port   port for the MCP server at http://127.0.0.1:<port>/mcp (default 7377)
  --no-mcp     don't start the MCP server
  --headless   no window; serve MCP over stdin/stdout; graphics initialized on first render";

const DEFAULT_MCP_PORT: u16 = 7377;
const SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(10);

struct ShutdownGuard(SharedSession);

impl Drop for ShutdownGuard {
    fn drop(&mut self) {
        lock(&self.0).shutdown();
        loop {
            let busy = {
                let session = lock(&self.0);
                session.launching() || session.recovery_busy() || session.compiling()
            };
            if !busy {
                break;
            }
            std::thread::sleep(SHUTDOWN_POLL_INTERVAL);
        }
    }
}

pub struct Args {
    pub map: Option<PathBuf>,
    pub gameinfo: Option<PathBuf>,
    pub fgd: Option<PathBuf>,
    pub engine_root: Option<PathBuf>,
    pub steam_root: Option<PathBuf>,
    pub mcp_port: Option<u16>,
    pub headless: bool,
}

impl Args {
    pub fn parse() -> Result<Self, String> {
        let mut args = std::env::args_os().skip(1);
        let mut parsed = Self {
            map: None,
            gameinfo: None,
            fgd: None,
            engine_root: None,
            steam_root: None,
            mcp_port: Some(DEFAULT_MCP_PORT),
            headless: false,
        };
        while let Some(arg) = args.next() {
            match arg.to_str() {
                Some("--game") => parsed.gameinfo = Some(args.next().ok_or(USAGE)?.into()),
                Some("--fgd") => parsed.fgd = Some(args.next().ok_or(USAGE)?.into()),
                Some("--engine-root") => {
                    parsed.engine_root = Some(args.next().ok_or(USAGE)?.into())
                }
                Some("--steam-root") => parsed.steam_root = Some(args.next().ok_or(USAGE)?.into()),
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

pub fn mount_game(gameinfo: Option<PathBuf>) -> Result<GameFs, String> {
    let gameinfo = gameinfo.or_else(steam::find_css_gameinfo).ok_or(
        "Counter-Strike: Source not found in any Steam library; pass --game <gameinfo.txt>",
    )?;
    GameFs::from_gameinfo(&gameinfo).map_err(|e| e.to_string())
}

pub struct Application {
    // Shutdown and exit recovery run while the session still has a strong owner.
    _shutdown: ShutdownGuard,
    _recovery_worker: recovery::Worker,
    pub session: SharedSession,
    pub open_message: Option<Result<String, String>>,
}

impl Application {
    pub fn open(args: &Args) -> Result<Self, String> {
        let session = Arc::new(Mutex::new(Session::new(mount_game(args.gameinfo.clone()))));
        if args.engine_root.is_some() || args.steam_root.is_some() {
            lock(&session).configure_native_capture_runtime(
                Actor::System,
                args.engine_root.as_deref(),
                args.steam_root.as_deref(),
            )?;
        }
        lock(&session).configure_recovery(recovery::default_directory());
        let worker = recovery::Worker::start(&session)?;
        let shutdown = ShutdownGuard(session.clone());
        let fgd = args
            .fgd
            .clone()
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
        Ok(Self {
            _shutdown: shutdown,
            _recovery_worker: worker,
            session,
            open_message,
        })
    }
}
