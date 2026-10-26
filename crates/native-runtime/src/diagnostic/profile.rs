use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::json;
use source_formats::vpk::Vpk;

use crate::util::{self, invalid};
use crate::{CaptureEnvironment, ContentMount};

pub(crate) const MAP_NAME: &str = "hammerr_runtime";
pub(super) const PROBE: &str = "pinned-hardware-observer.so";
pub(super) const PROBE_SHA: &str =
    "c32b9a920a3191e1d6ebdf75411be55f05a33f7e6d7ee2461674f3f9b984b84c";
pub(super) const HARDWARE_VERSION: &str = "MaterialSystemHardwareConfig012";
pub(super) const MAX_INPUT: usize = 32 * 1024 * 1024;
pub(super) const MAX_HARDWARE: usize = 64 * 1024;
pub(super) const SAMPLE_COUNT: usize = 8;
pub(super) const READY: &str = "HAMMERR_HDR_READY 2 100";
pub(super) const PREMAP: &str = "HAMMERR_PRIVATE_PREMAP_READY";
pub(super) const COMPLETE: &str = "HAMMERR_DIAGNOSTIC_CAPTURE_COMPLETE";
pub(super) const DISCONNECTED: &str = "HAMMERR_DIAGNOSTIC_DISCONNECTED";
pub(super) const TIMEOUT: Duration = Duration::from_secs(65);
const OPTIONAL_LOCALIZATION_VPK: &str = "cstrike/cstrike_english.vpk";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PplUv857Case {
    MissingUv,
    #[serde(rename = "uniform-R")]
    UniformR,
    #[serde(rename = "uniform-B")]
    UniformB,
    #[serde(rename = "uniform-G")]
    UniformG,
    #[serde(rename = "uniform-W")]
    UniformW,
    #[serde(rename = "quadrant-R")]
    QuadrantR,
    #[serde(rename = "quadrant-B")]
    QuadrantB,
    #[serde(rename = "quadrant-G")]
    QuadrantG,
    #[serde(rename = "quadrant-W")]
    QuadrantW,
}

impl PplUv857Case {
    pub(super) fn bsp_sha(self) -> &'static str {
        match self {
            Self::MissingUv => "48a34c8fae7f244ebf3faa38c9c87de36ba1c9bb4235131762c908678446d1ef",
            Self::UniformR => "a855ab6862ef08ee5292dcb82bc86c1e9dd28072f41b7aadb68e808d7450c6a9",
            Self::UniformB => "ba50ce4d2f564d9b7190babcadf4d7498e19e610551f330f38574e57e198dc08",
            Self::UniformG => "f0e0d87d038f24382a792bfc3a8a5dfc7feb8b120ffc0897c8b9708fd287f8f5",
            Self::UniformW => "30205d7b5cd624dddddbcc662c72e6f65dfb6ec9a12c06e65a07defa1534d667",
            Self::QuadrantR => "2013a4edeed9b65501f1a9fd8cbc5cf151c097c128988a7b9864cf22ff93e465",
            Self::QuadrantB => "b58196386b107d5835d6e1e9ddfe295551f2f205761583a85dec5549cf3e6441",
            Self::QuadrantG => "921a62e51ec0a9d134d021f0bfb78c2342bd6c9f8eb26d242007b7f60f4d4422",
            Self::QuadrantW => "f632066522b338725a76d5d1193e98554ab32298eab4788b873e65da15fa12a5",
        }
    }
}

pub struct PplUv857Request {
    pub case: PplUv857Case,
    pub environment: CaptureEnvironment,
    pub bsp: PathBuf,
    pub fixture_game: PathBuf,
    pub observer_library: PathBuf,
    pub artifacts_parent: PathBuf,
}

#[derive(Debug, Serialize)]
pub struct PplUv857Receipt {
    pub case: PplUv857Case,
    pub artifacts: PathBuf,
    pub elapsed_ms: u64,
    pub verified_controls: bool,
    pub hardware_observed: bool,
    pub preservation_passed: bool,
}

