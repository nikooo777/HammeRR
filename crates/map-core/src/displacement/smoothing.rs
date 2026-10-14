use crate::EditError;
use source_formats::vmf::DispInfo;

pub fn validate(angle: Option<f64>) -> Result<(), String> {
    if angle.is_some_and(|angle| !angle.is_finite() || !(0.0..=180.0).contains(&angle)) {
        return Err("Terrain border smoothing angle must be 0–180 degrees".into());
    }
    Ok(())
}

pub fn angle(info: &DispInfo) -> Result<Option<f64>, String> {
    let values = info
        .extra_properties
        .iter()
        .filter(|(key, _)| key.eq_ignore_ascii_case("smooth"))
        .map(|(_, value)| value)
        .collect::<Vec<_>>();
    let value =
        match values.as_slice() {
            [] => return Ok(None),
            [value] => value
                .trim()
                .parse::<f64>()
                .map_err(|_| "Terrain smooth field must be a finite angle".to_owned())?,
            _ => return Err(
                "Terrain has multiple smooth fields; choose its border smoothing to repair them"
                    .into(),
            ),
        };
    validate(Some(value))?;
    Ok(Some(value))
}

pub fn edit(info: &DispInfo, value: Option<f64>) -> Result<DispInfo, EditError> {
    validate(value).map_err(EditError::Invalid)?;
    if angle(info).ok() == Some(value) {
        return Ok(info.clone());
    }
    let mut result = info.clone();
    result
        .extra_properties
        .0
        .retain(|(key, _)| !key.eq_ignore_ascii_case("smooth"));
    if let Some(value) = value {
        result.extra_properties.set("smooth", value.to_string());
    }
    Ok(result)
}
