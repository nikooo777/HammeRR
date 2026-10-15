use super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OverlayPreviewArgs {
    pub snapshot: String,
    pub source: FaceRef,
    pub material: String,
    pub targetname: Option<String>,
    pub render_order: Option<u16>,
    pub u: Option<[f32; 2]>,
    pub v: Option<[f32; 2]>,
    pub fade_min: Option<f32>,
    pub fade_max: Option<f32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OverlayTokenArgs {
    pub token: u64,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OverlayRefitArgs {
    pub snapshot: String,
    pub id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_requests_require_an_explicit_source_and_reject_derived_projection_fields() {
        let valid =
            json!({"snapshot":"current","source":{"solid":1,"side":2},"material":"test/checker"});
        assert!(serde_json::from_value::<OverlayPreviewArgs>(valid.clone()).is_ok());
        let mut missing = valid.clone();
        missing.as_object_mut().unwrap().remove("source");
        assert!(serde_json::from_value::<OverlayPreviewArgs>(missing).is_err());
        let refit = json!({"snapshot":"current", "id":"entity:12"});
        assert!(serde_json::from_value::<OverlayRefitArgs>(refit.clone()).is_ok());
        for field in ["source", "material", "u", "BasisU", "sides"] {
            let mut invalid = refit.clone();
            invalid[field] = json!("replacement");
            assert!(serde_json::from_value::<OverlayRefitArgs>(invalid).is_err());
        }
        for field in ["BasisU", "BasisNormal", "uv0", "sides"] {
            let mut derived = valid.clone();
            derived[field] = json!("1 0 0");
            assert!(serde_json::from_value::<OverlayPreviewArgs>(derived).is_err());
        }
    }
}