pub(crate) fn path_text<'a>(path: &'a Path, label: &str) -> io::Result<&'a str> {
    path.to_str()
        .ok_or_else(|| invalid(format!("Frozen857 {label} path is not valid UTF-8")))
}

pub(crate) fn validate_request_paths(request: &PplUv857Request) -> io::Result<()> {
    for (label, path) in [
        ("BSP", request.bsp.as_path()),
        ("fixture", request.fixture_game.as_path()),
        ("observer", request.observer_library.as_path()),
        ("artifact parent", request.artifacts_parent.as_path()),
        ("engine root", request.environment.engine_root.as_path()),
        ("Steam root", request.environment.steam_root.as_path()),
    ] {
        path_text(path, label)?;
    }
    for mount in &request.environment.mounts {
        path_text(&mount.path, "content mount")?;
    }
    Ok(())
}

pub(super) const FIXTURE_FILES: &[(&str, &str)] = &[
    (
        "models/hammerr/invalid-root.dx90.vtx",
        "42879ae821ab5c95a482c1deca5451dc53c8875009c46dafc7195676c6c4734d",
    ),
    (
        "models/hammerr/invalid-root.mdl",
        "8de4857aca332b237c3f3d9692abbfd247884cece012df738e7c238df930d857",
    ),
    (
        "models/hammerr/invalid-root.vvd",
        "3fa20fc03c7cd6f754018f69c486ff5390344ee8af6684f0a2395be85d720849",
    ),
    (
        "models/props_junk/wood_crate001a.dx90.vtx",
        "42879ae821ab5c95a482c1deca5451dc53c8875009c46dafc7195676c6c4734d",
    ),
    (
        "models/props_junk/wood_crate001a.mdl",
        "8fc43694049559d0512805630ff0ac7d6bb77fe819c98deeec2e02c364648126",
    ),
    (
        "models/props_junk/wood_crate001a.phy",
        "797b9a2c522b0d16b5cee1a0ad60bfbd5b01aa88f44353c0fff87d7432382436",
    ),
    (
        "models/props_junk/wood_crate001a.vvd",
        "3fa20fc03c7cd6f754018f69c486ff5390344ee8af6684f0a2395be85d720849",
    ),
    (
        "materials/hammerr/wall.vmt",
        "51e8a36239fcde5b1e409f440a88d1096e301badb3238d2a575e6bedda05ff1a",
    ),
    (
        "materials/hammerr/white.vtf",
        "ec4f387079c6885de9481a68f40b0b605bdcde17c4a766796977cef59efc1d53",
    ),
    (
        "materials/hammerr_prop_vertex/wall.vmt",
        "7963a59ef6a17f910d8d16103dffde3f20acbde1b4489c644b91c748d8aabdaf",
    ),
    (
        "materials/hammerr_prop_vertex/wall.vtf",
        "82fbf08073d8267eac6bc267e32a9c7b12896942a5f330ba06cf0659938d468b",
    ),
    (
        "materials/models/props_junk/woodcrates01a.vmt",
        "35ec2dd6afa6b17c8aaf61fd2064daa9cf97191b178b6dd2791e14b34f5b8dec",
    ),
];

