use super::*;
use crate::session::SelectMode;
use glam::{DVec2, DVec3, Vec3};
use render::{Projection, handles::Mode};

fn fixture() -> Session {
    let mut session = Session::new(Err("no assets needed".into()));
    session
        .create_block(
            Actor::User,
            DVec3::new(-128.0, -128.0, -16.0),
            DVec3::new(128.0, 128.0, 0.0),
            "brick/test",
            false,
        )
        .unwrap();
    session
        .set_transform_tool(
            Actor::User,
            TransformTool {
                mode: Some(Mode::Rotate),
                local: true,
                pivot: Some(DVec3::new(1.25, 2.5, 3.75)),
                texture_lock: false,
                texture_scale_lock: Some(true),
                angle_step: 22.5,
            },
        )
        .unwrap();
    session
}

fn begin(session: &mut Session, actor: Actor, camera: Camera) -> u64 {
    session
        .begin_transform_pivot_pick(
            actor,
            camera,
            (511, 511),
            &session.transform_pivot_pick_stamp(),
        )
        .unwrap()
        .picking
        .unwrap()
        .token
}

fn ray() -> Ray {
    Ray {
        origin: DVec3::new(3.123456789012, -5.987654321012, 96.0),
        direction: -DVec3::Z * 7.0,
    }
}

#[test]
fn grid_pivot_rounds_fractional_world_coordinates_and_freezes_selection_center() {
    for actor in [Actor::User, Actor::Agent] {
        for mode in ["selection", "world", "custom"] {
            let mut session = fixture();
            session
                .create_block(
                    actor,
                    DVec3::new(-8.0, -4.0, -2.0),
                    DVec3::new(8.25, 3.75, 2.75),
                    "brick/test",
                    false,
                )
                .unwrap();
            session
                .set_spatial(
                    actor,
                    crate::spatial::Update {
                        grid_size: Some(0.25),
                        snap: Some(false),
                        ..Default::default()
                    },
                )
                .unwrap();
            let before = DVec3::new(0.125, -0.125, 0.375);
            session
                .set_transform_tool(
                    actor,
                    TransformTool {
                        pivot: match mode {
                            "selection" => None,
                            "world" => Some(DVec3::ZERO),
                            _ => Some(before),
                        },
                        ..session.transform_tool
                    },
                )
                .unwrap();
            if mode != "selection" {
                session.select(&[], SelectMode::Replace).unwrap();
            }
            let tool = session.transform_tool;
            let document = session.document.vmf().clone();
            let history = session.history_snapshot();
            let camera = session.camera;
            let preferences = session.pivot_preferences_stamp();
            let expected = if mode == "world" {
                DVec3::ZERO
            } else {
                DVec3::new(0.25, -0.25, 0.5)
            };
            let inspected = session.transform_pivot_grid_state();
            assert!(inspected.error.is_none(), "{mode}: {inspected:?}");
            assert!(!inspected.changed);
            let report = session
                .snap_transform_pivot_to_grid(actor, &inspected.snapshot)
                .unwrap();
            assert_eq!(
                report.before,
                Some(if mode == "world" { DVec3::ZERO } else { before }.to_array())
            );
            assert_eq!(report.after, Some(expected.to_array()));
            assert_eq!(report.grid, 0.25);
            assert_eq!(report.changed, mode != "world");
            assert_eq!(
                session.transform_tool,
                TransformTool {
                    pivot: Some(expected),
                    ..tool
                }
            );
            assert_eq!(session.document.vmf(), &document);
            assert_eq!(session.history_snapshot(), history);
            assert_eq!(session.camera, camera);
            assert!(!session.spatial.snap);
            assert_eq!(
                session.pivot_preferences_stamp() != preferences,
                report.changed
            );
            let preferences = session.pivot_preferences_stamp();
            assert!(
                !session
                    .snap_transform_pivot_to_grid(actor, &report.snapshot)
                    .unwrap()
                    .changed
            );
            assert_eq!(session.pivot_preferences_stamp(), preferences);
            session.select(&[], SelectMode::Replace).unwrap();
            assert_eq!(session.transform_tool.pivot, Some(expected));
        }
    }
}

