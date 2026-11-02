use crate::vmf::Entity;

pub const MIN_MODEL_SCALE: f32 = 1.0 / 64.0;
pub const MAX_MODEL_SCALE: f32 = 64.0;
pub const MAX_MODEL_SCALE_TEXT_BYTES: usize = 64;

pub fn scale_editable(entity: &Entity) -> bool {
    !entity.classname().is_some_and(|name| {
        [
            "prop_static",
            "prop_detail",
            "detail_prop",
            "prop_detail_sprite",
        ]
        .iter()
        .any(|class| name.eq_ignore_ascii_case(class))
    })
}

fn range_error() -> String {
    format!(
        "Model scale must be a finite native float from {MIN_MODEL_SCALE} to {MAX_MODEL_SCALE}; these are editor numeric limits"
    )
}

fn validate(value: f32) -> Result<f32, String> {
    if !value.is_finite() || !(MIN_MODEL_SCALE..=MAX_MODEL_SCALE).contains(&value) {
        return Err(range_error());
    }
    Ok(value)
}

fn unsupported(entity: &Entity) -> String {
    if entity
        .classname()
        .is_some_and(|name| name.eq_ignore_ascii_case("prop_static"))
    {
        "Static-prop output does not support modelscale other than 1; use a compatible dynamic model entity for scale".into()
    } else {
        "Native detail records require modelscale 1; dynamic model scaling does not apply to detail props".into()
    }
}

pub fn scale(entity: &Entity) -> Result<f32, String> {
    if entity.properties.get_all("modelscale").count() > 1 {
        return Err("Model scale has ambiguous duplicate modelscale properties".into());
    }
    let raw = entity.properties.get("modelscale");
    let value = raw.map_or(Ok(1.0), |raw| {
        if raw.len() > MAX_MODEL_SCALE_TEXT_BYTES {
            return Err(format!(
                "Model scale exceeds {MAX_MODEL_SCALE_TEXT_BYTES} UTF-8 bytes"
            ));
        }
        let value = raw.parse::<f32>().map_err(|_| range_error())?;
        validate(value)
    })?;
    if !scale_editable(entity) {
        if entity
            .classname()
            .is_some_and(|name| name.eq_ignore_ascii_case("prop_static"))
        {
            if value != 1.0 {
                return Err(unsupported(entity));
            }
        } else if raw.is_some_and(|value| value.parse::<f64>().ok() != Some(1.0)) {
            return Err(unsupported(entity));
        }
    }
    Ok(value)
}

pub fn compose(entity: &Entity, factor: f64) -> Result<f32, String> {
    let current = scale(entity)?;
    if !factor.is_finite() || factor <= 0.0 {
        return Err("Model scale factor must be finite and positive".into());
    }
    if !scale_editable(entity) && factor != 1.0 {
        return Err(unsupported(entity));
    }
    validate((f64::from(current) * factor) as f32)
}
