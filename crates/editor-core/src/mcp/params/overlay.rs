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

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OverlayTargetsArgs {
    pub query: Option<String>,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OverlayRebindArgs {
    pub snapshot: String,
    pub id: String,
    pub source: FaceRef,
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
        let rebind = json!({"snapshot":"current", "id":"entity:12", "source":{"solid":1,"side":2}});
        assert!(serde_json::from_value::<OverlayRebindArgs>(rebind.clone()).is_ok());
        for field in ["id", "source", "snapshot"] {
            let mut missing = rebind.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(serde_json::from_value::<OverlayRebindArgs>(missing).is_err());
        }
        for field in ["material", "u", "BasisU", "sides", "origin"] {
            let mut invalid = rebind.clone();
            invalid[field] = json!("replacement");
            assert!(serde_json::from_value::<OverlayRebindArgs>(invalid).is_err());
        }
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
