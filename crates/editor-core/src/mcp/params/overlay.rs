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

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OverlayCornersArgs {
    pub snapshot: String,
    pub id: Option<String>,
    pub corners: [[f64; 2]; 4],
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
    #[test]
    fn corner_requests_require_four_local_pairs_and_reject_binding_or_setting_changes() {
        let valid = json!({"snapshot":"current","id":"entity:12","corners":[[-24,-16],[-20,24],[28,16],[32,-20]]});
        let args: OverlayCornersArgs = serde_json::from_value(valid.clone()).unwrap();
        assert_eq!(args.corners[0], [-24.0, -16.0]);
        for field in ["source", "sides", "material", "BasisU", "origin", "u"] {
            let mut invalid = valid.clone();
            invalid[field] = json!("replacement");
            assert!(serde_json::from_value::<OverlayCornersArgs>(invalid).is_err());
        }
        for corners in [
            json!([[0, 0], [0, 1], [1, 1]]),
            json!([[0, 0, 0], [0, 1, 0], [1, 1, 0], [1, 0, 0]]),
        ] {
            let mut invalid = valid.clone();
            invalid["corners"] = corners;
            assert!(serde_json::from_value::<OverlayCornersArgs>(invalid).is_err());
        }
        let schema = serde_json::to_value(schemars::schema_for!(OverlayCornersArgs)).unwrap();
        assert_eq!(schema["additionalProperties"], false);
    }
}
