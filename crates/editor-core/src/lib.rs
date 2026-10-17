pub mod build_settings;
pub mod game;
pub mod grid_preferences;
pub mod identity;
pub mod inspector;
pub mod interaction;
pub mod material_favorites;
pub mod materials;
pub mod model_preview;
pub mod prefabs;
pub mod recent_maps;
pub mod recovery;
pub mod runtime;
pub mod session;
pub mod sky_assets;
pub mod spatial;
pub mod startup;
pub mod steam;

#[cfg(feature = "mcp")]
pub mod mcp;

#[cfg(any(test, feature = "test-support"))]
pub mod test_assets;
