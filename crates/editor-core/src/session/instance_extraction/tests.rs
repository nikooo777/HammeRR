use super::*;

fn opened() -> (tempfile::TempDir, Session, PathBuf, Vec<ObjectId>) {
    let (directory, game, parent, selected) = crate::test_assets::instance_extraction_map();
    let mut session = Session::new(Ok(game));
    session.open(Actor::User, &parent, true).unwrap();
    session.select(&selected, SelectMode::Replace).unwrap();
    (directory, session, parent, selected)
}

fn prepared(session: &mut Session, actor: Actor, file: &str) -> u64 {
    let request = session.begin_instance_extraction(actor, file).unwrap();
    let token = request.token();
    let completed = request.run();
    let report = session
        .finish_instance_extraction(actor, completed)
        .unwrap();
    assert_eq!(report["ready"], true, "{report}");
    token
}

#[test]
fn instance_extraction_roundtrips_ids_uvs_and_world_detail_geometry_in_one_parent_undo() {
    for actor in [Actor::User, Actor::Agent] {
        let (directory, mut session, parent, selected) = opened();
        let original = session.document.vmf().clone();
        let camera = session.camera();
        let history = session.history_snapshot();
        let disk = std::fs::read(&parent).unwrap();
        let child = directory.path().join("export.vmf");
        let token = prepared(&mut session, actor, "export.vmf");
        assert!(!child.exists());
        assert_eq!(session.document.vmf(), &original);
        assert_eq!(session.history_snapshot(), history);
        let report = session.get_instance_extraction();
        assert_eq!(report["world_brushes"], 1);
        assert_eq!(report["detail_brushes"], 2);
        assert_eq!(report["sides"], 18);
        let work = session.publish_instance_extraction(actor, token).unwrap();
        let done = work.run();
        let result = session.finish_instance_extraction(actor, done).unwrap();
        assert_eq!(result["last"]["published"], true);
        assert_eq!(session.document.undo_labels().count(), 1);
        let exported = Vmf::parse(&std::fs::read_to_string(&child).unwrap()).unwrap();
        let world = original.world.solids.last().unwrap();
        assert_eq!(exported.world.solids, [world.clone()]);
        let detail = original
            .entities
            .iter()
            .find(|entity| entity.classname() == Some("func_detail"))
            .unwrap();
        assert_eq!(exported.entities, [detail.clone()]);
        for corner in [DVec3::new(32.0, 40.0, 8.0), DVec3::new(48.0, 56.0, 24.0)] {
            let uv = map_core::texel_uv(&exported.world.solids[0].sides[0], corner);
            assert_eq!(uv.x, corner.dot(DVec3::new(1.0, 0.5, 0.0)) / -0.25 + 7.5);
            assert_eq!(uv.y, corner.dot(DVec3::new(0.0, 1.0, 0.25)) / 0.5 - 3.25);
        }
        let location = SourceLocation::capture(&parent)
            .unwrap()
            .hydrate(&Control::default())
            .unwrap();
        let mut report = compiler::Report::default();
        let resolved = instances::resolve(
            session.document.vmf(),
            Some(&location),
            &Control::default(),
            &mut report,
        )
        .unwrap();
        let brushes = resolved.generated_brushes().collect::<Vec<_>>();
        assert_eq!(
            brushes
                .iter()
                .map(|brush| (brush.original, brush.detail))
                .collect::<Vec<_>>(),
            vec![
                (world.id, false),
                (detail.solids[0].id, true),
                (detail.solids[1].id, true)
            ]
        );
        let bounds = brushes
            .iter()
            .map(|brush| {
                let geometry = map_core::BrushGeometry::from_solid(brush.solid).unwrap();
                (geometry.mins, geometry.maxs)
            })
            .collect::<Vec<_>>();
        assert_eq!(
            bounds,
            vec![
                (DVec3::new(32.0, 40.0, 8.0), DVec3::new(48.0, 56.0, 24.0)),
                (DVec3::new(64.0, 40.0, 8.0), DVec3::new(72.0, 56.0, 24.0)),
                (DVec3::new(80.0, 40.0, 8.0), DVec3::new(88.0, 56.0, 24.0))
            ]
        );
        drop(resolved);
        assert_eq!(session.camera(), camera);
        assert_eq!(std::fs::read(&parent).unwrap(), disk);
        assert_eq!(
            std::fs::read(parent.with_extension("bsp")).unwrap(),
            b"prior BSP"
        );
        assert_eq!(
            std::fs::read(parent.with_extension("lin")).unwrap(),
            b"prior LIN"
        );
        session.undo(actor).unwrap();
        assert_eq!(session.document.vmf(), &original);
        assert!(child.exists());
        let bytes = std::fs::read(&child).unwrap();
        session.redo(actor).unwrap();
        assert_eq!(std::fs::read(child).unwrap(), bytes);
        assert!(selected.iter().all(|id| !session.document.contains(*id)));
    }
}

