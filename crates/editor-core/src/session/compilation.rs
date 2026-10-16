use super::*;
use serde_json::{Value, json};
use std::sync::Mutex;
use std::time::{Duration, Instant};

mod recipes;
#[cfg(test)]
mod reflection_settings_tests;
#[cfg(test)]
mod reflection_transaction_tests;

#[derive(Default)]
pub struct State {
    serial: u64,
    job: Option<Job>,
    options: compiler::Options,
    recipes: recipes::State,
}

struct Job {
    token: u64,
    control: compiler::Control,
    path: PathBuf,
    source_path: Option<PathBuf>,
    snapshot: Snapshot,
    content_epoch: u64,
    native_runtime: Option<Value>,
    region: Value,
    options: compiler::Options,
    recipe: Option<recipes::Identity>,
    started: Instant,
    finished: Option<Duration>,
    status: &'static str,
    result: Option<Value>,
}

pub struct CompileRequest {
    token: u64,
    control: compiler::Control,
    vmf: Vmf,
    game: Arc<GameFs>,
    path: PathBuf,
    generation: u64,
    revision: u64,
    options: compiler::Options,
    content_epoch: u64,
    context: Option<compiler::CompileContext>,
    environment: Option<native_runtime::CaptureEnvironment>,
    source_path: Option<PathBuf>,
    source_stamp: Option<compiler::FileStamp>,
    destination_stamp: Option<compiler::FileStamp>,
}

pub struct CompileResult {
    token: u64,
    pub output: compiler::Output,
    pub path: PathBuf,
    generation: u64,
    revision: u64,
    options: compiler::Options,
    content_epoch: u64,
    source: Option<compiler::FileIdentity>,
    destination_stamp: Option<compiler::FileStamp>,
    control: compiler::Control,
    publication: Mutex<Publication>,
}

struct Publication {
    pending: Option<compiler::PreparedWrite>,
    lease: Option<compiler::OutputLease>,
    pointfile_lease: Option<compiler::OutputLease>,
    sources: Option<compiler::cubemap_assets::ReflectionSourceGuard>,
    published: bool,
    committed: bool,
    error: Option<String>,
}

impl CompileResult {
    pub(crate) fn published(&self) -> bool {
        self.publication
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .published
    }

    pub(crate) fn publication_error(&self) -> Option<String> {
        self.publication
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .error
            .clone()
    }
}

