use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use source_formats::bsp::{Bsp, PakArchive, PakLimits, lump};
use source_formats::keyvalues::{FlatBlockLimits, parse_flat_blocks_bounded};

use crate::util::{self, invalid};
use crate::{
    CleanupDisposition, MovementControlReceipt, MovementMode, MovementPhase, MovementReceipt,
    MovementSample, MovementTrainIdentity,
};

mod runtime;
pub(crate) use runtime::execute;

const SERVER_SHA: &str = "7887c8cc24a3dec6b8739bfebcad48c00b95423ebcbfc647f49fef1ab904dbe2";
const PREFIX: &str = "HAMMERR_MOVEMENT_";
const STATIONARY_TOLERANCE: f64 = 0.05;
const MOVEMENT_THRESHOLD: f64 = 0.1;

impl MovementPhase {
    fn name(self) -> &'static str {
        match self {
            Self::Stationary => "stationary",
            Self::Running => "running",
            Self::Stopped => "stopped",
            Self::Restarted => "restarted",
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct MovementProfile {
    pub map_name: String,
    pub source_sha256: String,
    pub bsp_sha256: String,
    pub staged_bsp_sha256: String,
    pub train: MovementTrainIdentity,
    pub mode: MovementMode,
    pub sample_interval_ms: u16,
    pub samples_per_phase: u16,
    pub startup_settle_ms: u16,
}

impl MovementProfile {
    fn phases(&self) -> &'static [MovementPhase] {
        match self.mode {
            MovementMode::Stationary => &[MovementPhase::Stationary],
            MovementMode::StopStartStopStart => &[
                MovementPhase::Stationary,
                MovementPhase::Running,
                MovementPhase::Stopped,
                MovementPhase::Restarted,
            ],
        }
    }

    fn input(&self, phase: MovementPhase) -> &'static str {
        match phase {
            MovementPhase::Stationary | MovementPhase::Stopped => "Stop",
            MovementPhase::Running | MovementPhase::Restarted => {
                if self.train.classname == "func_tracktrain" {
                    "StartForward"
                } else {
                    "Start"
                }
            }
        }
    }

    pub(crate) fn validate(&self) -> io::Result<()> {
        if self.startup_settle_ms != crate::MOVEMENT_STARTUP_SETTLE_MS {
            return Err(invalid("Unsupported native movement startup barrier"));
        }
        util::validate_name(&self.map_name)?;
        for hash in [
            &self.source_sha256,
            &self.bsp_sha256,
            &self.staged_bsp_sha256,
        ] {
            if hash.len() != 64 || !hash.bytes().all(|value| value.is_ascii_hexdigit()) {
                return Err(invalid(
                    "Movement observation requires complete SHA256 identities",
                ));
            }
        }
        if self.train.hammer_id <= 0
            || !matches!(
                self.train.classname.as_str(),
                "func_train" | "func_tracktrain"
            )
            || self.train.targetname.is_empty()
            || self.train.targetname.len() > 128
            || !self
                .train
                .targetname
                .bytes()
                .all(|value| value.is_ascii_alphanumeric() || b"_.-".contains(&value))
        {
            return Err(invalid(
                "Movement observation requires a positive Hammer ID and an exact named func_train or func_tracktrain; names admit ASCII letters, digits, underscore, period and hyphen",
            ));
        }
        let count = self.phases().len() * usize::from(self.samples_per_phase);
        let duration = count * usize::from(self.sample_interval_ms);
        if !(100..=1000).contains(&self.sample_interval_ms)
            || !(3..=64).contains(&self.samples_per_phase)
            || count > 256
            || duration > 60_000
        {
            return Err(invalid(
                "Movement observation requires 100–1000ms intervals, 3–64 samples per phase, at most 256 samples and at most 60 seconds of requested sampling",
            ));
        }
        Ok(())
    }
}

pub(crate) fn verify_server(root: &Path) -> io::Result<()> {
    if util::hash_file(&root.join("cstrike/bin/linux64/server.so"), util::MAX_BSP)? != SERVER_SHA {
        return Err(invalid(
            "Native movement server library changed; its VScript bindings require verification",
        ));
    }
    Ok(())
}

