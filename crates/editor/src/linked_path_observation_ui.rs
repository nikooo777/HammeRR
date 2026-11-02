use std::{cell::RefCell, rc::Rc, time::Duration};

use serde_json::Value;
use slint::ComponentHandle;

use crate::{
    LinkedPathWindow, identity,
    runtime::DocumentAccess,
    session::{Actor, linked_path_observation::Options},
};

fn summary(state: &Value) -> String {
    if state["busy"] == true {
        return if state["cancel_requested"] == true {
            "Stopping native observation and checking cleanup…"
        } else {
            "Observing an isolated native train. Your map stays unchanged."
        }
        .into();
    }
    if let Some(error) = state["error"].as_str() {
        return error.into();
    }
    let last = &state["last"];
    if last["status"] == "complete" {
        return format!(
            "{} native samples. {}",
            last["sample_count"],
            if last["current"] == true {
                "Source and cleanup verified."
            } else {
                "The map or build has changed since this observation."
            }
        );
    }
    if state["ready"] == true {
        "Save and compile a named train, then observe its native motion.".into()
    } else {
        "Native observation needs a saved, current build and a configured Linux CS:S capture runtime."
            .into()
    }
}

pub fn bind(window: &LinkedPathWindow, access: DocumentAccess) -> slint::Timer {
    let error = Rc::new(RefCell::new(None::<String>));
    window.on_observe_native_motion({
        let (window, target, error) = (window.as_weak(), access.downgrade(), error.clone());
        move |stationary| {
            let Some(window) = window.upgrade() else {
                return;
            };
            let result = (|| {
                let access = target.upgrade()?;
                let map_core::ObjectId::Entity(train) = identity::parse_id(&window.get_train_id())?
                else {
                    return Err("Choose an entity:<id> train".into());
                };
                let state =
                    access.with_session(|session| Ok(session.linked_path_observation(false)))?;
                let snapshot = state["snapshot"]
                    .as_str()
                    .ok_or("Movement observation snapshot is unavailable")?
                    .to_owned();
                access.start_linked_path_observation(
                    Actor::User,
                    Options {
                        train,
                        stationary,
                        sample_interval_ms: 200,
                        samples_per_phase: 8,
                        snapshot,
                    },
                )
            })();
            match result {
                Ok(state) => {
                    *error.borrow_mut() = None;
                    window.set_native_motion_ready(state["ready"] == true);
                    window.set_native_motion_busy(state["busy"] == true);
                    window.set_native_motion_status(summary(&state).into());
                }
                Err(message) => *error.borrow_mut() = Some(message),
            }
        }
    });
    window.on_cancel_native_motion({
        let (target, error) = (access.downgrade(), error.clone());
        move || {
            let result = (|| {
                let access = target.upgrade()?;
                access.with_session(|session| {
                    let state = session.linked_path_observation(false);
                    let token = state["token"]
                        .as_str()
                        .ok_or("No native movement observation is running")?;
                    session.cancel_linked_path_observation(Actor::User, token)
                })
            })();
            *error.borrow_mut() = result.err();
        }
    });
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(500), {
        let (window, target) = (window.as_weak(), access.downgrade());
        move || {
            let Some(window) = window.upgrade() else {
                return;
            };
            match target.upgrade().and_then(|access| {
                access.with_session(|session| Ok(session.linked_path_observation(false)))
            }) {
                Ok(state) => {
                    if state["busy"] == true {
                        *error.borrow_mut() = None;
                    }
                    window.set_native_motion_ready(state["ready"] == true);
                    window.set_native_motion_busy(state["busy"] == true);
                    let message = error.borrow().clone().unwrap_or_else(|| summary(&state));
                    window.set_native_motion_status(message.into());
                }
                Err(message) => {
                    window.set_native_motion_ready(false);
                    window.set_native_motion_busy(false);
                    window.set_native_motion_status(message.into());
                }
            }
        }
    });
    timer
}
