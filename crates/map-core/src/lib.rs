//! Editor-independent map model.

pub mod brush;
pub mod displacement;
pub mod document;
pub mod edit;
pub mod plane;
pub mod query;
pub mod surface;
pub mod transform;

pub use brush::{BrushError, BrushGeometry, Face, texel_uv};
pub use displacement::{DisplacementError, DisplacementMesh};
pub use document::{Document, EditError, Object, ObjectId, Step, Transaction};
pub use plane::Plane;