#[test]
fn instance_extraction_stale_cancel_races_and_existing_destination_preserve_parent() {
    let (directory, mut session, _parent, selected) = opened();
    let original = session.document.vmf().clone();
    let history = session.history_snapshot();
    let token = prepared(&mut session, Actor::Agent, "race.vmf");
    std::fs::write(directory.path().join("race.vmf"), b"other owner").unwrap();
    assert!(
        session
            .publish_instance_extraction(Actor::Agent, token)
            .is_err()
    );
    assert_eq!(session.document.vmf(), &original);
    assert_eq!(session.history_snapshot(), history);
    session
        .cancel_instance_extraction(Actor::Agent, token)
        .unwrap();
    let token = prepared(&mut session, Actor::User, "cancel.vmf");
    let request = session
        .publish_instance_extraction(Actor::User, token)
        .unwrap();
    session
        .cancel_instance_extraction(Actor::User, token)
        .unwrap();
    assert!(
        session
            .finish_instance_extraction(Actor::User, request.run())
            .is_err()
    );
    assert!(!directory.path().join("cancel.vmf").exists());
    let token = prepared(&mut session, Actor::User, "stale.vmf");
    let request = session
        .publish_instance_extraction(Actor::User, token)
        .unwrap();
    session.select(&[], SelectMode::Replace).unwrap();
    assert!(
        session
            .finish_instance_extraction(Actor::User, request.run())
            .is_err()
    );
    assert_eq!(session.document.vmf(), &original);
    assert!(!directory.path().join("stale.vmf").exists());
    session
        .cancel_instance_extraction(Actor::User, token)
        .unwrap();
    session.select(&selected, SelectMode::Replace).unwrap();
    let request = session
        .begin_instance_extraction(Actor::Agent, "late.vmf")
        .unwrap();
    let token = request.token();
    session
        .cancel_instance_extraction(Actor::Agent, token)
        .unwrap();
    assert!(
        session
            .begin_instance_extraction(Actor::User, "next.vmf")
            .is_err()
    );
    assert!(
        session
            .finish_instance_extraction(Actor::Agent, request.run())
            .is_err()
    );
    assert_eq!(
        std::fs::read_dir(directory.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry
                .file_name()
                .to_string_lossy()
                .starts_with(".hammerr-extract-"))
            .count(),
        0
    );
}

