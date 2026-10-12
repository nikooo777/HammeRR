use std::cell::Cell;
use std::rc::Rc;

use slint::ComponentHandle;

use crate::CompileWindow;
use crate::mcp::{SharedSession, lock};
use crate::session::{Actor, Session};

pub struct Controller {
    pub window: CompileWindow,
    token: Rc<Cell<Option<u64>>>,
}

impl Controller {
    pub fn new(shared: &SharedSession) -> Result<Self, slint::PlatformError> {
        let window = CompileWindow::new()?;
        let token = Rc::new(Cell::new(None));
        window.on_play_build({
            let (shared, window) = (shared.clone(), window.as_weak());
            move || {
                let result = crate::mcp::start_launch(&shared, Actor::User);
                window.unwrap().set_message(match result {
                    Ok(()) => "Launching the last successful build with -insecure. Open Game Log for engine output.".into(),
                    Err(error) => error.into(),
                });
            }
        });
        window.on_cancel_build({
            let (shared, window, token) = (shared.clone(), window.as_weak(), token.clone());
            move || {
                if let Some(token) = token.get() {
                    let result = lock(&shared).cancel_compile(Actor::User, token);
                    window.unwrap().set_message(match result {
                        Ok(()) => {
                            "Cancellation requested. Waiting for the compiler to stop.".into()
                        }
                        Err(error) => error.into(),
                    });
                }
            }
        });
        window.on_close_dialog({
            let window = window.as_weak();
            move || {
                let _ = window.unwrap().hide();
            }
        });
        Ok(Self { window, token })
    }

    pub fn refresh(&self, session: &Session) {
        self.token.set(session.compile_token());
        self.window.set_cancellable(session.compile_cancellable());
        let status = session.compile_status();
        let launch = &status["launch"];
        self.window
            .set_launch_ready(launch["ready"].as_bool().unwrap_or(false));
        self.window.set_launch_details(
            match launch["bsp"].as_str() {
                Some(path) => format!("Ready to play without rebuilding: {path}"),
                None => format!(
                    "Play unavailable: {}",
                    launch["reason"].as_str().unwrap_or_default()
                ),
            }
            .into(),
        );
        let job = &status["job"];
        if job.is_null() {
            self.window
                .set_summary("No build has run in this session.".into());
            self.window.set_details(
                "Compile the current map to inspect its stages and diagnostics.".into(),
            );
            self.window.set_diagnostics("".into());
            return;
        }
        self.window.set_summary(session.compile_label().into());
        let result = &job["result"];
        let samples = result["stats"]["cubemap_samples"]
            .as_u64()
            .map(|count| format!("\nCubemap samples: {count}"))
            .unwrap_or_default();
        let visibility = result["stats"]["clusters"]
            .as_u64()
            .map(|count| {
                let portals = result["stats"]["visibility_portals"]
                    .as_u64()
                    .unwrap_or_default();
                let cluster_label = if count == 1 { "cluster" } else { "clusters" };
                let portal_label = if portals == 1 { "portal" } else { "portals" };
                format!("\nVisibility: {count} {cluster_label} · {portals} {portal_label}")
            })
            .unwrap_or_default();
        let water = result["stats"]["water_regions"]
            .as_u64()
            .filter(|&count| count > 0)
            .map(|count| {
                let leaves = result["stats"]["water_leaves"].as_u64().unwrap_or_default();
                let region_label = if count == 1 { "region" } else { "regions" };
                let leaf_label = if leaves == 1 { "leaf" } else { "leaves" };
                format!("\nWater: {count} {region_label} · {leaves} submerged {leaf_label}")
            })
            .unwrap_or_default();
        let sky = result["stats"]["sky_clusters"]
            .as_u64()
            .filter(|&count| count > 0)
            .map(|count| {
                let label = if count == 1 { "cluster" } else { "clusters" };
                format!("\nSky visible from: {count} {label}")
            })
            .unwrap_or_default();
        self.window.set_details(
            format!(
                "Output: {}\nStage: {} · {:.2}s elapsed{samples}{visibility}{water}{sky}\n{}",
                job["path"].as_str().unwrap_or_default(),
                job["stage_label"].as_str().unwrap_or_default(),
                job["elapsed_ms"].as_u64().unwrap_or_default() as f64 / 1000.0,
                if job["stale"].as_bool().unwrap_or(false) {
                    "The map changed after this build started. Compile again before playing."
                } else {
                    "This build uses the current map snapshot. Unsaved VMF edits are included."
                }
            )
            .into(),
        );
        let mut messages = Vec::new();
        if let Some(error) = result["error"].as_str() {
            messages.push(error.to_owned());
        }
        if let Some(diagnostics) = result["diagnostics"].as_array() {
            for diagnostic in diagnostics {
                messages.push(format!(
                    "{}{}: {}",
                    diagnostic["level"].as_str().unwrap_or("info"),
                    diagnostic["object"]
                        .as_str()
                        .map(|object| format!(" · {object}"))
                        .unwrap_or_default(),
                    diagnostic["message"].as_str().unwrap_or_default()
                ));
            }
            let count = result["diagnostic_count"].as_u64().unwrap_or_default() as usize;
            if count > diagnostics.len() {
                messages.push(format!(
                    "Showing the first {} of {count} diagnostics.",
                    diagnostics.len()
                ));
            }
        }
        if messages.is_empty() {
            messages.push(
                if result.is_null() {
                    "Diagnostics appear when the build finishes."
                } else {
                    "No diagnostics."
                }
                .into(),
            );
        }
        self.window.set_diagnostics(messages.join("\n\n").into());
    }

    pub fn show(&self, session: &Session) -> Result<(), slint::PlatformError> {
        self.window.set_message("Closing this window leaves an active build running. Cancel build stops it and preserves previous output.".into());
        self.refresh(session);
        self.window.show()
    }
}