impl CompileRequest {
    pub fn run(self) -> Result<CompileResult, String> {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.run_inner()))
            .unwrap_or_else(|_| Err("Compilation worker panicked".into()))
    }

    fn run_inner(self) -> Result<CompileResult, String> {
        self.control.checkpoint()?;
        let mut lease = None;
        let mut source = None;
        let mut pointfile_lease = None;
        if self.options.generate_reflections {
            native_runtime::preflight_vmf(&self.vmf).map_err(|e| e.to_string())?;
            if let Some(stamp) = &self.source_stamp {
                stamp.verify_metadata().map_err(|e| e.to_string())?;
            }
            if let Some(path) = &self.source_path {
                source = Some(
                    compiler::FileIdentity::capture(path, &self.control)
                        .map_err(|e| e.to_string())?,
                );
            }
            if let (Some(stamp), Some(identity)) = (&self.source_stamp, &source) {
                stamp.verify_identity(identity).map_err(|e| e.to_string())?;
            }
            let stamp = self
                .destination_stamp
                .as_ref()
                .ok_or("Reflection destination snapshot missing")?;
            stamp.verify_metadata().map_err(|e| e.to_string())?;
            lease = Some(
                compiler::OutputLease::acquire(&self.path, &self.control)
                    .map_err(|e| e.to_string())?,
            );
            stamp
                .verify_lease(lease.as_ref().ok_or("Reflection output lease missing")?)
                .map_err(|e| e.to_string())?;
        }
        let path = lease
            .as_ref()
            .map(|lease| lease.path().to_owned())
            .unwrap_or_else(|| self.path.clone());
        let mut output = compiler::compile_with_context_controlled(
            &self.vmf,
            &self.game,
            self.options,
            self.context.as_ref(),
            &self.control,
        );
        self.control.checkpoint()?;
        if self.options.generate_reflections && output.bsp.is_some() {
            output = compiler::finalize_reflections(
                output,
                self.context
                    .as_ref()
                    .ok_or("Reflection compile context missing")?,
                &self.game,
                self.environment
                    .as_ref()
                    .ok_or("Native capture environment missing")?,
                &self.control,
            )
            .map_err(|e| e.to_string())?;
            if output.cubemap_assets.as_ref().is_none_or(|manifest| {
                manifest.phase != compiler::CubemapAssetPhase::Verified
                    || manifest.samples.is_empty()
            }) {
                return Err("Reflection finalizer did not return verified captured samples".into());
            }
        }
        self.control.checkpoint()?;
        let mut sources = None;
        let (pending, published, committed) = if self.options.generate_reflections {
            if output.bsp.is_none() && output.leak.is_some() {
                pointfile_lease = Some(
                    compiler::OutputLease::acquire(&path.with_extension("lin"), &self.control)
                        .map_err(|e| e.to_string())?,
                );
            }
            let pending = output
                .prepare_write_controlled(&path, &self.control)
                .map_err(|e| e.to_string())?;
            if let Some(manifest) = &output.cubemap_assets {
                sources = Some(
                    compiler::cubemap_assets::capture_reflection_source_guard(
                        manifest,
                        &self.game,
                        &self.control,
                    )
                    .map_err(|e| e.to_string())?,
                );
            }
            if let Some(source) = &source {
                source.verify(&self.control).map_err(|e| e.to_string())?;
            }
            lease
                .as_ref()
                .ok_or("Reflection destination lease missing")?
                .verify(&self.control)
                .map_err(|e| e.to_string())?;
            (pending, false, false)
        } else {
            output
                .write_controlled(&self.path, &self.control)
                .map_err(|e| format!("{}: {e}", self.path.display()))?;
            (None, output.bsp.is_some(), true)
        };
        Ok(CompileResult {
            token: self.token,
            output,
            path,
            generation: self.generation,
            revision: self.revision,
            options: self.options,
            content_epoch: self.content_epoch,
            source,
            destination_stamp: self.destination_stamp,
            control: self.control,
            publication: Mutex::new(Publication {
                pending,
                lease,
                pointfile_lease,
                sources,
                published,
                committed,
                error: None,
            }),
        })
    }
}

pub(crate) fn stats_json(stats: &compiler::Stats) -> Value {
    serde_json::to_value(stats).expect("compiler statistics are JSON-compatible")
}

fn result_json(result: &CompileResult) -> Value {
    let output = &result.output;
    json!({
        "success":result.published(),
        "published":result.published(),
        "error":result.publication_error(),
        "bsp":result.published().then(||result.path.display().to_string()),
        "diagnostic_count":output.report.diagnostics.len(),
        "diagnostics":output.report.diagnostics.iter().take(100).map(|diagnostic|json!({
            "level":match diagnostic.level { compiler::Level::Info=>"info",compiler::Level::Warning=>"warning",compiler::Level::Error=>"error" },
            "message":diagnostic.message,"object":diagnostic.object,"position":diagnostic.position.map(|p|p.to_array())
        })).collect::<Vec<_>>(),
        "leak":output.leak.as_ref().map(|leak|json!({"entity":format!("entity:{}",leak.entity),"points":leak.points.iter().map(|p|p.to_array()).collect::<Vec<_>>(),"pointfile":result.path.with_extension("lin").display().to_string()})),
        "stats":stats_json(&output.stats),
    })
}

#[derive(Default)]
pub struct BuildSettingsUpdate<'a> {
    pub lighting: Option<&'a str>,
    pub physics: Option<&'a str>,
    pub sky_quality: Option<&'a str>,
    pub visibility: Option<&'a str>,
    pub bounces: Option<u8>,
    pub smoothing_angle: Option<u8>,
    pub generate_reflections: Option<bool>,
}

impl Session {
    pub fn binding_output_namespace(&self) -> Result<Option<String>, String> {
        let path = self
            .compilation
            .job
            .as_ref()
            .filter(|job| {
                job.snapshot.generation == self.document_generation && job.source_path == self.path
            })
            .map(|job| job.path.clone())
            .or_else(|| self.path.as_ref().map(|path| path.with_extension("bsp")));
        path.as_ref()
            .map(|path| {
                compiler::CompileContext::from_output(path)
                    .map(|context| context.namespace().to_owned())
            })
            .transpose()
    }

