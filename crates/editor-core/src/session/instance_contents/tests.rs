use super::*;
use crate::session::{SelectMode, working_region};
use map_core::edit;

mod brush_models;
mod cubemaps;
mod environment_lights;
mod fog_controllers;
mod light_appearances;
mod light_value_bindings;
mod lighting_origins;
mod occluders;
mod overlays;
mod player_spawns;
mod spotlight_targets;
mod static_props;
mod terrain;
mod terrain_overlays;
mod visclusters;

fn nested_opened(
    actor: Actor,
) -> (
    tempfile::TempDir,
    Session,
    PathBuf,
    PathBuf,
    [PathBuf; 2],
    ObjectId,
) {
    let (directory, mut session, source, child, owner) = opened(actor);
    let leaves = [
        directory.path().join("leaf-a.vmf"),
        directory.path().join("leaf-b.vmf"),
    ];
    for leaf in &leaves {
        std::fs::copy(&child, leaf).unwrap();
    }
    let modules = directory.path().join("modules");
    std::fs::create_dir(&modules).unwrap();
    let middle = modules.join("middle.vmf");
    let mut document = Document::empty();
    document
        .edit("nested references", |tx| {
            for (file, origin) in [
                ("leaf-a.vmf", DVec3::ZERO),
                ("leaf-b.vmf", DVec3::new(64.0, 0.0, 0.0)),
            ] {
                edit::create_entity(
                    tx,
                    "func_instance",
                    Some(origin),
                    &[
                        ("file".into(), file.into()),
                        ("angles".into(), "0 0 0".into()),
                    ],
                    Vec::new(),
                )?;
            }
            Ok(())
        })
        .unwrap();
    std::fs::write(&middle, document.vmf().write(&Default::default())).unwrap();
    let ObjectId::Entity(id) = owner else {
        unreachable!()
    };
    session
        .edit(actor, "nested root reference", |tx| {
            tx.entity_mut(id)?
                .properties
                .set("file", r".\modules\middle.vmf");
            Ok(())
        })
        .unwrap();
    (directory, session, source, middle, leaves, owner)
}

