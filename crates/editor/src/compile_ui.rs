use std::cell::Cell;
use std::rc::Rc;

use slint::ComponentHandle;

use crate::CompileWindow;
use crate::mcp::{SharedSession, lock};
use crate::session::{Actor, Session};

pub struct Controller {
    pub window: CompileWindow,
    token: Rc<Cell<Option<u64>>>,
}

impl Controller {
    pub fn new(shared: &SharedSession) -> Result<Self, slint::PlatformError> {
        let window = CompileWindow::new()?;
        let token = Rc::new(Cell::new(None));
        let settings = lock(shared).compile_settings();
        let labels = settings["choices"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|choice| slint::SharedString::from(choice["label"].as_str().unwrap_or_default()))
            .collect::<Vec<_>>();
        window.set_lighting_modes(Rc::new(slint::VecModel::from(labels)).into());
        let physics_labels = settings["physics_choices"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|choice| slint::SharedString::from(choice["label"].as_str().unwrap_or_default()))
            .collect::<Vec<_>>();
        window.set_physics_modes(Rc::new(slint::VecModel::from(physics_labels)).into());
        let sky_labels = settings["sky_choices"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|choice| slint::SharedString::from(choice["label"].as_str().unwrap_or_default()))
            .collect::<Vec<_>>();
        window.set_sky_qualities(Rc::new(slint::VecModel::from(sky_labels)).into());
        let visibility_labels = settings["visibility_choices"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|choice| slint::SharedString::from(choice["label"].as_str().unwrap_or_default()))
            .collect::<Vec<_>>();
        window.set_visibility_modes(Rc::new(slint::VecModel::from(visibility_labels)).into());
        let bounce_labels = settings["bounce_choices"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|choice| slint::SharedString::from(choice["label"].as_str().unwrap_or_default()))
            .collect::<Vec<_>>();
        window.set_bounce_modes(Rc::new(slint::VecModel::from(bounce_labels)).into());
        window.on_change_smoothing_angle({
            let (shared, window) = (shared.clone(), window.as_weak());
            move |angle| {
                let result = u8::try_from(angle)
                    .map_err(|_| "Choose an angle from 0 to 90 degrees".to_string())
                    .and_then(|angle| {
                        lock(&shared).update_build_settings(
                            Actor::User,
                            crate::session::compilation::BuildSettingsUpdate {
                                smoothing_angle: Some(angle),
                                ..Default::default()
                            },
                        )
                    });
                window.unwrap().set_message(match result {
                    Ok(_) => {
                        "Brush smoothing angle updated. The next build will use this angle.".into()
                    }
                    Err(error) => error.into(),
                });
            }
        });
        window.on_change_bounces({
            let (shared, window) = (shared.clone(), window.as_weak());
            move |choice| {
                let mut session = lock(&shared);
                let settings = session.compile_settings();
                let result = settings["bounce_choices"][choice as usize]["bounces"]
                    .as_u64()
                    .and_then(|count| u8::try_from(count).ok())
                    .ok_or_else(|| "Choose a supported indirect lighting mode".to_string())
                    .and_then(|count| {
                        session.update_compile_options(
                            Actor::User,
                            None,
                            None,
                            None,
                            None,
                            Some(count),
                        )
                    });
                window.unwrap().set_message(match result {
                    Ok(_) => "Indirect light updated. The next build will use this setting.".into(),
                    Err(error) => error.into(),
                });
            }
        });
        window.on_change_visibility({
            let (shared, window) = (shared.clone(), window.as_weak());
            move |choice| {
                let mut session = lock(&shared);
                let settings = session.compile_settings();
                let result = settings["visibility_choices"][choice as usize]["name"]
                    .as_str()
                    .ok_or_else(|| "Choose a supported visibility quality".to_string())
                    .and_then(|mode| {
                        session.update_compile_options(
                            Actor::User,
                            None,
                            None,
                            None,
                            Some(mode),
                            None,
                        )
                    });
                window.unwrap().set_message(match result {
                    Ok(_) => {
                        "Visibility quality updated. The next build will use this quality.".into()
                    }
                    Err(error) => error.into(),
                });
            }
        });
        window.on_change_sky_quality({
            let (shared, window) = (shared.clone(), window.as_weak());
            move |choice| {
                let mut session = lock(&shared);
                let settings = session.compile_settings();
                let result = settings["sky_choices"][choice as usize]["name"]
                    .as_str()
                    .ok_or_else(|| "Choose a supported sky quality".to_string())
                    .and_then(|quality| {
                        session.update_compile_settings_quality(
                            Actor::User,
                            None,
                            None,
                            Some(quality),
                        )
                    });
                window.unwrap().set_message(match result {
                    Ok(_) => "Sky quality updated. The next build will use this quality.".into(),
                    Err(error) => error.into(),
                });
            }
        });
        window.on_change_physics({
            let (shared, window) = (shared.clone(), window.as_weak());
            move |choice| {
                let mut session = lock(&shared);
                let settings = session.compile_settings();
                let result = settings["physics_choices"][choice as usize]["name"]
                    .as_str()
                    .ok_or_else(|| "Choose a supported physics mode".to_string())
                    .and_then(|mode| {
                        session.update_compile_settings(Actor::User, None, Some(mode))
                    });
                window.unwrap().set_message(match result {
                    Ok(_) => "Physics mode updated. The next build will use this mode.".into(),
                    Err(error) => error.into(),
                });
            }
        });
        window.on_change_lighting({
            let (shared, window) = (shared.clone(), window.as_weak());
            move |choice| {
                let mut session = lock(&shared);
                let settings = session.compile_settings();
                let result = settings["choices"][choice as usize]["name"]
                    .as_str()
                    .ok_or_else(|| "Choose a supported lighting mode".to_string())
                    .and_then(|mode| session.set_compile_settings(Actor::User, mode));
                window.unwrap().set_message(match result {
                    Ok(_) => "Build lighting updated. The next build will use this mode.".into(),
                    Err(error) => error.into(),
                });
            }
        });
        window.on_play_build({
            let (shared, window) = (shared.clone(), window.as_weak());
            move || {
                let result = crate::mcp::start_launch(&shared, Actor::User);
                window.unwrap().set_message(match result {
                    Ok(()) => "Launching the last successful build with -insecure. Open Game Log for engine output.".into(),
                    Err(error) => error.into(),
                });
            }
        });
        window.on_cancel_build({
            let (shared, window, token) = (shared.clone(), window.as_weak(), token.clone());
            move || {
                if let Some(token) = token.get() {
                    let result = lock(&shared).cancel_compile(Actor::User, token);
                    window.unwrap().set_message(match result {
                        Ok(()) => {
                            "Cancellation requested. Waiting for the compiler to stop.".into()
                        }
                        Err(error) => error.into(),
                    });
                }
            }
        });
        window.on_close_dialog({
            let window = window.as_weak();
            move || {
                let _ = window.unwrap().hide();
            }
        });
        Ok(Self { window, token })
    }

