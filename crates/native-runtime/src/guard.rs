use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{self, Read, Seek, SeekFrom};
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha1::{Digest, Sha1};

use crate::CaptureEnvironment;
use crate::util::{self, MAX_CONFIG, invalid};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Identity {
    pub account: String,
    pub steam_id64: String,
    pub steam_root: PathBuf,
    pub app_id: u32,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Baseline {
    pub root: PathBuf,
    pub steam: PathBuf,
    pub identity: Identity,
    pub files: BTreeMap<String, Option<String>>,
    pub cloud: Value,
    pub cloud_log_size: u64,
}

pub(crate) fn ensure_no_client(root: &Path) -> io::Result<()> {
    let executable = root.join("cstrike_linux64").canonicalize()?;
    let mut scanned = 0usize;
    for entry in fs::read_dir("/proc")? {
        let entry = entry?;
        scanned += 1;
        if scanned > 32768 {
            return Err(invalid("Client process scan exceeds its budget"));
        }
        if entry
            .file_name()
            .to_str()
            .and_then(|s| s.parse::<u32>().ok())
            .is_none()
        {
            continue;
        }
        if fs::read_link(entry.path().join("exe")).ok().as_deref() == Some(&executable) {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "CS:S is already running; close it before capturing reflections",
            ));
        }
    }
    Ok(())
}

pub fn resolve_environment(environment: &CaptureEnvironment) -> io::Result<CaptureEnvironment> {
    let engine_root = environment.engine_root.canonicalize()?;
    let actual = live_steam_root()?;
    let steam_root = if environment.steam_root.as_os_str().is_empty() {
        actual.clone()
    } else {
        environment.steam_root.canonicalize()?
    };
    require_live_root(&steam_root, &actual)?;
    validate_steam_root(&steam_root, &engine_root)?;
    Ok(CaptureEnvironment {
        engine_root,
        steam_root,
        mounts: environment.mounts.clone(),
    })
}

fn require_live_root(configured: &Path, actual: &Path) -> io::Result<()> {
    if configured != actual {
        return Err(invalid(
            "Configured Steam root is not the actual running Steam client root",
        ));
    }
    Ok(())
}

fn live_steam_root() -> io::Result<PathBuf> {
    let uid = unsafe { libc::geteuid() };
    let mut roots = BTreeSet::new();
    let mut entries = 0usize;
    for entry in fs::read_dir("/proc")? {
        let entry = entry?;
        entries += 1;
        if entries > 32768 {
            return Err(invalid("Process identity scan exceeds its budget"));
        }
        if entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
            .is_none()
        {
            continue;
        }
        if entry
            .metadata()
            .ok()
            .is_none_or(|metadata| metadata.uid() != uid)
        {
            continue;
        }
        let Ok(executable) = fs::read_link(entry.path().join("exe")) else {
            continue;
        };
        if executable.file_name().is_none_or(|name| name != "steam") {
            continue;
        }
        let Some(parent) = executable.parent() else {
            continue;
        };
        if !parent
            .file_name()
            .is_some_and(|name| name == "ubuntu12_32" || name == "ubuntu12_64")
        {
            continue;
        }
        if let Some(root) = parent.parent() {
            roots.insert(root.canonicalize()?);
        }
    }
    if roots.len() != 1 {
        return Err(invalid(
            "Native capture requires one identifiable running Steam main client; sandbox-hidden processes cannot establish its root",
        ));
    }
    roots
        .pop_first()
        .ok_or_else(|| invalid("Live Steam client root unavailable"))
}

fn validate_steam_root(root: &Path, engine: &Path) -> io::Result<()> {
    if !root.join("config/loginusers.vdf").is_file()
        || !root.join("logs/cloud_log.txt").is_file()
        || !root.join("userdata").is_dir()
    {
        return Err(invalid(
            "Steam root requires existing client config, userdata and Cloud log",
        ));
    }
    let libraries = vdf(&root.join("steamapps/libraryfolders.vdf"))?;
    let mut roots = vec![root.to_owned()];
    if let Some(folders) = libraries["libraryfolders"].as_object() {
        for folder in folders.values() {
            if let Some(path) = folder["path"].as_str() {
                roots.push(PathBuf::from(path));
            }
        }
    }
    if !roots.iter().any(|root| {
        root.join("steamapps/common/Counter-Strike Source")
            .canonicalize()
            .ok()
            .as_deref()
            == Some(engine)
    }) {
        return Err(invalid(
            "Steam client library configuration does not contain the selected CS:S installation",
        ));
    }
    Ok(())
}