pub(crate) fn stage(bytes: &[u8], train: &MovementTrainIdentity) -> io::Result<Vec<u8>> {
    if bytes.len() > 96 * 1024 * 1024 {
        return Err(invalid(
            "Movement BSP exceeds the private staging budget of 96 MiB",
        ));
    }
    let mut bsp = Bsp::parse(bytes)?;
    let text = bsp.entities()?;
    let blocks = parse_flat_blocks_bounded(
        text,
        FlatBlockLimits {
            max_bytes: 4 * 1024 * 1024,
            max_blocks: 16_384,
            max_pairs: 131_072,
        },
    )
    .map_err(io::Error::other)?;
    crate::adapter::validate_private_entities(text)?;
    if blocks.iter().flatten().any(|(key, value)| {
        (key.eq_ignore_ascii_case("classname") && crate::adapter::dangerous_class(value))
            || (matches!(
                key.to_ascii_lowercase().as_str(),
                "vscripts" | "thinkfunction"
            ) && !value.trim().is_empty())
    }) {
        return Err(invalid(
            "Movement BSP contains authored scripts or console-command entities/inputs",
        ));
    }
    let named = blocks
        .iter()
        .filter(|block| {
            block.iter().any(|(key, value)| {
                key.eq_ignore_ascii_case("targetname")
                    && value.eq_ignore_ascii_case(&train.targetname)
            })
        })
        .collect::<Vec<_>>();
    if named.len() != 1 {
        return Err(invalid(
            "Movement BSP must contain exactly one requested train targetname",
        ));
    }
    if blocks
        .iter()
        .flatten()
        .filter(|(key, value)| {
            key.eq_ignore_ascii_case("hammerid")
                && value.parse::<i32>().ok() == Some(train.hammer_id)
        })
        .count()
        != 1
    {
        return Err(invalid(
            "Movement BSP must contain exactly one requested Hammer ID",
        ));
    }
    for (key, expected) in [
        ("targetname", train.targetname.clone()),
        ("classname", train.classname.clone()),
        ("hammerid", train.hammer_id.to_string()),
    ] {
        let values = named[0]
            .iter()
            .filter(|(field, _)| field.eq_ignore_ascii_case(key))
            .map(|(_, value)| value)
            .collect::<Vec<_>>();
        if values.len() != 1 || values[0] != &expected {
            return Err(invalid(format!(
                "Compiled movement train {key} differs from the requested identity"
            )));
        }
    }
    if named[0]
        .iter()
        .any(|(key, value)| key.eq_ignore_ascii_case("parentname") && !value.trim().is_empty())
    {
        return Err(invalid(
            "Movement observation currently requires an unparented train",
        ));
    }
    let pak = &bsp.lump(lump::PAKFILE).data;
    if !pak.is_empty() {
        let archive = PakArchive::parse(pak, PakLimits::default())?;
        if archive.entries.iter().any(|entry| {
            let name = entry.name.to_ascii_lowercase();
            !["materials/", "models/", "sound/", "particles/"]
                .iter()
                .any(|prefix| name.starts_with(prefix))
        }) {
            return Err(invalid(
                "Movement BSP PAK may contain only materials, models, sound and particles; private scripts and configuration cannot be overridden",
            ));
        }
    }
    let mut entities = text.to_owned();
    entities.push_str("\n{\n\"classname\" \"logic_script\"\n\"targetname\" \"__hammerr_movement_controller\"\n\"vscripts\" \"hammerr_movement.nut\"\n\"thinkfunction\" \"HammeRRMovementThink\"\n}\n");
    bsp.set_entities(&entities);
    let staged = bsp.to_bytes();
    if staged.len() > 96 * 1024 * 1024 {
        return Err(invalid("Staged movement BSP exceeds 96 MiB"));
    }
    Ok(staged)
}

