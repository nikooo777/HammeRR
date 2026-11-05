use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use compiler::Control;
use map_core::{Document, ObjectId};
use render::ropes::{self, Issue, Prepared, Preview, Progress, Simulation};
use source_formats::gamefs::GameFs;

use super::state::{Key, MAX_METADATA_BYTES, Machine, Settings, Sources};
use super::{collision, endpoints, materials};
use crate::session::worker::Ticket;

pub(super) const VERIFY_INTERVAL: Duration = Duration::from_secs(1);
pub(super) const MAX_REQUEST_COLLISION_WORK: usize = 128_000_000;

pub(super) enum Input {
    Source {
        document: Box<Document>,
        choice: Option<(Vec<u32>, Arc<render::models::Asset>)>,
        included: HashSet<ObjectId>,
        snapshot_bytes: usize,
    },
    Continue {
        machine: Box<Machine>,
        replay: bool,
    },
    Verify(Arc<Sources>),
}

pub struct Request {
    pub(super) token: String,
    pub(super) identity: Arc<()>,
    pub(super) control: Control,
    pub(super) key: Key,
    pub(super) game: Arc<GameFs>,
    pub(super) settings: Settings,
    pub(super) target: f64,
    pub(super) input: Input,
    pub(super) verify_only: bool,
    pub(super) worker: Ticket,
}

pub(super) enum Appearance {
    New(materials::Assets),
    Cached {
        appearance: Arc<render::ropes::materials::Prepared>,
        sources: compiler::asset_sources::AssetSourceGuard,
    },
}

pub(super) struct Ready {
    pub simulation: Simulation,
    pub prepared: Arc<Prepared>,
    pub collision: Option<collision::Snapshot>,
    pub appearance: Appearance,
    pub model_sources: Vec<Arc<render::models::Asset>>,
    pub preview: Arc<Preview>,
    pub progress: Progress,
    pub collision_work: usize,
    pub snapshot_bytes: usize,
    pub verified: Instant,
    pub sources: Option<Arc<Sources>>,
}

pub(super) enum Output {
    Ready(Box<Ready>),
    Verified(Instant),
}

#[derive(Default)]
pub(super) struct Diagnostics {
    pub issues: Vec<Issue>,
    pub omitted: usize,
}

pub struct Completed {
    pub(super) token: String,
    pub(super) identity: Arc<()>,
    pub(super) control: Control,
    pub(super) key: Key,
    pub(super) result: Result<Output, String>,
    pub(super) diagnostics: Diagnostics,
    pub(super) verify_only: bool,
    pub(super) worker: Ticket,
}

#[derive(Clone)]
pub(crate) struct RenderGuard {
    pub(super) key: Key,
    pub(super) sources: Arc<Sources>,
    pub(super) game: Arc<GameFs>,
}

pub(crate) struct VerifiedRenderGuard {
    pub(super) key: Key,
    pub(super) sources: Arc<Sources>,
    pub(super) result: Result<(), String>,
}

impl RenderGuard {
    pub(crate) fn verify_original_sources(&self) -> Result<(), String> {
        verify_sources(&self.sources, &self.game, &self.sources.control)
    }

    pub(crate) fn run(self) -> VerifiedRenderGuard {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            verify_sources(&self.sources, &self.game, &self.sources.control)
        }))
        .unwrap_or_else(|_| Err("Rope source verification panicked".into()));
        VerifiedRenderGuard {
            key: self.key,
            sources: self.sources,
            result,
        }
    }
}

fn verify_models(
    sources: &[Arc<render::models::Asset>],
    game: &GameFs,
    control: &Control,
) -> Result<(), String> {
    for asset in sources {
        control.checkpoint()?;
        asset
            .verify_source(game)
            .map_err(|error| format!("Rope parent model {}: {error}", asset.path))?;
    }
    control.checkpoint()
}

fn verify_sources(sources: &Sources, game: &GameFs, control: &Control) -> Result<(), String> {
    sources.control.checkpoint()?;
    if let Some(collision) = &sources.collision {
        collision
            .verify_original_metadata(control, collision::MAX_SOURCE_METADATA)
            .map_err(|error| format!("Rope collision sources changed: {error}"))?;
    }
    sources
        .appearance
        .verify_original_metadata(control, MAX_METADATA_BYTES)
        .map_err(|error| format!("Rope appearance sources changed: {error}"))?;
    verify_models(&sources.models, game, control)?;
    sources.control.checkpoint()
}

