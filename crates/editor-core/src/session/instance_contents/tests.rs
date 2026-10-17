use super::*;
use crate::session::{SelectMode, working_region};
use map_core::edit;

fn opened(actor: Actor) -> (tempfile::TempDir, Session, PathBuf, PathBuf, ObjectId) {
    let (directory, game, source, child) = crate::test_assets::instance_map(false);
    let mut session = Session::new(Ok(game));
    session.open(actor, &source, true).unwrap();
    let owner = ObjectId::Entity(session.document.vmf().entities.last().unwrap().id);
    session.set_view(render::ViewOptions {
        instance_contents: true,
        ..session.view()
    });
    (directory, session, source, child, owner)
}

fn refresh(session: &mut Session, actor: Actor) -> Value {
    let request = session.begin_instance_contents_refresh(actor).unwrap();
    let result = request.run();
    session
        .finish_instance_contents_refresh(actor, result)
        .unwrap()
}

#[test]
fn world_detail_geometry_owner_picks_and_camera_bounds_preserve_authored_state() {
    for actor in [Actor::User, Actor::Agent] {
        let (_directory, mut session, source, child, owner) = opened(actor);
        session.select(&[owner], SelectMode::Replace).unwrap();
        let memory = session.document.vmf().clone();
        let disk = std::fs::read(&source).unwrap();
        let child_bytes = std::fs::read(&child).unwrap();
        let history = session.history_snapshot();
        let camera = session.camera();
        let authoring = session.object_bounds_in(session.document(), &[owner]);
        let report = refresh(&mut session, actor);
        assert_eq!(report["state"], "ready");
        assert_eq!(report["stats"]["world_brushes"], 1);
        assert_eq!(report["stats"]["detail_brushes"], 1);
        assert_eq!(report["geometry"]["vertices"], 48);
        assert_eq!(report["geometry"]["indices"], 72);
        assert_eq!(report["geometry"]["line_vertices"], 96);
        let child_vmf = Vmf::parse(&String::from_utf8(child_bytes.clone()).unwrap()).unwrap();
        let reported_sides = report["face_provenance"]
            .as_array()
            .unwrap()
            .iter()
            .map(|face| face["child_side"].as_u64().unwrap() as u32)
            .collect::<BTreeSet<_>>();
        assert_eq!(reported_sides, child_vmf.side_ids().collect());
        assert_eq!(report["face_provenance_total"], 12);
        assert_eq!(
            report["owners"][0]["bounds"],
            json!([[0.0, 0.0, 32.0], [40.0, 16.0, 48.0]])
        );
        let hits = session.instance_contents_hits(
            &Ray {
                origin: DVec3::new(8.0, 8.0, 100.0),
                direction: DVec3::NEG_Z,
            },
            None,
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].object, owner);
        assert_eq!((hits[0].solid, hits[0].side), (None, None));
        assert_eq!(hits[0].point, DVec3::new(8.0, 8.0, 48.0));
        assert_eq!(
            session.object_bounds_in(session.document(), &[owner]),
            authoring
        );
        let framing = session
            .framing_bounds_in(session.document(), &[owner], None)
            .unwrap();
        assert!(framing.1.z >= 48.0 && framing.1.x >= 40.0);
        assert_eq!(session.document.vmf(), &memory);
        assert_eq!(std::fs::read(&source).unwrap(), disk);
        assert_eq!(std::fs::read(&child).unwrap(), child_bytes);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(session.selected_ids(), vec![owner]);
        assert_eq!(session.camera(), camera);
        assert_eq!(
            std::fs::read(source.with_extension("bsp")).unwrap(),
            b"previous BSP"
        );
        assert_eq!(
            std::fs::read(source.with_extension("lin")).unwrap(),
            b"previous pointfile"
        );
        let token = report["token"].as_u64().unwrap();
        session
            .frame_instance_contents(actor, token, &[owner], 1.5)
            .unwrap();
        assert_ne!(session.camera(), camera);
        assert_eq!(session.document.vmf(), &memory);
        assert_eq!(session.history_snapshot(), history);
    }
}