#[test]
fn nested_contents_keep_root_file_leaf_ids_and_root_only_geometry_targets() {
    for actor in [Actor::User, Actor::Agent] {
        let (_directory, mut session, source, middle, leaves, owner) = nested_opened(actor);
        session.select(&[owner], SelectMode::Replace).unwrap();
        let memory = session.document.vmf().clone();
        let source_bytes = std::fs::read(&source).unwrap();
        let middle_bytes = std::fs::read(&middle).unwrap();
        let leaf_bytes = leaves.each_ref().map(|path| std::fs::read(path).unwrap());
        let history = session.history_snapshot();
        let camera = session.camera();
        let settings = session.compile_settings();
        let report = refresh(&mut session, actor);
        assert_eq!(report["state"], "ready", "{report}");
        assert_eq!(report["stats"]["instances"], 3);
        assert_eq!(report["stats"]["files"], 3);
        assert_eq!(report["stats"]["max_expansion_depth"], 2);
        assert_eq!(report["limits"]["depth"], instances::MAX_DEPTH);
        assert_eq!(report["limits"]["expansion_depth"], 4);
        assert_eq!(report["owners"][0]["file"], "modules/middle.vmf");
        assert_eq!(
            report["owners"][0]["bounds"],
            json!([[0.0, 0.0, 32.0], [104.0, 16.0, 48.0]])
        );
        assert_eq!(report["owners"][0]["world_brushes"], 2);
        assert_eq!(report["owners"][0]["detail_brushes"], 2);
        let rows = report["brushes"].as_array().unwrap();
        assert_eq!(rows.len(), 4);
        assert_eq!(
            rows.iter()
                .map(|row| row["file"].as_str().unwrap())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from(["leaf-a.vmf", "leaf-b.vmf"])
        );
        for file in ["leaf-a.vmf", "leaf-b.vmf"] {
            let leaf = Vmf::parse(&String::from_utf8(leaf_bytes[0].clone()).unwrap()).unwrap();
            let source_solids = leaf
                .world
                .solids
                .iter()
                .chain(leaf.entities.iter().flat_map(|entity| &entity.solids))
                .map(|solid| solid.id)
                .collect::<BTreeSet<_>>();
            let actual_solids = rows
                .iter()
                .filter(|row| row["file"] == file)
                .map(|row| row["child_brush"].as_u64().unwrap() as u32)
                .collect::<BTreeSet<_>>();
            assert_eq!(actual_solids, source_solids);
            let actual_sides = report["face_provenance"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|row| row["file"] == file)
                .map(|row| row["child_side"].as_u64().unwrap() as u32)
                .collect::<BTreeSet<_>>();
            assert_eq!(actual_sides, leaf.side_ids().collect());
        }
        assert_eq!(report["face_provenance_total"], 24);
        assert_eq!(report["face_provenance_truncated"], false);
        assert!(rows.iter().all(|row| row["owner"]
            == format!(
                "entity:{}",
                match owner {
                    ObjectId::Entity(id) => id,
                    _ => unreachable!(),
                }
            )));
        for x in [8.0, 72.0] {
            let hits = session.instance_contents_hits(
                &Ray {
                    origin: DVec3::new(x, 8.0, 100.0),
                    direction: DVec3::NEG_Z,
                },
                Some(true),
            );
            assert_eq!(hits.len(), 1);
            assert_eq!(hits[0].object, owner);
            assert_eq!((hits[0].solid, hits[0].side), (None, None));
            assert_eq!(hits[0].point, DVec3::new(x, 8.0, 48.0));
        }
        let frame = session
            .framing_bounds_in(session.document(), &[owner], None)
            .unwrap();
        assert!(frame.1.x >= 104.0 && frame.1.z >= 48.0);
        assert_eq!(session.document.vmf(), &memory);
        assert_eq!(std::fs::read(&source).unwrap(), source_bytes);
        assert_eq!(std::fs::read(&middle).unwrap(), middle_bytes);
        assert_eq!(
            leaves.each_ref().map(|path| std::fs::read(path).unwrap()),
            leaf_bytes
        );
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(session.camera(), camera);
        assert_eq!(session.selected_ids(), vec![owner]);
        assert_eq!(session.compile_settings(), settings);
        assert_eq!(
            std::fs::read(source.with_extension("bsp")).unwrap(),
            b"previous BSP"
        );
        assert_eq!(
            std::fs::read(source.with_extension("lin")).unwrap(),
            b"previous pointfile"
        );
    }
}

