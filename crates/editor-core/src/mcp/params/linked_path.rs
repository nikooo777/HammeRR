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
    BeginPick {
        token: u64,
        row: usize,
        camera: Option<CameraSpec>,
        width: u32,
        height: u32,
        snapshot: String,
    },
    PickRay {
        token: u64,
        origin: [f64; 3],
        direction: [f64; 3],
    },
    PickPixel {
        token: u64,
        x: f64,
        y: f64,
        width: u32,
        height: u32,
    },
    CancelPick {
        token: u64,
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
            Some(10)
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
            json!({"action":"begin_pick","token":1,"row":0,"width":511,"height":511}),
            json!({"action":"begin_pick","token":1,"row":-1,"width":511,"height":511,"snapshot":"current"}),
            json!({"action":"pick_ray","token":1,"origin":[0,0,1],"direction":[0,0]}),
            json!({"action":"pick_pixel","token":1,"x":0,"y":0,"width":511,"height":511,"camera":{}}),
            json!({"action":"cancel_pick","token":1,"row":0}),
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

    #[test]
    fn path_surface_pick_actions_keep_exact_rays_and_continuous_pixels() {
        let ray: LinkedPathArgs = serde_json::from_value(json!({
            "action":"pick_ray","token":7,"origin":[0.125,-32.75,128.5],"direction":[0,0,-7],
        }))
        .unwrap();
        let LinkedPathArgs::PickRay {
            token,
            origin,
            direction,
        } = ray
        else {
            panic!()
        };
        assert_eq!(token, 7);
        assert_eq!(origin, [0.125, -32.75, 128.5]);
        assert_eq!(direction, [0.0, 0.0, -7.0]);
        let pixel: LinkedPathArgs = serde_json::from_value(json!({
            "action":"pick_pixel","token":8,"x":255.5,"y":255.5,"width":511,"height":511,
        }))
        .unwrap();
        let LinkedPathArgs::PickPixel {
            token,
            x,
            y,
            width,
            height,
        } = pixel
        else {
            panic!()
        };
        assert_eq!((token, width, height), (8, 511, 511));
        assert_eq!((x, y), (255.5, 255.5));
    }
}