pub(super) const INSTALLED_FILES: &[(&str, &str)] = &[
    (
        "cstrike_linux64",
        "3f5161f5bc5847902a8845eba546eca323641c645d60e1828a187299e57014d0",
    ),
    ("bin/linux64/engine.so", util::ENGINE_SHA),
    (
        "bin/linux64/materialsystem.so",
        "2ac4db05ea8ccc0ce63627a37c6a0551b2f9d21faf0cfd6c7128bf3f39622928",
    ),
    (
        "bin/linux64/datacache.so",
        "2b35615f04969fb53c7fdca9a625b38fb8a83087402966323dcc02376a9d489c",
    ),
    (
        "bin/linux64/studiorender.so",
        "5dfbc55e86c113738fb240f4ac860727605d59797a10a214c0b9e7d679a31117",
    ),
    (
        "bin/linux64/stdshader_dx9.so",
        "34c1afbc36844d8858a664bc99efb59feb6ae2ebafc7ead82f637ecd5c2d6421",
    ),
    (
        "bin/linux64/shaderapidx9.so",
        "59f2edf37d1f9afbc09eae7f2077ac21db09f2aecb6e36e948a0845a2f01cd8d",
    ),
    (
        "bin/linux64/shaderapivk.so",
        "f3b67d91c20679e56a75ddb6bab3b397ff46e57b01cf6e1a46c0e0b906fa5adf",
    ),
    ("bin/linux64/libsteam_api.so", util::STEAM_SHA),
    (
        "bin/linux64/libdxvk_d3d9.so",
        "423cb685e766bc795d3039e0b8d5e887602c19d19bab29a70866e61717e9f0f0",
    ),
];

pub(super) fn pinned_bytes(path: &Path, expected: &str, limit: usize) -> io::Result<Vec<u8>> {
    if !fs::symlink_metadata(path)?.is_file() {
        return Err(invalid(format!(
            "Pinned diagnostic input is not a regular file: {}",
            path.display()
        )));
    }
    let bytes = util::read(path, limit)?;
    if util::hash(&bytes) != expected {
        return Err(invalid(format!(
            "Frozen857 input changed: {}",
            path.display()
        )));
    }
    Ok(bytes)
}

pub(crate) fn environment(environment: &CaptureEnvironment) -> io::Result<CaptureEnvironment> {
    let mut environment = crate::guard::resolve_environment(environment)?;
    let root = &environment.engine_root;
    environment.mounts = content_mounts(root)?;
    crate::guard::validate_environment(&environment)?;
    verify_installed(root)?;
    Ok(environment)
}

fn content_mount_error(ids: &str, declared: &Path, physical: &Path, error: io::Error) -> io::Error {
    io::Error::new(
        error.kind(),
        format!(
            "Frozen857 content mount [{ids}] {} (physical {}): {error}",
            declared.display(),
            physical.display()
        ),
    )
}

fn content_mounts(root: &Path) -> io::Result<Vec<ContentMount>> {
    let mut mounts = Vec::new();
    for (ids, relative) in [
        ("game+mod", OPTIONAL_LOCALIZATION_VPK),
        ("game+mod", "cstrike/cstrike_pak.vpk"),
        ("game", "hl2/hl2_textures.vpk"),
        ("game", "hl2/hl2_sound_vo_english.vpk"),
        ("game", "hl2/hl2_sound_misc.vpk"),
        ("game", "hl2/hl2_misc.vpk"),
        ("game", "cstrike"),
        ("game", "hl2"),
        ("platform", "platform/platform_misc.vpk"),
        ("platform", "platform"),
    ] {
        let declared = root.join(relative);
        let physical = Vpk::dir_file_for(&declared);
        if relative == OPTIONAL_LOCALIZATION_VPK {
            match fs::symlink_metadata(&physical) {
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(content_mount_error(ids, &declared, &physical, error)),
            }
        }
        let path = physical
            .canonicalize()
            .map_err(|error| content_mount_error(ids, &declared, &physical, error))?;
        mounts.push(ContentMount {
            ids: ids.split('+').map(str::to_owned).collect(),
            path,
        });
    }
    Ok(mounts)
}

pub(super) fn verify_installed(root: &Path) -> io::Result<()> {
    for &(name, expected) in INSTALLED_FILES {
        pinned_bytes(&root.join(name), expected, util::MAX_BSP)?;
    }
    Ok(())
}

