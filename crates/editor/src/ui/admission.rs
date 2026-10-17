use std::rc::{Rc, Weak};

use slint::ComponentHandle;

use crate::AppWindow;
use crate::runtime::{DocumentAccess, DocumentContinuation};

pub(super) struct Callback<Owner> {
    window: slint::Weak<AppWindow>,
    owner: Weak<Owner>,
    target: DocumentContinuation,
}

impl<Owner> Callback<Owner> {
    pub(super) fn new(window: &AppWindow, owner: &Rc<Owner>, target: &DocumentAccess) -> Self {
        Self {
            window: window.as_weak(),
            owner: Rc::downgrade(owner),
            target: target.downgrade(),
        }
    }

    pub(super) fn input<R>(&self, blocked: R, body: impl FnOnce(AppWindow, Rc<Owner>) -> R) -> R {
        self.run(blocked, true, body)
    }

    pub(super) fn frame(&self, body: impl FnOnce(AppWindow, Rc<Owner>)) {
        self.run((), false, body);
    }

    pub(super) fn focus_lost(
        &self,
        body: impl FnOnce(AppWindow, Rc<Owner>),
        release: impl FnOnce(&Rc<Owner>),
    ) {
        self.input((), body);
        if let Some(owner) = self.owner.upgrade() {
            release(&owner);
        }
        if let Some(window) = self.window.upgrade() {
            window.set_flying(false);
        }
    }

    fn run<R>(
        &self,
        blocked: R,
        report_error: bool,
        body: impl FnOnce(AppWindow, Rc<Owner>) -> R,
    ) -> R {
        let (Some(window), Some(owner)) = (self.window.upgrade(), self.owner.upgrade()) else {
            return blocked;
        };
        let result = self.target.upgrade().and_then(|target| {
            target.with_shared(|_| Ok(body(window.clone_strong(), owner.clone())))
        });
        match result {
            Ok(result) => result,
            Err(error) => {
                if report_error {
                    window.set_message(error.into());
                }
                blocked
            }
        }
    }
}

#[cfg(test)]
mod tests;