#[test]
fn grid_pivot_clears_surface_provenance_without_writing_saved_defaults() {
    let directory = tempfile::tempdir().unwrap();
    let mut session = fixture();
    session.configure_recovery(Ok(directory.path().into()));
    session
        .change_pivot_preferences(Actor::User, "save", &session.pivot_preferences_stamp())
        .unwrap();
    let path = directory.path().join(crate::pivot_preferences::FILE_NAME);
    let saved = std::fs::read(&path).unwrap();
    let camera = session.camera;
    let token = begin(&mut session, Actor::User, camera);
    session
        .pick_transform_pivot_ray(
            Actor::User,
            token,
            Ray {
                origin: DVec3::new(16.0, -16.0, 96.0),
                direction: -DVec3::Z,
            },
        )
        .unwrap();
    assert!(session.transform_pivot_pick_state().last_pick_matches_pivot);
    let preferences = session.pivot_preferences_stamp();
    let inspected = session.transform_pivot_grid_state();
    let report = session
        .snap_transform_pivot_to_grid(Actor::Agent, &inspected.snapshot)
        .unwrap();
    assert!(!report.changed);
    assert_ne!(report.snapshot, inspected.snapshot);
    assert!(session.transform_pivot_pick_state().last_pick.is_none());
    assert_eq!(session.pivot_preferences_stamp(), preferences);
    assert_eq!(std::fs::read(path).unwrap(), saved);
}

#[test]
fn grid_pivot_rejects_stale_context_and_preserves_owned_work() {
    for active in [
        "transform",
        "drag",
        "pick",
        "block",
        "paint",
        "build",
        "launch",
        "closing",
    ] {
        let mut session = fixture();
        let camera = session.camera;
        match active {
            "transform" => {
                session
                    .preview_transform(
                        Actor::User,
                        &session.selected_ids(),
                        map_core::edit::Transform {
                            translation: DVec3::X,
                            ..Default::default()
                        },
                        None,
                    )
                    .unwrap();
            }
            "drag" => session
                .begin_transform_drag(render::handles::Handle::X, None)
                .unwrap(),
            "pick" => {
                begin(&mut session, Actor::User, camera);
            }
            "block" => session
                .preview_block(DVec3::ZERO, DVec3::splat(32.0))
                .unwrap(),
            "paint" => session
                .set_paint_operation(
                    Actor::User,
                    true,
                    8.0,
                    super::super::paint::Operation::Selection,
                )
                .unwrap(),
            "build" => session.compiling = true,
            "launch" => {
                session.pending_launch = Some(crate::session::LaunchOrigin::new(
                    session.document_generation,
                ))
            }
            "closing" => session.closing = true,
            _ => unreachable!(),
        }
        let tool = session.transform_tool;
        let epoch = session.interaction_epoch();
        let inspected = session.transform_pivot_grid_state();
        assert!(inspected.error.is_some(), "{active}");
        assert!(
            session
                .snap_transform_pivot_to_grid(Actor::Agent, &inspected.snapshot)
                .is_err(),
            "{active}"
        );
        assert_eq!(session.transform_tool, tool, "{active}");
        assert_eq!(session.interaction_epoch(), epoch, "{active}");
        match active {
            "transform" => assert!(session.transform_preview().is_some()),
            "drag" => assert!(session.transform_drag().is_some()),
            "pick" => assert!(session.transform_pivot_pick_owned(Actor::User)),
            "block" => assert!(session.block_preview().is_some()),
            "paint" => assert!(session.paint_enabled()),
            _ => {}
        }
    }
    for change in [
        "grid",
        "pivot",
        "selection",
        "document",
        "definitions",
        "models",
    ] {
        let mut session = fixture();
        let snapshot = session.transform_pivot_grid_stamp();
        match change {
            "grid" => {
                session
                    .set_spatial(
                        Actor::User,
                        crate::spatial::Update {
                            grid_size: Some(0.25),
                            ..Default::default()
                        },
                    )
                    .unwrap();
            }
            "pivot" => {
                session
                    .set_transform_tool(
                        Actor::User,
                        TransformTool {
                            pivot: Some(DVec3::ZERO),
                            ..session.transform_tool
                        },
                    )
                    .unwrap();
            }
            "selection" => {
                session.select(&[], SelectMode::Replace).unwrap();
            }
            "document" => session.new_map(Actor::User, true).unwrap(),
            "definitions" => session.set_definitions(
                Actor::User,
                source_formats::fgd::Catalog::parse("@PointClass = info_target []").unwrap(),
            ),
            "models" => {
                session.refresh_model_assets(Actor::User);
            }
            _ => unreachable!(),
        }
        let state = session.transform_pivot_grid_state();
        assert!(
            session
                .snap_transform_pivot_to_grid(Actor::Agent, &snapshot)
                .is_err(),
            "{change}"
        );
        assert_eq!(session.transform_pivot_grid_state(), state, "{change}");
    }
}