pub(crate) fn stage(directory: &Path, request: &PplUv857Request) -> io::Result<()> {
    validate_request_paths(request)?;
    path_text(directory, "artifact directory")?;
    let bsp = pinned_bytes(&request.bsp, request.case.bsp_sha(), MAX_INPUT)?;
    util::atomic_write(&directory.join("staged.bsp"), &bsp)?;
    let probe = pinned_bytes(&request.observer_library, PROBE_SHA, MAX_INPUT)?;
    util::atomic_write(&directory.join(PROBE), &probe)?;
    for &(name, expected) in FIXTURE_FILES {
        let bytes = pinned_bytes(&request.fixture_game.join(name), expected, MAX_INPUT)?;
        let path = directory.join("fixture").join(name);
        fs::create_dir_all(
            path.parent()
                .ok_or_else(|| invalid("Fixture has no parent"))?,
        )?;
        util::atomic_write(&path, &bytes)?;
    }
    util::json_write(
        &directory.join("frozen-inputs.json"),
        &json!({
            "case": request.case, "bsp": request.bsp, "bsp_sha256": request.case.bsp_sha(),
            "fixture_game": request.fixture_game, "fixture_files": FIXTURE_FILES,
            "observer_library": request.observer_library, "observer_sha256": PROBE_SHA,
            "installed_files": INSTALLED_FILES,
        }),
    )
}

pub(crate) fn verify_sources(request: &PplUv857Request) -> io::Result<()> {
    pinned_bytes(&request.bsp, request.case.bsp_sha(), MAX_INPUT)?;
    pinned_bytes(&request.observer_library, PROBE_SHA, MAX_INPUT)?;
    for &(name, expected) in FIXTURE_FILES {
        pinned_bytes(&request.fixture_game.join(name), expected, MAX_INPUT)?;
    }
    Ok(())
}

pub(super) fn verify_staging(directory: &Path, case: PplUv857Case) -> io::Result<()> {
    pinned_bytes(&directory.join("staged.bsp"), case.bsp_sha(), MAX_INPUT)?;
    pinned_bytes(&directory.join(PROBE), PROBE_SHA, MAX_INPUT)?;
    for &(name, expected) in FIXTURE_FILES {
        pinned_bytes(&directory.join("fixture").join(name), expected, MAX_INPUT)?;
    }
    Ok(())
}

pub(super) fn verify_private_fixture(game: &Path) -> io::Result<()> {
    for &(name, expected) in FIXTURE_FILES {
        pinned_bytes(&game.join(name), expected, MAX_INPUT)?;
    }
    Ok(())
}

pub(crate) fn observer_arguments(
    command: &mut Command,
    directory: &Path,
    program: &Path,
) -> io::Result<()> {
    pinned_bytes(&directory.join(PROBE), PROBE_SHA, MAX_INPUT)?;
    if directory.join("probe-ready").exists() || directory.join("hardware.jsonl").exists() {
        return Err(invalid("Hardware observer requires fresh output paths"));
    }
    let root = program
        .parent()
        .ok_or_else(|| invalid("Native executable has no parent"))?;
    for (name, value) in [
        ("LD_PRELOAD", directory.join(PROBE)),
        ("HAMMERR_HDR_EXPECTED_EXE", program.to_owned()),
        (
            "HAMMERR_HDR_LIBRARY_DIR",
            root.join("bin/linux64").canonicalize()?,
        ),
        ("HAMMERR_HDR_PRIVATE_GAME", directory.join("cstrike")),
        ("HAMMERR_HDR_READY", directory.join("probe-ready")),
        ("HAMMERR_HDR_REPORT", directory.join("hardware.jsonl")),
    ] {
        command.args(["--setenv", name]).arg(value);
    }
    Ok(())
}