#[test]
fn changed_or_failed_grandchildren_remove_the_whole_contents_layer_and_preserve_a_form() {
    for actor in [Actor::User, Actor::Agent] {
        for change in [
            "before_finish",
            "after_finish",
            "failed_refresh",
            "missing_middle",
        ] {
            let (_directory, mut session, source, middle, leaves, owner) = nested_opened(actor);
            session.select(&[owner], SelectMode::Replace).unwrap();
            let ready = refresh(&mut session, actor);
            assert_eq!(ready["state"], "ready");
            let ObjectId::Entity(id) = owner else {
                unreachable!()
            };
            let form = session.begin_instance_reference(actor, Some(id)).unwrap();
            let form = session
                .update_instance_reference(
                    actor,
                    form["token"].as_u64().unwrap(),
                    crate::session::instance_reference::Changes {
                        origin: Some("21.5 -7.25 3.125".into()),
                        ..Default::default()
                    },
                )
                .unwrap();
            let memory = session.document.vmf().clone();
            let history = session.history_snapshot();
            let camera = session.camera();
            let settings = session.compile_settings();
            let source_bytes = std::fs::read(&source).unwrap();
            let token;
            if change == "before_finish" {
                let request = session.begin_instance_contents_refresh(actor).unwrap();
                token = request.token();
                let result = request.run();
                std::fs::write(&leaves[0], b"changed grandchild").unwrap();
                let report = session
                    .finish_instance_contents_refresh(actor, result)
                    .unwrap();
                assert_eq!(report["state"], "failed");
            } else if change == "failed_refresh" {
                std::fs::write(
                    &leaves[0],
                    b"world { id 1 classname worldspawn } entity { id 2 classname info_null }",
                )
                .unwrap();
                let report = refresh(&mut session, actor);
                token = report["token"].as_u64().unwrap();
                assert_eq!(report["state"], "failed");
                assert!(report["error"].as_str().unwrap().contains("unsupported"));
            } else {
                token = ready["token"].as_u64().unwrap();
                if change == "missing_middle" {
                    std::fs::remove_file(&middle).unwrap();
                } else {
                    let mut bytes = std::fs::read(&leaves[1]).unwrap();
                    let newline = bytes.iter().position(|byte| *byte == b'\n').unwrap();
                    bytes[newline] = b' ';
                    std::fs::write(&leaves[1], bytes).unwrap();
                }
                assert!(session.preflight_instance_contents());
                assert_eq!(
                    session.inspect_instance_contents(0, 64).unwrap()["state"],
                    "stale"
                );
            }
            let report = session.inspect_instance_contents(0, 64).unwrap();
            assert_eq!(report["available"], false, "{change}");
            assert!(report["owners"].as_array().unwrap().is_empty());
            assert!(report["brushes"].as_array().unwrap().is_empty());
            assert!(report["face_provenance"].as_array().unwrap().is_empty());
            assert!(session.instance_contents.ready.is_none());
            assert!(session.instance_contents_preview(true).mesh.is_none());
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
                    .frame_instance_contents(actor, token, &[owner], 1.0)
                    .is_err()
            );
            assert_eq!(session.get_instance_reference(), form);
            assert_eq!(session.document.vmf(), &memory);
            assert_eq!(session.history_snapshot(), history);
            assert_eq!(session.camera(), camera);
            assert_eq!(session.selected_ids(), vec![owner]);
            assert_eq!(session.compile_settings(), settings);
            assert_eq!(std::fs::read(&source).unwrap(), source_bytes);
            assert_eq!(
                std::fs::read(source.with_extension("bsp")).unwrap(),
                b"previous BSP"
            );
            assert_eq!(
                std::fs::read(source.with_extension("lin")).unwrap(),
                b"previous pointfile"
            );
        }
    }
}

#[test]
fn retained_leaf_filename_budget_counts_capacity_instead_of_length() {
    let mut record = BrushRecord {
        bounds: [DVec3::ZERO; 2],
        file: String::with_capacity(8192),
        child: 7,
        detail: false,
        faces: Vec::new(),
    };
    record.file.push_str("leaf.vmf");
    let mesh = Mesh::default();
    let with_file = allocation(&mesh, std::slice::from_ref(&record), 1, &[], 0).unwrap();
    let capacity = record.file.capacity();
    record.file = String::new();
    let without_file = allocation(&mesh, std::slice::from_ref(&record), 1, &[], 0).unwrap();
    assert_eq!(with_file - without_file, capacity);
    assert!(capacity > "leaf.vmf".len());
}

#[test]
fn cancelled_contents_keep_one_worker_across_replacement_until_owned_result_retires() {
    let (_directory, mut session, _source, _child, _owner) = opened(Actor::Agent);
    let request = session
        .begin_instance_contents_refresh(Actor::Agent)
        .unwrap();
    session.new_map(Actor::User, true).unwrap();
    assert!(
        session
            .begin_instance_contents_refresh(Actor::User)
            .is_err()
    );
    let result = request.run();
    assert!(result.control.cancelled());
    assert!(
        session
            .begin_instance_contents_refresh(Actor::User)
            .is_err()
    );
    drop(result);
    let next = session
        .begin_instance_contents_refresh(Actor::User)
        .unwrap();
    drop(next);
    let result = session
        .begin_instance_contents_refresh(Actor::User)
        .unwrap()
        .run();
    assert_eq!(
        session
            .finish_instance_contents_refresh(Actor::User, result)
            .unwrap()["state"],
        "empty"
    );
    assert!(!session.instance_workers_busy());
}

