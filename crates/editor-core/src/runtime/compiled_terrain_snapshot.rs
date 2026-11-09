use std::sync::Arc;

use serde_json::Value;

use super::{DocumentOperation, SharedSession, lock};
use crate::session::{
    Actor,
    compiled_terrain_snapshot::{self, ComparisonOptions},
};

enum Request {
    Observation(Box<compiled_terrain_snapshot::Request>),
    Comparison(Box<compiled_terrain_snapshot::ComparisonRequest>),
}

enum Completed {
    Observation(Box<compiled_terrain_snapshot::Completed>),
    Comparison(Box<compiled_terrain_snapshot::ComparisonCompleted>),
}

impl Request {
    fn token(&self) -> u64 {
        match self {
            Self::Observation(request) => request.token(),
            Self::Comparison(request) => request.token(),
        }
    }
    fn comparison_identity(&self) -> Option<Arc<()>> {
        match self {
            Self::Observation(_) => None,
            Self::Comparison(request) => Some(request.identity()),
        }
    }

    fn run(self, session: &SharedSession) -> Completed {
        match self {
            Self::Observation(request) => Completed::Observation(Box::new(request.run(session))),
            Self::Comparison(request) => Completed::Comparison(Box::new(request.run(session))),
        }
    }
}

struct Worker {
    request: Request,
    session: SharedSession,
    actor: Actor,
    _operation: Option<DocumentOperation>,
}

impl Worker {
    fn run(self) {
        let Self {
            request,
            session,
            actor,
            _operation,
        } = self;
        let token = request.token();
        let comparison = request.comparison_identity();
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| request.run(&session)));
        let mut session = lock(&session);
        match result {
            Ok(Completed::Observation(completed)) => {
                let _ = session.finish_compiled_terrain_snapshot(actor, *completed);
            }
            Ok(Completed::Comparison(completed)) => {
                let _ = session.finish_compiled_terrain_basis_comparison(actor, *completed);
            }
            Err(_) if comparison.is_some() => {
                let _ = session.fail_compiled_terrain_basis_comparison_start(
                    actor,
                    token,
                    comparison
                        .as_ref()
                        .expect("Comparison identity was captured"),
                    "Native terrain basis comparison worker panicked".into(),
                );
            }
            Err(_) => {
                let _ = session.fail_compiled_terrain_snapshot_start(
                    actor,
                    token,
                    "Native terrain worker panicked; cleanup is unconfirmed".into(),
                    native_runtime::CleanupDisposition::Unconfirmed,
                );
            }
        }
    }
}

fn spawn(
    session: &SharedSession,
    actor: Actor,
    request: Request,
    operation: Option<DocumentOperation>,
) -> Result<Value, String> {
    let token = request.token();
    let comparison = request.comparison_identity();
    let worker = Worker {
        request,
        session: session.clone(),
        actor,
        _operation: operation,
    };
    let result = std::thread::Builder::new()
        .name(
            if comparison.is_some() {
                "native-basis"
            } else {
                "native-terrain"
            }
            .into(),
        )
        .spawn(move || worker.run());
    if let Err(error) = result {
        return if let Some(identity) = &comparison {
            lock(session).fail_compiled_terrain_basis_comparison_start(
                actor,
                token,
                identity,
                format!("Could not start native terrain basis comparison: {error}"),
            )
        } else {
            lock(session).fail_compiled_terrain_snapshot_start(
                actor,
                token,
                format!("Could not start native terrain observation: {error}"),
                native_runtime::CleanupDisposition::Confirmed,
            )
        };
    }
    Ok(lock(session).compiled_terrain_snapshot())
}

pub(super) fn start(
    session: &SharedSession,
    actor: Actor,
    snapshot: String,
    operation: Option<DocumentOperation>,
) -> Result<Value, String> {
    let request = lock(session).begin_compiled_terrain_snapshot(actor, &snapshot)?;
    spawn(
        session,
        actor,
        Request::Observation(Box::new(request)),
        operation,
    )
}

pub(super) fn compare(
    session: &SharedSession,
    actor: Actor,
    token: u64,
    options: ComparisonOptions,
    operation: Option<DocumentOperation>,
) -> Result<Value, String> {
    let request = lock(session).begin_compiled_terrain_basis_comparison(actor, token, options)?;
    spawn(
        session,
        actor,
        Request::Comparison(Box::new(request)),
        operation,
    )
}
