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

mod recipes;

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
    pub(crate) fn job_token(&self) -> u64 {
        self.token
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

const MAX_RETAINED_DIAGNOSTICS: usize = 100;

fn diagnostic_world_position(diagnostic: &Value) -> Result<DVec3, String> {
    if diagnostic["position_space"].as_str() != Some("world") {
        return Err("This diagnostic has no certified world position".into());
    }
    let coordinates = diagnostic["position"]
        .as_array()
        .filter(|coordinates| coordinates.len() == 3)
        .ok_or("The recorded diagnostic position is unavailable")?;
    let mut point = [0.0; 3];
    for (target, coordinate) in point.iter_mut().zip(coordinates) {
        *target = coordinate
            .as_f64()
            .ok_or("The recorded diagnostic position is not numeric")?;
    }
    let point = DVec3::from_array(point);
    if !point.is_finite() || point.abs().max_element() > compiler::map::MAX_COMPILE_COORD {
        return Err("The recorded diagnostic position is outside finite compile bounds".into());
    }
    Ok(point)
}

pub(crate) fn diagnostic_json(index: usize, diagnostic: &compiler::Diagnostic) -> Value {
    let position_space = match diagnostic.position_space {
        compiler::DiagnosticPositionSpace::Unspecified => None,
        compiler::DiagnosticPositionSpace::World => Some("world"),
    };
    let mut row = json!({
        "index":index,
        "level":match diagnostic.level { compiler::Level::Info=>"info",compiler::Level::Warning=>"warning",compiler::Level::Error=>"error" },
        "message":diagnostic.message,
        "object":diagnostic.object,
        "position":diagnostic.position.map(|point|point.to_array()),
        "position_space":position_space,
    });
    let unavailable = if index >= MAX_RETAINED_DIAGNOSTICS {
        Some("This diagnostic is outside the retained build report".to_owned())
    } else {
        diagnostic_world_position(&row).err()
    };
    row["frame_available"] = unavailable.is_none().into();
    row["frame_unavailable_reason"] = unavailable.into();
    row
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
        "diagnostics":output.report.diagnostics.iter().take(MAX_RETAINED_DIAGNOSTICS).enumerate().map(|(index,diagnostic)|diagnostic_json(index,diagnostic)).collect::<Vec<_>>(),
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
    pub reflect_static_props: Option<bool>,
    pub brush_texture_shadows: Option<bool>,
    pub colored_texture_shadows: Option<bool>,
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
        settings["material_emission_description"] = "World settings → Emission file (_hammerr_emission_file) selects an optional mounted relative declaration file. File contents and source assets validate at compile time. Records are [ldr:|hdr:] material R G B brightness; add texture: before the material to multiply radiance by nearest mip0 base-texture RGB with the 2.2 conversion and stored UV clamp/repeat. One mode per material; unsupported or changed textures reject. Sources are drawn opaque world/detail faces or eligible stationary func_brush models with Shadows Yes, using their compiled initial pose. Moving, unrequested and MDL emitting models are unavailable. By default, finite-area sources use sampled baked lighting and dynamic receivers use indexed ambient only. Exact emission visibility (_hammerr_emission_exact_visibility) is a separate default-off World 0/1 policy requiring a nonblank Emission file. It requests bounded exact rational finite-source shadow subtraction for uniquely owned world/detail or eligible stationary Shadows Yes func_brush emitting faces at root or assembled-child scope. Exactly planar convex actual native faces may differ from the retained posed source plane when outward fan coverage and actual-plane facing are certified. Nonzero-residual sources support uniform radiance, textures preserving original native triangles, or generated texture partitions certified on their actual binary64 geometry. Generated partitions require exact actual-plane support, positive outward area, original-face containment, pairwise disjoint interiors and exact total projected area, across all RGB groups, including black, with no floating coverage allowance. Zero-residual texture sources keep their existing scope. Colour selection retains nearest mip0 RGB and binary64 UV semantics; the coverage proof does not certify real-arithmetic UV boundaries or native filtering parity. The retained plane keeps original custody and native direct-emission semantics; every applicable owning hull still blocks light, including real self-occlusion. It uses all applicable original opaque brush hulls certified closed, bounded and convex from their final planes, including oblique planes, plus actual two-sided opaque native terrain triangles and strictly admitted opaque static-prop mesh triangles. Fragment conversion must certify binary64 kernel vertices exactly on the certified actual source plane and all converted coverage inside the exact visible region. Terrain and prop emitting sources, moving, parented or unrequested brush-model emitting sources, and alpha or unqualified prop shadow providers remain unsupported. Unsupported providers, uncertain source support, unqualified fragment conversion/conditioning and exhausted bounds reject without sampled fallback or BSP publication. Native direct emission (_hammerr_emission_native_direct) is a default-off World 0/1 policy requiring a nonblank Emission file. It adds native selected/folded runtime direct point approximations at constant-RGB integration-triangle centroids and removes only intrinsic direct emission from indexed ambient; baked maps, VHV, detail direct and reflected ambient remain, with emission visibility selected independently. Native light slots and angular/folding behavior do not reproduce finite-area response. The inherited full-VHV native path can add static-cache lighting when dynamic lights affect the prop. Native direct emission implies no shader glow or exact finite-source runtime response. Turn off both emission policies before clearing Emission file, or change them in one draft. Default-off paired emission keeps the existing authored HDR WorldLight requirement. Fullbright validates without baking or native emission records.".into();
        let native_emission =
            compiler::material_emission_native_direct(&self.document.vmf().world.properties);
        settings["material_emission_native_direct"] = json!({
            "world_key":compiler::MATERIAL_EMISSION_NATIVE_DIRECT_KEY,
            "build_option":false,"default":false,"values":["0","1"],
            "enabled":native_emission.as_ref().ok(),
            "error":native_emission.as_ref().err(),
            "requires":"nonblank_valid_emission_file",
            "fullbright":"validate_only",
            "runtime_approximation":"constant_rgb_integration_triangle_centroid_point",
            "intensity":"linear_radiance_times_triangle_area_over_pi",
            "style":0,"flags":0,
            "runtime_direct":"native_selected_and_folded",
            "radius_policy":{
                "bounds":"actual_stored_model0",
                "cache_expansion":[32,32,128],
                "distance":"farthest_corner","rounding":"upward_native"
            },
            "radius_coverage":"compiled_world_receiver_and_cache_bounds",
            "indexed_ambient":"reflected_emission_retained_intrinsic_direct_replaced",
            "sampled_receivers":"maps_vhv_and_detail_direct_unchanged",
            "emission_visibility":"selected_independently_by_world_exact_visibility_policy",
            "native_static_cache_on_dynamic_lit_full_vhv":true,
            "finite_area_fidelity":false,"exact_finite_source_visibility":false
        });
        let exact_emission =
            compiler::material_emission_exact_visibility(&self.document.vmf().world.properties);
        settings["material_emission_exact_visibility"] = json!({
            "world_key":compiler::MATERIAL_EMISSION_EXACT_VISIBILITY_KEY,
            "label":"Exact emission visibility",
            "build_option":false,"default":false,"values":["0","1"],
            "enabled":exact_emission.as_ref().ok(),
            "error":exact_emission.as_ref().err(),
            "requires":"nonblank_valid_emission_file",
            "fullbright":"validate_only_without_exact_geometry_admission",
            "source_scope":"uniquely_owned_world_detail_or_stationary_shadows_yes_func_brush_root_and_assembled_children_certified_actual_planar_faces_including_oblique_and_nonzero_retained_posed_plane_residual",
            "source_support_certificate":{
                "scope":"all_exact_source_faces",
                "outward_reference":"retained_posed_source_plane",
                "requires":["exact_actual_face_planarity","nonzero_area","outward_orientation","convex_simple_fan_coverage"],
                "facing":"certified_actual_source_plane",
                "retained_posed_plane":"original_side_hull_custody_and_native_direct_approximation",
                "owning_hulls":"all_applicable_original_hulls_active_for_real_self_occlusion",
                "nonzero_residual_scope":"uniform_radiance_or_original_native_triangles_or_exact_coverage_certified_generated_texture_partitions",
                "zero_residual_texture_scope":"unchanged"
            },
            "texture_partition_certificate":{
                "scope":"generated_texture_partitions_at_nonzero_retained_posed_plane_residual",
                "requires":["exact_actual_source_plane_support","positive_outward_area","original_face_containment","pairwise_disjoint_interiors","exact_total_projected_area"],
                "rgb_groups":"all_including_black_before_radiance_shortcuts",
                "shared_boundaries":"edges_and_vertices_allowed",
                "floating_coverage_allowance":"none",
                "geometry":"actual_generated_binary64_triangles",
                "colour_selection":"existing_nearest_mip0_RGB_and_binary64_UV_semantics",
                "real_arithmetic_uv_boundary_or_native_filter_parity":false
            },
            "occluder_scope":"all_applicable_original_opaque_brush_hulls_with_certified_closed_bounded_convex_final_planes_including_oblique_planes_and_actual_two_sided_opaque_native_terrain_and_strict_opaque_static_prop_triangles",
            "prop_shadow_materials":"strict_opaque_VertexLitGeneric_or_basetexture_Patch_chain",
            "prop_shadow_geometry":"actual_body0_lod0_selected_skin_final_pose_triangles",
            "topology":"bounded_exact_rational_open_segment_shadow_subtraction",
            "fragment_conversion":"certified_binary64_kernel_vertices_on_actual_source_plane_and_converted_coverage_inside_exact_visible_regions",
            "failure_policy":"reject_without_sampled_fallback_or_bsp_publication",
            "unsupported":["unrepresentable_on_plane_binary64_kernel_fragment_vertices","moving_parented_or_unrequested_brush_model_emitting_sources","nonplanar_or_uncertified_actual_source_faces","nonzero_residual_generated_texture_partitions_without_exact_actual_face_coverage","terrain_emitting_sources","prop_emitting_sources","unqualified_prop_materials_or_collapsed_primitives","sky_alpha_or_unknown_materials","uncertified_closed_convex_hulls","uncertain_source_support","unqualified_fragment_conversion_or_conditioning","exhausted_work_storage_or_arithmetic_bounds"],
            "default_visibility":"sampled_segments",
            "native_direct":"independent_centroid_point_approximation_unchanged"
        });
        settings["lighting_work_limit_range"] = json!({
            "min":compiler::MIN_LIGHTING_WORK_LIMIT,
            "max":compiler::MAX_LIGHTING_WORK_LIMIT,
            "default":compiler::DEFAULT_LIGHTING_WORK_LIMIT,
            "unit":"work_units"
        });
        settings["lighting_work_description"] = "One aggregate allowance for preparation, LDR and HDR lighting. Raising it preserves sampling quality and all independent allocation, sample, light, probe and geometry limits.".into();
        settings["bake_static_prop_vertices"] =
            self.compilation.options.bake_static_prop_vertices.into();
        settings["reflect_static_props"] = self.compilation.options.reflect_static_props.into();
        let prop_reflection_available = self.compilation.options.lighting.performs_lighting()
            && self.compilation.options.bake_static_prop_vertices;
        settings["static_prop_reflection"] = json!({
            "available":prop_reflection_available,
            "unavailable_reason":if prop_reflection_available { None } else { Some("Choose Direct LDR or Direct LDR + HDR and enable Bake static-prop vertex lighting first.") },
            "description":"Use eligible baked static props as reflected light sources. Requires Bake static-prop vertex lighting and a lit mode. Choose the shared bounce count under Indirect light; ambient and details sample final output. Runtime direct styles stay separate.",
            "limits":{
                "requires":["bake_static_prop_vertices","lit_mode"],
                "source_draw_lod":0,"source_skin":"selected",
                "source_materials":["admitted_opaque_vertexlitgeneric","admitted_strict_binary_cutout_vertexlitgeneric"],
                "source_light_styles":[0],
                "receivers":["world","terrain","brush_models","prop_vertices","indexed_ambient","detail"],
                "bounces":{"min":0,"max":compiler::MAX_LIGHT_BOUNCES},
                "snapshot":"previous_depth_world_and_props",
                "ambient_and_details":"filtered_final_output",
                "channels":"independent_ldr_hdr",
                "requires_shadow_casting":false,"receiver_self_shadow_setting":"preserved",
                "implicit_emission":false,"runtime_direct_styles":"excluded_from_sources",
                "switched_indirect_ambient":false
            }
        });
        settings["brush_model_shadowing"] = json!({
            "available":self.compilation.options.lighting.performs_lighting(),
            "property":"vrad_brush_cast_shadows","enabled_value":"1","default_value":"0",
            "fullbright":"admit_and_preserve_without_baking",
            "baked_count":"stats.brush_shadow_models",
            "description":"Advanced properties → Shadows → Yes requests global baked shadows from an unnamed, unparented stationary func_brush. Opaque root and child requests keep their ordinary SOLID material scope. Explicit root and child requests may also use the supported plain LightmappedGeneric alpha-tested or translucent material when Brush texture shadows is enabled in a lit mode. Child cutouts without a global request use only their own applicable baked receiver scope. Root requests use an explicit finite Origin (X Y Z). Child models may instead resolve and consume one origin volume before the ordinary body check. Root and child requests validate before region omission; Fullbright reports admitted opaque source requests without baking, including requests omitted by compile regions. Admitted child initial poses are baked before containing transforms; emitted child angles are zero. Eligible stationary Shadows Yes root and assembled-child func_brush models supply reflected radiance from their baked maps alongside world and terrain. Moving, parented and unrequested brush-model sources remain unavailable. Eligible VHV static-prop sources require Reflect light from static props; other MDL sources remain unavailable.",
            "limits":{
                "classname":"func_brush","origin":"resolved_finite",
                "root_origin":"explicit_finite","child_origin":"explicit_or_one_consumed_volume",
                "runtime_names_parenting_io":false,"source_materials":"resolved_opaque_ordinary_solid_or_opted_in_requested_texture_alpha",
                "material_shaders":["LightmappedGeneric","UnlitGeneric","Patch"],
                "patch_form":"unambiguous_scalar_include_edit",
                "root_binary_cutout_requires":"brush_texture_shadows_and_lit_mode",
                "child_binary_cutout":true,"child_binary_cutout_requires":"canonical_request_and_brush_texture_shadows_and_lit_mode","fractional_translucent":true,"additive":false,"terrain_liquid_origin_volumes":false,
                "child_origin_volumes":"one_resolved_consumed_before_body_validation",
                "radiance_source":true,"dynamic_relighting":false,
                "reflected_source_pose":"compiled_stationary_initial_pose",
                "reflected_source_materials":"existing_admitted_opaque_and_opted_in_cutout_fractional_colored",
                "reflected_source_receivers":["world","terrain","brush_models","indexed_ambient","detail","prop_vertices"],
                "map_and_vhv_bounces":{"min":0,"max":compiler::MAX_LIGHT_BOUNCES},
                "vhv_light_styles":[0],
                "ambient_and_details":"filtered_final_maps",
                "unavailable_reflected_sources":["moving","parented","unrequested","mdl"],
                "implicit_emission":false,"native_vrad_parity":false,
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
        settings["colored_texture_shadows"] =
            self.compilation.options.colored_texture_shadows.into();
        let colored_available = self.compilation.options.lighting.performs_lighting()
            && self.compilation.options.brush_texture_shadows;
        settings["colored_texture_shadowing"] = json!({
            "available":colored_available,
            "unavailable_reason":if colored_available { None } else { Some("Choose Direct LDR or Direct LDR + HDR and enable Brush texture shadows first.") },
            "description":"Tint transmitted light with the base color of supported translucent textures. Clear texels stay clear; fully opaque texels block light.",
            "limits":{
                "lighting_modes":["direct_ldr","direct_ldr_hdr"],
                "requires_brush_texture_shadows":true,
                "material_scope":"existing_admitted_fractional_base_texture_alpha",
                "casters":["world","func_detail","world_terrain","detail_terrain","brush_model_self","child_brush_model_self","requested_stationary_root_func_brush","requested_stationary_child_func_brush","mdl_requested_fractional_props"],
                "receivers":["direct","styles","sun","sky","directional_maps","bounces","ambient_probes","detail","prop_vertices","material_emission"],
                "rgb_decode":"gamma_2_2",
                "channel_coverage":"round(a+a*(1-a/255)*(1-C))",
                "combination":"additive_saturated_per_channel_byte_coverage",
                "hull_plane_ties":"componentwise_maximum",
                "clear_texels":"untinted_holes","opaque_texels":"fully_blocking",
                "white_rgb":"scalar_coverage_unchanged","binary_cutouts":"unchanged",
                "additional_bytes_per_fractional_texel":3,
                "work_and_live_memory":"existing_shared_checked_lighting_limits",
                "mip":0,"filter":"nearest","addressing":"vtf_clamp_or_repeat",
                "assets":"same_original_tracked_vmt_and_vtf",
                "native_vrad_parity":false,"physical_absorption_model":false
            }
        });
        settings["brush_texture_shadowing"] = json!({
            "available":self.compilation.options.lighting.performs_lighting(),
            "unavailable_reason":if self.compilation.options.lighting.performs_lighting() { None } else { Some("Choose Direct LDR or Direct LDR + HDR before enabling brush texture shadows.") },
            "description":"Use static base-texture alpha on plain LightmappedGeneric world/detail brushes, terrain and admitted initial-pose brush-model self shadows. An eligible stationary root or child func_brush with Advanced properties → Shadows → Yes also casts these texture shadows into global baked lighting, including world and other-model receivers. Child effective origins and source initial poses lower before containing transforms. Unrequested root and child model cutouts stay within their own lit receiver scope. Absent or zero global requests never cast into world or other-model lighting. Eligible stationary Shadows Yes root and assembled-child func_brush models supply reflected radiance from their baked maps alongside world and terrain. Moving, parented and unrequested brush-model sources remain unavailable. Eligible VHV static-prop sources require Reflect light from static props; other MDL sources remain unavailable. Default off; independent of native model-requested prop texture shadows. Geometry, collision and sealing remain unchanged. Requested unsupported materials, model scopes or missing textures stop publication.",
            "limits":{
                "lighting_modes":["direct_ldr","direct_ldr_hdr"],
                "casters":["world","func_detail","world_terrain","detail_terrain","brush_model_self","child_brush_model_self","requested_stationary_root_func_brush","requested_stationary_child_func_brush"],
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
                    "radiance_source":true,"world_occupancy":false
                },
                "global_child_model_scope":{
                    "property":"vrad_brush_cast_shadows","enabled_value":"1",
                    "requires_brush_texture_shadows":true,"requires_lit_mode":true,
                    "classname":"func_brush","pose":"lowered_before_containing_transforms",
                    "origin":"explicit_or_one_consumed_volume","requires_lit_receiver_scope":false,
                    "reference_preparation":"deferred","optionless_compile_inspection":"ordinary_solid_only",
                    "casts_into_world":true,"casts_into_other_models":true,
                    "radiance_source":true,"world_occupancy":false
                },
                "material":"plain LightmappedGeneric",
                "coverage":"binary_or_fractional_base_texture_alpha",
                "coverage_modes":["binary_alpha_test","fractional_base_texture_alpha"],
                "combination":if self.compilation.options.colored_texture_shadows { "additive_saturated_per_channel_byte_coverage" } else { "additive_saturated_byte_coverage" },
                "hull_boundaries":"entry_and_exit_with_maximum_at_plane_ties",
                "terrain_crossings":"certified_shared_topology_once",
                "alpha_modulation":1,
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
                "colored_transmission_option":"colored_texture_shadows",
                "unsupported":["alpha modulation","blend-shader terrain opacity","unadmitted model poses","unrequested cutout models without lit receivers","unadmitted moving parented unrequested or MDL model radiance sources","conditional or animated materials"]
            }
        });
        settings["pack_water_assets"] = self.compilation.options.pack_water_assets.into();
        settings["pack_static_prop_assets"] =
            self.compilation.options.pack_static_prop_assets.into();
        settings["static_prop_packing_description"] = "Pack original model, companion, all declared skin/LOD materials and basetexture files for emitted static props during Compile or Run Map. Default off; independent of vertex lighting and available in every lighting mode. Requires bounded static MDL 44/48, body 0/root LOD 0, trivial local animation and VertexLitGeneric terminals. Terminals allow only one canonical $basetexture, optionally plus literal $alphatest 1 at the default 0.5 cutoff; authored $alphatestreference is unsupported. Patch chains allow at most eight include edges, with one basetexture insert or replace block per Patch. Deeper edits win within each type; replacements apply after all inserts. All original VMT bytes and every declared VTF remain packed, including overridden textures. Extra keys, combined edit sections, proxies and $envmap reject. Matching PHY is included only for VPhysics props. Missing or unsupported dependencies stop publication. Packing preserves model flags. Compiler texture shadows require the original MDL CAST_TEXTURE_SHADOWS request; without it, enabled prop shadows remain opaque mesh shadows with the existing warning. Selected default-alpha-test vertex-lighting receivers accept the same bounded nested Patch basetexture edits and terminal contract. Every hardware vertex is retained with VHV alpha 255; receiver-only textures are read only if casting, packing or enabled reflected-source preparation needs them. Runtime requires standard CS:S shaders and surface/prop-data definitions with DX90 or newer.".into();
        settings["water_packing_description"] = "Pack original files for complete supported static Water or opaque SLIME material/Patch/texture closures during Compile or Run Map. Default off. SLIME supports static opaque LightmappedGeneric, UnlitGeneric and unambiguous scalar Patch chains with an explicit basetexture and exact native self-bottom. Original base, bump, tool and named LDR/HDR envmap assets remain packed, including overridden references. Explicit runtime Water surface envmaps and runtime SLIME env_cubemap require Generate reflections and Direct LDR + HDR; generated pairs, bound aliases and original inputs verify before publication. Water bottom/underwater-overlay graphs require explicit named static LDR/HDR envmaps; omitted envmaps, runtime dependencies below the surface root and material cycles reject. Runtime Water envmapframe must be absent or zero and Patch chains cannot edit a key in multiple materials. All original declared resources remain packed, including overridden references. Other resources/effects remain unsupported. Static named-envmap packing works in every lighting mode without capture.".into();
        settings["prop_vertex_lighting"] = json!({
            "available":unavailable_reason.is_none(),
            "unavailable_reason":unavailable_reason,
            "description":"Bake direct lighting and up to four diffuse bounces from world, terrain and eligible stationary Shadows Yes root/assembled-child func_brush maps on unit-scale static props with one bodypart/model, body0/root0, all 1–8 draw LODs and plain opaque or strict default-alpha-test VertexLitGeneric materials, up to 256 hardware sections total. Direct LDR + HDR evaluates both channels independently. Stored VHV contains the static style-0 base; supported effective point/spot styles remain runtime direct lighting through native WorldLight records and are excluded from VHV. Editor VHV preview stays static; switched indirect ambient remains unsupported. At depth N above zero, prop reception uses world/terrain and eligible stationary requested brush-model lighting from depth N-1 and adds direct light once. When Reflect light from static props is enabled, eligible prop sources join the same previous-depth snapshot. Indirect reception requires texture-header or authored material reflectivity for every consumed world/terrain or eligible stationary requested brush-model source. Dimensions independently use diffuse or tool VTF headers, or the SDK no-preview 128x128 convention; every complete VTF actually read for metrics is hashed. Authored $reflectivity accepts one ASCII [r g b] triple of finite linear values from 0 to 1, at most 128 bytes; only trimmed %notooltexture 0 or 1 selects the no-preview convention before dimension reads. Empty or other no-preview values and non-path tool values reject; failed required providers never fall back. disableselfshadowing=1 omits only the receiving prop’s own shadow triangles. lightingorigin preserves a uniquely resolved info_lighting point for native runtime lighting; baked samples, normals and PVS remain at actual vertices, without embedded-vertex recovery. ignorenormals=1 supports zero to four bounces: constant 0.35 point/spot/sun reception, full-sphere sky ambient, and spherical reflected reception with SDK angular normalization. It retains real shadows and finite nonzero model normals. Original vertices may touch opaque world brushes; buried vertices and terrain/other-prop contacts reject. Model/material compatibility is checked during compilation. Unsupported requested receivers stop the build; disablevertexlighting=1 explicitly opts out one prop. No native capture runtime is required.",
            "limits":{
                "lighting_modes":["direct_ldr","direct_ldr_hdr"],
                "bounces":{"min":0,"max":compiler::MAX_LIGHT_BOUNCES},"indirect_sources":["world","terrain","requested_stationary_root_func_brush","requested_stationary_assembled_child_func_brush"],"indirect_reflectivity":"texture_header_or_authored_material","indirect_dimensions":"header_or_native_no_preview","indirect_dimension_sources":["diffuse_vtf_header","tool_vtf_header","native_no_preview_convention"],"receiver":{"body":0,"root_lod":0,"bodyparts":1,"models":1,"lods":{"min":1,"max":8,"all_required":true},"hardware_meshes_max":compiler::prop_lighting_assets::MAX_RECEIVER_MESHES,"scale":1,"sample_origin":"original_vertex","lighting_origin":{"property":"lightingorigin","baked_samples":"actual_vertices","embedded_recovery":false},"support_contacts":"opaque_world_brush","self_shadowing":{"default":true,"disable_property":"disableselfshadowing","disable_value":1,"disabled_scope":"receiver_owned_triangles","retained_blockers":["other_props","world","terrain"]},"ignore_normals":{"property":"ignorenormals","enabled_value":1,"bounces":{"min":0,"max":compiler::MAX_LIGHT_BOUNCES},"direct_factor":compiler::lighting::IGNORE_NORMALS_FACTOR,"sky_ambient":"full_sphere_visibility_mean","indirect":"spherical_sdk_normalization","keeps_shadows":true}},
                "material":"plain opaque or strict default-alpha-test selected VertexLitGeneric",
                "cutout_material":{"terminal_parameters":{"$basetexture":"canonical_path","$alphatest":"1"},"alpha_test_reference":0.5,"authored_reference":false,"include_edges_max":8,"patch_edit":"one_basetexture_insert_or_replace_per_patch","extra_parameters":false,"selected_skin_only":true,"all_hardware_vertices":true,"vhv_alpha":255,"receiver_texture_read":false,"texture_shadow_request":"original_mdl_flag","packing_required":false},
                "light_styles":[0],
                "light_styles_scope":"stored_vhv_base",
                "runtime_direct":{
                    "light_kinds":["point","spot"],
                    "styles":"effective_nonzero",
                    "provider":"native_world_lights",
                    "baked_into_vhv":false,
                    "switched_indirect_ambient":false
                },
                "unsupported":["buried vertices","terrain/other-prop support contacts","specialized materials","per-texel prop lightmaps","unadmitted prop reflected sources"]
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
            reflect_static_props,
            brush_texture_shadows,
            colored_texture_shadows,
            refine_ambient_probes,
        } = update;
        let bounces = bounces.unwrap_or(self.compilation.options.bounces);
        let options = compiler::Options {
            refine_ambient_probes: refine_ambient_probes
                .unwrap_or(self.compilation.options.refine_ambient_probes),
            brush_texture_shadows: brush_texture_shadows
                .unwrap_or(self.compilation.options.brush_texture_shadows),
            colored_texture_shadows: colored_texture_shadows
                .unwrap_or(self.compilation.options.colored_texture_shadows),
            bake_static_prop_vertices: bake_static_prop_vertices
                .unwrap_or(self.compilation.options.bake_static_prop_vertices),
            reflect_static_props: reflect_static_props
                .unwrap_or(self.compilation.options.reflect_static_props),
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
                    "Build settings: {} · {} · {} visibility · {} diffuse bounces · {}° brush smoothing · {} lighting work units · ambient probe refinement {} · brush texture shadows {} · colored texture shadows {} · static-prop vertex lighting {} · reflected static-prop light {} · reflections {} · sky packing {} · liquid packing {} · static-prop packing {}",
                    options.lighting.label(),
                    options.physics.label(),
                    options.visibility.label(),
                    options.bounces,
                    options.smoothing_angle,
                    options.lighting_work_limit,
                    if options.refine_ambient_probes { "on" } else { "off" },
                    if options.brush_texture_shadows { "on" } else { "off" },
                    if options.colored_texture_shadows { "on" } else { "off" },
                    if options.bake_static_prop_vertices { "on" } else { "off" },
                    if options.reflect_static_props { "on" } else { "off" },
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
                self.reset_baked_surfaces();
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

    pub fn frame_build_diagnostic(
        &mut self,
        actor: Actor,
        job_token: &str,
        index: usize,
        aspect: f32,
    ) -> Result<Value, String> {
        self.ensure_not_closing()?;
        super::inspection_framing::validate_aspect(aspect)?;
        self.inspection_viewport_ready("framing a recorded build position")?;
        let (position, snapshot) = {
            let job = self
                .compilation
                .job
                .as_ref()
                .ok_or("No retained build job is available")?;
            if job.token.to_string() != job_token {
                return Err("Build job token is invalid or has been replaced".into());
            }
            if matches!(job.status(), "queued" | "running") {
                return Err("Wait for the build to finish before framing a diagnostic".into());
            }
            let diagnostic = job
                .result
                .as_ref()
                .and_then(|result| result["diagnostics"].as_array())
                .and_then(|diagnostics| diagnostics.get(index))
                .ok_or("The diagnostic index is outside the retained build report")?;
            (diagnostic_world_position(diagnostic)?, job.snapshot.clone())
        };
        let center = position.as_vec3();
        let half = glam::Vec3::splat(super::inspection_framing::ORIGIN_HALF_EXTENT);
        let (mins, maxs) = (center - half, center + half);
        let camera =
            self.inspection_frame_camera(mins, maxs, aspect, "Recorded build diagnostic")?;
        self.set_camera(camera);
        self.log(
            actor,
            format!("framed recorded build diagnostic {index} from job {job_token}"),
        );
        Ok(json!({
            "job_token":job_token,
            "index":index,
            "position_space":"world",
            "recorded_position":position.to_array(),
            "captured_snapshot":{"generation":snapshot.generation,"revision":snapshot.revision},
            "framed_bounds":[mins.to_array(),maxs.to_array()],
        }))
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
            status["settings"]["colored_texture_shadows"] = job.options.colored_texture_shadows.into();
            status["settings"]["reflect_static_props"] = job.options.reflect_static_props.into();
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
