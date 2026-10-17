use std::sync::{
    Arc, OnceLock,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

struct State {
    active: AtomicBool,
    started: Instant,
    terminal: OnceLock<Terminal>,
}

pub(super) struct Terminal {
    pub status: &'static str,
    pub elapsed: Duration,
    pub error: String,
}

#[derive(Clone)]
pub(crate) struct Origin(Arc<State>);

impl Origin {
    pub(super) fn new() -> Self {
        Self(Arc::new(State {
            active: AtomicBool::new(true),
            started: Instant::now(),
            terminal: OnceLock::new(),
        }))
    }

    pub(crate) fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    pub(super) fn busy(&self) -> bool {
        self.0.active.load(Ordering::Acquire)
    }

    pub(super) fn started(&self) -> Instant {
        self.0.started
    }

    pub(super) fn terminal(&self) -> Option<&Terminal> {
        self.0.terminal.get()
    }
}

pub(super) struct Ticket {
    origin: Origin,
    started: bool,
    accepted: bool,
}

impl Ticket {
    pub(super) fn new(origin: Origin) -> Self {
        Self {
            origin,
            started: false,
            accepted: false,
        }
    }

    pub(super) fn start(&mut self) {
        self.started = true;
    }

    pub(super) fn fail(&self, error: String, cancelled: bool) {
        let _ = self.origin.0.terminal.set(Terminal {
            status: if cancelled { "cancelled" } else { "failed" },
            elapsed: self.origin.0.started.elapsed(),
            error,
        });
    }

    pub(super) fn accept(mut self) {
        self.accepted = true;
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        if !self.accepted {
            let error = if self.started {
                "Compilation result abandoned before completion"
            } else {
                "Compilation abandoned before execution"
            };
            self.fail(error.into(), false);
        }
        self.origin.0.active.store(false, Ordering::Release);
    }
}
