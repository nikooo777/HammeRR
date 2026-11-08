use serde_json::Value;

use super::{DocumentOperation, SharedSession, lock};
use crate::session::{Actor, compiled_terrain_snapshot::Request};

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
        let result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| request.run(&session)));
        let mut session = lock(&session);
        match result {
            Ok(completed) => {
                let _ = session.finish_compiled_terrain_snapshot(actor, completed);
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

pub(super) fn start(
    session: &SharedSession,
    actor: Actor,
    snapshot: String,
    operation: Option<DocumentOperation>,
) -> Result<Value, String> {
    let request = lock(session).begin_compiled_terrain_snapshot(actor, &snapshot)?;
    let token = request.token();
    let worker = Worker {
        request,
        session: session.clone(),
        actor,
        _operation: operation,
    };
    let result = std::thread::Builder::new()
        .name("native-terrain".into())
        .spawn(move || worker.run());
    if let Err(error) = result {
        return lock(session).fail_compiled_terrain_snapshot_start(
            actor,
            token,
            format!("Could not start native terrain observation: {error}"),
            native_runtime::CleanupDisposition::Confirmed,
        );
    }
    Ok(lock(session).compiled_terrain_snapshot())
}
