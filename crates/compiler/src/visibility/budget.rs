use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use super::{Control, WORK_LIMIT};

const CREDIT_CHUNK: usize = 4096;

struct State {
    free: usize,
    leases: usize,
}

pub(super) struct Budget {
    state: Mutex<State>,
    returned: Condvar,
    control: Control,
}

impl Budget {
    pub fn new(free: usize, control: Control) -> Self {
        Self {
            state: Mutex::new(State { free, leases: 0 }),
            returned: Condvar::new(),
            control,
        }
    }
}

pub(super) struct Lease {
    budget: Arc<Budget>,
    credit: usize,
}

impl Lease {
    pub fn new(budget: Arc<Budget>) -> Self {
        Self { budget, credit: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.budget
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .free
            + self.credit
    }

    pub fn consume(&mut self, count: usize) -> Result<(), String> {
        if count == 0 {
            return Ok(());
        }
        if self.credit >= count {
            self.credit -= count;
            if self.credit == 0 {
                let mut state = self
                    .budget
                    .state
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                state.leases -= 1;
                self.budget.returned.notify_all();
            }
            return Ok(());
        }
        self.budget.control.checkpoint()?;
        let mut state = self
            .budget
            .state
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if self.credit != 0 {
            state.free += self.credit;
            state.leases -= 1;
            self.credit = 0;
            self.budget.returned.notify_all();
        }
        while state.free < count {
            self.budget.control.checkpoint()?;
            if state.leases == 0 {
                return Err(WORK_LIMIT.into());
            }
            state = self
                .budget
                .returned
                .wait_timeout(state, Duration::from_millis(5))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        let amount = count.max(CREDIT_CHUNK).min(state.free);
        state.free -= amount;
        self.credit = amount - count;
        if self.credit != 0 {
            state.leases += 1;
        }
        Ok(())
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if self.credit != 0 {
            let mut state = self
                .budget
                .state
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            state.free += self.credit;
            state.leases -= 1;
            self.budget.returned.notify_all();
        }
    }
}
