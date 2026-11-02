use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Options {
    pub draw_distance: Option<f64>,
    #[serde(default)]
    pub follow_camera: bool,
    pub filter: Option<super::super::management::ObjectFilter>,
}

impl Options {
    pub(super) fn validate(&self) -> Result<(), String> {
        if self
            .draw_distance
            .is_some_and(|distance| !distance.is_finite() || distance <= 0.0)
        {
            return Err("Draw distance must be finite and positive".into());
        }
        if let Some(filter) = &self.filter {
            filter.validate()?;
        }
        Ok(())
    }
}