#[test]
fn compound_rotations_repeated_sources_and_uvs_use_the_shared_compiler_phase() {
    let (_directory, mut session, _source, child, owner) = opened(Actor::Agent);
    let ObjectId::Entity(id) = owner else {
        unreachable!()
    };
    session
        .edit(Actor::Agent, "instance transforms", |tx| {
            tx.entity_mut(id)?.properties.set("origin", "80 16 8");
            tx.entity_mut(id)?.properties.set("angles", "17 39 -23");
            edit::create_entity(
                tx,
                "func_instance",
                Some(DVec3::new(-80.0, 16.0, 8.0)),
                &[
                    ("file".into(), "child.vmf".into()),
                    ("angles".into(), "-11 71 13".into()),
                ],
                Vec::new(),
            )?;
            Ok(())
        })
        .unwrap();
    let report = refresh(&mut session, Actor::Agent);
    assert_eq!(report["stats"]["instances"], 2);
    assert_eq!(report["stats"]["files"], 1);
    assert_eq!(report["stats"]["world_brushes"], 2);
    let ready = session.instance_contents.ready.as_ref().unwrap();
    let context =
        instances::SourceContext::capture(session.path().unwrap(), &compiler::Control::default())
            .unwrap();
    let prepared = instances::resolve(
        session.document.vmf(),
        Some(&context),
        &compiler::Control::default(),
        &mut compiler::Report::default(),
    )
    .unwrap();
    for ((actual, record), source) in ready
        .mesh
        .brushes
        .iter()
        .zip(&ready.brushes)
        .zip(prepared.generated_brushes())
    {
        assert_eq!(actual.owner, source.owner);
        assert_eq!(record.child, source.original);
        let geometry = BrushGeometry::from_solid(source.solid).unwrap();
        let vertices =
            &ready.mesh.vertices[actual.vertices.start as usize..actual.vertices.end as usize];
        for (actual, (point, side)) in vertices.iter().zip(geometry.faces.iter().flat_map(|face| {
            face.vertices
                .iter()
                .map(|point| (point, &source.solid.sides[face.side]))
        })) {
            let position = DVec3::from_array(actual.position.map(f64::from));
            assert!(position.abs_diff_eq(*point, 1e-4));
            let expected = map_core::texel_uv(side, position);
            assert!((f64::from(actual.uv[0]) - expected.x).abs() < 1e-3);
            assert!((f64::from(actual.uv[1]) - expected.y).abs() < 1e-3);
        }
    }
    assert!(source_formats::vmf::Vmf::parse(&std::fs::read_to_string(child).unwrap()).is_ok());
}

#[test]
fn stale_dependencies_late_results_and_cancel_remove_pixels_hits_and_frame_targets() {
    for mutation in [
        "child_before_finish",
        "child_after_finish",
        "edit",
        "cancel",
        "replace",
    ] {
        let (_directory, mut session, source, child, owner) = opened(Actor::Agent);
        let request = session
            .begin_instance_contents_refresh(Actor::Agent)
            .unwrap();
        let token = request.token();
        let result = request.run();
        let camera = session.camera();
        match mutation {
            "child_before_finish" => std::fs::write(&child, b"changed dependency").unwrap(),
            "edit" => {
                session
                    .edit(Actor::Agent, "unrelated parent edit", |tx| {
                        edit::create_entity(
                            tx,
                            "info_target",
                            Some(DVec3::new(12.0, 0.0, 0.0)),
                            &[],
                            Vec::new(),
                        )?;
                        Ok(())
                    })
                    .unwrap();
            }
            "cancel" => {
                session
                    .cancel_instance_contents_refresh(Actor::Agent, token)
                    .unwrap();
            }
            "replace" => {
                session.new_map(Actor::Agent, true).unwrap();
            }
            _ => {}
        }
        let finished = session.finish_instance_contents_refresh(Actor::Agent, result);
        if mutation == "edit" || mutation == "replace" {
            assert!(finished.is_err());
        } else {
            assert!(finished.is_ok());
        }
        if mutation == "child_after_finish" {
            let before = session.scene_serial();
            std::fs::write(&child, b"changed dependency").unwrap();
            assert!(session.preflight_instance_contents());
            assert!(session.scene_serial() > before);
            assert!(!session.preflight_instance_contents());
        }
        let preview = session.instance_contents_preview(true);
        assert!(preview.mesh.is_none(), "{mutation}");
        assert!(
            session
                .instance_contents_hits(
                    &Ray {
                        origin: DVec3::new(8.0, 8.0, 100.0),
                        direction: DVec3::NEG_Z
                    },
                    Some(true)
                )
                .is_empty()
        );
        assert!(
            session
                .frame_instance_contents(Actor::Agent, token, &[owner], 1.0)
                .is_err()
        );
        if mutation != "replace" {
            assert_eq!(session.camera(), camera);
        }
        assert_eq!(
            std::fs::read(source.with_extension("bsp")).unwrap(),
            b"previous BSP"
        );
        assert_eq!(
            std::fs::read(source.with_extension("lin")).unwrap(),
            b"previous pointfile"
        );
        assert!(
            !session.inspect_instance_contents(0, 64).unwrap()["loading"]
                .as_bool()
                .unwrap()
        );
    }
}

