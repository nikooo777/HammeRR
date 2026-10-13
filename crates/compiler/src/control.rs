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
            Self::Writing => "Writing compiled map",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Progress {
    pub stage: Stage,
    pub cancellable: bool,
    pub cancel_requested: bool,
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
            }),
        }))
    }
}

impl Control {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepted_cancellation_prevents_publication() {
        let control = Control::default();
        control.advance(Stage::Faces).unwrap();
        control.cancel().unwrap();
        assert!(control.advance(Stage::Writing).is_err());
        assert!(
            control
                .publish::<()>(|| panic!("cancelled output published"))
                .is_err()
        );
        assert_eq!(control.progress().stage, Stage::Faces);
    }

    #[test]
    fn publication_wins_race_before_cancellation_is_accepted() {
        let control = Control::default();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let worker_control = control.clone();
        let worker = std::thread::spawn(move || {
            worker_control.publish(|| {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(42)
            })
        });
        started_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        let cancellation = std::thread::spawn(move || control.cancel());
        release_tx.send(()).unwrap();
        assert_eq!(worker.join().unwrap().unwrap(), 42);
        assert!(cancellation.join().unwrap().is_err());
    }
}