#[test]
fn contents_panic_completion_retires_its_ticket_without_stranding_the_session() {
    let mut session = Session::new(Err("no assets".into()));
    let request = session
        .begin_instance_contents_refresh(Actor::User)
        .unwrap();
    let result = request.run_with(|_| panic!("injected contents worker panic"));
    assert!(session.instance_workers_busy());
    assert!(
        session
            .begin_instance_contents_refresh(Actor::Agent)
            .is_err()
    );
    let report = session
        .finish_instance_contents_refresh(Actor::User, result)
        .unwrap();
    assert_eq!(report["error"], "Instance contents worker panicked");
    assert!(!session.instance_workers_busy());
    assert_eq!(refresh(&mut session, Actor::User)["state"], "empty");
}

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
fn child_atlas_metadata_refresh_compile_and_metadata_only_staleness_share_original_sources() {
    use map_core::texture_atlas::regions::{Region, Regions};
    use map_core::texture_atlas::{FitRegion, Rectangle};
    use sha2::{Digest, Sha256};

    for actor in [Actor::User, Actor::Agent] {
        let (_directory, mut session, source, child, owner) = opened(actor);
        session.select(&[owner], SelectMode::Replace).unwrap();
        let plain = refresh(&mut session, actor);
        assert_eq!(plain["state"], "ready", "{plain}");
        let memory = session.document().vmf().clone();
        let history = session.history_snapshot();
        let camera = session.camera();
        let parent_bytes = std::fs::read(&source).unwrap();
        let mut document =
            Document::new(Vmf::parse(&std::fs::read_to_string(&child).unwrap()).unwrap());
        let mut catalog = Regions {
            regions: vec![Region {
                id: 1,
                name: "Panel".into(),
                material: "unmounted/editor_only_atlas".into(),
                image_identity: "a".repeat(64),
                dimensions: [64, 32],
                region: FitRegion {
                    rectangle: Rectangle {
                        x: 16,
                        y: 8,
                        width: 32,
                        height: 16,
                    },
                    padding: 2,
                },
                extra: Vec::new(),
            }],
            extra: vec![source_formats::keyvalues::Node::string(
                "extension",
                "original bytes",
            )],
        };
        document
            .edit("save atlas region", |tx| catalog.clone().write(tx))
            .unwrap();
        let child_bytes = document.vmf().write(&Default::default()).into_bytes();
        std::fs::write(&child, &child_bytes).unwrap();
        assert_eq!(
            session.inspect_instance_contents(0, 64).unwrap()["state"],
            "stale"
        );
        let report = refresh(&mut session, actor);
        assert_eq!(report["state"], "ready", "{report}");
        assert_eq!(report["geometry"], plain["geometry"]);
        assert_eq!(report["face_provenance"], plain["face_provenance"]);
        assert_eq!(
            report["stats"]["dependencies"][0]["bytes"],
            child_bytes.len()
        );
        assert_eq!(
            report["stats"]["dependencies"][0]["sha256"],
            format!("{:x}", Sha256::digest(&child_bytes))
        );
        let request = session.begin_compile(actor, None).unwrap();
        let result = request.run();
        assert!(
            result.as_ref().unwrap().output.bsp.is_some(),
            "{:?}",
            result.as_ref().unwrap().output.report
        );
        session.finish_compile(actor, &result);
        assert!(result.as_ref().unwrap().published());
        assert_eq!(session.compile_status()["job"]["stale"], false);
        let published = std::fs::read(source.with_extension("bsp")).unwrap();
        assert_eq!(std::fs::read(&child).unwrap(), child_bytes);
        drop(result);

        catalog.regions[0].name = "Renamed metadata only".into();
        document
            .edit("rename atlas region", |tx| catalog.write(tx))
            .unwrap();
        std::fs::write(&child, document.vmf().write(&Default::default())).unwrap();
        assert_eq!(session.compile_status()["job"]["stale"], true);
        assert_eq!(
            session.inspect_instance_contents(0, 64).unwrap()["state"],
            "stale"
        );
        assert!(session.instance_contents_preview(true).mesh.is_none());
        assert_eq!(refresh(&mut session, actor)["state"], "ready");
        assert_eq!(
            std::fs::read(source.with_extension("bsp")).unwrap(),
            published
        );
        assert_eq!(session.document().vmf(), &memory);
        assert_eq!(session.history_snapshot(), history);
        assert_eq!(session.camera(), camera);
        assert_eq!(session.selected_ids(), vec![owner]);
        assert_eq!(std::fs::read(&source).unwrap(), parent_bytes);
    }
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
            let before = session.instance_contents_serial();
            std::fs::write(&child, b"changed dependency").unwrap();
            assert!(session.preflight_instance_contents());
            assert!(session.instance_contents_serial() > before);
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
    let raw = session.pick(&ray);
    assert_eq!(session.pick_for_selection(&ray), raw);
    session
        .set_selection_filters(
            Actor::Agent,
            crate::session::selection_filters::SelectionFilters {
                point_entities: false,
                ..Default::default()
            },
        )
        .unwrap();
    assert_ne!(
        session.pick_for_selection(&ray).map(|hit| hit.object),
        Some(ObjectId::Entity(id))
    );
    assert_eq!(session.pick(&ray), raw);
    assert_eq!(session.instance_contents_hits(&ray, None).len(), 1);
    session
        .set_selection_filters(Actor::User, Default::default())
        .unwrap();
    session.set_face_mode(true);
    session.preflight_instance_contents();
    assert!(session.instance_contents_preview(true).mesh.is_none());
    assert!(session.instance_contents_hits(&ray, None).is_empty());
    assert_eq!(
        session.inspect_instance_contents(0, 64).unwrap()["status"],
        "context_suppressed"
    );
    session.set_face_mode(false);
    let before = session.instance_contents_serial();
    assert!(session.preflight_instance_contents());
    assert!(session.instance_contents_serial() > before);
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
        "world { id 1 classname worldspawn } entity {id 2 classname info_null}",
    )
    .unwrap();
    let failed = refresh(&mut session, Actor::Agent);
    assert_eq!(failed["state"], "failed");
    assert!(failed["error"].as_str().unwrap().contains("unsupported"));
    assert!(session.inspect_instance_contents(0, 65).is_err());
}

