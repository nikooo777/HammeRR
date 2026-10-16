use super::*;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
#[schemars(extend("type" = "object"))]
pub enum MeasurementArgs {
    Preview {
        id: Option<u32>,
        name: String,
        start: [f64; 3],
        end: [f64; 3],
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
    Delete {
        id: u32,
        snapshot: String,
    },
    Frame {
        id: Option<u32>,
        token: Option<u64>,
        aspect: Option<f32>,
        snapshot: String,
    },
    View {
        show_guides: bool,
        snapshot: String,
    },
}
