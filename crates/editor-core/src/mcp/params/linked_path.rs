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
    Topology {
        seed: String,
        classname: String,
        closed: bool,
        points: Option<Vec<[f64; 3]>>,
        snapshot: String,
    },
    Train {
        seed: String,
        train: String,
        speed: f64,
        start_on: bool,
        snapshot: String,
    },
    Duplicate {
        seed: String,
        train: Option<String>,
        offset: [f64; 3],
        policies: crate::session::repeat::Policies,
        snapshot: String,
    },
    Position {
        token: u64,
        row: usize,
        origin: [f64; 3],
    },
    BeginHandles {
        token: u64,
        row: usize,
        camera: Option<CameraSpec>,
        width: u32,
        height: u32,
        snapshot: String,
    },
    BeginDrag {
        token: u64,
        handle: String,
        pointer: Option<PointerArgs>,
    },
    UpdateDrag {
        token: u64,
        pointer: Option<PointerArgs>,
        amount: Option<[f64; 2]>,
    },
    FinishDrag {
        token: u64,
    },
    CancelDrag {
        token: u64,
    },
    CancelHandles {
        token: u64,
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