    pub fn reflection_output_namespace(&self) -> Result<String, String> {
        self.binding_output_namespace()?.ok_or(
            "Save the map or compile to a named output before inspecting reflection aliases".into(),
        )
    }

    pub fn compile_options(&self) -> compiler::Options {
        self.compilation.options
    }

    pub fn compile_settings(&self) -> Value {
        let choices = [compiler::LightingMode::Fullbright, compiler::LightingMode::DirectLdr, compiler::LightingMode::DirectLdrHdr]
            .map(|mode| json!({"name":mode.name(),"label":mode.label(),"description":mode.description()}));
        let physics_choices = [compiler::PhysicsMode::None, compiler::PhysicsMode::Engine]
            .map(|mode| json!({"name":mode.name(),"label":mode.label(),"description":mode.description()}));
        let sky_choices = [compiler::SkyQuality::Fast, compiler::SkyQuality::Balanced, compiler::SkyQuality::Detailed].map(|quality| json!({"name":quality.name(), "label":quality.label(), "directions":quality.directions()}));
        let visibility_choices = [compiler::VisibilityMode::Fast, compiler::VisibilityMode::Detailed].map(|mode| json!({"name":mode.name(), "label":mode.label(), "description":mode.description()}));
        let bounce_choices = compiler::BOUNCE_LABELS
            .iter()
            .enumerate()
            .map(|(count, label)| json!({"bounces": count, "label": label}))
            .collect::<Vec<_>>();
        json!({"generate_reflections":self.compilation.options.generate_reflections,"reflection_description":"Generate one native LDR reflection bounce during Compile or Run Map. Requires Direct LDR and a verified CS:S capture runtime. HDR and Fullbright generation are unsupported; turn generation off before selecting either mode.","smoothing_angle":self.compilation.options.smoothing_angle,"smoothing_angle_range":{"min":0,"max":90,"unit":"degrees","disabled":0},"bounces":self.compilation.options.bounces,"bounce_choices":bounce_choices,"visibility":self.compilation.options.visibility.name(), "visibility_choices":visibility_choices, "sky_quality":self.compilation.options.sky_quality.name(),"sky_choices":sky_choices,"lighting":self.compilation.options.lighting.name(),"physics":self.compilation.options.physics.name(),
            "recipe":self.compilation.recipes.active,"editable":!self.compiling && !self.closing,"choices":choices,"physics_choices":physics_choices})
    }

    pub fn set_compile_settings(&mut self, actor: Actor, lighting: &str) -> Result<Value, String> {
        self.update_compile_settings(actor, Some(lighting), None)
    }

    pub fn update_compile_settings(
        &mut self,
        actor: Actor,
        lighting: Option<&str>,
        physics: Option<&str>,
    ) -> Result<Value, String> {
        self.update_compile_settings_quality(actor, lighting, physics, None)
    }

    pub fn update_compile_settings_quality(
        &mut self,
        actor: Actor,
        lighting: Option<&str>,
        physics: Option<&str>,
        sky_quality: Option<&str>,
    ) -> Result<Value, String> {
        self.update_compile_options(actor, lighting, physics, sky_quality, None, None)
    }

    pub fn update_compile_options(
        &mut self,
        actor: Actor,
        lighting: Option<&str>,
        physics: Option<&str>,
        sky_quality: Option<&str>,
        visibility: Option<&str>,
        bounces: Option<u8>,
    ) -> Result<Value, String> {
        self.update_build_settings(
            actor,
            BuildSettingsUpdate {
                lighting,
                physics,
                sky_quality,
                visibility,
                bounces,
                ..Default::default()
            },
        )
    }

