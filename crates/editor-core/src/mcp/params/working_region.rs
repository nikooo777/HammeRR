use super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkingRegionInspectArgs {
    #[serde(default)]
    pub offset: usize,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkingRegionSetArgs {
    pub snapshot: String,
    pub action: String,
    pub enabled: Option<bool>,
    pub mins: Option<[f64; 3]>,
    pub maxs: Option<[f64; 3]>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_require_a_snapshot_and_reject_unknown_metadata() {
        let valid = json!({"snapshot":"current","action":"set","mins":[-1.25,-2.5,-3.75],"maxs":[4.25,5.5,6.75]});
        assert!(serde_json::from_value::<WorkingRegionSetArgs>(valid.clone()).is_ok());
        let mut missing = valid.clone();
        missing.as_object_mut().unwrap().remove("snapshot");
        assert!(serde_json::from_value::<WorkingRegionSetArgs>(missing).is_err());
        let mut unknown = valid;
        unknown["compile_region"] = json!(true);
        assert!(serde_json::from_value::<WorkingRegionSetArgs>(unknown).is_err());
    }
}
