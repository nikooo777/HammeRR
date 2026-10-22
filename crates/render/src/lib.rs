//! wgpu renderer for Source maps, independent of the editor UI.

pub mod camera;
pub mod fog;
pub mod gpu;
mod grid;
pub mod handles;
pub mod icons;
pub mod lighting;
pub mod materials;
pub mod models;
pub mod normal_maps;
pub mod references;
pub mod ropes;
pub mod scene;
pub mod sky;
pub mod sky3d;

pub use camera::{Camera, Projection};
pub use gpu::{
    AreaGuide, COLOR_FORMAT, ClipGuide, GpuContext, GpuScene, PaintGuide, RenderTarget, Renderer,
    SceneKey, SurfaceView, ViewOptions,
};
pub use materials::{MaterialId, MaterialLibrary};
pub use scene::SceneMesh;
pub use wgpu;
