use super::*;
use serde_json::{Value, json};
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct State {
    serial: u64,
    job: Option<Job>,
}

struct Job {
    token: u64,
    control: compiler::Control,
    path: PathBuf,
    snapshot: Snapshot,
    started: Instant,
    finished: Option<Duration>,
    status: &'static str,
    result: Option<Value>,
}

pub struct CompileRequest {
    control: compiler::Control,
    vmf: Vmf,
    game: Arc<GameFs>,
    path: PathBuf,
    generation: u64,
    revision: u64,
}

pub struct CompileResult {
    pub output: compiler::Output,
    pub path: PathBuf,
    generation: u64,
    revision: u64,
}

impl CompileRequest {
    pub fn run(self) -> Result<CompileResult, String> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.run_inner()))
            .unwrap_or_else(|_| Err("Compilation worker panicked".into()))
    }

    fn run_inner(self) -> Result<CompileResult, String> {
        let output = compiler::compile_controlled(&self.vmf, &self.game, &self.control);
        self.control.checkpoint()?;
        output
            .write_controlled(&self.path, &self.control)
            .map_err(|e| format!("{}: {e}", self.path.display()))?;
        Ok(CompileResult {
            output,
            path: self.path,
            generation: self.generation,
            revision: self.revision,
        })
    }
}

pub(crate) fn stats_json(stats: &compiler::Stats) -> Value {
    json!({
        "brushes": stats.brushes,
        "nodes": stats.nodes,
        "leaves": stats.leaves,
        "faces": stats.faces,
        "models": stats.models,
        "cubemap_samples": stats.cubemap_samples,
        "clusters": stats.clusters,
        "visibility_portals": stats.visibility_portals,
        "visibility_bytes": stats.visibility_bytes,
        "water_regions": stats.water_regions,
        "water_leaves": stats.water_leaves,
        "sky_clusters": stats.sky_clusters,
        "elapsed_ms": stats.elapsed.as_millis(),
    })
}

fn result_json(result: &CompileResult) -> Value {
    let output = &result.output;
    json!({
        "success":output.bsp.is_some(),
        "bsp":output.bsp.as_ref().map(|_|result.path.display().to_string()),
        "diagnostic_count":output.report.diagnostics.len(),
        "diagnostics":output.report.diagnostics.iter().take(100).map(|diagnostic|json!({
            "level":match diagnostic.level { compiler::Level::Info=>"info",compiler::Level::Warning=>"warning",compiler::Level::Error=>"error" },
            "message":diagnostic.message,"object":diagnostic.object,"position":diagnostic.position.map(|p|p.to_array())
        })).collect::<Vec<_>>(),
        "leak":output.leak.as_ref().map(|leak|json!({"entity":format!("entity:{}",leak.entity),"points":leak.points.iter().map(|p|p.to_array()).collect::<Vec<_>>(),"pointfile":result.path.with_extension("lin").display().to_string()})),
        "stats":stats_json(&output.stats),
    })
}