#[test]
fn context_and_working_region_preflight_track_contents_instead_of_marker_origin() {
    let (_directory, mut session, _source, child, owner) = opened(Actor::User);
    let mut child_vmf = Vmf::parse(&std::fs::read_to_string(&child).unwrap()).unwrap();
    for solid in child_vmf.world.solids.iter_mut().chain(
        child_vmf
            .entities
            .iter_mut()
            .flat_map(|entity| &mut entity.solids),
    ) {
        for side in &mut solid.sides {
            for point in &mut side.plane.0 {
                point.x -= 512.0;
            }
        }
    }
    std::fs::write(&child, child_vmf.write(&Default::default())).unwrap();
    let ObjectId::Entity(id) = owner else {
        unreachable!()
    };
    session
        .edit(Actor::User, "marker outside", |tx| {
            tx.entity_mut(id)?.properties.set("origin", "512 0 0");
            Ok(())
        })
        .unwrap();
    refresh(&mut session, Actor::User);
    session
        .set_working_region(
            Actor::User,
            working_region::Action::Set {
                bounds: [DVec3::new(-16.0, -16.0, 16.0), DVec3::new(64.0, 32.0, 64.0)],
                enabled: true,
            },
            &session.working_region_stamp(),
        )
        .unwrap();
    assert!(session.instance_contents_preview(true).owners.contains(&id));
    let ray = Ray {
        origin: DVec3::new(8.0, 8.0, 100.0),
        direction: DVec3::NEG_Z,
    };
    assert_eq!(session.instance_contents_hits(&ray, None).len(), 1);
    session.set_face_mode(true);
    session.preflight_instance_contents();
    assert!(session.instance_contents_preview(true).mesh.is_none());
    assert!(session.instance_contents_hits(&ray, None).is_empty());
    assert_eq!(
        session.inspect_instance_contents(0, 64).unwrap()["status"],
        "context_suppressed"
    );
    session.set_face_mode(false);
    let before = session.scene_serial();
    assert!(session.preflight_instance_contents());
    assert!(session.scene_serial() > before);
    assert!(session.instance_contents_preview(true).mesh.is_some());
}

#[test]
fn failure_tokens_single_worker_and_empty_untitled_maps_remain_explicit() {
    let mut empty = Session::new(Err("no assets".into()));
    assert_eq!(refresh(&mut empty, Actor::Agent)["state"], "empty");
    let (_directory, mut session, _source, child, _owner) = opened(Actor::Agent);
    let request = session
        .begin_instance_contents_refresh(Actor::Agent)
        .unwrap();
    let token = request.token();
    assert!(
        session
            .begin_instance_contents_refresh(Actor::Agent)
            .is_err()
    );
    assert!(
        session
            .cancel_instance_contents_refresh(Actor::Agent, token + 1)
            .is_err()
    );
    assert!(
        session
            .clear_instance_contents(Actor::Agent, token)
            .is_err()
    );
    drop(request);
    let failed = session
        .fail_instance_contents_refresh(Actor::Agent, token, "worker unavailable".into())
        .unwrap();
    assert_eq!(failed["state"], "failed");
    let token = failed["token"].as_u64().unwrap();
    session
        .clear_instance_contents(Actor::Agent, token)
        .unwrap();
    assert!(
        session
            .clear_instance_contents(Actor::Agent, token)
            .is_err()
    );
    std::fs::write(
        child,
        "world { id 1 classname worldspawn } entity {id 2 classname info_target}",
    )
    .unwrap();
    let failed = refresh(&mut session, Actor::Agent);
    assert_eq!(failed["state"], "failed");
    assert!(failed["error"].as_str().unwrap().contains("unsupported"));
    assert!(session.inspect_instance_contents(0, 65).is_err());
}