pub(crate) fn configure_game(
    directory: &Path,
    game: &Path,
    profile: &MovementProfile,
) -> io::Result<()> {
    profile.validate()?;
    let script = observer(profile)?;
    crate::guard::reject_writes(&script, false)?;
    util::atomic_write(
        &game.join("scripts/vscripts/hammerr_movement.nut"),
        script.as_bytes(),
    )?;
    util::atomic_write(
        &game.join("scripts/vscripts/mapspawn.nut"),
        b"printl(\"HAMMERR_MOVEMENT_MAPSPAWN\");\n",
    )?;
    let controls = "toggle sv_lan 1\ntoggle sv_cheats 1\ntoggle bot_quota 0\ntoggle mp_timelimit 0\ntoggle mp_roundtime 60\nhelp sv_lan\n";
    for name in [
        "server.cfg",
        "listenserver.cfg",
        "hammerr_capture_postmap.cfg",
    ] {
        util::atomic_write(&game.join("cfg").join(name), controls.as_bytes())?;
    }
    let launch = format!(
        "toggle con_logfile hammerr-movement.log\ntoggle servercfgfile hammerr_capture_postmap.cfg\ntoggle fps_max 60\n{controls}echo HAMMERR_MOVEMENT_PREMAP\nmap {}\n",
        profile.map_name
    );
    crate::guard::reject_writes(&launch, false)?;
    util::atomic_write(
        &game.join("cfg/hammerr_private_launch.cfg"),
        launch.as_bytes(),
    )?;
    util::atomic_write(&game.join("cfg/valve.rc"), b"stuffcmds\n")?;
    util::atomic_write(
        &game.join("motd.txt"),
        b"HammeRR private movement observation\n",
    )?;
    fs::copy(
        directory.join("staged.bsp"),
        game.join("maps").join(format!("{}.bsp", profile.map_name)),
    )?;
    Ok(())
}