#[test]
fn grid_pivot_checks_every_selection_member_and_bounded_rounding() {
    let mut session = fixture();
    let solid = session.selected_ids()[0];
    let invalid = session
        .edit(Actor::User, "missing origin fixture", |tx| {
            map_core::edit::create_entity(tx, "info_target", None, &[], Vec::new())
        })
        .unwrap();
    session
        .select(&[solid, invalid], SelectMode::Replace)
        .unwrap();
    session
        .set_transform_tool(
            Actor::User,
            TransformTool {
                pivot: None,
                ..session.transform_tool
            },
        )
        .unwrap();
    for selection in [&[solid, invalid][..], &[]] {
        session.select(selection, SelectMode::Replace).unwrap();
        let report = session.transform_pivot_grid_state();
        assert!(report.error.is_some());
        assert!(
            session
                .snap_transform_pivot_to_grid(Actor::User, &report.snapshot)
                .is_err()
        );
        assert_eq!(session.transform_tool.pivot, None);
    }
    session
        .set_spatial(
            Actor::User,
            crate::spatial::Update {
                grid_size: Some(6.0),
                ..Default::default()
            },
        )
        .unwrap();
    for point in [
        DVec3::NAN,
        DVec3::INFINITY,
        DVec3::splat(65_537.0),
        DVec3::splat(65_536.0),
    ] {
        session.transform_tool.pivot = Some(point);
        let report = session.transform_pivot_grid_state();
        assert!(report.error.is_some(), "{point}");
        assert!(
            session
                .snap_transform_pivot_to_grid(Actor::User, &report.snapshot)
                .is_err()
        );
    }
    session
        .set_spatial(
            Actor::User,
            crate::spatial::Update {
                grid_size: Some(0.25),
                ..Default::default()
            },
        )
        .unwrap();
    for (point, expected) in [
        (
            DVec3::new(0.125 - f64::EPSILON, -0.125 + f64::EPSILON, 0.375),
            DVec3::new(0.0, 0.0, 0.5),
        ),
        (
            DVec3::new(0.125 + f64::EPSILON, -0.125 - f64::EPSILON, -0.375),
            DVec3::new(0.25, -0.25, -0.5),
        ),
    ] {
        session.transform_tool.pivot = Some(point);
        let report = session
            .snap_transform_pivot_to_grid(Actor::User, &session.transform_pivot_grid_stamp())
            .unwrap();
        assert_eq!(report.after, Some(expected.to_array()));
    }
}

#[test]
fn exact_surface_pivot_changes_only_shared_settings_and_preserves_framing_inputs() {
    for actor in [Actor::User, Actor::Agent] {
        let mut session = fixture();
        let source = session.document.vmf().clone();
        let history: Vec<_> = session.document.undo_labels().map(str::to_owned).collect();
        let selection = session.selected_ids();
        let bounds = session.selection_bounds();
        let camera = session.camera;
        let tool = session.transform_tool;
        let token = begin(&mut session, actor, camera);
        assert_eq!(session.transform_tool, tool);
        let state = session
            .pick_transform_pivot_ray(actor, token, ray())
            .unwrap();
        let expected = DVec3::new(ray().origin.x, ray().origin.y, 0.0);
        assert_eq!(
            session.transform_tool,
            TransformTool {
                pivot: Some(expected),
                ..tool
            }
        );
        let picked = state.last_pick.unwrap();
        assert_eq!(picked.target, crate::identity::id_string(selection[0]));
        assert!(picked.solid.is_some() && picked.side.is_some());
        assert_eq!(picked.point, expected.to_array());
        assert_eq!(picked.normal, DVec3::Z.to_array());
        assert!(
            state.picking.is_none()
                && state.last_pick_matches_pivot
                && state.last_pick_document_current
        );
        assert!(session.cancel_transform_pivot_pick(actor, token).is_err());
        assert_eq!(
            session.transform_guide().unwrap().origin,
            expected.as_vec3()
        );
        assert_eq!(session.document.vmf(), &source);
        assert_eq!(session.document.undo_labels().collect::<Vec<_>>(), history);
        assert_eq!(session.selected_ids(), selection);
        assert_eq!(session.selection_bounds(), bounds);
        assert_eq!(session.camera, camera);
        assert_eq!(
            session.selection_pivot(&selection).unwrap(),
            DVec3::new(0.0, 0.0, -8.0)
        );
    }
}

