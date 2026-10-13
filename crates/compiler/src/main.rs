use std::path::PathBuf;
use std::process::ExitCode;

use compiler::{Level, LightingMode, Options, PhysicsMode, compile_with_options};
use source_formats::gamefs::GameFs;
use source_formats::vmf::Vmf;

const USAGE: &str = "usage: hammerr-compile <map.vmf> --game <gameinfo.txt> [-o <map.bsp>] [--lighting fullbright|direct_ldr] [--physics none|engine] [--sky-quality fast|balanced|detailed] [--visibility fast|detailed]";

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let mut map = None;
    let mut game = None;
    let mut destination = None;
    let mut options = Options::default();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--game") => game = Some(PathBuf::from(args.next().ok_or(USAGE)?)),
            Some("-o" | "--output") => destination = Some(PathBuf::from(args.next().ok_or(USAGE)?)),
            Some("--lighting") => {
                let name = args.next().ok_or(USAGE)?;
                options.lighting = LightingMode::parse(name.to_str().ok_or(USAGE)?)?;
            }
            Some("--sky-quality") => {
                let name = args.next().ok_or(USAGE)?;
                options.sky_quality = compiler::SkyQuality::parse(name.to_str().ok_or(USAGE)?)?;
            }
            Some("--physics") => {
                let name = args.next().ok_or(USAGE)?;
                options.physics = PhysicsMode::parse(name.to_str().ok_or(USAGE)?)?;
            }
            Some("--visibility") => {
                let name = args.next().ok_or(USAGE)?;
                options.visibility = compiler::VisibilityMode::parse(name.to_str().ok_or(USAGE)?)?;
            }
            Some("-h" | "--help") => {
                println!("{USAGE}");
                return Ok(());
            }
            Some(flag) if flag.starts_with('-') => return Err(USAGE.into()),
            _ if map.is_none() => map = Some(PathBuf::from(arg)),
            _ => return Err(USAGE.into()),
        }
    }
    let map = map.ok_or(USAGE)?;
    let game = game.ok_or(USAGE)?;
    let path = destination.unwrap_or_else(|| map.with_extension("bsp"));
    if path == map
        || !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("bsp"))
    {
        return Err("output must be a .bsp file distinct from the source map".into());
    }
    let vmf = Vmf::parse(&std::fs::read_to_string(&map)?)?;
    let fs = GameFs::from_gameinfo(game)?;
    let output = compile_with_options(&vmf, &fs, options);
    for diagnostic in &output.report.diagnostics {
        let level = match diagnostic.level {
            Level::Info => "info",
            Level::Warning => "warning",
            Level::Error => "error",
        };
        let object = diagnostic.object.as_deref().unwrap_or("map");
        eprintln!("{level}: {object}: {}", diagnostic.message);
    }
    if !output.write(&path)? {
        return Err("compilation failed; no BSP written".into());
    }
    let stats = &output.stats;
    println!(
        "{}: {} brushes, {} nodes, {} leaves, {} faces, {} models, {} cubemap samples, {} visibility clusters, {} water regions, {} sky-visible clusters in {:.1?}",
        path.display(),
        stats.brushes,
        stats.nodes,
        stats.leaves,
        stats.faces,
        stats.models,
        stats.cubemap_samples,
        stats.clusters,
        stats.water_regions,
        stats.sky_clusters,
        stats.elapsed
    );
    println!(
        "Visibility areas: {} · areaportals: {} · window portals: {} · clip vertices: {}",
        stats.areas, stats.areaportals, stats.areaportal_windows, stats.areaportal_vertices
    );
    println!(
        "Visibility {}: workers {} · {} cluster pairs removed · {} conservative fallbacks · {} work units",
        stats.visibility_mode.label(),
        stats.visibility_workers,
        stats.visibility_removed_pairs,
        stats.visibility_fallbacks,
        stats.visibility_work
    );
    println!(
        "overlays: {} · face references: {}",
        stats.overlays, stats.overlay_face_refs
    );
    println!(
        "hint surfaces: {} · added splits: {} · work units: {}",
        stats.hint_surfaces, stats.hint_splits, stats.hint_work
    );
    println!(
        "lighting {}: {} baked faces, {} samples, {} light sources, {} bytes",
        stats.lighting_mode.label(),
        stats.lighting_faces,
        stats.lightmap_samples,
        stats.world_lights,
        stats.lighting_bytes
    );
    println!(
        "model ambient: {} probes, {} bytes",
        stats.ambient_samples, stats.ambient_bytes
    );
    println!(
        "static props: {}, model assets: {}, leaf references: {}",
        stats.static_props, stats.static_prop_models, stats.static_prop_leaf_refs
    );
    println!(
        "terrain: {} surfaces, {} vertices, {} triangles, {} neighbor pairs",
        stats.displacements,
        stats.displacement_vertices,
        stats.displacement_triangles,
        stats.displacement_neighbor_pairs
    );
    println!(
        "physics {}: {} models, {} bodies, {} convexes, {} fluid regions, {} terrain triangles, {} bytes",
        stats.physics_mode.label(),
        stats.physics_models,
        stats.physics_solids,
        stats.physics_convexes,
        stats.physics_fluids,
        stats.physics_mesh_triangles,
        stats.physics_bytes
    );
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
