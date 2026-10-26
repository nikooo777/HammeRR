use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use render::color_correction::{self, Lut, Preview};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use super::Session;
use crate::bounded_file::Source;

const POLL_INTERVAL: Duration = Duration::from_secs(1);
const MAX_PATH_BYTES: usize = 4096;

struct Loaded {
    source: Source,
    lut: Arc<Lut>,
    sha256: String,
}

pub(super) struct State {
    loaded: Option<Loaded>,
    path: Option<String>,
    enabled: bool,
    weight: f32,
    error: Option<String>,
    last_poll: Option<Instant>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            loaded: None,
            path: None,
            enabled: false,
            weight: 1.0,
            error: None,
            last_poll: None,
        }
    }
}

fn load(path: &Path) -> Result<(String, Loaded), String> {
    let absolute = std::path::absolute(path).map_err(|error| error.to_string())?;
    let label = absolute.to_str().ok_or("LUT path must be valid UTF-8")?;
    if label.len() > MAX_PATH_BYTES {
        return Err(format!("LUT path exceeds {MAX_PATH_BYTES} bytes"));
    }
    if !absolute
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("raw"))
    {
        return Err("Choose a Source .raw color-correction LUT".into());
    }
    let source = Source::capture(&absolute, color_correction::RAW_BYTES as u64)
        .map_err(|error| format!("Could not open LUT: {error}"))?;
    let bytes = source
        .clone()
        .read()
        .map_err(|error| format!("Could not read LUT: {error}"))?;
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    let lut = Arc::new(Lut::from_raw(bytes)?);
    source
        .verify()
        .map_err(|error| format!("LUT source changed: {error}"))?;
    Ok((
        label.to_owned(),
        Loaded {
            source,
            lut,
            sha256,
        },
    ))
}

impl Session {
    fn fail_color_correction(&mut self, error: String) {
        self.color_correction.loaded = None;
        self.color_correction.error = Some(error);
        self.view_serial += 1;
    }

    pub fn load_color_correction(&mut self, path: &Path) -> Result<Value, String> {
        self.color_correction.enabled = false;
        match load(path) {
            Ok((path, loaded)) => {
                self.color_correction.path = Some(path);
                self.color_correction.loaded = Some(loaded);
                self.color_correction.enabled = false;
                self.color_correction.error = None;
                self.color_correction.last_poll = Some(Instant::now());
                self.view_serial += 1;
                Ok(self.color_correction_report())
            }
            Err(error) => {
                self.color_correction.path = path
                    .to_str()
                    .filter(|path| path.len() <= MAX_PATH_BYTES)
                    .map(str::to_owned);
                self.fail_color_correction(error.clone());
                Err(error)
            }
        }
    }

    pub fn set_color_correction(
        &mut self,
        enabled: bool,
        weight: f32,
        expected_identity: Option<u64>,
    ) -> Result<Value, String> {
        color_correction::validate_weight(weight)?;
        if enabled {
            if expected_identity.is_some_and(|expected| {
                self.color_correction
                    .loaded
                    .as_ref()
                    .is_none_or(|loaded| loaded.lut.identity() != expected)
            }) {
                return Err(
                    "Loaded LUT changed; inspect color correction again before applying preview"
                        .into(),
                );
            }
            self.check_color_correction_source(true)?;
            if self.color_correction.loaded.is_none() {
                return Err(self.color_correction.error.clone().unwrap_or_else(|| {
                    "Load a Source .raw LUT before enabling color correction".into()
                }));
            }
        }
        if self.color_correction.enabled != enabled || self.color_correction.weight != weight {
            self.color_correction.enabled = enabled;
            self.color_correction.weight = weight;
            self.view_serial += 1;
        }
        Ok(self.color_correction_report())
    }

    pub fn clear_color_correction(&mut self) -> Value {
        self.color_correction = State::default();
        self.view_serial += 1;
        self.color_correction_report()
    }

    fn check_color_correction_source(&mut self, read: bool) -> Result<(), String> {
        let Some(loaded) = &self.color_correction.loaded else {
            return Ok(());
        };
        let result = if read {
            loaded
                .source
                .clone()
                .read()
                .map_err(|error| error.to_string())
                .and_then(|bytes| {
                    if bytes == loaded.lut.raw() {
                        Ok(())
                    } else {
                        Err("LUT bytes changed since loading".into())
                    }
                })
        } else {
            loaded.source.verify().map_err(|error| error.to_string())
        };
        if let Err(error) = result {
            let error = format!("Color-correction LUT is unavailable: {error}. Load the LUT again");
            self.fail_color_correction(error.clone());
            return Err(error);
        }
        Ok(())
    }

    pub fn poll_color_correction(&mut self) {
        let now = Instant::now();
        if self
            .color_correction
            .last_poll
            .is_none_or(|last| now.duration_since(last) >= POLL_INTERVAL)
        {
            self.color_correction.last_poll = Some(now);
            let _ = self.check_color_correction_source(false);
        }
    }

    pub fn color_correction_for_render(&mut self) -> Result<Option<Preview>, String> {
        if !self.color_correction.enabled {
            return Ok(None);
        }
        self.check_color_correction_source(true)?;
        let loaded = self.color_correction.loaded.as_ref().ok_or_else(|| {
            self.color_correction
                .error
                .clone()
                .unwrap_or_else(|| "Color-correction LUT is unavailable; load it again".into())
        })?;
        Ok(Some(Preview {
            lut: loaded.lut.clone(),
            weight: self.color_correction.weight,
        }))
    }

    pub fn color_correction_state(&mut self) -> Value {
        let _ = self.check_color_correction_source(true);
        self.color_correction_report()
    }

    pub fn verify_color_correction_render(
        &mut self,
        preview: Option<&Preview>,
    ) -> Result<(), String> {
        let Some(preview) = preview else {
            return Ok(());
        };
        if !self.color_correction.enabled
            || self.color_correction.weight != preview.weight
            || self
                .color_correction
                .loaded
                .as_ref()
                .is_none_or(|loaded| loaded.lut.identity() != preview.lut.identity())
        {
            return Err(
                "Color-correction settings or LUT changed during rendering; render again".into(),
            );
        }
        self.check_color_correction_source(true)
    }

    pub fn color_correction_report(&self) -> Value {
        let state = &self.color_correction;
        let summary = if let Some(error) = &state.error {
            error.clone()
        } else if state.enabled {
            format!("Color correction enabled · weight {}", state.weight)
        } else if state.loaded.is_some() {
            "Color correction is off · LUT loaded".into()
        } else {
            "Color correction is off · no LUT loaded".into()
        };
        json!({"enabled":state.enabled,"weight":state.weight,"ready":state.loaded.is_some(),
            "source":state.path,"identity":state.loaded.as_ref().map(|loaded|loaded.lut.identity()),
            "sha256":state.loaded.as_ref().map(|loaded|&loaded.sha256),"error":state.error,"summary":summary,
            "gpu_status":"Not determined by source inspection; render_view reports applied only after a successful frame. Desktop GPU failures show an ungraded frame and a visible error.",
            "limits":{"raw_bytes":color_correction::RAW_BYTES,"edge":color_correction::EDGE,"path_bytes":MAX_PATH_BYTES,
                "gpu_lut_bytes":color_correction::RGBA_BYTES,"lut_upload_bytes":color_correction::UPLOAD_BYTES,"target_bytes":color_correction::MAX_TARGET_BYTES,
                "target_dimension":color_correction::MAX_TARGET_DIMENSION},"policy":color_correction::POLICY})
    }
}

#[cfg(test)]
mod tests;