#[test]
fn misses_bad_rays_and_foreign_or_replaced_tokens_leave_pivot_and_pick_unchanged() {
    let mut session = fixture();
    let camera = session.camera;
    let token = begin(&mut session, Actor::User, camera);
    let state = session.transform_pivot_pick_state();
    for ray in [
        Ray {
            origin: DVec3::splat(1000.0),
            direction: DVec3::Z,
        },
        Ray {
            origin: DVec3::NAN,
            direction: -DVec3::Z,
        },
        Ray {
            origin: DVec3::Z,
            direction: DVec3::ZERO,
        },
    ] {
        assert!(
            session
                .pick_transform_pivot_ray(Actor::User, token, ray)
                .is_err()
        );
        assert_eq!(session.transform_pivot_pick_state(), state);
    }
    assert!(
        session
            .pick_transform_pivot_ray(Actor::Agent, token, ray())
            .is_err()
    );
    assert!(
        session
            .cancel_transform_pivot_pick(Actor::Agent, token)
            .is_err()
    );
    assert!(!session.transform_pivot_pick_interactive(Actor::Agent, camera, (511, 511)));
    assert_eq!(session.transform_pivot_pick_state(), state);
    session
        .cancel_transform_pivot_pick(Actor::User, token)
        .unwrap();
    let replacement = begin(&mut session, Actor::User, camera);
    assert_ne!(replacement, token);
    assert!(
        session
            .pick_transform_pivot_ray(Actor::User, token, ray())
            .is_err()
    );
    assert_eq!(
        session.transform_pivot_pick_state().current_pivot,
        state.current_pivot
    );
    session
        .cancel_transform_pivot_pick(Actor::User, replacement)
        .unwrap();
    assert_eq!(
        session.transform_tool.pivot,
        Some(DVec3::new(1.25, 2.5, 3.75))
    );
}

#[test]
fn captured_pixels_hit_the_exact_surface_in_all_projections_without_moving_camera() {
    for projection in [
        Projection::Perspective,
        Projection::Top,
        Projection::Front,
        Projection::Side,
    ] {
        let mut session = fixture();
        let camera = if projection == Projection::Perspective {
            Camera::looking_at(Vec3::new(3.125, -5.25, 96.0), Vec3::new(3.125, -5.25, 0.0))
        } else {
            Camera::planar(projection, Vec3::new(3.125, -5.25, -8.0), 300.0)
        };
        let original = session.camera;
        let token = begin(&mut session, Actor::Agent, camera);
        let pointer = Pointer {
            position: DVec2::splat(255.5),
            size: (511, 511),
        };
        assert!(!session.transform_pivot_pick_interactive(Actor::Agent, camera, (512, 511)));
        assert!(
            session
                .pick_transform_pivot_pixel(
                    Actor::Agent,
                    token,
                    Pointer {
                        size: (512, 511),
                        ..pointer
                    }
                )
                .is_err()
        );
        assert!(
            session
                .pick_transform_pivot_pixel(
                    Actor::Agent,
                    token,
                    Pointer {
                        position: DVec2::new(-1.0, 5.0),
                        ..pointer
                    }
                )
                .is_err()
        );
        let (origin, direction) = camera.ray(0.0, 0.0, 1.0);
        let expected = map_core::query::pick_surface(
            session.document(),
            &Ray {
                origin: origin.as_dvec3(),
                direction: direction.as_dvec3(),
            },
            |_| true,
        )
        .unwrap();
        let state = session
            .pick_transform_pivot_pixel(Actor::Agent, token, pointer)
            .unwrap();
        assert_eq!(state.current_pivot, Some(expected.hit.point.to_array()));
        assert_eq!(session.camera, original);
    }
}

#[test]
fn stale_source_selection_settings_definitions_camera_and_view_stop_input_but_allow_cancel() {
    for change in [
        "document",
        "source",
        "selection",
        "tool",
        "definitions",
        "camera",
        "view",
        "scene",
    ] {
        let mut session = fixture();
        let camera = session.camera;
        let token = begin(&mut session, Actor::User, camera);
        match change {
            "document" => session.new_map(Actor::Agent, true).unwrap(),
            "source" => {
                let id = session.selected_ids()[0];
                session.set_visibility(Actor::Agent, "hide", &[id]).unwrap();
            }
            "selection" => session.select(&[], SelectMode::Replace).unwrap(),
            "tool" => session
                .set_transform_tool(
                    Actor::Agent,
                    TransformTool {
                        pivot: Some(DVec3::splat(7.25)),
                        ..session.transform_tool
                    },
                )
                .unwrap(),
            "definitions" => session.set_definitions(
                Actor::Agent,
                source_formats::fgd::Catalog::parse("@PointClass = info_target []").unwrap(),
            ),
            "camera" => session.set_camera(camera),
            "view" => session.set_view(render::ViewOptions {
                tool_textures: !session.view().tool_textures,
                ..session.view()
            }),
            "scene" => session.scene_serial += 1,
            _ => unreachable!(),
        }
        let tool = session.transform_tool;
        let source = session.document.vmf().clone();
        assert!(
            !session.transform_pivot_pick_interactive(Actor::User, camera, (511, 511)),
            "{change}"
        );
        assert!(
            !session.transform_pivot_pick_state().picking.unwrap().valid,
            "{change}"
        );
        assert!(
            session
                .pick_transform_pivot_ray(Actor::User, token, ray())
                .is_err(),
            "{change}"
        );
        session
            .cancel_transform_pivot_pick(Actor::User, token)
            .unwrap();
        assert_eq!(session.transform_tool, tool, "{change}");
        assert_eq!(session.document.vmf(), &source, "{change}");
    }
}

