use source_formats::bsp::PakLimits;

pub(crate) fn bounded(limits: PakLimits) -> PakLimits {
    let defaults = PakLimits::default();
    PakLimits {
        max_archive_bytes: limits.max_archive_bytes.min(defaults.max_archive_bytes),
        max_entries: limits.max_entries.min(defaults.max_entries),
        max_entry_bytes: limits.max_entry_bytes.min(defaults.max_entry_bytes),
        max_total_bytes: limits.max_total_bytes.min(defaults.max_total_bytes),
        max_name_bytes: limits.max_name_bytes.min(defaults.max_name_bytes),
        max_bsp_bytes: limits.max_bsp_bytes.min(defaults.max_bsp_bytes),
    }
}
