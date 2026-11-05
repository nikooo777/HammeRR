use glam::DVec3;

use super::{Issue, MAX_EXTENT, MAX_LINKS};

pub const MAX_PHYSICS_NODES: usize = 10;
pub const STEP_SECONDS: f64 = 0.02f32 as f64;
pub const INITIAL_HANG_SECONDS: f64 = 5.0;
pub const MAX_TIME_SECONDS: f64 = 60.0;
pub const MAX_BATCH_STEPS: usize = 50;
pub const MAX_SIMULATION_WORK: usize = 128_000_000;
pub const MAX_WIND: f64 = 1_000.0;
pub const MAX_GRAVITY: f64 = 3_000.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Environment {
    pub gravity: f64,
    pub wind: DVec3,
}

impl Default for Environment {
    fn default() -> Self {
        Self {
            gravity: 1500.0,
            wind: DVec3::ZERO,
        }
    }
}

impl Environment {
    pub fn validate(&self) -> Result<(), String> {
        if !self.gravity.is_finite() || !(0.0..=MAX_GRAVITY).contains(&self.gravity) {
            return Err(format!(
                "Preview gravity must be between 0 and {MAX_GRAVITY}"
            ));
        }
        if !self.wind.is_finite() || self.wind.abs().max_element() > MAX_WIND {
            return Err(format!(
                "Preview wind components must be finite within ±{MAX_WIND}"
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Endpoint {
    pub position: DVec3,
    pub forward: DVec3,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedEndpoints {
    pub start: Endpoint,
    pub end: Option<Endpoint>,
    pub rope_length: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpanSettings {
    pub nodes: usize,
    pub subdiv_authored: u8,
    pub subdiv: usize,
    pub slack: i32,
    pub width: f64,
    pub texture_scale: f64,
    pub material: String,
    pub scroll_speed: f64,
    pub dangling: bool,
    pub collide: bool,
    pub no_wind: bool,
    pub barbed: bool,
    pub breakable: bool,
    pub inert_keys: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub source: u32,
    pub target: Option<u32>,
    pub endpoints: ResolvedEndpoints,
    pub settings: SpanSettings,
}

#[derive(Debug, Clone, Default)]
pub struct Prepared {
    pub spans: Vec<Span>,
    pub issues: Vec<Issue>,
    pub omitted: usize,
}

impl Prepared {
    pub fn validate(&self) -> Result<(), String> {
        if self.omitted != 0 || self.spans.len() > MAX_LINKS {
            return Err(format!("Rope preview exceeds {MAX_LINKS} source spans"));
        }
        if let Some(issue) = self.issues.first() {
            return Err(format!(
                "Rope entity:{}: {} ({} unavailable sources)",
                issue.entity,
                issue.message,
                self.issues.len()
            ));
        }
        let mut sources = std::collections::HashSet::new();
        for span in &self.spans {
            if !sources.insert(span.source) {
                return Err(format!("Rope source entity:{} is duplicated", span.source));
            }
            span.settings.validate()?;
            if !(0..=16_383).contains(&span.endpoints.rope_length) {
                return Err(format!(
                    "Rope entity:{} length exceeds the supported 0–16383 range",
                    span.source
                ));
            }
            if span.endpoints.end.is_none() {
                return Err(format!(
                    "Rope entity:{} needs a resolved end anchor, including dangling ropes",
                    span.source
                ));
            }
            if span.target.is_none()
                && (!span.settings.dangling
                    || span.endpoints.rope_length != 0
                    || span
                        .endpoints
                        .end
                        .is_some_and(|end| end.position != DVec3::ZERO))
            {
                return Err(format!(
                    "Rope entity:{} has an invalid absent-end binding",
                    span.source
                ));
            }
            for endpoint in [Some(span.endpoints.start), span.endpoints.end]
                .into_iter()
                .flatten()
            {
                if !endpoint.position.is_finite()
                    || endpoint.position.abs().max_element() + span.settings.width * 0.5
                        > MAX_EXTENT
                    || !endpoint.forward.is_finite()
                {
                    return Err(format!(
                        "Rope entity:{} has an invalid resolved anchor",
                        span.source
                    ));
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Progress {
    pub complete: bool,
    pub time: f64,
    pub steps: usize,
    pub warmup_steps: usize,
    pub work: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct RopeVertex {
    pub position: DVec3,
    pub normal: DVec3,
    pub uv: [f64; 2],
}
