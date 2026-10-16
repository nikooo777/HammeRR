use super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CubemapBindingsArgs {
    pub helper: Option<u32>,
    pub query: Option<String>,
    pub offset: Option<usize>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum SetCubemapBindingsArgs {
    Preview {
        helper: u32,
        faces: Vec<CubemapFaceRef>,
        operation: crate::session::cubemap_bindings::Operation,
        snapshot: String,
    },
    Review {
        token: u64,
    },
    Apply {
        token: u64,
    },
    Cancel {
        token: u64,
    },
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CubemapFaceRef {
    pub solid: u32,
    pub side: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binding_requests_require_explicit_faces_and_separate_preview_from_owned_commit() {
        let valid = json!({"action":"preview","helper":12,"faces":[{"solid":1,"side":2}],"operation":"bind","snapshot":"current"});
        assert!(serde_json::from_value::<SetCubemapBindingsArgs>(valid.clone()).is_ok());
        for key in ["helper", "faces", "operation", "snapshot"] {
            let mut missing = valid.clone();
            missing.as_object_mut().unwrap().remove(key);
            assert!(serde_json::from_value::<SetCubemapBindingsArgs>(missing).is_err());
        }
        for key in ["sides", "alias", "position", "token"] {
            let mut derived = valid.clone();
            derived[key] = json!("unreviewed");
            assert!(serde_json::from_value::<SetCubemapBindingsArgs>(derived).is_err());
        }
        let mut derived_face = valid.clone();
        derived_face["faces"][0]["alias"] = json!("derived");
        assert!(serde_json::from_value::<SetCubemapBindingsArgs>(derived_face).is_err());
        assert!(
            serde_json::from_value::<SetCubemapBindingsArgs>(
                json!({"action":"apply","token":1,"helper":12})
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<SetCubemapBindingsArgs>(json!({"action":"apply"})).is_err()
        );
    }
}
