use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::CaptureEnvironment;
use crate::util::invalid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PplSpatial919Case {
    ChartA,
    ChartB,
}

impl PplSpatial919Case {
    pub(super) fn bsp_sha(self) -> &'static str {
        match self {
            Self::ChartA => "3d2d7a21581dfe4ae8b989b4c65d2ad1ca97bead82392b21f99efb1dd0158c76",
            Self::ChartB => "f9dff566c480057c016afa510a5ccc34048ca86ecd1bdfddf2c18a7cd9852610",
        }
    }
}

pub struct PplSpatial919Request {
    pub case: PplSpatial919Case,
    pub environment: CaptureEnvironment,
    pub bsp: PathBuf,
    pub fixture_game: PathBuf,
    pub observer_library: PathBuf,
    pub artifacts_parent: PathBuf,
}

#[derive(Debug, Serialize)]
pub struct PplSpatial919Receipt {
    pub case: PplSpatial919Case,
    pub artifacts: PathBuf,
    pub elapsed_ms: u64,
    pub verified_controls: bool,
    pub hardware_observed: bool,
    pub preservation_passed: bool,
}

pub(super) const CONTROLS: &[(&str, f64)] = &[
    ("r_aspectratio", 0.0),
    ("mat_picmip", 0.0),
    ("mat_filtertextures", 1.0),
    ("mat_trilinear", 1.0),
    ("mat_forceaniso", 1.0),
];
const ANGLE16_STEP_DEGREES: f64 = 360.0 / 65536.0;
const CAMERA_PRINT_HALF_STEP: f64 = 0.0000005;
const MAX_OBSERVATION: usize = 16 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Phase {
    Readiness,
    Capture,
    Final,
}

impl Phase {
    pub(super) const ALL: [Self; 3] = [Self::Readiness, Self::Capture, Self::Final];

    fn name(self) -> &'static str {
        match self {
            Self::Readiness => "READINESS",
            Self::Capture => "CAPTURE",
            Self::Final => "FINAL",
        }
    }

    fn marker(self, end: bool) -> String {
        format!(
            "HAMMERR_SPATIAL_{}_{}",
            self.name(),
            if end { "END" } else { "BEGIN" }
        )
    }

    pub(super) fn started(self, text: &str) -> bool {
        text.contains(self.marker(false).as_str()) || self.complete(text)
    }

    pub(super) fn complete(self, text: &str) -> bool {
        text.contains(self.marker(true).as_str())
    }

    pub(super) fn request(self) -> io::Result<&'static str> {
        match self {
            Self::Capture => {
                Ok("ent_fire hammerr_observer RunScriptCode \"HammeRRSpatialObserve(1)\"\n")
            }
            Self::Final => {
                Ok("ent_fire hammerr_observer RunScriptCode \"HammeRRSpatialObserve(2)\"\n")
            }
            Self::Readiness => Err(invalid(
                "Spatial readiness must be observed by the fixed observer",
            )),
        }
    }
}

