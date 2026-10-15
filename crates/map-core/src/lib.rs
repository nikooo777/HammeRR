//! Editor-independent map model.

pub mod brush;
pub mod carve;
pub mod merge;
pub mod checks;
pub mod clip;
pub mod connections;
pub mod context;
pub mod displacement;
pub mod document;
pub mod edit;
pub mod hollow;
pub mod groups;
pub mod light;
pub mod partition_metadata;
pub mod picking;
pub mod plane;
pub mod polygon;
pub mod primitive;
pub mod query;
pub mod references;
pub mod region;
pub mod repeat;
pub mod saved_views;
pub mod subobject;
pub mod surface;
pub mod surface_group;
pub mod texture_chain;
pub mod texture_deform;
pub mod transform;

pub use brush::{BrushError, BrushGeometry, Face, texel_uv};
pub use displacement::{DisplacementError, DisplacementMesh};
pub use document::{Document, EditError, Object, ObjectId, Step, Transaction};
pub use plane::Plane;
