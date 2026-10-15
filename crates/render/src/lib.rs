//! wgpu renderer for Source maps, independent of the editor UI.

pub mod camera;
pub mod gpu;
mod grid;
pub mod handles;
mod helpers;
pub mod materials;
pub mod models;
pub mod scene;

pub use camera::{Camera, Projection};
pub use gpu::{
    AreaGuide, COLOR_FORMAT, ClipGuide, GpuContext, GpuScene, PaintGuide, RenderTarget, Renderer,
    SurfaceView, ViewOptions,
};
pub use materials::{MaterialId, MaterialLibrary};
pub use scene::SceneMesh;
pub use wgpu;