#[test]
fn instance_extraction_opaque_metadata_and_face_bindings_reject_before_staging() {
    use source_formats::keyvalues::Node;
    for kind in [
        "world",
        "views",
        "group",
        "layer",
        "binding",
        "note",
        "partition",
        "existing_instance",
        "not_suffix",
    ] {
        let (directory, mut session, _parent, _selected) = opened();
        let selected = session.document.vmf().world.solids.last().unwrap().clone();
        let mut vmf = session.document.vmf().clone();
        match kind {
            "world" => vmf.world.extra.push(Node::block(
                "binding",
                vec![Node::string("solid", selected.id.to_string())],
            )),
            "views" => {
                vmf.extra.push(Node::block(
                    "hammerr_saved_views",
                    vec![
                        Node::string("version", "1"),
                        Node::string("selected_solid", selected.id.to_string()),
                    ],
                ));
            }
            "group" => {
                let id = vmf.object_ids().max().unwrap() + 1;
                vmf.world.groups.push(source_formats::vmf::Group {
                    id,
                    editor: None,
                    extra_properties: Default::default(),
                    extra: Vec::new(),
                });
                vmf.world
                    .solids
                    .last_mut()
                    .unwrap()
                    .editor
                    .as_mut()
                    .unwrap()
                    .set("groupid", id.to_string());
            }
            "layer" => {
                vmf.visgroups = Some(vec![source_formats::vmf::VisGroup {
                    name: "Layer".into(),
                    id: 1,
                    color: source_formats::vmf::Color([10, 20, 30]),
                    children: Vec::new(),
                    extra_properties: Default::default(),
                    extra: Vec::new(),
                }]);
                vmf.world
                    .solids
                    .last_mut()
                    .unwrap()
                    .editor
                    .as_mut()
                    .unwrap()
                    .set("visgroupid", "1");
            }
            "binding" => {
                let mut entity = Entity::new(vmf.object_ids().max().unwrap() + 1, "env_cubemap");
                entity
                    .properties
                    .set("sides", selected.sides[0].id.to_string());
                vmf.entities.push(entity);
            }
            "note" => {
                let mut notes = map_core::context::Metadata::default();
                notes.notes.insert(
                    map_core::context::Scope::Object(ObjectId::Solid(selected.id)),
                    map_core::context::Note {
                        summary: "Keep".into(),
                        body: String::new(),
                        author: "human".into(),
                        observed: String::new(),
                        region: None,
                        extra: Vec::new(),
                    },
                );
                vmf.extra.push(notes.to_node());
            }
            "partition" => vmf.extra.push(Node::block(
                "hammerr_partitions",
                vec![Node::string("version", "unsupported")],
            )),
            "existing_instance" => {
                let mut helper = Entity::new(vmf.object_ids().max().unwrap() + 1, "func_instance");
                helper.properties.set("file", "other.vmf");
                vmf.entities.push(helper);
            }
            "not_suffix" => vmf.world.solids.swap(0, 5),
            _ => unreachable!(),
        }
        let ids = session.selected_ids();
        session.document = Document::new(vmf.clone());
        session.selection = ids.into_iter().collect();
        if kind == "not_suffix" {
            session.selection = BTreeSet::from([ObjectId::Solid(vmf.world.solids[0].id)]);
        }
        let history = session.history_snapshot();
        let error = match session.begin_instance_extraction(Actor::User, "blocked.vmf") {
            Ok(_) => panic!("unexpected admission: {kind}"),
            Err(error) => error,
        };
        let expected = match kind {
            "group" | "layer" => "Selected editor metadata",
            "note" => "attached notes",
            "binding" => "binds selected side",
            "existing_instance" => "existing visible instance",
            "not_suffix" => "visible world-brush suffix",
            "partition" => "version",
            _ => "extension metadata",
        };
        assert!(error.contains(expected), "{kind}: {error}");
        assert_eq!(session.document.vmf(), &vmf, "{kind}");
        assert_eq!(session.history_snapshot(), history);
        assert!(!directory.path().join("blocked.vmf").exists());
        assert!(session.instance_extraction.form.is_none());
    }
}

#[test]
fn instance_extraction_compiled_geometry_and_texinfo_match_unchanged_manual_control() {
    use source_formats::bsp::lump;
    let (_directory, mut session, parent, _ids) = opened();
    let options = compiler::Options {
        lighting: compiler::LightingMode::Fullbright,
        physics: compiler::PhysicsMode::None,
        ..Default::default()
    };
    let control = compiler::compile_with_source_controlled(
        session.document.vmf(),
        &session.game,
        options,
        None,
        None,
        &Control::default(),
    );
    assert!(!control.report.has_errors(), "{:?}", control.report);
    assert!(control.stats.instances.is_none());
    let before = control.bsp.unwrap();
    let token = prepared(&mut session, Actor::Agent, "compiled.vmf");
    let request = session
        .publish_instance_extraction(Actor::Agent, token)
        .unwrap();
    session
        .finish_instance_extraction(Actor::Agent, request.run())
        .unwrap();
    let source = SourceContext::capture(&parent, &Control::default()).unwrap();
    let actual = compiler::compile_with_source_controlled(
        session.document.vmf(),
        &session.game,
        options,
        None,
        Some(&source),
        &Control::default(),
    );
    assert!(!actual.report.has_errors(), "{:?}", actual.report);
    assert_eq!(actual.stats.instances.as_ref().unwrap().detail_brushes, 2);
    let after = actual.bsp.unwrap();
    for index in [
        lump::PLANES,
        lump::TEXDATA,
        lump::VERTEXES,
        lump::TEXINFO,
        lump::FACES,
        lump::EDGES,
        lump::SURFEDGES,
        lump::MODELS,
        lump::BRUSHES,
        lump::BRUSHSIDES,
    ] {
        assert_eq!(
            after.lump(index).data,
            before.lump(index).data,
            "compiled lump {index}"
        );
    }
}

