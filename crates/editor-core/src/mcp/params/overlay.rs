use super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OverlayPreviewArgs {
    pub snapshot: String,
    pub source: FaceRef,
    pub additional_sources: Option<Vec<FaceRef>>,
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
    pub reference: Option<FaceRef>,
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
    pub additional_sources: Option<Vec<FaceRef>>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OverlayCornersArgs {
    pub snapshot: String,
    pub id: Option<String>,
    pub corners: [[f64; 2]; 4],
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(extend("type" = "object"))]
pub enum OverlayCornerHandleArgs {
    Inspect,
    Begin {
        token: u64,
        width: u32,
        height: u32,
        camera: Option<CameraSpec>,
        snapshot: String,
    },
    Pick {
        token: u64,
        x: f64,
        y: f64,
        width: u32,
        height: u32,
    },
    Move {
        token: u64,
        x: f64,
        y: f64,
        width: u32,
        height: u32,
    },
    Finish {
        token: u64,
    },
    Cancel {
        token: u64,
    },
}
