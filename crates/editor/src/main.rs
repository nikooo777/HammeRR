#![recursion_limit = "256"]

mod atlas_ui;
mod carve_ui;
mod checks_ui;
mod clipboard_ui;
mod color_correction_ui;
mod color_ui;
mod compile_region_ui;
mod compile_ui;
mod components_ui;
mod connection_ui;
mod context_ui;
mod cubemap_ui;
mod entity_gallery_ui;
mod entity_graph_ui;
mod entity_placement_ui;
mod face_edit_ui;
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
mod navigation_defaults_ui;
mod overlay_ui;
mod particles_ui;
mod pivot_defaults_ui;
mod placement_ui;
mod pointfile_ui;
mod polygon_ui;
mod prefabs_ui;
mod preview_defaults_ui;
mod primitive_ui;
mod recent_maps_ui;
mod recovery_ui;
mod reference_images_ui;
mod rename_ui;
mod repeat_ui;
mod saved_views_ui;
mod selection_defaults_ui;
mod shortcuts_ui;
mod sky3d_ui;
mod sky_assets_ui;
mod sounds_ui;
mod statistics_ui;
mod surface_group_ui;
mod terrain_triangles_ui;
mod terrain_ui;
mod working_region_ui;

mod ui;
mod viewport;
mod volume_ui;

pub use editor_ui::*;

use editor_core::startup::{Application, Args};
use editor_core::{
    game, identity, inspector, interaction, material_favorites, mcp, model_preview, recent_maps,
    recovery, runtime, session, spatial, steam,
};

use render::GpuContext;
use runtime::{DocumentHandle, SharedSession, lock};
use session::{Actor, Session};
use std::net::{Ipv4Addr, SocketAddr};
const BLOCK_SIZE: f64 = 128.0;
const ACTIVITY_SHOWN: usize = 30;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse()?;
    let mut http = None;
    let mut workspace_owner = None;
    let application = Application::open(&args)?;
    let document = application.document();
    let factory = application.workspace();
    let result = (|| {
        if args.headless {
            if let Some(Err(message)) = &application.open_message {
                eprintln!("{message}");
            }
            factory.set_visibility(
                document.id(),
                editor_core::startup::DocumentVisibility::Headless,
            )?;
            let owner = mcp::WorkspaceOwner::start(factory.clone(), None, None)?;
            let result = mcp::serve_workspace_stdio(owner.server()).map_err(Into::into);
            return owner.finish(result);
        }

        let ctx = GpuContext::new()?;
        let server_ctx = ctx.clone();
        ui::run(
            factory.clone(),
            document.clone(),
            ctx,
            application.open_message.clone(),
            |workspace_ui| {
                let owner = mcp::WorkspaceOwner::start(
                    factory.clone(),
                    Some(server_ctx),
                    Some(workspace_ui),
                )?;
                let workspace = owner.workspace();
                let server = owner.server();
                let mcp_status = match args.mcp_port {
                    Some(port) => {
                        match mcp::spawn_workspace_http(
                            server,
                            SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
                        ) {
                            Ok(server) => {
                                let status = format!("MCP http://{}/mcp", server.local_addr());
                                http = Some(server);
                                status
                            }
                            Err(message) => message,
                        }
                    }
                    None => "MCP off".into(),
                };
                workspace_owner = Some(owner);
                Ok((workspace, mcp_status))
            },
        )
    })();
    let result = match workspace_owner {
        Some(owner) => owner.finish(result),
        None => result,
    };
    if let Some(server) = &mut http {
        server.stop();
    }
    let result = application.finish(result);
    match http {
        Some(server) => server.finish(result),
        None => result,
    }
}
