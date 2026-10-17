use crate::runtime::{DocumentHandle, lock};
use crate::session::{Actor, Session};
use crate::{build_recipes, material_favorites, prefabs, recent_maps, recovery, shortcuts, steam};
use source_formats::{fgd::Catalog, gamefs::GameFs};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

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
    compile_queue: Option<crate::runtime::CompileQueue>,
    _path_claims: crate::path_claims::Authority,
    pub material_favorites: material_favorites::Authority,
    pub recent_maps: recent_maps::Authority,
    pub shortcuts: shortcuts::Authority,
    pub saved_defaults: crate::saved_defaults::SavedDefaults,
    pub build_recipes: build_recipes::Authority,
    pub recovery_settings: recovery::Authority,
    pub prefabs: prefabs::Authority,
    document: DocumentHandle,
    pub open_message: Option<Result<String, String>>,
}

impl Application {
    pub fn open(args: &Args) -> Result<Self, String> {
        let compile_queue = crate::runtime::CompileQueue::process()?;
        let directory = recovery::default_directory();
        let saved_defaults = crate::saved_defaults::SavedDefaults::configured(directory.clone());
        let session = Arc::new(Mutex::new(Session::with_saved_defaults(
            mount_game(args.gameinfo.clone()),
            saved_defaults.clone(),
        )));
        let path_claims = crate::path_claims::Authority::default();
        lock(&session).use_application_path_claims(path_claims.clone());
        if args.engine_root.is_some() || args.steam_root.is_some() {
            lock(&session).configure_native_capture_runtime(
                Actor::System,
                args.engine_root.as_deref(),
                args.steam_root.as_deref(),
            )?;
        }
        let material_favorites = material_favorites::Authority::configured(directory.clone());
        lock(&session).use_application_material_favorites(material_favorites.clone());
        let recent_maps =
            recent_maps::Authority::configured(recent_maps::directory(directory.clone()));
        lock(&session).use_application_recent_maps(recent_maps.clone());
        let shortcuts = shortcuts::Authority::configured(directory.clone());
        lock(&session).use_application_shortcuts(shortcuts.clone());
        let build_recipes = build_recipes::Authority::configured(directory.clone());
        lock(&session).use_application_build_recipes(build_recipes.clone());
        let recovery_settings = recovery::Authority::configured(directory.clone());
        lock(&session).use_application_recovery_settings(recovery_settings.clone());
        let prefabs = prefabs::Authority::configured(directory.clone());
        lock(&session).use_application_prefabs(prefabs.clone());
        lock(&session).configure_recovery(directory);
        let document = DocumentHandle::start_managed(session.clone(), &compile_queue)?;
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
            compile_queue: Some(compile_queue),
            _path_claims: path_claims,
            material_favorites,
            recent_maps,
            shortcuts,
            saved_defaults,
            build_recipes,
            recovery_settings,
            prefabs,
            document,
            open_message,
        })
    }
}

impl Application {
    pub fn document(&self) -> DocumentHandle {
        self.document.clone()
    }

    pub fn shutdown(&mut self) -> Result<(), String> {
        if let Some(queue) = &self.compile_queue {
            queue.close_admission();
        }
        let document = self.document.shutdown();
        let queue = self
            .compile_queue
            .as_mut()
            .map_or(Ok(()), crate::runtime::CompileQueue::shutdown);
        let native = self
            .compile_queue
            .as_ref()
            .map_or(Ok(()), |queue| queue.native().cleanup_result());
        let drained = match (document, queue) {
            (Ok(()), result) | (result, Ok(())) => result,
            (Err(document), Err(queue)) => Err(format!(
                "{document}; compiler queue shutdown failed: {queue}"
            )),
        };
        match (drained, native) {
            (Ok(()), result) | (result, Ok(())) => result,
            (Err(drained), Err(native)) => Err(format!(
                "{drained}; native activity shutdown failed: {native}"
            )),
        }
    }

    pub fn finish(
        mut self,
        primary: Result<(), Box<dyn std::error::Error>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        match (primary, self.shutdown()) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), Ok(())) => Err(error),
            (Ok(()), Err(error)) => Err(error.into()),
            (Err(primary), Err(shutdown)) => {
                Err(format!("{primary}; shutdown failed: {shutdown}").into())
            }
        }
    }
}

impl Drop for Application {
    fn drop(&mut self) {
        self.document.shutdown_on_drop();
    }
}

#[cfg(test)]
mod tests;
