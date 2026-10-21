const UNITS_PER_MILLION: u64 = 1_000_000;
const DECIMAL_PLACES: usize = 6;

pub(super) fn millions(units: u64) -> String {
    let whole = units / UNITS_PER_MILLION;
    let remainder = units % UNITS_PER_MILLION;
    if remainder == 0 {
        return whole.to_string();
    }
    let fraction = format!("{remainder:06}");
    format!("{whole}.{}", fraction.trim_end_matches('0'))
}

pub(super) fn parse_millions(value: &str) -> Result<u64, String> {
    let invalid = || {
        "Enter 0.000001–10000 million lighting work units with at most six decimal places"
            .to_string()
    };
    let (whole, fraction) = value.trim().split_once('.').unwrap_or((value.trim(), ""));
    if whole.is_empty()
        || !whole.bytes().all(|digit| digit.is_ascii_digit())
        || !fraction.bytes().all(|digit| digit.is_ascii_digit())
        || fraction.len() > DECIMAL_PLACES
    {
        return Err(invalid());
    }
    let whole = whole.parse::<u64>().map_err(|_| invalid())?;
    let fraction_units = if fraction.is_empty() {
        0
    } else {
        let fraction_value = fraction.parse::<u64>().map_err(|_| invalid())?;
        fraction_value
            .checked_mul(10u64.pow((DECIMAL_PLACES - fraction.len()) as u32))
            .ok_or_else(invalid)?
    };
    let units = whole
        .checked_mul(UNITS_PER_MILLION)
        .and_then(|whole| whole.checked_add(fraction_units))
        .ok_or_else(invalid)?;
    compiler::validate_lighting_work_limit(units)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile_ui::Controller;
    use crate::runtime::lock;
    use crate::session::Session;
    use std::sync::{Arc, Mutex};

    #[test]
    fn million_units_roundtrip_exactly_without_clamping_or_float_rounding() {
        for units in [
            compiler::MIN_LIGHTING_WORK_LIMIT,
            999_999,
            1_000_001,
            1_234_567_890,
            compiler::DEFAULT_LIGHTING_WORK_LIMIT,
            compiler::MAX_LIGHTING_WORK_LIMIT,
        ] {
            assert_eq!(parse_millions(&millions(units)).unwrap(), units);
        }
        assert_eq!(millions(1_234_567_890), "1234.56789");
        for invalid in [
            "",
            "-1",
            "0",
            "10000.000001",
            "0.0000001",
            "1.0000000",
            "1e3",
            "NaN",
            "1.2.3",
            "18446744073709551615",
            "18446744073709551616",
            "1,000",
            ".1",
        ] {
            assert!(parse_millions(invalid).is_err(), "accepted {invalid}");
        }
    }

    #[test]
    fn lighting_work_controls_apply_exact_units_before_the_first_compile() {
        slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
            i_slint_backend_testing::TestingBackendOptions {
                renderer_name: Some("software".into()),
                mock_time: true,
                threading: false,
            },
        )))
        .unwrap();
        let shared = Arc::new(Mutex::new(Session::new(Err("no assets needed".into()))));
        let source = lock(&shared).document().vmf().clone();
        let history = lock(&shared).history_snapshot();
        let controller = Controller::new(&shared).unwrap();
        controller.show(&lock(&shared)).unwrap();
        assert!(lock(&shared).compile_status()["job"].is_null());
        assert!(controller.window.get_lighting_enabled());
        assert_eq!(controller.window.get_lighting_work_limit_millions(), "1000");
        controller
            .window
            .invoke_change_lighting_work_limit("1234.56789".into());
        assert_eq!(
            lock(&shared).compile_options().lighting_work_limit,
            1_234_567_890
        );
        assert_eq!(
            controller.window.get_lighting_work_limit_millions(),
            "1234.56789"
        );
        let selected = lock(&shared).compile_settings();
        for invalid in [
            "0",
            "-1",
            "10000.000001",
            "1.0000001",
            "18446744073709551615",
        ] {
            controller
                .window
                .invoke_change_lighting_work_limit(invalid.into());
            assert_eq!(lock(&shared).compile_settings(), selected);
            assert!(!controller.window.get_message().is_empty());
            assert_eq!(
                controller.window.get_lighting_work_limit_millions(),
                "1234.56789"
            );
        }
        controller.window.set_lighting_work_limit_editing(true);
        controller
            .window
            .set_lighting_work_limit_millions("9000".into());
        controller.refresh(&lock(&shared));
        assert_eq!(controller.window.get_lighting_work_limit_millions(), "9000");
        assert_eq!(lock(&shared).compile_settings(), selected);
        controller
            .window
            .invoke_change_lighting_work_limit("10000".into());
        assert_eq!(
            lock(&shared).compile_options().lighting_work_limit,
            compiler::MAX_LIGHTING_WORK_LIMIT
        );
        controller
            .window
            .invoke_change_lighting_work_limit("0.000001".into());
        assert_eq!(
            lock(&shared).compile_options().lighting_work_limit,
            compiler::MIN_LIGHTING_WORK_LIMIT
        );
        assert_eq!(
            controller.window.get_lighting_work_limit_millions(),
            "0.000001"
        );
        controller.window.set_lighting_work_limit_editing(true);
        controller
            .window
            .set_lighting_work_limit_millions("9000".into());
        controller.window.invoke_choose_recipe(0);
        controller.window.invoke_apply_recipe();
        assert_eq!(
            lock(&shared).compile_options().lighting_work_limit,
            compiler::DEFAULT_LIGHTING_WORK_LIMIT
        );
        assert_eq!(controller.window.get_lighting_work_limit_millions(), "1000");
        assert_eq!(lock(&shared).document().vmf(), &source);
        assert_eq!(lock(&shared).history_snapshot(), history);
    }
}
