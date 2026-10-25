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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiler_ceiling_preserves_lower_limits_and_caps_raised_fields() {
        let defaults = PakLimits::default();
        assert_eq!(
            bounded(PakLimits {
                max_archive_bytes: 1,
                max_entries: defaults.max_entries,
                max_entry_bytes: usize::MAX,
                max_total_bytes: 2,
                max_name_bytes: usize::MAX,
                max_bsp_bytes: defaults.max_bsp_bytes,
            }),
            PakLimits {
                max_archive_bytes: 1,
                max_entries: defaults.max_entries,
                max_entry_bytes: defaults.max_entry_bytes,
                max_total_bytes: 2,
                max_name_bytes: defaults.max_name_bytes,
                max_bsp_bytes: defaults.max_bsp_bytes,
            }
        );
    }
}
