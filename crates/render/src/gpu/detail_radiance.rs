use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const MAX_TABLE_BYTES: usize = 262_144;
const MAX_DRAW_METADATA_BYTES: usize = 16_384 * std::mem::size_of::<u32>();
const MAX_RETAINED_BYTES: usize = 4 * (MAX_TABLE_BYTES + MAX_DRAW_METADATA_BYTES + BINDING_BYTES);
const BINDING_BYTES: usize = 256;

pub(super) struct Reservation {
    allocated: Arc<AtomicUsize>,
    bytes: usize,
}

impl Reservation {
    pub(super) fn new(
        allocated: &Arc<AtomicUsize>,
        table_bytes: usize,
        ordinals: usize,
    ) -> Result<Self, String> {
        let ordinal_bytes = ordinals
            .checked_mul(std::mem::size_of::<u32>())
            .filter(|bytes| *bytes <= MAX_DRAW_METADATA_BYTES)
            .ok_or("Detail HDR draw metadata exceeds its record budget")?;
        let bytes = table_bytes
            .checked_add(ordinal_bytes)
            .ok_or("Detail HDR retained byte count overflowed")?
            .checked_add(BINDING_BYTES)
            .ok_or("Detail HDR GPU byte count overflowed")?;
        #[allow(deprecated, reason = "Atomic::try_update requires Rust 1.95")]
        allocated.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            current.checked_add(bytes).filter(|total| *total <= MAX_RETAINED_BYTES)
        }).map_err(|_| "Retained detail HDR GPU tables, draw metadata and replacement overlap exceed the four-table budget".to_owned())?;
        Ok(Self {
            allocated: allocated.clone(),
            bytes,
        })
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.allocated.fetch_sub(self.bytes, Ordering::Relaxed);
    }
}
