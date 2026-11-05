use super::timeline::checked_time;

pub const MAX_PASS_EVENTS: usize = super::MAX_EVENTS;
pub const PASS_HISTORY_POLICY: &str = "Explicit captured fog-setup history. The initial radial state is unknown. A declared shader default-state Reset writes false. Enabled World and Sky setups write their sampled radial bit; disabled or absent setups preserve it. Underwater material fog consumes the last established radial bit without writing it. Captures retain their sampled values and equal-time append order; Query and Seek do not add or resample passes. This declared history does not discover or reproduce arbitrary native render-pass ordering.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderPass {
    Reset,
    World {
        controller: Option<u32>,
        radial: Option<bool>,
    },
    Sky {
        camera: u32,
        radial: Option<bool>,
    },
}

impl RenderPass {
    pub fn name(self) -> &'static str {
        match self {
            Self::Reset => "reset",
            Self::World { .. } => "world",
            Self::Sky { .. } => "sky",
        }
    }

    pub fn radial_write(self) -> Option<bool> {
        match self {
            Self::Reset => Some(false),
            Self::World { radial, .. } | Self::Sky { radial, .. } => radial,
        }
    }

    fn validate(self) -> Result<(), String> {
        if matches!(
            self,
            Self::World {
                controller: None,
                radial: Some(_),
            }
        ) {
            return Err("A world fog setup without a controller cannot write radial state".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderPassEvent {
    pub time_seconds: f64,
    pub pass: RenderPass,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PassWriter {
    pub time_seconds: f64,
    pub order: usize,
    pub pass: RenderPass,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PassFrame {
    pub radial: Option<bool>,
    pub last_writer: Option<PassWriter>,
    pub event_count: usize,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PassHistory {
    events: Vec<RenderPassEvent>,
}

impl PassHistory {
    pub fn events(&self) -> &[RenderPassEvent] {
        &self.events
    }

    pub fn prepare_push(&self, time_seconds: f64) -> Result<(), String> {
        checked_time(time_seconds)?;
        if self.events.len() >= MAX_PASS_EVENTS {
            return Err(format!(
                "Fog pass history exceeds {MAX_PASS_EVENTS} events; Reset the preview before adding more"
            ));
        }
        if self
            .events
            .last()
            .is_some_and(|previous| time_seconds < previous.time_seconds)
        {
            return Err(
                "Fog passes must be appended in time order; Reset the preview before editing an earlier time"
                    .into(),
            );
        }
        Ok(())
    }

    pub fn push(&mut self, event: RenderPassEvent) -> Result<(), String> {
        self.prepare_push(event.time_seconds)?;
        event.pass.validate()?;
        self.events
            .try_reserve(1)
            .map_err(|_| "Fog pass history allocation failed")?;
        self.events.push(event);
        Ok(())
    }

    pub fn evaluate(&self, time_seconds: f64) -> Result<PassFrame, String> {
        checked_time(time_seconds)?;
        let mut frame = PassFrame::default();
        for (order, event) in self.events.iter().enumerate() {
            if event.time_seconds > time_seconds {
                break;
            }
            frame.event_count += 1;
            if let Some(radial) = event.pass.radial_write() {
                frame.radial = Some(radial);
                frame.last_writer = Some(PassWriter {
                    time_seconds: event.time_seconds,
                    order,
                    pass: event.pass,
                });
            }
        }
        Ok(frame)
    }
}