#[test]
fn foreign_worker_result_cannot_finish_an_equal_numbered_request() {
    let (_directory, mut left, source, _child, _owner) = opened(Actor::Agent);
    let mut right = Session::new(Err("no assets".into()));
    right.open(Actor::Agent, &source, true).unwrap();
    let left_request = left.begin_instance_contents_refresh(Actor::Agent).unwrap();
    let right_request = right.begin_instance_contents_refresh(Actor::Agent).unwrap();
    assert_eq!(left_request.token(), right_request.token());
    let result = right_request.run();
    assert!(
        left.finish_instance_contents_refresh(Actor::Agent, result)
            .is_err()
    );
    assert_eq!(
        left.inspect_instance_contents(0, 64).unwrap()["loading"],
        true
    );
    let result = left_request.run();
    assert_eq!(
        left.finish_instance_contents_refresh(Actor::Agent, result)
            .unwrap()["state"],
        "ready"
    );
}

#[test]
fn worker_spawn_failure_after_reopen_releases_only_its_expired_pending_slot() {
    let (_directory, mut session, _source, _child, _owner) = opened(Actor::Agent);
    let request = session
        .begin_instance_contents_refresh(Actor::Agent)
        .unwrap();
    let token = request.token();
    session.new_map(Actor::Agent, true).unwrap();
    drop(request);
    assert!(
        session
            .fail_instance_contents_refresh(Actor::Agent, token, "worker unavailable".into())
            .is_err()
    );
    assert_eq!(refresh(&mut session, Actor::Agent)["state"], "empty");
}

#[test]
fn render_budget_rejects_whole_admitted_geometry_without_partial_snapshot() {
    use map_core::primitive::{self, Shape, Spec};
    let (_directory, mut session, _source, child, _owner) = opened(Actor::Agent);
    let mut document = Document::empty();
    document
        .edit("reusable cylinder module", |tx| {
            for index in 0..12 {
                primitive::create(
                    tx,
                    &Spec {
                        shape: Shape::Cylinder,
                        segments: 16,
                        mins: DVec3::new(f64::from(index) * 4.0, 0.0, 32.0),
                        maxs: DVec3::new(f64::from(index) * 4.0 + 2.0, 2.0, 48.0),
                        material: "dev/flat".into(),
                        ..Spec::default()
                    },
                )?;
            }
            Ok(())
        })
        .unwrap();
    std::fs::write(&child, document.vmf().write(&Default::default())).unwrap();
    session
        .edit(Actor::Agent, "repeated modules", |tx| {
            for _ in 1..instances::MAX_INSTANCES {
                edit::create_entity(
                    tx,
                    "func_instance",
                    Some(DVec3::ZERO),
                    &[("file".into(), "child.vmf".into())],
                    Vec::new(),
                )?;
            }
            Ok(())
        })
        .unwrap();
    let report = refresh(&mut session, Actor::Agent);
    assert_eq!(report["state"], "failed");
    assert!(report["error"].as_str().unwrap().contains("geometry"));
    assert!(session.instance_contents_preview(true).mesh.is_none());
}

#[cfg(unix)]
#[test]
fn frozen_root_alias_retarget_rejects_refresh_before_loading_new_dependencies() {
    use std::os::unix::fs::symlink;
    let (directory, mut session, source, _child, _owner) = opened(Actor::Agent);
    let physical = directory.path().join("physical.vmf");
    std::fs::rename(&source, &physical).unwrap();
    symlink(&physical, &source).unwrap();
    let request = session
        .begin_instance_contents_refresh(Actor::Agent)
        .unwrap();
    std::fs::remove_file(&source).unwrap();
    let replacement = directory.path().join("replacement.vmf");
    std::fs::copy(&physical, &replacement).unwrap();
    symlink(&replacement, &source).unwrap();
    let result = request.run();
    let report = session
        .finish_instance_contents_refresh(Actor::Agent, result)
        .unwrap();
    assert_eq!(report["state"], "failed");
    assert!(session.instance_contents_preview(true).mesh.is_none());
}