#[test]
fn large_no_reference_maps_return_empty_without_clone_or_filesystem_admission() {
    use source_formats::vmf::Entity;
    for dimension in ["objects", "sides"] {
        let mut vmf = Document::empty().into_vmf();
        if dimension == "objects" {
            for index in 0..instances::MAX_OBJECTS {
                vmf.entities
                    .push(Entity::new(index as u32 + 2, "info_target"));
            }
        } else {
            let template = crate::test_assets::room().world.solids.remove(0);
            for index in 0..=instances::MAX_SIDES / 6 {
                let mut solid = template.clone();
                solid.id = index as u32 + 2;
                for (side_index, side) in solid.sides.iter_mut().enumerate() {
                    side.id = (index * 6 + side_index + 1) as u32;
                }
                vmf.world.solids.push(solid);
            }
        }
        vmf.validate_ids().unwrap();
        let mut hidden = Entity::new(vmf.object_ids().max().unwrap() + 1, "func_instance");
        hidden.hidden = true;
        hidden.properties.set("file", "../missing.vmf");
        vmf.entities.push(hidden);
        let mut session = Session::new(Err("no assets".into()));
        session.document = Document::try_new(vmf).unwrap();
        session.path = Some(PathBuf::from(
            "/nonexistent-instance-preview-parent/missing.vmf",
        ));
        let source = session.document.vmf().clone();
        let request = session
            .begin_instance_contents_refresh(Actor::Agent)
            .unwrap();
        assert!(request.vmf.is_none() && request.location.is_none());
        let report = session
            .finish_instance_contents_refresh(Actor::Agent, request.run())
            .unwrap();
        assert_eq!(report["state"], "empty", "{dimension}");
        assert_eq!(report["stats"]["instances"], 0);
        assert_eq!(session.document.vmf(), &source);
        let id = session.document.vmf().object_ids().max().unwrap() + 1;
        session
            .document
            .edit("unsupported parameter helper", |tx| {
                tx.add_entity(Entity::new(id, "FuNc_InStAnCe_PaRmS"))?;
                Ok(())
            })
            .unwrap();
        assert!(
            session
                .begin_instance_contents_refresh(Actor::Agent)
                .is_err()
        );
    }
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
fn derived_layer_changes_preserve_fresh_owned_pivot_picks_and_genuine_stale_guards() {
    for actor in [Actor::User, Actor::Agent] {
        for initially_enabled in [false, true] {
            let (_directory, mut session, _source, child, _owner) = opened(actor);
            refresh(&mut session, actor);
            let floor = ObjectId::Solid(session.document.vmf().world.solids[0].id);
            session.select(&[floor], SelectMode::Replace).unwrap();
            session.set_view(render::ViewOptions {
                instance_contents: initially_enabled,
                ..session.view()
            });
            let report = session
                .begin_transform_pivot_pick(
                    actor,
                    session.camera(),
                    (511, 511),
                    &session.transform_pivot_pick_stamp(),
                )
                .unwrap();
            let token = report.picking.unwrap().token;
            let serials = (session.scene_serial(), session.view_serial());
            let render_key = session.render_scene_key(session.view());
            session.preflight_instance_contents();
            assert_ne!(session.render_scene_key(session.view()), render_key);
            session.build_scene();
            assert_eq!((session.scene_serial(), session.view_serial()), serials);
            session.set_view(render::ViewOptions {
                instance_contents: !initially_enabled,
                ..session.view()
            });
            assert_eq!((session.scene_serial(), session.view_serial()), serials);
            let preview_token = session.inspect_instance_contents(0, 64).unwrap()["token"]
                .as_u64()
                .unwrap();
            session
                .clear_instance_contents(actor, preview_token)
                .unwrap();
            refresh(&mut session, actor);
            std::fs::write(&child, b"changed child source").unwrap();
            session.preflight_instance_contents();
            session.build_scene();
            assert_eq!((session.scene_serial(), session.view_serial()), serials);
            assert!(session.transform_pivot_pick_state().picking.unwrap().valid);
            let ray = Ray {
                origin: DVec3::new(100.0, 100.0, 96.0),
                direction: DVec3::NEG_Z,
            };
            let report = session.pick_transform_pivot_ray(actor, token, ray).unwrap();
            assert_eq!(report.last_pick.unwrap().point, [100.0, 100.0, 0.0]);

            for changed in ["camera", "view", "source"] {
                let token = session
                    .begin_transform_pivot_pick(
                        actor,
                        session.camera(),
                        (511, 511),
                        &session.transform_pivot_pick_stamp(),
                    )
                    .unwrap()
                    .picking
                    .unwrap()
                    .token;
                match changed {
                    "camera" => {
                        let camera = render::Camera::looking_at(
                            glam::Vec3::new(180.0, -200.0, 160.0),
                            glam::Vec3::ZERO,
                        );
                        session.set_camera(camera);
                    }
                    "view" => session.set_view(render::ViewOptions {
                        tool_textures: !session.view().tool_textures,
                        ..session.view()
                    }),
                    "source" => {
                        session
                            .edit(actor, "genuine root change", |tx| {
                                tx.world_properties_mut().set("message", "changed");
                                Ok(())
                            })
                            .unwrap();
                    }
                    _ => unreachable!(),
                }
                assert!(
                    session.pick_transform_pivot_ray(actor, token, ray).is_err(),
                    "{changed}"
                );
                let _ = session.cancel_transform_pivot_pick(actor, token);
            }
        }
    }
}

#[test]
fn measurement_endpoint_picking_suppresses_contents_without_expiring_its_context() {
    use crate::session::{Pointer, measurements};
    use glam::{DVec2, Vec3};
    for actor in [Actor::User, Actor::Agent] {
        let (_directory, mut session, _source, _child, owner) = opened(actor);
        refresh(&mut session, actor);
        session.set_camera(render::Camera::looking_at(
            Vec3::new(8.0, 8.0, 96.0),
            Vec3::new(8.0, 8.0, 0.0),
        ));
        let ray = Ray {
            origin: DVec3::new(8.0, 8.0, 96.0),
            direction: DVec3::NEG_Z,
        };
        assert_eq!(session.instance_contents_hits(&ray, None)[0].object, owner);
        let source = session.document.vmf().clone();
        let history = session.history_snapshot();
        let state = session
            .preview_measurement(
                actor,
                measurements::Request {
                    id: None,
                    name: "Root floor".into(),
                    start: DVec3::ZERO,
                    end: DVec3::X,
                },
                &session.measurement_stamp(),
            )
            .unwrap();
        let state = session
            .begin_measurement_pick(
                actor,
                state["draft"]["token"].as_u64().unwrap(),
                measurements::Endpoint::Start,
                session.camera(),
                (511, 511),
                &session.measurement_pick_stamp(),
            )
            .unwrap();
        let token = state["draft"]["token"].as_u64().unwrap();
        let serials = (session.scene_serial(), session.view_serial());
        session.preflight_instance_contents();
        session.build_scene();
        let preview = session.instance_contents_preview(true);
        assert!(preview.mesh.is_none());
        assert_eq!(
            preview.report["suppression_reason"],
            "active_editing_tool_or_draft"
        );
        assert!(session.instance_contents_hits(&ray, Some(true)).is_empty());
        assert_eq!((session.scene_serial(), session.view_serial()), serials);
        let state = session.measurement_state();
        assert_eq!(state["draft"]["token"], token);
        assert_eq!(state["draft"]["picking"]["valid"], true);
        let picked = session
            .pick_measurement_pixel(
                actor,
                token,
                Pointer {
                    position: DVec2::splat(255.0),
                    size: (511, 511),
                },
            )
            .unwrap();
        assert_eq!(
            picked["draft"]["last_pick"]["target"],
            format!("solid:{}", source.world.solids[0].id)
        );
        assert_eq!(picked["draft"]["last_pick"]["point"][2], 0.0);
        assert_eq!(session.document.vmf(), &source);
        assert_eq!(session.history_snapshot(), history);
        assert!(session.instance_contents_preview(true).mesh.is_some());
    }
}

#[test]
fn suppressed_reference_refresh_preserves_transform_and_terrain_owned_previews() {
    use glam::DVec2;
    use map_core::surface::FaceId;
    use render::handles::{Handle, Mode};
    for actor in [Actor::User, Actor::Agent] {
        let (_directory, mut session, _source, child, _owner) = opened(actor);
        let floor = ObjectId::Solid(session.document.vmf().world.solids[0].id);
        session.select(&[floor], SelectMode::Replace).unwrap();
        refresh(&mut session, actor);
        session
            .set_transform_tool(
                actor,
                super::super::TransformTool {
                    mode: Some(Mode::Move),
                    ..session.transform_tool
                },
            )
            .unwrap();
        session.begin_transform_drag(Handle::X, None).unwrap();
        session
            .update_transform_drag(None, Some(DVec2::new(16.0, 0.0)))
            .unwrap();
        let serials = (session.scene_serial(), session.view_serial());
        let preview = session.render_document().vmf().clone();
        let request = session.begin_instance_contents_refresh(actor).unwrap();
        session
            .finish_instance_contents_refresh(actor, request.run())
            .unwrap();
        session.set_view(render::ViewOptions {
            instance_contents: false,
            ..session.view()
        });
        session.preflight_instance_contents();
        session.build_scene();
        assert_eq!((session.scene_serial(), session.view_serial()), serials);
        assert!(session.transform_drag().is_some());
        assert_eq!(session.render_document().vmf(), &preview);
        session
            .update_transform_drag(None, Some(DVec2::new(32.0, 0.0)))
            .unwrap();
        session.cancel_transform_preview();

        let ObjectId::Solid(solid) = floor else {
            unreachable!()
        };
        let side = session
            .document
            .solid(solid)
            .unwrap()
            .sides
            .iter()
            .find(|side| map_core::Plane::from_points(side.plane.0).unwrap().normal.z > 0.99)
            .unwrap()
            .id;
        let face = FaceId { solid, side };
        session.create_terrain(actor, &[face], 2, 1).unwrap();
        session.select_faces(&[face], SelectMode::Replace).unwrap();
        let mask = session
            .highlight_terrain_vertices(actor, &[(face, vec![7, 12])])
            .unwrap();
        refresh(&mut session, actor);
        let token = session
            .enable_terrain_move_handles(actor, &[face], mask)
            .unwrap();
        session
            .begin_terrain_move_drag(token, Handle::Z, None)
            .unwrap();
        session
            .update_terrain_move_drag(actor, token, None, Some(DVec2::new(16.0, 0.0)))
            .unwrap();
        let serials = (session.scene_serial(), session.view_serial());
        let preview_token = session.face_preview_token();
        let preview = session.render_document().vmf().clone();
        refresh(&mut session, actor);
        std::fs::write(&child, b"changed child source").unwrap();
        session.preflight_instance_contents();
        session.build_scene();
        assert_eq!((session.scene_serial(), session.view_serial()), serials);
        assert!(session.check_terrain_move_token(token).is_ok());
        assert_eq!(session.face_preview_token(), preview_token);
        assert_eq!(session.render_document().vmf(), &preview);
        session
            .update_terrain_move_drag(actor, token, None, Some(DVec2::new(32.0, 0.0)))
            .unwrap();
    }
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

#[test]
fn temporary_visibility_excludes_cached_contents_without_reloading_or_losing_explicit_inspection() {
    let (_directory, mut session, _source, _child, owner) = opened(Actor::Agent);
    refresh(&mut session, Actor::Agent);
    let ObjectId::Entity(id) = owner else {
        unreachable!()
    };
    let cached = session.instance_contents.ready.clone().unwrap();
    assert!(session.instance_contents_preview(true).owners.contains(&id));
    session
        .set_editor_visibility(
            Actor::User,
            crate::session::editor_visibility::Action::Hide(vec![owner]),
            &session.working_region_stamp(),
        )
        .unwrap();
    session.preflight_instance_contents();
    assert!(!session.instance_contents_preview(true).owners.contains(&id));
    let report = session.inspect_instance_contents(0, 64).unwrap();
    assert!(
        report["summary"]
            .as_str()
            .unwrap()
            .contains("editor visibility")
    );
    assert!(Arc::ptr_eq(
        &cached,
        session.instance_contents.ready.as_ref().unwrap()
    ));
    assert!(session.document().contains(owner));
    session
        .set_editor_visibility(
            Actor::User,
            crate::session::editor_visibility::Action::ShowAll,
            &session.working_region_stamp(),
        )
        .unwrap();
    session.preflight_instance_contents();
    assert!(session.instance_contents_preview(true).owners.contains(&id));
    assert!(Arc::ptr_eq(
        &cached,
        session.instance_contents.ready.as_ref().unwrap()
    ));
}