    pub fn refresh(&self, session: &Session) {
        self.token.set(session.compile_token());
        self.window.set_cancellable(session.compile_cancellable());
        let status = session.compile_status();
        let settings = &status["settings"];
        let choices = settings["choices"].as_array().cloned().unwrap_or_default();
        let choice = choices
            .iter()
            .position(|choice| choice["name"] == settings["lighting"])
            .unwrap_or_default() as i32;
        self.window.set_lighting_choice(choice);
        self.window.set_selected_lighting(
            choices[choice as usize]["label"]
                .as_str()
                .unwrap_or_default()
                .into(),
        );
        self.window
            .set_lighting_enabled(settings["editable"].as_bool().unwrap_or(false));
        self.window.set_lighting_description(
            settings["choices"][choice as usize]["description"]
                .as_str()
                .unwrap_or_default()
                .into(),
        );
        let physics_choices = settings["physics_choices"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let physics_choice = physics_choices
            .iter()
            .position(|choice| choice["name"] == settings["physics"])
            .unwrap_or_default();
        self.window.set_selected_physics(
            physics_choices[physics_choice]["label"]
                .as_str()
                .unwrap_or_default()
                .into(),
        );
        self.window.set_physics_description(
            physics_choices[physics_choice]["description"]
                .as_str()
                .unwrap_or_default()
                .into(),
        );
        let sky_choice = settings["sky_choices"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|choice| choice["name"] == settings["sky_quality"]);
        self.window.set_selected_sky_quality(
            sky_choice
                .and_then(|choice| choice["label"].as_str())
                .unwrap_or_default()
                .into(),
        );
        self.window.set_sky_enabled(
            settings["editable"].as_bool().unwrap_or(false) && settings["lighting"] == "direct_ldr",
        );
        self.window
            .set_smoothing_angle(settings["smoothing_angle"].as_u64().unwrap_or_default() as i32);
        let bounce_choice = settings["bounce_choices"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|choice| choice["bounces"] == settings["bounces"]);
        self.window.set_selected_bounces(
            bounce_choice
                .and_then(|choice| choice["label"].as_str())
                .unwrap_or_default()
                .into(),
        );
        let visibility_choice = settings["visibility_choices"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|choice| choice["name"] == settings["visibility"]);
        self.window.set_selected_visibility(
            visibility_choice
                .and_then(|choice| choice["label"].as_str())
                .unwrap_or_default()
                .into(),
        );
        self.window.set_visibility_description(
            visibility_choice
                .and_then(|choice| choice["description"].as_str())
                .unwrap_or_default()
                .into(),
        );
        let launch = &status["launch"];
        self.window
            .set_launch_ready(launch["ready"].as_bool().unwrap_or(false));
        self.window.set_launch_details(
            match launch["bsp"].as_str() {
                Some(path) => format!("Ready to play without rebuilding: {path}"),
                None => format!(
                    "Play unavailable: {}",
                    launch["reason"].as_str().unwrap_or_default()
                ),
            }
            .into(),
        );
        let job = &status["job"];
        if job.is_null() {
            self.window
                .set_summary("No build has run in this session.".into());
            self.window.set_details(
                "Choose settings above, then Compile or Run Map from the toolbar.".into(),
            );
            self.window.set_diagnostics("".into());
            return;
        }
        self.window.set_summary(session.compile_label().into());
        let result = &job["result"];
        let samples = result["stats"]["cubemap_samples"]
            .as_u64()
            .map(|count| format!("\nCubemap samples: {count}"))
            .unwrap_or_default();
        let props = result["stats"]["static_props"]
            .as_u64()
            .filter(|&count| count > 0)
            .map(|count| {
                let models = result["stats"]["static_prop_models"]
                    .as_u64()
                    .unwrap_or_default();
                let leaves = result["stats"]["static_prop_leaf_refs"]
                    .as_u64()
                    .unwrap_or_default();
                format!(
                    "\nStatic props: {count} · Model assets: {models} · Leaf references: {leaves}"
                )
            })
            .unwrap_or_default();
        let visibility = result["stats"]["clusters"]
            .as_u64()
            .map(|count| {
                let portals = result["stats"]["visibility_portals"]
                    .as_u64()
                    .unwrap_or_default();
                let cluster_label = if count == 1 { "cluster" } else { "clusters" };
                let portal_label = if portals == 1 { "portal" } else { "portals" };
                let removed = result["stats"]["visibility_removed_pairs"].as_u64().unwrap_or_default();
                let fallbacks = result["stats"]["visibility_fallbacks"].as_u64().unwrap_or_default();
                let workers = result["stats"]["visibility_workers"].as_u64().unwrap_or(1);
                format!("\nVisibility: {count} {cluster_label} · {portals} {portal_label} · Workers: {workers} · {removed} cluster pairs removed · {fallbacks} conservative fallbacks")
            })
            .unwrap_or_default();
        let bumps = result["stats"]["bump_faces"]
            .as_u64()
            .filter(|&count| count > 0)
            .map(|count| {
                format!(
                    "\nNormal-map lighting: {count} faces · {} directional maps",
                    result["stats"]["bump_lightmaps"]
                        .as_u64()
                        .unwrap_or_default()
                )
            })
            .unwrap_or_default();
        let model_maps = result["stats"]["lit_brush_models"]
            .as_u64()
            .filter(|&count| count > 0)
            .map(|count| {
                format!(
                    "\nBrush entity lightmaps: {count} models · {} faces",
                    result["stats"]["model_lighting_faces"]
                        .as_u64()
                        .unwrap_or_default()
                )
            })
            .unwrap_or_default();
        let styles = result["stats"]["light_styles"]
            .as_u64()
            .filter(|&count| count > 0)
            .map(|count| {
                format!(
                    "\nLight styles: {count} · {} styled faces · {} lightmaps · {} switched groups",
                    result["stats"]["styled_faces"].as_u64().unwrap_or_default(),
                    result["stats"]["light_style_maps"]
                        .as_u64()
                        .unwrap_or_default(),
                    result["stats"]["switchable_light_groups"]
                        .as_u64()
                        .unwrap_or_default()
                )
            })
            .unwrap_or_default();
        let smoothing = result["stats"]["smoothed_faces"]
            .as_u64()
            .filter(|&count| count > 0)
            .map(|count| {
                format!(
                    "\nBrush smoothing: {count} faces · {} corners",
                    result["stats"]["smoothed_corners"]
                        .as_u64()
                        .unwrap_or_default()
                )
            })
            .unwrap_or_default();
        let model_bounce = result["stats"]["model_bounce_faces"]
            .as_u64()
            .filter(|&count| count > 0)
            .map(|count| {
                format!(
                    "\nBrush entity bounce: {count} faces · {} samples lit",
                    result["stats"]["model_bounce_samples"]
                        .as_u64()
                        .unwrap_or_default()
                )
            })
            .unwrap_or_default();
        let bounce = result["stats"]["bounces"]
            .as_u64()
            .filter(|&count| count > 0)
            .map(|count| {
                format!(
                    "\nIndirect light: {} · {} rays · {} samples lit{model_bounce}",
                    compiler::bounce_label(count as u8),
                    result["stats"]["bounce_rays"].as_u64().unwrap_or_default(),
                    result["stats"]["bounce_samples"]
                        .as_u64()
                        .unwrap_or_default()
                )
            })
            .unwrap_or_default();
        let hints = result["stats"]["hint_surfaces"]
            .as_u64()
            .filter(|&count| count > 0)
            .map(|count| {
                let splits = result["stats"]["hint_splits"].as_u64().unwrap_or_default();
                format!(
                    "\nHint surfaces: {count} · Added splits: {splits} · Initial priority: {}",
                    result["stats"]["hint_priority_splits"]
                        .as_u64()
                        .unwrap_or_default()
                )
            })
            .unwrap_or_default();
        let terrain = result["stats"]["displacements"]
            .as_u64()
            .filter(|&count| count > 0)
            .map(|count| {
                let vertices = result["stats"]["displacement_vertices"]
                    .as_u64()
                    .unwrap_or_default();
                let triangles = result["stats"]["displacement_triangles"]
                    .as_u64()
                    .unwrap_or_default();
                let neighbors = result["stats"]["displacement_neighbor_pairs"].as_u64().unwrap_or_default();
                let corners=result["stats"]["displacement_corner_neighbor_pairs"].as_u64().unwrap_or_default();
                let mixed=result["stats"]["displacement_mixed_power_pairs"].as_u64().unwrap_or_default();
                let halves=result["stats"]["displacement_half_edge_pairs"].as_u64().unwrap_or_default();
                let disabled=result["stats"]["displacement_lod_disabled_vertices"].as_u64().unwrap_or_default();
                let snapped=result["stats"]["displacement_lod_snapped_vertices"].as_u64().unwrap_or_default();
                let smoothing=result["stats"]["displacement_smoothing_overrides"].as_u64().unwrap_or_default();
                format!(
                    "\nTerrain surfaces: {count} · Vertices: {vertices} · Triangles: {triangles} · Neighbor pairs: {neighbors} · Corner pairs: {corners}\nMixed-power pairs: {mixed} · Half-edge pairs: {halves} · LOD disabled vertices: {disabled} · Snapped vertices: {snapped}\nBorder smoothing: {smoothing} authored surfaces"
                )
            })
            .unwrap_or_default();
        let areas = result["stats"]["areaportals"]
            .as_u64()
            .filter(|&count| count > 0)
            .map(|count| {
                let areas = result["stats"]["areas"].as_u64().unwrap_or_default();
                let windows = result["stats"]["areaportal_windows"].as_u64().unwrap_or_default();
                let vertices = result["stats"]["areaportal_vertices"]
                    .as_u64()
                    .unwrap_or_default();
                format!(
                    "\nVisibility areas: {areas} · Areaportals: {count} · Window portals: {windows} · Clip vertices: {vertices}"
                )
            })
            .unwrap_or_default();
        let overlays = result["stats"]["overlays"]
            .as_u64()
            .filter(|&count| count > 0)
            .map(|count| {
                let faces = result["stats"]["overlay_face_refs"]
                    .as_u64()
                    .unwrap_or_default();
                format!("\nOverlays: {count} · Surface references: {faces}")
            })
            .unwrap_or_default();
        let water = result["stats"]["water_regions"]
            .as_u64()
            .filter(|&count| count > 0)
            .map(|count| {
                let leaves = result["stats"]["water_leaves"].as_u64().unwrap_or_default();
                let region_label = if count == 1 { "region" } else { "regions" };
                let leaf_label = if leaves == 1 { "leaf" } else { "leaves" };
                format!("\nWater: {count} {region_label} · {leaves} submerged {leaf_label}")
            })
            .unwrap_or_default();
        let sky = result["stats"]["sky_clusters"]
            .as_u64()
            .filter(|&count| count > 0)
            .map(|count| {
                let label = if count == 1 { "cluster" } else { "clusters" };
                format!("\nSky visible from: {count} {label}")
            })
            .unwrap_or_default();
        let lighting = result["stats"]["lighting_faces"]
            .as_u64()
            .filter(|&count| count > 0)
            .map(|count| {
                let samples = result["stats"]["lightmap_samples"]
                    .as_u64()
                    .unwrap_or_default();
                let lights = result["stats"]["world_lights"].as_u64().unwrap_or_default();
                let light_label = if lights == 1 {
                    "light source"
                } else {
                    "light sources"
                };
                format!(
                    "\nBaked lighting: {count} faces · {samples} samples · {lights} {light_label}\nLight culling: {} evaluated · {} hidden · {} beyond range\nModel ambient: {} probes · {} bytes",
                    result["stats"]["light_evaluations"].as_u64().unwrap_or_default(),
                    result["stats"]["light_pvs_skipped"].as_u64().unwrap_or_default(),
                    result["stats"]["light_range_skipped"].as_u64().unwrap_or_default(),
                    result["stats"]["ambient_samples"].as_u64().unwrap_or_default(),
                    result["stats"]["ambient_bytes"].as_u64().unwrap_or_default(),
                )
            })
            .unwrap_or_default();
        let physics = result["stats"]["physics_solids"]
            .as_u64()
            .filter(|&count| count > 0)
            .map(|count| {
                let convexes = result["stats"]["physics_convexes"]
                    .as_u64()
                    .unwrap_or_default();
                let label = if count == 1 { "body" } else { "bodies" };
                let fluids = result["stats"]["physics_fluids"]
                    .as_u64()
                    .unwrap_or_default();
                let mesh = result["stats"]["physics_mesh_triangles"]
                    .as_u64()
                    .filter(|&count| count > 0)
                    .map(|count| format!(" · Terrain triangles: {count}"))
                    .unwrap_or_default();
                let fluid_label = if fluids == 1 {
                    "fluid region"
                } else {
                    "fluid regions"
                };
                format!(
                    "\nPhysics: {count} {label} · {convexes} hulls · {fluids} {fluid_label}{mesh} · Surface properties: {}",
                    result["stats"]["physics_surface_properties"].as_u64().unwrap_or_default()
                )
            })
            .unwrap_or_default();
        self.window.set_details(
            format!(
                "Output: {}\nBuild settings: {} · {}\nVisibility: {} · Sampling: {}\nIndirect light: {} · Brush smoothing angle: {}°\nStage: {} · {:.2}s elapsed{samples}{props}{visibility}{hints}{bounce}{areas}{terrain}{overlays}{water}{sky}{lighting}{bumps}{model_maps}{styles}{smoothing}{physics}\n{}",
                job["path"].as_str().unwrap_or_default(),
                compiler::LightingMode::parse(job["settings"]["lighting"].as_str().unwrap_or_default()).unwrap_or_default().label(),
                compiler::PhysicsMode::parse(job["settings"]["physics"].as_str().unwrap_or_default()).unwrap_or_default().label(),
                compiler::VisibilityMode::parse(job["settings"]["visibility"].as_str().unwrap_or_default()).unwrap_or_default().label(),
                compiler::SkyQuality::parse(job["settings"]["sky_quality"].as_str().unwrap_or_default()).unwrap_or_default().label(),
                compiler::bounce_label(job["settings"]["bounces"].as_u64().unwrap_or_default() as u8),
                job["settings"]["smoothing_angle"].as_u64().unwrap_or_default(),
                job["stage_label"].as_str().unwrap_or_default(),
                job["elapsed_ms"].as_u64().unwrap_or_default() as f64 / 1000.0,
                if job["stale"].as_bool().unwrap_or(false) {
                    "The map or build settings changed. Compile again before playing."
                } else {
                    "This build uses the current map snapshot. Unsaved VMF edits are included."
                }
            )
            .into(),
        );
        let mut messages = Vec::new();
        if let Some(error) = result["error"].as_str() {
            messages.push(error.to_owned());
        }
        if let Some(diagnostics) = result["diagnostics"].as_array() {
            for diagnostic in diagnostics {
                messages.push(format!(
                    "{}{}: {}",
                    diagnostic["level"].as_str().unwrap_or("info"),
                    diagnostic["object"]
                        .as_str()
                        .map(|object| format!(" · {object}"))
                        .unwrap_or_default(),
                    diagnostic["message"].as_str().unwrap_or_default()
                ));
            }
            let count = result["diagnostic_count"].as_u64().unwrap_or_default() as usize;
            if count > diagnostics.len() {
                messages.push(format!(
                    "Showing the first {} of {count} diagnostics.",
                    diagnostics.len()
                ));
            }
        }
        if messages.is_empty() {
            messages.push(
                if result.is_null() {
                    "Diagnostics appear when the build finishes."
                } else {
                    "No diagnostics."
                }
                .into(),
            );
        }
        self.window.set_diagnostics(messages.join("\n\n").into());
    }

    pub fn show(&self, session: &Session) -> Result<(), slint::PlatformError> {
        self.window.set_message("Closing this window leaves an active build running. Cancel build stops it and preserves previous output.".into());
        self.refresh(session);
        self.window.show()
    }
}