fn upload_lines(steam: &Path, cursor: u64) -> io::Result<Vec<String>> {
    let mut file = fs::File::open(steam.join("logs/cloud_log.txt"))?;
    if file.metadata()?.len() < cursor {
        return Err(invalid("Steam Cloud log rotated during native preflight"));
    }
    file.seek(SeekFrom::Start(cursor))?;
    let mut bytes = Vec::new();
    file.take(8 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(invalid(
            "Steam Cloud log exceeded the preflight read budget",
        ));
    }
    Ok(String::from_utf8_lossy(&bytes)
        .lines()
        .filter(|line| {
            line.contains("[AppID 240]")
                && [
                    "Need to upload file",
                    "HTTP upload",
                    "Upload batch initiated",
                    "Upload OK",
                ]
                .iter()
                .any(|term| line.contains(term))
        })
        .map(str::to_owned)
        .collect())
}

pub fn validate_environment(environment: &CaptureEnvironment) -> io::Result<()> {
    crate::isolation::preflight()?;
    let root = environment.engine_root.canonicalize()?;
    if !root.join("cstrike_linux64").is_file() || !root.join("cstrike/gameinfo.txt").is_file() {
        return Err(invalid(
            "Configured native engine root is not a Linux CS:S installation",
        ));
    }
    for (name, expected) in [
        ("engine.so", util::ENGINE_SHA),
        ("vtex_dll.so", util::VTEX_SHA),
        ("bsppack.so", util::BSPPACK_SHA),
        ("libsteam_api.so", util::STEAM_SHA),
    ] {
        if util::hash_file(&root.join("bin/linux64").join(name), util::MAX_BSP)? != expected {
            return Err(invalid(format!(
                "Native {name} changed; the supported runtime must be re-verified"
            )));
        }
    }
    if environment.mounts.len() > 256 {
        return Err(invalid("Native content mount budget exceeded"));
    }
    for mount in &environment.mounts {
        let path = mount.path.canonicalize()?;
        if path
            .to_str()
            .is_none_or(|p| p.contains(['"', '\n', '\r', '\0']))
            || mount.ids.len() > 16
        {
            return Err(invalid("Unsupported native content mount path or IDs"));
        }
    }
    if !environment.steam_root.as_os_str().is_empty()
        && !environment
            .steam_root
            .join("config/loginusers.vdf")
            .is_file()
    {
        return Err(invalid("Configured Steam root has no loginusers.vdf"));
    }
    if std::env::var_os("LD_PRELOAD").is_some_and(|v| !v.is_empty()) {
        return Err(invalid("Private capture cannot inherit LD_PRELOAD"));
    }
    Ok(())
}

