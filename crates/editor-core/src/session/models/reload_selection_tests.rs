use super::*;
use crate::session::{Pointer, SelectMode, paint, volume};
use glam::{DVec2, Vec3};
use map_core::region::Match;
use render::{Camera, Projection};

fn pixel(camera: Camera, point: DVec3) -> DVec2 {
    let ndc = camera.view_projection(1.0).project_point3(point.as_vec3());
    DVec2::new((ndc.x as f64 + 1.0) * 128.0, (1.0 - ndc.y as f64) * 128.0)
}

fn capture(session: &mut Session, actor: Actor, kind: &str) -> (u64, Vec<ObjectId>) {
    match kind {
        "paint" => {
            let token = session
                .begin_paint_selection(paint::Spec {
                    camera: session.camera(),
                    pointer: Pointer {
                        position: pixel(session.camera(), DVec3::new(22.0, 0.0, 0.0)),
                        size: (256, 256),
                    },
                    radius: 2.0,
                    mode: SelectMode::Replace,
                    faces: false,
                })
                .unwrap();
            (
                token,
                session
                    .paint_stroke()
                    .unwrap()
                    .objects
                    .iter()
                    .copied()
                    .collect(),
            )
        }
        "volume" => {
            let token = session
                .preview_volume_selection(volume::Spec {
                    mins: DVec3::new(20.0, -2.0, -1.0),
                    maxs: DVec3::new(30.0, 2.0, 1.0),
                    rule: Match::Intersecting,
                    mode: SelectMode::Replace,
                    faces: false,
                })
                .unwrap();
            (
                token,
                session.volume_preview().unwrap().matches.objects.clone(),
            )
        }
        _ => {
            let a = pixel(session.camera(), DVec3::new(20.0, -2.0, 0.0));
            let b = pixel(session.camera(), DVec3::new(30.0, 2.0, 0.0));
            let matches = session
                .preview_area_selection(
                    actor,
                    session.camera(),
                    Pointer {
                        position: a.min(b),
                        size: (256, 256),
                    },
                    a.max(b),
                    Match::Intersecting,
                    SelectMode::Replace,
                )
                .unwrap();
            (0, matches.objects)
        }
    }
}

#[test]
fn reload_retires_selections_captured_from_model_geometry() {
    for actor in [Actor::User, Actor::Agent] {
        for kind in ["paint", "volume", "area"] {
            let (directory, game) = crate::test_assets::model_lighting::game();
            let mut session = Session::new(Ok(game));
            let id = session
                .create_entity(
                    actor,
                    "prop_dynamic",
                    Some(DVec3::ZERO),
                    BTreeMap::from([("model".into(), "models/test/receiver.mdl".into())]),
                )
                .unwrap();
            session.select(&[], SelectMode::Replace).unwrap();
            session.set_camera(Camera::planar(Projection::Top, Vec3::ZERO, 128.0));
            let source = session.document.vmf().clone();
            let history = session.history_snapshot();
            let camera = session.camera();
            let (token, matches) = capture(&mut session, actor, kind);
            assert_eq!(matches, [id], "{kind}");
            std::fs::remove_file(directory.path().join("models/test/receiver.vvd")).unwrap();

            session.refresh_model_assets(actor);

            match kind {
                "paint" => {
                    assert!(
                        session
                            .update_paint_selection(token, &[DVec2::splat(128.0)])
                            .is_err()
                    );
                    assert!(session.commit_paint_selection(actor, token).is_err());
                }
                "volume" => assert!(session.commit_volume_selection(actor, token).is_err()),
                _ => assert!(session.commit_area_selection().is_err()),
            }
            assert!(session.selected_ids().is_empty());
            assert_eq!(session.document.vmf(), &source);
            assert_eq!(session.history_snapshot(), history);
            assert_eq!(session.camera(), camera);
            let (_, fresh_matches) = capture(&mut session, actor, kind);
            assert!(fresh_matches.is_empty(), "{kind}: {fresh_matches:?}");
        }
    }
}