#[test]
fn instance_extraction_finalizer_failure_restores_allocator_objects_history_and_stage() {
    let (directory, mut session, _parent, _ids) = opened();
    let original = session.document.vmf().clone();
    let history = session.history_snapshot();
    let selection = session.selection.clone();
    let camera = session.camera();
    let probe = |session: &mut Session| {
        let mut id = 0;
        let result: Result<(), EditError> = session.document.edit("allocation probe", |tx| {
            id = tx.new_object_id()?;
            Err(EditError::Invalid("probe rollback".into()))
        });
        assert!(result.is_err());
        id
    };
    let expected_id = probe(&mut session);
    let token = prepared(&mut session, Actor::User, "finalizer.vmf");
    let request = session
        .publish_instance_extraction(Actor::User, token)
        .unwrap();
    let completed = request.run();
    let staged = completed.result.as_ref().unwrap().stage.path().to_owned();
    std::fs::write(&staged, b"changed after exact worker verification").unwrap();
    let error = session
        .finish_instance_extraction(Actor::User, completed)
        .unwrap_err();
    assert!(error.contains("changed"), "{error}");
    assert_eq!(session.document.vmf(), &original);
    assert_eq!(session.history_snapshot(), history);
    assert_eq!(session.selection, selection);
    assert_eq!(session.camera(), camera);
    assert_eq!(probe(&mut session), expected_id);
    assert!(!directory.path().join("finalizer.vmf").exists());
    assert!(!staged.exists());
}

#[test]
fn instance_extraction_directory_rename_cleanup_preserves_replacements_and_parent() {
    for ancestor in [false, true] {
        let (directory, mut session, _parent, _ids) = opened();
        let branch = directory.path().join("modules");
        let leaf = if ancestor {
            branch.join("nested")
        } else {
            branch.clone()
        };
        std::fs::create_dir_all(&leaf).unwrap();
        let relative = if ancestor {
            "modules/nested/new.vmf"
        } else {
            "modules/new.vmf"
        };
        let original = session.document.vmf().clone();
        let history = session.history_snapshot();
        let token = prepared(&mut session, Actor::Agent, relative);
        let basename = session
            .instance_extraction
            .form
            .as_ref()
            .unwrap()
            .ready
            .as_ref()
            .unwrap()
            .stage
            .path()
            .file_name()
            .unwrap()
            .to_owned();
        let moved = directory.path().join("moved-modules");
        std::fs::rename(&branch, &moved).unwrap();
        std::fs::create_dir_all(&leaf).unwrap();
        let replacement = leaf.join(&basename);
        std::fs::write(&replacement, b"unrelated replacement").unwrap();
        assert!(
            session
                .publish_instance_extraction(Actor::Agent, token)
                .is_err()
        );
        let moved_leaf = if ancestor {
            moved.join("nested")
        } else {
            moved
        };
        assert!(!moved_leaf.join(&basename).exists());
        assert_eq!(
            std::fs::read(replacement).unwrap(),
            b"unrelated replacement"
        );
        assert!(!leaf.join("new.vmf").exists());
        assert_eq!(session.document.vmf(), &original);
        assert_eq!(session.history_snapshot(), history);
    }
}

#[test]
fn instance_extraction_completed_worker_retains_lease_until_result_and_stage_retire() {
    let (directory, mut session, _parent, _ids) = opened();
    let request = session
        .begin_instance_extraction(Actor::Agent, "pending.vmf")
        .unwrap();
    let token = request.token();
    let completed = request.run();
    let stage = completed.result.as_ref().unwrap().stage.path().to_owned();
    assert!(stage.exists());
    session
        .cancel_instance_extraction(Actor::Agent, token)
        .unwrap();
    assert!(
        session
            .begin_instance_extraction(Actor::User, "replacement.vmf")
            .is_err()
    );
    drop(completed);
    assert!(!stage.exists());
    let request = session
        .begin_instance_extraction(Actor::User, "replacement.vmf")
        .unwrap();
    let token = request.token();
    drop(request);
    session
        .cancel_instance_extraction(Actor::User, token)
        .unwrap();
    assert!(!directory.path().join("pending.vmf").exists());
}

