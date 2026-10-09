//! Editor-independent map model.

pub mod brush;
pub mod displacement;
pub mod plane;

pub use brush::{BrushError, BrushGeometry, Face, texel_uv};
pub use displacement::{DisplacementError, DisplacementMesh};
pub use plane::Plane;
