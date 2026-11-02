use serde_json::Value;

use super::{DocumentOperation, SharedSession, lock};
use crate::session::{Actor, linked_path_observation::Options};

pub(super) fn start(
    session: &SharedSession,
    actor: Actor,
    options: Options,
    operation: Option<DocumentOperation>,
) -> Result<Value, String> {
    let request = lock(session).begin_linked_path_observation(actor, options)?;
    let token = request.token.clone();
    let worker_token = token.clone();
    let worker = (request, session.clone(), operation);
    let result = std::thread::Builder::new()
        .name("native-movement".into())
        .spawn(move || {
            let (request, session, _operation) = worker;
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| request.run(&session))) {
                Ok(completed) => {
                    let _ = lock(&session).finish_linked_path_observation(actor, completed);
                }
                Err(_) => {
                    let _ = lock(&session).fail_linked_path_observation_start(
                        &worker_token,
                        "Native movement worker panicked; cleanup is unconfirmed".into(),
                        native_runtime::CleanupDisposition::Unconfirmed,
                    );
                }
            }
        });
    if let Err(error) = result {
        return lock(session).fail_linked_path_observation_start(
            &token,
            format!("Could not start native movement worker: {error}"),
            native_runtime::CleanupDisposition::Confirmed,
        );
    }
    Ok(lock(session).linked_path_observation(false))
}
