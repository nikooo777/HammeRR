use std::path::PathBuf;
use std::process::ExitCode;

use compiler::{
    CompileContext, Control, FileIdentity, Level, LightingMode, Options, OutputLease, PhysicsMode,
    compile_with_context_controlled, finalize_reflections,
};
use source_formats::gamefs::GameFs;
use source_formats::vmf::Vmf;

const USAGE: &str = "usage: hammerr-compile <map.vmf> --game <gameinfo.txt> [-o <map.bsp>] [--lighting fullbright|direct_ldr|direct_ldr_hdr] [--physics none|engine] [--sky-quality fast|balanced|detailed] [--visibility fast|detailed] [--bounces 0..4] [--smoothing-angle 0..90] [--generate-reflections] [--reflection-engine-root <directory>] [--reflection-steam-root <directory>]";

fn run(control: &Control) -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let mut map = None;
    let mut game = None;
    let mut destination = None;
    let mut options = Options::default();
    let mut reflection_engine = None;
    let mut reflection_steam = PathBuf::new();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--generate-reflections") => options.generate_reflections = true,
            Some("--reflection-engine-root") => {
                reflection_engine = Some(PathBuf::from(args.next().ok_or(USAGE)?))
            }
            Some("--reflection-steam-root") => {
                reflection_steam = PathBuf::from(args.next().ok_or(USAGE)?)
            }
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
            Some("--smoothing-angle") => {
                let value = args.next().ok_or(USAGE)?;
                options.smoothing_angle = value.to_str().ok_or(USAGE)?.parse::<u8>()?;
            }
            Some("--bounces") => {
                let value = args.next().ok_or(USAGE)?;
                options.bounces = value.to_str().ok_or(USAGE)?.parse::<u8>()?;
                if options.bounces > compiler::MAX_LIGHT_BOUNCES {
                    return Err(format!(
                        "Light bounces must be between 0 and {}",
                        compiler::MAX_LIGHT_BOUNCES
                    )
                    .into());
                }
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
    options.validate()?;
    let lease = options
        .generate_reflections
        .then(|| OutputLease::acquire(&path, control))
        .transpose()?;
    let source_identity = options
        .generate_reflections
        .then(|| FileIdentity::capture(&map, control))
        .transpose()?;
    let path = lease
        .as_ref()
        .map(|lease| lease.path().to_path_buf())
        .unwrap_or(path);
    let source_path = source_identity
        .as_ref()
        .map(FileIdentity::path)
        .unwrap_or(&map);
    let vmf = Vmf::parse(&std::fs::read_to_string(source_path)?)?;
    if let Some(identity) = &source_identity {
        identity.verify(control)?;
    }
    let fs = GameFs::from_gameinfo(game)?;
    let context = options
        .generate_reflections
        .then(|| CompileContext::from_output(&path))
        .transpose()?;
    let mut output = compile_with_context_controlled(&vmf, &fs, options, context.as_ref(), control);
    if let Some(context) = &context {
        if output.bsp.is_some() && !output.report.has_errors() {
            let environment =
                native_runtime::resolve_environment(&native_runtime::CaptureEnvironment {
                    engine_root: reflection_engine.unwrap_or_else(|| fs.base_dir.clone()),
                    steam_root: reflection_steam,
                    mounts: fs
                        .mounts
                        .iter()
                        .map(|mount| native_runtime::ContentMount {
                            ids: mount.ids.clone(),
                            path: mount.mount.location().to_path_buf(),
                        })
                        .collect(),
                })?;
            output = finalize_reflections(output, context, &fs, &environment, control)?;
        }
    }
    for diagnostic in &output.report.diagnostics {
        let level = match diagnostic.level {
            Level::Info => "info",
            Level::Warning => "warning",
            Level::Error => "error",
        };
        let object = diagnostic.object.as_deref().unwrap_or("map");
        eprintln!("{level}: {object}: {}", diagnostic.message);
    }
    let published = if let Some(lease) = &lease {
        if let Some(identity) = &source_identity {
            identity.verify(control)?;
        }
        let prepared = output.prepare_write_controlled(&path, control)?;
        let pointfile = (output.bsp.is_none() && output.leak.is_some())
            .then(|| OutputLease::acquire(&path.with_extension("lin"), control))
            .transpose()?;
        let assets = output
            .cubemap_assets
            .as_ref()
            .map(|manifest| {
                compiler::cubemap_assets::capture_reflection_source_guard(manifest, &fs, control)
            })
            .transpose()?;
        lease.verify(control)?;
        let validate = || {
            if let Some(identity) = &source_identity {
                identity.verify_metadata()?;
            }
            if let Some(assets) = &assets {
                assets.verify_metadata()?;
            }
            Ok(())
        };
        match prepared {
            Some(prepared) if pointfile.is_some() => {
                let pointfile = pointfile.as_ref().unwrap();
                pointfile.verify(control)?;
                prepared.commit_checked_with(pointfile, control, validate)?
            }
            Some(prepared) => prepared.commit_checked_with(lease, control, validate)?,
            None => false,
        }
    } else {
        output.write_controlled(&path, control)?
    };
    if !published {
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
        "Occluders: {} · polygons: {} · vertex references: {}",
        stats.occluders, stats.occluder_polygons, stats.occluder_vertex_refs
    );
    println!(
        "Visibility {}: workers {} · {} cluster pairs removed · {} work units",
        stats.visibility_mode.label(),
        stats.visibility_workers,
        stats.visibility_removed_pairs,
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
    let ldr_prefix = if stats.hdr.is_some() { "LDR " } else { "" };
    println!(
        "{ldr_prefix}lighting {}: {} baked faces, {} samples, {} light sources, {} bytes",
        stats.lighting_mode.label(),
        stats.lighting_faces,
        stats.lightmap_samples,
        stats.world_lights,
        stats.lighting_bytes
    );
    println!(
        "{ldr_prefix}brush entity lightmaps: {} models · {} faces",
        stats.lit_brush_models, stats.model_lighting_faces
    );
    println!(
        "{ldr_prefix}normal-map lighting: {} faces · {} directional maps",
        stats.bump_faces, stats.bump_lightmaps
    );
    println!(
        "light culling: {} evaluated · {} hidden · {} beyond range · {} lookup work units",
        stats.light_evaluations,
        stats.light_pvs_skipped,
        stats.light_range_skipped,
        stats.light_culling_work
    );
    println!(
        "{ldr_prefix}light styles: {} · styled faces: {} · lightmaps: {} · switched groups: {}",
        stats.light_styles,
        stats.styled_faces,
        stats.light_style_maps,
        stats.switchable_light_groups
    );
    println!(
        "{ldr_prefix}indirect light: {} bounces, {} rays, {} samples lit, {} model faces, {} model samples lit, {} work units",
        stats.bounces,
        stats.bounce_rays,
        stats.bounce_samples,
        stats.model_bounce_faces,
        stats.model_bounce_samples,
        stats.bounce_work
    );
    println!(
        "brush smoothing: {} faces, {} corners, {} work units",
        stats.smoothed_faces, stats.smoothed_corners, stats.normal_work
    );
    println!(
        "{ldr_prefix}model ambient: {} probes, {} bytes",
        stats.ambient_samples, stats.ambient_bytes
    );
    if let Some(hdr) = &stats.hdr {
        println!(
            "HDR lighting: {} faces, {} samples, {} world lights, {} bytes, {} work units",
            hdr.lighting_faces,
            hdr.lightmap_samples,
            hdr.world_lights,
            hdr.lighting_bytes,
            hdr.lighting_work
        );
        println!(
            "HDR styles: {} · styled faces: {} · lightmaps: {} · brush models: {} · model faces: {} · normal-map faces: {} · directional maps: {}",
            hdr.light_styles,
            hdr.styled_faces,
            hdr.light_style_maps,
            hdr.lit_brush_models,
            hdr.model_lighting_faces,
            hdr.bump_faces,
            hdr.bump_lightmaps
        );
        println!(
            "HDR indirect light: {} bounces, {} rays, {} samples lit, {} model faces, {} model samples lit, {} work units",
            hdr.bounces,
            hdr.bounce_rays,
            hdr.bounce_samples,
            hdr.model_bounce_faces,
            hdr.model_bounce_samples,
            hdr.bounce_work
        );
        println!(
            "HDR model ambient: {} probes, {} bytes · native map data: {}",
            hdr.ambient_samples,
            hdr.ambient_bytes,
            if hdr.map_data_present {
                "HDR data ready"
            } else {
                "HDR unavailable: incomplete map data"
            }
        );
    }
    if stats.lighting_mode.performs_lighting() {
        println!(
            "Lighting work: LDR {} · HDR {} · shared total {}",
            stats.lighting_work,
            stats.hdr.as_ref().map_or(0, |hdr| hdr.lighting_work),
            stats.lighting_total_work
        );
    }
    println!(
        "static props: {}, model assets: {}, leaf references: {}",
        stats.static_props, stats.static_prop_models, stats.static_prop_leaf_refs
    );
    println!(
        "static-prop mesh shadows: {} props, {} models, {} triangles, {} disabled, {} preparation work units; {} alpha-test triangles, {} materials, {} textures, {} decoded alpha bytes",
        stats.prop_shadow_props,
        stats.prop_shadow_models,
        stats.prop_shadow_triangles,
        stats.prop_shadow_disabled,
        stats.prop_shadow_work,
        stats.prop_shadow_alpha_triangles,
        stats.prop_shadow_alpha_materials,
        stats.prop_shadow_alpha_textures,
        stats.prop_shadow_alpha_bytes
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
    let control = Control::default();
    match run_with_signals(&control) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run_with_signals(control: &Control) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(all(target_os = "linux", target_pointer_width = "64"))]
    {
        use std::sync::atomic::{AtomicBool, Ordering};
        static INTERRUPTED: AtomicBool = AtomicBool::new(false);
        extern "C" fn interrupt(_signal: libc::c_int) {
            INTERRUPTED.store(true, Ordering::Relaxed);
        }
        unsafe {
            libc::signal(libc::SIGINT, interrupt as libc::sighandler_t);
            libc::signal(libc::SIGTERM, interrupt as libc::sighandler_t);
        }
        let finished = AtomicBool::new(false);
        struct Finish<'a>(&'a AtomicBool);
        impl Drop for Finish<'_> {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Relaxed);
            }
        }
        return std::thread::scope(|scope| {
            scope.spawn(|| {
                while !finished.load(Ordering::Relaxed) {
                    if INTERRUPTED.load(Ordering::Relaxed) {
                        let _ = control.cancel();
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
            });
            let _finish = Finish(&finished);
            run(control)
        });
    }
    #[cfg(not(all(target_os = "linux", target_pointer_width = "64")))]
    run(control)
}
