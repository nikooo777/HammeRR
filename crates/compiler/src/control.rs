use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Stage {
    #[default]
    Queued,
    Loading,
    WorldTree,
    Hints,
    Portals,
    Flood,
    Areas,
    Water,
    Visibility,
    Faces,
    Models,
    SkyVisibility,
    Emitting,
    StaticProps,
    Lighting,
    Physics,
    PrepareReflections,
    CaptureReflections,
    VerifyReflections,
    Writing,
}

impl Stage {
    pub fn name(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Loading => "loading",
            Self::WorldTree => "world_tree",
            Self::Hints => "hint_planes",
            Self::Portals => "portals",
            Self::Flood => "leak_check",
            Self::Areas => "area_portals",
            Self::Water => "water_data",
            Self::Visibility => "visibility",
            Self::Faces => "faces",
            Self::Models => "brush_models",
            Self::SkyVisibility => "sky_visibility",
            Self::Emitting => "bsp_records",
            Self::StaticProps => "static_props",
            Self::Lighting => "direct_lighting",
            Self::Physics => "physics_collision",
            Self::PrepareReflections => "prepare_reflections",
            Self::CaptureReflections => "capture_reflections",
            Self::VerifyReflections => "verify_reflections",
            Self::Writing => "writing",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Queued => "Waiting to compile",
            Self::Loading => "Reading brushes and materials",
            Self::WorldTree => "Partitioning world geometry",
            Self::Hints => "Partitioning hint surfaces",
            Self::Portals => "Connecting empty space",
            Self::Flood => "Checking for leaks",
            Self::Areas => "Connecting visibility areas",
            Self::Water => "Building water regions",
            Self::Visibility => "Computing visible and audible space",
            Self::Faces => "Building world faces",
            Self::Models => "Building brush entities",
            Self::SkyVisibility => "Finding sky-visible space",
            Self::Emitting => "Assembling BSP records",
            Self::StaticProps => "Placing static props",
            Self::Lighting => "Baking static point lights",
            Self::Physics => "Building physics bodies",
            Self::PrepareReflections => "Preparing reflection bindings",
            Self::CaptureReflections => "Capturing reflections",
            Self::VerifyReflections => "Verifying packed reflections",
            Self::Writing => "Writing compiled map",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Progress {
    pub stage: Stage,
    pub cancellable: bool,
    pub cancel_requested: bool,
    pub reflection_samples_complete: usize,
    pub reflection_samples_total: usize,
}

struct Inner {
    cancelled: AtomicBool,
    progress: Mutex<Progress>,
}

#[derive(Clone)]
pub struct Control(Arc<Inner>);

impl Default for Control {
    fn default() -> Self {
        Self(Arc::new(Inner {
            cancelled: AtomicBool::new(false),
            progress: Mutex::new(Progress {
                stage: Stage::Queued,
                cancellable: true,
                cancel_requested: false,
                reflection_samples_complete: 0,
                reflection_samples_total: 0,
            }),
        }))
    }
}

impl Control {
    pub fn reflection_sample_progress(&self, current: usize, total: usize) {
        if total > crate::cubemap_assets::MAX_SAMPLES || current > total {
            return;
        }
        let mut progress = self
            .0
            .progress
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        progress.reflection_samples_complete = current;
        progress.reflection_samples_total = total;
    }

    pub fn progress(&self) -> Progress {
        *self
            .0
            .progress
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    pub fn cancel(&self) -> Result<(), String> {
        let mut progress = self
            .0
            .progress
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if !progress.cancellable {
            return Err("Compilation already finished publishing its output".into());
        }
        progress.cancel_requested = true;
        self.0.cancelled.store(true, Ordering::Relaxed);
        Ok(())
    }

    pub fn cancelled(&self) -> bool {
        self.0.cancelled.load(Ordering::Relaxed)
    }

    pub fn checkpoint(&self) -> Result<(), String> {
        if self.cancelled() {
            Err("Compilation cancelled".into())
        } else {
            Ok(())
        }
    }

    pub fn advance(&self, stage: Stage) -> Result<(), String> {
        self.checkpoint()?;
        self.0
            .progress
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .stage = stage;
        self.checkpoint()
    }

    pub fn finish(&self) {
        self.0
            .progress
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .cancellable = false;
    }

    pub(crate) fn publish<T>(
        &self,
        write: impl FnOnce() -> std::io::Result<T>,
    ) -> std::io::Result<T> {
        let mut progress = self
            .0
            .progress
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if progress.cancel_requested {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "Compilation cancelled",
            ));
        }
        let result = write();
        progress.cancellable = false;
        result
    }
}
