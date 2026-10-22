use super::*;
use std::collections::BTreeMap;

fn top(solid: &source_formats::vmf::Solid) -> FaceId {
    FaceId {
        solid: solid.id,
        side: solid
            .sides
            .iter()
            .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
            .unwrap()
            .id,
    }
}

fn overlay(source: FaceId) -> crate::session::overlay::Request {
    crate::session::overlay::Request {
        source,
        options: map_core::overlay::Options {
            material: "test/checker".into(),
            ..Default::default()
        },
    }
}

#[test]
fn session_authored_terrain_overlays_keep_real_world_detail_reference_geometry_for_both_actors() {
    for actor in [Actor::User, Actor::Agent] {
        for detail in [false, true] {
            let (directory, game) = crate::test_assets::game();
            let source = directory.path().join("parent.vmf");
            let child = directory.path().join("child.vmf");
            let mut session = Session::new(Ok(game));
            let solid = session
                .create_block(
                    actor,
                    DVec3::ZERO,
                    DVec3::new(128.0, 128.0, 16.0),
                    "test/checker",
                    false,
                )
                .unwrap();
            let ObjectId::Solid(id) = solid else {
                unreachable!()
            };
            let face = top(map_core::surface::solid(session.document(), id).unwrap());
            session.select_faces(&[face], SelectMode::Replace).unwrap();
            let token = session
                .preview_overlay(actor, overlay(face), &session.overlay_stamp())
                .unwrap()["draft"]["token"]
                .as_u64()
                .unwrap();
            let token = session.review_overlay(actor, token).unwrap()["draft"]["token"]
                .as_u64()
                .unwrap();
            let applied = session.apply_overlay(actor, token).unwrap();
            let ObjectId::Entity(helper) =
                crate::identity::parse_id(applied["created"].as_str().unwrap()).unwrap()
            else {
                unreachable!()
            };
            session.select_faces(&[face], SelectMode::Replace).unwrap();
            session.create_terrain(actor, &[face], 2, 0).unwrap();
            session
                .preview_terrain_edit(
                    actor,
                    &[face],
                    map_core::displacement::Operation::VertexMove {
                        vertices: vec![12],
                        offset: DVec3::Z * 32.0,
                    },
                )
                .unwrap();
            session.commit_face_preview(actor).unwrap();
            if detail {
                session
                    .tie_to_entity(actor, &[solid], "func_detail", BTreeMap::new())
                    .unwrap();
            }
            session.save(actor, Some(&child)).unwrap();
            session.open(actor, &child, false).unwrap();
            let target = map_core::surface::side(session.document(), face).unwrap();
            assert_eq!(target.displacement.as_ref().unwrap().power, 2);
            let helper_entity = session.document().entity(helper).unwrap();
            assert_eq!(helper_entity.classname(), Some("info_overlay"));
            assert!(helper_entity.properties.get("targetname").is_none());
            assert_eq!(
                helper_entity.properties.get("sides"),
                Some(face.side.to_string().as_str())
            );
            let saved = session.document().vmf().clone();
            let original = std::fs::read(&child).unwrap();
            let history = session.history_snapshot();
            let draft = session.get_overlay_state();
            for error in [
                session
                    .preview_overlay(actor, overlay(face), &session.overlay_stamp())
                    .unwrap_err(),
                session
                    .preview_overlay_refit(actor, helper, &session.overlay_stamp())
                    .unwrap_err(),
                session
                    .preview_overlay_rebind(actor, helper, face, &session.overlay_stamp())
                    .unwrap_err(),
            ] {
                assert!(error.contains("terrain"), "{error}");
            }
            assert_eq!(session.document().vmf(), &saved);
            assert_eq!(session.history_snapshot(), history);
            assert_eq!(session.get_overlay_state(), draft);
            assert_eq!(std::fs::read(&child).unwrap(), original);
            let mut without_overlay = session.document().vmf().clone();
            without_overlay
                .entities
                .retain(|entity| entity.id != helper);
            let bare_child = without_overlay.write(&Default::default());
            std::fs::write(
                &source,
                crate::test_assets::room().write(&Default::default()),
            )
            .unwrap();
            session.open(actor, &source, false).unwrap();
            let form = session.begin_instance_reference(actor, None).unwrap()["token"]
                .as_u64()
                .unwrap();
            session
                .update_instance_reference(
                    actor,
                    form,
                    crate::session::instance_reference::Changes {
                        file: Some("child.vmf".into()),
                        origin: Some("64 -32 48".into()),
                        angles: Some("0 0 0".into()),
                    },
                )
                .unwrap();
            assert_eq!(
                session.check_instance_reference(actor, form).unwrap()["presence_checked"],
                true
            );
            let reference = session.apply_instance_reference(actor, form).unwrap();
            let owner = crate::identity::parse_id(reference["id"].as_str().unwrap()).unwrap();
            session.save(actor, None).unwrap();
            session.set_face_mode(false);
            session.set_view(render::ViewOptions {
                instance_contents: true,
                ..session.view()
            });
            session.select(&[owner], SelectMode::Replace).unwrap();
            let root_face = top(&session.document().vmf().world.solids[0]);
            let root_token = session
                .preview_overlay(actor, overlay(root_face), &session.overlay_stamp())
                .unwrap()["draft"]["token"]
                .as_u64()
                .unwrap();
            let root_draft = session.get_overlay_state();
            let memory = session.document().vmf().clone();
            let history = session.history_snapshot();
            let camera = session.camera();
            let settings = session.compile_settings();
            let parent = std::fs::read(&source).unwrap();
            let ray = Ray {
                origin: DVec3::new(128.0, 32.0, 160.0),
                direction: DVec3::NEG_Z,
            };
            std::fs::write(&child, bare_child).unwrap();
            let baseline = refresh(&mut session, actor);
            assert_eq!(baseline["state"], "ready", "{baseline}");
            assert_eq!(session.camera(), camera);
            assert!(baseline["stats"].get("overlays").is_none());
            assert!(baseline.get("overlay_preview").is_none());
            let baseline_mesh = session.instance_contents_preview(true).mesh.unwrap();
            let baseline_hits = session.instance_contents_hits(&ray, Some(true));
            session
                .frame_instance_contents(actor, baseline["token"].as_u64().unwrap(), &[owner], 1.5)
                .unwrap();
            let framed = session.camera();
            session.set_camera(camera);
            std::fs::write(&child, &original).unwrap();
            let report = refresh(&mut session, actor);
            assert_eq!(report["state"], "ready", "{report}");
            assert_eq!(session.camera(), camera);
            assert_eq!(report["stats"]["overlays"], 1);
            assert_eq!(report["stats"]["terrain_patches"], 1);
            assert_eq!(report["stats"]["terrain_vertices"], 25);
            assert_eq!(report["stats"]["world_brushes"], usize::from(!detail));
            assert_eq!(report["stats"]["detail_brushes"], usize::from(detail));
            assert_eq!(report["stats"]["dependencies"][0]["path"], "child.vmf");
            assert_eq!(report["stats"]["dependencies"][0]["bytes"], original.len());
            assert_eq!(report["overlay_preview"]["available"], false);
            assert_eq!(report["overlay_preview"]["reason"], OVERLAY_PREVIEW_REASON);
            assert_eq!(report["available"], true);
            assert_eq!(report["displayed"], true);
            assert!(report["suppression_reason"].is_null());
            for field in ["geometry", "brushes", "face_provenance", "owners"] {
                assert_eq!(report[field], baseline[field], "{field}");
            }
            assert_eq!(report["geometry"]["vertices"], 25);
            assert_eq!(report["geometry"]["triangles"], 32);
            assert_eq!(report["face_provenance_total"], 1);
            let provenance = &report["face_provenance"][0];
            assert_eq!(provenance["owner"], reference["id"]);
            assert_eq!(provenance["file"], "child.vmf");
            assert_eq!(provenance["child_brush"], face.solid);
            assert_eq!(provenance["child_side"], face.side);
            assert_eq!(provenance["terrain"], true);
            assert_eq!(provenance["power"], 2);
            let mesh = session.instance_contents_preview(true).mesh.unwrap();
            assert_eq!(mesh.vertices, baseline_mesh.vertices);
            assert_eq!(mesh.indices, baseline_mesh.indices);
            assert_eq!(mesh.edges, baseline_mesh.edges);
            assert_eq!(mesh.brushes.len(), 1);
            assert_eq!(
                report["owners"][0]["bounds"],
                json!([[64.0, -32.0, 64.0], [192.0, 96.0, 96.0]])
            );
            let hits = session.instance_contents_hits(&ray, Some(true));
            assert_eq!(hits.len(), 1);
            assert_eq!(hits, baseline_hits);
            assert_eq!(hits[0].object, owner);
            assert_eq!(hits[0].point, DVec3::new(128.0, 32.0, 96.0));
            assert_eq!((hits[0].solid, hits[0].side), (None, None));
            assert!(
                session
                    .instance_contents_hits(
                        &Ray {
                            origin: DVec3::new(128.0, 32.0, 60.0),
                            direction: DVec3::NEG_Z,
                        },
                        Some(true),
                    )
                    .is_empty()
            );
            session
                .frame_instance_contents(actor, report["token"].as_u64().unwrap(), &[owner], 1.5)
                .unwrap();
            assert_eq!(session.camera(), framed);
            assert_eq!(session.get_overlay_state(), root_draft);
            assert_eq!(session.document().vmf(), &memory);
            assert_eq!(session.history_snapshot(), history);
            assert_eq!(session.compile_settings(), settings);
            assert_eq!(session.selected_ids(), vec![owner]);
            assert_eq!(std::fs::read(&source).unwrap(), parent);
            assert_eq!(std::fs::read(&child).unwrap(), original);
            session.cancel_overlay(actor, root_token).unwrap();
        }
    }
}
