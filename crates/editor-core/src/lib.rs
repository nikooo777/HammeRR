#![recursion_limit = "256"]

mod bounded_file;
pub mod build_recipes;
pub mod build_settings;
pub mod color;
pub mod color_palette;
pub mod game;
pub mod grid_preferences;
pub mod identity;
pub mod inspector;
pub mod interaction;
pub mod material_favorites;
pub mod materials;
pub mod model_preview;
pub mod navigation_preferences;
mod path_claims;
pub mod pivot_preferences;
pub mod prefabs;
mod preference_file;
pub mod preview_preferences;
pub mod recent_maps;
pub mod recovery;
mod report_path;
pub mod runtime;
pub mod saved_defaults;
pub mod selection_preferences;
pub mod session;
pub mod shortcuts;
pub mod sky_assets;
pub mod spatial;
pub mod sprite_resource;
pub mod startup;
pub mod steam;

#[cfg(feature = "mcp")]
pub mod mcp;