#[test]
fn instance_extraction_stale_ready_retires_stage_and_replaced_stage_is_preserved_with_diagnostic() {
    let (_directory, mut session, _parent, _ids) = opened();
    let token = prepared(&mut session, Actor::User, "late-edit.vmf");
    let stage = session
        .instance_extraction
        .form
        .as_ref()
        .unwrap()
        .ready
        .as_ref()
        .unwrap()
        .stage
        .path()
        .to_owned();
    session
        .edit(Actor::Agent, "other actor edit", |tx| {
            tx.world_properties_mut().set("message", "retained");
            Ok(())
        })
        .unwrap();
    let changed = session.document.vmf().clone();
    let history = session.history_snapshot();
    assert!(
        session
            .publish_instance_extraction(Actor::User, token)
            .is_err()
    );
    assert!(!stage.exists());
    assert_eq!(session.document.vmf(), &changed);
    assert_eq!(session.history_snapshot(), history);
    session
        .cancel_instance_extraction(Actor::User, token)
        .unwrap();
    let token = prepared(&mut session, Actor::User, "replacement-test.vmf");
    let stage = session
        .instance_extraction
        .form
        .as_ref()
        .unwrap()
        .ready
        .as_ref()
        .unwrap()
        .stage
        .path()
        .to_owned();
    std::fs::remove_file(&stage).unwrap();
    std::fs::write(&stage, b"unrelated leaf replacement").unwrap();
    let report = session
        .cancel_instance_extraction(Actor::User, token)
        .unwrap();
    assert!(
        report["cleanup_error"]
            .as_str()
            .unwrap()
            .contains("unrelated replacement preserved")
    );
    assert_eq!(
        std::fs::read(&stage).unwrap(),
        b"unrelated leaf replacement"
    );
}

#[test]
fn instance_extraction_generated_id_exhaustion_and_reserved_headroom_roll_back_before_publish() {
    for reserved in [false, true] {
        let (directory, mut session, _parent, _ids) = opened();
        let mut vmf = session.document.vmf().clone();
        let high = if reserved { u32::MAX - 3 } else { u32::MAX - 1 };
        vmf.world.solids.last_mut().unwrap().id = high;
        if reserved {
            let mut notes = map_core::context::Metadata::default();
            notes.notes.insert(
                map_core::context::Scope::Object(ObjectId::Entity(u32::MAX - 1)),
                map_core::context::Note {
                    summary: "Reserved unavailable attachment".into(),
                    body: String::new(),
                    author: "human".into(),
                    observed: String::new(),
                    region: None,
                    extra: Vec::new(),
                },
            );
            vmf.extra.push(notes.to_node());
            use source_formats::keyvalues::Node;
            vmf.extra.push(Node::block(
                "hammerr_measurements",
                vec![
                    Node::string("version", "1"),
                    Node::block(
                        "measurement",
                        vec![
                            Node::string("id", "1"),
                            Node::string("name_base64", "UmVzZXJ2ZWQ="),
                            Node::string("start", "0 0 0"),
                            Node::string("end", "8 0 0"),
                            Node::string("start_entity_origin", u32::MAX.to_string()),
                        ],
                    ),
                ],
            ));
        }
        session.document = Document::new(vmf.clone());
        session.selection = BTreeSet::from([ObjectId::Solid(high)]);
        let token = prepared(&mut session, Actor::User, "id-exhaustion.vmf");
        let history = session.history_snapshot();
        let request = session
            .publish_instance_extraction(Actor::User, token)
            .unwrap();
        let error = session
            .finish_instance_extraction(Actor::User, request.run())
            .unwrap_err();
        assert!(error.contains("IDs are exhausted"), "{reserved}: {error}");
        assert_eq!(session.document.vmf(), &vmf);
        assert_eq!(session.history_snapshot(), history);
        assert!(!directory.path().join("id-exhaustion.vmf").exists());
    }
}

#[test]
fn instance_extraction_unavailable_inbound_side_cannot_become_generated_child_alias() {
    let (_directory, mut session, _parent, ids) = opened();
    let ObjectId::Solid(world) = ids[0] else {
        unreachable!()
    };
    let mut vmf = session.document.vmf().clone();
    let selected = vmf
        .world
        .solids
        .iter_mut()
        .find(|solid| solid.id == world)
        .unwrap();
    for (index, side) in selected.sides.iter_mut().enumerate() {
        side.id = 1000 + index as u32;
    }
    let mut detail_side = 2000;
    for entity in vmf
        .entities
        .iter_mut()
        .filter(|entity| entity.classname() == Some("func_detail"))
    {
        for side in entity.solids.iter_mut().flat_map(|solid| &mut solid.sides) {
            side.id = detail_side;
            detail_side += 1;
        }
    }
    let missing = vmf.side_ids().filter(|id| *id < 1000).max().unwrap() + 1;
    assert_eq!(missing, 37);
    let mut entity = Entity::new(vmf.object_ids().max().unwrap() + 1, "env_cubemap");
    entity.properties.set("sides", missing.to_string());
    vmf.entities.push(entity);
    session.document = Document::new(vmf.clone());
    let error = match session.begin_instance_extraction(Actor::User, "side-alias.vmf") {
        Ok(_) => panic!("unavailable side admitted"),
        Err(error) => error,
    };
    assert!(error.contains("unavailable root side"), "{error}");
    assert_eq!(session.document.vmf(), &vmf);
}

