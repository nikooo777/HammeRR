use std::path::PathBuf;
use std::process::ExitCode;

use compiler::{
    CompileContext, Control, FileIdentity, Level, LightingMode, Options, OutputLease, PhysicsMode,
    WriteReservation, compile_with_source_controlled, finalize_reflections, instances,
};
use source_formats::file_publication::{BoundDestination, Policy};
use source_formats::gamefs::GameFs;
use source_formats::vmf::Vmf;

const MAX_PRINTED_LIGHTING_CONTRIBUTORS: usize = 8;

const USAGE: &str = "usage: hammerr-compile <map.vmf> --game <gameinfo.txt> [-o <map.bsp>] [--lighting fullbright|direct_ldr|direct_ldr_hdr] [--physics none|engine] [--sky-quality fast|balanced|detailed] [--lighting-work-limit <units>] [--visibility fast|detailed] [--bounces 0..4] [--smoothing-angle 0..90] [--pack-sky-assets] [--pack-water-assets] [--pack-static-prop-assets] [--bake-static-prop-vertices] [--brush-texture-shadows] [--generate-reflections] [--reflection-engine-root <directory>] [--reflection-steam-root <directory>]";

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
            Some("--pack-sky-assets") => options.pack_sky_assets = true,
            Some("--pack-water-assets") => options.pack_water_assets = true,
            Some("--pack-static-prop-assets") => options.pack_static_prop_assets = true,
            Some("--bake-static-prop-vertices") => options.bake_static_prop_vertices = true,
            Some("--brush-texture-shadows") => options.brush_texture_shadows = true,
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
            Some("--lighting-work-limit") => {
                let value = args.next().ok_or(USAGE)?;
                let value = value.to_str().ok_or(USAGE)?.parse::<u64>()?;
                options.lighting_work_limit = compiler::validate_lighting_work_limit(value)?;
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
                println!(
                    "{USAGE}\n\n--lighting-work-limit sets one aggregate preparation/LDR/HDR allowance in integer work units. Default {}; range {}..{}. Independent memory, sample and geometry limits still apply.\n\n--brush-texture-shadows enables binary alpha-test shadows for plain LightmappedGeneric world/detail brush faces and admitted initial-pose same-model self shadows. Requires lit mode; default off. Uses nearest mip0 base alpha, default cutoff 0.5, checked transforms and VTF clamp/repeat. Fractional/terrain alpha casters, unsupported requested materials and model alpha without an applicable baked self-shadow scope fail. No permanent model-to-world/other-model shadows or model radiance sources. Geometry/collision remain unchanged. Limits: 65536 boundary sides, 4096 pixels per axis, 64 MiB alpha, 256 MiB input reads, 64 KiB per VMT and 65536 material lookups; one texture frame/face/depth. Shared lighting work and memory limits also apply.\n\n--pack-water-assets packs original files for complete supported static Water material/Patch/texture closures. Every reachable Water material requires an explicit static cubemap with original LDR/HDR siblings. Runtime/default env_cubemap and missing or unsupported assets stop publication. Default off; available in every lighting mode without native capture.\n\n--pack-static-prop-assets packs original MDL, VVD, DX90 VTX, every declared supported VertexLitGeneric material and basetexture VTF, plus matching PHY only for emitted VPhysics props. Terminals require one basetexture and optionally literal $alphatest 1 at the default reference; authored alpha references remain unsupported. Supports include-only Patch chains up to eight include edges and one direct basetexture edit over a terminal; both original and replacement textures are retained. Supports bounded static MDL 44/48, body 0/root LOD 0, trivial local animation and empty or exact Metal.Medium prop metadata. Missing or unsupported inputs stop publication. Default off; independent of vertex lighting and available in every lighting mode. Requires standard CS:S shaders and surface/prop-data definitions with DX90 or newer at runtime.",
                    compiler::DEFAULT_LIGHTING_WORK_LIMIT,
                    compiler::MIN_LIGHTING_WORK_LIMIT,
                    compiler::MAX_LIGHTING_WORK_LIMIT
                );
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
    let source = BoundDestination::capture(&map, Policy::FollowSource)?;
    let destination = BoundDestination::capture(&path, Policy::ReplaceEntry)?;
    let pointfile = BoundDestination::capture(&path.with_extension("lin"), Policy::ReplaceEntry)?;
    let source_slots = source.claim_slots();
    if [&destination, &pointfile].iter().any(|output| {
        output
            .claim_slots()
            .iter()
            .any(|slot| source_slots.contains(slot))
    }) {
        return Err("BSP and pointfile outputs must be distinct from the source map".into());
    }
    let source_bytes = source.read_existing()?.ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "source map is missing")
    })?;
    let source_text = String::from_utf8(source_bytes)?;
    let vmf = Vmf::parse(&source_text)?;
    let has_instances = instances::requires_source(&vmf);
    let guarded = options.requires_guarded_publication_for(&vmf);
    let lease = guarded
        .then(|| OutputLease::acquire(&path, control))
        .transpose()?;
    let source_identity = (guarded && !has_instances)
        .then(|| FileIdentity::capture(&map, control))
        .transpose()?;
    let reservation = guarded
        .then(|| {
            WriteReservation::for_destinations(destination.clone(), pointfile.clone(), control)
        })
        .transpose()?;
    let path = lease
        .as_ref()
        .map(|lease| lease.path().to_path_buf())
        .unwrap_or(path);
    if let Some(identity) = &source_identity {
        identity.verify_bytes(source_text.as_bytes())?;
        identity.verify(control)?;
    }
    let source_context = has_instances
        .then(|| instances::SourceContext::capture(&map, control))
        .transpose()?;
    if let Some(source) = &source_context {
        source.verify_bytes(source_text.as_bytes())?;
    }
    let fs = GameFs::from_gameinfo(game)?;
    let context = options
        .generate_reflections
        .then(|| CompileContext::from_output(&path))
        .transpose()?;
    let mut output = compile_with_source_controlled(
        &vmf,
        &fs,
        options,
        context.as_ref(),
        source_context.as_ref(),
        control,
    );
    if options.pack_static_prop_assets
        && output.bsp.is_some()
        && output.static_prop_assets.is_none()
    {
        return Err(
            "Static-prop asset packing was requested but no packing plan was produced".into(),
        );
    }
    if options.generate_reflections
        && let Some(context) = &context
        && output.bsp.is_some()
        && !output.report.has_errors()
    {
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
    for diagnostic in &output.report.diagnostics {
        let level = match diagnostic.level {
            Level::Info => "info",
            Level::Warning => "warning",
            Level::Error => "error",
        };
        let object = diagnostic.object.as_deref().unwrap_or("map");
        eprintln!("{level}: {object}: {}", diagnostic.message);
    }
    if let Some(progress) = &output.stats.lighting_progress {
        print_lighting_progress(progress);
    }
    let published = if let Some(lease) = &lease {
        if let Some(identity) = &source_identity {
            identity.verify(control)?;
        }
        let prepared = output.prepare_write_reserved(reservation.unwrap(), control)?;
        let pointfile = (output.bsp.is_none() && output.leak.is_some())
            .then(|| OutputLease::acquire(&path.with_extension("lin"), control))
            .transpose()?;
        let assets =
            compiler::asset_sources::capture_output_asset_source_guard(&output, &fs, control)?;
        lease.verify(control)?;
        let validate = || {
            source.verify()?;
            if let Some(identity) = &source_identity {
                identity.verify_metadata()?;
            }
            assets.verify_metadata()?;
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
        let reservation = WriteReservation::for_destinations(destination, pointfile, control)?;
        match output.prepare_write_reserved(reservation, control)? {
            Some(prepared) => {
                source.verify()?;
                prepared.commit(control)?
            }
            None => false,
        }
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
    if let Some(sky) = &stats.sky_assets {
        println!(
            "Packed sky {}: {} · {} asset files · {} asset bytes",
            sky.family, sky.channels, sky.files, sky.asset_bytes
        );
    }
    if let Some(water) = &stats.water_assets {
        println!(
            "Water assets: {} materials · {} · {} asset files · {} asset bytes",
            water.roots, water.channels, water.files, water.asset_bytes
        );
    }
    if let Some(props) = &stats.static_prop_assets {
        println!(
            "Static-prop assets: {} models · {} asset files · {} asset bytes",
            props.models, props.files, props.asset_bytes
        );
    }
    if let Some(props) = &stats.prop_vertex_lighting {
        println!(
            "Static-prop vertex lighting: {} props · {} meshes · {} vertices · {} LDR bytes · {} HDR bytes",
            props.props, props.meshes, props.vertices, props.ldr_bytes, props.hdr_bytes
        );
    }
    println!(
        "Visibility {}: workers {} · {} leaf-cluster pairs removed before authored grouping · {} work units",
        stats.visibility_mode.label(),
        stats.visibility_workers,
        stats.visibility_removed_pairs,
        stats.visibility_work
    );
    println!(
        "Visibility clusters: {} solved · {} emitted · {} helpers · {} groups · {} grouped leaves",
        stats.visibility_solved_clusters,
        stats.clusters,
        stats.viscluster_helpers,
        stats.viscluster_groups,
        stats.viscluster_grouped_leaves
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
    println!(
        "static props: {}, model assets: {}, leaf references: {}",
        stats.static_props, stats.static_prop_models, stats.static_prop_leaf_refs
    );
    println!(
        "detail props: {}, model assets: {}, sprite entries: {} (unbaked lighting)",
        stats.detail_props, stats.detail_prop_models, stats.detail_prop_sprites
    );
    println!(
        "static-prop mesh shadows: {} props, {} models, {} triangles, {} disabled, {} preparation work units; {} alpha-texture triangles, {} materials, {} textures, {} decoded alpha bytes",
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

fn print_lighting_progress(progress: &compiler::lighting::LightingProgress) {
    println!(
        "Lighting attempted work: preparation {} · LDR {} · HDR {} · shared {} / {} · {} sampling · {:?}/{:?} · {}",
        progress.preparation_work,
        progress.ldr_work,
        progress.hdr_work,
        progress.work,
        progress.limit,
        progress.sky_quality,
        progress.current_channel,
        progress.current_phase,
        if progress.completed {
            "complete"
        } else {
            "incomplete"
        },
    );
    println!(
        "Lightmap samples: {} completed / {} planned{} · LDR {} / {} · HDR {} / {}",
        progress.lightmap_samples_completed,
        progress.lightmap_samples_planned,
        if progress.lightmap_samples_planned_complete {
            ""
        } else {
            " so far"
        },
        progress.ldr_lightmap_samples_completed,
        progress.ldr_lightmap_samples_planned,
        progress.hdr_lightmap_samples_completed,
        progress.hdr_lightmap_samples_planned,
    );
    for phase in progress.phases.iter().filter(|phase| phase.work != 0) {
        println!(
            "Lighting {:?}/{:?}: {} work · {} / {} samples{}",
            phase.channel,
            phase.phase,
            phase.work,
            phase.samples_completed,
            phase.samples_planned,
            if phase.samples_planned_complete {
                ""
            } else {
                " planned so far"
            },
        );
    }
    let mut contributors = progress.contributors.iter().collect::<Vec<_>>();
    contributors.sort_by_key(|entry| std::cmp::Reverse(entry.work));
    let omitted_work = contributors
        .iter()
        .skip(MAX_PRINTED_LIGHTING_CONTRIBUTORS)
        .map(|entry| entry.work)
        .sum::<u64>();
    if contributors.len() > MAX_PRINTED_LIGHTING_CONTRIBUTORS {
        println!(
            "Lighting contributors: largest {} of {} retained entries; {} work in other retained entries",
            MAX_PRINTED_LIGHTING_CONTRIBUTORS,
            contributors.len(),
            omitted_work,
        );
    }
    for entry in contributors
        .into_iter()
        .take(MAX_PRINTED_LIGHTING_CONTRIBUTORS)
    {
        println!(
            "Lighting contributor entity:{} {} {:?}/{:?}: {} work",
            entry.entity_id, entry.classname, entry.channel, entry.phase, entry.work,
        );
        if let Some(instance) = &entry.instance {
            println!(
                "  instance {}{} · child entity:{} · compiled entity:{}",
                instance.file,
                if instance.file_truncated { "…" } else { "" },
                instance.original_entity_id,
                entry.compiled_entity_id,
            );
        }
    }
    if progress.contributors_truncated {
        println!(
            "Lighting contributor detail truncated; {} work from other contributors",
            progress.other_contributors_work
        );
    }
    println!(
        "Lighting work without a light contributor: {}",
        progress.unattributed_work
    );
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
            libc::signal(libc::SIGINT, interrupt as *const () as libc::sighandler_t);
            libc::signal(libc::SIGTERM, interrupt as *const () as libc::sighandler_t);
        }
        let finished = AtomicBool::new(false);
        struct Finish<'a>(&'a AtomicBool);
        impl Drop for Finish<'_> {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Relaxed);
            }
        }
        std::thread::scope(|scope| {
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
        })
    }
    #[cfg(not(all(target_os = "linux", target_pointer_width = "64")))]
    run(control)
}
