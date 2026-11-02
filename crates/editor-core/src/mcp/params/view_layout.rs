use super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ViewLayoutGetArgs {
    pub width: Option<u32>,
    pub height: Option<u32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(extend("type" = "object"))]
pub enum ViewLayoutArgs {
    Update {
        layout: Option<crate::session::view_layout::Layout>,
        linked_orthographic: Option<bool>,
        snapshot: Option<String>,
    },
    Activate {
        pane: usize,
        snapshot: Option<String>,
    },
    Camera {
        pane: usize,
        camera: CameraSpec,
        aspect: Option<f32>,
        snapshot: Option<String>,
    },
    Projection {
        pane: usize,
        projection: String,
        snapshot: Option<String>,
    },
    Frame {
        aspects: Option<[f32; crate::session::view_layout::MAX_PANES]>,
        snapshot: Option<String>,
    },
}
