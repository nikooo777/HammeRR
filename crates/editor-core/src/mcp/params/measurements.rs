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
    BeginPick {
        token: u64,
        endpoint: crate::session::measurements::Endpoint,
        width: u32,
        height: u32,
        camera: Option<CameraSpec>,
        snapshot: String,
    },
    PickRay {
        token: u64,
        ray_origin: [f64; 3],
        ray_direction: [f64; 3],
    },
    PickPixel {
        token: u64,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    },
    CancelPick {
        token: u64,
    },
    AttachOrigin {
        token: u64,
        endpoint: crate::session::measurements::Endpoint,
        object: String,
    },
    DetachOrigin {
        token: u64,
        endpoint: crate::session::measurements::Endpoint,
    },
    WorldEndpoint {
        token: u64,
        endpoint: crate::session::measurements::Endpoint,
        point: [f64; 3],
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