pub(crate) fn identity_worker(root: &Path, steam_root: &Path) -> io::Result<Identity> {
    if !cfg!(all(target_os = "linux", target_arch = "x86_64"))
        || std::env::var("SteamAppId").ok().as_deref() != Some("240")
        || std::env::var("SteamGameId").ok().as_deref() != Some("240")
        || std::env::var_os("LD_PRELOAD").is_some_and(|v| !v.is_empty())
    {
        return Err(invalid(
            "Steam identity requires the verified Linux64 App240 environment",
        ));
    }
    let actual_root = live_steam_root()?;
    require_live_root(&steam_root.canonicalize()?, &actual_root)?;
    let library = root.join("bin/linux64/libsteam_api.so");
    if util::hash_file(&library, util::MAX_BSP)? != util::STEAM_SHA {
        return Err(invalid("Steam flat API library changed"));
    }
    unsafe {
        let library = libloading::Library::new(&library).map_err(io::Error::other)?;
        let init = library
            .get::<unsafe extern "C" fn() -> bool>(b"SteamAPI_Init\0")
            .map_err(io::Error::other)?;
        let shutdown = library
            .get::<unsafe extern "C" fn()>(b"SteamAPI_Shutdown\0")
            .map_err(io::Error::other)?;
        let user = library
            .get::<unsafe extern "C" fn() -> *mut libc::c_void>(b"SteamAPI_SteamUser_v021\0")
            .map_err(io::Error::other)?;
        let logged_on = library
            .get::<unsafe extern "C" fn(*mut libc::c_void) -> bool>(
                b"SteamAPI_ISteamUser_BLoggedOn\0",
            )
            .map_err(io::Error::other)?;
        let get_id = library
            .get::<unsafe extern "C" fn(*mut libc::c_void) -> u64>(
                b"SteamAPI_ISteamUser_GetSteamID\0",
            )
            .map_err(io::Error::other)?;
        let utils = library
            .get::<unsafe extern "C" fn() -> *mut libc::c_void>(b"SteamAPI_SteamUtils_v010\0")
            .map_err(io::Error::other)?;
        let app_id = library
            .get::<unsafe extern "C" fn(*mut libc::c_void) -> u32>(
                b"SteamAPI_ISteamUtils_GetAppID\0",
            )
            .map_err(io::Error::other)?;
        if !init() {
            return Err(invalid("Read-only Steam identity initialization failed"));
        }
        let result = (|| {
            let user = user();
            let utils = utils();
            if user.is_null() || utils.is_null() || app_id(utils) != 240 || !logged_on(user) {
                return Err(invalid("Steam must have a live logged-on App240 identity"));
            }
            let id = get_id(user);
            let account = id as u32;
            if account == 0
                || id >> 56 != 1
                || (id >> 52) & 15 != 1
                || (id >> 32) & 0xfffff != 1
                || get_id(user) != id
            {
                return Err(invalid(
                    "Steam returned an invalid or changing public individual identity",
                ));
            }
            Ok(Identity {
                account: account.to_string(),
                steam_id64: id.to_string(),
                steam_root: {
                    require_live_root(&actual_root, &live_steam_root()?)?;
                    actual_root.clone()
                },
                app_id: 240,
            })
        })();
        shutdown();
        result
    }
}

fn vdf(path: &Path) -> io::Result<Value> {
    crate::vdf::parse(&util::text(path, MAX_CONFIG)?)
}

pub(crate) fn query_identity(
    environment: &CaptureEnvironment,
    helper: &Path,
    directory: &Path,
) -> io::Result<Identity> {
    let log_path = directory.join("steam-identity.log");
    let log = fs::File::create(&log_path)?;
    let mut child = Command::new(helper)
        .arg("--steam-identity")
        .arg(&environment.engine_root)
        .arg(&environment.steam_root)
        .current_dir(directory)
        .env("SteamAppId", "240")
        .env("SteamGameId", "240")
        .env(
            "LD_LIBRARY_PATH",
            environment.engine_root.join("bin/linux64"),
        )
        .env_remove("LD_PRELOAD")
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .spawn()?;
    let started = Instant::now();
    let result = (|| {
        loop {
            if let Some(status) = child.try_wait()? {
                return if status.success() {
                    Ok(())
                } else {
                    Err(invalid(format!(
                        "Steam identity query failed; inspect {}",
                        log_path.display()
                    )))
                };
            }
            if fs::metadata(&log_path)?.len() > 1024 * 1024 {
                return Err(invalid("Steam identity output exceeds1MiB"));
            }
            if started.elapsed() > Duration::from_secs(15) {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Steam identity query timed out",
                ));
            }
            thread::sleep(Duration::from_millis(50));
        }
    })();
    let cleanup = crate::process::stop_group(&mut child);
    match (result, cleanup) {
        (Ok(()), Ok(())) => {}
        (Err(a), Err(b)) => return Err(invalid(format!("{a}; child cleanup: {b}"))),
        (Err(e), _) | (_, Err(e)) => return Err(e),
    }
    let log = util::text(&log_path, 1024 * 1024)?;
    let records: Vec<&str> = log
        .lines()
        .filter_map(|line| line.strip_prefix("HAMMERR_STEAM_IDENTITY "))
        .collect();
    if records.len() != 1 {
        return Err(invalid("Steam identity child returned no unique identity"));
    }
    let identity: Identity = serde_json::from_str(records[0]).map_err(io::Error::other)?;
    if !environment.steam_root.as_os_str().is_empty()
        && environment.steam_root.canonicalize()? != identity.steam_root
    {
        return Err(invalid(
            "Configured Steam root differs from the live Steam client",
        ));
    }
    let users = vdf(&identity.steam_root.join("config/loginusers.vdf"))?;
    if users["users"].get(&identity.steam_id64).is_none() {
        return Err(invalid(
            "Live Steam user has no matching cached login profile",
        ));
    }
    let start = Instant::now();
    let mut stable = Instant::now();
    let mut previous = None;
    loop {
        let state = cloud_state(&identity).ok();
        if state.is_none() || previous != state {
            stable = Instant::now();
            previous = state.clone();
        }
        if state.is_some()
            && start.elapsed() >= Duration::from_secs(4)
            && stable.elapsed() >= Duration::from_secs(3)
        {
            break;
        }
        if start.elapsed() >= Duration::from_secs(12) {
            return Err(invalid(
                "Steam Cloud did not settle after the identity query",
            ));
        }
        thread::sleep(Duration::from_millis(500));
    }
    util::json_write(&directory.join("steam-identity.json"), &identity)?;
    Ok(identity)
}