#[test]
fn instance_extraction_foreign_completed_request_cannot_cross_equivalent_sessions() {
    let (_directory, mut a, parent, ids) = opened();
    let mut b = Session::new(Ok(source_formats::gamefs::GameFs {
        game_dir: a.game.game_dir.clone(),
        base_dir: a.game.base_dir.clone(),
        mounts: Vec::new(),
    }));
    b.open(Actor::User, &parent, true).unwrap();
    b.select(&ids, SelectMode::Replace).unwrap();
    let a_request = a.begin_instance_extraction(Actor::User, "a.vmf").unwrap();
    let a_token = a_request.token();
    let a_completed = a_request.run();
    let b_request = b.begin_instance_extraction(Actor::User, "b.vmf").unwrap();
    let b_token = b_request.token();
    let b_completed = b_request.run();
    let b_stage = b_completed.result.as_ref().unwrap().stage.path().to_owned();
    assert!(
        b.finish_instance_extraction(Actor::User, a_completed)
            .is_err()
    );
    assert!(b_stage.exists());
    assert_eq!(a.document.vmf(), b.document.vmf());
    assert!(
        b.finish_instance_extraction(Actor::User, b_completed)
            .unwrap()["ready"]
            == true
    );
    a.cancel_instance_extraction(Actor::User, a_token).unwrap();
    b.cancel_instance_extraction(Actor::User, b_token).unwrap();
    assert!(!parent.with_file_name("a.vmf").exists());
    assert!(!parent.with_file_name("b.vmf").exists());
}

#[test]
fn instance_extraction_identical_unowned_stage_replacement_cannot_publish() {
    let directory = tempfile::tempdir().unwrap();
    let cleanup = staging::Cleanup::default();
    let mut stage = Stage::create(directory.path(), cleanup.clone()).unwrap();
    stage.write(b"same serialized child").unwrap();
    let path = stage.path().to_owned();
    std::fs::remove_file(&path).unwrap();
    std::fs::write(&path, b"same serialized child").unwrap();
    assert!(stage.verify_owned().is_err());
    let destination = directory.path().join("child.vmf");
    assert!(stage.publish(&destination).is_err());
    assert!(!destination.exists());
    assert_eq!(std::fs::read(path).unwrap(), b"same serialized child");
    assert!(
        cleanup
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .contains("unrelated replacement preserved")
    );
}

#[test]
fn instance_extraction_stage_directory_fd_cannot_publish_to_a_different_guarded_parent() {
    let directory = tempfile::tempdir().unwrap();
    let a = directory.path().join("a");
    let b = directory.path().join("b");
    std::fs::create_dir(&a).unwrap();
    std::fs::create_dir(&b).unwrap();
    let mut stage = Stage::create(&a, Default::default()).unwrap();
    stage.write(b"owned bytes").unwrap();
    let old = stage.path().to_owned();
    let alias = b.join(old.file_name().unwrap());
    std::fs::hard_link(&old, &alias).unwrap();
    assert!(stage.publish(&b.join("new.vmf")).is_err());
    assert!(!old.exists());
    assert!(!b.join("new.vmf").exists());
    assert_eq!(std::fs::read(alias).unwrap(), b"owned bytes");
}

#[test]
fn instance_extraction_generated_side_id_exhaustion_preserves_source_and_absence() {
    let (directory, mut session, _parent, _ids) = opened();
    let mut vmf = session.document.vmf().clone();
    vmf.world.solids[0].sides[0].id = u32::MAX;
    session.document = Document::new(vmf.clone());
    let token = prepared(&mut session, Actor::User, "side-exhaustion.vmf");
    let history = session.history_snapshot();
    let request = session
        .publish_instance_extraction(Actor::User, token)
        .unwrap();
    let error = session
        .finish_instance_extraction(Actor::User, request.run())
        .unwrap_err();
    assert!(error.contains("side IDs are exhausted"), "{error}");
    assert_eq!(session.document.vmf(), &vmf);
    assert_eq!(session.history_snapshot(), history);
    assert!(!directory.path().join("side-exhaustion.vmf").exists());
}
