use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[derive(Default)]
pub(super) struct Slot(Arc<AtomicBool>);

impl Slot {
    pub(super) fn busy(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    pub(super) fn begin(&self) -> Option<Ticket> {
        self.0
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Ticket(self.0.clone()))
    }
}

pub(super) struct Ticket(Arc<AtomicBool>);

impl Drop for Ticket {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