pub(super) fn observer() -> io::Result<String> {
    let position = "noclip; setpos -192 -240 160; setang 25 50 0";
    let query = r#"help r_proplightingfromdisk; help r_debugrandomstaticlighting; help r_proplightingpooling; help r_rootlod; help mat_queue_mode; help r_lod; help mat_hdr_level; help mat_force_tonemap_scale; help mat_autoexposure_min; help mat_autoexposure_max; help mat_disable_bloom; help mat_bloomscale; help mat_fullbright; help sv_lan; echo HAMMERR_HDR_READY " + player.GetTeam() + " " + player.GetHealth());
        stage = 3;"#;
    let capture = r#"if (stage == 3) SendToConsole("exec hammerr_diagnostic_once.cfg");"#;
    if [position, query, capture]
        .iter()
        .any(|&anchor| super::profile::OBSERVER.matches(anchor).count() != 1)
    {
        return Err(invalid("Spatial diagnostic observer template changed"));
    }
    let queries = super::profile::CONTROLS
        .iter()
        .chain(CONTROLS)
        .map(|(name, _)| format!("help {name}; "))
        .collect::<String>();
    let functions = format!(
        r#"
spatial_phase <- 0;
spatial_pending <- false;
spatial_due <- 0.0;
function HammeRRSpatialObserve(phase) {{
    if (spatial_pending || phase <= spatial_phase) return;
    if (phase != spatial_phase + 1 || phase > 2) {{
        printl("HAMMERR_SPATIAL_PHASE_ERROR");
        return;
    }}
    spatial_phase = phase;
    spatial_pending = true;
    spatial_due = Time() + 1.0;
    local name = phase == 1 ? "CAPTURE" : "FINAL";
    SendToConsole("echo HAMMERR_SPATIAL_" + name + "_BEGIN; {queries}getpos; fov");
}}
"#
    );
    let script = super::profile::OBSERVER
        .replace(
            position,
            "noclip; fov 90; setpos -192 -240 160; setang 25 50 0",
        )
        .replace(
            query,
            &format!(
                r#"echo HAMMERR_SPATIAL_READINESS_BEGIN; {queries}getpos; fov");
        ready_at = Time() + 1.0;
        stage = 3;"#
            ),
        )
        .replace(
            capture,
            r#"if (stage == 3 && Time() > ready_at) {
        SendToConsole("echo HAMMERR_SPATIAL_READINESS_END; echo HAMMERR_HDR_READY " + player.GetTeam() + " " + player.GetHealth());
        stage = 4;
    }
    if (stage == 4) {
        if (spatial_pending && Time() > spatial_due) {
            local name = spatial_phase == 1 ? "CAPTURE" : "FINAL";
            SendToConsole("echo HAMMERR_SPATIAL_" + name + "_END");
            spatial_pending = false;
        }
        SendToConsole("exec hammerr_diagnostic_once.cfg");
    }"#,
        );
    Ok(format!("{functions}{script}"))
}

#[derive(Debug, Serialize)]
pub(super) struct Camera {
    pub eye_origin: [f64; 3],
    pub angles: [f64; 3],
    pub base_fov: u16,
}

#[derive(Debug, Serialize)]
pub(super) struct Observation {
    pub phase: Phase,
    pub controls: BTreeMap<String, f64>,
    pub camera: Camera,
}

fn bounds(text: &str, phase: Phase) -> io::Result<(usize, usize)> {
    let begin = phase.marker(false);
    let end = phase.marker(true);
    let begins = text.match_indices(begin.as_str()).collect::<Vec<_>>();
    let ends = text.match_indices(end.as_str()).collect::<Vec<_>>();
    if begins.len() != 1 || ends.len() != 1 {
        return Err(invalid(format!(
            "Spatial {:?} requires exactly one begin/end observation pair",
            phase
        )));
    }
    let first = begins[0].0 + begin.len();
    let last = ends[0].0;
    if last < first || last - first > MAX_OBSERVATION {
        return Err(invalid(
            "Spatial observation is unordered or exceeds its bound",
        ));
    }
    Ok((first, last))
}

