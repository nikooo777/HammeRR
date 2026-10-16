use super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(extend("type" = "object"))]
pub enum LinkedPathArgs {
    Inspect {
        id: String,
    },
    Edit {
        nodes: Vec<LinkedPathNodeArgs>,
        snapshot: String,
        #[serde(default)]
        seed: Option<String>,
        #[serde(default)]
        prefix: Option<String>,
    },
    Preview {
        classname: String,
        prefix: String,
        points: Vec<[f64; 3]>,
        #[serde(default)]
        closed: bool,
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
pub struct LinkedPathNodeArgs {
    #[serde(default)]
    pub id: Option<String>,
    pub origin: [f64; 3],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_actions_have_an_object_schema_and_separate_owned_tokens() {
        let schema = schemars::schema_for!(LinkedPathArgs);
        assert_eq!(schema.get("type"), Some(&json!("object")));
        assert_eq!(
            schema.get("oneOf").and_then(Value::as_array).map(Vec::len),
            Some(6)
        );
        let preview = json!({"action":"preview","classname":"path_corner","prefix":"route","points":[[0,0,0],[64,0,0]],"snapshot":"current"});
        let args = serde_json::from_value::<LinkedPathArgs>(preview.clone()).unwrap();
        assert!(matches!(
            args,
            LinkedPathArgs::Preview { closed: false, .. }
        ));
        let changed = serde_json::from_value::<LinkedPathArgs>(json!({
            "action":"edit", "seed":"entity:2", "prefix":"added",
            "nodes":[{"id":"entity:2","origin":[0,0,0]},{"origin":[64,0,0]}],
            "snapshot":"current",
        }))
        .unwrap();
        assert!(matches!(
            changed,
            LinkedPathArgs::Edit {
                seed: Some(_),
                prefix: Some(_),
                ..
            }
        ));
        for key in ["classname", "prefix", "points", "snapshot"] {
            let mut missing = preview.clone();
            missing.as_object_mut().unwrap().remove(key);
            assert!(serde_json::from_value::<LinkedPathArgs>(missing).is_err());
        }
        for value in [
            json!({"action":"inspect","id":"entity:1","points":[]}),
            json!({"action":"edit","nodes":[{"id":"entity:1","origin":[0,0,0]}]}),
            json!({"action":"edit","nodes":[{"id":"entity:1","origin":[0,0]}],"snapshot":"current"}),
            json!({"action":"edit","nodes":[],"closed":true,"snapshot":"current"}),
            json!({"action":"edit","nodes":[],"seed":1,"snapshot":"current"}),
            json!({"action":"edit","nodes":[],"prefix":false,"snapshot":"current"}),
            json!({"action":"apply","token":1,"points":[[0,0,0]]}),
            json!({"action":"review"}),
            json!({"action":"cancel","token":"1"}),
            json!({"action":"preview","classname":"path_corner","prefix":"route","points":[[0,0]],"snapshot":"current"}),
        ] {
            assert!(serde_json::from_value::<LinkedPathArgs>(value).is_err());
        }
    }
}