impl Session {
    pub fn begin_compile(
        &mut self,
        actor: Actor,
        path: Option<&Path>,
    ) -> Result<CompileRequest, String> {
        self.ensure_partitions_resolved()?;
        if self.closing {
            return Err("the editor is closing".into());
        }
        if self.compiling {
            return Err("a map compilation is already running".into());
        }
        if self.launching {
            return Err("wait for CS:S to finish launching".into());
        }
        let path = path
            .map(Path::to_owned)
            .or_else(|| self.path.as_ref().map(|p| p.with_extension("bsp")))
            .ok_or("save the map first or give a .bsp output path")?;
        if !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("bsp"))
        {
            return Err("compile output must be a .bsp file".into());
        }
        if self.path.as_deref() == Some(path.as_path()) {
            return Err("compile output must be distinct from the source map".into());
        }
        if let Some(error) = &self.game_error {
            return Err(error.clone());
        }
        let control = compiler::Control::default();
        self.compilation.serial += 1;
        self.compilation.job = Some(Job {
            token: self.compilation.serial,
            control: control.clone(),
            path: path.clone(),
            snapshot: Snapshot {
                generation: self.document_generation,
                revision: self.document.revision(),
            },
            started: Instant::now(),
            finished: None,
            status: "running",
            result: None,
        });
        let request = CompileRequest {
            control,
            vmf: self.document.vmf().clone(),
            game: self.game.clone(),
            path,
            generation: self.document_generation,
            revision: self.document.revision(),
        };
        self.compiling = true;
        self.log(actor, "compiling map");
        Ok(request)
    }

    pub fn finish_compile(&mut self, actor: Actor, result: &Result<CompileResult, String>) {
        self.compiling = false;
        if let Some(job) = &mut self.compilation.job {
            job.control.finish();
            job.finished = Some(job.started.elapsed());
            job.status = if job.control.cancelled() {
                "cancelled"
            } else if result.as_ref().is_ok_and(|r| r.output.bsp.is_some()) {
                "completed"
            } else {
                "failed"
            };
            job.result = Some(match result {
                Ok(result) => result_json(result),
                Err(error) => json!({"success":false,"error":error}),
            });
        }
        if let Ok(result) = result {
            let snapshot = Snapshot {
                generation: result.generation,
                revision: result.revision,
            };
            if self.current_snapshot(&snapshot) {
                self.reset_pointfile();
                self.leak = result
                    .output
                    .leak
                    .clone()
                    .map(|leak| (snapshot.clone(), leak));
                self.changed();
            }
            if result.output.bsp.is_some() {
                self.compiled = Some(CompiledMap {
                    path: result.path.clone(),
                    snapshot,
                });
            }
        }
        let message = match result {
            Err(_)
                if self
                    .compilation
                    .job
                    .as_ref()
                    .is_some_and(|job| job.control.cancelled()) =>
            {
                "Compilation cancelled; previous output preserved".into()
            }
            Err(message) => format!("compile failed: {message}"),
            Ok(result) if result.output.bsp.is_none() => {
                let message = result
                    .output
                    .report
                    .diagnostics
                    .iter()
                    .find(|d| d.level == compiler::Level::Error)
                    .map(|d| d.message.as_str())
                    .unwrap_or("no BSP produced");
                let pointfile = if result.output.leak.is_some() {
                    format!(
                        "; pointfile {}",
                        result.path.with_extension("lin").display()
                    )
                } else {
                    String::new()
                };
                format!("compile failed: {message}{pointfile}")
            }
            Ok(result) => {
                let stale = if result.generation != self.document_generation
                    || result.revision != self.document.revision()
                {
                    " (an earlier snapshot)"
                } else {
                    ""
                };
                let stats = &result.output.stats;
                format!(
                    "compiled {}: {} brushes, {} faces in {:.1?}{stale}",
                    result.path.display(),
                    stats.brushes,
                    stats.faces,
                    stats.elapsed
                )
            }
        };
        self.log(actor, message);
    }

    pub fn compile_status(&self) -> Value {
        let job = self.compilation.job.as_ref().map(|job| {
            let progress = job.control.progress();
            json!({"token":job.token.to_string(),"status":if job.status=="running" && progress.cancel_requested {"cancelling"} else {job.status},
                "stage":progress.stage.name(),"stage_label":progress.stage.label(),"cancellable":job.status=="running" && progress.cancellable && !progress.cancel_requested,
                "elapsed_ms":job.finished.unwrap_or_else(||job.started.elapsed()).as_millis(),
                "path":job.path.display().to_string(),"generation":job.snapshot.generation,"revision":job.snapshot.revision,
                "stale":!self.current_snapshot(&job.snapshot),"result":job.result})
        });
        let launch = match self.launch_readiness() {
            Ok(path) => json!({"ready":true,"bsp":path.display().to_string(),"reason":null}),
            Err(reason) => json!({"ready":false,"bsp":null,"reason":reason}),
        };
        json!({"job":job,"launch":launch})
    }

    pub fn compile_token(&self) -> Option<u64> {
        self.compilation.job.as_ref().map(|job| job.token)
    }

    pub fn cancel_compile(&mut self, actor: Actor, token: u64) -> Result<(), String> {
        let job = self
            .compilation
            .job
            .as_ref()
            .filter(|job| job.token == token)
            .ok_or("Compile job was replaced")?;
        if job.status != "running" {
            return Err("Compile job is already finished".into());
        }
        job.control.cancel()?;
        self.log(actor, "Cancelling compilation");
        Ok(())
    }

    pub fn compile_label(&self) -> String {
        let Some(job) = &self.compilation.job else {
            return String::new();
        };
        let progress = job.control.progress();
        match job.status {
            "running" if progress.cancel_requested => "Cancelling compilation…".into(),
            "running" => format!("Compiling · {}", progress.stage.label()),
            "completed" => format!(
                "Compiled in {:.2}s{}",
                job.finished.unwrap_or_default().as_secs_f64(),
                if self.current_snapshot(&job.snapshot) {
                    ""
                } else {
                    " · earlier map snapshot"
                }
            ),
            "cancelled" => "Compilation cancelled · previous output preserved".into(),
            _ => "Compilation failed · open Build details".into(),
        }
    }

    pub fn compile_cancellable(&self) -> bool {
        self.compilation.job.as_ref().is_some_and(|job| {
            let progress = job.control.progress();
            job.status == "running" && progress.cancellable && !progress.cancel_requested
        })
    }

    pub(super) fn cancel_compile_on_shutdown(&self) {
        if let Some(job) = &self.compilation.job {
            let _ = job.control.cancel();
        }
    }
}