pub(super) const CONTROLS: &[(&str, f64)] = &[
    ("mat_force_tonemap_scale", 0.25),
    ("mat_autoexposure_min", 0.25),
    ("mat_autoexposure_max", 0.25),
    ("mat_disable_bloom", 1.0),
    ("mat_bloomscale", 0.0),
    ("mat_fullbright", 0.0),
    ("sv_lan", 1.0),
    ("r_proplightingfromdisk", 1.0),
    ("r_debugrandomstaticlighting", 0.0),
    ("r_proplightingpooling", 0.0),
    ("r_rootlod", 0.0),
    ("mat_queue_mode", 0.0),
    ("r_lod", 0.0),
    ("mat_hdr_level", 0.0),
];

pub(super) const OBSERVER: &str = r#"
stage <- 0;
ready_at <- 0.0;
function HammeRRThink() {
    local player = Entities.FindByClassname(null, "player");
    if (player == null) return 0.5;
    if (stage == 0) {
        SendToConsole("toggle sv_cheats 1; toggle sv_lan 1; toggle joystick 0; toggle bot_quota 0; bot_kick; jointeam 2; joinclass 1; hidepanel info; hidepanel team; hidepanel class_ter; hidepanel class_ct; toggle r_drawvgui 0; gameui_hide; toggle cl_drawhud 0; toggle r_drawviewmodel 0; toggle r_drawothermodels 0; toggle mat_fullbright 0; toggle mat_force_tonemap_scale 0.25; toggle mat_autoexposure_min 0.25; toggle mat_autoexposure_max 0.25; toggle mat_disable_bloom 1; toggle mat_bloomscale 0");
        ready_at = Time() + 6.0;
        stage = 1;
    }
    if (stage == 1 && Time() > ready_at && player.GetTeam() == 2 && player.GetHealth() == 100) {
        SendToConsole("noclip; setpos -192 -240 160; setang 25 50 0");
        ready_at = Time() + 3.0;
        stage = 2;
    }
    if (stage == 2 && Time() > ready_at) {
        SendToConsole("help r_proplightingfromdisk; help r_debugrandomstaticlighting; help r_proplightingpooling; help r_rootlod; help mat_queue_mode; help r_lod; help mat_hdr_level; help mat_force_tonemap_scale; help mat_autoexposure_min; help mat_autoexposure_max; help mat_disable_bloom; help mat_bloomscale; help mat_fullbright; help sv_lan; echo HAMMERR_HDR_READY " + player.GetTeam() + " " + player.GetHealth());
        stage = 3;
    }
    if (stage == 3) SendToConsole("exec hammerr_diagnostic_once.cfg");
    return 0.5;
}
"#;

pub(super) fn private_write(path: &Path, text: &str) -> io::Result<()> {
    crate::guard::reject_writes(text, false)?;
    util::atomic_write(path, text.as_bytes())
}

pub(crate) fn configure_game(directory: &Path, game: &Path) -> io::Result<()> {
    for &(name, expected) in FIXTURE_FILES {
        let bytes = pinned_bytes(&directory.join("fixture").join(name), expected, MAX_INPUT)?;
        let destination = game.join(name);
        fs::create_dir_all(
            destination
                .parent()
                .ok_or_else(|| invalid("Fixture has no parent"))?,
        )?;
        util::atomic_write(&destination, &bytes)?;
    }
    private_write(
        &game.join("scripts/vscripts/hammerr_observer.nut"),
        OBSERVER,
    )?;
    private_write(
        &game.join("scripts/vscripts/mapspawn.nut"),
        "printl(\"HAMMERR_MAPSPAWN\");\n",
    )?;
    private_write(
        &game.join("cfg/hammerr_diagnostic_once.cfg"),
        "echo HAMMERR_DIAGNOSTIC_WAITING\n",
    )?;
    let controls = CONTROLS
        .iter()
        .map(|(name, value)| format!("toggle {name} {value}\n"))
        .collect::<String>();
    let queries = CONTROLS
        .iter()
        .map(|(name, _)| format!("help {name}\n"))
        .collect::<String>();
    let postmap = format!("{controls}{queries}");
    for name in [
        "server.cfg",
        "listenserver.cfg",
        "hammerr_capture_postmap.cfg",
    ] {
        private_write(&game.join("cfg").join(name), &postmap)?;
    }
    let profile = format!(
        "toggle con_logfile hammerr-runtime.log\ntoggle sv_cheats 1\ntoggle fps_max 60\ntoggle servercfgfile hammerr_capture_postmap.cfg\n{controls}{queries}echo {PREMAP}\nmap {MAP_NAME}\n"
    );
    private_write(&game.join("cfg/hammerr_private_launch.cfg"), &profile)?;
    private_write(&game.join("cfg/valve.rc"), "stuffcmds\n")?;
    util::atomic_write(&game.join("motd.txt"), b"HammeRR private diagnostic\n")
}

