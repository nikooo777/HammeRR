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