#[test]
fn admission_preserves_active_geometry_and_rejects_stale_or_invalid_capture() {
    let mut session = fixture();
    let camera = session.camera;
    for size in [(0, 511), (511, 8193)] {
        assert!(
            session
                .begin_transform_pivot_pick(
                    Actor::User,
                    camera,
                    size,
                    &session.transform_pivot_pick_stamp()
                )
                .is_err()
        );
    }
    assert!(
        session
            .begin_transform_pivot_pick(
                Actor::User,
                Camera {
                    position: Vec3::NAN,
                    ..camera
                },
                (511, 511),
                &session.transform_pivot_pick_stamp()
            )
            .is_err()
    );
    assert!(
        session
            .begin_transform_pivot_pick(Actor::User, camera, (511, 511), "stale")
            .is_err()
    );
    session
        .preview_block(DVec3::ZERO, DVec3::splat(32.0))
        .unwrap();
    let bounds = session.block_preview;
    assert!(
        session
            .begin_transform_pivot_pick(
                Actor::User,
                camera,
                (511, 511),
                &session.transform_pivot_pick_stamp()
            )
            .is_err()
    );
    assert_eq!(session.block_preview, bounds);
    session.cancel_block_preview();
    session
        .set_paint_operation(
            Actor::User,
            true,
            8.0,
            super::super::paint::Operation::Selection,
        )
        .unwrap();
    assert!(
        session
            .begin_transform_pivot_pick(
                Actor::User,
                camera,
                (511, 511),
                &session.transform_pivot_pick_stamp()
            )
            .is_err()
    );
    assert!(session.paint_enabled());
    session
        .set_paint_operation(
            Actor::User,
            false,
            8.0,
            super::super::paint::Operation::Selection,
        )
        .unwrap();
    for flag in ["build", "launch"] {
        session.compiling = flag == "build";
        session.pending_launch = (flag == "launch")
            .then(|| crate::session::LaunchOrigin::new(session.document_generation));
        assert!(
            session
                .begin_transform_pivot_pick(
                    Actor::User,
                    camera,
                    (511, 511),
                    &session.transform_pivot_pick_stamp()
                )
                .is_err()
        );
    }
    assert!(!session.transform_pivot_picking());
}

#[test]
fn raised_terrain_is_picked_as_a_sheet_and_hidden_surfaces_do_not_replace_the_pivot() {
    let mut session = fixture();
    let ObjectId::Solid(solid) = session.selected_ids()[0] else {
        unreachable!()
    };
    let shape =
        map_core::BrushGeometry::from_solid(session.document.solid(solid).unwrap()).unwrap();
    let face = shape
        .faces
        .iter()
        .find(|face| face.plane.normal.z > 0.99)
        .unwrap();
    let side = session.document.solid(solid).unwrap().sides[face.side].id;
    session
        .edit(Actor::User, "raised terrain fixture", |tx| {
            map_core::displacement::edit::create(
                tx,
                &[map_core::surface::FaceId { solid, side }],
                2,
                0,
            )?;
            tx.solid_mut(solid)?
                .sides
                .iter_mut()
                .find(|face| face.id == side)
                .unwrap()
                .displacement
                .as_mut()
                .unwrap()
                .elevation = 24.0;
            Ok(())
        })
        .unwrap();
    let camera = session.camera;
    let token = begin(&mut session, Actor::User, camera);
    let state = session
        .pick_transform_pivot_ray(Actor::User, token, ray())
        .unwrap();
    assert_eq!(state.current_pivot.unwrap()[2], 24.0);
    session
        .set_visibility(Actor::User, "hide", &[ObjectId::Solid(solid)])
        .unwrap();
    let pivot = session.transform_tool.pivot;
    let token = begin(&mut session, Actor::User, camera);
    assert!(
        session
            .pick_transform_pivot_ray(Actor::User, token, ray())
            .is_err()
    );
    assert_eq!(session.transform_tool.pivot, pivot);
    session
        .cancel_transform_pivot_pick(Actor::User, token)
        .unwrap();
}
