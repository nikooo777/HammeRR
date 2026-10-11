//! Editor-independent map model.

pub mod brush;
pub mod checks;
pub mod clip;
pub mod connections;
pub mod context;
pub mod displacement;
pub mod document;
pub mod edit;
pub mod picking;
pub mod plane;
pub mod primitive;
pub mod query;
pub mod references;
pub mod region;
pub mod saved_views;
pub mod subobject;
pub mod surface;
pub mod surface_group;
pub mod texture_chain;
pub mod transform;

pub use brush::{BrushError, BrushGeometry, Face, texel_uv};
pub use displacement::{DisplacementError, DisplacementMesh};
pub use document::{Document, EditError, Object, ObjectId, Step, Transaction};
pub use plane::Plane;