fn cloud_state(identity: &Identity) -> io::Result<Value> {
    let userdata = identity.steam_root.join("userdata").join(&identity.account);
    let cache = userdata.join("240");
    let remote = util::read(&cache.join("remote/cfg/config.cfg"), 1024 * 1024)?;
    if remote.is_empty() {
        return Err(invalid(
            "Existing synchronized App240 Cloud config is required",
        ));
    }
    let manifest = vdf(&cache.join("remotecache.vdf"))?;
    let manifest = manifest["240"]
        .as_object()
        .ok_or_else(|| invalid("Missing App240 remote cache"))?;
    let entry = manifest
        .get("cfg/config.cfg")
        .ok_or_else(|| invalid("Cloud config has no cache entry"))?;
    if entry["sha"].as_str() != Some(format!("{:x}", Sha1::digest(&remote)).as_str())
        || entry["syncstate"] != "1"
    {
        return Err(invalid(
            "Cloud config differs from its synchronized cache entry",
        ));
    }
    let shared = vdf(&userdata.join("7/remote/sharedconfig.vdf"))?;
    let local = vdf(&userdata.join("config/localconfig.vdf"))?;
    let app_flag = &shared["userroamingconfigstore"]["software"]["valve"]["steam"]["apps"]["240"]["cloudenabled"];
    if !app_flag.is_null() && app_flag != "1" {
        return Err(invalid("Existing App240 Cloud preference must be enabled"));
    }
    if local["userlocalconfigstore"]["software"]["valve"]["steam"]["apps"]["240"]["cloud"]["last_sync_state"]
        != "synchronized"
    {
        return Err(invalid("App240 Steam Cloud must already be synchronized"));
    }
    let mut entries = BTreeMap::new();
    for (name, value) in manifest {
        if let Some(object) = value.as_object() {
            entries.insert(
                name.clone(),
                object
                    .iter()
                    .filter(|(key, _)| !matches!(key.as_str(), "localtime" | "time"))
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect::<BTreeMap<_, _>>(),
            );
        }
    }
    Ok(
        json!({"app_cloud_enabled":true,"sync":"synchronized","remote_cache":{"change_number":manifest.get("changenumber"),"entries":entries}}),
    )
}

