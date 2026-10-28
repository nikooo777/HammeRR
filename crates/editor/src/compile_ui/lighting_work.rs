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
