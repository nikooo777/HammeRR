use std::{
    io::{self, Read, Write},
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use tempfile::NamedTempFile;

const MAX_DECODE_BYTES: usize = 2 * 1024 * 1024;
const DECODE_TIMEOUT: Duration = Duration::from_secs(5);
pub(super) const MAX_SECONDS: u32 = 10;

#[derive(Default)]
struct PlaybackObservation {
    finished: Option<bool>,
    error: Option<String>,
}

impl PlaybackObservation {
    fn pending(&self) -> bool {
        self.finished.is_none() && self.error.is_none()
    }

    fn observe(&mut self, result: io::Result<Option<ExitStatus>>) {
        if !self.pending() {
            return;
        }
        match result {
            Ok(status) => self.finished = status.map(|status| status.success()),
            Err(error) => {
                self.error = Some(format!("Cannot observe sound preview process: {error}"));
            }
        }
    }

    fn state(&self) -> &'static str {
        if self.error.is_some() {
            "failed"
        } else {
            match self.finished {
                None => "playing",
                Some(true) => "finished",
                Some(false) => "failed",
            }
        }
    }
}

pub(super) fn decode(path: &str, source: &[u8], volume: f32) -> Result<Vec<u8>, String> {
    let mut input = NamedTempFile::new().map_err(|error| error.to_string())?;
    input.write_all(source).map_err(|error| error.to_string())?;
    let output = NamedTempFile::new().map_err(|error| error.to_string())?;
    let format = if path.ends_with(".mp3") { "mp3" } else { "wav" };
    let mut child=Command::new("ffmpeg")
        .args(["-nostdin", "-hide_banner", "-loglevel", "error"])
        .args(["-protocol_whitelist", "file,pipe", "-threads", "1", "-f", format, "-i"])
        .arg(input.path())
        .arg("-t").arg(MAX_SECONDS.to_string())
        .args(["-ac", "2", "-ar", "44100", "-map_metadata", "-1", "-af"])
        .arg(format!("volume={volume}"))
        .args(["-c:a", "pcm_s16le", "-f", "wav", "-fs"])
        .arg(MAX_DECODE_BYTES.to_string()).arg("-y").arg(output.path())
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
        .spawn().map_err(|error|format!("Cannot start FFmpeg for sound previews: {error}. Install ffmpeg; browsing and assignment remain available."))?;
    let deadline = Instant::now() + DECODE_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            result => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(result
                    .err()
                    .map_or("Sound decoding exceeded 5 seconds".into(), |error| {
                        error.to_string()
                    }));
            }
        }
    };
    if !status.success() {
        return Err(format!(
            "FFmpeg could not decode {path}; choose a supported WAV/MP3 variant"
        ));
    }
    let file = std::fs::File::open(output.path()).map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    file.take(MAX_DECODE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > MAX_DECODE_BYTES || bytes.len() < 44 {
        return Err("Decoded sound exceeds preview limits or has no audio".into());
    }
    source_formats::sound::inspect_audio("preview.wav", &bytes)
        .map_err(|error| error.to_string())?;
    Ok(bytes)
}

pub(super) struct Playback {
    pub token: u64,
    pub report: Value,
    _file: NamedTempFile,
    child: Child,
    started: Instant,
    observation: PlaybackObservation,
}

impl Playback {
    pub(super) fn close_identity(&self) -> Value {
        json!({"token": self.token, "finished": self.observation.finished, "error": self.observation.error})
    }

    pub fn start(token: u64, report: Value, bytes: &[u8]) -> Result<Self, String> {
        let mut file = NamedTempFile::new().map_err(|error| error.to_string())?;
        file.write_all(bytes).map_err(|error| error.to_string())?;
        let child=Command::new("ffplay")
            .args(["-nodisp", "-autoexit", "-loglevel", "error", "-threads", "1", "-t"])
            .arg(MAX_SECONDS.to_string()).arg("-i").arg(file.path())
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
            .spawn().map_err(|error|format!("Cannot start ffplay for audition: {error}. Install ffmpeg; file inspection and assignment remain available."))?;
        Ok(Self {
            token,
            report,
            _file: file,
            child,
            started: Instant::now(),
            observation: PlaybackObservation::default(),
        })
    }
    pub fn status(&mut self) -> Value {
        if self.observation.pending() {
            let result = self.child.try_wait();
            self.observation.observe(result);
        }
        json!({"token":self.token,"state":self.observation.state(),"error":self.observation.error,"elapsed":self.started.elapsed().as_secs_f64(),"sound":self.report})
    }
}

impl Drop for Playback {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod close_tests;

#[cfg(all(test, unix))]
mod status_tests;
