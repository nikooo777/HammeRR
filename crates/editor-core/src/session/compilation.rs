use super::*;
use serde_json::{Value, json};
use std::sync::Mutex;
use std::time::{Duration, Instant};

mod ownership;
pub(crate) use ownership::Origin as CompileOrigin;
use ownership::Ticket;

impl State {
    pub(super) fn close_work(&self) -> serde_json::Value {
        serde_json::json!({
            "sequence": self.serial,
            "job": self.job.as_ref().map(|job| serde_json::json!({
                "token": job.token, "status": job.status(),
                "active": job.origin.busy(), "cancelled": job.control.cancelled(),
            }))
        })
    }
}

#[cfg(test)]
mod ownership_tests;

#[cfg(test)]
mod instance_dependency_tests;
#[cfg(test)]
mod instance_reflection_tests;
#[cfg(test)]
mod instance_rejection_tests;
#[cfg(test)]
mod lighting_work_settings_tests;
#[cfg(test)]
mod prop_vertex_lighting_settings_tests;
mod recipes;
#[cfg(test)]
mod reflection_settings_tests;
#[cfg(test)]
mod reflection_transaction_tests;
#[cfg(test)]
mod sky_packing_settings_tests;
#[cfg(test)]
mod static_prop_packing_settings_tests;
#[cfg(test)]
mod static_prop_source_tests;
#[cfg(test)]
mod viscluster_tests;
#[cfg(test)]
mod water_packing_settings_tests;

#[derive(Default)]
pub struct State {
    pub(super) retained_pointfile: Option<crate::path_claims::Claim>,
    serial: u64,
    job: Option<Job>,
    options: compiler::Options,
    recipes: recipes::State,
}

struct Job {
    queue: Option<crate::runtime::QueueStatus>,
    origin: CompileOrigin,
    token: u64,
    control: compiler::Control,
    path: PathBuf,
    source_path: Option<PathBuf>,
    inspection_source: Result<Option<compiler::FileIdentity>, String>,
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
    instance_dependencies: Option<compiler::instances::DependencyGuard>,
    static_prop_sources: Option<StaticPropSources>,
    guarded: bool,
}

pub struct CompileRequest {
    #[cfg(test)]
    before_preparation: Option<Box<dyn FnOnce() + Send>>,
    #[cfg(test)]
    after_compilation: Option<CompilationObserver>,
    #[cfg(test)]
    capture: Option<CaptureBackend>,
    native_plan: Option<NativeCapturePlan>,
    output_claims: crate::path_claims::OutputClaims,
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
    instance_source: Option<compiler::instances::SourceContext>,
    pointfile_stamp: Option<compiler::FileStamp>,
    destination_stamp: Option<compiler::FileStamp>,
    origin: CompileOrigin,
    ticket: Option<Ticket>,
}

#[cfg(test)]
type CompilationObserver = Box<dyn Fn(&compiler::Output) + Send>;

#[cfg(test)]
pub(crate) type CaptureBackend = Box<
    dyn FnMut(
            native_runtime::CaptureRequest,
            &mut dyn native_runtime::CaptureObserver,
        ) -> Result<native_runtime::CaptureReceipt, native_runtime::CaptureFailure>
        + Send,
>;

struct NativeCapturePlan {
    client: Option<crate::runtime::native_activity::Client>,
    capture: crate::runtime::native_activity::Target,
    play: Option<crate::runtime::native_activity::Target>,
}

pub struct CompileResult {
    origin: CompileOrigin,
    token: u64,
    pub output: compiler::Output,
    pub path: PathBuf,
    generation: u64,
    revision: u64,
    options: compiler::Options,
    content_epoch: u64,
    static_prop_sources: Option<StaticPropSources>,
    source: Option<compiler::FileIdentity>,
    destination_stamp: Option<compiler::FileStamp>,
    control: compiler::Control,
    publication: Mutex<Publication>,
}

struct Publication {
    #[cfg(test)]
    before_cleanup: Option<Box<dyn FnOnce() + Send>>,
    pending: Option<compiler::PreparedWrite>,
    claims: Option<crate::path_claims::OutputClaims>,
    artifact: Option<crate::path_claims::Claim>,
    warning: Option<String>,
    lease: Option<compiler::OutputLease>,
    pointfile_lease: Option<compiler::OutputLease>,
    pointfile_stamp: Option<compiler::FileStamp>,
    sources: Option<compiler::asset_sources::OutputAssetSourceGuard>,
    published: bool,
    committed: bool,
    error: Option<String>,
    ticket: Option<Ticket>,
}

#[cfg(test)]
impl Drop for Publication {
    fn drop(&mut self) {
        if let Some(before_cleanup) = self.before_cleanup.take() {
            before_cleanup();
        }
    }
}

impl State {
    pub(super) fn busy(&self, pending: bool) -> bool {
        self.job.as_ref().map_or(pending, |job| {
            job.origin.busy() || (pending && job.origin.terminal().is_none())
        })
    }
}

impl Job {
    fn status(&self) -> &'static str {
        if self.finished.is_none()
            && self.terminal().is_none()
            && self
                .queue
                .as_ref()
                .is_some_and(crate::runtime::QueueStatus::queued)
        {
            return "queued";
        }
        self.terminal()
            .map_or(self.status, |terminal| terminal.status)
    }

    fn terminal(&self) -> Option<&ownership::Terminal> {
        if self.finished.is_some() {
            None
        } else {
            self.origin.terminal()
        }
    }

    fn elapsed(&self) -> Duration {
        self.finished
            .or_else(|| self.terminal().map(|terminal| terminal.elapsed))
            .unwrap_or_else(|| self.started.elapsed())
    }

    fn result(&self) -> Option<Value> {
        self.terminal().map_or_else(
            || self.result.clone(),
            |terminal| Some(json!({"success":false,"error":terminal.error})),
        )
    }
}

impl CompileResult {
    #[cfg(test)]
    pub(crate) fn before_cleanup(&self, hook: impl FnOnce() + Send + 'static) {
        self.publication
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .before_cleanup = Some(Box::new(hook));
    }
    fn take_artifact_claim(&self) -> Option<crate::path_claims::Claim> {
        self.publication
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .artifact
            .take()
    }

    fn prop_lighting_guard(&self) -> Option<compiler::asset_sources::AssetSourceGuard> {
        self.publication
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .sources
            .as_ref()
            .and_then(compiler::asset_sources::OutputAssetSourceGuard::prop_lighting_guard)
    }

    fn publication_warning(&self) -> Option<String> {
        self.publication
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .warning
            .clone()
    }

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
    #[cfg(test)]
    pub(crate) fn capture_with(&mut self, capture: CaptureBackend) {
        self.capture = Some(capture);
    }
    pub(crate) fn freeze_native_play(&mut self, install: Option<&game::Install>) {
        if let Some(plan) = &mut self.native_plan {
            plan.play = install.map(|install| install.target().clone());
        }
    }
    pub(crate) fn control(&self) -> compiler::Control {
        self.control.clone()
    }

    pub(crate) fn origin(&self) -> CompileOrigin {
        self.origin.clone()
    }

    pub fn run(self) -> Result<CompileResult, String> {
        self.run_with(Self::run_inner)
    }

    pub(crate) fn run_admitted(
        self,
    ) -> (
        Result<CompileResult, String>,
        Option<crate::runtime::native_activity::Permit>,
    ) {
        let mut native = None;
        let result = self.run_controlled(Self::run_inner_admitted, &mut native);
        (result, native)
    }