    pub fn update_build_settings(
        &mut self,
        actor: Actor,
        update: BuildSettingsUpdate<'_>,
    ) -> Result<Value, String> {
        let BuildSettingsUpdate {
            lighting,
            physics,
            sky_quality,
            visibility,
            bounces,
            smoothing_angle,
            generate_reflections,
        } = update;
        let bounces = bounces.unwrap_or(self.compilation.options.bounces);
        let options = compiler::Options {
            generate_reflections: generate_reflections
                .unwrap_or(self.compilation.options.generate_reflections),
            bounces,
            smoothing_angle: smoothing_angle.unwrap_or(self.compilation.options.smoothing_angle),
            visibility: visibility
                .map(compiler::VisibilityMode::parse)
                .transpose()?
                .unwrap_or(self.compilation.options.visibility),
            sky_quality: sky_quality
                .map(compiler::SkyQuality::parse)
                .transpose()?
                .unwrap_or(self.compilation.options.sky_quality),
            lighting: lighting
                .map(compiler::LightingMode::parse)
                .transpose()?
                .unwrap_or(self.compilation.options.lighting),
            physics: physics
                .map(compiler::PhysicsMode::parse)
                .transpose()?
                .unwrap_or(self.compilation.options.physics),
        }
        .validate()?;
        if self.closing {
            return Err("The editor is closing".into());
        }
        if self.compiling {
            return Err(
                "Wait for the active compile to finish before changing build settings".into(),
            );
        }
        if options != self.compilation.options {
            self.compilation.options = options;
            self.compilation.recipes.active = None;
            self.remember_current_map_context();
            self.log(
                actor,
                format!(
                    "Build settings: {} · {} · {} visibility · {} diffuse bounces · {}° brush smoothing · LDR reflections {}",
                    options.lighting.label(),
                    options.physics.label(),
                    options.visibility.label(),
                    options.bounces,
                    options.smoothing_angle,
                    if options.generate_reflections { "on" } else { "off" }
                ),
            );
        }
        Ok(self.compile_settings())
    }

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
        let generating = self.compilation.options.generate_reflections;
        let (context, environment, source_stamp, destination_stamp) = if generating {
            if self.game_running() {
                return Err("Close CS:S before capturing reflections".into());
            }
            let context = compiler::CompileContext::from_output(&path)?;
            let environment = self.capture_environment()?;
            let source_stamp = self
                .path
                .as_ref()
                .map(|path| compiler::FileStamp::capture(path).map_err(|e| e.to_string()))
                .transpose()?;
            let destination_stamp =
                compiler::FileStamp::capture(&path).map_err(|e| e.to_string())?;
            (
                Some(context),
                Some(environment),
                source_stamp,
                Some(destination_stamp),
            )
        } else {
            (None, None, None, None)
        };
        self.compilation.serial += 1;
        self.compilation.job = Some(Job {
            token: self.compilation.serial,
            control: control.clone(),
            path: path.clone(),
            source_path:self.path.clone(),
            content_epoch:self.content_epoch,
            native_runtime:environment.as_ref().map(|environment|serde_json::json!({"engine_root":environment.engine_root,"steam_root":environment.steam_root})),
            snapshot: Snapshot {
                generation: self.document_generation,
                revision: self.document.revision(),
            },
            options: self.compilation.options,
            region: self.compile_region_build_scope(),
            recipe: self.compilation.recipes.active.clone(),
            started: Instant::now(),
            finished: None,
            status: "running",
            result: None,
        });
        let request = CompileRequest {
            token: self.compilation.serial,
            control,
            vmf: self.document.vmf().clone(),
            game: self.game.clone(),
            path,
            generation: self.document_generation,
            revision: self.document.revision(),
            options: self.compilation.options,
            content_epoch: self.content_epoch,
            context,
            environment,
            source_path: self.path.clone(),
            source_stamp,
            destination_stamp,
        };
        self.compiling = true;
        self.log(actor, "compiling map");
        Ok(request)
    }

    fn commit_reflection_output(&self, result: &CompileResult) -> Result<(), String> {
        let mut publication = result
            .publication
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if publication.committed || publication.pending.is_none() {
            return Ok(());
        }
        let outcome = (|| {
            if self.closing
                || result.generation != self.document_generation
                || result.revision != self.document.revision()
                || result.options != self.compilation.options
                || result.content_epoch != self.content_epoch
            {
                return Err("Map, content, runtime or build settings changed during reflection capture; previous output preserved".into());
            }
            if self
                .compilation
                .job
                .as_ref()
                .is_none_or(|job| job.token != result.token)
            {
                return Err("Reflection compile job was replaced".into());
            }
            result.control.checkpoint()?;
            let pending = publication
                .pending
                .take()
                .ok_or("Prepared reflection output missing")?;
            let lease = publication
                .pointfile_lease
                .as_ref()
                .or(publication.lease.as_ref())
                .ok_or("Reflection destination lease missing")?;
            let success = pending
                .commit_checked_with(lease, &result.control, || {
                    if let Some(source) = &result.source {
                        source.verify_metadata()?;
                    }
                    if let Some(sources) = &publication.sources {
                        sources.verify_metadata()?;
                    }
                    if let Some(stamp) = &result.destination_stamp {
                        stamp.verify_metadata()?;
                    }
                    Ok(())
                })
                .map_err(|e| e.to_string())?;
            publication.published = success;
            publication.committed = true;
            Ok(())
        })();
        publication.lease = None;
        publication.pointfile_lease = None;
        publication.sources = None;
        if let Err(error) = &outcome {
            publication.pending = None;
            publication.error = Some(error.clone());
        }
        outcome
    }

    pub fn finish_compile(&mut self, actor: Actor, result: &Result<CompileResult, String>) {
        if let Ok(result) = result {
            if self
                .compilation
                .job
                .as_ref()
                .is_none_or(|job| job.token != result.token)
            {
                self.log(actor, "Discarded completion from a replaced compile job");
                return;
            }
            if result.options.generate_reflections {
                let _ = self.commit_reflection_output(result);
            }
        }
        self.compiling = false;
        if let Some(job) = &mut self.compilation.job {
            job.control.finish();
            job.finished = Some(job.started.elapsed());
            job.status = if job.control.cancelled() {
                "cancelled"
            } else if result.as_ref().is_ok_and(CompileResult::published) {
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
            if self.current_snapshot(&snapshot) && result.publication_error().is_none() {
                self.reset_pointfile();
                self.leak = result
                    .output
                    .leak
                    .clone()
                    .map(|leak| (snapshot.clone(), leak));
                self.refresh_scene();
            }
            if result.published() {
                self.compiled = Some(CompiledMap {
                    path: result.path.clone(),
                    snapshot,
                    options: result.options,
                    content_epoch: result.content_epoch,
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
            Ok(result) if result.publication_error().is_some() => format!(
                "compile failed: {}",
                result.publication_error().unwrap_or_default()
            ),
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
                "compile_region":job.region,"recipe":job.recipe,"native_runtime":job.native_runtime,"reflections":{"samples_complete":progress.reflection_samples_complete,"samples_total":progress.reflection_samples_total},"settings":{"generate_reflections":job.options.generate_reflections,"smoothing_angle":job.options.smoothing_angle,"bounces":job.options.bounces,"lighting":job.options.lighting.name(),"physics":job.options.physics.name(),"sky_quality":job.options.sky_quality.name(),"visibility":job.options.visibility.name()},
                "stale":!self.current_snapshot(&job.snapshot) || job.options != self.compilation.options || (job.options.generate_reflections && job.content_epoch != self.content_epoch),"result":job.result})
        });
        let launch = match self.launch_readiness() {
            Ok(path) => json!({"ready":true,"bsp":path.display().to_string(),"reason":null}),
            Err(reason) => json!({"ready":false,"bsp":null,"reason":reason}),
        };
        json!({"job":job,"launch":launch,"settings":self.compile_settings()})
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
                if self.current_snapshot(&job.snapshot) && job.options == self.compilation.options {
                    ""
                } else {
                    " · earlier map snapshot or build settings"
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_lighting_settings_are_shared_and_do_not_edit_map_history() {
        for (actor, lighting) in [Actor::User, Actor::Agent]
            .into_iter()
            .flat_map(|actor| ["direct_ldr", "direct_ldr_hdr"].map(|lighting| (actor, lighting)))
        {
            let (directory, game, _) = crate::test_assets::game_process(true);
            let mut session = Session::new(Ok(game));
            session.replace_document(
                Document::new(crate::test_assets::room()),
                Some(directory.path().join("room.vmf")),
            );
            let before = session.document.vmf().write(&Default::default());
            let revision = session.document.revision();
            let choices = session.compile_settings()["choices"].clone();
            assert_eq!(choices.as_array().unwrap().len(), 3);
            assert_eq!(choices[2]["name"], "direct_ldr_hdr");
            assert_eq!(choices[2]["label"], "Direct LDR + HDR");
            assert_eq!(session.compile_settings()["lighting"], "fullbright");
            assert_eq!(session.compile_settings()["sky_quality"], "fast");
            assert_eq!(session.compile_settings()["visibility"], "fast");
            assert_eq!(session.compile_settings()["bounces"], 0);
            assert_eq!(session.compile_settings()["smoothing_angle"], 0);
            session
                .update_build_settings(
                    actor,
                    BuildSettingsUpdate {
                        smoothing_angle: Some(30),
                        ..Default::default()
                    },
                )
                .unwrap();
            let selected = session.compile_settings();
            assert!(
                session
                    .update_build_settings(
                        actor,
                        BuildSettingsUpdate {
                            lighting: Some(lighting),
                            smoothing_angle: Some(91),
                            ..Default::default()
                        }
                    )
                    .is_err()
            );
            assert_eq!(session.compile_settings(), selected);
            session
                .update_compile_options(actor, None, None, None, None, Some(1))
                .unwrap();
            let selected = session.compile_settings();
            assert!(
                session
                    .update_compile_options(actor, Some(lighting), None, None, None, Some(5))
                    .is_err()
            );
            assert_eq!(session.compile_settings(), selected);
            session
                .update_compile_options(actor, None, None, None, Some("detailed"), None)
                .unwrap();
            let settings = session.compile_settings();
            assert_eq!(settings["visibility"], "detailed");
            assert!(
                session
                    .update_compile_options(
                        actor,
                        Some(lighting),
                        Some("engine"),
                        Some("detailed"),
                        Some("invalid"),
                        None
                    )
                    .is_err()
            );
            assert_eq!(session.compile_settings(), settings);
            let result = session
                .update_compile_settings_quality(actor, None, None, Some("balanced"))
                .unwrap();
            assert_eq!(result["sky_quality"], "balanced");
            assert!(
                session
                    .update_compile_settings_quality(actor, Some(lighting), None, Some("unknown"))
                    .is_err()
            );
            assert_eq!(session.compile_settings()["lighting"], "fullbright");
            assert_eq!(session.compile_settings()["sky_quality"], "balanced");
            assert_eq!(
                session.compile_settings()["bounce_choices"]
                    .as_array()
                    .unwrap()
                    .len(),
                5
            );
            for count in [2, 3, 4, 1] {
                session
                    .update_compile_options(actor, None, None, None, None, Some(count))
                    .unwrap();
                assert_eq!(session.compile_settings()["bounces"], count);
            }
            let result = session.set_compile_settings(actor, lighting).unwrap();
            assert_eq!(result["lighting"], lighting);
            assert!(
                session
                    .update_compile_settings(actor, Some("fullbright"), Some("unknown"))
                    .is_err()
            );
            assert_eq!(session.compile_settings()["lighting"], lighting);
            let result = session
                .update_compile_settings(actor, None, Some("engine"))
                .unwrap();
            assert_eq!(result["physics"], "engine");
            session
                .update_compile_settings(actor, None, Some("none"))
                .unwrap();
            assert!(session.set_compile_settings(actor, "unsupported").is_err());
            assert_eq!(session.document.vmf().write(&Default::default()), before);
            assert_eq!(session.document.revision(), revision);
            let request = session.begin_compile(actor, None).unwrap();
            assert_eq!(
                session.compile_status()["job"]["settings"]["smoothing_angle"],
                30
            );
            assert!(
                session
                    .update_build_settings(
                        actor,
                        BuildSettingsUpdate {
                            smoothing_angle: Some(0),
                            ..Default::default()
                        }
                    )
                    .is_err()
            );
            assert_eq!(session.compile_status()["job"]["settings"]["bounces"], 1);
            assert!(
                session
                    .update_compile_options(actor, None, None, None, None, Some(0))
                    .is_err()
            );
            assert_eq!(
                session.compile_status()["job"]["settings"]["visibility"],
                "detailed"
            );
            assert!(
                session
                    .update_compile_options(actor, None, None, None, Some("fast"), None)
                    .is_err()
            );
            assert_eq!(
                session.compile_status()["job"]["settings"]["lighting"],
                lighting
            );
            assert_eq!(session.compile_settings()["editable"], false);
            assert_eq!(
                session.compile_status()["job"]["settings"]["sky_quality"],
                "balanced"
            );
            assert!(
                session
                    .update_compile_settings_quality(actor, None, None, Some("detailed"))
                    .is_err()
            );
            assert!(session.set_compile_settings(actor, "fullbright").is_err());
            assert!(
                session
                    .update_compile_settings(actor, None, Some("engine"))
                    .is_err()
            );
            session
                .cancel_compile(actor, session.compile_token().unwrap())
                .unwrap();
            let result = request.run();
            session.finish_compile(actor, &result);
            assert!(result.is_err());
            assert_eq!(session.compile_settings()["editable"], true);
        }
    }

    #[test]
    fn last_build_is_stale_only_while_lighting_settings_differ() {
        let (directory, game, _) = crate::test_assets::game_process(true);
        let mut session = Session::new(Ok(game));
        session.replace_document(
            Document::new(crate::test_assets::room()),
            Some(directory.path().join("room.vmf")),
        );
        let result = session.begin_compile(Actor::User, None).unwrap().run();
        session.finish_compile(Actor::User, &result);
        assert!(session.launch_readiness().is_ok());
        session
            .update_build_settings(
                Actor::Agent,
                BuildSettingsUpdate {
                    smoothing_angle: Some(45),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(session.launch_readiness().is_err());
        session
            .update_build_settings(
                Actor::User,
                BuildSettingsUpdate {
                    smoothing_angle: Some(0),
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(session.launch_readiness().is_ok());
        session
            .update_compile_options(Actor::Agent, None, None, None, None, Some(1))
            .unwrap();
        assert!(session.launch_readiness().is_err());
        session
            .update_compile_options(Actor::User, None, None, None, None, Some(0))
            .unwrap();
        assert!(session.launch_readiness().is_ok());
        session
            .update_compile_options(Actor::Agent, None, None, None, Some("detailed"), None)
            .unwrap();
        assert!(session.launch_readiness().is_err());
        session
            .update_compile_options(Actor::User, None, None, None, Some("fast"), None)
            .unwrap();
        assert!(session.launch_readiness().is_ok());
        session
            .update_compile_settings(Actor::Agent, None, Some("engine"))
            .unwrap();
        assert_eq!(session.compile_status()["job"]["stale"], true);
        assert!(session.launch_readiness().is_err());
        session
            .update_compile_settings(Actor::User, None, Some("none"))
            .unwrap();
        assert!(session.launch_readiness().is_ok());
        let before = session.document.vmf().write(&Default::default());
        session
            .set_compile_settings(Actor::Agent, "direct_ldr")
            .unwrap();
        assert_eq!(session.compile_status()["job"]["stale"], true);
        assert!(
            session
                .launch_readiness()
                .unwrap_err()
                .contains("Build settings changed")
        );
        session
            .set_compile_settings(Actor::User, "fullbright")
            .unwrap();
        assert_eq!(session.compile_status()["job"]["stale"], false);
        assert!(session.launch_readiness().is_ok());
        session
            .set_compile_settings(Actor::User, "direct_ldr")
            .unwrap();
        let result = session.begin_compile(Actor::Agent, None).unwrap().run();
        session.finish_compile(Actor::Agent, &result);
        assert!(result.as_ref().unwrap().output.bsp.is_some());
        assert!(session.launch_readiness().is_ok());
        assert_eq!(
            session.compile_status()["job"]["result"]["stats"]["lighting_mode"],
            "direct_ldr"
        );
        assert!(
            session.compile_status()["job"]["result"]["stats"]["lighting_faces"]
                .as_u64()
                .unwrap()
                > 0
        );
        session
            .set_compile_settings(Actor::Agent, "direct_ldr_hdr")
            .unwrap();
        assert_eq!(session.compile_status()["job"]["stale"], true);
        assert!(session.launch_readiness().is_err());
        session
            .set_compile_settings(Actor::User, "direct_ldr")
            .unwrap();
        assert_eq!(session.compile_status()["job"]["stale"], false);
        assert!(session.launch_readiness().is_ok());
        assert_eq!(session.document.vmf().write(&Default::default()), before);
    }
}