fn preference_files(
    root: &Path,
    identity: &Identity,
) -> io::Result<BTreeMap<String, Option<String>>> {
    let userdata = identity.steam_root.join("userdata").join(&identity.account);
    let mut paths = vec![
        root.join("cstrike/cfg/config.cfg"),
        root.join("cstrike/videoconfig_linux.cfg"),
        userdata.join("240/remote/cfg/config.cfg"),
    ];
    let sprays = userdata.join("240/remote/materials/vgui/logos");
    match fs::read_dir(sprays) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry?;
                if entry.file_name().to_string_lossy().starts_with("spray.") {
                    paths.push(entry.path());
                }
                if paths.len() > 128 {
                    return Err(invalid("Steam spray file budget exceeded"));
                }
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    paths
        .into_iter()
        .map(|path| {
            let hash = match util::hash_file(&path, MAX_CONFIG) {
                Ok(h) => Some(h),
                Err(e) if e.kind() == io::ErrorKind::NotFound => None,
                Err(e) => return Err(e),
            };
            Ok((path.to_string_lossy().into_owned(), hash))
        })
        .collect()
}

pub(crate) fn reject_writes(source: &str, imported: bool) -> io::Result<()> {
    if source.len() > 1024 * 1024 {
        return Err(invalid("Oversized private configuration"));
    }
    let writes = Regex::new(
        r"(?i)\b(?:host_writeconfig|writeconfig|cl_cloud_settings|SetCloudEnabledForApp)\b",
    )
    .map_err(io::Error::other)?;
    let chains = Regex::new(r#"(?im)(?:^|[;\r\n])\s*"?(?:exec|alias|stuffcmds)"?(?:\s|;|$)"#)
        .map_err(io::Error::other)?;
    if writes.is_match(source) || imported && chains.is_match(source) {
        return Err(invalid(
            "Configuration may write preferences, alter Cloud or execute an imported command chain",
        ));
    }
    Ok(())
}

pub(crate) fn finish_preflight(directory: &Path) -> io::Result<()> {
    let path = directory.join("preflight-cloud-before.json");
    if !path.exists() {
        return Ok(());
    }
    let before: Value = util::json_read(&path, 4096)?;
    let root = before["steam_root"]
        .as_str()
        .ok_or_else(|| invalid("Preflight Steam root missing"))?;
    let cursor = before["cursor"]
        .as_u64()
        .ok_or_else(|| invalid("Preflight Cloud cursor missing"))?;
    let started = Instant::now();
    let mut stable = Instant::now();
    let mut previous_length = None;
    let mut uploads = BTreeSet::new();
    let mut errors = BTreeSet::new();
    loop {
        match upload_lines(Path::new(root), cursor) {
            Ok(lines) => uploads.extend(lines),
            Err(error) => {
                errors.insert(error.to_string());
            }
        }
        let length =
            fs::metadata(Path::new(root).join("logs/cloud_log.txt")).map(|metadata| metadata.len());
        match length {
            Ok(length) => {
                if previous_length != Some(length) {
                    stable = Instant::now();
                    previous_length = Some(length);
                }
            }
            Err(error) => {
                errors.insert(error.to_string());
                stable = Instant::now();
            }
        }
        if started.elapsed() >= Duration::from_secs(6) && stable.elapsed() >= Duration::from_secs(3)
        {
            break;
        }
        if started.elapsed() >= Duration::from_secs(12) {
            errors.insert("Cloud log did not settle after failed identity preflight".into());
            break;
        }
        thread::sleep(Duration::from_millis(500));
    }
    util::json_write(
        &directory.join("private-preservation-preflight.json"),
        &json!({
            "success":false,"configuration_baseline_available":false,
            "cloud_log_observation_passed":uploads.is_empty()&&errors.is_empty(),
            "app240_cloud_uploads":uploads,"errors":errors,
            "observation_seconds":started.elapsed().as_secs_f64(),"settled_seconds":stable.elapsed().as_secs_f64(),
        }),
    )?;
    Err(invalid(
        "Preference preservation is unavailable because identity preflight did not establish a configuration baseline; inspect private-preservation-preflight.json",
    ))
}

pub(crate) fn begin(
    environment: &CaptureEnvironment,
    helper: &Path,
    directory: &Path,
) -> io::Result<Baseline> {
    let earliest_cursor = fs::metadata(environment.steam_root.join("logs/cloud_log.txt"))?.len();
    util::json_write(
        &directory.join("preflight-cloud-before.json"),
        &json!({"steam_root":environment.steam_root,"cursor":earliest_cursor}),
    )?;
    let identity = query_identity(environment, helper, directory);
    let preflight = upload_lines(&environment.steam_root, earliest_cursor);
    util::json_write(
        &directory.join("preflight-cloud.json"),
        &json!({"cloud_log_cursor":earliest_cursor,"uploads":preflight.as_ref().ok(),"error":preflight.as_ref().err().map(ToString::to_string)}),
    )?;
    if !preflight?.is_empty() {
        return Err(invalid(
            "App240 Cloud uploaded during the read-only identity preflight",
        ));
    }
    let identity = identity?;
    let remote = identity
        .steam_root
        .join("userdata")
        .join(&identity.account)
        .join("240/remote/cfg/config.cfg");
    reject_writes(&util::text(&remote, 1024 * 1024)?, true)?;
    let baseline = Baseline {
        root: environment.engine_root.canonicalize()?,
        steam: identity.steam_root.clone(),
        files: preference_files(&environment.engine_root, &identity)?,
        cloud: cloud_state(&identity)?,
        cloud_log_size: earliest_cursor,
        identity,
    };
    util::json_write(
        &directory.join("private-preservation-before.json"),
        &baseline,
    )?;
    Ok(baseline)
}

pub(crate) fn finish(baseline: &Baseline, directory: &Path, observer: &str) -> io::Result<()> {
    let previous = directory.join("private-preservation.json");
    let previous_pass = if previous.exists() {
        util::json_read::<Value>(&previous, MAX_CONFIG)
            .ok()
            .is_some_and(|v| v["success"] == true)
    } else {
        true
    };
    let started = Instant::now();
    let mut stable = Instant::now();
    let mut signature = None;
    let mut changed = BTreeSet::new();
    let mut cloud_changed = false;
    let mut errors = BTreeSet::new();
    let (after_files, after_cloud, uploads) = loop {
        let after_files = match preference_files(&baseline.root, &baseline.identity) {
            Ok(files) => {
                for key in baseline.files.keys().chain(files.keys()) {
                    if baseline.files.get(key) != files.get(key) {
                        changed.insert(key.clone());
                    }
                }
                files
            }
            Err(e) => {
                errors.insert(e.to_string());
                BTreeMap::new()
            }
        };
        let cloud = cloud_state(&baseline.identity);
        let busy = cloud
            .as_ref()
            .is_err_and(|e| e.to_string() == "App240 Steam Cloud must already be synchronized");
        let after_cloud = cloud.unwrap_or_else(|e| json!({"state_error":e.to_string()}));
        if !busy && after_cloud != baseline.cloud {
            cloud_changed = true;
        }
        let log_path = baseline.steam.join("logs/cloud_log.txt");
        let appended = (|| {
            let mut file = fs::File::open(&log_path)?;
            if file.metadata()?.len() < baseline.cloud_log_size {
                return Err(invalid("Steam Cloud log rotated during capture"));
            }
            file.seek(SeekFrom::Start(baseline.cloud_log_size))?;
            let mut bytes = Vec::new();
            file.take(8 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
            if bytes.len() > 8 * 1024 * 1024 {
                return Err(invalid("Steam Cloud log exceeds audit budget"));
            }
            Ok(String::from_utf8_lossy(&bytes).into_owned())
        })();
        let log = match appended {
            Ok(log) => log,
            Err(e) => {
                errors.insert(e.to_string());
                String::new()
            }
        };
        let lines: Vec<&str> = log
            .lines()
            .filter(|line| line.contains("[AppID 240]"))
            .collect();
        let uploads: Vec<String> = lines
            .iter()
            .filter(|line| {
                [
                    "Need to upload file",
                    "HTTP upload",
                    "Upload batch initiated",
                    "Upload OK",
                ]
                .iter()
                .any(|term| line.contains(term))
            })
            .map(|line| line.to_string())
            .collect();
        let next = json!([after_files, after_cloud, lines]);
        if signature.as_ref() != Some(&next) || busy {
            stable = Instant::now();
            signature = Some(next);
        }
        if started.elapsed() >= Duration::from_secs(6)
            && stable.elapsed() >= Duration::from_secs(3)
            && !busy
        {
            break (after_files, after_cloud, uploads);
        }
        if started.elapsed() >= Duration::from_secs(12) {
            errors.insert("Steam Cloud state did not settle after native exit".into());
            break (after_files, after_cloud, uploads);
        }
        thread::sleep(Duration::from_millis(500));
    };
    let success = previous_pass
        && changed.is_empty()
        && !cloud_changed
        && after_cloud == baseline.cloud
        && uploads.is_empty()
        && errors.is_empty();
    let report = json!({"success":success,"observer":observer,"before":baseline,"after":{"files":after_files,"cloud":after_cloud},"changed_preference_files":changed,"cloud_preferences_unchanged":!cloud_changed,"app240_cloud_uploads":uploads,"errors":errors,"observation_seconds":started.elapsed().as_secs_f64(),"settled_seconds":stable.elapsed().as_secs_f64(),"previous_audit_success":previous_pass});
    util::json_write(
        &directory.join(format!("private-preservation-{observer}.json")),
        &report,
    )?;
    util::json_write(&previous, &report)?;
    if success {
        Ok(())
    } else {
        Err(invalid(format!(
            "Native preservation audit failed; inspect {}",
            directory.display()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn valid_cached_clone_cannot_substitute_for_live_root() {
        let directory = tempfile::tempdir().unwrap();
        let library = directory.path().join("library");
        let engine = library.join("steamapps/common/Counter-Strike Source");
        fs::create_dir_all(&engine).unwrap();
        let actual = directory.path().join("actual");
        let clone = directory.path().join("clone");
        for root in [&actual, &clone] {
            for subdirectory in ["config", "logs", "userdata", "steamapps"] {
                fs::create_dir_all(root.join(subdirectory)).unwrap();
            }
            fs::write(root.join("config/loginusers.vdf"), "\"users\" {}").unwrap();
            fs::write(root.join("logs/cloud_log.txt"), "").unwrap();
            fs::write(
                root.join("steamapps/libraryfolders.vdf"),
                format!(
                    "\"libraryfolders\" {{ \"0\" {{ \"path\" \"{}\" }} }}",
                    library.display()
                ),
            )
            .unwrap();
            validate_steam_root(root, &engine.canonicalize().unwrap()).unwrap();
        }
        assert!(require_live_root(&clone, &actual).is_err());
        assert!(require_live_root(&actual, &actual).is_ok());
    }

    #[test]
    fn failed_identity_preflight_observes_delayed_uploads_without_claiming_a_config_baseline() {
        let directory = tempfile::tempdir().unwrap();
        let steam = directory.path().join("Steam");
        fs::create_dir_all(steam.join("logs")).unwrap();
        fs::write(steam.join("logs/cloud_log.txt"), "").unwrap();
        util::json_write(
            &directory.path().join("preflight-cloud-before.json"),
            &json!({"steam_root":steam,"cursor":0}),
        )
        .unwrap();
        let log = steam.join("logs/cloud_log.txt");
        let delayed = thread::spawn(move || {
            thread::sleep(Duration::from_millis(700));
            fs::write(log, "[AppID 240] Upload OK delayed configuration\n").unwrap();
        });
        let started = Instant::now();
        assert!(finish_preflight(directory.path()).is_err());
        delayed.join().unwrap();
        assert!(started.elapsed() >= Duration::from_secs(6));
        let report: Value = util::json_read(
            &directory.path().join("private-preservation-preflight.json"),
            MAX_CONFIG,
        )
        .unwrap();
        assert_eq!(report["configuration_baseline_available"], false);
        assert_eq!(report["cloud_log_observation_passed"], false);
        assert_eq!(report["app240_cloud_uploads"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn imported_commands_cannot_bypass_automatic_write_guard() {
        for source in [
            "host_writeconfig other",
            "toggle cl_cloud_settings 0",
            "\"exec\" other",
            "alias safe unsafe",
            "stuffcmds",
            "bind x jump; exec other",
        ] {
            assert!(reject_writes(source, true).is_err(), "{source}");
        }
        assert!(reject_writes("bind x +jump\nvolume 0.7\n", true).is_ok());
    }
}
