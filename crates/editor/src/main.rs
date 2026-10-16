mod carve_ui;
mod checks_ui;
mod clipboard_ui;
mod compile_region_ui;
mod compile_ui;
mod components_ui;
mod connection_ui;
mod context_ui;
mod cubemap_ui;
mod entity_placement_ui;
mod fog_ui;
mod grid_defaults_ui;
mod groups_ui;
mod history_ui;
mod hollow_ui;
mod layers_ui;
mod lighting_ui;
mod linked_path_ui;
mod management_ui;
mod material_replace_ui;
mod material_source_ui;
mod measurements_ui;
mod merge_ui;
mod model_browser_ui;
mod model_ui;
mod overlay_ui;
mod particles_ui;
mod placement_ui;
mod pointfile_ui;
mod polygon_ui;
mod prefabs_ui;
mod primitive_ui;
mod recent_maps_ui;
mod recovery_ui;
mod rename_ui;
mod repeat_ui;
mod saved_views_ui;
mod sounds_ui;
mod statistics_ui;
mod surface_group_ui;
mod terrain_triangles_ui;
mod terrain_ui;
mod working_region_ui;
#[cfg(test)]
use editor_core::test_assets;
mod ui;
mod viewport;
mod volume_ui;

pub use editor_ui::*;

use editor_core::startup::{Application, Args};
use editor_core::{
    game, identity, inspector, interaction, material_favorites, mcp, model_preview, recent_maps,
    recovery, runtime, session, spatial, steam,
};
use mcp::EditorServer;
use render::GpuContext;
use runtime::{SharedSession, lock};
use session::{Actor, Session};
use std::net::{Ipv4Addr, SocketAddr};
const BLOCK_SIZE: f64 = 128.0;
const ACTIVITY_SHOWN: usize = 30;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse()?;
    let application = Application::open(&args)?;
    let session = application.session.clone();
    if args.headless {
        if let Some(Err(message)) = &application.open_message {
            eprintln!("{message}");
        }
        return Ok(mcp::serve_stdio(EditorServer::new(session.clone(), None))?);
    }

    let ctx = GpuContext::new()?;
    let server = EditorServer::new(session.clone(), Some(ctx.clone()));

    let mcp_status = match args.mcp_port {
        Some(port) => {
            match mcp::spawn_http(server, SocketAddr::from((Ipv4Addr::LOCALHOST, port))) {
                Ok(addr) => format!("MCP http://{addr}/mcp"),
                Err(message) => message,
            }
        }
        None => "MCP off".into(),
    };

    ui::run(session, ctx, mcp_status, application.open_message.clone())
}