pub(super) fn verify_phase(text: &str, phase: Phase) -> io::Result<Observation> {
    let (first, last) = bounds(text, phase)?;
    let readiness = bounds(text, Phase::Readiness)?;
    let ready = text
        .match_indices(super::profile::READY)
        .collect::<Vec<_>>();
    if ready.len() != 1 || readiness.1 >= ready[0].0 {
        return Err(invalid(
            "Spatial readiness observation must complete before READY",
        ));
    }
    if phase != Phase::Readiness {
        let capture = bounds(text, Phase::Capture)?;
        if ready[0].0 >= capture.0 {
            return Err(invalid("Spatial capture observation must follow readiness"));
        }
        if phase == Phase::Final && capture.1 >= first {
            return Err(invalid(
                "Spatial final observation must follow capture observation",
            ));
        }
    }
    let block = &text[first..last];
    let lower = block.to_ascii_lowercase();
    if lower.contains("unknown command")
        || lower.contains("unrecognized command")
        || text.contains("HAMMERR_SPATIAL_PHASE_ERROR")
    {
        return Err(invalid(
            "Spatial observation contains a command or phase error",
        ));
    }
    let mut controls = BTreeMap::new();
    for &(name, expected) in super::profile::CONTROLS.iter().chain(CONTROLS) {
        let pattern = Regex::new(&format!(r#""{name}" = "([^"]+)""#)).map_err(io::Error::other)?;
        let replies = pattern.captures_iter(block).collect::<Vec<_>>();
        if replies.len() != 1 {
            return Err(invalid(format!(
                "Spatial {:?} requires one actual response for {name}",
                phase
            )));
        }
        let actual = replies[0][1].parse::<f64>().ok();
        if actual != Some(expected) {
            return Err(invalid(format!(
                "Spatial {:?} control {name} is {actual:?}, expected {expected}",
                phase
            )));
        }
        controls.insert(name.to_owned(), expected);
    }
    Ok(Observation {
        phase,
        controls,
        camera: verify_camera(block)?,
    })
}

pub(super) fn verify_final(text: &str) -> io::Result<Vec<Observation>> {
    let observations = Phase::ALL
        .into_iter()
        .map(|phase| verify_phase(text, phase))
        .collect::<io::Result<Vec<_>>>()?;
    let final_end = bounds(text, Phase::Final)?.1;
    let complete = text
        .find(super::profile::COMPLETE)
        .ok_or_else(|| invalid("Missing spatial capture completion"))?;
    if final_end >= complete {
        return Err(invalid(
            "Spatial final observation must precede capture completion",
        ));
    }
    Ok(observations)
}

fn verify_camera(block: &str) -> io::Result<Camera> {
    let number = r"(-?\d+\.\d{6})";
    let view = Regex::new(&format!(
        r"(?m)^setpos {number} {number} {number};[ \t]*setang {number} {number} {number}\r?$"
    ))
    .map_err(io::Error::other)?;
    let captures = view.captures_iter(block).collect::<Vec<_>>();
    if captures.len() != 1 {
        return Err(invalid(
            "Spatial diagnostic main view is missing or ambiguous",
        ));
    }
    let mut actual = [0.0; 6];
    for (index, value) in actual.iter_mut().enumerate() {
        *value = captures[0][index + 1]
            .parse::<f64>()
            .map_err(|_| invalid("Invalid spatial diagnostic camera value"))?;
    }
    let expected = [
        -192.0,
        -240.0,
        224.0,
        4551.0 * ANGLE16_STEP_DEGREES,
        9102.0 * ANGLE16_STEP_DEGREES,
        0.0,
    ];
    if actual
        .iter()
        .zip(expected)
        .any(|(actual, expected)| (actual - expected).abs() > CAMERA_PRINT_HALF_STEP)
    {
        return Err(invalid(format!(
            "Spatial diagnostic main view changed: {actual:?}"
        )));
    }
    let fov = Regex::new(r#""fov" is "(\d+)""#).map_err(io::Error::other)?;
    let fovs = fov.captures_iter(block).collect::<Vec<_>>();
    if fovs.len() != 1 || &fovs[0][1] != "90" {
        return Err(invalid("Spatial diagnostic requires observed base FOV 90"));
    }
    Ok(Camera {
        eye_origin: [actual[0], actual[1], actual[2]],
        angles: [actual[3], actual[4], actual[5]],
        base_fov: 90,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn replies() -> String {
        let controls = super::super::profile::CONTROLS
            .iter()
            .chain(CONTROLS)
            .map(|(name, value)| format!("\"{name}\" = \"{value}\"\n"))
            .collect::<String>();
        format!(
            "{controls}setpos -192.000000 -240.000000 224.000000;setang 24.999390 49.998779 0.000000\n\"fov\" is \"90\"\n"
        )
    }

    fn observed(phase: Phase) -> String {
        format!(
            "{}\n{}{}\n",
            phase.marker(false),
            replies(),
            phase.marker(true)
        )
    }

    fn ready() -> String {
        format!(
            "{}{}\n",
            observed(Phase::Readiness),
            super::super::profile::READY
        )
    }

    #[test]
    fn spatial_controls_require_fresh_unambiguous_phase_observations() {
        let first = ready();
        assert_eq!(
            verify_phase(&first, Phase::Readiness)
                .unwrap()
                .controls
                .len(),
            19
        );
        let capture = format!("{first}{}", observed(Phase::Capture));
        let final_log = format!(
            "{capture}{}{}\n",
            observed(Phase::Final),
            super::super::profile::COMPLETE
        );
        verify_final(&final_log).unwrap();
        assert!(verify_phase("", Phase::Readiness).is_err());
        let response = "\"mat_trilinear\" = \"1\"\n";
        let block = observed(Phase::Readiness);
        for invalid in [
            format!(
                "{}{}{}\n",
                replies(),
                block.replace(response, ""),
                super::super::profile::READY
            ),
            format!(
                "{}{}{}\n",
                replies(),
                block.replace(response, "Unknown command: mat_trilinear\n"),
                super::super::profile::READY
            ),
            first.replace(response, &format!("\"mat_trilinear\" = \"0\"\n{response}")),
            first.replace(response, &format!("{response}{response}")),
            first.replace(
                Phase::Readiness.marker(true).as_str(),
                &format!(
                    "Unknown command: mat_trilinear\n{}",
                    Phase::Readiness.marker(true)
                ),
            ),
            format!(
                "{}{}{}\n",
                replies(),
                block.replace("\"sv_lan\" = \"1\"\n", ""),
                super::super::profile::READY
            ),
            first.replace("224.000000", "160.000000"),
            first.replace("24.999390", "25.000000"),
            first.replace("49.998779", "50.000000"),
            first.replace("24.999390", "24.999391"),
            first.replace("49.998779", "49.998780"),
            first.replace("24.999390", "25.004883"),
            first.replace("-192.000000", "-192.000001"),
            first.replace("24.999390", "24.99939"),
            first.replace("0.000000\n\"fov\"", "0.0000001\n\"fov\""),
            first.replace("\"fov\" is \"90\"", "\"fov\" is \"75\""),
            first.replace(Phase::Readiness.marker(true).as_str(), ""),
            format!("{first}{}\n", Phase::Readiness.marker(false)),
            format!("{}\n{}", super::super::profile::READY, block),
        ] {
            assert!(verify_phase(&invalid, Phase::Readiness).is_err());
        }
        for phase in [Phase::Capture, Phase::Final] {
            let complete = observed(phase);
            for invalid in [
                final_log.replace(complete.as_str(), &complete.replace(response, "")),
                final_log.replace(
                    complete.as_str(),
                    &complete.replace(response, "Unknown command: mat_trilinear\n"),
                ),
                final_log.replace(
                    complete.as_str(),
                    &complete.replace(response, &format!("{response}{response}")),
                ),
                final_log.replace(
                    complete.as_str(),
                    &complete.replace(response, &format!("\"mat_trilinear\" = \"0\"\n{response}")),
                ),
                final_log.replace(
                    complete.as_str(),
                    &complete.replace("\"fov\" is \"90\"\n", ""),
                ),
                final_log.replace(phase.marker(true).as_str(), ""),
                final_log.replace(complete.as_str(), ""),
            ] {
                assert!(verify_final(&invalid).is_err());
            }
        }
        assert!(verify_phase(&first, Phase::Capture).is_err());
        assert!(verify_final(&capture).is_err());
        let reversed = format!(
            "{first}{}{}{}\n",
            observed(Phase::Final),
            observed(Phase::Capture),
            super::super::profile::COMPLETE
        );
        assert!(verify_final(&reversed).is_err());
        let observer = observer().unwrap();
        assert_eq!(observer.matches("fov 90").count(), 1);
        assert!(observer.contains("function HammeRRSpatialObserve(phase)"));
        assert!(observer.contains("HAMMERR_SPATIAL_READINESS_END; echo HAMMERR_HDR_READY"));
        assert!(observer.contains("spatial_due = Time() + 1.0;"));
        for &(name, _) in super::super::profile::CONTROLS.iter().chain(CONTROLS) {
            assert!(observer.contains(format!("help {name};").as_str()));
        }
    }
}