pub(super) fn verify_controls(text: &str) -> io::Result<BTreeMap<String, f64>> {
    let mut controls = BTreeMap::new();
    for &(name, expected) in CONTROLS {
        let pattern = Regex::new(&format!(r#""{name}" = "([^"]+)""#)).map_err(io::Error::other)?;
        let actual = pattern
            .captures_iter(text)
            .last()
            .and_then(|capture| capture[1].parse::<f64>().ok());
        if actual != Some(expected) {
            return Err(invalid(format!(
                "Diagnostic did not prove {name}={expected}"
            )));
        }
        controls.insert(name.to_owned(), expected);
    }
    Ok(controls)
}

pub(super) fn verify_order(text: &str, finished: bool) -> io::Result<()> {
    let mut previous = None;
    let markers: &[&str] = if finished {
        &[PREMAP, "HAMMERR_MAPSPAWN", READY, COMPLETE, DISCONNECTED]
    } else {
        &[PREMAP, "HAMMERR_MAPSPAWN", READY]
    };
    for &marker in markers {
        if text.matches(marker).count() != 1 {
            return Err(invalid(format!("Diagnostic requires exactly one {marker}")));
        }
        let offset = text
            .find(marker)
            .ok_or_else(|| invalid("Missing diagnostic marker"))?;
        if previous.is_some_and(|previous| offset <= previous) {
            return Err(invalid("Diagnostic handshake is out of order"));
        }
        previous = Some(offset);
    }
    Ok(())
}

pub(super) fn verify_loads(text: &str) -> io::Result<()> {
    crate::worker::verify_native_diagnostics(text)?;
    if text.contains("Level unlit") {
        return Err(invalid("Native diagnostic level is unlit"));
    }
    let pattern = Regex::new(r"(?i)Error (?:Vertex|Index) File|Level unlit|(?:error|failed|unable|not found|couldn.t|can.t)[^\n]*(?:\.mdl|\.vvd|\.vtx|\.vhv|\.vmt|\.vtf|static prop|material)|(?:\.mdl|\.vvd|\.vtx|\.vhv)[^\n]*(?:error|failed|not found)").map_err(io::Error::other)?;
    const ALLOWED: &[&str] = &[
        "Error: Material \"___fillrate_0\" uses unknown shader \"FillRate\"",
        "Error: Material \"___debugnormalmap_1\" uses unknown shader \"DebugNormalMap\"",
        "Error: Material \"___debugdrawenvmapmask_2\" uses unknown shader \"DebugDrawEnvmapMask\"",
        "Error: Material \"___debugdepth_3\" uses unknown shader \"DebugDepth\"",
        "Error: Material \"___debugdepth_4\" uses unknown shader \"DebugDepth\"",
        "Error: Material \"debug/debugluxels\" uses unknown shader \"DebugLuxels\"",
        "Error: Material \"models/humans/female/pupil_r\" uses unknown shader \"Eyeball\"",
        "Error: Material \"models/humans/female/pupil_l\" uses unknown shader \"Eyeball\"",
        "##### CTexture::LoadTextureBitsFromFile couldn't find materials/maps/hammerr_runtime/cubemapdefault.vtf",
    ];
    for line in text.lines().map(str::trim) {
        if pattern.is_match(line) && !ALLOWED.contains(&line) {
            return Err(invalid(format!(
                "Native diagnostic asset/lighting error: {line}"
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const REQUIRED_MOUNTS: &[(&str, &str)] = &[
        ("game+mod", "cstrike/cstrike_pak_dir.vpk"),
        ("game", "hl2/hl2_textures_dir.vpk"),
        ("game", "hl2/hl2_sound_vo_english_dir.vpk"),
        ("game", "hl2/hl2_sound_misc_dir.vpk"),
        ("game", "hl2/hl2_misc_dir.vpk"),
        ("game", "cstrike"),
        ("game", "hl2"),
        ("platform", "platform/platform_misc_dir.vpk"),
        ("platform", "platform"),
    ];

    fn installed_content() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        for name in ["cstrike", "hl2", "platform"] {
            fs::create_dir(directory.path().join(name)).unwrap();
        }
        let mut index = 0x55aa_1234u32.to_le_bytes().to_vec();
        index.extend(1u32.to_le_bytes());
        index.extend(1u32.to_le_bytes());
        index.push(0);
        for &(_, name) in REQUIRED_MOUNTS {
            if name.ends_with(".vpk") {
                fs::write(directory.path().join(name), &index).unwrap();
            }
        }
        directory
    }

    #[test]
    fn fixed_mounts_resolve_physical_indexes_in_approved_order() {
        let directory = installed_content();
        let root = directory.path();
        let mounts = content_mounts(root).unwrap();
        assert_eq!(mounts.len(), REQUIRED_MOUNTS.len());
        for (mount, &(ids, name)) in mounts.iter().zip(REQUIRED_MOUNTS) {
            assert_eq!(mount.ids.join("+"), ids);
            assert_eq!(mount.path, root.join(name).canonicalize().unwrap());
            if name.ends_with(".vpk") {
                let declared = root.join(name.replace("_dir.vpk", ".vpk"));
                assert!(!declared.exists());
                assert_eq!(Vpk::open(&mount.path).unwrap().path(), mount.path.as_path());
            }
        }
        let optional = root.join("cstrike/cstrike_english_dir.vpk");
        fs::copy(root.join("cstrike/cstrike_pak_dir.vpk"), &optional).unwrap();
        let localized = content_mounts(root).unwrap();
        assert_eq!(localized.len(), mounts.len() + 1);
        assert_eq!(localized[0].ids, ["game", "mod"]);
        assert_eq!(localized[0].path, optional.canonicalize().unwrap());
        for (before, after) in mounts.iter().zip(&localized[1..]) {
            assert_eq!(before.ids, after.ids);
            assert_eq!(before.path, after.path);
        }
    }

    #[test]
    fn absent_optional_localization_does_not_skip_required_indexes() {
        for (ids, name) in [
            ("game+mod", "cstrike/cstrike_pak_dir.vpk"),
            ("game", "hl2/hl2_sound_vo_english_dir.vpk"),
        ] {
            let directory = installed_content();
            let root = directory.path();
            assert_eq!(content_mounts(root).unwrap().len(), REQUIRED_MOUNTS.len());
            let missing = root.join(name);
            fs::remove_file(&missing).unwrap();
            let error = content_mounts(root).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::NotFound);
            let message = error.to_string();
            assert!(message.contains(&format!("[{ids}]")));
            assert!(message.contains(missing.to_str().unwrap()));
            assert!(message.contains(&name.replace("_dir.vpk", ".vpk")));
        }
    }

    #[test]
    fn optional_localization_broken_link_is_refused_with_context() {
        let directory = installed_content();
        let optional = directory.path().join("cstrike/cstrike_english_dir.vpk");
        std::os::unix::fs::symlink("missing_dir.vpk", &optional).unwrap();
        let error = content_mounts(directory.path()).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        let message = error.to_string();
        assert!(message.contains("[game+mod]"));
        assert!(message.contains(optional.to_str().unwrap()));
    }
}