fn observer(profile: &MovementProfile) -> io::Result<String> {
    let names = profile
        .phases()
        .iter()
        .map(|phase| format!("\"{}\"", phase.name()))
        .collect::<Vec<_>>()
        .join(",");
    let inputs = profile
        .phases()
        .iter()
        .map(|phase| format!("\"{}\"", profile.input(*phase)))
        .collect::<Vec<_>>()
        .join(",");
    let prelude = format!(
        "expected_name <- \"{}\"; expected_class <- \"{}\"; phases <- [{}]; inputs <- [{}]; interval <- {:.6}; per_phase <- {}; startup_delay <- {:.6};\n",
        profile.train.targetname,
        profile.train.classname,
        names,
        inputs,
        f64::from(profile.sample_interval_ms) / 1000.0,
        profile.samples_per_phase,
        f64::from(profile.startup_settle_ms) / 1000.0
    );
    Ok(prelude
        + r#"
train <- null; entity_index <- -1; phase <- 0; count <- 0; sequence <- 0; next_at <- 0.0; initialized <- false; finished <- false; start_at <- null;
function HammeRRMovementTrain() {
    local found = Entities.FindByName(null, expected_name);
    if (found == null) throw "requested train missing";
    if (Entities.FindByName(found, expected_name) != null) throw "requested train name is ambiguous";
    if (found.GetName() != expected_name || found.GetClassname() != expected_class) throw "requested train identity mismatch";
    if (initialized && (found != train || found.GetEntityIndex() != entity_index)) throw "requested train instance changed";
    return found;
}
function HammeRRMovementControl() {
    train = HammeRRMovementTrain();
    EntFireByHandle(train, inputs[phase], "", 0.0, null, null);
    printl(format("HAMMERR_MOVEMENT_CONTROL %d %s %s %.9f", phase, phases[phase], inputs[phase], Time()));
    next_at = Time() + 0.5;
}
function HammeRRMovementThink() {
    if (finished) return -1;
    try {
        if (!initialized) {
            if (start_at == null) start_at = Time() + startup_delay;
            if (Time() < start_at) return 0.05;
            train = HammeRRMovementTrain();
            entity_index = train.GetEntityIndex();
            if (entity_index <= 0) throw "train entity index invalid";
            initialized = true;
            printl(format("HAMMERR_MOVEMENT_IDENTITY %s %s %d", train.GetClassname(), train.GetName(), entity_index));
            HammeRRMovementControl();
        }
        if (Time() < next_at) return 0.05;
        train = HammeRRMovementTrain();
        local origin = train.GetOrigin();
        printl(format("HAMMERR_MOVEMENT_SAMPLE %d %s %.9f %.9f %.9f %.9f %d", sequence, phases[phase], Time(), origin.x, origin.y, origin.z, entity_index));
        sequence++; count++; next_at = Time() + interval;
        if (count == per_phase) {
            phase++; count = 0;
            if (phase == phases.len()) {
                finished = true;
                printl(format("HAMMERR_MOVEMENT_COMPLETE %d", sequence));
                return -1;
            }
            HammeRRMovementControl();
        }
        return 0.05;
    } catch (error) {
        finished = true;
        printl("HAMMERR_MOVEMENT_ERROR " + error);
        return -1;
    }
}
"#)
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Observation {
    pub samples: Vec<MovementSample>,
    pub controls: Vec<MovementControlReceipt>,
}

fn number(value: &str) -> io::Result<f64> {
    let value = value.parse::<f64>().map_err(io::Error::other)?;
    if !value.is_finite() {
        return Err(invalid("Native movement returned a nonfinite number"));
    }
    Ok(value)
}

pub(crate) fn parse(text: &str, profile: &MovementProfile) -> io::Result<Observation> {
    profile.validate()?;
    runtime::verify_log(text)?;
    if !text.ends_with('\n') {
        return Err(invalid(
            "Native movement console ended with a truncated line",
        ));
    }
    let mut identity = None;
    let mut samples = Vec::new();
    let mut controls = Vec::new();
    let mut complete = false;
    let mut mapspawn = false;
    let mut premap = false;
    for line in text.lines().map(str::trim) {
        let Some(record) = line.strip_prefix(PREFIX) else {
            continue;
        };
        let fields = record.split_whitespace().collect::<Vec<_>>();
        match fields.as_slice() {
            ["PREMAP"] if !premap && !mapspawn && identity.is_none() => premap = true,
            ["MAPSPAWN"] if premap && !mapspawn && identity.is_none() => mapspawn = true,
            ["IDENTITY", class, name, index] if mapspawn && identity.is_none() && !complete => {
                let index = index.parse::<i32>().map_err(io::Error::other)?;
                if *class != profile.train.classname
                    || *name != profile.train.targetname
                    || index <= 0
                {
                    return Err(invalid(
                        "Native movement entity identity differs from the requested train",
                    ));
                }
                identity = Some(index);
            }
            ["CONTROL", phase_index, phase_name, input, time]
                if identity.is_some() && !complete =>
            {
                let phase_index = phase_index.parse::<usize>().map_err(io::Error::other)?;
                let expected = profile
                    .phases()
                    .get(controls.len())
                    .copied()
                    .ok_or_else(|| invalid("Native movement returned extra controls"))?;
                let game_time = number(time)?;
                if phase_index != controls.len()
                    || *phase_name != expected.name()
                    || *input != profile.input(expected)
                    || samples.len() != controls.len() * usize::from(profile.samples_per_phase)
                    || samples
                        .last()
                        .is_some_and(|sample: &MovementSample| game_time < sample.game_time)
                    || game_time < 0.0
                {
                    return Err(invalid(
                        "Native movement controls are incomplete or out of order",
                    ));
                }
                controls.push(MovementControlReceipt {
                    phase: expected,
                    input: (*input).to_owned(),
                    game_time,
                });
            }
            ["SAMPLE", sequence, phase_name, time, x, y, z, index]
                if identity.is_some() && !complete =>
            {
                let sequence = sequence.parse::<usize>().map_err(io::Error::other)?;
                let index = index.parse::<i32>().map_err(io::Error::other)?;
                let expected = profile
                    .phases()
                    .get(samples.len() / usize::from(profile.samples_per_phase))
                    .copied()
                    .ok_or_else(|| invalid("Native movement returned extra samples"))?;
                let game_time = number(time)?;
                let control = controls
                    .last()
                    .ok_or_else(|| invalid("Native movement sample preceded its control"))?;
                if sequence != samples.len()
                    || *phase_name != expected.name()
                    || Some(index) != identity
                    || control.phase != expected
                    || game_time < control.game_time + 0.49
                    || samples.last().is_some_and(|sample| {
                        game_time <= sample.game_time
                            || (sample.phase == expected
                                && game_time - sample.game_time
                                    < f64::from(profile.sample_interval_ms) / 1000.0 - 0.001)
                    })
                {
                    return Err(invalid(
                        "Native movement samples changed identity, repeated time or arrived out of order",
                    ));
                }
                samples.push(MovementSample {
                    sequence,
                    phase: expected,
                    game_time,
                    origin: [number(x)?, number(y)?, number(z)?],
                    entity_index: index,
                });
            }
            ["COMPLETE", count] if identity.is_some() && !complete => {
                let expected = profile.phases().len() * usize::from(profile.samples_per_phase);
                if count.parse::<usize>().map_err(io::Error::other)? != expected
                    || samples.len() != expected
                    || controls.len() != profile.phases().len()
                {
                    return Err(invalid(
                        "Native movement completed with a truncated sample/control stream",
                    ));
                }
                complete = true;
            }
            _ => {
                return Err(invalid(format!(
                    "Malformed, repeated or failed native movement record: {line}"
                )));
            }
        }
    }
    if !complete {
        return Err(invalid("Native movement sample stream did not complete"));
    }
    for phase in profile.phases() {
        let phase_samples = samples
            .iter()
            .filter(|sample| sample.phase == *phase)
            .collect::<Vec<_>>();
        let first = phase_samples
            .first()
            .ok_or_else(|| invalid("Native movement phase has no samples"))?;
        let displacement = phase_samples
            .iter()
            .map(|sample| {
                sample
                    .origin
                    .iter()
                    .zip(first.origin)
                    .map(|(a, b)| (a - b).powi(2))
                    .sum::<f64>()
                    .sqrt()
            })
            .fold(0.0, f64::max);
        if !displacement.is_finite() {
            return Err(invalid("Native movement displacement is nonfinite"));
        }
        match phase {
            MovementPhase::Stationary | MovementPhase::Stopped
                if displacement > STATIONARY_TOLERANCE =>
            {
                return Err(invalid(format!(
                    "Native {} phase moved {displacement} units after Stop",
                    phase.name()
                )));
            }
            MovementPhase::Running | MovementPhase::Restarted
                if displacement < MOVEMENT_THRESHOLD =>
            {
                return Err(invalid(format!(
                    "Native {} phase did not prove movement after {}",
                    phase.name(),
                    profile.input(*phase)
                )));
            }
            _ => {}
        }
    }
    Ok(Observation { samples, controls })
}

pub(crate) fn receipt(
    profile: MovementProfile,
    observation: Observation,
    directory: PathBuf,
    elapsed_ms: u64,
    cleanup: CleanupDisposition,
) -> MovementReceipt {
    MovementReceipt {
        source_sha256: profile.source_sha256,
        bsp_sha256: profile.bsp_sha256,
        staged_bsp_sha256: profile.staged_bsp_sha256,
        train: profile.train,
        mode: profile.mode,
        samples: observation.samples,
        controls: observation.controls,
        artifacts: directory,
        elapsed_ms,
        startup_settle_ms: profile.startup_settle_ms,
        verified_controls: true,
        preservation_passed: true,
        cleanup,
        position_semantics: "Native GetOrigin absolute entity origin; game_time is native Time server time. Brush origins include the model's center offset and must not be compared directly with path-node origins.".to_owned(),
        stationary_tolerance: STATIONARY_TOLERANCE,
        movement_threshold: MOVEMENT_THRESHOLD,
    }
}
