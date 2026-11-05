use std::time::{Duration, Instant};

pub(super) const INTERVAL: Duration = Duration::from_millis(20);
const RESTART_TIMEOUT: Duration = Duration::from_secs(2);

pub(super) fn wait_for_restart<T>(
    mut attempt: impl FnMut() -> Result<Option<T>, String>,
) -> Result<T, String> {
    let started = Instant::now();
    loop {
        if let Some(value) = attempt()? {
            return Ok(value);
        }
        if started.elapsed() >= RESTART_TIMEOUT {
            return Err("Previous preview clock is still draining; wait and retry playback".into());
        }
        std::thread::sleep(INTERVAL);
    }
}