    fn run_with(
        self,
        run: impl FnOnce(Self) -> Result<CompileResult, String>,
    ) -> Result<CompileResult, String> {
        self.run_controlled(|request, _| run(request), &mut None)
    }

    fn run_controlled(
        mut self,
        run: impl FnOnce(
            Self,
            &mut Option<crate::runtime::native_activity::Permit>,
        ) -> Result<CompileResult, String>,
        native: &mut Option<crate::runtime::native_activity::Permit>,
    ) -> Result<CompileResult, String> {
        let mut ticket = self.ticket.take().expect("compile request owns its ticket");
        ticket.start();
        let control = self.control.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run(self, native)))
            .unwrap_or_else(|_| Err("Compilation worker panicked".into()));
        match result {
            Ok(mut result) => {
                result
                    .publication
                    .get_mut()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .ticket = Some(ticket);
                Ok(result)
            }
            Err(error) => {
                ticket.fail(error.clone(), control.cancelled());
                Err(error)
            }
        }
    }

    fn run_inner(self) -> Result<CompileResult, String> {
        if self
            .native_plan
            .as_ref()
            .is_some_and(|plan| plan.client.is_some())
        {
            return Err("Managed reflection capture requires the shared compile submission".into());
        }
        self.run_inner_admitted(&mut None)
    }

    fn run_inner_admitted(
        self,
        native: &mut Option<crate::runtime::native_activity::Permit>,
    ) -> Result<CompileResult, String> {
        self.control.checkpoint()?;
        *native = if let Some(plan) = &self.native_plan {
            plan.capture.verify()?;
            plan.client
                .as_ref()
                .map(|client| {
                    client.reserve(
                        crate::runtime::native_activity::Phase::Capture,
                        Some(&plan.capture),
                        plan.play.as_ref(),
                    )
                })
                .transpose()?
        } else {
            None
        };
        let instances = compiler::instances::requires_source(&self.vmf);
        let guarded = self.options.requires_guarded_publication_for(&self.vmf);
        let mut lease = None;
        let mut source = None;
        let mut pointfile_lease = None;
        if self.options.generate_reflections {
            native_runtime::preflight_vmf(&self.vmf).map_err(|e| e.to_string())?;
        }
        if guarded {
            if !instances {
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
            } else {
                self.instance_source
                    .as_ref()
                    .ok_or("Instance source context missing")?
                    .verify(&self.control)
                    .map_err(|error| error.to_string())?;
            }
            let stamp = self
                .destination_stamp
                .as_ref()
                .ok_or("Guarded destination snapshot missing")?;
            stamp.verify_metadata().map_err(|e| e.to_string())?;
            lease = Some(
                compiler::OutputLease::acquire(&self.path, &self.control)
                    .map_err(|e| e.to_string())?,
            );
            stamp
                .verify_lease(lease.as_ref().ok_or("Guarded output lease missing")?)
                .map_err(|e| e.to_string())?;
            if let Some(stamp) = &self.pointfile_stamp {
                stamp.verify_metadata().map_err(|error| error.to_string())?;
            }
            pointfile_lease = Some(
                compiler::OutputLease::acquire(
                    self.output_claims.pointfile.destination().physical_path(),
                    &self.control,
                )
                .map_err(|error| error.to_string())?,
            );
            if let Some(stamp) = &self.pointfile_stamp {
                stamp
                    .verify_lease(
                        pointfile_lease
                            .as_ref()
                            .ok_or("Guarded pointfile lease missing")?,
                    )
                    .map_err(|error| error.to_string())?;
            }
        }
        let path = lease
            .as_ref()
            .map(|lease| lease.path().to_owned())
            .unwrap_or_else(|| self.path.clone());
        let reservation = compiler::WriteReservation::for_destinations(
            self.output_claims.bsp.destination(),
            self.output_claims.pointfile.destination(),
            &self.control,
        )
        .map_err(|error| error.to_string())?;
        let mut output = if instances {
            compiler::compile_with_source_controlled(
                &self.vmf,
                &self.game,
                self.options,
                self.context.as_ref(),
                self.instance_source.as_ref(),
                &self.control,
            )
        } else {
            compiler::compile_with_context_controlled(
                &self.vmf,
                &self.game,
                self.options,
                self.context.as_ref(),
                &self.control,
            )
        };
        #[cfg(test)]
        if let Some(hook) = &self.after_compilation {
            hook(&output);
        }
        if let Err(error) = self.control.checkpoint() {
            return self.cancelled_output(output, path, source, error);
        }
        if instances
            && (output.bsp.is_some() || output.leak.is_some())
            && output.instance_dependencies.is_none()
        {
            return Err("Instance output is missing its retained dependency guard".into());
        }
        if self.options.generate_reflections && output.bsp.is_some() {
            #[cfg(test)]
            let mut backend = self.capture;
            output = compiler::finalize_reflections_with_capture(
                output,
                self.context
                    .as_ref()
                    .ok_or("Reflection compile context missing")?,
                &self.game,
                self.environment
                    .as_ref()
                    .ok_or("Native capture environment missing")?,
                &self.control,
                |request, observer| {
                    self.control.checkpoint().map_err(std::io::Error::other)?;
                    if let Some(plan) = &self.native_plan {
                        plan.capture.verify().map_err(std::io::Error::other)?;
                    }
                    if let Some(permit) = native.as_mut() {
                        permit.started();
                    }
                    #[cfg(test)]
                    let result = match backend.as_mut() {
                        Some(capture) => capture(request, observer),
                        None => native_runtime::capture_with_cleanup(request, observer),
                    };
                    #[cfg(not(test))]
                    let result = native_runtime::capture_with_cleanup(request, observer);
                    if let Some(permit) = native.as_mut() {
                        match &result {
                            Ok(_) => permit.confirmed_clean(),
                            Err(failure)
                                if failure.cleanup
                                    == native_runtime::CleanupDisposition::Confirmed =>
                            {
                                permit.confirmed_clean()
                            }
                            Err(failure) => permit.failed_cleanup(&failure.to_string()),
                        }
                    }
                    result.map_err(|failure| failure.error)
                },
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
        if self.options.pack_sky_assets && output.bsp.is_some() && output.sky_assets.is_none() {
            return Err("Sky packing did not return its retained asset plan".into());
        }
        if self.options.pack_water_assets && output.bsp.is_some() && output.water_assets.is_none() {
            return Err("Liquid packing did not return its retained asset plan".into());
        }
        if self.options.pack_static_prop_assets
            && output.bsp.is_some()
            && output.static_prop_assets.is_none()
        {
            return Err("Static-prop packing did not return its retained asset plan".into());
        }
        #[cfg(test)]
        if let Some(hook) = self.before_preparation {
            hook();
        }
        let mut sources = None;
        let (pending, published, committed, claims, artifact, warning) = if guarded {
            if output.bsp.is_none() && output.leak.is_some() {
                if let Some(stamp) = &self.pointfile_stamp {
                    stamp.verify_metadata().map_err(|error| error.to_string())?;
                }
                if let Some(stamp) = &self.pointfile_stamp {
                    stamp
                        .verify_lease(
                            pointfile_lease
                                .as_ref()
                                .ok_or("Guarded pointfile lease missing")?,
                        )
                        .map_err(|error| error.to_string())?;
                }
            }
            let pending = output
                .prepare_write_reserved(reservation, &self.control)
                .map_err(|e| e.to_string())?;
            sources = Some(
                compiler::asset_sources::capture_output_asset_source_guard(
                    &output,
                    &self.game,
                    &self.control,
                )
                .map_err(|e| e.to_string())?,
            );
            if let Some(source) = &source {
                source.verify(&self.control).map_err(|e| e.to_string())?;
            }
            if let Some(dependencies) = &output.instance_dependencies {
                dependencies
                    .verify(&self.control)
                    .map_err(|error| error.to_string())?;
            }
            lease
                .as_ref()
                .ok_or("Guarded destination lease missing")?
                .verify(&self.control)
                .map_err(|e| e.to_string())?;
            let claims = pending.as_ref().map(|_| self.output_claims);
            (pending, false, false, claims, None, None)
        } else {
            let pending = output
                .prepare_write_reserved(reservation, &self.control)
                .map_err(|error| format!("{}: {error}", self.path.display()))?;
            match pending {
                Some(pending) => {
                    let receipt = pending
                        .commit_receipt(&self.control)
                        .map_err(|error| format!("{}: {error}", self.path.display()))?;
                    let claim = if receipt.success {
                        self.output_claims.bsp.clone()
                    } else {
                        self.output_claims.pointfile.clone()
                    };
                    claim.published(receipt.publication.destination);
                    (
                        None,
                        receipt.success,
                        true,
                        None,
                        Some(claim),
                        receipt.publication.warning,
                    )
                }
                None => (None, false, false, None, None, None),
            }
        };
        let static_prop_sources = sources
            .as_ref()
            .and_then(compiler::asset_sources::OutputAssetSourceGuard::static_prop_guard)
            .map(|guard| StaticPropSources {
                guard,
                game: self.game.clone(),
                content_epoch: self.content_epoch,
            });
        Ok(CompileResult {
            origin: self.origin,
            token: self.token,
            output,
            path,
            generation: self.generation,
            revision: self.revision,
            options: self.options,
            content_epoch: self.content_epoch,
            static_prop_sources,
            source,
            destination_stamp: self.destination_stamp,
            control: self.control,
            publication: Mutex::new(Publication {
                #[cfg(test)]
                before_cleanup: None,
                claims,
                artifact,
                warning,
                pending,
                lease,
                pointfile_lease,
                pointfile_stamp: self.pointfile_stamp,
                sources,
                published,
                committed,
                error: None,
                ticket: None,
            }),
        })
    }

    fn cancelled_output(
        self,
        mut output: compiler::Output,
        path: PathBuf,
        source: Option<compiler::FileIdentity>,
        error: String,
    ) -> Result<CompileResult, String> {
        if output.stats.lighting_progress.is_none() {
            return Err(error);
        }
        output.bsp = None;
        output.leak = None;
        output.cubemap_assets = None;
        output.sky_assets = None;
        output.water_assets = None;
        output.static_prop_assets = None;
        output.prop_lighting_assets = None;
        Ok(CompileResult {
            origin: self.origin,
            token: self.token,
            output,
            path,
            generation: self.generation,
            revision: self.revision,
            options: self.options,
            content_epoch: self.content_epoch,
            static_prop_sources: None,
            source,
            destination_stamp: self.destination_stamp,
            control: self.control,
            publication: Mutex::new(Publication {
                #[cfg(test)]
                before_cleanup: None,
                pending: None,
                claims: None,
                artifact: None,
                warning: None,
                lease: None,
                pointfile_lease: None,
                pointfile_stamp: None,
                sources: None,
                published: false,
                committed: false,
                error: Some(error),
                ticket: None,
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
        "warning":result.publication_warning(),
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
    pub lighting_work_limit: Option<u64>,
    pub generate_reflections: Option<bool>,
    pub pack_sky_assets: Option<bool>,
    pub pack_water_assets: Option<bool>,
    pub pack_static_prop_assets: Option<bool>,
    pub bake_static_prop_vertices: Option<bool>,
    pub brush_texture_shadows: Option<bool>,
    pub refine_ambient_probes: Option<bool>,
}

impl Session {
    pub(super) fn instance_dependency_issue(
        &self,
        dependencies: Option<&compiler::instances::DependencyGuard>,
        source_path: &Option<PathBuf>,
    ) -> Option<String> {
        let dependencies = dependencies?;
        if source_path != &self.path {
            return Some("The source map path changed since instance compilation".into());
        }
        dependencies
            .verify_metadata()
            .err()
            .map(|error| error.to_string())
    }

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
        let mut settings = json!({"pack_sky_assets":self.compilation.options.pack_sky_assets,"sky_packing_description":"Pack the complete exact authored sky family, original materials, Patch dependencies and supported LDR/HDR textures during Compile or Run Map. Default off. Incomplete or unsupported families fail without publication or automatic family fallback. Does not pack unrelated assets or select the game's rendering channel.","generate_reflections":self.compilation.options.generate_reflections,"reflection_description":"Generate one native reflection bounce during Compile or Run Map. Direct LDR captures LDR; Direct LDR + HDR captures and verifies both channels before publication. Requires a verified CS:S capture runtime. Fullbright generation is unsupported.","smoothing_angle":self.compilation.options.smoothing_angle,"smoothing_angle_range":{"min":0,"max":90,"unit":"degrees","disabled":0},"bounces":self.compilation.options.bounces,"bounce_choices":bounce_choices,"visibility":self.compilation.options.visibility.name(), "visibility_choices":visibility_choices, "sky_quality":self.compilation.options.sky_quality.name(),"sky_choices":sky_choices,"lighting":self.compilation.options.lighting.name(),"physics":self.compilation.options.physics.name(),
            "recipe":self.active_build_recipe(),"editable":!self.compiling() && !self.closing,"choices":choices,"physics_choices":physics_choices});
        let unavailable_reason = if !self.compilation.options.lighting.performs_lighting() {
            Some(
                "Choose Direct LDR or Direct LDR + HDR before enabling static-prop vertex lighting.",
            )
        } else {
            None
        };
        settings["lighting_work_limit"] = self.compilation.options.lighting_work_limit.into();
        settings["light_value_bindings"] = instance_reference::light_value_binding_scope();
        settings["material_emission_description"] = "World settings → Emission file (_hammerr_emission_file) selects an optional mounted relative declaration file; blank or Remove override turns emission off. File contents and source assets validate at compile time. Sources are drawn opaque world/detail faces or eligible stationary func_brush models with Shadows Yes, using their compiled initial pose. Moving, unrequested and MDL model sources are unavailable. Finite-area sources use sampled baked lighting; dynamic receivers use indexed ambient lighting only. No native emission records or shader glow. Direct LDR + HDR with emitting faces requires a real authored point, spot or environment world light; zero brightness is allowed.".into();
        settings["lighting_work_limit_range"] = json!({
            "min":compiler::MIN_LIGHTING_WORK_LIMIT,
            "max":compiler::MAX_LIGHTING_WORK_LIMIT,
            "default":compiler::DEFAULT_LIGHTING_WORK_LIMIT,
            "unit":"work_units"
        });
        settings["lighting_work_description"] = "One aggregate allowance for preparation, LDR and HDR lighting. Raising it preserves sampling quality and all independent allocation, sample, light, probe and geometry limits.".into();
        settings["bake_static_prop_vertices"] =
            self.compilation.options.bake_static_prop_vertices.into();
        settings["brush_model_shadowing"] = json!({
            "available":self.compilation.options.lighting.performs_lighting(),
            "property":"vrad_brush_cast_shadows","enabled_value":"1","default_value":"0",
            "fullbright":"admit_and_preserve_without_baking",
            "baked_count":"stats.brush_shadow_models",
            "description":"Advanced properties → Shadows → Yes requests global baked shadows from an unnamed, unparented stationary func_brush. Opaque root and child requests keep their ordinary SOLID material scope. Explicit root and child requests may also use the supported plain LightmappedGeneric binary cutout material when Brush texture shadows is enabled in a lit mode. Child cutouts without a global request use only their own applicable baked receiver scope. Root requests use an explicit finite Origin (X Y Z). Child models may instead resolve and consume one origin volume before the ordinary body check. Root and child requests validate before region omission; Fullbright reports admitted opaque source requests without baking, including requests omitted by compile regions. Admitted child initial poses are baked before containing transforms; emitted child angles are zero. Model surfaces never supply reflected radiance.",
            "limits":{
                "classname":"func_brush","origin":"resolved_finite",
                "root_origin":"explicit_finite","child_origin":"explicit_or_one_consumed_volume",
                "runtime_names_parenting_io":false,"source_materials":"resolved_opaque_ordinary_solid_or_opted_in_requested_binary_cutout",
                "material_shaders":["LightmappedGeneric","UnlitGeneric","Patch"],
                "patch_form":"unambiguous_scalar_include_edit",
                "root_binary_cutout_requires":"brush_texture_shadows_and_lit_mode",
                "child_binary_cutout":true,"child_binary_cutout_requires":"canonical_request_and_brush_texture_shadows_and_lit_mode","fractional_translucent_additive":false,"terrain_liquid_origin_volumes":false,
                "child_origin_volumes":"one_resolved_consumed_before_body_validation",
                "radiance_source":false,"dynamic_relighting":false,
                "geometry_collision_pvs_probe_placement":"unchanged",
                "child_refresh":"source_keys_uv_world_bounds_checked_effective_pivot_initial_pose_local_bounds_materials_deferred",
                "lighting_work_limit":"shared_aggregate"
            }
        });
        settings["refine_ambient_probes"] = self.compilation.options.refine_ambient_probes.into();
        let refinement_available = self.compilation.options.lighting.performs_lighting()
            && self.compilation.options.sky_quality != compiler::SkyQuality::Fast;
        settings["ambient_probe_refinement"] = json!({
            "available":refinement_available,
            "unavailable_reason":if refinement_available { None } else { Some("Choose Direct LDR or Direct LDR + HDR and Balanced or Detailed sampling.") },
            "description":"Tries more positions for ambient lighting samples while keeping the sample count and spacing. Evaluates up to twice as many positions and can take longer.",
            "limits":{"lighting_modes":["direct_ldr","direct_ldr_hdr"],"sampling":["balanced","detailed"],
                "probe_cap":{"balanced":4,"detailed":8},"candidate_gather_factor_max":2,
                "replaced_slots_max":1,"uniform_and_ties":"retain_baseline","work":"shared_checked_budget",
                "quality_guarantee":false,"native_pixel_qualification":false}
        });
        settings["brush_texture_shadows"] = self.compilation.options.brush_texture_shadows.into();
        settings["brush_texture_shadowing"] = json!({
            "available":self.compilation.options.lighting.performs_lighting(),
            "unavailable_reason":if self.compilation.options.lighting.performs_lighting() { None } else { Some("Choose Direct LDR or Direct LDR + HDR before enabling brush texture shadows.") },
            "description":"Use binary base-texture alpha on plain LightmappedGeneric world/detail faces and admitted initial-pose brush-model self shadows. An eligible stationary root or child func_brush with Advanced properties → Shadows → Yes also casts these cutouts into global baked lighting, including world and other-model receivers. Child effective origins and source initial poses lower before containing transforms. Unrequested root and child model cutouts stay within their own lit receiver scope. Absent or zero global requests never cast into world or other-model lighting. Model surfaces never supply reflected radiance. Default off; independent of native model-requested prop texture shadows. Geometry, collision and sealing remain unchanged. Requested unsupported materials, model scopes or missing textures stop publication.",
            "limits":{
                "lighting_modes":["direct_ldr","direct_ldr_hdr"],
                "casters":["world","func_detail","brush_model_self","child_brush_model_self","requested_stationary_root_func_brush","requested_stationary_child_func_brush"],
                "model_scope":{
                    "receivers":"same_model_only",
                    "pose":"existing_admitted_initial_pose",
                    "nonzero_rotation":{"classname":"func_brush","parented":false},
                    "requires_lit_receiver_scope":true,
                    "casts_into_world":false,"casts_into_other_models":false,"radiance_source":false
                },
                "child_self_model_scope":{
                    "classname":"func_brush","receivers":"same_model_only",
                    "pose":"lowered_before_containing_transforms",
                    "requires_brush_texture_shadows":true,"requires_lit_mode":true,
                    "requires_lit_receiver_scope":true,"global_request_required":false,
                    "request_field":"vrad_brush_cast_shadows","non_global_values":["absent","0"],
                    "malformed_request":"reject",
                    "casts_into_world":false,"casts_into_other_models":false,"radiance_source":false
                },
                "global_root_model_scope":{
                    "property":"vrad_brush_cast_shadows","enabled_value":"1",
                    "requires_brush_texture_shadows":true,"requires_lit_mode":true,
                    "classname":"func_brush","pose":"existing_admitted_stationary_initial_pose",
                    "requires_lit_receiver_scope":false,"child_models":false,
                    "casts_into_world":true,"casts_into_other_models":true,
                    "radiance_source":false,"world_occupancy":false
                },
                "global_child_model_scope":{
                    "property":"vrad_brush_cast_shadows","enabled_value":"1",
                    "requires_brush_texture_shadows":true,"requires_lit_mode":true,
                    "classname":"func_brush","pose":"lowered_before_containing_transforms",
                    "origin":"explicit_or_one_consumed_volume","requires_lit_receiver_scope":false,
                    "reference_preparation":"deferred","optionless_compile_inspection":"ordinary_solid_only",
                    "casts_into_world":true,"casts_into_other_models":true,
                    "radiance_source":false,"world_occupancy":false
                },
                "material":"plain LightmappedGeneric",
                "coverage":"binary_alpha_test",
                "mip":0,
                "filter":"nearest",
                "addressing":"vtf_clamp_or_repeat",
                "sides_max":compiler::MAX_BRUSH_SHADOW_SIDES,
                "texture_axis_max":compiler::MAX_BRUSH_SHADOW_TEXTURE_SIZE,
                "alpha_bytes_max":compiler::MAX_BRUSH_SHADOW_ALPHA_BYTES,
                "asset_bytes_max":compiler::MAX_BRUSH_SHADOW_ASSET_BYTES,
                "material_bytes_max":compiler::MAX_BRUSH_SHADOW_MATERIAL_BYTES,
                "material_lookups_max":compiler::MAX_BRUSH_SHADOW_MATERIAL_LOOKUPS,
                "transform_absolute_max":compiler::MAX_BRUSH_SHADOW_TRANSFORM,
                "frames":1,"faces":1,"depth":1,
                "alpha_reference":{"default":0.5,"min":0,"max":1,"zero_uses_default":true},
                "unsupported":["fractional translucency","terrain alpha casters","unadmitted model poses","unrequested cutout models without lit receivers","model radiance sources","conditional or animated materials"]
            }
        });
        settings["pack_water_assets"] = self.compilation.options.pack_water_assets.into();
        settings["pack_static_prop_assets"] =
            self.compilation.options.pack_static_prop_assets.into();
        settings["static_prop_packing_description"] = "Pack original model, companion, all declared skin/LOD materials and basetexture files for emitted static props during Compile or Run Map. Default off; independent of vertex lighting and available in every lighting mode. Requires bounded static MDL 44/48, body 0/root LOD 0, trivial local animation and VertexLitGeneric terminals. Terminals allow only one canonical $basetexture, optionally plus literal $alphatest 1 at the default 0.5 cutoff; authored $alphatestreference is unsupported. Patch chains allow at most eight include edges, with one basetexture insert or replace block per Patch. Deeper edits win within each type; replacements apply after all inserts. All original VMT bytes and every declared VTF remain packed, including overridden textures. Extra keys, combined edit sections, proxies and $envmap reject. Matching PHY is included only for VPhysics props. Missing or unsupported dependencies stop publication. Packing preserves model flags. Compiler texture shadows require the original MDL CAST_TEXTURE_SHADOWS request; without it, enabled prop shadows remain opaque mesh shadows with the existing warning. Selected default-alpha-test vertex-lighting receivers accept the same bounded nested Patch basetexture edits and terminal contract. Every hardware vertex is retained with VHV alpha 255; receiver-only textures are read only if casting or packing needs them. Runtime requires standard CS:S shaders and surface/prop-data definitions with DX90 or newer.".into();
        settings["water_packing_description"] = "Pack original files for complete supported static Water or opaque SLIME material/Patch/texture closures during Compile or Run Map. Default off. SLIME supports static opaque LightmappedGeneric, UnlitGeneric and unambiguous scalar Patch chains with an explicit basetexture and exact native self-bottom. Original base, bump, tool and named LDR/HDR envmap assets remain packed, including overridden references. Explicit runtime Water surface envmaps and runtime SLIME env_cubemap require Generate reflections and Direct LDR + HDR; generated pairs, bound aliases and original inputs verify before publication. Water bottom/underwater-overlay graphs require explicit named static LDR/HDR envmaps; omitted envmaps, runtime dependencies below the surface root and material cycles reject. Runtime Water envmapframe must be absent or zero and Patch chains cannot edit a key in multiple materials. All original declared resources remain packed, including overridden references. Other resources/effects remain unsupported. Static named-envmap packing works in every lighting mode without capture.".into();
        settings["prop_vertex_lighting"] = json!({
            "available":unavailable_reason.is_none(),
            "unavailable_reason":unavailable_reason,
            "description":"Bake direct lighting and up to four diffuse world/terrain bounces on unit-scale static props with one bodypart/model, body0/root0, all 1–8 draw LODs and plain opaque or strict default-alpha-test VertexLitGeneric materials, up to 256 hardware sections total. Direct LDR + HDR evaluates both channels independently. At depth N above zero, prop reception uses world/terrain lighting from depth N-1 and adds direct light once. Indirect reception requires texture-header or authored material reflectivity for every lit world/terrain source. Dimensions independently use diffuse or tool VTF headers, or the SDK no-preview 128x128 convention; every complete VTF actually read for metrics is hashed. Authored $reflectivity accepts one ASCII [r g b] triple of finite linear values from 0 to 1, at most 128 bytes; only trimmed %notooltexture 0 or 1 selects the no-preview convention before dimension reads. Empty or other no-preview values and non-path tool values reject; failed required providers never fall back. disableselfshadowing=1 omits only the receiving prop’s own shadow triangles. lightingorigin preserves a uniquely resolved info_lighting point for native runtime lighting; baked samples, normals and PVS remain at actual vertices, without embedded-vertex recovery. ignorenormals=1 supports zero to four bounces: constant 0.35 point/spot/sun reception, full-sphere sky ambient, and spherical reflected reception with SDK angular normalization. It retains real shadows and finite nonzero model normals. Original vertices may touch opaque world brushes; buried vertices and terrain/other-prop contacts reject. Model/material compatibility is checked during compilation. Unsupported requested receivers stop the build; disablevertexlighting=1 explicitly opts out one prop. No native capture runtime is required.",
            "limits":{
                "lighting_modes":["direct_ldr","direct_ldr_hdr"],
                "bounces":{"min":0,"max":compiler::MAX_LIGHT_BOUNCES},"indirect_sources":["world","terrain"],"indirect_reflectivity":"texture_header_or_authored_material","indirect_dimensions":"header_or_native_no_preview","indirect_dimension_sources":["diffuse_vtf_header","tool_vtf_header","native_no_preview_convention"],"receiver":{"body":0,"root_lod":0,"bodyparts":1,"models":1,"lods":{"min":1,"max":8,"all_required":true},"hardware_meshes_max":compiler::prop_lighting_assets::MAX_RECEIVER_MESHES,"scale":1,"sample_origin":"original_vertex","lighting_origin":{"property":"lightingorigin","baked_samples":"actual_vertices","embedded_recovery":false},"support_contacts":"opaque_world_brush","self_shadowing":{"default":true,"disable_property":"disableselfshadowing","disable_value":1,"disabled_scope":"receiver_owned_triangles","retained_blockers":["other_props","world","terrain"]},"ignore_normals":{"property":"ignorenormals","enabled_value":1,"bounces":{"min":0,"max":compiler::MAX_LIGHT_BOUNCES},"direct_factor":compiler::lighting::IGNORE_NORMALS_FACTOR,"sky_ambient":"full_sphere_visibility_mean","indirect":"spherical_sdk_normalization","keeps_shadows":true}},
                "material":"plain opaque or strict default-alpha-test selected VertexLitGeneric",
                "cutout_material":{"terminal_parameters":{"$basetexture":"canonical_path","$alphatest":"1"},"alpha_test_reference":0.5,"authored_reference":false,"include_edges_max":8,"patch_edit":"one_basetexture_insert_or_replace_per_patch","extra_parameters":false,"selected_skin_only":true,"all_hardware_vertices":true,"vhv_alpha":255,"receiver_texture_read":false,"texture_shadow_request":"original_mdl_flag","packing_required":false},
                "light_styles":[0],
                "unsupported":["buried vertices","terrain/other-prop support contacts","specialized materials","per-texel prop lightmaps","reflected prop sources"]
            }
        });
        settings
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
        self.update_build_settings_inner(actor, update, true)
    }

    pub(super) fn update_build_settings_inner(
        &mut self,
        actor: Actor,
        update: BuildSettingsUpdate<'_>,
        remember: bool,
    ) -> Result<Value, String> {
        let BuildSettingsUpdate {
            lighting,
            physics,
            sky_quality,
            visibility,
            bounces,
            smoothing_angle,
            lighting_work_limit,
            generate_reflections,
            pack_sky_assets,
            pack_water_assets,
            pack_static_prop_assets,
            bake_static_prop_vertices,
            brush_texture_shadows,
            refine_ambient_probes,
        } = update;
        let bounces = bounces.unwrap_or(self.compilation.options.bounces);
        let options = compiler::Options {
            refine_ambient_probes: refine_ambient_probes
                .unwrap_or(self.compilation.options.refine_ambient_probes),
            brush_texture_shadows: brush_texture_shadows
                .unwrap_or(self.compilation.options.brush_texture_shadows),
            bake_static_prop_vertices: bake_static_prop_vertices
                .unwrap_or(self.compilation.options.bake_static_prop_vertices),
            pack_sky_assets: pack_sky_assets.unwrap_or(self.compilation.options.pack_sky_assets),
            pack_water_assets: pack_water_assets
                .unwrap_or(self.compilation.options.pack_water_assets),
            pack_static_prop_assets: pack_static_prop_assets
                .unwrap_or(self.compilation.options.pack_static_prop_assets),
            generate_reflections: generate_reflections
                .unwrap_or(self.compilation.options.generate_reflections),
            bounces,
            smoothing_angle: smoothing_angle.unwrap_or(self.compilation.options.smoothing_angle),
            lighting_work_limit: lighting_work_limit
                .unwrap_or(self.compilation.options.lighting_work_limit),
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
        if self.compiling() {
            return Err(
                "Wait for the active compile to finish before changing build settings".into(),
            );
        }
        if options != self.compilation.options {
            if options.brush_texture_shadows != self.compilation.options.brush_texture_shadows {
                self.cubemap_bindings.texture_shadows_changed();
            }
            self.compilation.options = options;
            self.compilation.recipes.set_active(None);
            self.compilation.recipes.changed_local();
            if remember {
                self.remember_current_map_context();
            }
            self.log(
                actor,
                format!(
                    "Build settings: {} · {} · {} visibility · {} diffuse bounces · {}° brush smoothing · {} lighting work units · ambient probe refinement {} · brush texture shadows {} · static-prop vertex lighting {} · reflections {} · sky packing {} · liquid packing {} · static-prop packing {}",
                    options.lighting.label(),
                    options.physics.label(),
                    options.visibility.label(),
                    options.bounces,
                    options.smoothing_angle,
                    options.lighting_work_limit,
                    if options.refine_ambient_probes { "on" } else { "off" },
                    if options.brush_texture_shadows { "on" } else { "off" },
                    if options.bake_static_prop_vertices { "on" } else { "off" },
                    if options.generate_reflections { "on" } else { "off" },
                    if options.pack_sky_assets { "on" } else { "off" },
                    if options.pack_water_assets { "on" } else { "off" },
                    if options.pack_static_prop_assets { "on" } else { "off" }
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
        self.ensure_entity_placement_resolved()?;
        if self.closing {
            return Err("the editor is closing".into());
        }
        if self.compiling() {
            return Err("a map compilation is already running".into());
        }
        if self.launching() {
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
        if let Some(claim) = &self.file_claims.source {
            claim.destination().verify().map_err(|error| {
                format!("Map source binding changed; reopen or Save As before compiling: {error}")
            })?;
        }
        let control = compiler::Control::default();
        let instances = compiler::instances::requires_source(self.document.vmf());
        let guarded = self
            .compilation
            .options
            .requires_guarded_publication_for(self.document.vmf());
        if instances && self.path.is_none() {
            return Err("Save the parent VMF before compiling instances; the BSP output path is not a source directory".into());
        }
        let generating = self.compilation.options.generate_reflections;
        let instance_source = if instances {
            let source = self
                .path
                .as_deref()
                .ok_or("Save the parent VMF before compiling instances")?;
            Some(
                compiler::instances::SourceContext::capture(source, &control)
                    .map_err(|error| error.to_string())?,
            )
        } else {
            None
        };
        let pointfile_stamp = guarded
            .then(|| {
                compiler::FileStamp::capture(&path.with_extension("lin"))
                    .map_err(|error| error.to_string())
            })
            .transpose()?;
        let (context, environment) = if generating {
            if self.game_running() {
                return Err("Close CS:S before capturing reflections".into());
            }
            let context = compiler::CompileContext::from_output(&path)?;
            let environment = self.capture_environment()?;
            (Some(context), Some(environment))
        } else {
            (None, None)
        };
        let native_plan = environment
            .as_ref()
            .map(|environment| {
                Ok::<_, String>(NativeCapturePlan {
                    client: self.native_activity.clone(),
                    capture: crate::runtime::native_activity::Target::capture(environment)?,
                    play: None,
                })
            })
            .transpose()?;
        let (source_stamp, destination_stamp) = if guarded {
            let source_stamp = (!instances)
                .then_some(self.path.as_ref())
                .flatten()
                .map(|path| compiler::FileStamp::capture(path).map_err(|e| e.to_string()))
                .transpose()?;
            let destination_stamp =
                compiler::FileStamp::capture(&path).map_err(|e| e.to_string())?;
            (source_stamp, Some(destination_stamp))
        } else {
            (None, None)
        };
        if self.file_claims.source.is_none()
            && let Some(source) = &self.path
        {
            let destination = source_formats::file_publication::BoundDestination::capture(
                source,
                source_formats::file_publication::Policy::FollowSource,
            )
            .map_err(|error| error.to_string())?;
            self.file_claims.source = Some(self.file_claims.source(destination)?);
        }
        let destination = source_formats::file_publication::BoundDestination::capture(
            &path,
            source_formats::file_publication::Policy::ReplaceEntry,
        )
        .map_err(|error| error.to_string())?;
        let pointfile = source_formats::file_publication::BoundDestination::capture(
            &path.with_extension("lin"),
            source_formats::file_publication::Policy::ReplaceEntry,
        )
        .map_err(|error| error.to_string())?;
        let output_claims = self.file_claims.outputs(destination, pointfile)?;
        if let Some(claim) = &self.file_claims.source {
            claim.destination().verify().map_err(|error| {
                format!("Map source binding changed; reopen or Save As before compiling: {error}")
            })?;
        }
        self.compilation.serial += 1;
        let inspection_source = if let Some(source) = &instance_source {
            Ok(Some(source.root_identity().clone()))
        } else {
            self.path
                .as_ref()
                .map(|path| baked_lightmaps::BuildIdentity::source(path, &control))
                .transpose()
        };
        let origin = CompileOrigin::new();
        self.compilation.job = Some(Job {
            queue: None,
            origin: origin.clone(),
            token: self.compilation.serial,
            control: control.clone(),
            path: path.clone(),
            source_path:self.path.clone(),
            inspection_source,
            content_epoch:self.content_epoch,
            native_runtime:environment.as_ref().map(|environment|serde_json::json!({"engine_root":environment.engine_root,"steam_root":environment.steam_root})),
            snapshot: Snapshot {
                generation: self.document_generation,
                revision: self.document.revision(),
            },
            options: self.compilation.options,
            region: self.compile_region_build_scope(),
            recipe: self.active_build_recipe(),
            started: origin.started(),
            finished: None,
            status: "running",
            result: None,
            instance_dependencies: None,
            static_prop_sources: None,
            guarded,
        });
        let request = CompileRequest {
            #[cfg(test)]
            before_preparation: None,
            #[cfg(test)]
            after_compilation: None,
            #[cfg(test)]
            capture: None,
            native_plan,
            output_claims,
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
            instance_source,
            pointfile_stamp,
            destination_stamp,
            origin: origin.clone(),
            ticket: Some(Ticket::new(origin)),
        };
        self.compiling = true;
        self.log(actor, "compiling map");
        Ok(request)
    }

    fn commit_guarded_output(&self, result: &CompileResult) -> Result<(), String> {
        let mut publication = result
            .publication
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if publication.committed || publication.pending.is_none() {
            return Ok(());
        }
        let outcome: Result<(), String> = (|| {
            let activity = if result.options.generate_reflections {
                "reflection capture"
            } else if result.options.bake_static_prop_vertices {
                "static-prop vertex lighting"
            } else if result.options.pack_static_prop_assets {
                "static-prop asset packing"
            } else if result.options.pack_water_assets {
                "water asset packing"
            } else if result.options.pack_sky_assets {
                "sky asset packing"
            } else if result.output.instance_dependencies.is_some() {
                "instance compilation"
            } else if result.static_prop_sources.is_some() {
                "static-prop compilation"
            } else {
                "build publication"
            };
            if result.options.pack_sky_assets
                && result.output.bsp.is_some()
                && result.output.sky_assets.is_none()
            {
                return Err("Packed sky output is missing its retained asset plan".into());
            }
            if result.options.pack_water_assets
                && result.output.bsp.is_some()
                && result.output.water_assets.is_none()
            {
                return Err("Packed water output is missing its retained asset plan".into());
            }
            if result.options.pack_static_prop_assets
                && result.output.bsp.is_some()
                && result.output.static_prop_assets.is_none()
            {
                return Err("Packed static-prop output is missing its retained asset plan".into());
            }
            if self.closing
                || result.generation != self.document_generation
                || result.revision != self.document.revision()
                || result.options != self.compilation.options
                || result.content_epoch != self.content_epoch
            {
                return Err(format!(
                    "Map, content, runtime or build settings changed during {activity}; previous output preserved"
                ));
            }
            let job = self
                .compilation
                .job
                .as_ref()
                .filter(|job| job.token == result.token)
                .ok_or("Guarded compile job was replaced")?;
            if job.source_path != self.path {
                return Err(format!(
                    "Map source path changed during {activity}; previous output preserved"
                ));
            }
            result.control.checkpoint()?;
            let pending = publication
                .pending
                .take()
                .ok_or("Prepared guarded output missing")?;
            let leak_output = result.output.bsp.is_none() && result.output.leak.is_some();
            let lease = if leak_output {
                publication.pointfile_lease.as_ref()
            } else {
                publication.lease.as_ref()
            }
            .ok_or("Guarded destination lease missing")?;
            let receipt = pending
                .commit_checked_with_receipt(lease, &result.control, || {
                    if let Some(source) = &result.source {
                        source.verify_metadata()?;
                    }
                    if let Some(dependencies) = &result.output.instance_dependencies {
                        dependencies.verify_metadata()?;
                    }
                    if let Some(sources) = &result.static_prop_sources {
                        sources.guard.verify_sources(&self.game, &result.control)?;
                    }
                    publication
                        .sources
                        .as_ref()
                        .ok_or_else(|| std::io::Error::other("Guarded asset sources missing"))?
                        .verify_metadata()?;
                    if let Some(stamp) = &result.destination_stamp {
                        stamp.verify_metadata()?;
                    }
                    if leak_output && let Some(stamp) = &publication.pointfile_stamp {
                        stamp.verify_metadata()?;
                    }
                    Ok(())
                })
                .map_err(|e| e.to_string())?;
            if let Some(claims) = publication.claims.take() {
                let claim = if receipt.success {
                    claims.bsp
                } else {
                    claims.pointfile
                };
                claim.published(receipt.publication.destination);
                publication.artifact = Some(claim);
            }
            publication.warning = receipt.publication.warning;
            publication.published = receipt.success;
            publication.committed = true;
            Ok(())
        })();
        publication.lease = None;
        publication.pointfile_lease = None;
        publication.sources = None;
        if let Err(error) = &outcome {
            publication.pending = None;
            publication.claims = None;
            publication.error = Some(error.clone());
        }
        outcome
    }

    pub(crate) fn finish_compile_for_origin(
        &mut self,
        actor: Actor,
        origin: &CompileOrigin,
        result: &Result<CompileResult, String>,
    ) -> bool {
        if !self.accepts_compile_completion(origin, result) {
            return false;
        }
        self.finish_compile(actor, result);
        true
    }

    fn accepts_compile_completion(
        &self,
        origin: &CompileOrigin,
        result: &Result<CompileResult, String>,
    ) -> bool {
        if self.compilation.job.as_ref().is_none_or(|job| {
            !job.origin.same(origin)
                || job.finished.is_some()
                || result
                    .as_ref()
                    .is_ok_and(|result| job.token != result.token)
        }) || result
            .as_ref()
            .is_ok_and(|result| !result.origin.same(origin))
        {
            return false;
        }
        true
    }

    pub fn finish_compile(&mut self, actor: Actor, result: &Result<CompileResult, String>) {
        let mut prop_lighting_guard = None;
        if let Ok(result) = result {
            if self.compilation.job.as_ref().is_none_or(|job| {
                !job.origin.same(&result.origin)
                    || job.token != result.token
                    || job.finished.is_some()
            }) {
                return;
            }
            prop_lighting_guard = result.prop_lighting_guard();
            let _ = self.commit_guarded_output(result);
        }
        self.compiling = false;
        if let Some(job) = &mut self.compilation.job {
            if let Ok(result) = result {
                job.instance_dependencies = result.output.instance_dependencies.clone();
                job.static_prop_sources = result.static_prop_sources.clone();
            }
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
        if !self.closing
            && let Ok(result) = result
        {
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
                let source = self
                    .compilation
                    .job
                    .as_ref()
                    .map(|job| job.inspection_source.clone())
                    .unwrap_or_else(|| Err("Build source identity is unavailable".into()));
                self.compiled = Some(CompiledMap {
                    visibility_inspection: result.output.visibility_inspection.clone(),
                    world_light_origins: result.output.world_light_origins.clone(),
                    static_prop_origins: result.output.static_prop_origins.clone(),
                    prop_lighting_guard,
                    material_sources: result.output.material_sources.clone().map(|guard| {
                        super::MaterialSources {
                            guard,
                            game: self.game.clone(),
                        }
                    }),
                    static_prop_sources: result.static_prop_sources.clone(),
                    claim: result.take_artifact_claim(),
                    output_identity: baked_build::capture_output_identity(
                        &result.path,
                        result.output.bsp.as_ref(),
                        source_formats::bsp::PakLimits::default().max_bsp_bytes as u64,
                        "Run",
                    ),
                    face_origins: result.output.face_origins.clone(),
                    path: result.path.clone(),
                    snapshot,
                    options: result.options,
                    content_epoch: result.content_epoch,
                    instance_dependencies: result.output.instance_dependencies.clone(),
                    instance_source_path: result.output.instance_dependencies.as_ref().and_then(
                        |_| {
                            self.compilation
                                .job
                                .as_ref()
                                .and_then(|job| job.source_path.clone())
                        },
                    ),
                    lightmap_identity: baked_lightmaps::BuildIdentity::capture(
                        result.token,
                        &result.path,
                        source,
                        result.output.bsp.as_ref(),
                    ),
                });
                self.reset_compiled_details();
                self.compiled_visibility = super::compiled_visibility::State::default();
                self.compiled_prop_leaf_bounds =
                    super::baked_prop_vertices::leaf_bounds::State::default();
                self.scene_serial += 1;
            } else if result.output.leak.is_some()
                && let Some(claim) = result.take_artifact_claim()
            {
                self.compilation.retained_pointfile = Some(claim);
            }
            if let Some(warning) = result.publication_warning() {
                self.log(actor, format!("Build output was written, but {warning}"));
            }
        }
        let message = match result {
            _ if self
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
        if let Ok(result) = result {
            let mut publication = result
                .publication
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            publication.pending = None;
            publication.claims = None;
            publication.lease = None;
            publication.pointfile_lease = None;
            publication.sources = None;
            if self.closing {
                publication.artifact = None;
            }
            if let Some(ticket) = publication.ticket.take() {
                ticket.accept();
            }
        }
    }

    fn compiled_material_source_issue(&self, token: u64) -> Option<String> {
        self.compiled
            .as_ref()
            .filter(|build| build.lightmap_identity.token == token)
            .and_then(|build| {
                baked_build::material_source_issue(
                    build.material_sources.as_ref(),
                    &compiler::Control::default(),
                )
            })
    }

    pub fn compile_status(&self) -> Value {
        let job = self.compilation.job.as_ref().map(|job| {
            let progress = job.control.progress();
            let dependency_issue=self.instance_dependency_issue(job.instance_dependencies.as_ref(),&job.source_path);
            let static_prop_source_issue=self.static_prop_source_issue(job.static_prop_sources.as_ref());
            let material_source_issue=self.compiled_material_source_issue(job.token);
            let mut status=json!({"token":job.token.to_string(),"status":if matches!(job.status(), "queued" | "running") && progress.cancel_requested {"cancelling"} else {job.status()},
                "stage":progress.stage.name(),"stage_label":progress.stage.label(),"cancellable":matches!(job.status(), "queued" | "running") && progress.cancellable && !progress.cancel_requested,
                "elapsed_ms":job.elapsed().as_millis(),
                "path":job.path.display().to_string(),"generation":job.snapshot.generation,"revision":job.snapshot.revision,
                "compile_region":job.region,"recipe":job.recipe,"native_runtime":job.native_runtime,"reflections":{"channel":if job.options.lighting==compiler::LightingMode::DirectLdrHdr {"ldr_hdr"} else {"ldr"},"samples_complete":progress.reflection_samples_complete,"samples_total":progress.reflection_samples_total},"settings":{"bake_static_prop_vertices":job.options.bake_static_prop_vertices,"pack_sky_assets":job.options.pack_sky_assets,"pack_water_assets":job.options.pack_water_assets,"pack_static_prop_assets":job.options.pack_static_prop_assets,"generate_reflections":job.options.generate_reflections,"smoothing_angle":job.options.smoothing_angle,"bounces":job.options.bounces,"lighting":job.options.lighting.name(),"physics":job.options.physics.name(),"sky_quality":job.options.sky_quality.name(),"visibility":job.options.visibility.name()},
                "stale":!self.current_snapshot(&job.snapshot) || job.options != self.compilation.options || (job.guarded && job.content_epoch != self.content_epoch) || dependency_issue.is_some() || static_prop_source_issue.is_some() || material_source_issue.is_some(),"static_prop_source_issue":static_prop_source_issue,"material_source_issue":material_source_issue,"result":job.result()});
            status["settings"]["lighting_work_limit"] = job.options.lighting_work_limit.into();
            status["settings"]["brush_texture_shadows"] = job.options.brush_texture_shadows.into();
            status["settings"]["refine_ambient_probes"] = job.options.refine_ambient_probes.into();
            if let Some(queue) = &job.queue { status["queue"] = queue.snapshot(); }
            if job.instance_dependencies.is_some() { status["instance_dependencies"]=json!({"current":dependency_issue.is_none(),"error":dependency_issue}); }
            status
        });
        let launch = match self.launch_readiness() {
            Ok(path) => json!({"ready":true,"bsp":path.display().to_string(),"reason":null}),
            Err(reason) => json!({"ready":false,"bsp":null,"reason":reason}),
        };
        json!({"job":job,"launch":launch,"settings":self.compile_settings(),"native_activity":self.native_activity_status()})
    }

    pub fn compile_token(&self) -> Option<u64> {
        self.compilation.job.as_ref().map(|job| job.token)
    }

    pub(crate) fn queue_compile(
        &mut self,
        origin: &CompileOrigin,
        queue: crate::runtime::QueueStatus,
    ) -> Result<(), String> {
        let job = self
            .compilation
            .job
            .as_mut()
            .filter(|job| job.origin.same(origin) && job.finished.is_none())
            .ok_or("Compile job was replaced")?;
        job.queue = Some(queue);
        Ok(())
    }

    pub(crate) fn compile_origin(&self, token: u64) -> Result<CompileOrigin, String> {
        self.compilation
            .job
            .as_ref()
            .filter(|job| job.token == token)
            .map(|job| job.origin.clone())
            .ok_or_else(|| "Compile job was replaced".into())
    }

    pub fn cancel_compile(&mut self, actor: Actor, token: u64) -> Result<(), String> {
        let job = self
            .compilation
            .job
            .as_ref()
            .filter(|job| job.token == token)
            .ok_or("Compile job was replaced")?;
        if !matches!(job.status(), "queued" | "running") {
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
        let material_source_issue = self.compiled_material_source_issue(job.token);
        match job.status() {
            "running" | "queued" if progress.cancel_requested => "Cancelling compilation…".into(),
            "queued" => "Queued · Waiting to compile".into(),
            "running" => format!("Compiling · {}", progress.stage.label()),
            "completed" => format!(
                "Compiled in {:.2}s{}",
                job.finished.unwrap_or_default().as_secs_f64(),
                if self.current_snapshot(&job.snapshot)
                    && job.options == self.compilation.options
                    && self
                        .instance_dependency_issue(
                            job.instance_dependencies.as_ref(),
                            &job.source_path
                        )
                        .is_none()
                    && self
                        .static_prop_source_issue(job.static_prop_sources.as_ref())
                        .is_none()
                    && material_source_issue.is_none()
                    && !(job.guarded && job.content_epoch != self.content_epoch)
                {
                    ""
                } else if material_source_issue.is_some() {
                    " · earlier material sources"
                } else if job.static_prop_sources.is_some() {
                    " · earlier map snapshot, build settings or model/physics sources"
                } else if job.instance_dependencies.is_some() {
                    " · earlier map snapshot, build settings or dependencies"
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
            matches!(job.status(), "queued" | "running")
                && progress.cancellable
                && !progress.cancel_requested
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
    fn ordinary_detail_builds_keep_material_freshness_and_publication_context() {
        let (directory, game) = crate::test_assets::model_lighting::game();
        for name in ["one", "two"] {
            std::fs::write(
                directory
                    .path()
                    .join(format!("materials/models/test/{name}.vmt")),
                "UnlitGeneric { }",
            )
            .unwrap();
        }
        let mut vmf = crate::test_assets::room();
        let mut prop =
            source_formats::vmf::Entity::new(vmf.object_ids().max().unwrap() + 1, "prop_detail");
        prop.properties.set("model", "models/test/receiver.mdl");
        prop.properties.set("origin", "0 0 32");
        vmf.entities.push(prop);
        let source = directory.path().join("room.vmf");
        let output = source.with_extension("bsp");
        std::fs::write(&source, vmf.write(&Default::default())).unwrap();
        std::fs::write(&output, b"previous BSP").unwrap();
        std::fs::write(output.with_extension("lin"), b"previous LIN").unwrap();
        let mut session = Session::new(Ok(game));
        session.open(Actor::User, &source, false).unwrap();
        session
            .update_compile_settings(Actor::User, Some("fullbright"), Some("none"))
            .unwrap();
        let result = session.begin_compile(Actor::User, None).unwrap().run();
        assert_eq!(result.as_ref().unwrap().output.stats.detail_props, 1);
        session.content_epoch += 1;
        session.finish_compile(Actor::User, &result);
        assert!(!result.as_ref().unwrap().published());
        assert!(
            result
                .as_ref()
                .unwrap()
                .publication_error()
                .unwrap()
                .contains("changed during build publication")
        );
        assert_eq!(std::fs::read(&output).unwrap(), b"previous BSP");
        assert_eq!(
            std::fs::read(output.with_extension("lin")).unwrap(),
            b"previous LIN"
        );
        let result = session.begin_compile(Actor::Agent, None).unwrap().run();
        session.finish_compile(Actor::Agent, &result);
        assert!(
            result.as_ref().unwrap().published(),
            "{}",
            session.compile_status()
        );
        assert_eq!(session.baked_lightmap_build_state()["current"], true);
        let bytes = std::fs::read(&output).unwrap();
        std::fs::write(
            directory.path().join("materials/models/test/one.vmt"),
            "VertexLitGeneric { }",
        )
        .unwrap();
        assert_eq!(session.baked_lightmap_build_state()["current"], false);
        let status = session.compile_status();
        assert_eq!(status["job"]["stale"], true);
        assert!(status["job"]["material_source_issue"].is_string());
        assert_eq!(std::fs::read(&output).unwrap(), bytes);
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