fn advance(
    simulation: &mut Simulation,
    collision: Option<&collision::Snapshot>,
    time: f64,
    control: &Control,
) -> Result<(Arc<Preview>, Progress, usize), String> {
    let mut collision_work = 0usize;
    loop {
        control.checkpoint()?;
        let remaining = MAX_REQUEST_COLLISION_WORK
            .checked_sub(collision_work)
            .filter(|&work| work > 0)
            .ok_or("Rope request exceeded its aggregate collision work limit")?;
        let mut budget = map_core::rope_collision::TraceBudget::limited(remaining);
        let progress = simulation.advance_to(
            time,
            ropes::MAX_BATCH_STEPS,
            &mut |start, end| {
                control.checkpoint()?;
                collision
                    .ok_or_else(|| {
                        "Rope collision was requested without a prepared world".to_owned()
                    })?
                    .world
                    .trace_bounded(start, end, &mut budget)
                    .map_err(|error| error.to_string())
            },
            &mut || control.checkpoint(),
        )?;
        collision_work = collision_work
            .checked_add(budget.work())
            .filter(|&work| work <= MAX_REQUEST_COLLISION_WORK)
            .ok_or("Rope request exceeded its aggregate collision work limit")?;
        if progress.complete {
            let preview = Arc::new(simulation.snapshot()?);
            control.checkpoint()?;
            return Ok((preview, progress, collision_work));
        }
    }
}

impl Request {
    pub fn token(&self) -> &str {
        &self.token
    }

    pub fn run(self) -> Completed {
        let Self {
            token,
            identity,
            control,
            key,
            game,
            settings,
            target,
            input,
            verify_only,
            worker,
        } = self;
        let mut diagnostics = Diagnostics::default();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            prepare(
                input,
                &game,
                settings,
                target,
                verify_only,
                &control,
                &mut diagnostics,
            )
        }))
        .unwrap_or_else(|_| Err("Rope preview worker panicked".into()));
        Completed {
            token,
            identity,
            control,
            key,
            result,
            diagnostics,
            verify_only,
            worker,
        }
    }
}

fn prepare(
    input: Input,
    game: &GameFs,
    settings: Settings,
    target: f64,
    verify_only: bool,
    control: &Control,
    diagnostics: &mut Diagnostics,
) -> Result<Output, String> {
    control.checkpoint()?;
    match input {
        Input::Source {
            document,
            choice,
            included,
            snapshot_bytes,
        } => {
            let models = endpoints::resolve_required_models(
                &document,
                game,
                &included,
                choice.as_ref().map(|(ids, asset)| (ids.as_slice(), asset)),
                control,
            )?;
            let mut model_sources = BTreeMap::new();
            let prepared = Arc::new(ropes::prepare_with_endpoints(
                document.vmf(),
                |id| included.contains(&id),
                |source, target| {
                    control.checkpoint()?;
                    endpoints::resolve(&document, &models, source, target, &mut model_sources)
                },
            ));
            diagnostics.issues = prepared.issues.clone();
            diagnostics.omitted = prepared.omitted;
            let mut simulation = Simulation::new(prepared.clone(), settings.environment()?)?;
            let model_sources = model_sources.into_values().collect::<Vec<_>>();
            verify_models(&model_sources, game, control)?;
            let collision = prepared
                .spans
                .iter()
                .any(|span| span.settings.collide)
                .then(|| collision::prepare(document.vmf(), game, control))
                .transpose()?;
            let (preview, progress, collision_work) =
                advance(&mut simulation, collision.as_ref(), target, control)?;
            let appearance = materials::prepare(game, &preview, control)?;
            if let Some(collision) = &collision {
                collision
                    .sources
                    .verify_original_metadata(control, collision::MAX_SOURCE_METADATA)
                    .map_err(|error| format!("Rope collision sources changed: {error}"))?;
            }
            verify_models(&model_sources, game, control)?;
            appearance.verify_sources(game, control)?;
            control.checkpoint()?;
            Ok(Output::Ready(Box::new(Ready {
                simulation,
                prepared,
                collision,
                appearance: Appearance::New(appearance),
                model_sources,
                preview,
                progress,
                collision_work,
                snapshot_bytes,
                verified: Instant::now(),
                sources: None,
            })))
        }
        Input::Continue {
            mut machine,
            replay,
        } => {
            let verify = replay || verify_only || machine.verified.elapsed() >= VERIFY_INTERVAL;
            if verify {
                verify_sources(&machine.sources, game, control)?;
            }
            if replay {
                machine.simulation =
                    Simulation::new(machine.prepared.clone(), settings.environment()?)?;
            }
            let (preview, progress, collision_work) = advance(
                &mut machine.simulation,
                machine.collision.as_ref(),
                target,
                control,
            )?;
            if verify {
                verify_sources(&machine.sources, game, control)?;
            }
            Ok(Output::Ready(Box::new(Ready {
                simulation: machine.simulation,
                prepared: machine.prepared,
                collision: machine.collision,
                appearance: Appearance::Cached {
                    appearance: machine.appearance,
                    sources: machine.appearance_sources,
                },
                model_sources: machine.model_sources,
                preview,
                progress,
                collision_work,
                snapshot_bytes: machine.snapshot_bytes,
                verified: if verify {
                    Instant::now()
                } else {
                    machine.verified
                },
                sources: Some(machine.sources),
            })))
        }
        Input::Verify(sources) => {
            verify_sources(&sources, game, control)?;
            Ok(Output::Verified(Instant::now()))
        }
    }
}
